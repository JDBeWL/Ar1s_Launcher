//! 配置文件读写模块
//!
//! 绿色版布局：配置固定放在 exe 同级目录。所有读改写走跨进程文件锁，读取用 mtime/长度
//! 失效缓存，写入走临时文件 rename，解析失败时备份 .bak 再重建默认配置。
//! token 与其余设置同存 ar1s.json，所以落盘前要收紧权限（Unix 0600、Windows 收紧 DACL）；
//! 旧版本的 credentials.json 在第一次读取时并入配置，写回成功后删除。

use serde::Deserialize;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, SystemTime};
use tauri::Emitter;

use crate::errors::LauncherError;
use crate::models::{GameConfig, GameDirInfo};
use crate::services::memory::get_memory_warning_message;
use crate::utils::file_lock;

static CONFIG_CACHE: std::sync::LazyLock<RwLock<Option<CachedConfig>>> =
    std::sync::LazyLock::new(|| RwLock::new(None));

static CONFIG_MUTATION_LOCK: Mutex<()> = Mutex::new(());

/// 上次真正 stat 配置文件的时间：`load_config()` 调用极密（遍历实例与资源包时每个都要问一次），
/// 每次 `fs::metadata` 是纯浪费；不检查又会读到另一个实例刚写进去的旧值。
/// 折中：最多每 [`STAMP_TTL`] 真 stat 一次，期间的变化最晚 500ms 后被看到
static LAST_STAMP_AT: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);

/// 上一次 stat 的结果，TTL 内直接复用
static LAST_STAMP: std::sync::Mutex<Option<FileStamp>> = std::sync::Mutex::new(None);

const STAMP_TTL: Duration = Duration::from_millis(500);

const CONFIG_LOCK_TIMEOUT: Duration = Duration::from_secs(3);
const AUTH_LOCK_TIMEOUT: Duration = Duration::from_secs(90);

pub const MIN_DOWNLOAD_THREADS: u16 = 1;
pub const MAX_DOWNLOAD_THREADS: u16 = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileStamp {
    modified: Option<SystemTime>,
    len: u64,
}

struct CachedConfig {
    config: Arc<GameConfig>,
    stamp: FileStamp,
}

fn exe_dir_or_dot() -> PathBuf {
    match EXE_DIR.as_ref() {
        Ok(dir) => dir.clone(),
        Err(_) => PathBuf::from("."),
    }
}

static STORAGE: OnceLock<PathBuf> = OnceLock::new();

/// 未初始化时退回 exe 目录，保证测试和早期调用仍有可用路径
fn storage() -> &'static Path {
    STORAGE.get_or_init(exe_dir_or_dot).as_path()
}

pub fn init_storage() -> &'static Path {
    storage()
}

fn default_game_dir(root: &Path) -> PathBuf {
    root.join(".minecraft")
}

fn lock_config() -> Result<std::sync::MutexGuard<'static, ()>, LauncherError> {
    match CONFIG_MUTATION_LOCK.lock() {
        Ok(guard) => Ok(guard),
        Err(poisoned) => {
            log::warn!("配置写锁被破坏，正在恢复");
            Ok(poisoned.into_inner())
        }
    }
}

fn with_read_lock<F, R>(f: F) -> R
where
    F: FnOnce(&Option<CachedConfig>) -> R,
{
    let guard = match CONFIG_CACHE.read().ok() {
        Some(g) => g,
        None => {
            log::warn!("配置缓存读取锁被破坏，使用空缓存");
            return f(&None);
        }
    };
    f(&guard)
}

fn with_write_lock<F, R>(f: F) -> R
where
    F: FnOnce(&mut Option<CachedConfig>) -> R,
{
    let mut guard = match CONFIG_CACHE.write() {
        Ok(g) => g,
        Err(poisoned) => {
            log::warn!("配置缓存写入锁被破坏，正在恢复");
            poisoned.into_inner()
        }
    };
    f(&mut guard)
}

fn file_stamp(path: &Path) -> FileStamp {
    match fs::metadata(path) {
        Ok(meta) => FileStamp {
            modified: meta.modified().ok(),
            len: meta.len(),
        },
        Err(_) => FileStamp {
            modified: None,
            len: 0,
        },
    }
}

fn current_cache_stamp() -> FileStamp {
    let now = std::time::Instant::now();

    if let Some(at) = *LAST_STAMP_AT.lock().unwrap_or_else(|e| e.into_inner())
        && now.duration_since(at) < STAMP_TTL
        && let Some(stamp) = *LAST_STAMP.lock().unwrap_or_else(|e| e.into_inner())
    {
        return stamp;
    }

    let stamp = file_stamp(&config_path());
    *LAST_STAMP.lock().unwrap_or_else(|e| e.into_inner()) = Some(stamp);
    *LAST_STAMP_AT.lock().unwrap_or_else(|e| e.into_inner()) = Some(now);
    stamp
}

