//! CurseForge 数据源模块
//!
//! 经麦块开放 API（api.minekuai.cn）免 key 检索 CurseForge 的模组与整合包，
//! 响应统一转换成 Modrinth 同构模型，安装流水线与前端无需感知平台差异。
//! 该 API 限流为每 IP 600 次/分钟，这里用独立令牌桶收着自己的请求节奏，
//! 不与 Modrinth 或文件下载共享额度；解析旧文件时按项目回落 cfwidget 全量列表。

use crate::errors::LauncherError;
use crate::models::modpack::*;
use crate::services::http_client;
use log::{info, warn};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, RwLock};
use std::time::{Duration, Instant};

const MINEKUAI_API_BASE: &str = "https://api.minekuai.cn/panel/public";
const CFWIDGET_API_BASE: &str = "https://api.cfwidget.com";
const MAX_API_RETRIES: u32 = 3;

// ---------------------------------------------------------------------------
// minekuai 专用客户端：不跟随系统代理
// ---------------------------------------------------------------------------
// minekuai 是国内直连可达的服务；系统代理（常见为翻墙代理）会把请求绕到境外出口，
// 清单解析的大并发会把代理节点打崩（实测系统代理全量劫持时 versions 查询全军覆没）。
// 只有用户显式配置的自定义代理才跟随
static MINEKUAI_CLIENT: LazyLock<RwLock<reqwest::Client>> =
    LazyLock::new(|| RwLock::new(build_minekuai_client()));

fn minekuai_client() -> reqwest::Client {
    MINEKUAI_CLIENT
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

/// 代理设置切换后由 proxy 模块回调重建
pub fn reload_minekuai_client() {
    let client = build_minekuai_client();
    *MINEKUAI_CLIENT
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = client;
}

fn build_minekuai_client() -> reqwest::Client {
    match crate::services::proxy::apply_domestic(
        reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(10))
            .pool_max_idle_per_host(10)
            .pool_idle_timeout(Duration::from_secs(90))
            .user_agent(http_client::USER_AGENT),
    )
    .build()
    {
        Ok(client) => client,
        Err(error) => {
            warn!("创建 minekuai 直连客户端失败，回退为默认客户端: {}", error);
            reqwest::Client::new()
        }
    }
}

/// 资源组：路径前缀决定查的是模组还是整合包
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CfResource {
    Mods,
    Modpacks,
}

impl CfResource {
    fn path(self) -> &'static str {
        match self {
            CfResource::Mods => "mc-cf-mods",
            CfResource::Modpacks => "mc-cf-modpacks",
        }
    }
}

// ---------------------------------------------------------------------------
// 独立限流：令牌桶，600 次/分钟 → 每秒补 10 个，桶容量 30 允许搜索翻页的小突发
// ---------------------------------------------------------------------------

pub struct TokenBucket {
    state: Mutex<BucketState>,
    capacity: f64,
    refill_per_sec: f64,
}

struct BucketState {
    tokens: f64,
    last: Instant,
}

impl TokenBucket {
    pub fn new(per_minute: u32, burst: u32) -> Self {
        let capacity = burst as f64;
        Self {
            state: Mutex::new(BucketState {
                tokens: capacity,
                last: Instant::now(),
            }),
            capacity,
            refill_per_sec: per_minute as f64 / 60.0,
        }
    }

    /// 计算当前时刻取一个令牌要等多久；返回零时长表示立即扣减成功
    ///
    /// 与 `acquire` 拆开是为了让令牌数学可以脱离真实时钟做单测
    fn next_wait(&self, now: Instant, state: &mut BucketState) -> Duration {
        let elapsed = now
            .saturating_duration_since(state.last)
            .as_secs_f64()
            .min(self.capacity / self.refill_per_sec);
        state.tokens = (state.tokens + elapsed * self.refill_per_sec).min(self.capacity);
        state.last = now;

        if state.tokens >= 1.0 {
            state.tokens -= 1.0;
            Duration::ZERO
        } else {
            let deficit = 1.0 - state.tokens;
            Duration::from_secs_f64(deficit / self.refill_per_sec)
        }
    }

    pub async fn acquire(&self) {
        loop {
            let wait = {
                let mut state = self
                    .state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                self.next_wait(Instant::now(), &mut state)
            };
            if wait.is_zero() {
                return;
            }
            tokio::time::sleep(wait).await;
        }
    }
}

static MINEKUAI_LIMITER: LazyLock<TokenBucket> = LazyLock::new(|| TokenBucket::new(600, 30));

// ---------------------------------------------------------------------------
// 数据模型
// ---------------------------------------------------------------------------

/// CF 文件条目：模组版本与整合包安装包共用同一结构
#[derive(Debug, Clone, Default)]
pub struct CfFile {
    pub id: u64,
    pub project_id: u64,
    pub display_name: String,
    pub file_name: String,
    pub release_type: u8,
    pub file_date: String,
    pub file_length: u64,
    /// 受限项目为空串：作者禁止第三方分发时上游不给直链
    pub download_url: String,
    pub sha1: String,
    pub game_versions: Vec<String>,
}

