//! 数据模型模块集合
//!
//! 前后端之间传递的 serde 结构与默认值函数，只有字段定义、不含读写逻辑

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

pub fn default_download_threads() -> u16 {
    16
}

// 单位 MB
pub fn default_max_memory() -> u32 {
    4096
}

pub fn default_true() -> bool {
    true
}

pub fn default_false() -> bool {
    false
}

pub const MIN_SAVE_BACKUP_RETENTION: u32 = 1;
pub const MAX_SAVE_BACKUP_RETENTION: u32 = 50;

pub fn default_save_backup_retention() -> u32 {
    3
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum AuthType {
    #[default]
    Offline,
    Microsoft,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum GcStrategy {
    #[default]
    G1,
    /// 不指定回收器，交给 JVM 默认，与官方启动器一致
    Default,
    /// 吞吐量优先，适合核心数较少的机器
    Parallel,
    /// 单线程回收，通常只用于排查 GC 相关问题
    Serial,
    /// 极低停顿，适合超大堆；Java 15+，旧 Java 自动回退 G1
    Z,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ProxyMode {
    #[default]
    System,
    Direct,
    Custom,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProxySettings {
    #[serde(default)]
    pub mode: ProxyMode,
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyTestResult {
    pub latency_ms: u64,
    pub status: u16,
}

// 默认只做 G1GC 优化，其余 JVM 参数与官方启动器保持一致；用户可在「设置 - 高级选项」逐项开启
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
#[derive(Default)]
pub struct JvmAdvancedOptions {
    #[serde(default)]
    pub gc_strategy: GcStrategy,
    /// 启动时提交整个堆换帧时间稳定，代价是启动变慢
    #[serde(default = "default_false")]
    pub always_pre_touch: bool,
    /// 禁止模组调用 System.gc() 触发的 Full GC
    #[serde(default = "default_false")]
    pub disable_explicit_gc: bool,
    /// 字符串去重：省内存，但会拉长 Young GC 时间
    #[serde(default = "default_false")]
    pub string_deduplication: bool,
    /// 关闭 hsperfdata 性能计数器共享内存
    #[serde(default = "default_false")]
    pub perf_disable_shared_mem: bool,
    /// Metaspace 初始水位 (MB)，None 表示不加该参数、交给 JVM 默认
    #[serde(default)]
    pub metaspace_size_mb: Option<u32>,
    /// Metaspace 上限 (MB)，None 表示不限制；不限的话大型整合包的类元数据会一直向系统申请本地内存，最终拖垮整机
    #[serde(default)]
    pub metaspace_max_size_mb: Option<u32>,
    /// OutOfMemoryError 时导出堆转储到 ./logs/heapdump.hprof
    #[serde(default = "default_false")]
    pub heap_dump_on_oom: bool,
    /// 自定义追加参数，每行一条（或空格分隔），追加在所有内置参数之后
    #[serde(default)]
    pub extra_args: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameConfig {
    pub game_dir: String,
    #[serde(default = "default_true")]
    pub version_isolation: bool,
    pub java_path: Option<String>,
    #[serde(default)]
    pub custom_java_paths: Vec<String>,
    #[serde(default = "default_true")]
    pub auto_match_java: bool,
    #[serde(default = "default_download_threads")]
    pub download_threads: u16,
    pub language: Option<String>,
    pub username: Option<String>,
    pub uuid: Option<String>,
    /// 正版皮肤原图地址，登录/刷新 token 时从 Mojang 个人资料接口取
    pub skin_url: Option<String>,
    #[serde(default = "default_max_memory")]
    pub max_memory: u32,
    pub download_mirror: Option<String>,
    /// 启动器网络代理模式，默认跟随系统
    #[serde(default)]
    pub proxy_mode: ProxyMode,
    #[serde(default)]
    pub proxy_url: Option<String>,
    #[serde(default = "default_false")]
    pub auto_memory_enabled: bool,
    pub window_width: Option<u32>,
    pub window_height: Option<u32>,
    #[serde(default = "default_false")]
    pub fullscreen: bool,
    /// 实例上次启动时间 (实例名 -> 时间戳毫秒)
    #[serde(default)]
    pub instance_last_played: HashMap<String, i64>,
    pub last_selected_version: Option<String>,
    /// 界面配色方案 (indigo / emerald / rose)
    pub theme_palette: Option<String>,
    /// 界面明暗模式 (dark / light)
    pub theme_mode: Option<String>,
    /// 启动器界面语言 (zh-CN / en-US)，与游戏内语言 language 相互独立
    #[serde(default)]
    pub ui_language: Option<String>,
    #[serde(default)]
    pub auth_type: AuthType,
    /// Microsoft access_token，与其余设置同存 ar1s.json（落盘前会收紧文件权限）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ms_access_token: Option<String>,
    /// Microsoft refresh_token，与其余设置同存 ar1s.json
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ms_refresh_token: Option<String>,
    /// 正版 token 过期时间 (Unix 时间戳秒)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ms_expires_at: Option<i64>,
    #[serde(default)]
    pub jvm_advanced: JvmAdvancedOptions,
    /// 模组更新前是否自动备份存档；默认关闭
    #[serde(default = "default_false")]
    pub save_auto_backup: bool,
    /// 自动备份每个世界保留的份数（1-50）
    #[serde(default = "default_save_backup_retention")]
    pub save_backup_retention: u32,
}

impl Default for GameConfig {
    fn default() -> Self {
        Self {
            game_dir: String::new(),
            version_isolation: true,
            java_path: None,
            custom_java_paths: Vec::new(),
            auto_match_java: true,
            download_threads: default_download_threads(),
            language: None,
            username: None,
            uuid: None,
            skin_url: None,
            max_memory: default_max_memory(),
            download_mirror: None,
            proxy_mode: ProxyMode::System,
            proxy_url: None,
            auto_memory_enabled: false,
            window_width: None,
            window_height: None,
            fullscreen: false,
            instance_last_played: HashMap::new(),
            last_selected_version: None,
            theme_palette: None,
            theme_mode: None,
            ui_language: None,
            auth_type: AuthType::default(),
            ms_access_token: None,
            ms_refresh_token: None,
            ms_expires_at: None,
            jvm_advanced: JvmAdvancedOptions::default(),
            save_auto_backup: false,
            save_backup_retention: default_save_backup_retention(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GameDirInfo {
    pub path: String,
    pub versions: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MinecraftVersion {
    pub id: String,
    #[serde(rename = "type")]
    pub version_type: String,
    pub url: String,
    pub time: String,
    #[serde(rename = "releaseTime")]
    pub release_time: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct VersionManifest {
    pub latest: LatestVersions,
    pub versions: Vec<MinecraftVersion>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LatestVersions {
    pub release: String,
    pub snapshot: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LaunchOptions {
    pub version: String,
    pub username: String,
    pub memory: Option<u32>,
    pub window_width: Option<u32>,
    pub window_height: Option<u32>,
    pub fullscreen: Option<bool>,
    pub auth_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uuid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub override_java_path: Option<String>,
    /// 实例级覆盖；None 表示全部跟随全局
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance_settings: Option<crate::models::InstanceSettings>,
    /// 判据不过、但用户已经看过诊断报告并坚持启动
    #[serde(default)]
    pub force_java_mismatch: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JavaCompatibilityResult {
    pub compatible: bool,
    /// 已知起不来（违反硬上限、或低于 mods 实测出的下限），界面不应再给"继续启动"
    pub hard_block: bool,
    /// hard_block 时的纯文本诊断报告，其余情况为空
    pub diagnostic: String,
    pub current_java_version: Option<u32>,
    pub required_java_version: u32,
    pub recommended_min_java_version: u32,
    pub recommended_max_java_version: u32,
    pub current_java_path: Option<String>,
    pub recommended_java_path: Option<String>,
    pub recommended_java_version: Option<u32>,
    pub recommended_java_optimal: bool,
    pub recommended_java_warning: Option<String>,
    pub auto_match_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DownloadStatus {
    Downloading,
    Completed,
    Cancelled,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadProgress {
    pub bytes_downloaded: u64,
    pub total_bytes: u64,
    pub speed: f64,
    pub status: DownloadStatus,
    pub percent: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DownloadJob {
    /// 首选地址
    pub url: String,
    /// 备用地址，按顺序尝试（其他镜像、官方源等）
    pub fallback_urls: Vec<String>,
    pub path: PathBuf,
    pub size: u64,
    pub hash: Option<String>,
}

impl DownloadJob {
    pub fn new(
        url: String,
        fallback_urls: Vec<String>,
        path: PathBuf,
        size: u64,
        hash: Option<String>,
    ) -> Self {
        let fallback_urls = fallback_urls
            .into_iter()
            .filter(|candidate| !candidate.is_empty() && *candidate != url)
            .fold(Vec::new(), |mut unique: Vec<String>, candidate| {
                if !unique.contains(&candidate) {
                    unique.push(candidate);
                }
                unique
            });

        Self {
            url,
            fallback_urls,
            path,
            size,
            hash,
        }
    }

    /// 本次尝试的起始地址：`attempt` 递增时轮换首选源，避免每次都先撞同一个坏源
    pub fn url_for_attempt(&self, attempt: usize) -> &str {
        match attempt % (1 + self.fallback_urls.len()) {
            0 => &self.url,
            index => &self.fallback_urls[index - 1],
        }
    }

    /// 从 `preferred` 开始的所有候选地址（去重，保持顺序）
    pub fn urls_from<'a>(&'a self, preferred: &'a str) -> Vec<&'a str> {
        let mut urls = vec![preferred];
        for candidate in
            std::iter::once(self.url.as_str()).chain(self.fallback_urls.iter().map(String::as_str))
        {
            if candidate != preferred && !urls.contains(&candidate) {
                urls.push(candidate);
            }
        }
        urls
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstanceInfo {
    pub id: String,
    pub name: String,
    pub version: String,
    pub path: String,
    pub created_time: Option<String>,
    pub loader_type: Option<String>,
    pub loader_version: Option<String>,
    pub game_version: Option<String>,
    pub last_played: Option<i64>,
    pub mod_count: Option<u32>,
    /// 整合包自带的封面图 URL，没有时前端回退到加载器图标
    pub icon_url: Option<String>,
    /// 自动内存开启时，本实例启动预计分配的堆大小
    pub auto_memory_mb: Option<u32>,
}

pub mod instance_settings;
pub mod modpack;
pub mod saves;

pub use instance_settings::InstanceSettings;
pub use saves::{SaveBackupInfo, SaveBackupKind, SaveImportSummary, SaveInfo};

#[cfg(test)]
mod tests {
    use super::{GameConfig, ProxyMode};

    #[test]
    fn test_auto_match_java_is_enabled_by_default() {
        assert!(GameConfig::default().auto_match_java);
    }

    #[test]
    fn test_proxy_follows_system_by_default() {
        let config = GameConfig::default();
        assert_eq!(config.proxy_mode, ProxyMode::System);
        assert_eq!(config.proxy_url, None);
    }

    #[test]
    fn test_download_threads_default_to_16() {
        assert_eq!(GameConfig::default().download_threads, 16);
    }
}
