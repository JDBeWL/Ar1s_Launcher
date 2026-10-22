//! 批量下载调度
//!
//! 按配置线程数限并发，跳过 .ar1s/download_state 里已完成项、按校验指纹跳过未变的文件，
//! 退避重试并响应取消

use super::file::download_file;
use super::http::get_http_client;
use super::state::DownloadState;
use crate::errors::LauncherError;
use crate::models::{DownloadJob, DownloadProgress, DownloadStatus};
use crate::services::config::load_config;
use log::{debug, info, warn};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;
use tauri::async_runtime;
use tauri::{Emitter, Listener, Window};
use tokio::sync::Mutex;

/// 取消标志跨下载会话保留，不会随单次下载结束销毁
static CANCEL_FLAG: std::sync::OnceLock<Arc<AtomicBool>> = std::sync::OnceLock::new();

fn get_cancel_flag() -> Arc<AtomicBool> {
    CANCEL_FLAG
        .get_or_init(|| Arc::new(AtomicBool::new(false)))
        .clone()
}

/// 只在新下载开始时重置；中途清标志会让取消失效
pub fn reset_cancel_flag() {
    if let Some(flag) = CANCEL_FLAG.get() {
        flag.store(false, Ordering::Release);
    }
}

pub fn set_cancel_flag() {
    get_cancel_flag().store(true, Ordering::Release);
}