/// 写完配置后必须让下一次 `load_config` 立刻看到新内容，
/// 否则会命中 TTL 内缓存的旧 stamp，读回旧配置
fn invalidate_stamp_cache() {
    *LAST_STAMP_AT.lock().unwrap_or_else(|e| e.into_inner()) = None;
    *LAST_STAMP.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

fn cache_config(config: Arc<GameConfig>) {
    invalidate_stamp_cache();
    let stamp = current_cache_stamp();
    with_write_lock(|cache| {
        *cache = Some(CachedConfig { config, stamp });
    });
}

pub fn preload_config() -> Result<(), LauncherError> {
    log::info!("预加载配置文件...");
    cleanup_legacy_lock_files();
    let _ = load_config()?;
    log::info!("配置文件预加载完成");
    Ok(())
}

pub fn invalidate_config_cache() {
    invalidate_stamp_cache();
    with_write_lock(|cache| {
        *cache = None;
    });
    log::debug!("配置缓存已清除");
}

pub fn get_saved_username() -> Result<Option<String>, LauncherError> {
    let config = load_config()?;
    Ok(config.username.clone())
}

pub fn set_saved_username(username: String) -> Result<(), LauncherError> {
    set_config_value(|config| config.username = Some(username))
}

pub fn get_saved_uuid() -> Result<Option<String>, LauncherError> {
    let config = load_config()?;
    Ok(config.uuid.clone())
}

pub fn set_saved_uuid(uuid: String) -> Result<(), LauncherError> {
    set_config_value(|config| config.uuid = Some(uuid))
}

pub fn load_config() -> Result<Arc<GameConfig>, LauncherError> {
    let stamp = current_cache_stamp();
    if let Some(config) = with_read_lock(|cache| {
        cache
            .as_ref()
            .filter(|cached| cached.stamp == stamp)
            .map(|cached| cached.config.clone())
    }) {
        return Ok(config);
    }

    // 读取期间另一实例可能刚好写完；时间戳变了就重读，避免缓存内容和标记错位
    let mut attempts = 0;
    let loaded = loop {
        let before = current_cache_stamp();
        let loaded = read_config()?;
        attempts += 1;
        if before == current_cache_stamp() || attempts >= 3 {
            break loaded;
        }
    };

    let arc = Arc::new(loaded.config);
    if loaded.needs_save {
        save_config(&arc)?;
    } else {
        cache_config(arc.clone());
    }
    Ok(arc)
}

struct LoadedConfig {
    config: GameConfig,
    needs_save: bool,
}

/// 旧版本把 token 存在单独的 credentials.json 里；这里只做一次迁移读取
#[derive(Debug, Default, Deserialize)]
struct LegacyCredentials {
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_at: Option<i64>,
}

impl LegacyCredentials {
    fn is_empty(&self) -> bool {
        self.access_token.is_none() && self.refresh_token.is_none()
    }
}

fn load_legacy_credentials(path: &Path) -> Option<LegacyCredentials> {
    let content = fs::read_to_string(path).ok()?;
    match serde_json::from_str::<LegacyCredentials>(&content) {
        Ok(credentials) => Some(credentials),
        Err(e) => {
            // 损坏就丢掉，不能让一个旧文件卡住启动；里面顶多是已经失效的 token
            log::warn!("旧的凭据文件无法解析，已忽略: {}", e);
            None
        }
    }
}

/// 把旧凭据文件里的 token 并进配置；返回是否需要写回（写回成功后旧文件会被删除）
fn merge_legacy_credentials_from(path: &Path, config: &mut GameConfig) -> bool {
    if !path.exists() {
        return false;
    }
    let Some(legacy) = load_legacy_credentials(path) else {
        // 解析不了就没法合并，但仍要写回一次把坏文件清掉
        return true;
    };
    // 配置文件里已经有 token 就以配置为准，避免用更旧的值覆盖
    if !legacy.is_empty() && !has_tokens(config) {
        config.ms_access_token = legacy.access_token;
        config.ms_refresh_token = legacy.refresh_token;
        config.ms_expires_at = legacy.expires_at;
        log::info!("已将旧的凭据文件合并进 ar1s.json");
    }
    true
}

fn read_config() -> Result<LoadedConfig, LauncherError> {
    read_config_from(storage())
}

fn read_config_from(root: &Path) -> Result<LoadedConfig, LauncherError> {
    let config_path = root.join("ar1s.json");
    if !config_path.exists() {
        let mut config = create_default_config(true)?;
        merge_legacy_credentials_from(&root.join("credentials.json"), &mut config);
        return Ok(LoadedConfig {
            config,
            needs_save: true,
        });
    }

    let content = fs::read_to_string(&config_path)?;
    // 配置损坏时备份为 .bak 并重建默认配置，不向上报错
    match serde_json::from_str::<GameConfig>(&content) {
        Ok(mut config) => {
            // 旧版本把 token 放在单独的凭据文件里，读进来后由写回逻辑并进 ar1s.json
            let migrated =
                merge_legacy_credentials_from(&root.join("credentials.json"), &mut config);
            Ok(LoadedConfig {
                config,
                needs_save: migrated,
            })
        }
        Err(_) => {
            let backup_path = config_path.with_extension("bak");
            let _ = fs::copy(&config_path, &backup_path);
            log::warn!("配置文件损坏，已备份并重建默认配置");
            let mut config = create_default_config(false)?;
            merge_legacy_credentials_from(&root.join("credentials.json"), &mut config);
            Ok(LoadedConfig {
                config,
                needs_save: true,
            })
        }
    }
}

fn create_default_config(is_first_run: bool) -> Result<GameConfig, LauncherError> {
    let mc_dir = default_game_dir(storage());
    let mc_dir_str = mc_dir.to_string_lossy().into_owned();

    if !mc_dir.exists() {
        fs::create_dir_all(&mc_dir)?;
        let sub_dirs = [
            "versions",
            "libraries",
            "assets",
            "saves",
            "resourcepacks",
            "logs",
        ];
        for dir in sub_dirs {
            fs::create_dir_all(mc_dir.join(dir))?;
        }
    }

    let mut config = GameConfig {
        game_dir: mc_dir_str,
        version_isolation: true,
        java_path: None,
        custom_java_paths: Vec::new(),
        auto_match_java: true,
        download_threads: 16,
        language: Some("zh_cn".to_string()),
        username: None,
        uuid: None,
        skin_url: None,
        max_memory: crate::models::default_max_memory(),
        download_mirror: Some("auto".to_string()),
        proxy_mode: crate::models::ProxyMode::System,
        proxy_url: None,
        auto_memory_enabled: false,
        window_width: None,
        window_height: None,
        fullscreen: false,
        instance_last_played: std::collections::HashMap::new(),
        last_selected_version: None,
        theme_palette: None,
        theme_mode: None,
        ui_language: None,
        auth_type: crate::models::AuthType::Offline,
        ms_access_token: None,
        ms_refresh_token: None,
        ms_expires_at: None,
        jvm_advanced: crate::models::JvmAdvancedOptions::default(),
        save_auto_backup: false,
        save_backup_retention: crate::models::default_save_backup_retention(),
    };

    if is_first_run
        && let Ok(java_paths) = auto_detect_java()
        && !java_paths.is_empty()
    {
        let best = java_paths
            .iter()
            .filter_map(|p| {
                crate::services::launcher::java::detect_java_version(p).map(|v| (p.clone(), v))
            })
            .max_by_key(|(_, v)| *v)
            .map(|(p, _)| p)
            .unwrap_or_else(|| java_paths[0].clone());
        config.java_path = Some(best.clone());
        log::info!("首次启动自动检测到Java路径: {}", best);
    }

    // 首次启动/损坏重建由 load_config 在锁内落盘，这里只负责构造
    Ok(config)
}

use crate::services::java::auto_detect_java;

/// 全量保存配置快照；登录/登出等 token 变更也走 update_config
pub fn save_config(config: &GameConfig) -> Result<(), LauncherError> {
    let _guard = lock_config()?;
    let _file_lock = file_lock::acquire(&config_lock_path(), CONFIG_LOCK_TIMEOUT)?;

    // token 和设置同处一个文件，全量写回会把调用方手里可能过期的 token 盖回去：
    // 锁内重读一次，磁盘上已经有 token 就以磁盘为准（真正改 token 的登录/登出走 update_config）
    let mut config = config.clone();
    if let Some(current) = read_tokens_from_disk()
        && has_tokens(&current)
    {
        config.ms_access_token = current.ms_access_token;
        config.ms_refresh_token = current.ms_refresh_token;
        config.ms_expires_at = current.ms_expires_at;
    }

    save_config_internal(&config)?;
    cache_config(Arc::new(config));
    Ok(())
}

/// 只取磁盘上已有的 token；文件不存在或损坏时返回 None，不触发默认配置的构造
fn read_tokens_from_disk() -> Option<GameConfig> {
    read_tokens_from_disk_at(storage())
}

fn read_tokens_from_disk_at(root: &Path) -> Option<GameConfig> {
    let content = fs::read_to_string(root.join("ar1s.json")).ok()?;
    serde_json::from_str::<GameConfig>(&content).ok()
}

/// 所有读改写都走这里：跨进程锁 + 重读磁盘，避免两个实例用旧快照互相覆盖
pub fn update_config<F, R>(update: F) -> Result<R, LauncherError>
where
    F: FnOnce(&mut GameConfig) -> Result<R, LauncherError>,
{
    let _guard = lock_config()?;
    let _file_lock = file_lock::acquire(&config_lock_path(), CONFIG_LOCK_TIMEOUT)?;

    let loaded = read_config()?;
    let mut config = loaded.config;
    let result = update(&mut config)?;

    save_config_internal(&config)?;
    cache_config(Arc::new(config));
    Ok(result)
}

/// update_config 的异步版：跨进程文件锁最多轮询 3 秒，加上整文件重读、全量写回与
/// 权限收紧全是阻塞操作，async 上下文（启动、登录、刷新 token 等）必须走这里，
/// 别占住 tokio worker。同步命令（已在主线程/阻塞池上的）仍用 update_config
pub async fn update_config_async<F, R>(update: F) -> Result<R, LauncherError>
where
    F: FnOnce(&mut GameConfig) -> Result<R, LauncherError> + Send + 'static,
    R: Send + 'static,
{
    tokio::task::spawn_blocking(move || update_config(update))
        .await
        .map_err(|e| LauncherError::Custom(format!("配置更新任务异常终止: {}", e)))?
}

fn has_tokens(config: &GameConfig) -> bool {
    config.ms_access_token.is_some() || config.ms_refresh_token.is_some()
}

/// 诊断包里的配置快照：token、账户标识与代理 URL 都不能带出去
pub(crate) fn config_for_diagnostics(config: &GameConfig) -> GameConfig {
    let mut snapshot = config.clone();
    snapshot.ms_access_token = None;
    snapshot.ms_refresh_token = None;
    snapshot.username = None;
    snapshot.uuid = None;
    snapshot.skin_url = None;
    snapshot.ms_expires_at = None;
    if snapshot.proxy_url.is_some() {
        snapshot.proxy_url = Some("***".to_string());
    }
    snapshot.instance_last_played.clear();
    snapshot
}

fn save_config_internal(config: &GameConfig) -> Result<(), LauncherError> {
    save_config_to(storage(), config)
}

/// 写配置并收紧权限；写完顺手删掉已经合并过的旧凭据文件
fn save_config_to(root: &Path, config: &GameConfig) -> Result<(), LauncherError> {
    let config_path = root.join("ar1s.json");
    let dir = config_path
        .parent()
        .ok_or_else(|| LauncherError::Custom("配置文件没有父目录".to_string()))?;
    fs::create_dir_all(dir)?;
    let json = serde_json::to_string_pretty(config)?;

    // 直接覆写会在崩溃时留下截断的 ar1s.json，先写临时文件再 rename
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    tmp.write_all(json.as_bytes())?;
    tmp.flush()?;

    // 文件里有 token，权限在 rename 之前设好，避免落盘瞬间是可读的
    if let Err(e) = crate::utils::secure_file::set_owner_only(tmp.path()) {
        log::warn!("设置配置文件权限失败: {}", e);
    }

    tmp.persist(&config_path).map_err(|e| e.error)?;
    crate::utils::secure_file::restrict_to_current_user(&config_path);

    // 旧的凭据文件已经并进配置，写完再删，失败也不影响本次启动
    let legacy = root.join("credentials.json");
    if legacy.exists() {
        match fs::remove_file(&legacy) {
            Ok(()) => log::info!("旧的凭据文件已合并并删除"),
            Err(e) => log::warn!("删除旧的凭据文件失败: {}", e),
        }
    }
    Ok(())
}

static EXE_DIR: std::sync::LazyLock<Result<PathBuf, String>> = std::sync::LazyLock::new(|| {
    std::env::current_exe()
        .map_err(|e| format!("无法获取可执行文件路径: {}", e))?
        .parent()
        .map(|p| p.to_path_buf())
        .ok_or_else(|| "无法获取可执行文件目录".to_string())
});

/// 兼容旧版本留下的函数名；实际路径就是 exe 同级的 ar1s.json
fn config_path() -> PathBuf {
    storage().join("ar1s.json")
}

/// 配置写入锁：临界区只有一次读改写，等不久
fn config_lock_path() -> PathBuf {
    storage().join(".ar1s-config.lock")
}

/// token 刷新锁要跨网络请求持有，不能和配置写锁共用同一个文件：
/// 刷新流程内部还会调用 update_config，同进程二次加锁会自己把自己挡住
fn auth_lock_path() -> PathBuf {
    storage().join(".ar1s-auth.lock")
}

/// 只为升级过的目录清掉历史垃圾；新锁由内核在最后一个持有者释放时删除
pub fn cleanup_legacy_lock_files() {
    for name in [".ar1s-config.lock", ".ar1s-auth.lock"] {
        let path = storage().join(name);
        if crate::utils::file_lock::remove_if_unused(&path) {
            log::debug!("已清理残留锁文件: {}", name);
        }
    }
}

pub(crate) async fn acquire_auth_lock() -> Result<file_lock::FileLock, LauncherError> {
    file_lock::acquire_async(&auth_lock_path(), AUTH_LOCK_TIMEOUT).await
}

/// token 刷新拿到跨进程锁后强制重读，确认别的实例有没有刚轮换过
pub(crate) fn reload_config_from_disk() -> Result<Arc<GameConfig>, LauncherError> {
    let loaded = read_config()?;
    let arc = Arc::new(loaded.config);
    if loaded.needs_save {
        save_config(&arc)?;
    } else {
        cache_config(arc.clone());
    }
    Ok(arc)
}

#[derive(Debug, Clone, Copy)]
enum ConfigKey {
    JavaPath,
    GameDir,
    VersionIsolation,
    DownloadThreads,
    Language,
    Username,
    Uuid,
    MaxMemory,
    DownloadMirror,
    AutoMatchJava,
    AutoMemoryEnabled,
    WindowWidth,
    WindowHeight,
    Fullscreen,
    AuthType,
    LastSelectedVersion,
    ThemePalette,
    ThemeMode,
    UiLanguage,
    SaveAutoBackup,
    SaveBackupRetention,
}

impl ConfigKey {
    fn from_str(key: &str) -> Option<Self> {
        match key {
            "javaPath" => Some(Self::JavaPath),
            "gameDir" => Some(Self::GameDir),
            "versionIsolation" => Some(Self::VersionIsolation),
            "downloadThreads" => Some(Self::DownloadThreads),
            "language" => Some(Self::Language),
            "username" => Some(Self::Username),
            "uuid" => Some(Self::Uuid),
            "maxMemory" => Some(Self::MaxMemory),
            "downloadMirror" => Some(Self::DownloadMirror),
            "autoMatchJava" => Some(Self::AutoMatchJava),
            "autoMemoryEnabled" => Some(Self::AutoMemoryEnabled),
            "windowWidth" => Some(Self::WindowWidth),
            "windowHeight" => Some(Self::WindowHeight),
            "fullscreen" => Some(Self::Fullscreen),
            "authType" => Some(Self::AuthType),
            "lastSelectedVersion" => Some(Self::LastSelectedVersion),
            "themePalette" => Some(Self::ThemePalette),
            "themeMode" => Some(Self::ThemeMode),
            "uiLanguage" => Some(Self::UiLanguage),
            "saveAutoBackup" => Some(Self::SaveAutoBackup),
            "saveBackupRetention" => Some(Self::SaveBackupRetention),
            _ => None,
        }
    }

    fn get_value(&self, config: &GameConfig) -> Option<String> {
        match self {
            Self::JavaPath => config.java_path.clone(),
            Self::GameDir => Some(config.game_dir.clone()),
            Self::VersionIsolation => Some(config.version_isolation.to_string()),
            Self::DownloadThreads => Some(config.download_threads.to_string()),
            Self::Language => config.language.clone(),
            Self::Username => config.username.clone(),
            Self::Uuid => config.uuid.clone(),
            Self::MaxMemory => Some(config.max_memory.to_string()),
            Self::DownloadMirror => config.download_mirror.clone(),
            Self::AutoMatchJava => Some(config.auto_match_java.to_string()),
            Self::AutoMemoryEnabled => Some(config.auto_memory_enabled.to_string()),
            Self::WindowWidth => config.window_width.map(|v| v.to_string()),
            Self::WindowHeight => config.window_height.map(|v| v.to_string()),
            Self::Fullscreen => Some(config.fullscreen.to_string()),
            Self::AuthType => Some(match config.auth_type {
                crate::models::AuthType::Offline => "offline".to_string(),
                crate::models::AuthType::Microsoft => "microsoft".to_string(),
            }),
            Self::LastSelectedVersion => config.last_selected_version.clone(),
            Self::ThemePalette => config.theme_palette.clone(),
            Self::ThemeMode => config.theme_mode.clone(),
            Self::UiLanguage => config.ui_language.clone(),
            Self::SaveAutoBackup => Some(config.save_auto_backup.to_string()),
            Self::SaveBackupRetention => Some(config.save_backup_retention.to_string()),
        }
    }

    fn set_value(&self, config: &mut GameConfig, value: String) -> Result<(), LauncherError> {
        match self {
            Self::JavaPath => config.java_path = Some(value),
            Self::GameDir => config.game_dir = value,
            Self::VersionIsolation => {
                config.version_isolation = value
                    .parse()
                    .map_err(|_| LauncherError::Custom("版本隔离设置值无效".to_string()))?
            }
            Self::DownloadThreads => {
                let threads: u16 = value
                    .parse()
                    .map_err(|_| LauncherError::Custom("下载线程数设置值无效".to_string()))?;
                validate_download_threads(threads)?;
                config.download_threads = threads;
            }
            Self::Language => config.language = Some(value),
            Self::Username => config.username = Some(value),
            Self::Uuid => config.uuid = Some(value),
            Self::MaxMemory => {
                config.max_memory = value
                    .parse()
                    .map_err(|_| LauncherError::Custom("最大内存设置值无效".to_string()))?
            }
            Self::DownloadMirror => config.download_mirror = Some(value),
            Self::AutoMatchJava => {
                config.auto_match_java = value
                    .parse()
                    .map_err(|_| LauncherError::Custom("自动匹配Java设置值无效".to_string()))?
            }
            Self::AutoMemoryEnabled => {
                config.auto_memory_enabled = value
                    .parse()
                    .map_err(|_| LauncherError::Custom("自动内存设置值无效".to_string()))?
            }
            Self::WindowWidth => {
                config.window_width = Some(
                    value
                        .parse()
                        .map_err(|_| LauncherError::Custom("窗口宽度设置值无效".to_string()))?,
                )
            }
            Self::WindowHeight => {
                config.window_height = Some(
                    value
                        .parse()
                        .map_err(|_| LauncherError::Custom("窗口高度设置值无效".to_string()))?,
                )
            }
            Self::Fullscreen => {
                config.fullscreen = value
                    .parse()
                    .map_err(|_| LauncherError::Custom("全屏设置值无效".to_string()))?
            }
            Self::AuthType => {
                config.auth_type = match value.to_lowercase().as_str() {
                    "offline" => crate::models::AuthType::Offline,
                    "microsoft" => crate::models::AuthType::Microsoft,
                    _ => {
                        return Err(LauncherError::Custom(
                            "认证类型无效，仅支持 offline 或 microsoft".to_string(),
                        ));
                    }
                }
            }
            Self::LastSelectedVersion => config.last_selected_version = Some(value),
            Self::ThemePalette => config.theme_palette = Some(value),
            Self::ThemeMode => {
                config.theme_mode = match value.to_lowercase().as_str() {
                    "dark" | "light" => Some(value.to_lowercase()),
                    _ => {
                        return Err(LauncherError::Custom(
                            "主题模式无效，仅支持 dark 或 light".to_string(),
                        ));
                    }
                }
            }
            Self::UiLanguage => {
                // 归一化后再落盘：前端可能传 zh / en 这类简写，存成标准标签才不会反复判断
                config.ui_language = match value.to_lowercase().replace('_', "-").as_str() {
                    "zh" | "zh-cn" | "zh-hans" => Some("zh-CN".to_string()),
                    "en" | "en-us" => Some("en-US".to_string()),
                    _ => {
                        return Err(LauncherError::Custom(
                            "界面语言无效，仅支持 zh-CN 或 en-US".to_string(),
                        ));
                    }
                }
            }
            Self::SaveAutoBackup => {
                config.save_auto_backup = value
                    .parse()
                    .map_err(|_| LauncherError::Custom("自动备份设置值无效".to_string()))?
            }
            Self::SaveBackupRetention => {
                let retention: u32 = value
                    .parse()
                    .map_err(|_| LauncherError::Custom("自动备份保留份数无效".to_string()))?;
                if !(crate::models::MIN_SAVE_BACKUP_RETENTION
                    ..=crate::models::MAX_SAVE_BACKUP_RETENTION)
                    .contains(&retention)
                {
                    return Err(LauncherError::Custom(format!(
                        "自动备份保留份数必须在 {} 到 {} 之间",
                        crate::models::MIN_SAVE_BACKUP_RETENTION,
                        crate::models::MAX_SAVE_BACKUP_RETENTION
                    )));
                }
                config.save_backup_retention = retention;
            }
        }
        Ok(())
    }
}

pub fn load_config_key(key: String) -> Result<Option<String>, LauncherError> {
    let config = load_config()?;
    match ConfigKey::from_str(&key) {
        Some(config_key) => Ok(config_key.get_value(&config)),
        None => Err(LauncherError::Custom(format!("未知的配置项: {}", key))),
    }
}

pub fn save_config_key(key: String, value: String) -> Result<(), LauncherError> {
    let config_key = ConfigKey::from_str(&key)
        .ok_or_else(|| LauncherError::Custom(format!("未知的配置项: {}", key)))?;
    update_config(|config| config_key.set_value(config, value))
}

fn get_config_value<T, F>(getter: F) -> Result<T, LauncherError>
where
    F: FnOnce(&GameConfig) -> T,
{
    let config = load_config()?;
    Ok(getter(&config))
}

fn set_config_value<T, F>(setter: F) -> Result<(), LauncherError>
where
    F: FnOnce(&mut GameConfig) -> T,
{
    update_config(|config| {
        setter(config);
        Ok(())
    })
}

pub fn get_game_dir() -> Result<String, LauncherError> {
    get_config_value(|config| config.game_dir.clone())
}

/// 启动、模组管理、整合包安装都靠它决定内容落到实例目录还是共享根目录；
/// 配置读不出来时按默认（隔离）处理
pub fn is_version_isolated() -> bool {
    load_config()
        .map(|config| config.version_isolation)
        .unwrap_or(true)
}

pub fn get_game_dir_info() -> Result<GameDirInfo, LauncherError> {
    let game_dir_str = get_game_dir()?;
    let versions_dir = PathBuf::from(&game_dir_str).join("versions");
    let mut versions = Vec::new();

    if versions_dir.is_dir() {
        for entry in fs::read_dir(versions_dir)? {
            if let Ok(entry) = entry
                && entry.file_type()?.is_dir()
            {
                let version_id = entry.file_name().to_string_lossy().into_owned();
                let version_json_path = entry.path().join(format!("{}.json", version_id));
                if version_json_path.exists() {
                    versions.push(version_id);
                }
            }
        }
    }

    Ok(GameDirInfo {
        path: game_dir_str,
        versions,
    })
}

pub fn set_game_dir(path: String, window: &tauri::Window) -> Result<(), LauncherError> {
    let path_clone = path.clone();
    set_config_value(|config| config.game_dir = path_clone)?;
    window.emit("game-dir-changed", &path)?;
    Ok(())
}

pub fn get_download_threads() -> Result<u16, LauncherError> {
    get_config_value(|config| config.download_threads)
}

fn validate_download_threads(threads: u16) -> Result<(), LauncherError> {
    if !(MIN_DOWNLOAD_THREADS..=MAX_DOWNLOAD_THREADS).contains(&threads) {
        return Err(LauncherError::Custom(format!(
            "下载线程数必须在 {} 到 {} 之间",
            MIN_DOWNLOAD_THREADS, MAX_DOWNLOAD_THREADS
        )));
    }
    Ok(())
}

pub fn set_download_threads(threads: u16) -> Result<(), LauncherError> {
    validate_download_threads(threads)?;
    set_config_value(|config| config.download_threads = threads)
}

/// 超过系统内存 90% 时返回警告文案
pub fn check_memory_warning(memory_mb: u32) -> Result<Option<String>, LauncherError> {
    Ok(get_memory_warning_message(memory_mb))
}

pub fn update_instance_last_played(instance_name: &str) -> Result<(), LauncherError> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let key = instance_name.to_string();
    set_config_value(|config| config.instance_last_played.insert(key, now))
}

