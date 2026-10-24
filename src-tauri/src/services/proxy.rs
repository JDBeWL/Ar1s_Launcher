//! 网络代理模块
//!
//! 校验并保存启动器代理设置，切换后重建共享 HTTP 客户端；只影响启动器请求

use crate::errors::LauncherError;
use crate::models::{ProxyMode, ProxySettings, ProxyTestResult};
use crate::services::{config, curseforge, download, http_client};
use log::{info, warn};
use reqwest::ClientBuilder;
use std::time::{Duration, Instant};

const TEST_URL: &str = "https://launchermeta.mojang.com/mc/game/version_manifest_v2.json";

enum AppliedProxy {
    System,
    Direct,
    Custom(Box<reqwest::Proxy>),
}

pub fn current_settings() -> ProxySettings {
    match config::load_config() {
        Ok(config) => ProxySettings {
            mode: config.proxy_mode,
            url: config.proxy_url.clone(),
        },
        Err(error) => {
            warn!("读取代理配置失败，本次按跟随系统处理: {}", error);
            ProxySettings {
                mode: ProxyMode::System,
                url: None,
            }
        }
    }
}

pub fn save_settings(settings: ProxySettings) -> Result<ProxySettings, LauncherError> {
    let url = match settings.mode {
        ProxyMode::Custom => Some(normalize_proxy_url(settings.url.as_deref().unwrap_or(""))?),
        _ => settings
            .url
            .as_deref()
            .map(str::trim)
            .filter(|url| !url.is_empty())
            .map(str::to_string),
    };
    let mode = settings.mode;
    let url_for_config = url.clone();
    config::update_config(|config| {
        config.proxy_mode = mode;
        config.proxy_url = url_for_config;
        Ok(())
    })?;

    http_client::reload();
    download::reload_http_client();
    curseforge::reload_minekuai_client();
    info!(
        "网络代理已切换为{}",
        describe(&ProxySettings {
            mode,
            url: url.clone()
        })
    );
    Ok(ProxySettings { mode, url })
}

pub async fn test_connection(settings: ProxySettings) -> Result<ProxyTestResult, LauncherError> {
    let client = configure(
        reqwest::Client::builder()
            .user_agent(http_client::USER_AGENT)
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(20)),
        &settings,
    )?
    .build()
    .map_err(|error| LauncherError::Custom(format!("创建代理测试客户端失败: {}", error)))?;

    let started = Instant::now();
    let response = client.get(TEST_URL).send().await.map_err(|error| {
        LauncherError::Custom(format!("代理连接失败: {}", summarize_request_error(&error)))
    })?;
    let status = response.status();
    if !status.is_success() {
        return Err(LauncherError::Custom(format!(
            "代理已连接，但测试接口返回 HTTP {}",
            status.as_u16()
        )));
    }

    Ok(ProxyTestResult {
        latency_ms: started.elapsed().as_millis() as u64,
        status: status.as_u16(),
    })
}

/// 启动时或重建客户端时读取当前配置；配置坏掉时回退到 reqwest 默认的系统代理
pub(crate) fn apply(builder: ClientBuilder) -> ClientBuilder {
    match build_proxy(&current_settings()) {
        Ok(AppliedProxy::System) => builder,
        Ok(AppliedProxy::Direct) => builder.no_proxy(),
        Ok(AppliedProxy::Custom(proxy)) => builder.proxy(*proxy),
        Err(error) => {
            warn!("保存的代理配置无效，回退为跟随系统代理: {}", error);
            builder
        }
    }
}

/// 国内源（如麦块 API）的代理策略：系统代理不跟随——系统代理通常是翻墙通道，
/// 国内站点经它绕到境外出口反而大面积失败；只有显式配置的自定义代理才跟随
pub(crate) fn apply_domestic(builder: ClientBuilder) -> ClientBuilder {
    let settings = current_settings();
    match build_proxy(&settings) {
        Ok(AppliedProxy::Custom(proxy)) => builder.proxy(*proxy),
        Ok(AppliedProxy::System) | Ok(AppliedProxy::Direct) | Err(_) => builder.no_proxy(),
    }
}

fn configure(
    builder: ClientBuilder,
    settings: &ProxySettings,
) -> Result<ClientBuilder, LauncherError> {
    match build_proxy(settings)? {
        AppliedProxy::System => Ok(builder),
        AppliedProxy::Direct => Ok(builder.no_proxy()),
        AppliedProxy::Custom(proxy) => Ok(builder.proxy(*proxy)),
    }
}