pub async fn download_all_files(
    jobs: Vec<DownloadJob>,
    window: &Window,
) -> Result<(), LauncherError> {
    let config = load_config()?;
    // 旧配置或手改配置可能是 0 或超过上限；Semaphore::new(0) 会让所有任务永久等许可
    let threads = (config.download_threads as usize).clamp(
        crate::services::config::MIN_DOWNLOAD_THREADS as usize,
        crate::services::config::MAX_DOWNLOAD_THREADS as usize,
    );

    let http = get_http_client();

    let version_id = jobs
        .first()
        .and_then(|j| j.path.parent())
        .and_then(|p| p.file_name())
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "unknown".to_string());

    let game_dir = std::path::PathBuf::from(&config.game_dir);
    let lock_dir = crate::services::instance::lock_dir(&game_dir);
    let state_dir = lock_dir.join("download_state");
    std::fs::create_dir_all(&state_dir)?;
    let state_file = state_dir.join(format!("{}.json", version_id));

    // 共享 assets/libraries 的目标路径是全局的，按版本加锁挡不住两个版本写同一个 .part；
    // 同一游戏目录同一时间只允许一个下载任务
    let _download_lock = crate::utils::file_lock::acquire_async(
        &lock_dir.join("download.lock"),
        std::time::Duration::from_secs(2),
    )
    .await?;

    let download_state = Arc::new(Mutex::new(
        DownloadState::load_from_file(&state_file).unwrap_or_default(),
    ));

    let mut filtered_jobs: Vec<DownloadJob> = {
        let mut state = download_state.lock().await;
        jobs.iter()
            .filter(|job| {
                if state.is_completed(&job.url) {
                    if job.path.exists() {
                        false
                    } else {
                        warn!(
                            "File marked as completed but missing on disk: {}, re-downloading",
                            job.path.display()
                        );
                        state.completed_files.remove(&job.url);
                        state.mark_dirty();
                        true
                    }
                } else {
                    true
                }
            })
            .cloned()
            .collect()
    };
    order_jobs_for_dispatch(&mut filtered_jobs);

    // 必须在过滤之后统计，才能反映真实状态
    let (completed_count, resumed_bytes) = {
        let state = download_state.lock().await;
        let completed = state.completed_files.len() as u64;
        let completed_bytes: u64 = jobs
            .iter()
            .filter(|j| state.is_completed(&j.url))
            .map(|j| j.size)
            .sum();
        let partial_bytes: u64 = state.partial_downloads.values().sum();
        (completed, completed_bytes + partial_bytes)
    };

    let total_size: u64 = jobs.iter().map(|j| j.size).sum();

    if filtered_jobs.is_empty() {
        debug!("All files already downloaded, skipping");
        emit_completed_progress(window, total_size, total_size);
        return Ok(());
    }

    reset_cancel_flag();
    let global_cancel = get_cancel_flag();

    let state = Arc::new(AtomicBool::new(true));
    let was_cancelled = Arc::new(AtomicBool::new(false));
    let error_occurred = Arc::new(tokio::sync::Mutex::new(None::<String>));

    // 监听必须在预校验之前就位：预校验要读几百个文件，这段时间也得能取消
    let state_clone = state.clone();
    let was_cancelled_clone = was_cancelled.clone();
    let download_state_clone = download_state.clone();
    let state_file_clone = state_file.clone();
    let listener_id = window.listen("cancel-download", move |_| {
        if state_clone.swap(false, Ordering::AcqRel) {
            was_cancelled_clone.store(true, Ordering::Release);
            // 取消时立即保存状态，供下次续传
            let download_state = download_state_clone.clone();
            let state_file = state_file_clone.clone();
            std::thread::spawn(move || {
                if let Ok(mut state) = download_state.try_lock() {
                    let _ = state.save_to_file(&state_file);
                }
            });
        }
    });

    // 已在盘上的文件先单独核一遍，不占下载槽位
    let verified = preverify_existing(&filtered_jobs, &state, &global_cancel).await;

    if !state.load(Ordering::Acquire) || global_cancel.load(Ordering::Acquire) {
        window.unlisten(listener_id);
        crate::utils::file_utils::flush_verify_cache();
        emit_cancelled_progress(window, 0, total_size);
        return Err(LauncherError::Cancelled);
    }

    let verified_bytes: u64 = filtered_jobs
        .iter()
        .filter(|job| verified.contains(&job.url))
        .map(|job| job.size)
        .sum();
    let verified_count = filtered_jobs
        .iter()
        .filter(|job| verified.contains(&job.url))
        .count() as u64;

    filtered_jobs.retain(|job| !verified.contains(&job.url));
    let completed_count = completed_count + verified_count;
    let resumed_bytes = resumed_bytes + verified_bytes;

    // 预校验这一轮算出来的指纹必须落盘，下次才能跳过读盘
    crate::utils::file_utils::flush_verify_cache();

    if filtered_jobs.is_empty() {
        // 全都已经在盘上且校验通过，没有要下的
        window.unlisten(listener_id);
        emit_completed_progress(window, total_size, total_size);
        return Ok(());
    }

    info!(
        "Resuming download - {} files completed, {} remaining, {} bytes resumed",
        completed_count,
        filtered_jobs.len(),
        resumed_bytes
    );

    let files_downloaded = Arc::new(AtomicU64::new(completed_count));
    let bytes_downloaded = Arc::new(AtomicU64::new(resumed_bytes));
    let bytes_since_last = Arc::new(AtomicU64::new(0));

    let reporter_handle = spawn_progress_reporter(
        bytes_downloaded.clone(),
        bytes_since_last.clone(),
        state.clone(),
        window.clone(),
        total_size,
    );

    let state_saver_handle =
        spawn_state_saver(download_state.clone(), state_file.clone(), state.clone());

    // 大文件单独限流：几个几百 MB 的 client.jar / 整合包会长期占住槽位，
    // 后面成百上千个小资源文件只能干等。分成两个池子，小文件永远有槽可用
    let large_slots = (threads / 4).max(1).min(threads);
    let small_slots = threads.saturating_sub(large_slots).max(1);
    let large_semaphore = Arc::new(tokio::sync::Semaphore::new(large_slots));
    let small_semaphore = Arc::new(tokio::sync::Semaphore::new(small_slots));
    let mut handles = vec![];

    for job in filtered_jobs {
        if !state.load(Ordering::Acquire) || global_cancel.load(Ordering::Acquire) {
            break;
        }

        // 许可在任务内部获取：在这里 await 的话，大文件池满了会卡住整个派发循环，
        // 后面的小文件连排队的机会都没有
        let semaphore = if is_large_file(&job) {
            large_semaphore.clone()
        } else {
            small_semaphore.clone()
        };

        let global_cancel_clone = global_cancel.clone();
        let handle = spawn_download_task(
            job,
            http.clone(),
            state.clone(),
            global_cancel_clone,
            files_downloaded.clone(),
            bytes_downloaded.clone(),
            bytes_since_last.clone(),
            error_occurred.clone(),
            download_state.clone(),
            semaphore,
        );
        handles.push(handle);
    }

    let mut task_panics = 0usize;
    for handle in handles {
        if let Err(e) = handle.await {
            task_panics += 1;
            warn!("下载任务异常终止: {}", e);
        }
    }

    state.store(false, Ordering::Release);
    let reporter_result = reporter_handle.await;
    let saver_result = state_saver_handle.await;

    // 取消监听必须无条件执行，否则每失败一次就多留一个 cancel-download 监听
    window.unlisten(listener_id);

    {
        let mut state = download_state.lock().await;
        if state.dirty
            && let Err(e) = state.save_to_file(&state_file)
        {
            warn!("Failed to write final state file: {}", e);
        }
    }

    reporter_result?;
    saver_result?;

    // 这一轮校验过的文件指纹落盘，下次安装同一版本可跳过整盘哈希
    crate::utils::file_utils::flush_verify_cache();

    if was_cancelled.load(Ordering::Acquire) {
        emit_cancelled_progress(window, bytes_downloaded.load(Ordering::Relaxed), total_size);
        return Err(LauncherError::Cancelled);
    }

    if task_panics > 0 {
        return Err(LauncherError::Custom(format!(
            "{} 个下载任务异常终止",
            task_panics
        )));
    }

    let error_message = {
        let error_guard = error_occurred.lock().await;
        error_guard.clone()
    };

    if let Some(error_msg) = error_message {
        emit_error_progress(
            window,
            bytes_downloaded.load(Ordering::Relaxed),
            total_size,
            &error_msg,
        );
        return Err(LauncherError::Custom(error_msg));
    }

    let failed_list: Vec<String> = {
        let state = download_state.lock().await;
        state.failed_files.iter().cloned().collect()
    };
    if !failed_list.is_empty() {
        let _ = window.emit(
            "download-summary",
            &serde_json::json!({
                "status": "partial",
                "failed_count": failed_list.len(),
                "failed": failed_list,
            }),
        );
    }

    if failed_list.is_empty() {
        let _ = std::fs::remove_file(&state_file);
        if let Ok(entries) = std::fs::read_dir(&state_dir)
            && entries.count() == 0
        {
            let _ = std::fs::remove_dir(&state_dir);
        }
    }

    emit_completed_progress(window, bytes_downloaded.load(Ordering::Relaxed), total_size);

    Ok(())
}

