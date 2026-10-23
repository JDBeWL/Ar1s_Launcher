//! NeoForge 加载器安装
//!
//! 下载 installer jar，取其中的 version.json 与内嵌库落 libraries，1.20.2 起版本号不带 MC 前缀

use super::forge;
use crate::errors::LauncherError;
use crate::services::config;
use crate::services::download;
use crate::services::download::source::{self as download_sources, BMCL_BASE, DownloadSource};
use crate::services::http_client;
use crate::utils::file_utils;
use log::{info, warn};
use serde_json::Value;
use std::fs;
use std::io::Read;
use std::path::Path;
use std::sync::atomic::AtomicBool;

const NEOFORGE_MAVEN_URL: &str = "https://maven.neoforged.net/releases";
const BMCLAPI_NEOFORGE_URL: &str = "https://bmclapi2.bangbang93.com/neoforge";

pub async fn install_neoforge(
    mc_version: &str,
    neoforge_version: &str,
    instance_name: &str,
    game_dir: &Path,
    cancel: Option<&AtomicBool>,
) -> Result<(), LauncherError> {
    forge::check_cancelled(cancel)?;
    info!(
        "安装 NeoForge: MC {} + NeoForge {} -> {}",
        mc_version, neoforge_version, instance_name
    );

    // 和版本下载共用游戏目录锁，避免多实例同时写共享 libraries/
    let _download_lock = crate::utils::file_lock::acquire_async(
        &crate::services::instance::lock_dir(game_dir).join("download.lock"),
        std::time::Duration::from_secs(2),
    )
    .await?;

    // processors 要起 JVM；没在设置里指定 Java 时交给候选集，PATH 上的 java 也算兜底
    let app_config = config::load_config()?;
    let java_path = app_config
        .java_path
        .clone()
        .filter(|path| !path.is_empty())
        .unwrap_or_else(|| "java".to_string());

    let java_candidates = crate::services::launcher::java::collect_candidates(&app_config);

    let client = http_client::get_client();

    // 1.20.2 起 NeoForge 版本号不再带 mc 前缀，旧版才是 `mc-ver-loader-ver` 格式
    let full_version = if neoforge_version.contains('.') || neoforge_version.contains('-') {
        neoforge_version.to_string()
    } else {
        format!("{}-{}", mc_version, neoforge_version)
    };

    let temp_dir = game_dir.join("temp");
    fs::create_dir_all(&temp_dir)?;
    let installer_path = temp_dir.join(format!("neoforge-{}-installer.jar", full_version));
    // installer 只是安装过程的原料，成功失败都用不着；失败路径也清掉，别在 temp 里攒几十 MB 的 jar
    let _installer_guard = file_utils::TempPathGuard(installer_path.clone());

    let installer_url = format!(
        "{}/net/neoforged/neoforge/{}/neoforge-{}-installer.jar",
        NEOFORGE_MAVEN_URL, full_version, full_version
    );
    let source = DownloadSource::current();
    let maven_path =
        format!("net/neoforged/neoforge/{full_version}/neoforge-{full_version}-installer.jar");

    // MC 光速暂未镜像 NeoForge 的 maven（实测 502），国内模式下把 BMCL 放最前；
    // 光速地址仍排在后面，将来补上镜像时不用再改代码
    let mut installer_urls = Vec::new();
    if source.is_mirror() {
        installer_urls.push(format!(
            "{}/version/{}/download/installer.jar",
            BMCLAPI_NEOFORGE_URL, full_version
        ));
        installer_urls.push(format!("{BMCL_BASE}/maven/{maven_path}"));
        installer_urls.extend(
            download_sources::build_mirror_urls(source, &format!("/maven/{maven_path}"))
                .into_iter()
                .filter(|url| !url.starts_with(BMCL_BASE)),
        );
    }
    installer_urls.push(installer_url);
    let installer_urls = download_sources::dedup_urls(installer_urls);

    let mut downloaded = false;

    // installer 是几 MB 的大文件下载：用下载客户端（只设读超时），并流式落盘而不是整包进内存
    let download_client = download::get_http_client();

    for url in &installer_urls {
        forge::check_cancelled(cancel)?;
        info!("尝试下载 NeoForge installer: {}", url);

        let expected = forge::download_sha1(&client, url).await;
        if expected.is_none() {
            warn!("NeoForge: 未取到 installer sha1，仅校验 ZIP 头: {}", url);
        }

        match forge::download_to_file(
            url,
            &download_client,
            &installer_path,
            expected.as_deref(),
            true,
        )
        .await
        {
            Ok(true) => {
                downloaded = true;
                info!("NeoForge installer 下载成功: {}", url);
                break;
            }
            Ok(false) => warn!("NeoForge: installer 不是有效 ZIP，换下一个源"),
            Err(error) => warn!("NeoForge: installer 下载失败，换下一个源: {}", error),
        }
    }

    if !downloaded {
        return Err(LauncherError::Custom(
            "所有下载源都没能获取有效的 NeoForge installer".to_string(),
        ));
    }

    let libraries_dir = game_dir.join("libraries");
    let data_dir = libraries_dir
        .join("net/neoforged/neoforge")
        .join(&full_version);

    let (version_json, install_profile) = read_installer_jsons(&installer_path)?;
    let client = http_client::get_client();

    // 加载器本体（neoforge-<v>-universal.jar）只出现在 install_profile.libraries 里，
    // version.json 的 27 条里没有它 —— 只读 version.json 就永远下载不到
    let mut libs: Vec<Value> = vec![];
    if let Some(profile) = install_profile.as_ref() {
        libs.extend(array_of(profile, "libraries"));
    }
    libs.extend(array_of(&version_json, "libraries"));

    for lib in &libs {
        if let Err(e) = forge::download_library_from_profile(lib, &libraries_dir, &client).await {
            warn!("NeoForge: {}", e);
        }
    }

    // 单个库下载失败在这个函数里是静默的（老 Forge 的 pom-only 条目本来就没有 jar），
    // 所以缺件的重要在这里补：只重试还不在位的那些
    let missing: Vec<Value> = libs
        .iter()
        .filter(|lib| {
            lib["downloads"]["artifact"]["path"]
                .as_str()
                .is_some_and(|p| !libraries_dir.join(p).exists())
        })
        .cloned()
        .collect();
    for lib in &missing {
        info!("NeoForge: 重试下载缺失的库");
        if let Err(e) = forge::download_library_from_profile(lib, &libraries_dir, &client).await {
            warn!("NeoForge: {}", e);
        }
    }

    extract_installer_files(&installer_path, &libraries_dir, &data_dir)?;

    if let Some(profile) = install_profile.as_ref() {
        forge::check_cancelled(cancel)?;
        // PROCESS_MINECRAFT_JAR 产出 minecraft-client-patched，FML 运行时按
        // -DlibraryDirectory 去找它和 universal，两者都不在 profile 的 classpath 里
        forge::run_processors(
            profile,
            &forge::ProcessorVars {
                game_dir,
                libraries_dir: &libraries_dir,
                installer: &installer_path,
                minecraft_jar: game_dir
                    .join("versions")
                    .join(mc_version)
                    .join(format!("{}.jar", mc_version)),
                data_dir: data_dir.clone(),
                mc_version,
                loader_version: &full_version,
                java_fallback: &java_path,
                java_candidates: &java_candidates,
            },
            cancel,
        )
        .await?;
    }

    let universal = data_dir.join(format!("neoforge-{}-universal.jar", full_version));
    if !universal.exists() {
        return Err(LauncherError::Custom(format!(
            "装完了但加载器本体不在位（重试过一轮仍没下来，多半是到 maven.neoforged.net 的网络问题）：{}",
            universal.display()
        )));
    }

    let mut final_version = version_json;
    if let Some(obj) = final_version.as_object_mut() {
        obj.insert("id".to_string(), serde_json::json!(instance_name));
        // 少数构建的 version.json 没写父版本，缺了它启动时会少一半原版库
        if !obj.contains_key("inheritsFrom") {
            obj.insert("inheritsFrom".to_string(), serde_json::json!(mc_version));
        }
    }

    let version_dir = game_dir.join("versions").join(instance_name);
    fs::create_dir_all(&version_dir)?;

    let json_path = version_dir.join(format!("{}.json", instance_name));
    fs::write(&json_path, serde_json::to_string_pretty(&final_version)?)?;

    info!("NeoForge 版本 JSON 已创建: {}", json_path.display());

    Ok(())
}

