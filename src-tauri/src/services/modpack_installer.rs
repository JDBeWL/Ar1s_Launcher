//! 整合包安装模块
//!
//! 下载 Modrinth 整合包或直接读取本地 .mrpack，按 modrinth.index.json 并发补下模组与依赖加载器，
//! 失败或取消时回滚实例。在线安装与本地导入共用同一条流水线，区别只在压缩包与展示信息从哪来。

use crate::errors::LauncherError;
use crate::models::DownloadJob;
use crate::models::modpack::*;
use crate::services::launcher::isolation;
use crate::services::modpack_format::{
    self, ModrinthDependencies, ModrinthIndex, ModrinthIndexFile,
};
use crate::services::{config, download, loaders, modrinth};
use crate::utils::file_utils::{self, validate_instance_name_or_error};
use log::{error, info, warn};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::Emitter;

static MODPACK_CANCEL_FLAG: std::sync::OnceLock<Arc<AtomicBool>> = std::sync::OnceLock::new();

const MAX_EXTRACTED_BYTES: u64 = 8 * 1024 * 1024 * 1024;

/// 单个整合包的压缩包上限，防止误选超大文件把临时目录塞满
const MAX_MODPACK_ARCHIVE_BYTES: u64 = 4 * 1024 * 1024 * 1024;

fn get_cancel_flag() -> Arc<AtomicBool> {
    MODPACK_CANCEL_FLAG
        .get_or_init(|| Arc::new(AtomicBool::new(false)))
        .clone()
}

pub fn reset_modpack_cancel_flag() {
    get_cancel_flag().store(false, Ordering::SeqCst);
}

pub fn set_modpack_cancel_flag() {
    get_cancel_flag().store(true, Ordering::SeqCst);
    // 原版下载走独立的取消标志；不联动的话取消要等整轮下载跑完才生效
    crate::services::download::batch::set_cancel_flag();
}

fn is_cancelled() -> bool {
    get_cancel_flag().load(Ordering::SeqCst)
}

fn check_cancelled() -> Result<(), LauncherError> {
    if is_cancelled() {
        Err(LauncherError::Cancelled)
    } else {
        Ok(())
    }
}

#[derive(Clone, serde::Serialize)]
pub struct ModpackInstallProgress {
    pub progress: u8,
    pub message: String,
    pub indeterminate: bool,
}

#[derive(Clone, serde::Serialize)]
pub struct ModpackActiveFile {
    pub path: String,
    pub downloaded: u64,
    pub total: u64,
}

#[derive(Clone, serde::Serialize)]
pub struct ModpackFilesProgress {
    pub files: Vec<ModpackActiveFile>,
    pub completed_files: usize,
    pub total_files: usize,
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
    pub speed: f64,
}

/// 本地 .mrpack 的预览信息，用于安装前展示与默认实例名
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalModpackPreview {
    pub path: String,
    pub name: String,
    pub version_id: String,
    pub summary: Option<String>,
    pub minecraft: String,
    /// Modrinth 加载器标识：forge / fabric / quilt / neoforge，原版为空
    pub loader: Option<String>,
    pub loader_version: Option<String>,
    /// 需要下载的文件数（已排除服务端专用文件）
    pub file_count: usize,
    pub total_size: u64,
    pub suggested_instance_name: String,
}

const FILE_PENDING: u8 = 0;
const FILE_DOWNLOADING: u8 = 1;
const FILE_COMPLETED: u8 = 2;
const FILE_FAILED: u8 = 3;

struct ModpackFileState {
    path: String,
    size: u64,
    downloaded: Arc<AtomicU64>,
    status: AtomicU8,
}

impl ModpackFileState {
    fn new(path: String, size: u64) -> Arc<Self> {
        Arc::new(Self {
            path,
            size,
            downloaded: Arc::new(AtomicU64::new(0)),
            status: AtomicU8::new(FILE_PENDING),
        })
    }
}

fn snapshot_files(states: &[Arc<ModpackFileState>], speed: f64) -> ModpackFilesProgress {
    let mut files = Vec::new();
    let mut completed_files = 0usize;
    let mut downloaded_bytes = 0u64;
    let mut total_bytes = 0u64;

    for state in states {
        let downloaded = state.downloaded.load(Ordering::Relaxed);
        downloaded_bytes = downloaded_bytes.saturating_add(downloaded);
        total_bytes = total_bytes.saturating_add(state.size);

        match state.status.load(Ordering::Acquire) {
            FILE_DOWNLOADING => files.push(ModpackActiveFile {
                path: state.path.clone(),
                downloaded,
                total: state.size,
            }),
            FILE_COMPLETED => completed_files += 1,
            _ => {}
        }
    }

    ModpackFilesProgress {
        files,
        completed_files,
        total_files: states.len(),
        downloaded_bytes,
        total_bytes,
        speed,
    }
}