/// manifest 里待解析的条目
#[derive(Debug, Clone, Copy)]
pub struct CfManifestRef {
    pub project_id: u64,
    pub file_id: u64,
}

/// 解析后可直接进下载队列的文件；name 供进度展示
#[derive(Debug, Clone)]
pub struct CfResolvedFile {
    pub file: CfFile,
}

/// CF 的分类标签：slug 用于筛选，name 用于展示。
/// CF 与 Modrinth 的 taxonomy 完全不同（Modrinth 的 technology 在 CF 叫 tech），
/// 前端的分类选项必须按源分别拉取
#[derive(Debug, Clone, serde::Serialize, PartialEq)]
pub struct CfCategory {
    pub slug: String,
    pub name: String,
}

/// 解析不到直链的文件：受限或上游缺失，提示用户手动下载
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CfManualFile {
    pub name: String,
    pub url: String,
}

/// 由链接/slug/数字 ID 解析出的项目引用。
/// minekuai 的搜索库收录不全（部分新包搜不到但 versions 端点有数据），
/// 拿到数字 ID 就能绕过搜索库直接进版本列表与安装
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CfProjectRef {
    pub project_id: String,
    pub title: String,
    pub icon_url: Option<String>,
}

pub struct CurseForgeService;

impl Default for CurseForgeService {
    fn default() -> Self {
        Self::new()
    }
}

fn is_retryable_status(status: reqwest::StatusCode) -> bool {
    // 502 是文档明说的上游故障，稍后重试即可；400/404 是请求本身的问题
    status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error()
}

fn exponential_backoff(attempt: u32) -> Duration {
    let millis = 500u64.saturating_mul(1u64 << attempt.min(4));
    Duration::from_millis(millis.min(8_000))
}

impl CurseForgeService {
    pub fn new() -> Self {
        Self
    }

    /// 取一个令牌再发请求；429/5xx 按指数退避重试
    async fn get_json(&self, url: &str, query: &[(&str, String)]) -> Result<Value, LauncherError> {
        let mut last_err: Option<String> = None;

        for attempt in 0..=MAX_API_RETRIES {
            MINEKUAI_LIMITER.acquire().await;

            let client = minekuai_client();
            match client.get(url).query(query).send().await {
                Ok(resp) => {
                    let status = resp.status();
                    if status.is_success() {
                        let body: Value = resp.json().await.map_err(|e| {
                            LauncherError::Custom(format!("解析 CurseForge 接口响应失败: {}", e))
                        })?;
                        // 文档口径的错误体是 {"error": "..."}；实测未知路由还会返回
                        // HTTP 200 + {"code": 4xx, "msg": "..."}，两种形状都要认
                        if let Some(message) = body.get("error").and_then(|v| v.as_str()) {
                            return Err(LauncherError::Custom(format!(
                                "CurseForge 接口返回错误: {}",
                                message
                            )));
                        }
                        // 成功形状一定带 items / total / title 或是数组；code+msg 的错误形状另判
                        let looks_like_success = body.get("items").is_some()
                            || body.get("total").is_some()
                            || body.get("title").is_some()
                            || body.is_array();
                        if !looks_like_success {
                            let code = body.get("code").and_then(|v| v.as_u64()).unwrap_or(0);
                            let message = body.get("msg").and_then(|v| v.as_str()).unwrap_or("");
                            if code >= 400 && !message.is_empty() {
                                return Err(LauncherError::Custom(format!(
                                    "麦块 API 返回错误 {}: {}",
                                    code, message
                                )));
                            }
                        }
                        return Ok(body);
                    }

                    if !is_retryable_status(status) {
                        return Err(LauncherError::Custom(format!(
                            "CurseForge 接口返回错误: HTTP {}",
                            status
                        )));
                    }

                    let wait = exponential_backoff(attempt);
                    if attempt < MAX_API_RETRIES {
                        warn!(
                            "麦块 API 返回 {} (尝试 {}/{}), {}ms 后重试",
                            status,
                            attempt + 1,
                            MAX_API_RETRIES + 1,
                            wait.as_millis()
                        );
                        tokio::time::sleep(wait).await;
                    }
                    last_err = Some(format!("HTTP {}", status));
                }
                Err(e) => {
                    if attempt < MAX_API_RETRIES {
                        let backoff = exponential_backoff(attempt);
                        warn!(
                            "麦块 API 请求失败 (尝试 {}/{}): {}, {}ms 后重试",
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
            "麦块 API 请求失败 (已重试 {} 次): {}",
            MAX_API_RETRIES,
            last_err.unwrap_or_else(|| "未知错误".to_string())
        )))
    }

    /// 前端的排序值映射到麦块 API 的 order；相关度排序靠不带 order 实现
    fn order_param(sort_by: Option<&str>) -> Option<&'static str> {
        match sort_by {
            Some("downloads") => Some("downloads"),
            Some("newest") => Some("date_created"),
            Some("updated") => Some("date_modified"),
            // CF 侧没有 follows 维度，退到下载数
            Some("follows") | Some("relevance") | None => None,
            _ => None,
        }
    }

    /// 搜索并转成 Modrinth 同构结果；多值参数必须带 `[]` 后缀，否则只有最后一个值生效
    #[allow(clippy::too_many_arguments)]
    pub async fn search(
        &self,
        resource: CfResource,
        query: Option<String>,
        game_versions: Option<Vec<String>>,
        loaders: Option<Vec<String>>,
        categories: Option<Vec<String>>,
        limit: Option<u32>,
        offset: Option<u32>,
        sort_by: Option<String>,
    ) -> Result<ModrinthSearchResponse, LauncherError> {
        let mut params: Vec<(&str, String)> = Vec::new();
        if let Some(text) = query.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
            params.push(("q", text.to_string()));
        }
        if let Some(order) = Self::order_param(sort_by.as_deref()) {
            params.push(("order", order.to_string()));
        }
        for loader in loaders.iter().flatten() {
            params.push(("loader[]", loader.clone()));
        }
        for version in game_versions.iter().flatten() {
            params.push(("game_version[]", version.clone()));
        }
        for category in categories.iter().flatten() {
            params.push(("category[]", category.clone()));
        }
        params.push(("limit", limit.unwrap_or(20).min(1000).to_string()));
        params.push(("offset", offset.unwrap_or(0).to_string()));

        let url = format!("{}/{}", MINEKUAI_API_BASE, resource.path());
        let body = self.get_json(&url, &params).await?;

        let items = body["items"]
            .as_array()
            .ok_or_else(|| LauncherError::Custom("搜索响应缺少 items 字段".to_string()))?;
        let total_hits = body["total"].as_u64().unwrap_or(0) as u32;

        let hits = items.iter().filter_map(parse_project_hit).collect();
        Ok(ModrinthSearchResponse { hits, total_hits })
    }