/// 取 install_profile.json（新版 installer 才有，它带着加载器本体与 processors）
fn read_installer_jsons(installer_path: &Path) -> Result<(Value, Option<Value>), LauncherError> {
    let file = fs::File::open(installer_path)?;
    let mut archive = zip::ZipArchive::new(file)?;

    let read_entry = |archive: &mut zip::ZipArchive<fs::File>, name: &str| -> Option<String> {
        let mut f = archive.by_name(name).ok()?;
        let mut content = String::new();
        f.read_to_string(&mut content).ok()?;
        Some(content)
    };

    let version_str = read_entry(&mut archive, "version.json").ok_or_else(|| {
        LauncherError::Custom("NeoForge installer 中未找到 version.json".to_string())
    })?;
    let version_json: Value = serde_json::from_str(&version_str)
        .map_err(|e| LauncherError::Custom(format!("解析 version.json 失败: {}", e)))?;

    let profile = read_entry(&mut archive, "install_profile.json")
        .and_then(|s| serde_json::from_str::<Value>(&s).ok());

    Ok((version_json, profile))
}

fn array_of(json: &Value, key: &str) -> Vec<Value> {
    json.get(key)
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default()
}

/// installer 内部的 maven/ 归 libraries，data/ 归该加载器的库目录
/// （`{BINPATCH}` 就是这里的 `/data/client.lzma`）
fn extract_installer_files(
    installer_path: &Path,
    libraries_dir: &Path,
    data_dir: &Path,
) -> Result<(), LauncherError> {
    let file = fs::File::open(installer_path)?;
    let mut archive = zip::ZipArchive::new(file)?;

    for i in 0..archive.len() {
        let mut entry = match archive.by_index(i) {
            Ok(f) => f,
            Err(_) => continue,
        };
        let name = entry.name()?.to_string();

        if name.contains("..") || name.ends_with('/') {
            continue;
        }

        let target = if let Some(rel) = name.strip_prefix("maven/") {
            libraries_dir.join(rel)
        } else if let Some(rel) = name.strip_prefix("data/") {
            data_dir.join(rel)
        } else {
            continue;
        };

        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut buf = Vec::new();
        entry.read_to_end(&mut buf)?;
        fs::write(&target, &buf)?;
    }

    Ok(())
}