fn build_proxy(settings: &ProxySettings) -> Result<AppliedProxy, LauncherError> {
    match settings.mode {
        ProxyMode::System => Ok(AppliedProxy::System),
        ProxyMode::Direct => Ok(AppliedProxy::Direct),
        ProxyMode::Custom => {
            let url = normalize_proxy_url(settings.url.as_deref().unwrap_or(""))?;
            let proxy = reqwest::Proxy::all(&url)
                .map_err(|_| LauncherError::Custom("代理地址无效".to_string()))?;
            Ok(AppliedProxy::Custom(Box::new(proxy)))
        }
    }
}

fn normalize_proxy_url(raw: &str) -> Result<String, LauncherError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(LauncherError::Custom("代理地址不能为空".to_string()));
    }
    let candidate = if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("http://{}", trimmed)
    };
    let url = reqwest::Url::parse(&candidate)
        .map_err(|_| LauncherError::Custom("代理地址格式无效".to_string()))?;
    match url.scheme() {
        "http" | "https" | "socks4" | "socks4a" | "socks5" | "socks5h" => {}
        other => {
            return Err(LauncherError::Custom(format!(
                "不支持的代理协议: {}，可用 http、https、socks5",
                other
            )));
        }
    }
    if url.host_str().is_none() {
        return Err(LauncherError::Custom("代理地址缺少主机名".to_string()));
    }
    Ok(candidate)
}

fn mask_proxy_url(raw: &str) -> String {
    match reqwest::Url::parse(raw) {
        Ok(mut url) => {
            if url.password().is_some() {
                let _ = url.set_password(Some("***"));
            }
            url.to_string()
        }
        Err(_) => "无效地址".to_string(),
    }
}

fn describe(settings: &ProxySettings) -> String {
    match settings.mode {
        ProxyMode::System => "跟随系统代理".to_string(),
        ProxyMode::Direct => "直连".to_string(),
        ProxyMode::Custom => match settings.url.as_deref() {
            Some(url) if !url.trim().is_empty() => format!("自定义代理 {}", mask_proxy_url(url)),
            _ => "自定义代理（未填写地址）".to_string(),
        },
    }
}

fn summarize_request_error(error: &reqwest::Error) -> String {
    if error.is_timeout() {
        "连接超时".to_string()
    } else if error.is_connect() {
        "无法连接代理服务器".to_string()
    } else {
        error.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_adds_http_scheme_when_missing() {
        assert_eq!(
            normalize_proxy_url("127.0.0.1:7890").unwrap(),
            "http://127.0.0.1:7890"
        );
    }

    #[test]
    fn normalize_keeps_supported_schemes() {
        assert_eq!(
            normalize_proxy_url("https://proxy.example.com:443").unwrap(),
            "https://proxy.example.com:443"
        );
        assert_eq!(
            normalize_proxy_url("socks5://127.0.0.1:1080").unwrap(),
            "socks5://127.0.0.1:1080"
        );
    }

    #[test]
    fn normalize_trims_whitespace() {
        assert_eq!(
            normalize_proxy_url("  http://127.0.0.1:7890  ").unwrap(),
            "http://127.0.0.1:7890"
        );
    }

    #[test]
    fn normalize_rejects_empty_url() {
        assert!(normalize_proxy_url("   ").is_err());
    }

    #[test]
    fn normalize_rejects_unsupported_scheme() {
        assert!(normalize_proxy_url("ftp://127.0.0.1:21").is_err());
    }

    #[test]
    fn normalize_rejects_url_without_host() {
        assert!(normalize_proxy_url("http://").is_err());
    }

    #[test]
    fn normalize_keeps_credentials() {
        assert_eq!(
            normalize_proxy_url("user:secret@127.0.0.1:7890").unwrap(),
            "http://user:secret@127.0.0.1:7890"
        );
    }

    #[test]
    fn mask_hides_password() {
        let masked = mask_proxy_url("http://user:secret@127.0.0.1:7890");
        assert!(!masked.contains("secret"));
        assert!(masked.contains("***"));
        assert!(masked.contains("127.0.0.1:7890"));
    }

    #[test]
    fn system_and_direct_modes_do_not_need_url() {
        assert!(matches!(
            build_proxy(&ProxySettings {
                mode: ProxyMode::System,
                url: None,
            }),
            Ok(AppliedProxy::System)
        ));
        assert!(matches!(
            build_proxy(&ProxySettings {
                mode: ProxyMode::Direct,
                url: None,
            }),
            Ok(AppliedProxy::Direct)
        ));
    }

    #[test]
    fn custom_mode_without_url_is_rejected() {
        assert!(
            build_proxy(&ProxySettings {
                mode: ProxyMode::Custom,
                url: None,
            })
            .is_err()
        );
    }
}