/// 大文件先派发，尾部由大量小文件收尾，避免最后几个大文件把并发槽拖住
fn order_jobs_for_dispatch(jobs: &mut [DownloadJob]) {
    jobs.sort_by_key(|job| std::cmp::Reverse(job.size));
}

/// 预校验：把已在盘上的文件先核一遍，通过的直接从待下载列表里拿掉
///
/// 以前这一步混在 `download_file` 里，等于占着一个下载槽位做纯读盘。几百个资源文件
/// 里如果有大半已经存在，槽位全被哈希算占满，真正的网络下载反而起不来。
///
/// 现在读盘和下载彻底分开：这里用阻塞线程池并行核，核完再开始下载。
/// 校验结果按「长度 + 修改时间」进指纹缓存，第二次基本不读盘。
///
/// 核不过的文件不会被删，仍留在列表里交给 `download_file` 重新校验并重下；
/// 那种情况会多算一遍哈希，但损坏文件本来就是少数，不值得为它把接口搞复杂。
async fn preverify_existing(
    jobs: &[DownloadJob],
    state: &Arc<AtomicBool>,
    global_cancel: &Arc<AtomicBool>,
) -> std::collections::HashSet<String> {
    let candidates: Vec<DownloadJob> = jobs
        .iter()
        .filter(|job| job.path.exists())
        .cloned()
        .collect();

    if candidates.is_empty() {
        return std::collections::HashSet::new();
    }

    // 只用来读盘算哈希，与下载槽位无关；并行度过高反而互相抢 IO
    let workers = std::thread::available_parallelism()
        .map(|count| count.get())
        .unwrap_or(4)
        .clamp(1, 4);
    let verified = Arc::new(tokio::sync::Mutex::new(std::collections::HashSet::new()));

    for chunk in candidates.chunks(candidates.len().div_ceil(workers).max(1)) {
        if !state.load(Ordering::Acquire) || global_cancel.load(Ordering::Acquire) {
            break;
        }

        let chunk = chunk.to_vec();
        let verified = verified.clone();

        let handle = tokio::task::spawn_blocking(move || {
            chunk
                .into_iter()
                .filter(|job| {
                    crate::utils::file_utils::verify_file(&job.path, job.hash.as_deref(), job.size)
                        .unwrap_or(false)
                })
                .map(|job| job.url)
                .collect::<Vec<_>>()
        });

        // 各分片独立跑完，不能因为一个 join 失败就丢掉其他分片的结果
        if let Ok(urls) = handle.await {
            verified.lock().await.extend(urls);
        }
    }

    let verified = verified.lock().await.clone();
    debug!("预校验通过 {} 个已在盘上的文件", verified.len());
    verified
}

