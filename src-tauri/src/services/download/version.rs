//! 版本资源整合与下载
//!
//! 跟随 inheritsFrom 递归补基础版本，汇总成下载任务并按镜像改写地址。
//! 元数据（清单 / 版本 JSON）按「所选源 → 其他国内镜像 → 官方」依次尝试，
//! 每个文件的下载任务也带上整条镜像链，单源失败时由下载器直接换源。

use super::batch::download_all_files;
use super::http::get_http_client;
use super::source::{self as download_sources, DownloadSource};
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
    let source = DownloadSource::from_setting(config.download_mirror.as_deref());

    let mut downloads = Vec::new();
    collect_libraries(version_json, &libraries_base_dir, source, &mut downloads)?;

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
    let mut visited = std::collections::HashSet::new();
    download_version_inner(version_id, mirror, window, scope, &mut visited).await
}

async fn download_version_inner(
    version_id: String,
    mirror: Option<String>,
    window: &Window,
    scope: VersionDownloadScope,
    visited: &mut std::collections::HashSet<String>,
) -> Result<(), LauncherError> {
    // 环形继承链（A 继承 B、B 又继承 A）会让下面的递归永不终止，按已处理的版本 id 去重
    if !visited.insert(version_id.clone()) {
        return Err(LauncherError::Custom(format!(
            "版本继承链存在循环: {} 重复出现",
            version_id
        )));
    }

    let source = mirror
        .as_deref()
        .map(|value| DownloadSource::from_setting(Some(value)))
        .unwrap_or(DownloadSource::Official);

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
    let (actual_version_id, version_json, text, download_source) =
        if local_version_json_path.exists() {
            let local_text = fs::read_to_string(&local_version_json_path)?;
            let local_json: serde_json::Value = serde_json::from_str(&local_text)
                .map_err(|e| LauncherError::Custom(format!("解析本地版本JSON失败: {}", e)))?;

            if let Some(inherits_from) = local_json["inheritsFrom"].as_str() {
                file_utils::validate_version_id_or_error(inherits_from)?;
                info!("版本 {} 继承自 {}", version_id, inherits_from);
                Box::pin(download_version_inner(
                    inherits_from.to_string(),
                    mirror.clone(),
                    window,
                    scope,
                    visited,
                ))
                .await?;

                if scope == VersionDownloadScope::Full {
                    // 基础版本已下载，整合包的库文件另行处理
                    return download_modpack_libraries(
                        &local_json,
                        &libraries_base_dir,
                        source,
                        window,
                    )
                    .await;
                }

                return Ok(());
            }

            (version_id.clone(), local_json, local_text, source)
        } else {
            let (version_json, text, fetched_source) =
                fetch_version_json(&client, &version_id, source).await?;
            (version_id.clone(), version_json, text, fetched_source)
        };

    let mut downloads = Vec::new();

    collect_client_jar(
        &version_json,
        &version_dir,
        &actual_version_id,
        download_source,
        &mut downloads,
    )?;

    if scope == VersionDownloadScope::Full {
        collect_assets(
            &client,
            &version_json,
            &assets_base_dir,
            download_source,
            &mut downloads,
        )
        .await?;

        collect_libraries(
            &version_json,
            &libraries_base_dir,
            download_source,
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

/// 从所选源取版本清单与版本 JSON
///
/// 同一批源并发竞速，全部失败后再跑下一批；返回值带上真正成功的源，
/// 文件下载优先走这个源，而不是继续从已经失败的源开始。
async fn fetch_version_json(
    client: &reqwest::Client,
    version_id: &str,
    source: DownloadSource,
) -> Result<(serde_json::Value, String, DownloadSource), LauncherError> {
    let (primary, fallback) = source.manifest_batches();

    if let Some(result) = race_version_sources(client, version_id, primary).await {
        return Ok(result);
    }
    if let Some(result) = race_version_sources(client, version_id, fallback).await {
        return Ok(result);
    }

    Err(LauncherError::Custom(format!(
        "无法从任何下载源获取版本 {}",
        version_id
    )))
}

async fn race_version_sources(
    client: &reqwest::Client,
    version_id: &str,
    sources: Vec<(DownloadSource, String)>,
) -> Option<(serde_json::Value, String, DownloadSource)> {
    if sources.is_empty() {
        return None;
    }

    let mut tasks = tokio::task::JoinSet::new();
    for (candidate, manifest_url) in sources {
        let client = client.clone();
        let version_id = version_id.to_string();
        tasks.spawn(async move {
            let result =
                fetch_version_json_from(&client, &version_id, candidate, &manifest_url).await;
            (manifest_url, candidate, result)
        });
    }

    while let Some(joined) = tasks.join_next().await {
        match joined {
            Ok((_url, candidate, Ok((json, text)))) => {
                // 剩下的请求随 JoinSet 一起 abort，不再占连接
                tasks.abort_all();
                return Some((json, text, candidate));
            }
            Ok((url, _candidate, Err(error))) => {
                warn!("从 {} 获取版本 {} 失败: {}", url, version_id, error)
            }
            Err(e) => warn!("获取版本 {} 的元数据任务异常终止: {}", version_id, e),
        }
    }

    None
}

async fn fetch_version_json_from(
    client: &reqwest::Client,
    version_id: &str,
    source: DownloadSource,
    manifest_url: &str,
) -> Result<(serde_json::Value, String), LauncherError> {
    let manifest: VersionManifest = client
        .get(manifest_url)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    let version = manifest
        .versions
        .iter()
        .find(|v| v.id == version_id)
        .ok_or_else(|| LauncherError::Custom(format!("版本 {} 不存在", version_id)))?;

    let version_json_url = download_sources::version_json_url(&version.url, source);
    let text = client
        .get(&version_json_url)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    let version_json: serde_json::Value = serde_json::from_str(&text)
        .or_else(|_| serde_json::from_str(text.trim_start_matches('\u{feff}')))
        .map_err(|_| LauncherError::Custom(format!("无法解析版本JSON for {}", version_id)))?;

    Ok((version_json, text))
}

async fn download_modpack_libraries(
    version_json: &serde_json::Value,
    libraries_base_dir: &Path,
    source: DownloadSource,
    window: &Window,
) -> Result<(), LauncherError> {
    let mut downloads = Vec::new();

    collect_libraries(version_json, libraries_base_dir, source, &mut downloads)?;

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
    source: DownloadSource,
    downloads: &mut Vec<DownloadJob>,
) -> Result<(), LauncherError> {
    let client_info = &version_json["downloads"]["client"];
    let client_url = client_info["url"]
        .as_str()
        .ok_or_else(|| LauncherError::Custom("无法获取客户端下载URL".to_string()))?;
    let client_size = client_info["size"].as_u64().unwrap_or(0);
    let client_hash = client_info["sha1"].as_str().map(|h| h.to_string());
    let client_jar_path = version_dir.join(format!("{}.jar", version_id));

    let urls = download_sources::client_jar_urls(source, Some(client_url), client_hash.as_deref());
    let Some(primary_url) = urls.first() else {
        return Err(LauncherError::Custom("无法构造客户端下载地址".to_string()));
    };

    downloads.push(DownloadJob::new(
        primary_url.clone(),
        urls.into_iter().skip(1).collect(),
        client_jar_path,
        client_size,
        client_hash,
    ));

    Ok(())
}

async fn collect_assets(
    client: &reqwest::Client,
    version_json: &serde_json::Value,
    assets_base_dir: &Path,
    source: DownloadSource,
    downloads: &mut Vec<DownloadJob>,
) -> Result<(), LauncherError> {
    let assets_index_id = version_json["assetIndex"]["id"]
        .as_str()
        .ok_or_else(|| LauncherError::Custom("无法获取资源索引ID".to_string()))?;
    let assets_index_url = version_json["assetIndex"]["url"]
        .as_str()
        .ok_or_else(|| LauncherError::Custom("无法获取资源索引URL".to_string()))?;
    let assets_index_sha1 = version_json["assetIndex"]["sha1"].as_str();

    // id 来自版本清单（镜像可控），带路径分隔符或 .. 的值能让下面的 join 逃出 indexes/
    if !file_utils::is_safe_relative_path(assets_index_id)
        || assets_index_id.contains('/')
        || assets_index_id.contains('\\')
    {
        return Err(LauncherError::Custom(format!(
            "assetIndex.id 含非法路径字符: {}",
            assets_index_id
        )));
    }

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
        download_asset_index(
            client,
            assets_index_url,
            assets_index_sha1,
            assets_index_id,
            source,
            &assets_index_path,
        )
        .await?;
    }

    // 索引动辄十几 MB、几千个对象：读盘 + 解析 + 建任务是纯 CPU 活，留在 async worker 上会拖住其他命令
    let objects_dir = assets_base_dir.join("objects");
    let index_path = assets_index_path.clone();

    let jobs = tokio::task::spawn_blocking(move || -> Result<Vec<DownloadJob>, LauncherError> {
        let index_content = fs::read_to_string(&index_path)?;
        let index: serde_json::Value = serde_json::from_str(&index_content)?;
        let mut jobs = Vec::new();
        collect_asset_objects(&index, &objects_dir, source, &mut jobs)?;
        Ok(jobs)
    })
    .await
    .map_err(|e| LauncherError::Custom(format!("解析资源索引失败: {}", e)))??;

    downloads.extend(jobs);

    Ok(())
}

/// 索引文件很小，但同样值得换源：某个镜像缺文件时不必让整个版本下载失败
async fn download_asset_index(
    client: &reqwest::Client,
    declared_url: &str,
    sha1: Option<&str>,
    index_id: &str,
    source: DownloadSource,
    target: &Path,
) -> Result<(), LauncherError> {
    let urls = download_sources::asset_index_urls(source, Some(declared_url), sha1, index_id);
    let mut last_error = None;

    for url in &urls {
        match client.get(url).send().await {
            Ok(response) if response.status().is_success() => {
                let bytes = response.bytes().await?;
                fs::write(target, &bytes)?;
                return Ok(());
            }
            Ok(response) => {
                last_error = Some(LauncherError::Custom(format!(
                    "获取资源索引失败: HTTP {} for {}",
                    response.status(),
                    url
                )));
            }
            Err(error) => {
                last_error = Some(LauncherError::Http(error));
            }
        }
    }

    Err(last_error.unwrap_or_else(|| {
        LauncherError::Custom("无法获取资源索引：没有可用的下载地址".to_string())
    }))
}

/// 把资源索引里的对象展开成下载任务
fn collect_asset_objects(
    index: &serde_json::Value,
    objects_dir: &Path,
    source: DownloadSource,
    downloads: &mut Vec<DownloadJob>,
) -> Result<(), LauncherError> {
    let Some(objects) = index["objects"].as_object() else {
        return Ok(());
    };

    for (_path, obj) in objects {
        let hash = obj["hash"]
            .as_str()
            .ok_or_else(|| LauncherError::Custom("资源缺少hash".to_string()))?;
        // hash 直接拼进对象路径（objects/<前2位>/<hash>）与下载 URL，
        // 非标准格式既可能是数据损坏也可能是穿越攻击，一律按 sha1 校验后放行
        let Some(hash) = file_utils::parse_sha1(hash) else {
            warn!("资源索引里有异常的 hash，已跳过: {}", hash);
            continue;
        };
        let prefix = &hash[..2];
        let size = obj["size"].as_u64().unwrap_or(0);
        let urls = download_sources::asset_object_urls(source, prefix, &hash);
        let Some(primary_url) = urls.first() else {
            continue;
        };

        downloads.push(DownloadJob::new(
            primary_url.clone(),
            urls.into_iter().skip(1).collect(),
            objects_dir.join(prefix).join(&hash),
            size,
            Some(hash),
        ));
    }

    Ok(())
}

fn collect_libraries(
    version_json: &serde_json::Value,
    libraries_base_dir: &Path,
    source: DownloadSource,
    downloads: &mut Vec<DownloadJob>,
) -> Result<(), LauncherError> {
    fs::create_dir_all(libraries_base_dir)?;

    for file in crate::utils::library::resolve_library_files(version_json) {
        // relative_path 可能来自镜像返回的 downloads.artifact.path，
        // 带穿越段的值会让 join 逃出 libraries/，校验不过直接跳过
        if !file_utils::is_safe_relative_path(&file.relative_path) {
            warn!("库文件路径可疑，已跳过: {}", file.relative_path);
            continue;
        }

        let target_path = libraries_base_dir.join(&file.relative_path);

        // maven 回退路径才按“文件已在位”跳过；官方 artifact 的续传/校验交给下载状态
        if file.from_maven && target_path.exists() {
            continue;
        }

        let urls = download_sources::library_urls(source, &file.relative_path, file.url.as_deref());
        let Some(primary_url) = urls.first() else {
            continue;
        };

        downloads.push(DownloadJob::new(
            primary_url.clone(),
            urls.into_iter().skip(1).collect(),
            target_path,
            file.size,
            file.sha1,
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{collect_asset_objects, collect_libraries};
    use crate::services::download::source::{
        BMCL_BASE, DownloadSource, HKMC_EO_BASE, HKMC_MAIN_BASE,
    };
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
        collect_asset_objects(&index, &objects, DownloadSource::HkmcMain, &mut jobs).unwrap();

        assert_eq!(jobs.len(), 1);
        // 首选 MC 光速主线路，EO 与 BMCL 作为备用，官方资源站兜底
        assert_eq!(
            jobs[0].url,
            format!(
                "{}/assets/01/0123456789012345678901234567890123456789",
                HKMC_MAIN_BASE
            )
        );
        assert_eq!(
            jobs[0].fallback_urls[0],
            format!(
                "{}/assets/01/0123456789012345678901234567890123456789",
                HKMC_EO_BASE
            )
        );
        assert_eq!(
            jobs[0].fallback_urls[1],
            format!(
                "{}/assets/01/0123456789012345678901234567890123456789",
                BMCL_BASE
            )
        );
        assert_eq!(
            jobs[0].fallback_urls.last().unwrap(),
            "https://resources.download.minecraft.net/01/0123456789012345678901234567890123456789"
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
        collect_asset_objects(&index, dir.path(), DownloadSource::Official, &mut jobs).unwrap();

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
        source: DownloadSource,
    ) -> (Vec<crate::models::DownloadJob>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let libraries = dir.path().join("libraries");
        let mut jobs = Vec::new();
        collect_libraries(version, &libraries, source, &mut jobs).unwrap();
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

        let (jobs, _dir) = run(&version, DownloadSource::Official);

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

        let (jobs, _dir) = run(&version, DownloadSource::Official);

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

        let (jobs, _dir) = run(&version, DownloadSource::HkmcMain);

        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].url, format!("{}/maven/{path}", HKMC_MAIN_BASE));
        assert_eq!(
            jobs[0].fallback_urls.last().map(String::as_str),
            Some(official.as_str())
        );
    }

    #[test]
    fn test_maven_fallback_uses_mirror_maven() {
        let version = json!({
            "libraries": [{
                "name": "com.example:tool:1.0",
                "url": "https://libraries.minecraft.net/"
            }]
        });

        let (jobs, _dir) = run(&version, DownloadSource::Bmcl);

        assert_eq!(jobs.len(), 1);
        assert_eq!(
            jobs[0].url,
            format!("{}/maven/com/example/tool/1.0/tool-1.0.jar", BMCL_BASE)
        );
        assert_eq!(
            jobs[0].fallback_urls.last().map(String::as_str),
            Some("https://libraries.minecraft.net/com/example/tool/1.0/tool-1.0.jar")
        );

        let (jobs, _dir) = run(&version, DownloadSource::Official);
        assert_eq!(
            jobs[0].url,
            "https://libraries.minecraft.net/com/example/tool/1.0/tool-1.0.jar"
        );
        // 官方优先时镜像作为备用，首选源不可用仍能回退
        assert_eq!(
            jobs[0].fallback_urls[0],
            format!("{}/maven/com/example/tool/1.0/tool-1.0.jar", BMCL_BASE)
        );
    }
}
