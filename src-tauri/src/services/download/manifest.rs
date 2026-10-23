//! 版本清单
//!
//! 按设置里的三种模式请求：自动在官方与镜像间竞速，优先官方先试官方，
//! 优先镜像先并发尝试镜像，全部失败后再用官方源兜底

use crate::errors::LauncherError;
use crate::models::VersionManifest;
use crate::services::download::source::DownloadSource;
use crate::services::http_client;
use log::{debug, info, warn};

pub async fn get_versions() -> Result<VersionManifest, LauncherError> {
    let client = http_client::get_client();
    let (primary, fallback) = DownloadSource::current().manifest_batches();

    info!("开始获取版本列表");

    if let Some(manifest) = race_manifests(&client, primary).await {
        info!("成功获取版本列表，共{}个版本", manifest.versions.len());
        return Ok(manifest);
    }

    if let Some(manifest) = race_manifests(&client, fallback).await {
        info!("成功获取版本列表，共{}个版本", manifest.versions.len());
        return Ok(manifest);
    }

    Err(LauncherError::Custom(
        "所有源都尝试失败，请检查网络连接".to_string(),
    ))
}

/// 同一批内的源并发请求，先成功返回的那个胜出
async fn race_manifests(
    client: &reqwest::Client,
    sources: Vec<(DownloadSource, String)>,
) -> Option<VersionManifest> {
    if sources.is_empty() {
        return None;
    }

    let mut tasks = tokio::task::JoinSet::new();
    for (_source, url) in sources {
        let client = client.clone();
        tasks.spawn(async move {
            let result = fetch_versions(&client, &url).await;
            (url, result)
        });
    }

    while let Some(joined) = tasks.join_next().await {
        match joined {
            Ok((_url, Ok(manifest))) => {
                // 剩下的请求随 JoinSet 一起 abort，不再占连接
                tasks.abort_all();
                return Some(manifest);
            }
            Ok((url, Err(e))) => warn!("从源 {} 获取失败: {}", url, e),
            Err(e) => warn!("获取版本列表任务异常终止: {}", e),
        }
    }

    None
}

async fn fetch_versions(
    client: &reqwest::Client,
    url: &str,
) -> Result<VersionManifest, LauncherError> {
    debug!("准备发送请求到: {}", url);

    let response = client.get(url).send().await?.error_for_status()?;
    debug!("响应状态码: {}", response.status());

    let text = response.text().await?;
    let text = text.trim_start_matches('\u{feff}').to_string();

    let manifest = serde_json::from_str::<VersionManifest>(&text).map_err(|e| {
        warn!("JSON 解析错误: {}", e);
        LauncherError::Json(e)
    })?;

    debug!("解析版本清单完成，共 {} 个版本", manifest.versions.len());

    Ok(manifest)
}
