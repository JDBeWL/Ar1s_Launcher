//! Modrinth API 模块
//!
//! 封装 api.modrinth.com/v2 的搜索与项目接口，请求失败按指数退避重试 3 次

use crate::errors::LauncherError;
use crate::models::DownloadJob;
use crate::models::modpack::*;
use crate::services::download;
use crate::services::http_client;
use log::{debug, warn};
use reqwest::Client;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

const MODRINTH_API_BASE: &str = "https://api.modrinth.com/v2";
const MAX_API_RETRIES: u32 = 3;

/// 这些状态码值得重试：限流与临时性服务端故障
///
/// 404 / 400 / 401 这类是请求本身有问题，重试只会重复失败并浪费退避时间
fn is_retryable_status(status: reqwest::StatusCode) -> bool {
    status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error()
}

/// 指数退避：500ms、1s、2s……上限 8 秒，别让界面等太久
fn exponential_backoff(attempt: u32) -> std::time::Duration {
    let millis = 500u64.saturating_mul(1u64 << attempt.min(4));
    std::time::Duration::from_millis(millis.min(8_000))
}

/// 读 `Retry-After`：Modrinth 在限流时会给出剩余秒数
///
/// 只认秒数形式（HTTP 日期形式对这里没意义）；缺省或解析不出时返回 None，由调用方退回指数退避，上限 30 秒
fn retry_after(response: &reqwest::Response) -> Option<std::time::Duration> {
    let raw = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim();

    let seconds: u64 = raw.parse().ok()?;
    Some(std::time::Duration::from_secs(seconds.min(30)))
}

pub struct ModrinthService;

/// 项目在客户端/服务端的可用性：required / optional / unsupported
#[derive(Debug, Clone, Default)]
pub struct ProjectSides {
    pub client: Option<String>,
    pub server: Option<String>,
}

impl Default for ModrinthService {
    fn default() -> Self {
        Self::new()
    }
}

impl ModrinthService {
    pub fn new() -> Self {
        Self
    }