    /// 项目详情里只取安装需要的字段（标题与图标）。
    /// minekuai 的项目库偶有缺口（新包未收录时 detail 404 但 versions 端点仍有数据），
    /// 数字 ID 在库里查不到时回落 cfwidget 补标题，安装流程不至于因此中断
    pub async fn get_project_summary(
        &self,
        resource: CfResource,
        project_id: &str,
    ) -> Result<(String, Option<String>), LauncherError> {
        let url = format!(
            "{}/{}/detail/{}",
            MINEKUAI_API_BASE,
            resource.path(),
            project_id
        );
        match self.get_json(&url, &[]).await {
            Ok(body) => {
                let title = body["title"]
                    .as_str()
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                let icon = body["icon_url"]
                    .as_str()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string);
                Ok((title, icon))
            }
            Err(minekuai_err) => {
                // slug 形式的 project_id 没法查 cfwidget（它只收数字 ID 或路径），
                // 原样返回错误即可——slug 能走到这里说明项目本就在库外
                if project_id.is_empty() || !project_id.bytes().all(|b| b.is_ascii_digit()) {
                    return Err(minekuai_err);
                }
                let body = cfwidget_get_json(project_id).await.map_err(|cf_err| {
                    LauncherError::Custom(format!(
                        "minekuai 与 cfwidget 均查不到该项目 {}：{}；{}",
                        project_id, cf_err, minekuai_err
                    ))
                })?;
                let title = body["title"]
                    .as_str()
                    .map(str::trim)
                    .unwrap_or_default()
                    .to_string();
                Ok((title, cfwidget_thumbnail(&body)))
            }
        }
    }

    /// 某项目的文件列表（最新 50 条）；模组版本选择与整合包解析共用
    pub async fn get_files(
        &self,
        resource: CfResource,
        project_id: &str,
        game_version: Option<&str>,
    ) -> Result<Vec<CfFile>, LauncherError> {
        let url = format!(
            "{}/{}/detail/{}/versions",
            MINEKUAI_API_BASE,
            resource.path(),
            project_id
        );
        let mut params: Vec<(&str, String)> = Vec::new();
        if let Some(version) = game_version.map(str::trim).filter(|v| !v.is_empty()) {
            params.push(("game_version", version.to_string()));
        }

        let body = self.get_json(&url, &params).await?;
        let entries = body
            .as_array()
            .ok_or_else(|| LauncherError::Custom("版本响应不是数组".to_string()))?;
        Ok(entries.iter().filter_map(parse_file_entry).collect())
    }

    /// 资源组的分类标签表，前端按源展示各自的筛选选项
    pub async fn get_categories(
        &self,
        resource: CfResource,
    ) -> Result<Vec<CfCategory>, LauncherError> {
        let url = format!("{}/{}/categories", MINEKUAI_API_BASE, resource.path());
        let body = self.get_json(&url, &[]).await?;
        let entries = body
            .as_array()
            .ok_or_else(|| LauncherError::Custom("分类响应不是数组".to_string()))?;
        Ok(entries.iter().filter_map(parse_category_entry).collect())
    }

    /// 把用户输入的 CurseForge 链接 / 裸 slug / 数字 ID 解析成项目引用。
    /// slug 经 cfwidget 路径查询反查数字 ID；数字 ID 直接用，
    /// 之后即可走 `get_files` + 安装流水线，不依赖搜索库是否收录
    pub async fn resolve_project(
        &self,
        resource: CfResource,
        input: &str,
    ) -> Result<CfProjectRef, LauncherError> {
        match parse_project_input(input) {
            ProjectInput::Id(id) => {
                let (title, icon_url) = self.get_project_summary(resource, &id).await?;
                Ok(CfProjectRef {
                    project_id: id,
                    title,
                    icon_url,
                })
            }
            ProjectInput::Slug(slug) => {
                let path = match resource {
                    CfResource::Mods => format!("minecraft/mc-mods/{}", slug),
                    CfResource::Modpacks => format!("minecraft/modpacks/{}", slug),
                };
                let body = cfwidget_get_json(&path).await?;
                let id = body["id"]
                    .as_u64()
                    .ok_or_else(|| LauncherError::Custom("cfwidget 响应缺少项目 ID".to_string()))?
                    .to_string();
                let title = body["title"]
                    .as_str()
                    .map(str::trim)
                    .unwrap_or_default()
                    .to_string();
                Ok(CfProjectRef {
                    project_id: id,
                    title,
                    icon_url: cfwidget_thumbnail(&body),
                })
            }
            ProjectInput::Unrecognized => Err(LauncherError::Custom(
                "无法识别的项目引用：需要 CurseForge 链接、slug 或数字 ID".to_string(),
            )),
        }
    }

    /// 按 manifest 的 projectID+fileID 批量解析下载条目
    ///
    /// 麦块 versions 只保留最新 50 个文件，老整合包引用的旧文件要回落 cfwidget 的
    /// 全量列表，再按 CDN 地址规律拼直链（cfwidget 只给文件名不给直链）。
    /// 受限项目（作者禁止第三方分发）拿不到直链，进手动下载清单并附 CF 文件页地址。
    /// 同一项目多个文件只查一次接口；单个项目解析失败直接报错——网络故障时装一半
    /// 比明确失败更糟。
    pub async fn resolve_manifest_files(
        &self,
        refs: &[CfManifestRef],
        mut on_progress: impl FnMut(usize, usize),
    ) -> Result<(Vec<CfResolvedFile>, Vec<CfManualFile>), LauncherError> {
        // 项目 → 该项目要找的 file_id 集合；保持插入顺序让进度稳定
        let mut by_project: Vec<(u64, Vec<u64>)> = Vec::new();
        {
            let mut index: HashMap<u64, usize> = HashMap::new();
            for entry in refs {
                match index.get(&entry.project_id) {
                    Some(&slot) => by_project[slot].1.push(entry.file_id),
                    None => {
                        index.insert(entry.project_id, by_project.len());
                        by_project.push((entry.project_id, vec![entry.file_id]));
                    }
                }
            }
        }

        let total = by_project.len();
        let mut resolved: Vec<CfResolvedFile> = Vec::new();
        let mut manual: Vec<CfManualFile> = Vec::new();
        let mut done = 0usize;

        // 并发压着限流器的补给速度走，再多也只是排队
        let concurrency = 6usize;
        let semaphore = Arc::new(tokio::sync::Semaphore::new(concurrency));
        let mut handles = Vec::with_capacity(total);

        for (project_id, file_ids) in by_project {
            let semaphore = semaphore.clone();
            let service = Self::new();
            handles.push(tokio::spawn(async move {
                let _permit = semaphore
                    .acquire_owned()
                    .await
                    .map_err(|_| LauncherError::Custom("解析任务被中止".to_string()))?;
                service.resolve_project_files(project_id, &file_ids).await
            }));
        }

        for handle in handles {
            // 解析结果按提交顺序汇合，进度回调因此单调递增
            let (files, skipped) = handle
                .await
                .map_err(|e| LauncherError::Custom(format!("解析任务异常终止: {}", e)))??;
            resolved.extend(files);
            manual.extend(skipped);
            done += 1;
            on_progress(done, total);
        }

        Ok((resolved, manual))
    }

    /// 单个项目的 file_id → 文件条目；最新 50 条里找不到就查 cfwidget 全量列表
    async fn resolve_project_files(
        &self,
        project_id: u64,
        file_ids: &[u64],
    ) -> Result<(Vec<CfResolvedFile>, Vec<CfManualFile>), LauncherError> {
        let recent = self
            .get_files(CfResource::Mods, &project_id.to_string(), None)
            .await?;
        let mut found: Vec<CfResolvedFile> = Vec::new();
        let mut stale: Vec<u64> = Vec::new();
        let mut manual: Vec<CfManualFile> = Vec::new();

        for file_id in file_ids {
            match recent.iter().find(|file| file.id == *file_id) {
                Some(file) if !file.download_url.trim().is_empty() => {
                    found.push(CfResolvedFile { file: file.clone() });
                }
                Some(file) => stale.push(file.id),
                None => stale.push(*file_id),
            }
        }

        if stale.is_empty() {
            return Ok((found, manual));
        }

        info!(
            "项目 {} 有 {} 个文件不在最新列表或受限，回落 cfwidget 查询",
            project_id,
            stale.len()
        );
        let (full_files, project_page) = fetch_cfwidget_files(project_id).await?;
        for file_id in stale {
            // 老文件大多只是掉出了最新 50 的窗口：拼 CDN 直链继续自动下载。
            // 受限项目 cfwidget 也查不到对应直链产物，只能引导用户去文件页手动下载
            if let Some((file_name, file_size)) = full_files.get(&file_id)
                && !is_restricted(&recent, file_id)
            {
                let cdn = cdn_file_url(file_id, file_name);
                found.push(CfResolvedFile {
                    file: CfFile {
                        id: file_id,
                        project_id,
                        display_name: file_name.clone(),
                        file_name: file_name.clone(),
                        download_url: cdn,
                        // size 为 0 会让批量下载的总量统计偏小（实际字节照常计入），
                        // cfwidget 的 filesize 正好补上
                        file_length: *file_size,
                        ..CfFile::default()
                    },
                });
            } else {
                let name = full_files
                    .get(&file_id)
                    .map(|(name, _)| name.clone())
                    .unwrap_or_else(|| format!("fileID {}", file_id));
                manual.push(CfManualFile {
                    name,
                    url: format!("{}/files/{}", project_page, file_id),
                });
            }
        }

        Ok((found, manual))
    }

    /// 把 CfFile 转成 Modrinth 同构版本模型，前端版本列表与单模组安装直接复用
    pub fn file_to_modrinth_version(file: &CfFile) -> ModrinthModpackVersion {
        let (loaders, game_versions) = split_game_versions(&file.game_versions);
        ModrinthModpackVersion {
            id: file.id.to_string(),
            project_id: file.project_id.to_string(),
            name: file.display_name.clone(),
            version_number: file.display_name.clone(),
            game_versions,
            loaders,
            featured: file.release_type == 1,
            date_published: file.file_date.clone(),
            downloads: 0,
            files: vec![ModrinthFile {
                url: file.download_url.clone(),
                filename: file.file_name.clone(),
                primary: true,
                size: file.file_length,
                hashes: ModrinthHashes {
                    sha1: file.sha1.clone(),
                    // CF 只提供 sha1 与 md5，sha512 留空由下载层跳过校验
                    sha512: String::new(),
                },
            }],
            dependencies: Vec::new(),
        }
    }
}

