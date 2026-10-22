//! 单个文件下载
//!
//! 写 .part 临时文件按 Range 续传，边写边算 sha1（写完不再整盘重读），
//! 校验失败时切换到 fallback_url。
//!
//! 竞速（主备源同时发，先回的胜出）与分段（大文件切几段各用一条连接）都只在体积值得时才启用

use crate::errors::LauncherError;
use crate::models::DownloadJob;
use crate::utils::file_utils::{self, FileHasher};
use crate::utils::verify_cache;
use log::{debug, warn};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tokio::io::{AsyncSeekExt, AsyncWriteExt, BufWriter};

/// 写盘缓冲
///
/// `tokio::fs::File` 每次写都是一次阻塞池派发，而 reqwest 的 chunk 只有 8–16 KB：
/// 100 MB 文件就是上万次往返，攒到 256 KB 再落盘
const WRITE_BUFFER_BYTES: usize = 256 * 1024;

/// 两个源各发一次请求做竞速的起跳体积
/// 再小就不值：几千个资源文件每个多一次请求，换来的延迟收益抵不过翻倍的请求量
const RACE_MIN_BYTES: u64 = 2 * 1024 * 1024;

/// 分段下载的起跳体积
const MULTIPART_MIN_BYTES: u64 = 8 * 1024 * 1024;

/// 单个文件最多切几段
const MULTIPART_MAX_SEGMENTS: usize = 4;

/// 全进程额外分段连接的总预算
///
/// `download_threads` 限的是同时在下的文件数，不是连接数：一条大文件切 4 段就是 4 条连接，
/// 不设总量上限 16 线程能开出 64 条，容易被服务端限速甚至拒绝。借到几段用几段，借不到退回单连接
const MULTIPART_BUDGET: usize = 24;

static MULTIPART_PERMITS: std::sync::LazyLock<Arc<tokio::sync::Semaphore>> =
    std::sync::LazyLock::new(|| Arc::new(tokio::sync::Semaphore::new(MULTIPART_BUDGET)));

pub async fn download_file(
    http: &reqwest::Client,
    job: &DownloadJob,
    url: &str,
    state: &Arc<AtomicBool>,
    global_cancel: &Arc<AtomicBool>,
    bytes_downloaded: &Arc<AtomicU64>,
    bytes_since_last: &Arc<AtomicU64>,
) -> Result<(), LauncherError> {
    if !state.load(Ordering::Acquire) || global_cancel.load(Ordering::Acquire) {
        return Err(LauncherError::Cancelled);
    }

    if job.path.exists() {
        match verify_existing(job).await {
            Ok(true) => {
                debug!(
                    "File already exists and is valid, skipping: {}",
                    job.path.display()
                );
                bytes_downloaded.fetch_add(job.size, Ordering::Relaxed);
                return Ok(());
            }
            Ok(false) => {
                debug!(
                    "File exists but is invalid, attempting to download: {}",
                    job.path.display()
                );
            }
            Err(e) => {
                warn!(
                    "File verification failed, attempting to download: {} - {}",
                    job.path.display(),
                    e
                );
            }
        }
    }

    match download_with_resume(
        http,
        url,
        job,
        state,
        global_cancel,
        bytes_downloaded,
        bytes_since_last,
    )
    .await
    {
        Ok(()) => Ok(()),
        Err(e) => {
            if e.is_cancelled() {
                return Err(e);
            }
            if let Some(fallback_url) = &job.fallback_url
                && should_try_fallback(&e)
            {
                warn!(
                    "Primary URL {} failed ({}), trying fallback: {}",
                    job.url, e, fallback_url
                );
                return download_with_resume(
                    http,
                    fallback_url,
                    job,
                    state,
                    global_cancel,
                    bytes_downloaded,
                    bytes_since_last,
                )
                .await;
            }
            Err(e)
        }
    }
}

/// 校验会有整文件读盘，扔到阻塞线程池，别占着 async worker
async fn verify_existing(job: &DownloadJob) -> Result<bool, LauncherError> {
    let job = job.clone();
    tokio::task::spawn_blocking(move || {
        file_utils::verify_file(&job.path, job.hash.as_deref(), job.size)
    })
    .await
    .map_err(|e| LauncherError::Custom(format!("校验文件失败: {}", e)))?
}

fn should_try_fallback(e: &LauncherError) -> bool {
    let is_http_error = if let LauncherError::Http(err) = e {
        err.status() == Some(reqwest::StatusCode::NOT_FOUND) || err.is_timeout()
    } else {
        false
    };

    let err_str = e.to_string();
    is_http_error
        || err_str.contains("size or hash mismatch")
        || err_str.contains("File size mismatch")
        || err_str.contains("Unexpected Content-Length")
        || err_str.contains("Unexpected Content-Type")
}