pub fn get_instance_last_played(instance_name: &str) -> Option<i64> {
    load_config()
        .ok()
        .and_then(|config| config.instance_last_played.get(instance_name).copied())
}

pub fn remove_instance_last_played(instance_name: &str) -> Result<(), LauncherError> {
    let key = instance_name.to_string();
    set_config_value(|config| config.instance_last_played.remove(&key))
}

pub fn rename_instance_last_played(old_name: &str, new_name: &str) -> Result<(), LauncherError> {
    let old_key = old_name.to_string();
    let new_key = new_name.to_string();
    set_config_value(|config| {
        if let Some(time) = config.instance_last_played.remove(&old_key) {
            config.instance_last_played.insert(new_key, time);
        }
    })
}

pub fn get_last_selected_version() -> Option<String> {
    load_config()
        .ok()
        .and_then(|c| c.last_selected_version.clone())
}

pub fn set_last_selected_version(version: &str) -> Result<(), LauncherError> {
    let v = version.to_string();
    set_config_value(|config| config.last_selected_version = Some(v))
}

pub fn get_custom_java_paths() -> Vec<String> {
    load_config()
        .map(|c| c.custom_java_paths.clone())
        .unwrap_or_default()
}

pub fn add_custom_java_path(path: &str) -> Result<(), LauncherError> {
    let p = path.to_string();
    set_config_value(|config| {
        if !config
            .custom_java_paths
            .iter()
            .any(|x| x.eq_ignore_ascii_case(&p))
        {
            config.custom_java_paths.push(p);
        }
    })
}