/// 超过阈值的算大文件，单独用一个较小的池子限流
const LARGE_FILE_BYTES: u64 = 8 * 1024 * 1024;

fn is_large_file(job: &DownloadJob) -> bool {
    job.size >= LARGE_FILE_BYTES
}

fn spawn_progress_reporter(
    bytes_downloaded: Arc<AtomicU64>,
    bytes_since_last: Arc<AtomicU64>,
    state: Arc<AtomicBool>,
    window: Window,
    total_size: u64,
) -> tauri::async_runtime::JoinHandle<()> {
    let report_interval = Duration::from_millis(200);

    async_runtime::spawn(async move {
        while state.load(Ordering::Acquire) {
            tokio::time::sleep(report_interval).await;
            if !state.load(Ordering::Acquire) {
                break;
            }

            let current_bytes = bytes_downloaded.load(Ordering::Relaxed);
            let bytes_since = bytes_since_last.swap(0, Ordering::Relaxed);

            if bytes_since == 0 {
                continue;
            }

            let speed = (bytes_since as f64 / 1024.0) / report_interval.as_secs_f64();
            let progress_percent = if total_size > 0 {
                (current_bytes as f64 / total_size as f64 * 100.0).round() as u8
            } else {
                0
            };

            let progress = DownloadProgress {
                bytes_downloaded: current_bytes,
                total_bytes: total_size,
                speed,
                status: DownloadStatus::Downloading,
                percent: progress_percent,
                error: None,
            };
            let _ = window.emit("download-progress", &progress);
        }
    })
}

fn spawn_state_saver(
    download_state: Arc<Mutex<DownloadState>>,
    state_file: std::path::PathBuf,
    running: Arc<AtomicBool>,
) -> tauri::async_runtime::JoinHandle<()> {
    let save_interval = Duration::from_secs(30);

    async_runtime::spawn(async move {
        while running.load(Ordering::Acquire) {
            tokio::time::sleep(save_interval).await;
            if !running.load(Ordering::Acquire) {
                break;
            }

            let mut state = download_state.lock().await;
            if state.dirty {
                if let Err(e) = state.save_to_file(&state_file) {
                    warn!("Failed to save download state: {}", e);
                } else {
                    debug!("Download state saved to {}", state_file.display());
                }
            }
        }
    })
}