fn dispatch_order(files: &[ModrinthIndexFile]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..files.len()).collect();
    // 大文件先占并发槽，避免最后只剩几个大文件把安装拖住
    order.sort_by_key(|&index| std::cmp::Reverse(files[index].file_size.unwrap_or(0)));
    order
}

const FILE_PROGRESS_INTERVAL: Duration = Duration::from_millis(250);

fn spawn_file_progress_reporter(
    states: Arc<Vec<Arc<ModpackFileState>>>,
    window: tauri::Window,
    running: Arc<AtomicBool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(FILE_PROGRESS_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut last_bytes = 0u64;

        loop {
            ticker.tick().await;
            if !running.load(Ordering::Acquire) {
                break;
            }

            let downloaded: u64 = states
                .iter()
                .map(|state| state.downloaded.load(Ordering::Relaxed))
                .sum();
            let speed =
                downloaded.saturating_sub(last_bytes) as f64 / FILE_PROGRESS_INTERVAL.as_secs_f64();
            last_bytes = downloaded;

            let _ = window.emit(
                "modpack-file-progress",
                snapshot_files(states.as_slice(), speed),
            );
        }
    })
}

/// 一次安装涉及的所有落盘位置
struct InstallTargets {
    game_dir: PathBuf,
    instance_dir: PathBuf,
    content_root: PathBuf,
    temp_dir: PathBuf,
    extract_dir: PathBuf,
}

/// 待安装的包：压缩包本体 + 已经解析好的清单
struct ModpackPlan {
    /// 展示用名称（Modrinth 项目名或清单里的 name）
    display_name: String,
    /// 展示用版本号（Modrinth 版本号或清单里的 versionId）
    version_label: String,
    /// instance.json 里的 source 字段
    source: &'static str,
    modpack_id: String,
    icon_url: Option<String>,
    loaders: Vec<String>,
    index: ModrinthIndex,
    archive_path: PathBuf,
    /// 在线安装下载到 temp 的包可以删；用户自己的本地文件绝不能删
    remove_archive_after: bool,
}

pub struct ModpackInstaller {
    modrinth_service: modrinth::ModrinthService,
}

static INSTALLER: std::sync::LazyLock<ModpackInstaller> =
    std::sync::LazyLock::new(ModpackInstaller::new);

pub fn get_installer() -> &'static ModpackInstaller {
    &INSTALLER
}

impl ModpackInstaller {
    fn new() -> Self {
        Self {
            modrinth_service: modrinth::ModrinthService::new(),
        }
    }

    /// 在线安装：从 Modrinth 拉项目信息与版本，下载 .mrpack 后走统一流水线
    pub async fn install_modrinth_modpack(
        &self,
        options: ModpackInstallOptions,
        window: &tauri::Window,
    ) -> Result<(), LauncherError> {
        reset_modpack_cancel_flag();
        validate_instance_name_or_error(&options.instance_name)?;

        let config = config::load_config()?;
        let targets = build_targets(&config, &options.instance_name);
        let _install_lock = acquire_install_lock(&targets).await?;
        ensure_instance_free(&targets, &options.instance_name)?;

        let result = async {
            let plan = self
                .prepare_modrinth_plan(&options, &targets, window)
                .await?;
            let outcome = self
                .install_plan(&plan, &options.instance_name, &targets, window)
                .await;

            if plan.remove_archive_after {
                let _ = fs::remove_file(&plan.archive_path);
            }
            outcome
        }
        .await;

        finalize_install(&config, &targets, result)
    }

    /// 本地导入：直接读取用户选中的 .mrpack，其余流程与在线安装一致
    pub async fn install_local_modpack(
        &self,
        options: LocalModpackInstallOptions,
        window: &tauri::Window,
    ) -> Result<(), LauncherError> {
        reset_modpack_cancel_flag();
        validate_instance_name_or_error(&options.instance_name)?;

        let archive_path = PathBuf::from(&options.file_path);
        validate_local_archive(&archive_path)?;

        let config = config::load_config()?;
        let targets = build_targets(&config, &options.instance_name);
        let _install_lock = acquire_install_lock(&targets).await?;
        ensure_instance_free(&targets, &options.instance_name)?;

        let result = async {
            let index = read_archive_index(&archive_path).await?;
            let file_stem = file_stem_of(&archive_path);
            let loader = index.dependencies.loader_slug();
            let plan = ModpackPlan {
                display_name: trimmed_or(&index.name, &file_stem),
                version_label: trimmed_or(&index.version_id, "1.0.0"),
                source: "local",
                modpack_id: file_stem,
                icon_url: None,
                loaders: loader
                    .map(|slug| vec![slug.to_string()])
                    .unwrap_or_default(),
                index,
                archive_path,
                remove_archive_after: false,
            };
            self.install_plan(&plan, &options.instance_name, &targets, window)
                .await
        }
        .await;

        finalize_install(&config, &targets, result)
    }

