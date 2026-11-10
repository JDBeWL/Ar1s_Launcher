use crate::errors::LauncherError;
use crate::models::{
    DownloadJob, JavaDownloadOption, JavaDownloadPhase, JavaDownloadProgress, ManagedJavaState,
};
use crate::services::config;
use crate::services::download::{download_file_single, get_http_client, get_java_download_client};
use crate::services::java::JavaProbe;
use log::{info, warn};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter};
use zip::ZipArchive;

pub(crate) const JAVA_BIN: &str = if cfg!(windows) { "java.exe" } else { "java" };

#[derive(Debug, Clone)]
pub(crate) struct JavaPackage {
    pub(crate) feature: u32,
    pub(crate) release_name: String,
    pub(crate) package_name: String,
    pub(crate) size: u64,
    pub(crate) checksum: String,
    pub(crate) github_url: String,
}

impl JavaPackage {
    fn mirror_url(&self) -> String {
        format!(
            "https://mirrors.tuna.tsinghua.edu.cn/Adoptium/{}/jre/x64/windows/{}",
            self.feature, self.package_name
        )
    }
}

#[derive(Deserialize)]
struct AvailableReleases {
    #[serde(default)]
    available_lts_releases: Vec<u32>,
}

#[derive(Deserialize)]
struct AssetEntry {
    release_name: String,
    binary: AssetBinary,
}

#[derive(Deserialize)]
struct AssetBinary {
    image_type: String,
    os: String,
    architecture: String,
    package: AssetPackage,
}

#[derive(Deserialize)]
struct AssetPackage {
    name: String,
    size: u64,
    checksum: String,
    link: String,
}

pub(crate) fn parse_lts_features(text: &str) -> Result<Vec<u32>, LauncherError> {
    let parsed: AvailableReleases = serde_json::from_str(text)
        .map_err(|error| LauncherError::Custom(format!("解析 Adoptium 版本列表失败: {error}")))?;
    let mut features: Vec<u32> = parsed
        .available_lts_releases
        .into_iter()
        .filter(|feature| *feature >= 8)
        .collect();
    features.sort_unstable_by(|a, b| b.cmp(a));
    features.dedup();
    Ok(features)
}

pub(crate) fn parse_asset(feature: u32, text: &str) -> Result<Option<JavaPackage>, LauncherError> {
    let entries: Vec<AssetEntry> = serde_json::from_str(text)
        .map_err(|error| LauncherError::Custom(format!("解析 Adoptium 包信息失败: {error}")))?;

    Ok(entries.into_iter().find_map(|entry| {
        let binary = entry.binary;
        if binary.image_type != "jre"
            || binary.os != "windows"
            || !matches!(binary.architecture.as_str(), "x64" | "x86_64" | "amd64")
        {
            return None;
        }
        Some(JavaPackage {
            feature,
            release_name: entry.release_name,
            package_name: binary.package.name,
            size: binary.package.size,
            checksum: binary.package.checksum,
            github_url: binary.package.link,
        })
    }))
}

pub(crate) fn inspect_runtime(
    dir: &Path,
    feature: u32,
    probe: &dyn Fn(&Path) -> Option<JavaProbe>,
) -> (ManagedJavaState, Option<String>, Option<String>) {
    if !dir.exists() {
        return (ManagedJavaState::NotInstalled, None, None);
    }

    let exe = dir.join("bin").join(JAVA_BIN);
    let path = Some(exe.to_string_lossy().replace('\\', "/"));
    match probe(&exe) {
        Some(probe) if probe.version == Some(feature) => {
            (ManagedJavaState::Installed, probe.raw, path)
        }
        other => (
            ManagedJavaState::Broken,
            other.and_then(|probe| probe.raw),
            path,
        ),
    }
}

pub(crate) fn sha256_file(path: &Path) -> Result<String, LauncherError> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let digest = hasher.finalize();
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn part_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".part");
    PathBuf::from(name)
}

