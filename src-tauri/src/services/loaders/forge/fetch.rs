//! 库下载：重试、校验、镜像回退（模块名不叫 download，避免和 `crate::services::download` 撞名）

use super::*;
use super::{MAVEN_CENTRAL, MAVEN_FORGE, MAVEN_MINECRAFT};
use crate::errors::LauncherError;
use crate::services::download;
use crate::services::download::source::{self as download_sources, DownloadSource};
use crate::services::http_client;
use crate::utils::file_utils;
use log::{debug, info, warn};
use reqwest::Client;
use serde_json::Value;
use std::fs;
use std::io::{Read, Write};
use std::path::Path;

/// 流式把 URL 写到目标文件，边写边算 sha1，可选校验 ZIP 头
///
/// `resp.bytes()` 会把整个 jar 读进内存；库文件走同一条路径且并发下载，
/// 几个大 jar 同时进来就是几十 MB 峰值。流式之后内存占用与文件大小无关。
/// `validate` 拿到「ZIP 头是否合法 + 实际 sha1」，返回 false 表示这份内容不可用
pub(crate) async fn download_to_file(
    url: &str,
    client: &Client,
    target: &Path,
    expected_sha1: Option<&str>,
    require_zip: bool,
) -> Result<bool, LauncherError> {
    let response = download_with_retry(url, client, 2).await?;

    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }

    // 先写临时文件再改名：校验失败时不能留下半截 jar 冒充已下载
    let temp = target.with_extension("download.tmp");
    let mut file = fs::File::create(&temp)?;
    let mut hasher = file_utils::FileHasher::new();
    let mut head = [0u8; 4];
    let mut head_len = 0usize;
    let mut total: u64 = 0;

    let mut response = response;
    let written = async {
        while let Some(chunk) = response.chunk().await? {
            if head_len < head.len() {
                let take = (head.len() - head_len).min(chunk.len());
                head[head_len..head_len + take].copy_from_slice(&chunk[..take]);
                head_len += take;
            }
            file.write_all(&chunk)?;
            hasher.update(&chunk);
            total += chunk.len() as u64;
        }
        Ok::<(), LauncherError>(())
    }
    .await;

    if let Err(error) = written {
        drop(file);
        let _ = fs::remove_file(&temp);
        return Err(error);
    }
    drop(file);

    let is_zip = head_len >= 4 && head == [0x50, 0x4B, 0x03, 0x04];
    let actual = hasher.finish_hex();

    let hash_ok = match expected_sha1.map(str::trim).filter(|h| !h.is_empty()) {
        Some(expected) => {
            let ok = actual.eq_ignore_ascii_case(expected);
            if !ok {
                warn!("Forge: sha1 校验失败: {}", target.display());
            }
            ok
        }
        None => true,
    };

    let zip_ok = !require_zip || is_zip;
    if require_zip && !is_zip {
        warn!("Forge: 下载内容不是 ZIP: {}", target.display());
    }

    // 空文件/超小文件一律当作失败：多半是镜像返回的错误页
    if !hash_ok || !zip_ok || total < 100 {
        let _ = fs::remove_file(&temp);
        return Ok(false);
    }

    if let Err(error) = fs::rename(&temp, target) {
        let _ = fs::remove_file(&temp);
        return Err(error.into());
    }

    Ok(true)
}