async fn download_with_resume(
    client: &reqwest::Client,
    url: &str,
    job: &DownloadJob,
    state: &Arc<AtomicBool>,
    global_cancel: &Arc<AtomicBool>,
    bytes_downloaded: &Arc<AtomicU64>,
    bytes_since_last: &Arc<AtomicU64>,
) -> Result<(), LauncherError> {
    // 小于阈值的文件直接写目标路径：省掉一次 .part + rename 的元数据往返，
    // 也少触发一遍杀毒扫描；这类文件重下代价本来就低，不需要续传
    let direct = is_small_file(job);

    let tmp_path = if direct {
        job.path.clone()
    } else {
        let part = part_path(&job.path);
        adopt_legacy_part(&job.path, &part);
        part
    };

    let existing_size = if direct {
        // 直写模式下目标文件就是写一半的产物，不能当续传依据
        let _ = tokio::fs::remove_file(&tmp_path).await;
        forget_prefix(&tmp_path);
        0
    } else {
        tokio::fs::metadata(&tmp_path)
            .await
            .map(|m| m.len())
            .unwrap_or(0)
    };

    if existing_size > 0 && job.size > 0 && existing_size >= job.size {
        debug!(
            "Part file complete ({}), verifying: {}",
            existing_size,
            tmp_path.display()
        );
        let tmp_for_verify = tmp_path.clone();
        let hash = job.hash.clone();
        let size = job.size;
        let valid = tokio::task::spawn_blocking(move || {
            file_utils::verify_file(&tmp_for_verify, hash.as_deref(), size)
        })
        .await
        .map_err(|e| LauncherError::Custom(format!("校验临时文件失败: {}", e)))??;

        if valid {
            finalize_download(&tmp_path, &job.path).await?;
            remember_fingerprint(&job.path, job.hash.as_deref(), job.hash.as_deref());
            bytes_downloaded.fetch_add(job.size, Ordering::Relaxed);
            return Ok(());
        }
        warn!("Part file corrupted, restarting download");
        let _ = tokio::fs::remove_file(&tmp_path).await;
        forget_prefix(&tmp_path);
    }

    // 不做 HEAD 预检，直接发 Range；返回 200（不支持 Range）由 download_chunk_with_resume 兜底
    let resume_from = if !direct && existing_size > 0 && job.size > 0 && existing_size < job.size {
        debug!("Resuming download from byte {}: {}", existing_size, url);
        Some(existing_size)
    } else {
        None
    };

    dispatch_download(
        client,
        url,
        job,
        state,
        global_cancel,
        bytes_downloaded,
        bytes_since_last,
        resume_from,
    )
    .await
}

/// 按体积与续传状态挑下载策略
///
/// 续传必须单连接（分段要求整段重来）；全新的大文件优先分段，被服务端拒绝时退回单连接
#[allow(clippy::too_many_arguments)]
async fn dispatch_download(
    client: &reqwest::Client,
    url: &str,
    job: &DownloadJob,
    state: &Arc<AtomicBool>,
    global_cancel: &Arc<AtomicBool>,
    bytes_downloaded: &Arc<AtomicU64>,
    bytes_since_last: &Arc<AtomicU64>,
    resume_from: Option<u64>,
) -> Result<(), LauncherError> {
    if resume_from.is_none()
        && job.size >= MULTIPART_MIN_BYTES
        && let Some((segments, _permit)) = acquire_multipart_permits(job.size)
    {
        match download_multipart(
            client,
            url,
            job,
            state,
            global_cancel,
            bytes_downloaded,
            bytes_since_last,
            segments,
        )
        .await
        {
            Ok(true) => return Ok(()),
            Ok(false) => debug!("服务端不支持分段下载，退回单连接: {}", url),
            Err(e) => return Err(e),
        }
    }

    download_chunk_with_resume(
        client,
        url,
        job,
        state,
        global_cancel,
        bytes_downloaded,
        bytes_since_last,
        resume_from,
    )
    .await
}

/// 已建立连接的下载流
struct OpenedStream {
    response: reqwest::Response,
    /// 服务端实际接受的续传起点，None 表示从头
    resume: Option<u64>,
    url: String,
}

/// 主源与备源竞速拿到第一条可用连接
///
/// 镜像抽风时单连接要等满读超时（60s）才切源，这期间整个文件毫无进度；两源同时发，先回者胜出。
/// 只在主源上启用：调用方主动传进来的 fallback 说明主源已经失败过，再试它只是白费一次请求
async fn open_stream(
    client: &reqwest::Client,
    job: &DownloadJob,
    url: &str,
    resume_from: Option<u64>,
) -> Result<OpenedStream, LauncherError> {
    let fallback = job
        .fallback_url
        .as_deref()
        .filter(|fallback| *fallback != url && !fallback.is_empty());

    match fallback {
        Some(fallback) if url == job.url && job.size >= RACE_MIN_BYTES => {
            race_streams(client, job, url, fallback, resume_from).await
        }
        _ => send_stream_request(client.clone(), url.to_string(), job.size, resume_from).await,
    }
}

async fn race_streams(
    client: &reqwest::Client,
    job: &DownloadJob,
    primary: &str,
    fallback: &str,
    resume_from: Option<u64>,
) -> Result<OpenedStream, LauncherError> {
    let mut tasks = tokio::task::JoinSet::new();
    for candidate in [primary.to_string(), fallback.to_string()] {
        tasks.spawn(send_stream_request(
            client.clone(),
            candidate,
            job.size,
            resume_from,
        ));
    }

    let mut last_error: Option<LauncherError> = None;
    while let Some(joined) = tasks.join_next().await {
        match joined {
            Ok(Ok(stream)) => {
                // 剩下的那个任务随 JoinSet 一起 abort，不会继续占着连接
                debug!("竞速胜出的源: {}", stream.url);
                return Ok(stream);
            }
            Ok(Err(e)) => {
                if e.is_cancelled() {
                    return Err(e);
                }
                warn!("一个源建立连接失败: {}", e);
                last_error = Some(e);
            }
            Err(e) => {
                last_error = Some(LauncherError::Custom(format!("下载任务异常终止: {}", e)));
            }
        }
    }

    Err(last_error.unwrap_or_else(|| LauncherError::Custom("所有源都无法建立连接".to_string())))
}

/// 发一次请求并按响应头判定实际续传起点
async fn send_stream_request(
    client: reqwest::Client,
    url: String,
    expected_size: u64,
    resume_from: Option<u64>,
) -> Result<OpenedStream, LauncherError> {
    let mut request = client.get(&url);
    if let Some(offset) = resume_from {
        request = request.header("Range", format!("bytes={}-", offset));
    }

    let response = request.send().await?;
    let status = response.status();

    if !status.is_success() && status != reqwest::StatusCode::PARTIAL_CONTENT {
        return Err(LauncherError::Custom(format!(
            "HTTP error {} for {}",
            status, url
        )));
    }

    // 服务端不认 Range 会返回 200 和整个文件：续传点作废，从头写
    let resume = match resume_from {
        Some(_) if status == reqwest::StatusCode::OK => {
            debug!("Server returned 200 instead of 206, restarting download");
            None
        }
        other => other,
    };

    if resume.is_none() {
        validate_content_type(&response, &url)?;
    }
    validate_content_length(&response, &url, expected_size, resume)?;

    Ok(OpenedStream {
        response,
        resume,
        url,
    })
}