    /// 带退避重试地发一次请求：429 优先遵循 `Retry-After`，其余可重试的状态码按指数退避
    ///
    /// 批量接口（version_files、projects）最容易撞上速率限制，只重试传输层错误不够
    async fn retry_request(
        &self,
        request_fn: impl Fn(&Client) -> reqwest::RequestBuilder,
    ) -> Result<reqwest::Response, LauncherError> {
        let client = http_client::get_client();
        let mut last_err: Option<String> = None;

        for attempt in 0..=MAX_API_RETRIES {
            let builder = request_fn(&client);
            match builder.send().await {
                Ok(resp) => {
                    let status = resp.status();

                    // 其余的 4xx（404、400 之类）是请求本身的问题，重试没有意义，直接交回去
                    if status.is_success() || !is_retryable_status(status) {
                        return Ok(resp);
                    }

                    let wait = match retry_after(&resp) {
                        Some(duration) => duration,
                        None => exponential_backoff(attempt),
                    };

                    if attempt < MAX_API_RETRIES {
                        warn!(
                            "Modrinth API 返回 {} (尝试 {}/{}), {}ms 后重试",
                            status,
                            attempt + 1,
                            MAX_API_RETRIES + 1,
                            wait.as_millis()
                        );
                        tokio::time::sleep(wait).await;
                    } else {
                        warn!("Modrinth API 返回 {}，已达重试上限", status);
                    }
                    last_err = Some(format!("HTTP {}", status));
                }
                Err(e) => {
                    if attempt < MAX_API_RETRIES {
                        let backoff = exponential_backoff(attempt);
                        warn!(
                            "Modrinth API 请求失败 (尝试 {}/{}): {}, {}ms 后重试",
                            attempt + 1,
                            MAX_API_RETRIES + 1,
                            e,
                            backoff.as_millis()
                        );
                        tokio::time::sleep(backoff).await;
                    }
                    last_err = Some(e.to_string());
                }
            }
        }

        Err(LauncherError::Custom(format!(
            "Modrinth API 请求失败 (已重试 {} 次): {}",
            MAX_API_RETRIES,
            last_err.unwrap_or_else(|| "未知错误".to_string())
        )))
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn search_modpacks(
        &self,
        query: Option<String>,
        game_versions: Option<Vec<String>>,
        loaders: Option<Vec<String>>,
        categories: Option<Vec<String>>,
        limit: Option<u32>,
        offset: Option<u32>,
        sort_by: Option<String>,
    ) -> Result<ModrinthSearchResponse, LauncherError> {
        self.search_projects(
            "modpack",
            query,
            game_versions,
            loaders,
            categories,
            limit,
            offset,
            sort_by,
        )
        .await
    }

    /// 按项目类型搜索，整合包与模组共用同一套 facets 逻辑
    #[allow(clippy::too_many_arguments)]
    pub async fn search_projects(
        &self,
        project_type: &str,
        query: Option<String>,
        game_versions: Option<Vec<String>>,
        loaders: Option<Vec<String>>,
        categories: Option<Vec<String>>,
        limit: Option<u32>,
        offset: Option<u32>,
        sort_by: Option<String>,
    ) -> Result<ModrinthSearchResponse, LauncherError> {
        self.search_projects_with_loader_mode(
            project_type,
            query,
            game_versions,
            loaders,
            categories,
            limit,
            offset,
            sort_by,
            false,
        )
        .await
    }

    /// `loaders_any` 为 true 时把 loader 列表放进同一组（OR），用于光影包这种
    /// 「Iris 或 OptiFine 任一能用就行」的场景
    #[allow(clippy::too_many_arguments)]
    pub async fn search_projects_with_loader_mode(
        &self,
        project_type: &str,
        query: Option<String>,
        game_versions: Option<Vec<String>>,
        loaders: Option<Vec<String>>,
        categories: Option<Vec<String>>,
        limit: Option<u32>,
        offset: Option<u32>,
        sort_by: Option<String>,
        loaders_any: bool,
    ) -> Result<ModrinthSearchResponse, LauncherError> {
        let mut params = HashMap::new();

        let search_query = query.unwrap_or_else(|| "*".to_string());
        params.insert("query", search_query);

        if let Some(sort_val) = sort_by {
            params.insert("index", sort_val);
        }

        // facets 是分组数组：外层 AND、组内 OR
        let mut facets_groups: Vec<Vec<String>> =
            vec![vec![format!("project_type:{}", project_type)]];

        if let Some(versions) = game_versions {
            // 同组放多个版本才是 OR；这里每个版本单独一组（AND，更严格）
            for v in &versions {
                facets_groups.push(vec![format!("versions:{}", v)]);
            }
            debug!("添加游戏版本过滤: {:?}", versions);
        }

        if let Some(loader_list) = loaders {
            if loaders_any {
                // 同组才是 OR：光影包同时支持 iris 与 optifine，不能要求两个都命中
                let group = loader_list
                    .iter()
                    .map(|loader| format!("categories:{}", loader))
                    .collect::<Vec<_>>();
                if !group.is_empty() {
                    facets_groups.push(group);
                }
            } else {
                for loader in &loader_list {
                    facets_groups.push(vec![format!("categories:{}", loader)]);
                }
            }
            debug!("添加加载器过滤: {:?}", loader_list);
        }

        if let Some(category_list) = categories {
            for category in &category_list {
                facets_groups.push(vec![format!("categories:{}", category)]);
            }
            debug!("添加分类过滤: {:?}", category_list);
        }

        let facets_json = serde_json::to_string(&facets_groups)?;
        params.insert("facets", facets_json.clone());
        debug!("生成的 facets 参数: {}", facets_json);

        params.insert("limit", limit.unwrap_or(20).to_string());
        params.insert("offset", offset.unwrap_or(0).to_string());

        let url = format!("{}/search", MODRINTH_API_BASE);
        let response = self
            .retry_request(|client| client.get(&url).query(&params))
            .await
            .map_err(|e| LauncherError::Custom(format!("搜索失败: {}", e)))?;

        if !response.status().is_success() {
            return Err(LauncherError::Custom(format!(
                "Modrinth API返回错误: {}",
                response.status()
            )));
        }

        let json_response: Value = response
            .json()
            .await
            .map_err(|e| LauncherError::Custom(format!("解析响应失败: {}", e)))?;

        let hits = json_response["hits"]
            .as_array()
            .ok_or_else(|| LauncherError::Custom("无效的响应格式".to_string()))?
            .iter()
            .filter_map(|hit| {
                // 搜索结果可能混入其他类型，只保留请求的项目类型
                match hit.get("project_type").and_then(|v| v.as_str()) {
                    Some(t) if t == project_type => {}
                    _ => return None,
                }
                // 搜索结果的字段名与项目接口不同：游戏版本在 `versions`，loaders 混在 `categories`
                Some(ModrinthModpack {
                    slug: hit["slug"].as_str()?.to_string(),
                    title: hit["title"].as_str()?.to_string(),
                    description: hit["description"].as_str().unwrap_or("").to_string(),
                    icon_url: hit["icon_url"].as_str().map(|s| s.to_string()),
                    author: hit["author"].as_str()?.to_string(),
                    downloads: hit["downloads"].as_u64().unwrap_or(0),
                    date_created: hit["date_created"].as_str()?.to_string(),
                    date_modified: hit["date_modified"].as_str()?.to_string(),
                    latest_version: hit["latest_version"].as_str()?.to_string(),
                    game_versions: hit["versions"]
                        .as_array()
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                                .collect()
                        })
                        .unwrap_or_default(),
                    loaders: hit["categories"]
                        .as_array()
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|v| {
                                    let cat = v.as_str()?;
                                    match cat {
                                        "fabric" | "forge" | "quilt" | "neoforge" => {
                                            Some(cat.to_string())
                                        }
                                        _ => None,
                                    }
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                    categories: hit["categories"]
                        .as_array()
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|v| {
                                    let cat = v.as_str()?;
                                    match cat {
                                        "fabric" | "forge" | "quilt" | "neoforge" => None,
                                        _ => Some(cat.to_string()),
                                    }
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                })
            })
            .collect();

        let total_hits = json_response["total_hits"].as_u64().unwrap_or(0) as u32;

        Ok(ModrinthSearchResponse { hits, total_hits })
    }

    pub async fn get_modpack(&self, slug_or_id: &str) -> Result<ModrinthModpack, LauncherError> {
        let url = format!("{}/project/{}", MODRINTH_API_BASE, slug_or_id);
        let response = self
            .retry_request(|client| client.get(&url))
            .await
            .map_err(|e| LauncherError::Custom(format!("获取整合包信息失败: {}", e)))?;

        if !response.status().is_success() {
            return Err(LauncherError::Custom(format!(
                "获取整合包信息失败: {}",
                response.status()
            )));
        }

        let project: Value = response
            .json()
            .await
            .map_err(|e| LauncherError::Custom(format!("解析响应失败: {}", e)))?;

        Ok(ModrinthModpack {
            slug: project["slug"]
                .as_str()
                .ok_or_else(|| LauncherError::Custom("缺少slug字段".to_string()))?
                .to_string(),
            title: project["title"]
                .as_str()
                .ok_or_else(|| LauncherError::Custom("缺少title字段".to_string()))?
                .to_string(),
            description: project["description"].as_str().unwrap_or("").to_string(),
            icon_url: project["icon_url"].as_str().map(|s| s.to_string()),
            author: project["author"]
                .as_str()
                .or_else(|| project["team"].as_str())
                .unwrap_or("Unknown")
                .to_string(),
            downloads: project["downloads"].as_u64().unwrap_or(0),
            date_created: project["date_created"].as_str().unwrap_or("").to_string(),
            date_modified: project["date_modified"].as_str().unwrap_or("").to_string(),
            // 项目接口没有 latest_version 字段，取 versions（版本 ID 数组）的第一个
            latest_version: project["versions"]
                .as_array()
                .and_then(|arr| arr.first())
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            game_versions: project["game_versions"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default(),
            loaders: project["loaders"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default(),
            categories: project["categories"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default(),
        })
    }

    pub async fn get_modpack_versions(
        &self,
        project_id: &str,
        game_versions: Option<Vec<String>>,
        loaders: Option<Vec<String>>,
    ) -> Result<Vec<ModrinthModpackVersion>, LauncherError> {
        let mut params = HashMap::new();

        if let Some(versions) = game_versions {
            params.insert("game_versions", serde_json::to_string(&versions)?);
        }

        if let Some(loader_list) = loaders {
            params.insert("loaders", serde_json::to_string(&loader_list)?);
        }

        let url = format!("{}/project/{}/version", MODRINTH_API_BASE, project_id);
        let response = self
            .retry_request(|client| client.get(&url).query(&params))
            .await
            .map_err(|e| LauncherError::Custom(format!("获取整合包版本失败: {}", e)))?;

        if !response.status().is_success() {
            return Err(LauncherError::Custom(format!(
                "获取整合包版本失败: {}",
                response.status()
            )));
        }

        let versions: Vec<Value> = response
            .json()
            .await
            .map_err(|e| LauncherError::Custom(format!("解析响应失败: {}", e)))?;

        versions.iter().map(parse_version).collect()
    }

    /// 按项目取版本列表（模组与整合包通用）
    pub async fn get_project_versions(
        &self,
        project_id: &str,
        game_versions: Option<Vec<String>>,
        loaders: Option<Vec<String>>,
    ) -> Result<Vec<ModrinthModpackVersion>, LauncherError> {
        self.get_modpack_versions(project_id, game_versions, loaders)
            .await
    }

    /// 按版本 ID 取单个版本，安装依赖与更新时用
    pub async fn get_version(
        &self,
        version_id: &str,
    ) -> Result<ModrinthModpackVersion, LauncherError> {
        let url = format!("{}/version/{}", MODRINTH_API_BASE, version_id);
        let response = self
            .retry_request(|client| client.get(&url))
            .await
            .map_err(|e| LauncherError::Custom(format!("获取版本信息失败: {}", e)))?;

        if !response.status().is_success() {
            return Err(LauncherError::Custom(format!(
                "获取版本信息失败: {}",
                response.status()
            )));
        }

        let version: Value = response
            .json()
            .await
            .map_err(|e| LauncherError::Custom(format!("解析响应失败: {}", e)))?;
        parse_version(&version)
    }

    /// 批量查更新：本地文件的 sha1 交给 Modrinth，返回每个文件当前对应的最新版本
    pub async fn check_updates(
        &self,
        hashes: Vec<String>,
        game_versions: Vec<String>,
        loaders: Vec<String>,
    ) -> Result<HashMap<String, ModrinthModpackVersion>, LauncherError> {
        if hashes.is_empty() {
            return Ok(HashMap::new());
        }

        let url = format!("{}/version_files/update", MODRINTH_API_BASE);
        let body = serde_json::json!({
            "hashes": hashes,
            "algorithm": "sha1",
            "game_versions": game_versions,
            "loaders": loaders,
        });

        let response = self
            .retry_request(|client| client.post(&url).json(&body))
            .await
            .map_err(|e| LauncherError::Custom(format!("检查更新失败: {}", e)))?;

        if !response.status().is_success() {
            return Err(LauncherError::Custom(format!(
                "检查更新失败: {}",
                response.status()
            )));
        }

        let versions: HashMap<String, Value> = response
            .json()
            .await
            .map_err(|e| LauncherError::Custom(format!("解析响应失败: {}", e)))?;

        let mut result = HashMap::new();
        for (hash, value) in versions {
            match parse_version(&value) {
                Ok(version) => {
                    result.insert(hash, version);
                }
                Err(error) => warn!("跳过无法解析的版本: {}", error),
            }
        }
        Ok(result)
    }

    /// 批量按 sha1 反查文件所属版本
    ///
    /// 导出整合包时用：本地模组只要是 Modrinth 上有的文件就按哈希查回版本，写进 index 的 files，安装时走官方 CDN
    pub async fn get_versions_by_hashes(
        &self,
        hashes: Vec<String>,
    ) -> Result<HashMap<String, ModrinthModpackVersion>, LauncherError> {
        if hashes.is_empty() {
            return Ok(HashMap::new());
        }

        let url = format!("{}/version_files", MODRINTH_API_BASE);
        let body = serde_json::json!({
            "hashes": hashes,
            "algorithm": "sha1",
        });

        let response = self
            .retry_request(|client| client.post(&url).json(&body))
            .await
            .map_err(|e| LauncherError::Custom(format!("查询文件所属版本失败: {}", e)))?;

        if !response.status().is_success() {
            return Err(LauncherError::Custom(format!(
                "查询文件所属版本失败: {}",
                response.status()
            )));
        }

        let versions: HashMap<String, Value> = response
            .json()
            .await
            .map_err(|e| LauncherError::Custom(format!("解析响应失败: {}", e)))?;

        let mut result = HashMap::new();
        for (hash, value) in versions {
            match parse_version(&value) {
                Ok(version) => {
                    result.insert(hash.to_ascii_lowercase(), version);
                }
                Err(error) => warn!("跳过无法解析的版本: {}", error),
            }
        }
        Ok(result)
    }

    /// 批量取项目的端侧支持情况（client_side / server_side），导出时写进 index 的 env 字段
    pub async fn get_project_sides(
        &self,
        project_ids: &[String],
    ) -> Result<HashMap<String, ProjectSides>, LauncherError> {
        if project_ids.is_empty() {
            return Ok(HashMap::new());
        }

        let url = format!("{}/projects", MODRINTH_API_BASE);
        let ids = serde_json::to_string(project_ids)?;
        let response = self
            .retry_request(|client| client.get(&url).query(&[("ids", ids.as_str())]))
            .await
            .map_err(|e| LauncherError::Custom(format!("获取项目信息失败: {}", e)))?;

        if !response.status().is_success() {
            return Err(LauncherError::Custom(format!(
                "获取项目信息失败: {}",
                response.status()
            )));
        }

        let projects: Vec<Value> = response
            .json()
            .await
            .map_err(|e| LauncherError::Custom(format!("解析响应失败: {}", e)))?;

        let mut result = HashMap::new();
        for project in projects {
            let Some(id) = project["id"].as_str() else {
                continue;
            };
            result.insert(
                id.to_string(),
                ProjectSides {
                    client: project["client_side"].as_str().map(str::to_string),
                    server: project["server_side"].as_str().map(str::to_string),
                },
            );
        }
        Ok(result)
    }

    pub async fn download_modpack_file(
        &self,
        file_url: &str,
        destination: &std::path::Path,
        size: u64,
        sha1: Option<&str>,
        global_cancel: &Arc<AtomicBool>,
    ) -> Result<(), LauncherError> {
        if global_cancel.load(Ordering::Acquire) {
            return Err(LauncherError::Cancelled);
        }

        let job = DownloadJob {
            url: file_url.to_string(),
            fallback_urls: Vec::new(),
            path: destination.to_path_buf(),
            size,
            hash: sha1
                .filter(|hash| !hash.trim().is_empty())
                .map(str::to_string),
        };
        let state = Arc::new(AtomicBool::new(true));
        let bytes_downloaded = Arc::new(AtomicU64::new(0));
        let bytes_since_last = Arc::new(AtomicU64::new(0));
        let http = download::get_http_client();

        let mut last_error = None;
        for attempt in 0..3 {
            if global_cancel.load(Ordering::Acquire) {
                return Err(LauncherError::Cancelled);
            }

            match download::download_file(
                &http,
                &job,
                file_url,
                &state,
                global_cancel,
                &bytes_downloaded,
                &bytes_since_last,
            )
            .await
            {
                Ok(()) => return Ok(()),
                Err(e) => {
                    if e.is_cancelled() {
                        return Err(e);
                    }
                    warn!("整合包文件下载失败 (第 {} 次): {}", attempt + 1, e);
                    last_error = Some(e);
                    if attempt < 2 {
                        tokio::time::sleep(std::time::Duration::from_secs(1 << attempt)).await;
                    }
                }
            }
        }

        Err(last_error.unwrap_or_else(|| LauncherError::Custom("整合包文件下载失败".to_string())))
    }
}

/// 把 Modrinth 的 version JSON 解析成模型；项目版本列表、单版本、更新检查共用
fn parse_version(version: &Value) -> Result<ModrinthModpackVersion, LauncherError> {
    Ok(ModrinthModpackVersion {
        id: version["id"]
            .as_str()
            .ok_or_else(|| LauncherError::Custom("缺少id字段".to_string()))?
            .to_string(),
        // 单版本接口一定会给 project_id；旧数据缺失时留空，不影响安装
        project_id: version["project_id"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        name: version["name"]
            .as_str()
            .ok_or_else(|| LauncherError::Custom("缺少name字段".to_string()))?
            .to_string(),
        version_number: version["version_number"]
            .as_str()
            .ok_or_else(|| LauncherError::Custom("缺少version_number字段".to_string()))?
            .to_string(),
        game_versions: version["game_versions"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default(),
        loaders: version["loaders"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default(),
        featured: version["featured"].as_bool().unwrap_or(false),
        date_published: version["date_published"]
            .as_str()
            .ok_or_else(|| LauncherError::Custom("缺少date_published字段".to_string()))?
            .to_string(),
        downloads: version["downloads"].as_u64().unwrap_or(0),
        files: version["files"]
            .as_array()
            .map(|files| {
                files
                    .iter()
                    .filter_map(|file| {
                        Some(ModrinthFile {
                            url: file["url"].as_str()?.to_string(),
                            filename: file["filename"].as_str()?.to_string(),
                            primary: file["primary"].as_bool().unwrap_or(false),
                            size: file["size"].as_u64().unwrap_or(0),
                            hashes: ModrinthHashes {
                                sha1: file["hashes"]["sha1"].as_str()?.to_string(),
                                sha512: file["hashes"]["sha512"].as_str()?.to_string(),
                            },
                        })
                    })
                    .collect()
            })
            .unwrap_or_default(),
        dependencies: version["dependencies"]
            .as_array()
            .map(|deps| {
                deps.iter()
                    .filter_map(|dep| {
                        Some(ModrinthDependency {
                            version_id: dep["version_id"].as_str().map(|s| s.to_string()),
                            project_id: dep["project_id"].as_str().map(|s| s.to_string()),
                            dependency_type: dep["dependency_type"].as_str()?.to_string(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::{exponential_backoff, is_retryable_status, parse_version};
    use reqwest::StatusCode;
    use serde_json::json;

    /// 只有限流和 5xx 值得重试；4xx 是请求本身的问题
    #[test]
    fn test_only_rate_limit_and_server_errors_are_retried() {
        assert!(is_retryable_status(StatusCode::TOO_MANY_REQUESTS));
        assert!(is_retryable_status(StatusCode::INTERNAL_SERVER_ERROR));
        assert!(is_retryable_status(StatusCode::BAD_GATEWAY));
        assert!(is_retryable_status(StatusCode::SERVICE_UNAVAILABLE));
        assert!(is_retryable_status(StatusCode::GATEWAY_TIMEOUT));

        assert!(!is_retryable_status(StatusCode::OK));
        assert!(!is_retryable_status(StatusCode::NOT_FOUND));
        assert!(!is_retryable_status(StatusCode::BAD_REQUEST));
        assert!(!is_retryable_status(StatusCode::UNAUTHORIZED));
    }

    #[test]
    fn test_backoff_grows_and_is_capped() {
        assert_eq!(exponential_backoff(0).as_millis(), 500);
        assert_eq!(exponential_backoff(1).as_millis(), 1_000);
        assert_eq!(exponential_backoff(2).as_millis(), 2_000);
        assert_eq!(exponential_backoff(3).as_millis(), 4_000);
        // 封顶 8 秒：再往后也不该让用户干等
        assert_eq!(exponential_backoff(4).as_millis(), 8_000);
        assert_eq!(exponential_backoff(16).as_millis(), 8_000);
    }

    #[test]
    fn test_parse_version_keeps_files_and_dependencies() {
        let value = json!({
            "id": "ver1",
            "project_id": "proj1",
            "name": "Sodium 0.5.8",
            "version_number": "0.5.8",
            "game_versions": ["1.20.1"],
            "loaders": ["fabric"],
            "featured": false,
            "date_published": "2024-04-01T00:00:00Z",
            "downloads": 123,
            "files": [{
                "url": "https://cdn.modrinth.com/x.jar",
                "filename": "sodium.jar",
                "primary": true,
                "size": 10,
                "hashes": { "sha1": "abc", "sha512": "def" }
            }],
            "dependencies": [{
                "version_id": "dep1",
                "project_id": "depProj",
                "dependency_type": "required"
            }]
        });

        let version = parse_version(&value).unwrap();
        assert_eq!(version.id, "ver1");
        assert_eq!(version.project_id, "proj1");
        assert_eq!(version.files.len(), 1);
        assert!(version.files[0].primary);
        assert_eq!(version.dependencies[0].dependency_type, "required");
    }
}