pub(crate) async fn download_forge_installer(
    forge_version: &ForgeVersion,
) -> Result<tempfile::TempPath, LauncherError> {
    let installer_filename = format!(
        "forge-{}-{}-installer.jar",
        forge_version.mcversion, forge_version.version
    );

    let sources = forge_installer_sources(&forge_version.mcversion, &forge_version.version);

    // 安装器是实打实的大文件下载，用下载客户端（只设读超时）而不是 30 秒总超时的 API 客户端
    let download_client = download::get_http_client();
    let api_client = http_client::get_client();

    for url in &sources {
        info!("Forge: 尝试下载: {}", url);

        let tmp = tempfile::Builder::new()
            .prefix(&installer_filename)
            .suffix(".jar")
            .tempfile()
            .map_err(|e| LauncherError::Custom(format!("创建临时文件失败: {}", e)))?;
        let temp_path = tmp.into_temp_path();

        let expected = download_sha1(&api_client, url).await;
        if expected.is_none() {
            warn!("Forge: 未取到安装器 sha1，仅校验 ZIP 头: {}", url);
        }

        match download_to_file(url, &download_client, &temp_path, expected.as_deref(), true).await {
            Ok(true) => {
                if expected.is_some() {
                    info!("Forge: 安装器 sha1 校验通过");
                }
                info!("Forge: 安装器已下载");
                return Ok(temp_path);
            }
            Ok(false) => continue,
            Err(error) => {
                warn!("Forge: 下载安装器失败: {}", error);
                continue;
            }
        }
    }

    Err(LauncherError::Custom("安装器下载失败".to_string()))
}

pub(crate) async fn download_sha1(client: &Client, url: &str) -> Option<String> {
    let response = download_with_retry(&format!("{url}.sha1"), client, 0)
        .await
        .ok()?;
    let text = response.text().await.ok()?;
    file_utils::parse_sha1(&text)
}

pub(crate) async fn download_with_retry(
    url: &str,
    client: &Client,
    max_retries: usize,
) -> Result<reqwest::Response, LauncherError> {
    let mut retry_count = 0;

    while retry_count <= max_retries {
        retry_count += 1;
        debug!("Forge: 下载尝试第{}次: {}", retry_count, url);

        if retry_count > 1 {
            let delay = std::cmp::min(2u64.pow(retry_count as u32 - 1), 10);
            tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
        }

        match client.get(url).send().await {
            Ok(response) if response.status().is_success() => {
                return Ok(response);
            }
            Ok(response) => {
                warn!("Forge: 下载失败，状态: {}", response.status());
            }
            Err(e) => {
                warn!("Forge: 网络错误: {}", e);
            }
        }
    }

    Err(LauncherError::Custom(format!(
        "下载失败: 超过最大重试次数 {}",
        url
    )))
}

pub(crate) async fn download_library(
    libraries_dir: &Path,
    rel_path: &str,
    sources: Vec<String>,
    lib_name: &str,
) -> Result<(), LauncherError> {
    let target_path = libraries_dir.join(rel_path);
    let client = download::get_http_client();

    // maven 的 .sha1 旁车文件是这类预置库唯一能拿到的声明值；取不到就只能做 ZIP 头校验
    let expected_sha1 = if target_path.exists() {
        let mut found = None;
        for url in &sources {
            if let Some(sha1) = download_sha1(&client, url).await {
                found = Some(sha1);
                break;
            }
        }
        found
    } else {
        None
    };

    if target_path.exists() {
        if verify_existing_library(&target_path, expected_sha1.clone()).await? {
            return Ok(());
        }
        warn!(
            "Forge: {} 已存在的文件校验失败，重新下载: {}",
            lib_name,
            target_path.display()
        );
        fs::remove_file(&target_path)?;
    }

    if let Some(parent) = target_path.parent() {
        fs::create_dir_all(parent)?;
    }

    for url in &sources {
        match download_to_file(url, &client, &target_path, expected_sha1.as_deref(), true).await {
            Ok(true) => {
                info!("Forge: {} 下载成功", lib_name);
                return Ok(());
            }
            Ok(false) => continue,
            Err(error) => {
                warn!("Forge: {} 下载失败: {}", lib_name, error);
                continue;
            }
        }
    }

    Err(LauncherError::Custom(format!(
        "所有 {} 下载源均失败",
        lib_name
    )))
}

