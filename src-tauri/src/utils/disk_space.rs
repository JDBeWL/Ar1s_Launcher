//! 磁盘可用空间探测与安装前预检
//!
//! 只在「准备写入一大批文件」之前调用，用一条可读的错误替代写到一半的 IO 失败。
//! 探测失败一律放行：拿不到空间信息时不能把安装拦死。

use crate::errors::LauncherError;
use crate::models::DownloadJob;
use std::path::{Path, PathBuf};
use sysinfo::Disks;

/// 预留余量：至少 256 MiB，需求越大按 10% 追加（解压、临时文件、日志都要占地方）
const MIN_MARGIN_BYTES: u64 = 256 * 1024 * 1024;
const MARGIN_PERCENT: u64 = 10;

/// `path` 所在卷的可用字节数；路径不存在时向上找最近的已存在祖先
pub fn available_bytes(path: &Path) -> Option<u64> {
    let target = normalize(&existing_ancestor(path)?);
    // sysinfo 0.40 起构造磁盘列表会失败，拿不到就交给调用方按「未知」处理
    let disks = Disks::new_with_refreshed_list().ok()?;

    disks
        .iter()
        .filter(|disk| target.starts_with(disk.mount_point()))
        .max_by_key(|disk| disk.mount_point().as_os_str().len())
        .map(|disk| disk.available_space())
}

/// 写 `needed` 字节之前先确认空间够用，不够就返回 `disk_insufficient`
pub fn ensure_free_space(path: &Path, needed: u64) -> Result<(), LauncherError> {
    let Some(available) = available_bytes(path) else {
        return Ok(());
    };

    let required = needed.saturating_add(margin_for(needed));
    if available >= required {
        return Ok(());
    }

    Err(LauncherError::localized(
        "disk_insufficient",
        format!(
            "磁盘空间不足：{} 需要约 {}，当前可用 {}",
            path.display(),
            format_bytes(required),
            format_bytes(available)
        ),
        vec![
            format_bytes(required),
            format_bytes(available),
            path.display().to_string(),
        ],
    ))
}

/// 一批下载任务还差多少字节：已有文件按实际长度抵扣，支持断点续传的 `.part` 不重复计算
pub fn missing_download_bytes(jobs: &[DownloadJob]) -> u64 {
    jobs.iter()
        .map(|job| {
            if job.size == 0 {
                return 0;
            }
            let existing = std::fs::metadata(&job.path)
                .map(|meta| meta.len())
                .unwrap_or(0);
            job.size.saturating_sub(existing)
        })
        .sum()
}

/// 批量下载前的预检；没有实际要下的文件时直接放行
pub fn ensure_space_for_download(path: &Path, jobs: &[DownloadJob]) -> Result<(), LauncherError> {
    let needed = missing_download_bytes(jobs);
    if needed == 0 {
        return Ok(());
    }
    ensure_free_space(path, needed)
}

/// 人类可读的容量，用于错误提示
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];

    if bytes < 1024 {
        return format!("{} B", bytes);
    }

    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{:.1} {}", value, UNITS[unit])
}

fn margin_for(needed: u64) -> u64 {
    std::cmp::max(MIN_MARGIN_BYTES, needed / MARGIN_PERCENT)
}

fn existing_ancestor(path: &Path) -> Option<PathBuf> {
    let mut current = path.to_path_buf();
    loop {
        if current.exists() {
            return Some(current);
        }
        if !current.pop() {
            return None;
        }
    }
}

/// Windows 的 `canonicalize` 返回 `\\?\C:\...`，前缀会让 `starts_with("C:\")` 失配
fn normalize(path: &Path) -> PathBuf {
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());

    #[cfg(windows)]
    {
        let text = canonical.to_string_lossy();
        if let Some(stripped) = text.strip_prefix(r"\\?\") {
            return PathBuf::from(stripped);
        }
    }

    canonical
}

#[cfg(test)]
mod tests {
    use super::{
        DownloadJob, MIN_MARGIN_BYTES, available_bytes, ensure_free_space, format_bytes,
        margin_for, missing_download_bytes,
    };

    fn job(path: std::path::PathBuf, size: u64) -> DownloadJob {
        DownloadJob {
            url: "https://example.invalid/file".to_string(),
            fallback_urls: Vec::new(),
            path,
            size,
            hash: None,
        }
    }

    #[test]
    fn test_format_bytes_picks_a_readable_unit() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1024), "1.0 KB");
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_bytes(3 * 1024 * 1024 * 1024), "3.0 GB");
    }

    #[test]
    fn test_margin_is_at_least_the_floor() {
        assert_eq!(margin_for(0), MIN_MARGIN_BYTES);
        assert_eq!(margin_for(1024), MIN_MARGIN_BYTES);
        assert_eq!(margin_for(100 * MIN_MARGIN_BYTES), 10 * MIN_MARGIN_BYTES);
    }

    #[test]
    fn test_available_bytes_reads_an_existing_directory() {
        let dir = tempfile::TempDir::new().unwrap();
        let available = available_bytes(dir.path()).expect("临时目录所在卷必须可探测");
        assert!(available > 0);
    }

    #[test]
    fn test_available_bytes_walks_up_to_a_missing_path_parent() {
        let dir = tempfile::TempDir::new().unwrap();
        let missing = dir.path().join("not/created/yet");
        assert!(available_bytes(&missing).is_some());
    }

    #[test]
    fn test_missing_download_bytes_credits_existing_files() {
        let dir = tempfile::TempDir::new().unwrap();
        let present = dir.path().join("present.jar");
        std::fs::write(&present, vec![0u8; 100]).unwrap();

        let jobs = vec![
            job(present, 100),
            job(dir.path().join("absent.jar"), 300),
            // 断点续传的 .part 就是目标文件本身，已下载的部分要抵扣
            job(dir.path().join("partial.jar"), 1000),
        ];
        std::fs::write(dir.path().join("partial.jar"), vec![0u8; 400]).unwrap();

        // 完整文件不计入；缺失的 300 字节 + 只下了 400/1000 的剩余 600 字节
        assert_eq!(missing_download_bytes(&jobs), 900);
    }

    #[test]
    fn test_ensure_free_space_rejects_an_impossible_request() {
        let dir = tempfile::TempDir::new().unwrap();
        let error = ensure_free_space(dir.path(), u64::MAX / 2).unwrap_err();

        assert_eq!(error.code(), "disk_insufficient");
        // 三个参数：需要、可用、路径
        assert_eq!(error.params().len(), 3);
        assert!(error.params()[2].contains(dir.path().to_string_lossy().as_ref()));
    }

    #[test]
    fn test_ensure_free_space_allows_a_small_request() {
        let dir = tempfile::TempDir::new().unwrap();
        assert!(ensure_free_space(dir.path(), 1024).is_ok());
    }
}