    /// 只读预览本地包，供安装页在真正落盘前展示信息
    pub async fn preview_local_modpack(
        &self,
        file_path: String,
    ) -> Result<LocalModpackPreview, LauncherError> {
        let archive_path = PathBuf::from(&file_path);
        validate_local_archive(&archive_path)?;

        tokio::task::spawn_blocking(move || -> Result<LocalModpackPreview, LauncherError> {
            let index = modpack_format::read_index_from_archive(&archive_path)?;
            let client_files: Vec<&ModrinthIndexFile> = index
                .files
                .iter()
                .filter(|file| modpack_format::is_client_file(file))
                .collect();
            let total_size = client_files
                .iter()
                .map(|file| file.file_size.unwrap_or(0))
                .sum();
            let loader = index.dependencies.loader();
            let stem = file_stem_of(&archive_path);

            Ok(LocalModpackPreview {
                path: archive_path.to_string_lossy().to_string(),
                name: trimmed_or(&index.name, &stem),
                version_id: trimmed_or(&index.version_id, "1.0.0"),
                summary: index
                    .summary
                    .as_deref()
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .map(str::to_string),
                minecraft: index.dependencies.minecraft.clone(),
                loader: index.dependencies.loader_slug().map(str::to_string),
                loader_version: loader
                    .as_ref()
                    .map(|loaded| loaded.loader_version().to_string()),
                file_count: client_files.len(),
                total_size,
                suggested_instance_name: sanitize_instance_name(&trimmed_or(&index.name, &stem)),
            })
        })
        .await
        .map_err(|e| LauncherError::Custom(format!("读取整合包失败: {}", e)))?
    }

    async fn prepare_modrinth_plan(
        &self,
        options: &ModpackInstallOptions,
        targets: &InstallTargets,
        window: &tauri::Window,
    ) -> Result<ModpackPlan, LauncherError> {
        let send_progress = progress_emitter(window);

        send_progress(5, "检查实例目录...", false);
        check_cancelled()?;

        send_progress(10, "获取整合包信息...", false);
        check_cancelled()?;

        let modpack = self
            .modrinth_service
            .get_modpack(&options.modpack_id)
            .await
            .map_err(|e| LauncherError::Custom(format!("获取整合包信息失败: {}", e)))?;

        send_progress(15, "获取整合包版本...", false);
        check_cancelled()?;

        let versions = self
            .modrinth_service
            .get_modpack_versions(&options.modpack_id, None, None)
            .await
            .map_err(|e| LauncherError::Custom(format!("获取整合包版本失败: {}", e)))?;

        let selected_version = versions
            .iter()
            .find(|v| v.id == options.version_id)
            .ok_or_else(|| LauncherError::Custom("未找到指定的整合包版本".to_string()))?;

        send_progress(20, "下载整合包文件...", false);
        check_cancelled()?;

        let primary_file = selected_version
            .files
            .iter()
            .find(|f| f.primary)
            .or_else(|| selected_version.files.first())
            .ok_or_else(|| LauncherError::Custom("整合包没有可用的文件".to_string()))?;

        if !targets.temp_dir.exists() {
            fs::create_dir_all(&targets.temp_dir)?;
        }

        file_utils::validate_file_name_or_error(&primary_file.filename)?;
        let archive_path = targets.temp_dir.join(&primary_file.filename);
        let sha1 = (!primary_file.hashes.sha1.trim().is_empty())
            .then_some(primary_file.hashes.sha1.as_str());
        let cancel_flag = get_cancel_flag();

        self.modrinth_service
            .download_modpack_file(
                &primary_file.url,
                &archive_path,
                primary_file.size,
                sha1,
                &cancel_flag,
            )
            .await
            .map_err(|e| {
                if e.is_cancelled() {
                    LauncherError::Cancelled
                } else {
                    LauncherError::Custom(format!("下载整合包文件失败: {}", e))
                }
            })?;

        send_progress(30, "读取整合包清单...", false);
        check_cancelled()?;

        let index = read_archive_index(&archive_path).await?;

        Ok(ModpackPlan {
            display_name: modpack.title.clone(),
            version_label: selected_version.version_number.clone(),
            source: "modrinth",
            modpack_id: modpack.slug.clone(),
            icon_url: modpack.icon_url.clone(),
            loaders: selected_version.loaders.clone(),
            index,
            archive_path,
            remove_archive_after: true,
        })
    }