pub async fn get_neoforge_versions(
    mc_version: &str,
) -> Result<Vec<NeoForgeVersion>, LauncherError> {
    let client = http_client::get_client();

    let bmclapi_url = format!("{}/list/{}", BMCLAPI_NEOFORGE_URL, mc_version);

    if let Ok(response) = client.get(&bmclapi_url).send().await
        && response.status().is_success()
        && let Ok(versions) = response.json::<Vec<BmclapiNeoForgeVersion>>().await
    {
        let mut versions: Vec<NeoForgeVersion> = versions
            .into_iter()
            .map(|v| NeoForgeVersion {
                stable: !v.version.contains("beta"),
                version: v.version,
                mc_version: v.mc_version,
            })
            .collect();

        // BMCLAPI 的列表是升序（甚至混着字符串序，21.1.1 → 21.1.10 → 21.1.11），
        // 调用方默认取第一条，不自己排就会装到最老的版本
        versions.sort_by_key(|a| std::cmp::Reverse(version_key(&a.version)));
        versions.dedup_by(|a, b| a.version == b.version);

        return Ok(versions);
    }

    warn!("BMCLAPI 获取 NeoForge 版本失败，返回空列表");
    Ok(vec![])
}

#[derive(serde::Deserialize)]
struct BmclapiNeoForgeVersion {
    version: String,
    #[serde(rename = "mcversion")]
    mc_version: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct NeoForgeVersion {
    pub version: String,
    pub mc_version: String,
    /// 不带 `beta` 的算正式版
    pub stable: bool,
}

/// `26.3.0.4-beta` / `21.1.248` / `47.1.5` 都按数字段比较，同核心号下正式版优先
fn version_key(raw: &str) -> ([u64; 4], u8) {
    let (core, pre) = match raw.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (raw, None),
    };

    let mut parts = core.split('.').map(|part| part.parse::<u64>().unwrap_or(0));
    let mut numbers = [0u64; 4];
    for slot in numbers.iter_mut() {
        *slot = parts.next().unwrap_or(0);
    }

    (numbers, if pre.is_some() { 0 } else { 1 })
}

#[cfg(test)]
mod tests {
    use super::version_key;

    fn sorted<'a>(versions: &[&'a str]) -> Vec<&'a str> {
        let mut list: Vec<&str> = versions.to_vec();
        list.sort_by_key(|a| std::cmp::Reverse(version_key(a)));
        list
    }

    /// 字符串序必须按数字重排，否则前端默认取的第一条是 21.1.1
    #[test]
    fn test_versions_sort_numerically_newest_first() {
        assert_eq!(
            sorted(&["21.1.1", "21.1.10", "21.1.9", "21.1.262"]),
            vec!["21.1.262", "21.1.10", "21.1.9", "21.1.1"]
        );
        assert_eq!(
            sorted(&["26.3.0.0-beta", "26.3.0.86", "26.3.0.4-beta"]),
            vec!["26.3.0.86", "26.3.0.4-beta", "26.3.0.0-beta"]
        );
    }

    #[test]
    fn test_stable_beats_beta_with_the_same_core() {
        assert_eq!(
            sorted(&["47.1.106-beta", "47.1.106"]),
            vec!["47.1.106", "47.1.106-beta"]
        );
    }
}