pub fn remove_custom_java_path(path: &str) -> Result<(), LauncherError> {
    let p = path.to_string();
    set_config_value(|config| {
        config
            .custom_java_paths
            .retain(|x| !x.eq_ignore_ascii_case(&p));
    })
}

#[cfg(test)]
mod tests {
    use super::{ConfigKey, default_game_dir, file_stamp, validate_download_threads};
    use std::fs;
    use std::path::PathBuf;

    #[test]
    fn test_game_dir_is_next_to_exe() {
        let exe = PathBuf::from("C:/launcher");
        assert_eq!(default_game_dir(&exe), exe.join(".minecraft"));
    }

    #[test]
    fn test_download_threads_must_stay_in_range() {
        assert!(validate_download_threads(1).is_ok());
        assert!(validate_download_threads(256).is_ok());
        assert!(validate_download_threads(0).is_err());
        assert!(validate_download_threads(257).is_err());
    }

    /// 配置文件里现在带 token，落盘后必须能原样读回来
    #[test]
    fn test_config_file_keeps_tokens_in_one_file() {
        let config = crate::models::GameConfig {
            ms_access_token: Some("access".to_string()),
            ms_refresh_token: Some("refresh".to_string()),
            ms_expires_at: Some(123),
            ..Default::default()
        };

        let json = serde_json::to_string_pretty(&config).unwrap();
        let restored: crate::models::GameConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(restored.ms_access_token.as_deref(), Some("access"));
        assert_eq!(restored.ms_refresh_token.as_deref(), Some("refresh"));
        assert_eq!(restored.ms_expires_at, Some(123));
    }

