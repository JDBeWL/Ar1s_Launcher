//! 下载 HTTP 客户端
//!
//! 独立于全局客户端的大文件专用 reqwest：禁用压缩、只设读超时（不限制总时长）；
//! 代理设置变更后可整体重建

use std::sync::{LazyLock, RwLock};
use std::time::Duration;

const DEFAULT_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36 Ar1sLauncher/1.0";

static HTTP_CLIENT: LazyLock<RwLock<reqwest::Client>> =
    LazyLock::new(|| RwLock::new(create_client(16)));

/// 返回客户端副本：内部是 Arc，克隆很轻；正在进行的下载继续用旧客户端
pub fn get_http_client() -> reqwest::Client {
    HTTP_CLIENT
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

/// TUNA 对浏览器 UA 的直链请求回 403，Java 包下载用简洁 UA；代理仍沿用当前设置
pub fn get_java_download_client() -> reqwest::Client {
    build_client(16, "Ar1sLauncher/1.0")
}

pub(crate) fn reload() {
    let client = create_client(16);
    *HTTP_CLIENT
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = client;
}

fn create_client(max_connections_per_host: usize) -> reqwest::Client {
    build_client(max_connections_per_host, DEFAULT_USER_AGENT)
}

fn build_client(max_connections_per_host: usize, user_agent: &'static str) -> reqwest::Client {
    match crate::services::proxy::apply(client_builder(max_connections_per_host, user_agent))
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            log::error!(
                "创建下载 HTTP 客户端失败，回退为不启用代理的客户端: {}",
                error
            );
            client_builder(max_connections_per_host, user_agent)
                .build()
                .unwrap_or_else(|fallback_error| {
                    log::error!(
                        "创建默认下载 HTTP 客户端仍失败，使用 reqwest 默认值: {}",
                        fallback_error
                    );
                    reqwest::Client::new()
                })
        }
    }
}

fn client_builder(
    max_connections_per_host: usize,
    user_agent: &'static str,
) -> reqwest::ClientBuilder {
    let mut default_headers = reqwest::header::HeaderMap::new();
    default_headers.insert(
        reqwest::header::USER_AGENT,
        reqwest::header::HeaderValue::from_static(user_agent),
    );
    default_headers.insert(
        reqwest::header::ACCEPT_ENCODING,
        reqwest::header::HeaderValue::from_static("identity"),
    );

    reqwest::Client::builder()
        .default_headers(default_headers)
        .no_gzip()
        .no_brotli()
        .no_deflate()
        .pool_max_idle_per_host(max_connections_per_host * 4)
        .pool_idle_timeout(Duration::from_secs(90))
        .tcp_keepalive(Some(Duration::from_secs(60)))
        .tcp_nodelay(true)
        .connect_timeout(Duration::from_secs(10))
        // 不能用 .timeout()：那是整个请求的总时长，大文件在慢线路上必然超时重下。
        // 读超时只管「卡住不动」，只要还在出数据就让它继续下
        .read_timeout(Duration::from_secs(60))
}