    /// 解压 → 复制 overrides → 下载清单文件 → 装游戏与加载器 → 写 instance.json
    async fn install_plan(
        &self,
        plan: &ModpackPlan,
        instance_name: &str,
        targets: &InstallTargets,
        window: &tauri::Window,
    ) -> Result<(), LauncherError> {
        let send_progress = progress_emitter(window);

        send_progress(35, "解压整合包...", false);
        check_cancelled()?;

        if targets.extract_dir.exists() {
            // 删除整个解压临时目录耗时不可控，不能放在 async worker 上
            let extract_dir_owned = targets.extract_dir.clone();
            tokio::task::spawn_blocking(move || fs::remove_dir_all(&extract_dir_owned))
                .await
                .map_err(|e| LauncherError::Custom(format!("清理旧目录失败: {}", e)))?
                .map_err(|e| LauncherError::Custom(format!("清理旧目录失败: {}", e)))?;
        }
        fs::create_dir_all(&targets.extract_dir)?;

        // 解压是纯同步阻塞活，几百 MB 的包会占住 async worker 几十秒：
        // 期间进度心跳和取消响应都会跟着卡住，必须挪到阻塞线程池
        let archive = plan.archive_path.clone();
        let extract_dir = targets.extract_dir.clone();
        tokio::task::spawn_blocking(move || {
            modpack_format::extract_archive(&archive, &extract_dir, MAX_EXTRACTED_BYTES)
        })
        .await
        .map_err(|e| LauncherError::Custom(format!("解压任务异常终止: {}", e)))?
        .map_err(|e| LauncherError::Custom(format!("解压整合包失败: {}", e)))?;

        send_progress(45, "处理整合包配置...", false);
        check_cancelled()?;

        fs::create_dir_all(&targets.instance_dir)?;
        if targets.content_root != targets.instance_dir {
            fs::create_dir_all(&targets.content_root)?;
        }

        send_progress(50, "复制整合包文件...", false);
        check_cancelled()?;

        let extract_dir = targets.extract_dir.clone();
        let content_root = targets.content_root.clone();
        tokio::task::spawn_blocking(move || copy_overrides(&extract_dir, &content_root))
            .await
            .map_err(|e| LauncherError::Custom(format!("复制任务异常终止: {}", e)))??;

        let client_files: Vec<ModrinthIndexFile> = plan
            .index
            .files
            .iter()
            .filter(|file| modpack_format::is_client_file(file))
            .cloned()
            .collect();

        send_progress(55, "下载模组文件与游戏核心...", false);
        check_cancelled()?;

        // 模组下载和「游戏核心 + 加载器」互不依赖，串行只是白等网络；两段各用自己的信号量限流。
        //
        // 进度区间必须错开：模组占 55..74，加载器阶段只在 74 上报不确定态，
        // 否则核心先装完会把进度推到 80，模组随后又报 62，进度条就倒退了
        let mods_future = self.download_modpack_files(&client_files, &targets.content_root, window);
        let core_future = self.install_game_and_loader(
            &plan.index.dependencies,
            instance_name,
            &targets.game_dir,
            window,
        );

        let (mods_result, core_result) = tokio::join!(mods_future, core_future);
        // 先报核心的错：模组缺失还能靠修复补，核心或加载器没装好是完全跑不起来的
        core_result?;
        mods_result?;

        send_progress(90, "创建实例配置...", false);
        check_cancelled()?;

        let instance_config = serde_json::json!({
            "id": instance_name,
            "name": plan.display_name,
            "type": "modpack",
            "source": plan.source,
            "modpack_id": plan.modpack_id,
            "modpack_version": plan.version_label,
            "minecraft": plan.index.dependencies.minecraft,
            "loader": plan.index.dependencies.loader_slug(),
            "icon_url": plan.icon_url,
            "loaders": plan.loaders,
            "created": chrono::Utc::now().to_rfc3339(),
        });

        let config_path = targets.instance_dir.join("instance.json");
        fs::write(config_path, serde_json::to_string_pretty(&instance_config)?)?;

        if targets.extract_dir.exists() {
            let _ = fs::remove_dir_all(&targets.extract_dir);
        }

        send_progress(100, "整合包安装完成！", false);
        info!("整合包 {} 安装完成", instance_name);

        if let Ok(game_dir_str) = config::get_game_dir() {
            let _ = window.emit("game-dir-changed", &game_dir_str);
        }

        Ok(())
    }

