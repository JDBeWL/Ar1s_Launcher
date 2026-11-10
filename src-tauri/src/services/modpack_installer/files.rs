//! 按清单并发补文件（不叫 download，避免和 `crate::services::download` 撞名）
//!
//! 复用 `download::download_file` 的断点续传；大文件先发，小文件后发，避免最后卡在几十个
//! 几 KB 的配置上

use super::progress::spawn_file_progress_reporter;
use super::progress::{
    FILE_COMPLETED, FILE_DOWNLOADING, FILE_FAILED, FILE_PENDING, ModpackFileState, dispatch_order,
    snapshot_files,
};
use super::{
    ModpackInstallProgress, ModpackInstaller, check_cancelled, get_cancel_flag, is_cancelled,
};
use crate::errors::LauncherError;
use crate::models::DownloadJob;
use crate::services::modpack_format::ModrinthIndexFile;
use crate::services::{config, download};
use crate::utils::file_utils::{self};
use log::{error, info, warn};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::Emitter;

/// 并发下载整合包文件，复用 download_file 的断点续传
impl ModpackInstaller {
    pub(super) async fn download_modpack_files(
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
                fallback_urls: file.downloads.get(1).cloned().into_iter().collect(),
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
                    // 某个任务持锁 panic 时不级联引爆其余下载任务
                    failed
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .push(file_path);
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

        let failed = failed_files
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
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
}
