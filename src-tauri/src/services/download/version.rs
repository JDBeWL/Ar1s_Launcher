//! 版本资源整合与下载
//!
//! 跟随 inheritsFrom 递归补基础版本，汇总成下载任务并按镜像改写地址

use super::batch::download_all_files;
use super::http::get_http_client;
use crate::errors::LauncherError;
use crate::models::{DownloadJob, VersionManifest};
use crate::services::config::{self, load_config};
use crate::utils::file_utils;
use log::{info, warn};
use std::fs;
use std::path::{Path, PathBuf};
use tauri::{Emitter, Window};

/// 只补这份版本 JSON 自己声明的库（不递归父版本），供加载器装完后补齐
///
/// Fabric/Quilt 的「安装」只写一份 profile、库文件没人下；Forge/NeoForge 已下过自己那批，这里按在位跳过
pub async fn ensure_declared_libraries(
    version_json: &serde_json::Value,
    window: &Window,
) -> Result<(), LauncherError> {
    let config = load_config()?;
    let libraries_base_dir = PathBuf::from(&config.game_dir).join("libraries");
    let is_mirror = config.download_mirror.is_some();
    let base_url = if is_mirror {
        "https://bmclapi2.bangbang93.com"
    } else {
        "https://launchermeta.mojang.com"
    };

    let mut downloads = Vec::new();
    collect_libraries(
        version_json,
        &libraries_base_dir,
        is_mirror,
        base_url,
        &mut downloads,
    )?;

    // 按坐标拼出来的任务不看 downloads.artifact，这里统一按“文件已在位”再筛一次
    downloads.retain(|job| !job.path.exists());

    if downloads.is_empty() {
        return Ok(());
    }

    info!("补下载加载器库文件: {} 个", downloads.len());
    download_all_files(downloads, window).await
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum VersionDownloadScope {
    Full,
    Core,
}

pub async fn process_and_download_version(
    version_id: String,
    mirror: Option<String>,
    window: &Window,
) -> Result<(), LauncherError> {
    download_version(version_id, mirror, window, VersionDownloadScope::Full).await
}

/// 只准备加载器安装和启动校验需要的原版核心：版本 JSON + client jar
pub async fn ensure_version_core(
    version_id: String,
    mirror: Option<String>,
    window: &Window,
) -> Result<(), LauncherError> {
    download_version(version_id, mirror, window, VersionDownloadScope::Core).await
}

async fn download_version(
    version_id: String,
    mirror: Option<String>,
    window: &Window,
    scope: VersionDownloadScope,
) -> Result<(), LauncherError> {
    let is_mirror = mirror.is_some();
    let base_url = if is_mirror {
        "https://bmclapi2.bangbang93.com"
    } else {
        "https://launchermeta.mojang.com"
    };

    let config = load_config()?;
    let game_dir = PathBuf::from(&config.game_dir);
    let version_dir = file_utils::version_dir(&game_dir.join("versions"), &version_id)?;

    // 记录目录本次调用前是否已存在，供失败清理判断
    let dir_existed_before = version_dir.exists();

    fs::create_dir_all(&version_dir)?;
    let libraries_base_dir = game_dir.join("libraries");
    let assets_base_dir = game_dir.join("assets");

    let client = get_http_client();

    // 整合包/mod 加载器版本：本地 JSON 存在且有 inheritsFrom
    let local_version_json_path = version_dir.join(format!("{}.json", version_id));
    let (actual_version_id, version_json, text) = if local_version_json_path.exists() {
        let local_text = fs::read_to_string(&local_version_json_path)?;
        let local_json: serde_json::Value = serde_json::from_str(&local_text)
            .map_err(|e| LauncherError::Custom(format!("解析本地版本JSON失败: {}", e)))?;

        if let Some(inherits_from) = local_json["inheritsFrom"].as_str() {
            file_utils::validate_version_id_or_error(inherits_from)?;
            info!("版本 {} 继承自 {}", version_id, inherits_from);
            Box::pin(download_version(
                inherits_from.to_string(),
                mirror.clone(),
                window,
                scope,
            ))
            .await?;

            if scope == VersionDownloadScope::Full {
                // 基础版本已下载，整合包的库文件另行处理
                return download_modpack_libraries(
                    &local_json,
                    &libraries_base_dir,
                    is_mirror,
                    base_url,
                    window,
                )
                .await;
            }

            return Ok(());
        }

        (version_id.clone(), local_json, local_text)
    } else {
        let manifest: VersionManifest = client
            .get(format!("{}/mc/game/version_manifest.json", base_url))
            .send()
            .await?
            .json()
            .await?;

        let version = manifest
            .versions
            .iter()
            .find(|v| v.id == version_id)
            .ok_or_else(|| LauncherError::Custom(format!("版本 {} 不存在", version_id)))?;

        let version_json_url = if is_mirror {
            version
                .url
                .replace("https://launchermeta.mojang.com", base_url)
                .replace("https://piston-meta.mojang.com", base_url)
        } else {
            version.url.clone()
        };

        let text = client.get(&version_json_url).send().await?.text().await?;
        let version_json: serde_json::Value = serde_json::from_str(&text)
            .or_else(|_| serde_json::from_str(text.trim_start_matches('\u{feff}')))
            .map_err(|_| LauncherError::Custom(format!("无法解析版本JSON for {}", version_id)))?;

        (version_id.clone(), version_json, text)
    };

    let mut downloads = Vec::new();

    collect_client_jar(
        &version_json,
        &version_dir,
        &actual_version_id,
        is_mirror,
        base_url,
        &mut downloads,
    )?;

    if scope == VersionDownloadScope::Full {
        collect_assets(
            &client,
            &version_json,
            &assets_base_dir,
            is_mirror,
            base_url,
            &mut downloads,
        )
        .await?;

        collect_libraries(
            &version_json,
            &libraries_base_dir,
            is_mirror,
            base_url,
            &mut downloads,
        )?;
    }

    match download_all_files(downloads, window).await {
        Ok(_) => {
            let version_json_path = version_dir.join(format!("{}.json", actual_version_id));
            fs::write(version_json_path, text)?;

            if let Ok(game_dir_str) = config::get_game_dir() {
                let _ = window.emit("game-dir-changed", &game_dir_str);
            }

            Ok(())
        }
        Err(e) => {
            // 仅在本次创建的全新目录时才清理，避免删除用户已有的版本数据（mod、存档等）
            if !dir_existed_before && version_dir.exists() {
                warn!("下载失败，清理新建的版本文件夹: {}", version_dir.display());
                if let Err(cleanup_err) = fs::remove_dir_all(&version_dir) {
                    warn!("清理版本文件夹失败: {}", cleanup_err);
                }
            }
            Err(e)
        }
    }
}

async fn download_modpack_libraries(
    version_json: &serde_json::Value,
    libraries_base_dir: &Path,
    is_mirror: bool,
    base_url: &str,
    window: &Window,
) -> Result<(), LauncherError> {
    let mut downloads = Vec::new();

    collect_libraries(
        version_json,
        libraries_base_dir,
        is_mirror,
        base_url,
        &mut downloads,
    )?;

    if downloads.is_empty() {
        return Ok(());
    }

    info!("下载整合包库文件: {} 个", downloads.len());

    download_all_files(downloads, window).await
}

fn collect_client_jar(
    version_json: &serde_json::Value,
    version_dir: &Path,
    version_id: &str,
    is_mirror: bool,
    base_url: &str,
    downloads: &mut Vec<DownloadJob>,
) -> Result<(), LauncherError> {
    let client_info = &version_json["downloads"]["client"];
    let client_url = client_info["url"]
        .as_str()
        .ok_or_else(|| LauncherError::Custom("无法获取客户端下载URL".to_string()))?;
    let client_size = client_info["size"].as_u64().unwrap_or(0);
    let client_hash = client_info["sha1"].as_str().map(|h| h.to_string());
    let client_jar_path = version_dir.join(format!("{}.jar", version_id));

    downloads.push(DownloadJob {
        url: if is_mirror {
            client_url
                .replace("https://launcher.mojang.com", base_url)
                .replace("https://piston-data.mojang.com", base_url)
        } else {
            client_url.to_string()
        },
        fallback_url: if is_mirror {
            Some(client_url.to_string())
        } else {
            None
        },
        path: client_jar_path,
        size: client_size,
        hash: client_hash,
    });

    Ok(())
}

async fn collect_assets(
    client: &reqwest::Client,
    version_json: &serde_json::Value,
    assets_base_dir: &Path,
    is_mirror: bool,
    base_url: &str,
    downloads: &mut Vec<DownloadJob>,
) -> Result<(), LauncherError> {
    let assets_index_id = version_json["assetIndex"]["id"]
        .as_str()
        .ok_or_else(|| LauncherError::Custom("无法获取资源索引ID".to_string()))?;
    let assets_index_url = version_json["assetIndex"]["url"]
        .as_str()
        .ok_or_else(|| LauncherError::Custom("无法获取资源索引URL".to_string()))?;

    let assets_index_url = if is_mirror {
        assets_index_url
            .replace("https://launchermeta.mojang.com", base_url)
            .replace("https://piston-meta.mojang.com", base_url)
    } else {
        assets_index_url.to_string()
    };

    let assets_index_path = assets_base_dir
        .join("indexes")
        .join(format!("{}.json", assets_index_id));
    // 路径来自用户配置的 game_dir，不能假设一定有父目录
    let assets_index_dir = assets_index_path.parent().ok_or_else(|| {
        LauncherError::Custom(format!(
            "资源索引路径没有父目录: {}",
            assets_index_path.display()
        ))
    })?;
    fs::create_dir_all(assets_index_dir)?;

    if !assets_index_path.exists() {
        let response = client.get(&assets_index_url).send().await?;
        let bytes = response.bytes().await?;
        fs::write(&assets_index_path, &bytes)?;
    }

    // 索引动辄十几 MB、几千个对象：读盘 + 解析 + 建任务是纯 CPU 活，留在 async worker 上会拖住其他命令
    let objects_dir = assets_base_dir.join("objects");
    let index_path = assets_index_path.clone();
    let is_mirror_copy = is_mirror;
    let base_url = base_url.to_string();

    let jobs = tokio::task::spawn_blocking(move || -> Result<Vec<DownloadJob>, LauncherError> {
        let index_content = fs::read_to_string(&index_path)?;
        let index: serde_json::Value = serde_json::from_str(&index_content)?;
        let mut jobs = Vec::new();
        collect_asset_objects(&index, &objects_dir, &base_url, is_mirror_copy, &mut jobs)?;
        Ok(jobs)
    })
    .await
    .map_err(|e| LauncherError::Custom(format!("解析资源索引失败: {}", e)))??;

    downloads.extend(jobs);

    Ok(())
}

/// 把资源索引里的对象展开成下载任务
fn collect_asset_objects(
    index: &serde_json::Value,
    objects_dir: &Path,
    base_url: &str,
    is_mirror: bool,
    downloads: &mut Vec<DownloadJob>,
) -> Result<(), LauncherError> {
    let Some(objects) = index["objects"].as_object() else {
        return Ok(());
    };

    for (_path, obj) in objects {
        let hash = obj["hash"]
            .as_str()
            .ok_or_else(|| LauncherError::Custom("资源缺少hash".to_string()))?;
        // 官方索引里的 hash 都是完整的 sha1；短成这样说明数据坏了，跳过而不是在切片上 panic
        let Some(prefix) = hash.get(..2) else {
            warn!("资源索引里有异常的 hash，已跳过: {}", hash);
            continue;
        };
        let size = obj["size"].as_u64().unwrap_or(0);
        let original_url = format!(
            "https://resources.download.minecraft.net/{}/{}",
            prefix, hash
        );
        let download_url = if is_mirror {
            format!("{}/assets/{}/{}", base_url, prefix, hash)
        } else {
            original_url.clone()
        };
        let file_path = objects_dir.join(prefix).join(hash);

        downloads.push(DownloadJob {
            url: download_url,
            fallback_url: if is_mirror { Some(original_url) } else { None },
            path: file_path,
            size,
            hash: Some(hash.to_string()),
        });
    }

    Ok(())
}

fn collect_libraries(
    version_json: &serde_json::Value,
    libraries_base_dir: &Path,
    is_mirror: bool,
    base_url: &str,
    downloads: &mut Vec<DownloadJob>,
) -> Result<(), LauncherError> {
    fs::create_dir_all(libraries_base_dir)?;

    for file in crate::utils::library::resolve_library_files(version_json) {
        let target_path = libraries_base_dir.join(&file.relative_path);

        // maven 回退路径才按“文件已在位”跳过；官方 artifact 的续传/校验交给下载状态
        if file.from_maven && target_path.exists() {
            continue;
        }

        let Some((url, fallback_url)) = library_download_urls(&file, is_mirror, base_url) else {
            continue;
        };

        downloads.push(DownloadJob {
            url,
            fallback_url,
            path: target_path,
            size: file.size,
            hash: file.sha1,
        });
    }

    Ok(())
}

fn library_download_urls(
    file: &crate::utils::library::LibraryFile,
    is_mirror: bool,
    base_url: &str,
) -> Option<(String, Option<String>)> {
    if file.from_maven {
        let official = file.url.clone()?;
        if is_mirror {
            let kind = if file.is_native { "libraries" } else { "maven" };
            return Some((
                format!("{}/{}/{}", base_url, kind, file.relative_path),
                Some(official),
            ));
        }
        return Some((
            official,
            if file.is_native {
                None
            } else {
                Some(format!(
                    "https://bmclapi2.bangbang93.com/maven/{}",
                    file.relative_path
                ))
            },
        ));
    }

    let official = file.url.clone()?;
    if is_mirror {
        let mirrored = official
            .replace(
                "https://libraries.minecraft.net",
                &format!("{}/libraries", base_url),
            )
            .replace(
                "https://maven.minecraftforge.net",
                &format!("{}/maven", base_url),
            )
            .replace(
                "https://maven.neoforged.net/releases",
                &format!("{}/maven", base_url),
            );
        Some((mirrored, Some(official)))
    } else {
        Some((official, None))
    }
}

#[cfg(test)]
mod tests {
    use super::{collect_asset_objects, collect_libraries};
    use crate::utils::minecraft::get_mc_os_name;
    use serde_json::json;

    #[test]
    fn test_asset_objects_become_download_jobs() {
        let dir = tempfile::tempdir().unwrap();
        let objects = dir.path().join("objects");
        let index = json!({
            "objects": {
                "minecraft/sounds/a.ogg": {
                    "hash": "0123456789012345678901234567890123456789",
                    "size": 1234
                }
            }
        });

        let mut jobs = Vec::new();
        collect_asset_objects(
            &index,
            &objects,
            "https://bmclapi2.bangbang93.com",
            true,
            &mut jobs,
        )
        .unwrap();

        assert_eq!(jobs.len(), 1);
        // 镜像优先、官方源兜底，路径按 hash 前两位分桶
        assert_eq!(
            jobs[0].url,
            "https://bmclapi2.bangbang93.com/assets/01/0123456789012345678901234567890123456789"
        );
        assert_eq!(
            jobs[0].fallback_url.as_deref(),
            Some(
                "https://resources.download.minecraft.net/01/0123456789012345678901234567890123456789"
            )
        );
        assert!(
            jobs[0]
                .path
                .ends_with("01/0123456789012345678901234567890123456789")
        );
        assert_eq!(jobs[0].size, 1234);
    }

    #[test]
    fn test_asset_objects_skip_malformed_hash_instead_of_panicking() {
        let dir = tempfile::tempdir().unwrap();
        let index = json!({ "objects": { "a": { "hash": "x", "size": 1 } } });

        let mut jobs = Vec::new();
        collect_asset_objects(&index, dir.path(), "https://example.com", false, &mut jobs).unwrap();

        assert!(jobs.is_empty(), "短 hash 应该被跳过，不能切片 panic");
    }

    fn current_os() -> &'static str {
        get_mc_os_name()
    }

    fn native_classifier() -> String {
        format!("natives-{}", current_os())
    }

    fn run(
        version: &serde_json::Value,
        mirror: bool,
    ) -> (Vec<crate::models::DownloadJob>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let libraries = dir.path().join("libraries");
        let mut jobs = Vec::new();
        collect_libraries(
            version,
            &libraries,
            mirror,
            "https://bmclapi2.bangbang93.com",
            &mut jobs,
        )
        .unwrap();
        (jobs, dir)
    }

    #[test]
    fn test_collect_keeps_common_and_native_jars() {
        let classifier = native_classifier();
        let main = "com/example/native/1.0/native-1.0.jar";
        let native = format!("com/example/native/1.0/native-1.0-{classifier}.jar");
        let version = json!({
            "libraries": [{
                "name": "com.example:native:1.0",
                "natives": { current_os(): classifier },
                "downloads": {
                    "artifact": { "path": main, "url": format!("https://libraries.minecraft.net/{main}") },
                    "classifiers": {
                        classifier: { "path": native, "url": format!("https://libraries.minecraft.net/{native}") }
                    }
                }
            }]
        });

        let (jobs, _dir) = run(&version, false);

        assert_eq!(jobs.len(), 2);
        assert!(jobs.iter().any(|j| j.path.ends_with(main)));
        assert!(jobs.iter().any(|j| j.path.ends_with(&native)));
    }

    #[test]
    fn test_classifiers_only_library_has_no_main_job() {
        let classifier = native_classifier();
        let native =
            format!("org/lwjgl/lwjgl/lwjgl-platform/2.9.2/lwjgl-platform-2.9.2-{classifier}.jar");
        let version = json!({
            "libraries": [{
                "name": "org.lwjgl.lwjgl:lwjgl-platform:2.9.2",
                "natives": { current_os(): classifier },
                "downloads": {
                    "classifiers": {
                        classifier: { "path": native, "url": format!("https://libraries.minecraft.net/{native}") }
                    }
                }
            }]
        });

        let (jobs, _dir) = run(&version, false);

        assert_eq!(jobs.len(), 1);
        assert!(jobs[0].url.ends_with(&native));
        assert!(
            !jobs[0].url.ends_with("lwjgl-platform-2.9.2.jar"),
            "只有 classifiers 的库不该被拼出主 jar：{}",
            jobs[0].url
        );
    }

    #[test]
    fn test_declared_url_uses_mirror_with_official_fallback() {
        let path = "com/example/tool/1.0/tool-1.0.jar";
        let official = format!("https://libraries.minecraft.net/{path}");
        let version = json!({
            "libraries": [{
                "name": "com.example:tool:1.0",
                "downloads": { "artifact": { "path": path, "url": official } }
            }]
        });

        let (jobs, _dir) = run(&version, true);

        assert_eq!(jobs.len(), 1);
        assert_eq!(
            jobs[0].url,
            "https://bmclapi2.bangbang93.com/libraries/com/example/tool/1.0/tool-1.0.jar"
        );
        assert_eq!(jobs[0].fallback_url.as_deref(), Some(official.as_str()));
    }

    #[test]
    fn test_maven_fallback_uses_bmcl_maven() {
        let version = json!({
            "libraries": [{
                "name": "com.example:tool:1.0",
                "url": "https://libraries.minecraft.net/"
            }]
        });

        let (jobs, _dir) = run(&version, true);

        assert_eq!(jobs.len(), 1);
        assert_eq!(
            jobs[0].url,
            "https://bmclapi2.bangbang93.com/maven/com/example/tool/1.0/tool-1.0.jar"
        );
        assert_eq!(
            jobs[0].fallback_url.as_deref(),
            Some("https://libraries.minecraft.net/com/example/tool/1.0/tool-1.0.jar")
        );

        let (jobs, _dir) = run(&version, false);
        assert_eq!(
            jobs[0].url,
            "https://libraries.minecraft.net/com/example/tool/1.0/tool-1.0.jar"
        );
        assert_eq!(
            jobs[0].fallback_url.as_deref(),
            Some("https://bmclapi2.bangbang93.com/maven/com/example/tool/1.0/tool-1.0.jar")
        );
    }
}