    /// 并发下载整合包文件，复用 download_file 的断点续传
    async fn download_modpack_files(
        &self,
        files: &[ModrinthIndexFile],
        instance_dir: &Path,
        window: &tauri::Window,
    ) -> Result<(), LauncherError> {
        let total_files = files.len();
        info!("开始下载 {} 个文件", total_files);

        if total_files == 0 {
            return Ok(());
        }

        let dl_config = config::load_config()?;
        let max_concurrent = (dl_config.download_threads as usize).clamp(
            config::MIN_DOWNLOAD_THREADS as usize,
            config::MAX_DOWNLOAD_THREADS as usize,
        );
        let semaphore = Arc::new(tokio::sync::Semaphore::new(max_concurrent));
        let completed_count = Arc::new(AtomicUsize::new(0));
        let failed_files = Arc::new(Mutex::new(Vec::<String>::new()));
        let client = download::get_http_client();

        // download_file 靠这两个标志在流式读取中途响应取消
        let running = Arc::new(AtomicBool::new(true));
        let cancelled = get_cancel_flag();
        let bytes_since_last = Arc::new(AtomicU64::new(0));

        let states: Arc<Vec<Arc<ModpackFileState>>> = Arc::new(
            files
                .iter()
                .map(|file| ModpackFileState::new(file.path.clone(), file.file_size.unwrap_or(0)))
                .collect(),
        );
        let reporting = Arc::new(AtomicBool::new(true));
        let reporter =
            spawn_file_progress_reporter(states.clone(), window.clone(), reporting.clone());

        let _ = window.emit(
            "modpack-install-progress",
            ModpackInstallProgress {
                progress: 55,
                message: format!("下载模组文件 (0/{})", total_files),
                indeterminate: false,
            },
        );

        let mut handles = Vec::with_capacity(total_files);
        let order = dispatch_order(files);

        for &index in &order {
            if is_cancelled() {
                break;
            }

            let file = &files[index];
            let state = states[index].clone();
            let completed = completed_count.clone();
            let window_clone = window.clone();
            let file_path = file.path.clone();
            let failed = failed_files.clone();
            let running = running.clone();
            let cancelled = cancelled.clone();
            let bytes_since_last = bytes_since_last.clone();
            let client = client.clone();
            let semaphore = semaphore.clone();
            // Modrinth 每个文件给多个镜像：第一个当主源、第二个当备源
            let job = DownloadJob {
                url: file.downloads.first().cloned().unwrap_or_default(),
                fallback_url: file.downloads.get(1).cloned(),
                path: instance_dir.join(&file.path),
                size: file.file_size.unwrap_or(0),
                // index.json 允许 sha1 为空串，为空时不校验，否则好文件会被判坏
                hash: (!file.hashes.sha1.is_empty()).then(|| file.hashes.sha1.clone()),
            };

            let handle = tokio::spawn(async move {
                // 许可在任务内部获取，理由同 batch.rs 的分段池：在派发循环里 await 会卡住整个循环
                let Ok(_permit) = semaphore.acquire_owned().await else {
                    state.status.store(FILE_PENDING, Ordering::Release);
                    return;
                };
                if is_cancelled() {
                    state.status.store(FILE_PENDING, Ordering::Release);
                    return;
                }
                state.status.store(FILE_DOWNLOADING, Ordering::Release);

                // 重试时 .part 已经落了半截，download_file 会带 Range 续传而不是从头再下
                let mut last_error: Option<LauncherError> = None;
                for attempt in 0..3 {
                    if is_cancelled() {
                        state.status.store(FILE_PENDING, Ordering::Release);
                        return;
                    }
                    match download::download_file(
                        &client,
                        &job,
                        &job.url,
                        &running,
                        &cancelled,
                        &state.downloaded,
                        &bytes_since_last,
                    )
                    .await
                    {
                        Ok(()) => {
                            last_error = None;
                            break;
                        }
                        Err(e) => {
                            if e.is_cancelled() {
                                state.status.store(FILE_PENDING, Ordering::Release);
                                return;
                            }
                            warn!("下载失败 {} (第 {} 次尝试): {}", file_path, attempt + 1, e);
                            last_error = Some(e);
                            if attempt < 2 {
                                tokio::time::sleep(Duration::from_secs(1 << attempt)).await;
                            }
                        }
                    }
                }

                if let Some(e) = last_error {
                    error!("无法下载文件 {}: {}", file_path, e);
                    state.status.store(FILE_FAILED, Ordering::Release);
                    failed.lock().unwrap().push(file_path);
                    return;
                }

                state.status.store(FILE_COMPLETED, Ordering::Release);
                let done = completed.fetch_add(1, Ordering::AcqRel) + 1;
                let progress = 55 + ((done as f32 / total_files as f32) * 20.0) as u8;
                let _ = window_clone.emit(
                    "modpack-install-progress",
                    ModpackInstallProgress {
                        progress,
                        message: format!("下载模组文件 ({}/{})", done, total_files),
                        indeterminate: false,
                    },
                );
            });

            handles.push(handle);
        }

        let mut task_panics = 0usize;
        for handle in handles {
            if let Err(e) = handle.await {
                task_panics += 1;
                error!("整合包下载任务异常终止: {}", e);
            }
        }

        reporting.store(false, Ordering::Release);
        let _ = reporter.await;
        let _ = window.emit(
            "modpack-file-progress",
            snapshot_files(states.as_slice(), 0.0),
        );

        // 这一轮校验过的文件指纹落盘，重复导入同一整合包时可跳过整盘哈希
        file_utils::flush_verify_cache();

        check_cancelled()?;

        if task_panics > 0 {
            return Err(LauncherError::Custom(format!(
                "{} 个下载任务异常终止，整合包可能不完整",
                task_panics
            )));
        }

        let failed = failed_files.lock().unwrap();
        if !failed.is_empty() {
            const MAX_SHOWN: usize = 5;
            let shown = failed
                .iter()
                .take(MAX_SHOWN)
                .cloned()
                .collect::<Vec<_>>()
                .join("、");
            let suffix = if failed.len() > MAX_SHOWN {
                format!(" 等 {} 个文件", failed.len())
            } else {
                String::new()
            };
            return Err(LauncherError::Custom(format!(
                "{} 个文件下载失败，整合包可能不完整：{}{}",
                failed.len(),
                shown,
                suffix
            )));
        }

        Ok(())
    }