fn extract_archive(zip_path: &Path, destination: &Path) -> Result<(), LauncherError> {
    let file = fs::File::open(zip_path)?;
    let mut archive = ZipArchive::new(file)
        .map_err(|error| LauncherError::Custom(format!("打开安装包失败: {error}")))?;

    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| LauncherError::Custom(format!("读取安装包失败: {error}")))?;

        if entry
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            return Err(LauncherError::Custom(
                "安装包包含不支持的符号链接".to_string(),
            ));
        }

        let Some(relative) = entry.enclosed_name() else {
            return Err(LauncherError::Custom("安装包包含非法路径".to_string()));
        };
        let output = destination.join(relative);

        if entry.is_dir() {
            fs::create_dir_all(&output)?;
            continue;
        }
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = fs::File::create(&output)?;
        std::io::copy(&mut entry, &mut file)?;
    }

    Ok(())
}

fn locate_java_home(extracted: &Path) -> Result<PathBuf, LauncherError> {
    let has_java = |dir: &Path| dir.join("bin").join(JAVA_BIN).is_file();
    if has_java(extracted) {
        return Ok(extracted.to_path_buf());
    }

    let dirs: Vec<PathBuf> = fs::read_dir(extracted)?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    if dirs.len() == 1 && has_java(&dirs[0]) {
        return Ok(dirs[0].clone());
    }

    Err(LauncherError::Custom(
        "安装包内没有找到可用的 Java".to_string(),
    ))
}

fn install_archive_in(
    zip_path: &Path,
    feature: u32,
    root: &Path,
    probe: &dyn Fn(&Path) -> Option<JavaProbe>,
) -> Result<(), LauncherError> {
    fs::create_dir_all(root)?;
    let target = root.join(format!("temurin-{feature}"));
    if target.exists() {
        return Err(LauncherError::Custom(format!(
            "Java {feature} 的目标目录已存在，请先删除"
        )));
    }

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    let temp = root.join(format!(".tmp-temurin-{feature}-{stamp}"));

    let result = (|| {
        fs::create_dir_all(&temp)?;
        extract_archive(zip_path, &temp)?;
        let home = locate_java_home(&temp)?;
        let exe = home.join("bin").join(JAVA_BIN);
        let probed = probe(&exe)
            .ok_or_else(|| LauncherError::Custom("安装包内的 Java 无法运行".to_string()))?;
        if probed.version != Some(feature) {
            return Err(LauncherError::Custom(format!(
                "安装包内的 Java 版本不是 {feature}"
            )));
        }
        fs::rename(&home, &target)?;
        Ok(())
    })();

    if result.is_err() || temp.exists() {
        let _ = fs::remove_dir_all(&temp);
    }
    result
}

pub(crate) fn managed_dir_for_delete_at(
    root: &Path,
    feature: u32,
) -> Result<PathBuf, LauncherError> {
    let target = root.join(format!("temurin-{feature}"));
    if !target.is_dir() {
        return Err(LauncherError::Custom(format!("Java {feature} 未安装")));
    }

    let canonical_root = root
        .canonicalize()
        .map_err(|error| LauncherError::Custom(format!("无法定位 runtime/java: {error}")))?;
    let canonical_target = target.canonicalize()?;
    let expected_name = format!("temurin-{feature}");
    if canonical_target.parent() != Some(canonical_root.as_path())
        || canonical_target.file_name().and_then(|name| name.to_str())
            != Some(expected_name.as_str())
    {
        return Err(LauncherError::Custom(
            "拒绝删除不在 runtime/java 下的目录".to_string(),
        ));
    }

    Ok(canonical_target)
}

pub(crate) fn path_inside(candidate: &str, dir: &Path) -> bool {
    let resolved = crate::services::launcher::java::resolve_path(candidate);
    let exe = crate::services::java::resolve_java_exe(candidate);
    let dir = normalized_path(dir);

    [resolved, exe].iter().any(|path| {
        let path = normalized_path(path);
        path == dir || path.starts_with(&format!("{dir}/"))
    })
}