/// 小文件走直写：不值得为断点续传付两次元数据操作与一次重命名
const DIRECT_WRITE_LIMIT: u64 = 1024 * 1024;

fn is_small_file(job: &DownloadJob) -> bool {
    job.size > 0 && job.size <= DIRECT_WRITE_LIMIT
}

/// 保持 `<文件名>.part` 的形式：`with_extension` 会把 `a.jar` 变成 `a.part`，
/// 同目录下两个 jar 会撞名
fn part_path(final_path: &Path) -> PathBuf {
    let mut name = final_path
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_default();
    name.push(".part");
    final_path.with_file_name(name)
}

/// 老版本用 `with_extension("part")`，升级前中断的下载要把旧临时文件认领过来，
/// 否则一个下到一半的大 jar 会从头再下一遍
fn adopt_legacy_part(final_path: &Path, part: &Path) {
    if part.exists() {
        return;
    }
    let legacy = final_path.with_extension("part");
    if legacy != part && legacy.exists() {
        debug!(
            "沿用旧版临时文件: {} -> {}",
            legacy.display(),
            part.display()
        );
        let _ = std::fs::rename(&legacy, part);
    }
}

#[allow(clippy::too_many_arguments)]
async fn download_chunk_with_resume(
    client: &reqwest::Client,
    url: &str,
    job: &DownloadJob,
    state: &Arc<AtomicBool>,
    global_cancel: &Arc<AtomicBool>,
    bytes_downloaded: &Arc<AtomicU64>,
    bytes_since_last: &Arc<AtomicU64>,
    resume_from: Option<u64>,
) -> Result<(), LauncherError> {
    // 小文件直写目标文件，没有 .part 中转
    let direct = resume_from.is_none() && is_small_file(job);
    let tmp_path = if direct {
        job.path.clone()
    } else {
        part_path(&job.path)
    };
    let mut bytes_added_this_attempt: u64 = 0;

    let result = async {
        if let Some(parent) = job.path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let (mut file, mut hasher) = open_sink(&tmp_path, resume_from).await?;

        let OpenedStream {
            mut response,
            resume,
            url: stream_url,
        } = open_stream(client, job, url, resume_from).await?;

        // 服务端不认 Range 时从头写，之前那半截作废
        if resume.is_none() && resume_from.is_some() {
            // 先刷掉缓冲再丢弃：里面可能还有上一次请求写进去的字节
            let _ = file.flush().await;
            drop(file);
            let raw = tokio::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&tmp_path)
                .await?;
            file = BufWriter::with_capacity(WRITE_BUFFER_BYTES, raw);
            hasher = FileHasher::new();
        }

        if let Some(offset) = resume {
            bytes_downloaded.fetch_add(offset, Ordering::Relaxed);
            bytes_added_this_attempt += offset;
        }

        while let Some(chunk) = response.chunk().await? {
            if !state.load(Ordering::Acquire) || global_cancel.load(Ordering::Acquire) {
                return Err(LauncherError::Cancelled);
            }
            file.write_all(&chunk).await?;
            // 顺手进哈希：省掉写完之后再把整个文件读一遍
            hasher.update(&chunk);
            let len = chunk.len() as u64;
            bytes_downloaded.fetch_add(len, Ordering::Relaxed);
            bytes_since_last.fetch_add(len, Ordering::Relaxed);
            bytes_added_this_attempt += len;
        }

        // 必须先把缓冲刷进文件，verify_downloaded 会去 stat / 重读它
        file.flush().await?;
        drop(file);

        let actual_hash = verify_downloaded(&tmp_path, job, hasher).await?;

        // 直写模式下 tmp_path 就是目标文件，不需要再搬一次
        if !direct {
            finalize_download(&tmp_path, &job.path).await?;
        }
        forget_prefix(&tmp_path);
        remember_fingerprint(&job.path, job.hash.as_deref(), actual_hash.as_deref());
        debug!("下载完成: {}", stream_url);

        Ok::<(), LauncherError>(())
    }
    .await;

    if result.is_err() {
        bytes_downloaded.fetch_sub(bytes_added_this_attempt, Ordering::Relaxed);
    }

    result
}

/// 向全局预算借分段连接，借不到就少借
///
/// 返回 (实际段数, 许可)；段数 < 2 时返回 None，让调用方直接走单连接。
/// 许可必须由调用方持有到下载结束，提前 drop 会让别的文件以为自己还能再借
fn acquire_multipart_permits(size: u64) -> Option<(usize, Vec<tokio::sync::OwnedSemaphorePermit>)> {
    let wanted = multipart_segments(size);
    if wanted < 2 {
        return None;
    }

    let mut permits = Vec::with_capacity(wanted);
    for _ in 0..wanted {
        match Arc::clone(&MULTIPART_PERMITS).try_acquire_owned() {
            Ok(permit) => permits.push(permit),
            Err(_) => break,
        }
    }

    (permits.len() >= 2).then_some((permits.len(), permits))
}

/// 按体积决定想切几段
///
/// 每段至少要几 MB 才划算：段太多则每段都要独立的请求往返与落盘偏移，小段反而比顺序写慢
fn multipart_segments(size: u64) -> usize {
    const MIN_SEGMENT_BYTES: u64 = 4 * 1024 * 1024;

    let by_size = (size / MIN_SEGMENT_BYTES) as usize;
    by_size.clamp(1, MULTIPART_MAX_SEGMENTS)
}