    async fn install_game_and_loader(
        &self,
        deps: &ModrinthDependencies,
        instance_name: &str,
        game_dir: &Path,
        window: &tauri::Window,
    ) -> Result<(), LauncherError> {
        let mc_version = &deps.minecraft;
        info!("安装 Minecraft {}", mc_version);
        let cancel = get_cancel_flag();

        let config = config::load_config()?;
        let core_result = download::ensure_version_core(
            mc_version.clone(),
            config.download_mirror.clone(),
            window,
        )
        .await;
        // 下载被取消时错误文案来自下载模块，这里统一成安装取消
        check_cancelled()?;
        core_result?;

        let loader = deps.loader();
        if loader.is_some() {
            // 74 而不是 80：和并行跑的模组下载（55..74）错开，进度条才不会倒退
            let _ = window.emit(
                "modpack-install-progress",
                ModpackInstallProgress {
                    progress: 74,
                    message: "安装加载器...".to_string(),
                    indeterminate: true,
                },
            );
        }

        match loader {
            Some(loaded) => {
                info!("安装 {} {}", loaded.name(), loaded.loader_version());
                let result = loaders::install_loader(
                    &loaded,
                    instance_name,
                    game_dir,
                    Some(cancel.as_ref()),
                )
                .await;
                check_cancelled()?;
                result?;
            }
            // 无加载器依赖时手动补一份版本 JSON
            None => self.create_vanilla_version_json(mc_version, instance_name, game_dir)?,
        }

        // 加载器安装器只负责自己的文件；Fabric/Quilt 的 profile 库需要在这里补
        let instance_json = fs::read_to_string(
            game_dir
                .join("versions")
                .join(instance_name)
                .join(format!("{}.json", instance_name)),
        )
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok());
        if let Some(json) = instance_json
            && let Err(e) = download::ensure_declared_libraries(&json, window).await
        {
            if e.is_cancelled() {
                return Err(e);
            }
            warn!("加载器库文件没补全，首次启动前可能需要修复: {}", e);
        }
        // download_all_files 会重置自己的取消标志，这里再按整合包取消标志收口
        check_cancelled()?;