fn normalized_path(path: &Path) -> String {
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let raw = path.to_string_lossy();
    let stripped = raw.strip_prefix(r"\\?\").unwrap_or(&raw);
    stripped.replace('\\', "/").to_lowercase()
}

const CATALOG_TTL: Duration = Duration::from_secs(6 * 3600);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const PROGRESS_INTERVAL: Duration = Duration::from_millis(200);

type CatalogCache = Option<(Instant, Vec<JavaPackage>)>;

static CATALOG_CACHE: LazyLock<Mutex<CatalogCache>> = LazyLock::new(|| Mutex::new(None));

#[derive(Clone)]
struct ActiveDownload {
    feature: u32,
    cancel: Arc<AtomicBool>,
    running: Arc<AtomicBool>,
    progress: Arc<Mutex<JavaDownloadProgress>>,
}

static ACTIVE_DOWNLOAD: LazyLock<Mutex<Option<ActiveDownload>>> =
    LazyLock::new(|| Mutex::new(None));

fn managed_root() -> PathBuf {
    config::init_storage().join("runtime").join("java")
}

fn runtime_dir(feature: u32) -> PathBuf {
    managed_root().join(format!("temurin-{feature}"))
}

fn downloads_dir() -> PathBuf {
    managed_root().join(".downloads")
}

fn lock_active() -> std::sync::MutexGuard<'static, Option<ActiveDownload>> {
    ACTIVE_DOWNLOAD
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn lock_catalog() -> std::sync::MutexGuard<'static, CatalogCache> {
    CATALOG_CACHE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

async fn fetch_text(client: &reqwest::Client, url: String) -> Result<String, LauncherError> {
    let response = client
        .get(url)
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|error| LauncherError::Custom(format!("连接 Adoptium 失败: {error}")))?;
    if !response.status().is_success() {
        return Err(LauncherError::Custom(format!(
            "Adoptium 返回 HTTP {}",
            response.status()
        )));
    }
    response
        .text()
        .await
        .map_err(|error| LauncherError::Custom(format!("读取 Adoptium 响应失败: {error}")))
}

async fn fetch_catalog() -> Result<Vec<JavaPackage>, LauncherError> {
    let client = get_http_client();
    let releases = fetch_text(
        &client,
        "https://api.adoptium.net/v3/info/available_releases".to_string(),
    )
    .await?;
    let features = parse_lts_features(&releases)?;

    let mut tasks = tokio::task::JoinSet::new();
    for feature in features {
        let client = client.clone();
        tasks.spawn(async move {
            let url = format!(
                "https://api.adoptium.net/v3/assets/latest/{feature}/hotspot?architecture=x64&image_type=jre&os=windows&vendor=eclipse"
            );
            let text = fetch_text(&client, url).await?;
            parse_asset(feature, &text)
        });
    }

    let mut packages = Vec::new();
    while let Some(joined) = tasks.join_next().await {
        match joined {
            Ok(Ok(Some(package))) => packages.push(package),
            Ok(Ok(None)) => warn!("Adoptium 未返回 Windows x64 JRE"),
            Ok(Err(error)) => warn!("获取 Adoptium 包信息失败: {}", error),
            Err(error) => warn!("Adoptium 包信息任务异常终止: {}", error),
        }
    }

    if packages.is_empty() {
        return Err(LauncherError::Custom(
            "没有获取到可下载的 Java 版本".to_string(),
        ));
    }
    packages.sort_by_key(|package| std::cmp::Reverse(package.feature));
    Ok(packages)
}

async fn load_catalog() -> Result<Vec<JavaPackage>, LauncherError> {
    {
        let cache = lock_catalog();
        if let Some((fetched_at, packages)) = cache.as_ref()
            && fetched_at.elapsed() < CATALOG_TTL
        {
            return Ok(packages.clone());
        }
    }

    match fetch_catalog().await {
        Ok(packages) => {
            *lock_catalog() = Some((Instant::now(), packages.clone()));
            Ok(packages)
        }
        Err(error) => {
            let stale = lock_catalog().clone();
            if let Some((_, packages)) = stale {
                warn!("Adoptium 目录获取失败，使用缓存: {}", error);
                Ok(packages)
            } else {
                Err(error)
            }
        }
    }
}