#[allow(clippy::too_many_arguments)]
fn spawn_download_task(
    job: DownloadJob,
    http: reqwest::Client,
    state: Arc<AtomicBool>,
    global_cancel: Arc<AtomicBool>,
    files_downloaded: Arc<AtomicU64>,
    bytes_downloaded: Arc<AtomicU64>,
    bytes_since_last: Arc<AtomicU64>,
    error_occurred: Arc<tokio::sync::Mutex<Option<String>>>,
    download_state: Arc<Mutex<DownloadState>>,
    semaphore: Arc<tokio::sync::Semaphore>,
) -> tauri::async_runtime::JoinHandle<Result<(), LauncherError>> {
    async_runtime::spawn(async move {
        if !state.load(Ordering::Acquire) || global_cancel.load(Ordering::Acquire) {
            return Ok::<(), LauncherError>(());
        }

        {
            let mut state = download_state.lock().await;
            state.start_download(job.url.clone(), job.path.clone());
        }

        let mut current_job_error: Option<LauncherError> = None;
        let mut job_succeeded = false;

        const MAX_JOB_RETRIES: usize = 5;
        for retry in 0..MAX_JOB_RETRIES {
            if !state.load(Ordering::Acquire) || global_cancel.load(Ordering::Acquire) {
                break;
            }

            // 许可逐次重试各自申请：以前在循环外拿一次，一个持续失败的文件会
            // 连着槽位睡满 1+2+4+8=15 秒退避，小文件池很容易被几个坏文件整个占死
            let permit = match semaphore.clone().acquire_owned().await {
                Ok(permit) => permit,
                Err(_) => break,
            };
            if !state.load(Ordering::Acquire) || global_cancel.load(Ordering::Acquire) {
                drop(permit);
                break;
            }

            // 重试 2 次后改用官方源 fallback_url
            let current_url = if retry >= 2 && job.url.contains("bmclapi2.bangbang93.com") {
                job.fallback_url.as_deref().unwrap_or(&job.url)
            } else {
                &job.url
            };

            let attempt_str = if retry == 0 {
                "attempt 1".to_string()
            } else {
                format!("retry {}/{}", retry, MAX_JOB_RETRIES - 1)
            };
            debug!("Downloading file: {} ({})", current_url, attempt_str);

            let result = download_file(
                &http,
                &job,
                current_url,
                &state,
                &global_cancel,
                &bytes_downloaded,
                &bytes_since_last,
            )
            .await;
            // 先还槽位再决定要不要退避，退避期间不再占着并发名额
            drop(permit);

            match result {
                Ok(_) => {
                    files_downloaded.fetch_add(1, Ordering::Relaxed);
                    current_job_error = None;
                    job_succeeded = true;
                    break;
                }
                Err(e) => {
                    if e.is_cancelled() {
                        break;
                    }
                    warn!("Download failed: {} ({}) - {}", current_url, attempt_str, e);
                    current_job_error = Some(e);
                    if retry < MAX_JOB_RETRIES - 1 {
                        let backoff = Duration::from_secs(1 << retry);
                        debug!("Waiting {:?} before next attempt", backoff);
                        tokio::time::sleep(backoff).await;
                    }
                }
            }
        }

        {
            let mut state = download_state.lock().await;
            if job_succeeded {
                state.mark_completed(job.url.clone());
            } else {
                state.mark_failed(job.url.clone());
                if let Some(e) = current_job_error {
                    let mut error_guard = error_occurred.lock().await;
                    if error_guard.is_none() {
                        *error_guard = Some(e.to_string());
                    }
                }
            }
            state.finish_download(&job.url);
        }

        Ok::<(), LauncherError>(())
    })
}

fn emit_cancelled_progress(window: &Window, bytes: u64, total: u64) {
    let percent = if total > 0 {
        (bytes as f64 / total as f64 * 100.0).round() as u8
    } else {
        0
    };

    let _ = window.emit(
        "download-progress",
        &DownloadProgress {
            bytes_downloaded: bytes,
            total_bytes: total,
            speed: 0.0,
            status: DownloadStatus::Cancelled,
            percent,
            error: None,
        },
    );
}

fn emit_error_progress(window: &Window, bytes: u64, total: u64, error_msg: &str) {
    let percent = if total > 0 {
        (bytes as f64 / total as f64 * 100.0).round() as u8
    } else {
        0
    };

    let _ = window.emit(
        "download-progress",
        &DownloadProgress {
            bytes_downloaded: bytes,
            total_bytes: total,
            speed: 0.0,
            status: DownloadStatus::Error,
            percent,
            error: Some(error_msg.to_string()),
        },
    );
}

fn emit_completed_progress(window: &Window, bytes: u64, total: u64) {
    let _ = window.emit(
        "download-progress",
        &DownloadProgress {
            bytes_downloaded: bytes,
            total_bytes: total,
            speed: 0.0,
            status: DownloadStatus::Completed,
            percent: 100,
            error: None,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::order_jobs_for_dispatch;
    use crate::models::DownloadJob;
    use std::path::PathBuf;

    fn job(size: u64) -> DownloadJob {
        DownloadJob {
            url: format!("https://example.com/{size}"),
            fallback_url: None,
            path: PathBuf::from(format!("{size}.jar")),
            size,
            hash: None,
        }
    }

    #[test]
    fn test_order_jobs_dispatches_largest_first() {
        let mut jobs = vec![job(10), job(500), job(0), job(100)];
        order_jobs_for_dispatch(&mut jobs);
        let sizes: Vec<u64> = jobs.iter().map(|job| job.size).collect();
        assert_eq!(sizes, vec![500, 100, 10, 0]);
    }
}