        Ok(())
    }

    fn create_loader_version_json(
        &self,
        instance_name: &str,
        inherits_from: &str,
        game_dir: &Path,
    ) -> Result<(), LauncherError> {
        let version_dir = game_dir.join("versions").join(instance_name);
        fs::create_dir_all(&version_dir)?;

        let json_path = version_dir.join(format!("{}.json", instance_name));

        if json_path.exists() {
            return Ok(());
        }

        let version_json = serde_json::json!({
            "id": instance_name,
            "inheritsFrom": inherits_from,
            "type": "release"
        });

        fs::write(&json_path, serde_json::to_string_pretty(&version_json)?)?;
        info!("创建版本 JSON: {}", json_path.display());

        Ok(())
    }

    fn create_vanilla_version_json(
        &self,
        mc_version: &str,
        instance_name: &str,
        game_dir: &Path,
    ) -> Result<(), LauncherError> {
        self.create_loader_version_json(instance_name, mc_version, game_dir)
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
        self.modrinth_service
            .search_modpacks(
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

    pub async fn get_modpack_versions(
        &self,
        project_id: &str,
        game_versions: Option<Vec<String>>,
        loaders: Option<Vec<String>>,
    ) -> Result<Vec<ModrinthModpackVersion>, LauncherError> {
        self.modrinth_service
            .get_modpack_versions(project_id, game_versions, loaders)
            .await
    }
}

fn progress_emitter(window: &tauri::Window) -> impl Fn(u8, &str, bool) {
    let window = window.clone();
    move |progress: u8, message: &str, indeterminate: bool| {
        let _ = window.emit(
            "modpack-install-progress",
            ModpackInstallProgress {
                progress,
                message: message.to_string(),
                indeterminate,
            },
        );
    }
}

fn build_targets(config: &crate::models::GameConfig, instance_name: &str) -> InstallTargets {
    let game_dir = PathBuf::from(&config.game_dir);
    let instance_dir = game_dir.join("versions").join(instance_name);
    // 版本信息写在实例目录，模组/配置/存档按隔离开关落到实例目录或共享根目录
    let content_root = isolation::content_root(config, &game_dir, instance_name);
    if !config.version_isolation {
        warn!(
            "版本隔离已关闭：整合包内容会写入共享目录 {}",
            content_root.display()
        );
    }

    let temp_dir = game_dir.join("temp");
    let extract_dir = temp_dir.join(format!("{}_extract", instance_name));

    InstallTargets {
        game_dir,
        instance_dir,
        content_root,
        temp_dir,
        extract_dir,
    }
}

/// 与实例创建/删除/重命名共用一把锁；失败清理会删实例目录，不能和另一实例互相踩
async fn acquire_install_lock(
    targets: &InstallTargets,
) -> Result<crate::utils::file_lock::FileLock, LauncherError> {
    crate::utils::file_lock::acquire_async(
        &crate::services::instance::lock_dir(&targets.game_dir).join("instances.lock"),
        std::time::Duration::from_secs(2),
    )
    .await
}

fn ensure_instance_free(
    targets: &InstallTargets,
    instance_name: &str,
) -> Result<(), LauncherError> {
    if targets.instance_dir.exists() {
        return Err(LauncherError::Custom(format!(
            "名为 '{}' 的实例已存在，请使用其他名称",
            instance_name
        )));
    }
    Ok(())
}

/// 失败或被取消时回滚这次安装产生的文件
fn finalize_install(
    config: &crate::models::GameConfig,
    targets: &InstallTargets,
    result: Result<(), LauncherError>,
) -> Result<(), LauncherError> {
    if result.is_err() {
        info!("安装失败或被取消，清理已创建的文件...");

        if targets.instance_dir.exists() {
            if let Err(e) = fs::remove_dir_all(&targets.instance_dir) {
                warn!("清理实例目录失败: {}", e);
            } else {
                info!("已清理实例目录: {}", targets.instance_dir.display());
            }
        }

        if !config.version_isolation && targets.content_root != targets.instance_dir {
            // 关闭隔离时内容是共享的，删掉就等于删别人的文件，只能提示手动处理
            warn!(
                "版本隔离已关闭，本次写入共享目录 {} 的文件不会被自动清理",
                targets.content_root.display()
            );
        }

        if targets.extract_dir.exists() {
            let _ = fs::remove_dir_all(&targets.extract_dir);
        }
    }

    result
}

/// overrides 与 client-overrides 都是要原样铺到实例内容目录的
fn copy_overrides(extract_dir: &Path, content_root: &Path) -> Result<(), LauncherError> {
    for name in [
        modpack_format::OVERRIDES_DIR,
        modpack_format::CLIENT_OVERRIDES_DIR,
    ] {
        let dir = extract_dir.join(name);
        if dir.exists() {
            info!("复制 {} 目录到实例", name);
            file_utils::copy_dir_all(&dir, content_root)?;
        }
    }
    Ok(())
}

fn validate_local_archive(path: &Path) -> Result<(), LauncherError> {
    if !path.is_file() {
        return Err(LauncherError::Custom(format!(
            "整合包文件不存在: {}",
            path.display()
        )));
    }

    let size = fs::metadata(path)?.len();
    if size == 0 {
        return Err(LauncherError::Custom("整合包文件是空的".to_string()));
    }
    if size > MAX_MODPACK_ARCHIVE_BYTES {
        return Err(LauncherError::Custom(format!(
            "整合包文件超过 {} 字节上限",
            MAX_MODPACK_ARCHIVE_BYTES
        )));
    }
    Ok(())
}

async fn read_archive_index(path: &Path) -> Result<ModrinthIndex, LauncherError> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || modpack_format::read_index_from_archive(&path))
        .await
        .map_err(|e| LauncherError::Custom(format!("读取整合包失败: {}", e)))?
}

fn trimmed_or(value: &str, fallback: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        fallback.to_string()
    } else {
        value.to_string()
    }
}

fn file_stem_of(path: &Path) -> String {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .filter(|stem| !stem.trim().is_empty())
        .unwrap_or_else(|| "modpack".to_string())
}