/// 把文件切成互不重叠的字节区间
fn segment_ranges(size: u64, segments: usize) -> Vec<(u64, u64)> {
    let segments = segments.max(1) as u64;
    let mut ranges = Vec::with_capacity(segments as usize);
    let chunk = size.div_ceil(segments);

    let mut start = 0;
    while start < size {
        let end = (start + chunk - 1).min(size - 1);
        ranges.push((start, end));
        start = end + 1;
    }

    ranges
}

/// 分段下载：各段并发拉到同一个 .part 的不同偏移，全部落地后再统一校验
///
/// 返回 `Ok(false)` 表示服务端不认 Range（回了 200 或没有 Content-Range），
/// 此时不能继续分段，交给调用方退回单连接
#[allow(clippy::too_many_arguments)]
async fn download_multipart(
    client: &reqwest::Client,
    url: &str,
    job: &DownloadJob,
    state: &Arc<AtomicBool>,
    global_cancel: &Arc<AtomicBool>,
    bytes_downloaded: &Arc<AtomicU64>,
    bytes_since_last: &Arc<AtomicU64>,
    segments: usize,
) -> Result<bool, LauncherError> {
    let tmp_path = part_path(&job.path);
    if let Some(parent) = job.path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    // 预分配到最终长度：各段可以独立 seek 写入，不必等前一段写完
    tokio::fs::File::create(&tmp_path)
        .await?
        .set_len(job.size)
        .await?;

    let ranges = segment_ranges(job.size, segments);
    let mut tasks = tokio::task::JoinSet::new();

    for (start, end) in ranges {
        tasks.spawn(download_segment(
            client.clone(),
            url.to_string(),
            tmp_path.clone(),
            start,
            end,
            state.clone(),
            global_cancel.clone(),
            bytes_downloaded.clone(),
            bytes_since_last.clone(),
        ));
    }

    let mut unsupported = false;
    let mut failure: Option<LauncherError> = None;

    while let Some(joined) = tasks.join_next().await {
        match joined {
            Ok(Ok(())) => {}
            Ok(Err(SegmentError::RangeUnsupported)) => {
                unsupported = true;
                // 有一段不支持就全都不成立，别让其余段继续白跑
                tasks.abort_all();
            }
            Ok(Err(SegmentError::Fatal(e))) => {
                if failure.is_none() {
                    failure = Some(e);
                }
                tasks.abort_all();
            }
            Err(e) => {
                if failure.is_none() {
                    failure = Some(LauncherError::Custom(format!("分段下载异常终止: {}", e)));
                }
            }
        }
    }

    if unsupported {
        let _ = tokio::fs::remove_file(&tmp_path).await;
        forget_prefix(&tmp_path);
        return Ok(false);
    }

    if let Some(e) = failure {
        let _ = tokio::fs::remove_file(&tmp_path).await;
        forget_prefix(&tmp_path);
        return Err(e);
    }

    // 分段模式下哈希是各段分别算的，拼不回一条完整的 sha1 流，只能整文件重读一遍。
    // 这份开销换 4 倍并发是划算的：读盘 ~1GB/s，而瓶颈通常在网络上
    let verify_path = tmp_path.clone();
    let hash = job.hash.clone();
    let size = job.size;
    let valid = tokio::task::spawn_blocking(move || {
        file_utils::verify_file(&verify_path, hash.as_deref(), size)
    })
    .await
    .map_err(|e| LauncherError::Custom(format!("校验分段文件失败: {}", e)))??;

    if !valid {
        let _ = tokio::fs::remove_file(&tmp_path).await;
        forget_prefix(&tmp_path);
        return Err(LauncherError::Custom(format!(
            "File verification failed for {}: segment download mismatched (corrupted file deleted).",
            tmp_path.display()
        )));
    }

    finalize_download(&tmp_path, &job.path).await?;
    forget_prefix(&tmp_path);
    // 内容哈希由 verify_file 自己写进缓存（它已经整文件读过一遍算过了），这里只补期望值
    remember_fingerprint(&job.path, job.hash.as_deref(), None);

    Ok(true)
}

enum SegmentError {
    /// 服务端没按 Range 回，分段方案不成立
    RangeUnsupported,
    Fatal(LauncherError),
}

impl From<LauncherError> for SegmentError {
    fn from(error: LauncherError) -> Self {
        Self::Fatal(error)
    }
}

impl From<reqwest::Error> for SegmentError {
    fn from(error: reqwest::Error) -> Self {
        Self::Fatal(LauncherError::Http(error))
    }
}

impl From<std::io::Error> for SegmentError {
    fn from(error: std::io::Error) -> Self {
        Self::Fatal(LauncherError::Io(error))
    }
}

#[allow(clippy::too_many_arguments)]
async fn download_segment(
    client: reqwest::Client,
    url: String,
    path: PathBuf,
    start: u64,
    end: u64,
    state: Arc<AtomicBool>,
    global_cancel: Arc<AtomicBool>,
    bytes_downloaded: Arc<AtomicU64>,
    bytes_since_last: Arc<AtomicU64>,
) -> Result<(), SegmentError> {
    let response = client
        .get(&url)
        .header("Range", format!("bytes={}-{}", start, end))
        .send()
        .await?;

    let status = response.status();
    if status != reqwest::StatusCode::PARTIAL_CONTENT {
        // 200 说明服务端把 Range 忽略了；其余状态码是真错误
        return Err(if status.is_success() {
            SegmentError::RangeUnsupported
        } else {
            SegmentError::Fatal(LauncherError::Custom(format!(
                "分段请求返回 HTTP {} for {}",
                status, url
            )))
        });
    }

    let mut raw = tokio::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .await?;
    raw.seek(std::io::SeekFrom::Start(start)).await?;
    // 每段也是逐 chunk 写，同样要攒批；段之间不共享文件句柄，缓冲各算各的
    let mut file = BufWriter::with_capacity(WRITE_BUFFER_BYTES, raw);

    let expected = end - start + 1;
    let mut written: u64 = 0;
    let mut response = response;

    while let Some(chunk) = response.chunk().await? {
        if !state.load(Ordering::Acquire) || global_cancel.load(Ordering::Acquire) {
            // 取消时也要把已收到的部分刷出去，否则这段等于白下
            let _ = file.flush().await;
            return Err(SegmentError::Fatal(LauncherError::Cancelled));
        }
        file.write_all(&chunk).await?;
        let len = chunk.len() as u64;
        written += len;
        bytes_downloaded.fetch_add(len, Ordering::Relaxed);
        bytes_since_last.fetch_add(len, Ordering::Relaxed);
    }

    file.flush().await?;

    // 段内长度对不上说明连接中途断了，缺的那截会被当成功写进去
    if written != expected {
        return Err(SegmentError::Fatal(LauncherError::Custom(format!(
            "分段 {}..{} 长度不符: 期望 {} 字节，实收 {}",
            start, end, expected, written
        ))));
    }

    Ok(())
}