pub async fn options() -> Result<Vec<JavaDownloadOption>, LauncherError> {
    let packages = load_catalog().await?;
    tokio::task::spawn_blocking(move || {
        packages
            .into_iter()
            .map(|package| {
                let dir = runtime_dir(package.feature);
                let (state, installed_version, installed_path) =
                    inspect_runtime(&dir, package.feature, &|exe| {
                        crate::services::java::probe_java_exe(exe)
                    });
                JavaDownloadOption {
                    feature: package.feature,
                    release_name: package.release_name,
                    package_name: package.package_name,
                    size: package.size,
                    state,
                    installed_version,
                    installed_path,
                }
            })
            .collect()
    })
    .await
    .map_err(|error| LauncherError::Custom(format!("Java 状态检查任务异常终止: {error}")))
}

pub fn current_progress() -> Option<JavaDownloadProgress> {
    let active = lock_active().clone()?;
    let progress = active
        .progress
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    Some(progress.clone())
}

fn begin_download(feature: u32) -> Result<ActiveDownload, LauncherError> {
    let mut slot = lock_active();
    if let Some(active) = slot.as_ref() {
        return Err(LauncherError::Custom(format!(
            "已有 Java {} 下载任务在进行",
            active.feature
        )));
    }

    let active = ActiveDownload {
        feature,
        cancel: Arc::new(AtomicBool::new(false)),
        running: Arc::new(AtomicBool::new(true)),
        progress: Arc::new(Mutex::new(JavaDownloadProgress {
            feature,
            phase: JavaDownloadPhase::Downloading,
            bytes_downloaded: 0,
            total_bytes: 0,
            speed: 0.0,
            percent: 0,
            error: None,
        })),
    };
    *slot = Some(active.clone());
    Ok(active)
}

fn finish_download(active: &ActiveDownload) {
    let mut slot = lock_active();
    if slot
        .as_ref()
        .is_some_and(|current| current.feature == active.feature)
    {
        *slot = None;
    }
}

pub fn cancel() {
    if let Some(active) = lock_active().as_ref() {
        active.cancel.store(true, Ordering::Release);
    }
}

fn publish(app: &AppHandle, active: &ActiveDownload, progress: JavaDownloadProgress) {
    if let Ok(mut slot) = active.progress.lock() {
        *slot = progress.clone();
    }
    let _ = app.emit("java-download-progress", &progress);
}

fn publish_phase(
    app: &AppHandle,
    active: &ActiveDownload,
    phase: JavaDownloadPhase,
    percent: u8,
    error: Option<String>,
) {
    let mut progress = active
        .progress
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    progress.phase = phase;
    progress.percent = percent;
    progress.speed = 0.0;
    progress.error = error;
    publish(app, active, progress);
}

fn spawn_ticker(
    app: AppHandle,
    active: ActiveDownload,
    downloaded: Arc<AtomicU64>,
    since_last: Arc<AtomicU64>,
    total_bytes: u64,
) -> tauri::async_runtime::JoinHandle<()> {
    tauri::async_runtime::spawn(async move {
        let mut last = Instant::now();
        while active.running.load(Ordering::Acquire) {
            tokio::time::sleep(PROGRESS_INTERVAL).await;
            if !active.running.load(Ordering::Acquire) {
                break;
            }

            let current = downloaded.load(Ordering::Relaxed);
            let delta = since_last.swap(0, Ordering::Relaxed);
            let elapsed = last.elapsed().as_secs_f64();
            last = Instant::now();
            if delta == 0 || elapsed <= 0.0 {
                continue;
            }

            let percent = if total_bytes > 0 {
                ((current as f64 / total_bytes as f64) * 100.0)
                    .round()
                    .clamp(0.0, 100.0) as u8
            } else {
                0
            };
            publish(
                &app,
                &active,
                JavaDownloadProgress {
                    feature: active.feature,
                    phase: JavaDownloadPhase::Downloading,
                    bytes_downloaded: current,
                    total_bytes,
                    speed: delta as f64 / 1024.0 / elapsed,
                    percent,
                    error: None,
                },
            );
        }
    })
}