/// 已有文件按声明的 SHA1 校验；没有 SHA1 时至少确认它是一个完整的 ZIP，
/// 避免半截 jar 或镜像错误页被当成已下载
pub(crate) fn existing_library_is_valid(
    path: &Path,
    expected_sha1: Option<&str>,
) -> Result<bool, LauncherError> {
    if !path.is_file() {
        return Ok(false);
    }
    if fs::metadata(path)?.len() < 100 {
        return Ok(false);
    }

    if let Some(expected) = expected_sha1.map(str::trim).filter(|hash| !hash.is_empty()) {
        return file_utils::verify_file(path, Some(expected), 0);
    }

    let mut head = [0u8; 4];
    let mut file = fs::File::open(path)?;
    let mut read = 0usize;
    while read < head.len() {
        match file.read(&mut head[read..]) {
            Ok(0) => break,
            Ok(count) => read += count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error.into()),
        }
    }

    Ok(read == head.len() && head == [0x50, 0x4B, 0x03, 0x04])
}

pub(crate) async fn verify_existing_library(
    path: &Path,
    expected_sha1: Option<String>,
) -> Result<bool, LauncherError> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || existing_library_is_valid(&path, expected_sha1.as_deref()))
        .await
        .map_err(|e| LauncherError::Custom(format!("校验库文件任务异常终止: {}", e)))?
}

/// 老版本 Forge 预置库的候选地址：镜像 `/maven/{path}` 在前，官方 maven 兜底
pub(crate) fn preloaded_library_sources(path: &str, official_urls: Vec<String>) -> Vec<String> {
    let source = DownloadSource::current();
    let mut urls = download_sources::build_mirror_urls(source, &format!("/maven/{path}"));
    urls.extend(official_urls);
    download_sources::dedup_urls(urls)
}

pub(crate) async fn download_launchwrapper_library(
    libraries_dir: &Path,
    _mc_version: &str,
) -> Result<(), LauncherError> {
    let path = "net/minecraft/launchwrapper/1.12/launchwrapper-1.12.jar";
    download_library(
        libraries_dir,
        path,
        preloaded_library_sources(
            path,
            vec![
                format!("{}/{}", MAVEN_MINECRAFT, path),
                format!("{}/{}", MAVEN_CENTRAL, path),
            ],
        ),
        "LaunchWrapper",
    )
    .await
}

pub(crate) async fn download_asm_library(
    libraries_dir: &Path,
    mc_version: &str,
) -> Result<(), LauncherError> {
    let version = if mc_version.starts_with("1.7") {
        "5.0.3"
    } else if mc_version.starts_with("1.8")
        || mc_version.starts_with("1.9")
        || mc_version.starts_with("1.10")
        || mc_version.starts_with("1.11")
    {
        "5.0.4"
    } else {
        "5.2"
    };

    let path = format!("org/ow2/asm/asm-all/{}/asm-all-{}.jar", version, version);
    download_library(
        libraries_dir,
        &path,
        preloaded_library_sources(&path, vec![format!("{}/{}", MAVEN_CENTRAL, path)]),
        "ASM",
    )
    .await
}

pub(crate) async fn download_lzma_library(
    libraries_dir: &Path,
    mc_version: &str,
) -> Result<(), LauncherError> {
    let path = if mc_version.starts_with("1.7") {
        "lzma/lzma/0.0.1/lzma-0.0.1.jar"
    } else {
        "org/tukaani/xz/1.8/xz-1.8.jar"
    };
    download_library(
        libraries_dir,
        path,
        preloaded_library_sources(path, vec![format!("{}/{}", MAVEN_CENTRAL, path)]),
        "LZMA/XZ",
    )
    .await
}

use crate::utils::minecraft::maven_name_to_path;