/// 校验刚下完的数据：大小 + 哈希，失败删掉 .part 让下次从头来
///
/// 返回这次算出的内容哈希（没有期望哈希可算时为 None），供调用方写进指纹缓存
async fn verify_downloaded(
    tmp_path: &Path,
    job: &DownloadJob,
    hasher: FileHasher,
) -> Result<Option<String>, LauncherError> {
    let actual_size = tokio::fs::metadata(tmp_path).await?.len();

    if job.size > 0 && actual_size != job.size {
        let reason = format!("expected {} bytes, got {}", job.size, actual_size);
        let _ = tokio::fs::remove_file(tmp_path).await;
        warn!("下载校验失败: {} ({})", tmp_path.display(), reason);
        return Err(LauncherError::Custom(format!(
            "File verification failed for {}: {} (corrupted file deleted).",
            tmp_path.display(),
            reason
        )));
    }

    let actual = hasher.finish_hex();

    if let Some(expected) = job.hash.as_deref()
        && !actual.eq_ignore_ascii_case(expected.trim())
    {
        let reason = format!("expected sha1 {}, got {}", expected, actual);
        let _ = tokio::fs::remove_file(tmp_path).await;
        warn!("下载校验失败: {} ({})", tmp_path.display(), reason);
        return Err(LauncherError::Custom(format!(
            "File verification failed for {}: {} (corrupted file deleted).",
            tmp_path.display(),
            reason
        )));
    }

    Ok(Some(actual))
}

/// 记下「这份文件已校验通过」，下次安装命中指纹就跳过读盘
fn remember_fingerprint(path: &Path, expected_hash: Option<&str>, actual_hash: Option<&str>) {
    if let Ok(meta) = std::fs::metadata(path) {
        verify_cache::remember(path, &meta, expected_hash, actual_hash);
    }
}

/// 续传时把 `.part` 里已有的前缀补算进哈希；首次下载直接给个空哈希器
///
/// 返回带缓冲的写入端：调用方必须 `flush` 之后再用文件（校验前一定要先刷出去）
async fn open_sink(
    tmp_path: &Path,
    resume_from: Option<u64>,
) -> Result<(BufWriter<tokio::fs::File>, FileHasher), LauncherError> {
    match resume_from {
        Some(offset) => {
            let mut file = tokio::fs::OpenOptions::new()
                .write(true)
                .open(tmp_path)
                .await?;
            file.seek(std::io::SeekFrom::End(0)).await?;
            let hasher = hash_prefix(tmp_path, offset).await?;
            Ok((BufWriter::with_capacity(WRITE_BUFFER_BYTES, file), hasher))
        }
        None => {
            let file = tokio::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(tmp_path)
                .await?;
            Ok((
                BufWriter::with_capacity(WRITE_BUFFER_BYTES, file),
                FileHasher::new(),
            ))
        }
    }
}

/// 把 `.part` 前 `bytes` 个字节重新喂进哈希
///
/// 只有真正中断过的文件才会走这里，读盘量等于上次已下载的量；不补算的话续传完成后的 sha1 必然对不上。
/// 重试会把这里重复跑很多遍，所以先查进程内缓存：命中要求「文件长度没变过」，
/// 长度一变说明中途又写了新数据，只能重算
async fn hash_prefix(path: &Path, bytes: u64) -> Result<FileHasher, LauncherError> {
    if let Some(hasher) = cached_prefix(path, bytes) {
        debug!(
            "复用续传前缀哈希: {} 前 {} 字节（跳过重读）",
            path.display(),
            bytes
        );
        return Ok(hasher);
    }

    let path_buf = path.to_path_buf();
    let hasher = tokio::task::spawn_blocking(move || -> Result<FileHasher, LauncherError> {
        use std::io::Read;

        let mut file = std::fs::File::open(&path_buf)?;
        let mut hasher = FileHasher::new();
        let mut remaining = bytes;
        let mut buf = vec![0u8; 1024 * 1024];
        while remaining > 0 {
            let want = remaining.min(buf.len() as u64) as usize;
            let read = file.read(&mut buf[..want])?;
            if read == 0 {
                break;
            }
            hasher.update(&buf[..read]);
            remaining -= read as u64;
        }
        Ok(hasher)
    })
    .await
    .map_err(|e| LauncherError::Custom(format!("计算续传前缀哈希失败: {}", e)))??;

    remember_prefix(path, bytes, &hasher);
    Ok(hasher)
}

/// 续传前缀哈希的进程内缓存：路径 -> (已算长度, 该长度处的哈希器状态)
///
/// 只用来省重试时的重读，丢了不影响正确性；键里带上文件长度，防止 .part 又写了数据后仍命中旧状态
static PREFIX_CACHE: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<PathBuf, (u64, FileHasher)>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