/// cfwidget 返回项目的全量文件表（文件名 + 字节数）与文件页前缀；
/// 未缓存项目可能 202，等几秒重试一次
async fn fetch_cfwidget_files(
    project_id: u64,
) -> Result<(HashMap<u64, (String, u64)>, String), LauncherError> {
    let body = cfwidget_get_json(&project_id.to_string()).await?;
    let mut files = HashMap::new();
    if let Some(entries) = body["files"].as_array() {
        for entry in entries {
            let Some(id) = entry["id"].as_u64() else {
                continue;
            };
            let Some(name) = entry["name"].as_str() else {
                continue;
            };
            let size = entry["filesize"].as_u64().unwrap_or(0);
            files.insert(id, (name.to_string(), size));
        }
    }
    let project_page = body["urls"]["project"]
        .as_str()
        .unwrap_or(&format!(
            "https://www.curseforge.com/projects/{}",
            project_id
        ))
        .trim_end_matches('/')
        .to_string();
    Ok((files, project_page))
}

/// cfwidget GET；path 是数字 ID 或 `minecraft/{modpacks|mc-mods}/{slug}` 形式。
/// 未缓存项目先回 202（已进队列），等几秒重试一次
async fn cfwidget_get_json(path: &str) -> Result<Value, LauncherError> {
    let url = format!("{}/{}", CFWIDGET_API_BASE, path);
    // cfwidget 是境外服务，跟随共享客户端的代理设置（与 minekuai 的直连策略相反）
    let client = http_client::get_client();

    for attempt in 0..=1u32 {
        let resp = client
            .get(&url)
            .timeout(Duration::from_secs(20))
            .send()
            .await
            .map_err(|e| LauncherError::Custom(format!("cfwidget 请求失败: {}", e)))?;

        match resp.status().as_u16() {
            200 => {
                return resp
                    .json()
                    .await
                    .map_err(|e| LauncherError::Custom(format!("解析 cfwidget 响应失败: {}", e)));
            }
            // 202 = 已进队列，稍等重试一次；301 = 换了规范路径，按原路径重试大概率还是 301，直接放弃
            202 if attempt == 0 => {
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
            status => {
                return Err(LauncherError::Custom(format!(
                    "cfwidget 返回 HTTP {}",
                    status
                )));
            }
        }
    }

    Err(LauncherError::Custom("cfwidget 排队超时".to_string()))
}

/// cfwidget 的缩略图数组里挑第一张可用的头像；拿不到就返回 None，前端展示占位图标
fn cfwidget_thumbnail(body: &Value) -> Option<String> {
    body["thumbnails"].as_array()?.iter().find_map(|item| {
        item["thumbnailUrl"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    })
}

/// 用户输入的项目引用形态
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectInput {
    /// 数字 project ID
    Id(String),
    /// CurseForge 链接末段或裸 slug
    Slug(String),
    Unrecognized,
}

/// 识别数字 ID / CurseForge 链接 / 裸 slug。
/// 链接只认 curseforge.com 域名，丢掉查询串后取最后一个非空路径段（可能是 slug 也可能是数字 ID）
fn parse_project_input(input: &str) -> ProjectInput {
    let input = input.trim();
    if input.is_empty() {
        return ProjectInput::Unrecognized;
    }
    if input.bytes().all(|b| b.is_ascii_digit()) {
        return ProjectInput::Id(input.to_string());
    }
    let Some(rest) = input
        .strip_prefix("https://")
        .or_else(|| input.strip_prefix("http://"))
    else {
        return parse_bare_slug(input);
    };
    let rest = rest.trim_start_matches('/');
    let Some((host, path)) = rest.split_once('/') else {
        return ProjectInput::Unrecognized;
    };
    let host = host.to_ascii_lowercase();
    if host != "curseforge.com" && !host.ends_with(".curseforge.com") {
        return ProjectInput::Unrecognized;
    }
    let path = path.split(['?', '#']).next().unwrap_or("");
    // 末段可能是 slug 也可能是数字 ID，重新走一遍完整识别
    match path.rsplit('/').find(|seg| !seg.is_empty()) {
        Some(seg) => parse_project_input(seg),
        None => ProjectInput::Unrecognized,
    }
}

/// 裸段：单段、无空白，只允许字母数字与 - _ .；纯数字会先被上游分支接走
fn parse_bare_slug(input: &str) -> ProjectInput {
    let ok = !input.is_empty()
        && input
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if ok {
        ProjectInput::Slug(input.to_string())
    } else {
        ProjectInput::Unrecognized
    }
}

/// 最新列表里直链为空 = 作者禁止第三方分发，这类文件走手动下载
fn is_restricted(recent: &[CfFile], file_id: u64) -> bool {
    recent
        .iter()
        .any(|file| file.id == file_id && file.download_url.trim().is_empty())
}

/// forgecdn 直链规律：`/files/{id÷1000}/{id%1000}/{文件名}`，文件名按路径段转义
fn cdn_file_url(file_id: u64, file_name: &str) -> String {
    format!(
        "https://mediafilez.forgecdn.net/files/{}/{}/{}",
        file_id / 1000,
        file_id % 1000,
        percent_encode_path_segment(file_name)
    )
}

/// RFC 3986 unreserved 之外的字符全部转义；文件名里常见空格、括号、加号
fn percent_encode_path_segment(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.as_bytes() {
        let keep = byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~');
        if keep {
            out.push(*byte as char);
        } else {
            out.push_str(&format!("%{:02X}", byte));
        }
    }
    out
}

/// CF 的 game_versions 标签里混着加载器名与 Client/Server，拆成两个维度
fn split_game_versions(tags: &[String]) -> (Vec<String>, Vec<String>) {
    let mut loaders = Vec::new();
    let mut versions = Vec::new();
    for tag in tags {
        match tag.to_ascii_lowercase().as_str() {
            "forge" | "fabric" | "quilt" | "neoforge" => loaders.push(tag.to_ascii_lowercase()),
            "client" | "server" => {}
            _ => versions.push(tag.clone()),
        }
    }
    (loaders, versions)
}

/// 搜索条目 → Modrinth 同构项目；简介优先中文翻译（本启动器以中文用户为主）
fn parse_project_hit(hit: &Value) -> Option<ModrinthModpack> {
    let description = ["description_zh", "description_en"]
        .iter()
        .find_map(|key| {
            hit[key]
                .as_str()
                .map(str::trim)
                .filter(|text| !text.is_empty())
        })
        .unwrap_or("");

    Some(ModrinthModpack {
        slug: hit["slug"].as_str()?.to_string(),
        title: hit["title"].as_str()?.to_string(),
        description: description.to_string(),
        icon_url: hit["icon_url"].as_str().map(|s| s.to_string()),
        author: "CurseForge".to_string(),
        downloads: hit["downloads"].as_u64().unwrap_or(0),
        date_created: String::new(),
        date_modified: String::new(),
        latest_version: String::new(),
        game_versions: hit["game_versions"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default(),
        loaders: hit["loaders"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_ascii_lowercase()))
                    .collect()
            })
            .unwrap_or_default(),
        categories: hit["categories"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default(),
    })
}

/// 文件条目解析：缺 id 或文件名的条目直接丢弃（parse_version 的 filter_map 风格）
fn parse_file_entry(entry: &Value) -> Option<CfFile> {
    let sha1 = entry["hashes"]
        .as_array()?
        .iter()
        .find(|hash| hash["algo"].as_i64() == Some(1))
        .and_then(|hash| hash["value"].as_str())
        .unwrap_or("")
        .to_string();

    Some(CfFile {
        id: entry["id"].as_u64()?,
        project_id: entry["mod_id"].as_u64().unwrap_or(0),
        display_name: entry["display_name"].as_str().unwrap_or("").to_string(),
        file_name: entry["file_name"].as_str()?.to_string(),
        release_type: entry["release_type"].as_u64().unwrap_or(1) as u8,
        file_date: entry["file_date"].as_str().unwrap_or("").to_string(),
        file_length: entry["file_length"].as_u64().unwrap_or(0),
        download_url: normalize_cdn_host(entry["download_url"].as_str().unwrap_or("")),
        sha1,
        game_versions: entry["game_versions"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default(),
    })
}

/// minekuai 给的直链挂在 edge.forgecdn.net——它只是发 302 的薄跳板，
/// 个别网络/代理路径下会直接 404；mediafilez 才是真正提供文件的一侧，
/// 统一改写 host 既少一跳重定向也绕开跳板的抖动
fn normalize_cdn_host(url: &str) -> String {
    match url.strip_prefix("https://edge.forgecdn.net/") {
        Some(rest) => format!("https://mediafilez.forgecdn.net/{}", rest),
        None => url.to_string(),
    }
}

/// 分类条目解析：slug 或 name 缺失的丢弃
fn parse_category_entry(entry: &Value) -> Option<CfCategory> {
    Some(CfCategory {
        slug: entry["slug"].as_str()?.to_string(),
        name: entry["name"].as_str()?.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::{
        CfFile, CfResource, CurseForgeService, TokenBucket, cdn_file_url, parse_file_entry,
        parse_project_hit, percent_encode_path_segment, split_game_versions,
    };
    use serde_json::json;
    use std::time::{Duration, Instant};

    #[test]
    fn test_token_bucket_allows_burst_then_throttles() {
        let bucket = TokenBucket::new(600, 3);
        let start = Instant::now();

        let mut state = {
            // 直接构造内部状态来做纯数学断言
            let guard = bucket.state.lock().unwrap();
            let mut cloned = super::BucketState {
                tokens: guard.tokens,
                last: guard.last,
            };
            // 桶初始是满的：前 3 个立即通过
            for i in 0..3 {
                assert_eq!(
                    bucket.next_wait(start, &mut cloned),
                    Duration::ZERO,
                    "第 {} 个应当立即通过",
                    i + 1
                );
            }
            cloned
        };

        // 第 4 个需要等约 0.1 秒（10 个/秒）
        let wait = bucket.next_wait(start, &mut state);
        assert!(wait >= Duration::from_millis(95) && wait <= Duration::from_millis(105));

        // 过了 1 秒，令牌回满到容量 3，又能连续取 3 个
        let later = start + Duration::from_secs(1);
        for i in 0..3 {
            assert_eq!(
                bucket.next_wait(later, &mut state),
                Duration::ZERO,
                "回满后第 {} 个应当立即通过",
                i + 1
            );
        }
    }

    #[test]
    fn test_cdn_url_encodes_path_segments() {
        assert_eq!(
            cdn_file_url(4612979, "RLCraft 1.12.2 - Release v2.9.3.zip"),
            "https://mediafilez.forgecdn.net/files/4612/979/RLCraft%201.12.2%20-%20Release%20v2.9.3.zip"
        );
        assert_eq!(
            percent_encode_path_segment("a+b(c).jar"),
            "a%2Bb%28c%29.jar"
        );
        assert_eq!(
            percent_encode_path_segment("jei-1.20.1.jar"),
            "jei-1.20.1.jar"
        );
    }

    #[test]
    fn test_split_game_versions_separates_loaders() {
        let tags = vec![
            "Client".to_string(),
            "Fabric".to_string(),
            "1.20.1".to_string(),
            "Server".to_string(),
            "Forge".to_string(),
        ];
        let (loaders, versions) = split_game_versions(&tags);
        assert_eq!(loaders, vec!["fabric".to_string(), "forge".to_string()]);
        assert_eq!(versions, vec!["1.20.1".to_string()]);
    }

    #[test]
    fn test_parse_file_entry_takes_sha1_from_algo_1() {
        let entry = json!({
            "id": 9298210,
            "mod_id": 238222,
            "display_name": "15.74.0.229 for Fabric 1.20.1",
            "file_name": "jei-1.20.1-fabric-15.74.0.229.jar",
            "release_type": 1,
            "file_date": "2026-10-23T14:03:53Z",
            "file_length": 6928789,
            "download_url": "https://edge.forgecdn.net/files/9298/210/jei.jar",
            "game_versions": ["Client", "Fabric", "1.20.1"],
            "hashes": [
                { "value": "1311977c2d75ec837a3e255a1ee75b98", "algo": 2 },
                { "value": "21081fe00c041c45ed1ac3aa1b956b1952946362", "algo": 1 }
            ]
        });

        let file = parse_file_entry(&entry).unwrap();
        assert_eq!(file.id, 9298210);
        assert_eq!(file.project_id, 238222);
        assert_eq!(file.sha1, "21081fe00c041c45ed1ac3aa1b956b1952946362");
        assert_eq!(file.game_versions.len(), 3);
        // edge 跳板直链统一改写到 mediafilez
        assert_eq!(
            file.download_url,
            "https://mediafilez.forgecdn.net/files/9298/210/jei.jar"
        );
    }

    #[test]
    fn test_normalize_cdn_host_passes_through_non_edge_urls() {
        assert_eq!(super::normalize_cdn_host(""), "");
        assert_eq!(
            super::normalize_cdn_host("https://mediafilez.forgecdn.net/files/1/2/a.jar"),
            "https://mediafilez.forgecdn.net/files/1/2/a.jar"
        );
        assert_eq!(
            super::normalize_cdn_host("https://example.com/x.zip"),
            "https://example.com/x.zip"
        );
    }

    #[test]
    fn test_parse_file_entry_rejects_entries_without_filename() {
        assert!(parse_file_entry(&json!({ "id": 1 })).is_none());
    }

    #[test]
    fn test_parse_project_hit_prefers_chinese_description() {
        let hit = json!({
            "slug": "jei",
            "title": "Just Enough Items (JEI)",
            "description_zh": "查看物品与配方。",
            "description_en": "View items and recipes",
            "downloads": 590977164,
            "loaders": ["NeoForge", "fabric"],
            "game_versions": ["1.20.1"],
            "categories": ["map-information"]
        });

        let project = parse_project_hit(&hit).unwrap();
        assert_eq!(project.description, "查看物品与配方。");
        assert_eq!(project.author, "CurseForge");
        assert!(project.loaders.contains(&"neoforge".to_string()));
    }

    #[test]
    fn test_modrinth_version_conversion_uses_file_id_as_version() {
        let file = CfFile {
            id: 9207610,
            project_id: 238222,
            display_name: "31.14.0.72 for NeoForge".to_string(),
            file_name: "jei-26.3-neoforge-31.14.0.72.jar".to_string(),
            release_type: 1,
            file_date: "2026-10-01T00:00:00Z".to_string(),
            file_length: 123,
            download_url: "https://edge.forgecdn.net/files/9207/610/jei.jar".to_string(),
            sha1: "abc".to_string(),
            game_versions: vec!["NeoForge".to_string(), "1.21.11".to_string()],
        };

        let version = CurseForgeService::file_to_modrinth_version(&file);
        assert_eq!(version.id, "9207610");
        assert_eq!(version.project_id, "238222");
        assert!(version.featured);
        assert_eq!(version.loaders, vec!["neoforge".to_string()]);
        assert_eq!(version.game_versions, vec!["1.21.11".to_string()]);
        assert_eq!(version.files.len(), 1);
        assert_eq!(
            version.files[0].filename,
            "jei-26.3-neoforge-31.14.0.72.jar"
        );
    }

    #[test]
    fn test_parse_category_entry_requires_slug_and_name() {
        let entry = json!({
            "id": 4483,
            "name": "Combat / PvP",
            "slug": "combat-pvp",
            "iconUrl": "https://media.forgecdn.net/avatars/x.png"
        });
        let category = super::parse_category_entry(&entry).unwrap();
        assert_eq!(category.slug, "combat-pvp");
        assert_eq!(category.name, "Combat / PvP");

        assert!(super::parse_category_entry(&json!({ "name": "NoSlug" })).is_none());
        assert!(super::parse_category_entry(&json!({ "slug": "no-name" })).is_none());
    }

    #[test]
    fn test_resource_paths() {
        assert_eq!(CfResource::Mods.path(), "mc-cf-mods");
        assert_eq!(CfResource::Modpacks.path(), "mc-cf-modpacks");
    }

    #[test]
    fn test_parse_project_input_supports_id_url_and_slug() {
        use super::ProjectInput;

        // 纯数字 ID（含首尾空白）
        assert_eq!(
            super::parse_project_input(" 1582045 "),
            ProjectInput::Id("1582045".to_string())
        );
        // 整合包链接取末段 slug，忽略查询串与锚点
        assert_eq!(
            super::parse_project_input(
                "https://www.curseforge.com/minecraft/modpacks/new-age-science-and-technology"
            ),
            ProjectInput::Slug("new-age-science-and-technology".to_string())
        );
        assert_eq!(
            super::parse_project_input(
                "https://www.curseforge.com/minecraft/mc-mods/jei?tab=files"
            ),
            ProjectInput::Slug("jei".to_string())
        );
        // 末段是数字时识别为 ID；容忍结尾斜杠
        assert_eq!(
            super::parse_project_input("https://curseforge.com/projects/238222/"),
            ProjectInput::Id("238222".to_string())
        );
        // 裸 slug
        assert_eq!(
            super::parse_project_input("new-age-science-and-technology"),
            ProjectInput::Slug("new-age-science-and-technology".to_string())
        );
        // 空串、含空白的搜索词、非 curseforge 域名都不认
        assert_eq!(super::parse_project_input(""), ProjectInput::Unrecognized);
        assert_eq!(
            super::parse_project_input("new age science"),
            ProjectInput::Unrecognized
        );
        assert_eq!(
            super::parse_project_input("https://example.com/jei"),
            ProjectInput::Unrecognized
        );
    }
}