    /// 诊断快照必须把 token 和账户标识抹掉，代理地址只留占位
    #[test]
    fn test_config_for_diagnostics_never_contains_secrets() {
        let config = crate::models::GameConfig {
            ms_access_token: Some("access".to_string()),
            ms_refresh_token: Some("refresh".to_string()),
            ms_expires_at: Some(123),
            username: Some("Steve".to_string()),
            uuid: Some("uuid".to_string()),
            skin_url: Some("https://skins.example/x".to_string()),
            proxy_url: Some("http://user:pass@proxy.example".to_string()),
            ..Default::default()
        };

        let snapshot = super::config_for_diagnostics(&config);

        assert!(snapshot.ms_access_token.is_none());
        assert!(snapshot.ms_refresh_token.is_none());
        assert!(snapshot.ms_expires_at.is_none());
        assert!(snapshot.username.is_none());
        assert!(snapshot.uuid.is_none());
        assert!(snapshot.skin_url.is_none());
        assert_eq!(snapshot.proxy_url.as_deref(), Some("***"));

        let raw = serde_json::to_string(&snapshot).unwrap();
        assert!(!raw.contains("access"));
        assert!(!raw.contains("refresh"));
        assert!(!raw.contains("user:pass"));
    }

    #[test]
    fn test_file_stamp_changes_when_file_changes() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.json");