/// 缓存条目上限：同时下载的文件数不会太多，超了就整体丢弃，维护 LRU 不划算
const PREFIX_CACHE_MAX: usize = 64;

fn cached_prefix(path: &Path, bytes: u64) -> Option<FileHasher> {
    let cache = PREFIX_CACHE.lock().ok()?;
    let (cached_len, hasher) = cache.get(path)?;
    // 长度必须完全一致：短了状态不够用，长了说明这份状态已经过期
    (*cached_len == bytes).then(|| hasher.clone())
}

fn remember_prefix(path: &Path, bytes: u64, hasher: &FileHasher) {
    let Ok(mut cache) = PREFIX_CACHE.lock() else {
        return;
    };
    if cache.len() >= PREFIX_CACHE_MAX && !cache.contains_key(path) {
        cache.clear();
    }
    cache.insert(path.to_path_buf(), (bytes, hasher.clone()));
}

/// 下载完成或放弃时清掉缓存，避免把已经删掉的 .part 的状态留在内存里
fn forget_prefix(path: &Path) {
    if let Ok(mut cache) = PREFIX_CACHE.lock() {
        cache.remove(path);
    }
}

fn validate_content_type(response: &reqwest::Response, url: &str) -> Result<(), LauncherError> {
    if let Some(ct) = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
    {
        let ct_lower = ct.to_ascii_lowercase();
        if ct_lower.starts_with("text/") || ct_lower.contains("json") || ct_lower.contains("html") {
            return Err(LauncherError::Custom(format!(
                "Unexpected Content-Type {} for {}",
                ct, url
            )));
        }
    }
    Ok(())
}

fn validate_content_length(
    response: &reqwest::Response,
    url: &str,
    expected_size: u64,
    resume_from: Option<u64>,
) -> Result<(), LauncherError> {
    if let Some(len_hdr) = response.headers().get(reqwest::header::CONTENT_LENGTH)
        && let Ok(len_str) = len_hdr.to_str()
        && let Ok(remote_len) = len_str.parse::<u64>()
    {
        // 对于续传，Content-Length 是剩余部分的大小
        let expected_len = if let Some(offset) = resume_from {
            expected_size.saturating_sub(offset)
        } else {
            expected_size
        };

        if remote_len == 0 && expected_len > 0 {
            return Err(LauncherError::Custom(format!(
                "Unexpected Content-Length 0 for {}, expected {}",
                url, expected_len
            )));
        }
    }
    Ok(())
}