async fn run_download(
    app: &AppHandle,
    active: &ActiveDownload,
    package: &JavaPackage,
) -> Result<(), LauncherError> {
    let downloads = downloads_dir();
    fs::create_dir_all(&downloads)?;

    // 压缩包落地 + 解压是同一块盘，解压后通常是压缩包的 2.5 倍左右，按 3 倍预检
    crate::utils::disk_space::ensure_free_space(&downloads, package.size.saturating_mul(3))?;

    let zip_path = downloads.join(format!("temurin-{}.zip", package.feature));
    let job = DownloadJob::new(
        package.mirror_url(),
        vec![package.github_url.clone()],
        zip_path.clone(),
        package.size,
        None,
    );

    let downloaded = Arc::new(AtomicU64::new(0));
    let since_last = Arc::new(AtomicU64::new(0));
    let ticker = spawn_ticker(
        app.clone(),
        active.clone(),
        downloaded.clone(),
        since_last.clone(),
        package.size,
    );

    let http = get_java_download_client();
    let result = download_file_single(
        &http,
        &job,
        &job.url,
        &active.running,
        &active.cancel,
        &downloaded,
        &since_last,
    )
    .await;
    active.running.store(false, Ordering::Release);
    let _ = ticker.await;
    result?;

    if active.cancel.load(Ordering::Acquire) {
        return Err(LauncherError::Cancelled);
    }

    publish_phase(app, active, JavaDownloadPhase::Extracting, 100, None);

    let zip_for_hash = zip_path.clone();
    let actual = tokio::task::spawn_blocking(move || sha256_file(&zip_for_hash))
        .await
        .map_err(|error| LauncherError::Custom(format!("校验安装包任务异常终止: {error}")))??;
    if !actual.eq_ignore_ascii_case(&package.checksum) {
        let _ = fs::remove_file(&zip_path);
        let _ = fs::remove_file(part_path(&zip_path));
        return Err(LauncherError::Custom("安装包校验失败，请重试".to_string()));
    }

    let zip_for_install = zip_path.clone();
    let feature = package.feature;
    tokio::task::spawn_blocking(move || {
        install_archive_in(&zip_for_install, feature, &managed_root(), &|exe| {
            crate::services::java::probe_java_exe(exe)
        })
    })
    .await
    .map_err(|error| LauncherError::Custom(format!("安装 Java 任务异常终止: {error}")))??;

    let _ = fs::remove_file(&zip_path);
    crate::services::java::invalidate_java_cache();
    publish_phase(app, active, JavaDownloadPhase::Completed, 100, None);
    info!("Java {} 安装完成", feature);
    Ok(())
}

pub async fn download(app: AppHandle, feature: u32) -> Result<(), LauncherError> {
    let packages = load_catalog().await?;
    let package = packages
        .into_iter()
        .find(|package| package.feature == feature)
        .ok_or_else(|| LauncherError::Custom(format!("不支持下载 Java {feature}")))?;

    if runtime_dir(feature).exists() {
        return Err(LauncherError::Custom(format!(
            "Java {feature} 的目标目录已存在，请先删除"
        )));
    }

    let active = begin_download(feature)?;
    let result = run_download(&app, &active, &package).await;
    if let Err(error) = &result {
        if error.is_cancelled() {
            publish_phase(&app, &active, JavaDownloadPhase::Cancelled, 0, None);
        } else {
            publish_phase(
                &app,
                &active,
                JavaDownloadPhase::Error,
                0,
                Some(error.to_string()),
            );
        }
    }
    finish_download(&active);
    result
}

