//! 进度采样与上报
//!
//! 下载线程只更新原子量，真正的快照与 emit 走 250ms 节流，避免上千个文件把事件队列打满

use crate::services::modpack_format::ModrinthIndexFile;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::time::Duration;
use tauri::Emitter;

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

pub(super) const FILE_PENDING: u8 = 0;
pub(super) const FILE_DOWNLOADING: u8 = 1;
pub(super) const FILE_COMPLETED: u8 = 2;
pub(super) const FILE_FAILED: u8 = 3;

pub(super) struct ModpackFileState {
    pub(super) path: String,
    pub(super) size: u64,
    pub(super) downloaded: Arc<AtomicU64>,
    pub(super) status: AtomicU8,
}

impl ModpackFileState {
    pub(super) fn new(path: String, size: u64) -> Arc<Self> {
        Arc::new(Self {
            path,
            size,
            downloaded: Arc::new(AtomicU64::new(0)),
            status: AtomicU8::new(FILE_PENDING),
        })
    }
}

pub(super) fn snapshot_files(states: &[Arc<ModpackFileState>], speed: f64) -> ModpackFilesProgress {
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

pub(super) fn dispatch_order(files: &[ModrinthIndexFile]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..files.len()).collect();
    // 大文件先占并发槽，避免最后只剩几个大文件把安装拖住
    order.sort_by_key(|&index| std::cmp::Reverse(files[index].file_size.unwrap_or(0)));
    order
}

const FILE_PROGRESS_INTERVAL: Duration = Duration::from_millis(250);

pub(super) fn spawn_file_progress_reporter(
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

pub(super) fn progress_emitter(window: &tauri::Window) -> impl Fn(u8, &str, bool) {
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