async fn finalize_download(tmp_path: &Path, final_path: &Path) -> Result<(), LauncherError> {
    if let Some(parent) = final_path.parent() {
        tokio::fs::create_dir_all(parent).await.ok();
    }
    if tokio::fs::metadata(final_path).await.is_ok() {
        let _ = tokio::fs::remove_file(final_path).await;
    }
    tokio::fs::rename(tmp_path, final_path).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        DIRECT_WRITE_LIMIT, MULTIPART_MAX_SEGMENTS, MULTIPART_MIN_BYTES, RACE_MIN_BYTES,
        cached_prefix, download_file, forget_prefix, hash_prefix, is_small_file,
        multipart_segments, part_path, remember_prefix, segment_ranges,
    };
    use crate::models::DownloadJob;
    use crate::utils::file_utils::FileHasher;
    use std::path::{Path, PathBuf};

    fn job(size: u64) -> DownloadJob {
        DownloadJob {
            url: "https://example.com/x".to_string(),
            fallback_url: None,
            path: PathBuf::from("x.jar"),
            size,
            hash: None,
        }
    }

    #[test]
    fn test_small_files_take_the_direct_write_path() {
        assert!(is_small_file(&job(DIRECT_WRITE_LIMIT)));
        assert!(is_small_file(&job(1)));
        // 大小未知（0）时不敢省续传，仍然走 .part
        assert!(!is_small_file(&job(0)));
        assert!(!is_small_file(&job(DIRECT_WRITE_LIMIT + 1)));
    }

    #[test]
    fn test_part_path_appends_suffix_instead_of_replacing_extension() {
        assert_eq!(
            part_path(Path::new("/x/assets/ab/abcdef")),
            Path::new("/x/assets/ab/abcdef.part")
        );
        // 同一目录下的两个 jar 不能算出同一个临时文件
        assert_ne!(
            part_path(Path::new("/x/a.jar")),
            part_path(Path::new("/x/b.jar"))
        );
        assert_eq!(part_path(Path::new("/x/a.jar")), Path::new("/x/a.jar.part"));
    }

    #[test]
    fn test_streamed_hash_matches_single_shot_hash() {
        let data: Vec<u8> = (0..70_000u32).map(|i| (i % 251) as u8).collect();

        let mut hasher = FileHasher::new();
        for chunk in data.chunks(1024) {
            hasher.update(chunk);
        }

        assert_eq!(
            hasher.finish_hex(),
            crate::utils::file_utils::sha1_hex(&data)
        );
    }

    /// 续传重试时复用缓存的前缀状态，算出来的哈希必须和重新读一遍完全一致；
    /// 长度对不上时必须拒绝命中，否则会拿半截状态去校验完整文件
    #[tokio::test]
    async fn test_prefix_hash_cache_round_trips_and_rejects_wrong_length() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("a.jar.part");
        let data: Vec<u8> = (0..4096u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&path, &data).unwrap();

        // 首次：没有缓存，真读一遍
        let first = hash_prefix(&path, 2048).await.unwrap();
        assert_eq!(
            first.finish_hex(),
            crate::utils::file_utils::sha1_hex(&data[..2048])
        );

        // 第二次：应当命中缓存，结果一致
        let second = hash_prefix(&path, 2048).await.unwrap();
        assert_eq!(
            second.finish_hex(),
            crate::utils::file_utils::sha1_hex(&data[..2048])
        );
        assert!(cached_prefix(&path, 2048).is_some());

        // 长度不同不能命中，否则会把「算到 2048」的状态当成 4096 的
        assert!(cached_prefix(&path, 4096).is_none());

        forget_prefix(&path);
        assert!(cached_prefix(&path, 2048).is_none());
    }

    /// 缓存只影响重试时的读盘量，算出来的哈希必须与直接算一致
    #[test]
    fn test_cached_prefix_hasher_clone_is_independent() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("b.part");
        std::fs::write(&path, b"0123456789").unwrap();

        let mut base = FileHasher::new();
        base.update(b"0123456789");
        remember_prefix(&path, 10, &base);

        // 拿到缓存后继续 update，不能反过来污染缓存里的那份状态
        let mut resumed = cached_prefix(&path, 10).unwrap();
        resumed.update(b"extra");

        let again = cached_prefix(&path, 10).unwrap();
        assert_eq!(
            again.finish_hex(),
            crate::utils::file_utils::sha1_hex(b"0123456789")
        );
        forget_prefix(&path);
    }

    /// 段数随体积增长但有上限，小于起跳阈值时不切
    #[test]
    fn test_multipart_segment_count_scales_and_caps() {
        assert_eq!(multipart_segments(MULTIPART_MIN_BYTES - 1), 1);
        assert_eq!(multipart_segments(4 * 1024 * 1024), 1);
        assert_eq!(multipart_segments(8 * 1024 * 1024), 2);
        assert_eq!(multipart_segments(16 * 1024 * 1024), 4);
        assert_eq!(multipart_segments(1024 * 1024 * 1024), 4);
        assert_eq!(multipart_segments(u64::MAX), MULTIPART_MAX_SEGMENTS);
    }

    /// 分段必须首尾相接、无重叠、无空洞，否则文件会缺字节或互相覆盖
    #[test]
    fn test_segment_ranges_cover_the_file_exactly_once() {
        for (size, segments) in [
            (1000u64, 1usize),
            (1000, 2),
            (1000, 3),
            (1000, 4),
            (1000, 7),
            (8 * 1024 * 1024, 4),
        ] {
            let ranges = segment_ranges(size, segments);

            assert_eq!(ranges.first().unwrap().0, 0, "第一段必须从 0 开始");
            assert_eq!(ranges.last().unwrap().1, size - 1, "最后一段必须到文件尾");

            for (index, (start, end)) in ranges.iter().enumerate() {
                assert!(start <= end, "区间不能反向: {start}..{end}");
                if index > 0 {
                    assert_eq!(
                        *start,
                        ranges[index - 1].1 + 1,
                        "第 {index} 段与前一段之间不能有空洞或重叠"
                    );
                }
            }

            let total: u64 = ranges.iter().map(|(start, end)| end - start + 1).sum();
            assert_eq!(total, size, "各段总字节数必须正好等于文件大小");
        }
    }

    /// 体积小于一段时只能切出一段，range 不能被算成 0..size（越界一个字节）
    #[test]
    fn test_segment_ranges_handle_tiny_files() {
        assert_eq!(segment_ranges(1, 4), vec![(0, 0)]);
        assert_eq!(
            segment_ranges(5, 9),
            vec![(0, 0), (1, 1), (2, 2), (3, 3), (4, 4)]
        );
    }

    /// 竞速与分段的起跳阈值必须高于直写阈值，否则小文件也会被拉去做两件事
    #[test]
    fn test_thresholds_are_ordered() {
        const { assert!(DIRECT_WRITE_LIMIT < RACE_MIN_BYTES) };
        const { assert!(RACE_MIN_BYTES < MULTIPART_MIN_BYTES) };
    }

    /// 起一个最小 HTTP 服务，`honour_range` 决定它认不认 Range 头
    ///
    /// 返回 (base_url, Range 请求计数)。监听线程随测试进程结束，
    /// 测试里只关心「服务端到底收到几条分段请求」
    fn spawn_range_server(
        data: Vec<u8>,
        honour_range: bool,
    ) -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        spawn_delayed_server(data, honour_range, 0)
    }

    /// 同上，但每个响应固定延迟一段时间，用来模拟「主源慢、备源快」
    fn spawn_delayed_server(
        data: Vec<u8>,
        honour_range: bool,
        delay_ms: u64,
    ) -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        use std::io::{BufRead, BufReader, Write};
        use std::sync::atomic::{AtomicUsize, Ordering};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("无法绑定测试端口");
        let port = listener.local_addr().unwrap().port();
        let data = std::sync::Arc::new(data);
        let range_hits = std::sync::Arc::new(AtomicUsize::new(0));

        let counter = range_hits.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let data = data.clone();
                let range_hits = counter.clone();
                std::thread::spawn(move || {
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut request_line = String::new();
                    if reader.read_line(&mut request_line).is_err() {
                        return;
                    }

                    let mut range_header = None;
                    loop {
                        let mut line = String::new();
                        if reader.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
                            break;
                        }
                        let lower = line.to_ascii_lowercase();
                        if let Some(value) = lower.strip_prefix("range:") {
                            range_header = Some(value.trim().to_string());
                        }
                    }

                    // 延迟放在读完请求头之后：模拟「连得上但迟迟不回」的慢源
                    if delay_ms > 0 {
                        std::thread::sleep(std::time::Duration::from_millis(delay_ms));
                    }

                    // 只解析 `bytes=start-end` / `bytes=start-` 两种形态，够测试用
                    let parsed = range_header.as_deref().and_then(|value| {
                        let spec = value.strip_prefix("bytes=")?;
                        let (start, end) = spec.split_once('-')?;
                        let start: u64 = start.trim().parse().ok()?;
                        let end: u64 = end.trim().parse().unwrap_or(u64::MAX);
                        Some((start, end))
                    });

                    match parsed.filter(|_| honour_range) {
                        Some((start, end)) => {
                            range_hits.fetch_add(1, Ordering::Relaxed);
                            let end = end.min(data.len() as u64 - 1);
                            let body = &data[start as usize..=end as usize];
                            let head = format!(
                                "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Type: application/octet-stream\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n",
                                body.len()
                            );
                            let _ = stream.write_all(head.as_bytes());
                            let _ = stream.write_all(body);
                        }
                        None => {
                            let head = format!(
                                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/octet-stream\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n",
                                data.len()
                            );
                            let _ = stream.write_all(head.as_bytes());
                            let _ = stream.write_all(&data);
                        }
                    }
                });
            }
        });

        (format!("http://127.0.0.1:{}/file.bin", port), range_hits)
    }

    fn payload(size: usize) -> Vec<u8> {
        (0..size).map(|index| (index % 251) as u8).collect()
    }

    fn big_job(path: std::path::PathBuf, size: u64, hash: &str) -> DownloadJob {
        DownloadJob {
            url: "https://example.com/big.jar".to_string(),
            fallback_url: None,
            path,
            size,
            hash: Some(hash.to_string()),
        }
    }

    /// 端到端：大文件走分段，服务端真的收到多条 Range 请求，拼回来的字节完全一致
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_large_file_is_downloaded_in_segments_and_reassembled() {
        use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

        // 8 MiB 起跳 + 一点零头，确保最后一段不是整段、边界对齐被真正测到
        let size = MULTIPART_MIN_BYTES as usize + 12_345;
        let data = payload(size);
        let (url, range_hits) = spawn_range_server(data.clone(), true);

        let dir = tempfile::TempDir::new().unwrap();
        let target = dir.path().join("segment.bin");
        let hash = crate::utils::file_utils::sha1_hex(&data);
        let job = big_job(target.clone(), size as u64, &hash);

        let state = std::sync::Arc::new(AtomicBool::new(true));
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let downloaded = std::sync::Arc::new(AtomicU64::new(0));
        let since_last = std::sync::Arc::new(AtomicU64::new(0));

        download_file(
            &crate::services::download::http::get_http_client(),
            &job,
            &url,
            &state,
            &cancel,
            &downloaded,
            &since_last,
        )
        .await
        .expect("分段下载应当成功");

        let written = std::fs::read(&target).unwrap();
        assert_eq!(written.len(), size, "落盘长度必须等于文件大小");
        assert!(written == data, "分段拼回来的内容与原始数据不一致");

        assert!(
            range_hits.load(Ordering::Relaxed) >= 2,
            "大文件应当被切成多段请求，实际只有 {} 条 Range 请求",
            range_hits.load(Ordering::Relaxed)
        );
        assert_eq!(
            downloaded.load(Ordering::Relaxed),
            size as u64,
            "进度计数必须正好等于文件大小，多算会显示出 100% 以上"
        );
    }

    /// 服务端不认 Range 时必须退回单连接并下完整文件，不能直接失败
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_server_without_range_support_falls_back_to_single_stream() {
        use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

        let size = MULTIPART_MIN_BYTES as usize + 4_096;
        let data = payload(size);
        let (url, range_hits) = spawn_range_server(data.clone(), false);

        let dir = tempfile::TempDir::new().unwrap();
        let target = dir.path().join("single.bin");
        let hash = crate::utils::file_utils::sha1_hex(&data);
        let job = big_job(target.clone(), size as u64, &hash);

        let state = std::sync::Arc::new(AtomicBool::new(true));
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let downloaded = std::sync::Arc::new(AtomicU64::new(0));
        let since_last = std::sync::Arc::new(AtomicU64::new(0));

        download_file(
            &crate::services::download::http::get_http_client(),
            &job,
            &url,
            &state,
            &cancel,
            &downloaded,
            &since_last,
        )
        .await
        .expect("不认 Range 也应当能下完");

        assert_eq!(std::fs::read(&target).unwrap(), data);
        assert_eq!(
            range_hits.load(Ordering::Relaxed),
            0,
            "服务端没回 206，不该有成功的分段请求"
        );
    }

    /// 主源拖着不回、备源正常时，竞速应当直接采用备源，而不是等主源超时
    ///
    /// 取 2 MiB..8 MiB 之间：这个区间够大值得竞速，又还没到分段的起跳线，
    /// 走的正是 `open_stream` 的竞速分支
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_slow_primary_loses_the_race_to_the_fallback() {
        use std::sync::atomic::{AtomicBool, AtomicU64};

        const PRIMARY_DELAY_MS: u64 = 3_000;
        let size = (RACE_MIN_BYTES + 64 * 1024) as usize;
        let data = payload(size);

        let (slow_url, _) = spawn_delayed_server(data.clone(), true, PRIMARY_DELAY_MS);
        let (fast_url, _) = spawn_range_server(data.clone(), true);

        let dir = tempfile::TempDir::new().unwrap();
        let target = dir.path().join("raced.bin");
        let hash = crate::utils::file_utils::sha1_hex(&data);

        let job = DownloadJob {
            url: slow_url,
            fallback_url: Some(fast_url),
            path: target.clone(),
            size: size as u64,
            hash: Some(hash),
        };

        let state = std::sync::Arc::new(AtomicBool::new(true));
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let downloaded = std::sync::Arc::new(AtomicU64::new(0));
        let since_last = std::sync::Arc::new(AtomicU64::new(0));

        let started = std::time::Instant::now();
        download_file(
            &crate::services::download::http::get_http_client(),
            &job,
            &job.url,
            &state,
            &cancel,
            &downloaded,
            &since_last,
        )
        .await
        .expect("竞速应当拿到备源并下完");
        let elapsed = started.elapsed();

        assert_eq!(
            std::fs::read(&target).unwrap(),
            data,
            "内容必须来自胜出的源且完整"
        );
        assert!(
            elapsed < std::time::Duration::from_millis(PRIMARY_DELAY_MS),
            "应当在慢源返回之前就用上快源，实际耗时 {:?}",
            elapsed
        );
    }
}