pub async fn delete(feature: u32) -> Result<(), LauncherError> {
    if lock_active().is_some() {
        return Err(LauncherError::Custom(
            "正在下载 Java，请稍后再删除".to_string(),
        ));
    }

    let target = managed_dir_for_delete_at(&managed_root(), feature)?;
    let target_for_remove = target.clone();
    tokio::task::spawn_blocking(move || fs::remove_dir_all(&target_for_remove))
        .await
        .map_err(|error| LauncherError::Custom(format!("删除 Java 任务异常终止: {error}")))?
        .map_err(|error| {
            LauncherError::Custom(format!("删除失败，可能有游戏正在使用该 Java: {error}"))
        })?;

    let target_for_config = target.clone();
    if let Err(error) = config::update_config_async(move |config| {
        if config
            .java_path
            .as_deref()
            .is_some_and(|path| path_inside(path, &target_for_config))
        {
            config.java_path = None;
        }
        config
            .custom_java_paths
            .retain(|path| !path_inside(path, &target_for_config));
        Ok(())
    })
    .await
    {
        warn!("清理已删除 Java 的配置失败: {}", error);
    }

    crate::services::java::invalidate_java_cache();
    info!("已删除启动器安装的 Java {}", feature);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ManagedJavaState;
    use crate::services::java::{JavaArch, JavaProbe};
    use std::fs;
    use std::io::Write;

    fn probe_21(_: &Path) -> Option<JavaProbe> {
        Some(JavaProbe {
            version: Some(21),
            raw: Some("21.0.12.1".to_string()),
            arch: JavaArch::Bits64,
            home: None,
        })
    }

    #[test]
    fn test_parse_lts_features_sorts_desc_and_dedups() {
        let text = r#"{
            "available_lts_releases": [8, 11, 17, 21, 25, 21],
            "available_releases": [8, 16, 17, 21, 25, 26]
        }"#;
        assert_eq!(parse_lts_features(text).unwrap(), vec![25, 21, 17, 11, 8]);
    }

    #[test]
    fn test_parse_asset_keeps_only_windows_x64_jre() {
        let text = r#"[{
            "release_name": "jdk-21.0.12.1+1",
            "binary": {
                "image_type": "jre",
                "os": "windows",
                "architecture": "x64",
                "package": {
                    "name": "OpenJDK21U-jre_x64_windows_hotspot_21.0.12.1_1.zip",
                    "size": 49232123,
                    "checksum": "abc123",
                    "link": "https://github.com/adoptium/temurin21-binaries/releases/download/x.zip"
                }
            }
        }]"#;
        let package = parse_asset(21, text).unwrap().unwrap();
        assert_eq!(package.feature, 21);
        assert_eq!(package.release_name, "jdk-21.0.12.1+1");
        assert_eq!(package.size, 49232123);
        assert_eq!(package.checksum, "abc123");
        assert_eq!(
            package.mirror_url(),
            "https://mirrors.tuna.tsinghua.edu.cn/Adoptium/21/jre/x64/windows/OpenJDK21U-jre_x64_windows_hotspot_21.0.12.1_1.zip"
        );

        let jdk = text.replace("\"image_type\": \"jre\"", "\"image_type\": \"jdk\"");
        assert!(parse_asset(21, &jdk).unwrap().is_none());

        let linux = text.replace("\"os\": \"windows\"", "\"os\": \"linux\"");
        assert!(parse_asset(21, &linux).unwrap().is_none());
    }

    #[test]
    fn test_inspect_runtime_marks_installed_broken_and_missing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().join("temurin-21");
        fs::create_dir_all(dir.join("bin")).unwrap();
        fs::write(dir.join("bin").join(JAVA_BIN), b"x").unwrap();

        let (state, version, path) = inspect_runtime(&dir, 21, &probe_21);
        assert_eq!(state, ManagedJavaState::Installed);
        assert_eq!(version.as_deref(), Some("21.0.12.1"));
        assert!(path.unwrap().ends_with(&format!("bin/{JAVA_BIN}")));

        let wrong = |_: &Path| {
            Some(JavaProbe {
                version: Some(17),
                raw: Some("17.0.8".to_string()),
                arch: JavaArch::Bits64,
                home: None,
            })
        };
        assert_eq!(
            inspect_runtime(&dir, 21, &wrong).0,
            ManagedJavaState::Broken
        );
        assert_eq!(
            inspect_runtime(&dir, 21, &|_| None).0,
            ManagedJavaState::Broken
        );
        assert_eq!(
            inspect_runtime(&tmp.path().join("missing"), 21, &probe_21).0,
            ManagedJavaState::NotInstalled
        );
    }

    #[test]
    fn test_sha256_file_matches_known_digest() {
        let tmp = tempfile::TempDir::new().unwrap();
        let file = tmp.path().join("a.bin");
        fs::write(&file, b"abc").unwrap();
        assert_eq!(
            sha256_file(&file).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    fn write_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let mut zip = zip::ZipWriter::new(fs::File::create(path).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        for (name, bytes) in entries {
            zip.start_file(*name, options).unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap();
    }

    #[test]
    fn test_extract_archive_rejects_traversal() {
        let tmp = tempfile::TempDir::new().unwrap();
        let zip_path = tmp.path().join("evil.zip");
        write_zip(&zip_path, &[("../evil.txt", b"x")]);

        let destination = tmp.path().join("out");
        fs::create_dir_all(&destination).unwrap();
        assert!(extract_archive(&zip_path, &destination).is_err());
        assert!(!tmp.path().join("evil.txt").exists());
    }

    #[test]
    fn test_install_archive_moves_top_level_directory() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().join("runtime/java");
        let zip_path = tmp.path().join("jre.zip");
        write_zip(
            &zip_path,
            &[(format!("jdk-21-jre/bin/{JAVA_BIN}").as_str(), b"x")],
        );

        install_archive_in(&zip_path, 21, &root, &probe_21).unwrap();

        let target = root.join("temurin-21");
        assert!(target.join("bin").join(JAVA_BIN).is_file());
        assert!(
            fs::read_dir(&root)
                .unwrap()
                .flatten()
                .all(|entry| !entry.file_name().to_string_lossy().starts_with(".tmp-")),
            "临时目录必须清理"
        );
    }

    #[test]
    fn test_install_archive_rejects_missing_java_binary() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().join("runtime/java");
        let zip_path = tmp.path().join("bad.zip");
        write_zip(&zip_path, &[("readme.txt", b"x")]);

        assert!(install_archive_in(&zip_path, 21, &root, &probe_21).is_err());
        assert!(!root.join("temurin-21").exists());
    }

    #[test]
    fn test_install_archive_rejects_wrong_major() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().join("runtime/java");
        let zip_path = tmp.path().join("jre.zip");
        write_zip(
            &zip_path,
            &[(format!("jdk-17-jre/bin/{JAVA_BIN}").as_str(), b"x")],
        );

        let probe_17 = |_: &Path| {
            Some(JavaProbe {
                version: Some(17),
                raw: Some("17.0.8".to_string()),
                arch: JavaArch::Bits64,
                home: None,
            })
        };
        assert!(install_archive_in(&zip_path, 21, &root, &probe_17).is_err());
        assert!(!root.join("temurin-21").exists());
    }

    #[test]
    fn test_managed_delete_rejects_file_and_missing_dir() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().join("runtime/java");
        fs::create_dir_all(root.join("temurin-21")).unwrap();

        assert!(managed_dir_for_delete_at(&root, 21).is_ok());
        assert!(managed_dir_for_delete_at(&root, 17).is_err());

        fs::write(root.join("temurin-99"), b"x").unwrap();
        assert!(managed_dir_for_delete_at(&root, 99).is_err());
    }

    #[test]
    fn test_path_inside_handles_exe_and_directory_forms() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().join("runtime/java/temurin-21");
        fs::create_dir_all(dir.join("bin")).unwrap();
        let exe = dir.join("bin").join(JAVA_BIN);
        fs::write(&exe, b"x").unwrap();

        assert!(path_inside(exe.to_str().unwrap(), &dir));
        assert!(path_inside(dir.to_str().unwrap(), &dir));
        assert!(path_inside(dir.join("child").to_str().unwrap(), &dir));
        assert!(!path_inside(
            tmp.path().join("runtime/java/other").to_str().unwrap(),
            &dir
        ));
    }
}