        fs::write(&path, b"a").unwrap();
        let first = file_stamp(&path);

        fs::write(&path, b"ab").unwrap();
        let second = file_stamp(&path);
        assert_ne!(first, second);

        fs::remove_file(&path).unwrap();
        assert_ne!(second, file_stamp(&path));
    }

    #[test]
    fn test_ui_language_is_normalized_on_save() {
        let mut config = crate::models::GameConfig::default();
        let key = ConfigKey::UiLanguage;

        key.set_value(&mut config, "zh".to_string()).unwrap();
        assert_eq!(config.ui_language.as_deref(), Some("zh-CN"));

        key.set_value(&mut config, "EN_us".to_string()).unwrap();
        assert_eq!(config.ui_language.as_deref(), Some("en-US"));

        assert!(key.set_value(&mut config, "fr-FR".to_string()).is_err());
        assert!(key.set_value(&mut config, String::new()).is_err());
    }

    /// 旧版 credentials.json 只有这三个字段，缺省字段必须能容忍
    #[test]
    fn test_legacy_credentials_parsing() {
        let legacy: super::LegacyCredentials =
            serde_json::from_str(r#"{"access_token":"a","refresh_token":"r","expires_at":9}"#)
                .unwrap();
        assert!(!legacy.is_empty());
        assert_eq!(legacy.access_token.as_deref(), Some("a"));
        assert_eq!(legacy.expires_at, Some(9));

        let partial: super::LegacyCredentials = serde_json::from_str("{}").unwrap();
        assert!(partial.is_empty());
        assert!(super::LegacyCredentials::default().is_empty());
    }

    #[test]
    fn test_legacy_credentials_are_merged_into_config() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("credentials.json");
        fs::write(
            &path,
            r#"{"access_token":"old-access","refresh_token":"old-refresh","expires_at":42}"#,
        )
        .unwrap();

        let mut config = crate::models::GameConfig::default();
        assert!(super::merge_legacy_credentials_from(&path, &mut config));
        assert_eq!(config.ms_access_token.as_deref(), Some("old-access"));
        assert_eq!(config.ms_refresh_token.as_deref(), Some("old-refresh"));
        assert_eq!(config.ms_expires_at, Some(42));
    }

    /// 配置里已经有 token 时不能被更旧的凭据文件覆盖，否则刷新过的 token 会倒退
    #[test]
    fn test_legacy_credentials_never_override_newer_tokens() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("credentials.json");
        fs::write(&path, r#"{"access_token":"old","refresh_token":"old"}"#).unwrap();

        let mut config = crate::models::GameConfig {
            ms_access_token: Some("new-access".to_string()),
            ms_refresh_token: Some("new-refresh".to_string()),
            ..Default::default()
        };
        assert!(super::merge_legacy_credentials_from(&path, &mut config));
        assert_eq!(config.ms_access_token.as_deref(), Some("new-access"));
        assert_eq!(config.ms_refresh_token.as_deref(), Some("new-refresh"));
    }

    #[test]
    fn test_corrupt_legacy_credentials_are_ignored_but_still_removable() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("credentials.json");
        fs::write(&path, b"not json").unwrap();

        let mut config = crate::models::GameConfig::default();
        // 返回 true 才会触发一次写回，写回成功后坏文件被删掉
        assert!(super::merge_legacy_credentials_from(&path, &mut config));
        assert!(config.ms_access_token.is_none());
    }

    #[test]
    fn test_save_backup_settings_defaults_and_validation() {
        let config = crate::models::GameConfig::default();
        assert!(!config.save_auto_backup);
        assert_eq!(config.save_backup_retention, 3);

        let mut config = crate::models::GameConfig::default();
        let key = ConfigKey::SaveAutoBackup;
        key.set_value(&mut config, "true".to_string()).unwrap();
        assert!(config.save_auto_backup);

        let key = ConfigKey::SaveBackupRetention;
        key.set_value(&mut config, "10".to_string()).unwrap();
        assert_eq!(config.save_backup_retention, 10);
        assert!(key.set_value(&mut config, "0".to_string()).is_err());
        assert!(key.set_value(&mut config, "51".to_string()).is_err());
    }

    #[test]
    fn test_missing_legacy_credentials_need_no_write_back() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut config = crate::models::GameConfig::default();
        assert!(!super::merge_legacy_credentials_from(
            &dir.path().join("credentials.json"),
            &mut config
        ));
    }

    /// 端到端：旧版只写了 credentials.json，第一次读配置要把 token 并进 ar1s.json 并删掉旧文件
    #[test]
    fn test_legacy_credentials_are_folded_into_ar1s_json_on_first_read() {
        let root = tempfile::TempDir::new().unwrap();
        let storage = root.path();
        fs::write(
            storage.join("credentials.json"),
            r#"{"access_token":"acc","refresh_token":"ref","expires_at":77}"#,
        )
        .unwrap();
        fs::write(
            storage.join("ar1s.json"),
            r#"{"game_dir":"D:/mc","auth_type":"microsoft"}"#,
        )
        .unwrap();

        let loaded = super::read_config_from(storage).unwrap();
        assert!(loaded.needs_save, "有旧凭据时应当触发一次写回");
        assert_eq!(loaded.config.ms_access_token.as_deref(), Some("acc"));
        assert_eq!(loaded.config.ms_refresh_token.as_deref(), Some("ref"));

        super::save_config_to(storage, &loaded.config).unwrap();

        // 旧文件消失，token 落在 ar1s.json 里
        assert!(!storage.join("credentials.json").exists());
        let raw = fs::read_to_string(storage.join("ar1s.json")).unwrap();
        assert!(raw.contains("acc"));
        assert!(raw.contains("ref"));

        // 再读一次不该再触发写回
        let again = super::read_config_from(storage).unwrap();
        assert!(!again.needs_save);
        assert_eq!(again.config.ms_access_token.as_deref(), Some("acc"));
    }

    /// 登出后 token 为空也必须真的写进文件，否则下次启动会把旧 token 读回来
    #[test]
    fn test_logout_clears_tokens_from_disk() {
        let root = tempfile::TempDir::new().unwrap();
        let storage = root.path();
        let mut config = crate::models::GameConfig {
            game_dir: "D:/mc".to_string(),
            ms_access_token: Some("acc".to_string()),
            ms_refresh_token: Some("ref".to_string()),
            ..Default::default()
        };
        super::save_config_to(storage, &config).unwrap();

        config.ms_access_token = None;
        config.ms_refresh_token = None;
        super::save_config_to(storage, &config).unwrap();

        let loaded = super::read_config_from(storage).unwrap();
        assert!(loaded.config.ms_access_token.is_none());
        assert!(loaded.config.ms_refresh_token.is_none());
    }

    /// token 与设置同文件后，全量写回不能把另一实例刚轮换的 token 盖回旧值
    #[test]
    fn test_stale_snapshot_does_not_clobber_rotated_tokens() {
        let root = tempfile::TempDir::new().unwrap();
        let storage = root.path();

        // 磁盘上已经是最新的 token（另一个实例刚刷新过）
        let rotated = crate::models::GameConfig {
            game_dir: "D:/mc".to_string(),
            ms_access_token: Some("fresh-access".to_string()),
            ms_refresh_token: Some("fresh-refresh".to_string()),
            ms_expires_at: Some(999),
            ..Default::default()
        };
        super::save_config_to(storage, &rotated).unwrap();

        // 本进程手里还是旧快照，只想改个内存设置
        let stale = crate::models::GameConfig {
            game_dir: "D:/mc".to_string(),
            max_memory: 8192,
            ms_access_token: Some("stale-access".to_string()),
            ms_refresh_token: Some("stale-refresh".to_string()),
            ms_expires_at: Some(111),
            ..Default::default()
        };

        let mut merged = stale.clone();
        if let Some(current) = super::read_tokens_from_disk_at(storage)
            && super::has_tokens(&current)
        {
            merged.ms_access_token = current.ms_access_token;
            merged.ms_refresh_token = current.ms_refresh_token;
            merged.ms_expires_at = current.ms_expires_at;
        }
        super::save_config_to(storage, &merged).unwrap();

        let loaded = super::read_config_from(storage).unwrap();
        assert_eq!(
            loaded.config.ms_access_token.as_deref(),
            Some("fresh-access")
        );
        assert_eq!(
            loaded.config.ms_refresh_token.as_deref(),
            Some("fresh-refresh")
        );
        assert_eq!(loaded.config.max_memory, 8192, "非 token 字段仍要生效");
    }
}