pub(crate) async fn download_library_from_profile(
    library: &Value,
    libraries_dir: &Path,
    client: &Client,
) -> Result<(), LauncherError> {
    let name = match library["name"].as_str() {
        Some(n) => n,
        None => return Ok(()),
    };

    if let Some(false) = library.get("clientreq").and_then(|v| v.as_bool()) {
        return Ok(());
    }

    let artifact_sha1 = library
        .get("downloads")
        .and_then(|d| d.get("artifact"))
        .and_then(|a| a.get("sha1"))
        .and_then(|v| v.as_str())
        .filter(|value| !value.trim().is_empty());
    let checksum_sha1 = library
        .get("checksums")
        .and_then(|v| v.as_array())
        .and_then(|checksums| {
            checksums
                .iter()
                .filter_map(|value| value.as_str())
                .find(|value| file_utils::parse_sha1(value).is_some())
        });
    let expected_sha1 = artifact_sha1.or(checksum_sha1);

    if let Some(artifact) = library.get("downloads").and_then(|d| d.get("artifact"))
        && let Some(path) = artifact.get("path").and_then(|p| p.as_str())
    {
        // path 来自 installer 的 profile（镜像/安装包可控），穿越段会让 join 逃出 libraries/
        if !file_utils::is_safe_relative_path(path) {
            return Err(LauncherError::Custom(format!(
                "库路径可疑，拒绝下载: {}",
                path
            )));
        }
        let target_path = libraries_dir.join(path);
        if target_path.exists() {
            if verify_existing_library(&target_path, expected_sha1.map(str::to_string)).await? {
                return Ok(());
            }
            warn!(
                "Forge: 已存在的库校验失败，重新下载: {}",
                target_path.display()
            );
            fs::remove_file(&target_path)?;
        }

        if let Some(parent) = target_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let source = DownloadSource::current();
        let mut sources = Vec::new();
        if let Some(url) = artifact.get("url").and_then(|u| u.as_str()) {
            // 镜像优先：官方 maven 在国内可能整段超时，先试镜像能把等待从几十秒压到几百毫秒
            sources.extend(mirror_urls(url, source));
            sources.push(url.to_string());
        }
        sources.extend(download_sources::build_mirror_urls(
            source,
            &format!("/maven/{path}"),
        ));
        sources.push(format!("{}/{}", MAVEN_MINECRAFT, path));
        sources.push(format!("{}/{}", MAVEN_FORGE, path));
        let sources = download_sources::dedup_urls(sources);

        for url in &sources {
            match download_to_file(url, client, &target_path, expected_sha1, true).await {
                Ok(true) => return Ok(()),
                Ok(false) => continue,
                Err(error) => {
                    warn!("Forge: 下载 {} 失败: {}", target_path.display(), error);
                    continue;
                }
            }
        }
    }

    if let Some(maven_path) = maven_name_to_path(name, None, "jar") {
        // 坐标同样来自 profile，`..` 段在转路径后同样能逃出 libraries/
        if !file_utils::is_safe_relative_path(&maven_path) {
            return Err(LauncherError::Custom(format!(
                "库坐标可疑，拒绝下载: {}",
                name
            )));
        }
        let target_path = libraries_dir.join(&maven_path);
        if target_path.exists() {
            if verify_existing_library(&target_path, expected_sha1.map(str::to_string)).await? {
                return Ok(());
            }
            warn!(
                "Forge: 已存在的库校验失败，重新下载: {}",
                target_path.display()
            );
            fs::remove_file(&target_path)?;
        }

        let source = DownloadSource::current();
        let sources = download_sources::dedup_urls(
            [
                download_sources::build_mirror_urls(source, &format!("/maven/{maven_path}")),
                vec![
                    format!("{}/{}", MAVEN_MINECRAFT, maven_path),
                    format!("{}/{}", MAVEN_FORGE, maven_path),
                    format!("{}/{}", MAVEN_CENTRAL, maven_path),
                ],
            ]
            .concat(),
        );

        let client = download::get_http_client();
        for url in &sources {
            match download_to_file(url, &client, &target_path, expected_sha1, true).await {
                Ok(true) => return Ok(()),
                Ok(false) => continue,
                Err(error) => {
                    warn!("Forge: 下载 {} 失败: {}", target_path.display(), error);
                    continue;
                }
            }
        }
    }

    Err(LauncherError::Custom(format!(
        "库 {} 没下载下来，最后几个源都没给出可用的 jar（网络问题或该坐标在上游不存在）",
        name
    )))
}