/// 把清单里的包名收敛成合法的实例目录名；实在救不回来就用固定兜底
fn sanitize_instance_name(raw: &str) -> String {
    let mut cleaned = String::with_capacity(raw.len());
    for ch in raw.chars() {
        let reserved = matches!(ch, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*');
        cleaned.push(if reserved || (ch as u32) < 32 {
            '-'
        } else {
            ch
        });
    }

    // 实例名上限 64，校验函数量的是字节数，所以先按字符裁再按字节收缩
    let mut candidate: String = cleaned.chars().take(64).collect();
    candidate = candidate
        .trim_matches(|c: char| c == ' ' || c == '.')
        .to_string();
    while candidate.len() > 64 {
        candidate.pop();
    }
    let candidate = candidate.trim_end_matches([' ', '.']).to_string();

    // 长度之外还不合法（CON、NUL 之类的保留名）就没救了，直接用兜底名
    if candidate.is_empty() || !file_utils::validate_instance_name(&candidate).is_valid {
        return "modpack".to_string();
    }

    candidate
}
#[cfg(test)]
mod tests {
    use super::{
        FILE_COMPLETED, FILE_DOWNLOADING, ModpackFileState, dispatch_order, sanitize_instance_name,
        snapshot_files,
    };
    use crate::services::modpack_format::{ModrinthIndexFile, ModrinthIndexHashes};
    use crate::utils::file_utils;
    use std::sync::Arc;
    use std::sync::atomic::Ordering;

    fn make_index_file(path: &str, size: Option<u64>) -> ModrinthIndexFile {
        ModrinthIndexFile {
            path: path.to_string(),
            hashes: ModrinthIndexHashes {
                sha1: String::new(),
                sha512: None,
            },
            downloads: Vec::new(),
            file_size: size,
            env: None,
        }
    }

    fn make_file_state(path: &str, size: u64) -> Arc<ModpackFileState> {
        ModpackFileState::new(path.to_string(), size)
    }

    #[test]
    fn test_dispatch_order_starts_with_largest_files() {
        let files = vec![
            make_index_file("small.jar", Some(10)),
            make_index_file("large.jar", Some(500)),
            make_index_file("unknown.jar", None),
            make_index_file("medium.jar", Some(100)),
        ];

        assert_eq!(dispatch_order(&files), vec![1, 3, 0, 2]);
    }

    #[test]
    fn test_file_progress_snapshot_exposes_active_only() {
        let active = make_file_state("mods/active.jar", 100);
        active.downloaded.store(40, Ordering::Relaxed);
        active.status.store(FILE_DOWNLOADING, Ordering::Release);

        let completed = make_file_state("mods/done.jar", 200);
        completed.downloaded.store(200, Ordering::Relaxed);
        completed.status.store(FILE_COMPLETED, Ordering::Release);

        let pending = make_file_state("config/pending.toml", 50);
        let states = vec![active, completed, pending];

        let snapshot = snapshot_files(&states, 1024.0);

        assert_eq!(snapshot.total_files, 3);
        assert_eq!(snapshot.completed_files, 1);
        assert_eq!(snapshot.files.len(), 1);
        assert_eq!(snapshot.files[0].path, "mods/active.jar");
        assert_eq!(snapshot.files[0].downloaded, 40);
        assert_eq!(snapshot.files[0].total, 100);
        assert_eq!(snapshot.downloaded_bytes, 240);
        assert_eq!(snapshot.total_bytes, 350);
        assert_eq!(snapshot.speed, 1024.0);
    }

    #[test]
    fn test_sanitize_instance_name_keeps_readable_names() {
        assert_eq!(
            sanitize_instance_name("Fabulously Optimized"),
            "Fabulously Optimized"
        );
        assert_eq!(sanitize_instance_name("我的整合包"), "我的整合包");
        assert_eq!(sanitize_instance_name("Pack: 1/2?"), "Pack- 1-2-");
    }

    #[test]
    fn test_sanitize_instance_name_falls_back_for_unusable_names() {
        assert_eq!(sanitize_instance_name(""), "modpack");
        assert_eq!(sanitize_instance_name(".."), "modpack");
        assert_eq!(sanitize_instance_name("CON"), "modpack");
        assert_eq!(sanitize_instance_name("   "), "modpack");
    }

    #[test]
    fn test_sanitize_instance_name_truncates_to_the_byte_limit() {
        // 校验按字节算上限，64 个汉字有 192 字节，必须先截短
        let long = sanitize_instance_name(&"名".repeat(100));
        assert!(long.len() <= 64);
        assert!(file_utils::validate_instance_name(&long).is_valid);

        let ascii = sanitize_instance_name(&"a".repeat(100));
        assert_eq!(ascii.chars().count(), 64);
    }
}
