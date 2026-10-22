//! 跨进程文件锁
//!
//! 锁文件由内核在最后一个持有者释放时删除，进程被强杀也不留垃圾。
//! Windows：`FILE_FLAG_DELETE_ON_CLOSE` 加共享模式做互斥（独占 `share_mode(0)`，共享 READ|DELETE）。
//! Unix：`flock`，加锁后必须确认路径仍指向同一个 inode，否则说明别人删了重建，要重来；释放时先 unlink 再关闭。

use crate::errors::LauncherError;
use std::fs::File;
use std::path::Path;
use std::time::{Duration, Instant};

const RETRY_INTERVAL: Duration = Duration::from_millis(25);

#[derive(Debug)]
pub struct FileLock {
    /// Windows 下只靠持有句柄维持锁（句柄关闭即删文件），Unix 下还要显式解锁
    #[cfg_attr(windows, allow(dead_code))]
    file: File,
    /// Unix 下释放时要 unlink，Windows 靠内核删除，不需要路径
    #[cfg(unix)]
    path: std::path::PathBuf,
}

impl Drop for FileLock {
    fn drop(&mut self) {
        // Windows 由 DELETE_ON_CLOSE 负责，这里什么都不用做
        #[cfg(unix)]
        {
            // 先解锁，再判断能不能删。
            //
            // 不能直接 remove_file：共享锁允许多个持有者，第一个释放时删掉文件，
            // 其余持有者锁住的就成了一个已删除的 inode，之后新来的排他锁会
            // 在新建的空文件上成功——互斥直接失效，正在跑的游戏会被当成"没人在跑"。
            //
            // 解锁后立刻抢一次排他锁：抢得到说明确实没人了，删掉；
            // 抢不到说明还有别的持有者，原样留着，等最后一个释放时再删
            let _ = self.file.unlock();

            if let Ok(file) = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&self.path)
                && file.try_lock().is_ok()
            {
                // 仍持有排他锁时删除：删除与重建之间不会有人插队
                let _ = std::fs::remove_file(&self.path);
                let _ = file.unlock();
            }
        }
    }
}

fn busy_error(path: &Path, timeout: Duration) -> LauncherError {
    LauncherError::Busy(format!(
        "{}（等待 {:.1} 秒超时）",
        path.display(),
        timeout.as_secs_f32()
    ))
}

fn open_error(path: &Path, err: std::io::Error) -> LauncherError {
    LauncherError::Custom(format!("打开锁文件 {} 失败: {}", path.display(), err))
}

// ---------------------------------------------------------------- Windows

#[cfg(windows)]
mod platform {
    use std::fs::{File, OpenOptions};
    use std::io;
    use std::os::windows::fs::OpenOptionsExt;
    use std::path::Path;

    const GENERIC_READ: u32 = 0x8000_0000;
    const GENERIC_WRITE: u32 = 0x4000_0000;
    const DELETE: u32 = 0x0001_0000;
    const FILE_SHARE_READ: u32 = 0x0000_0001;
    const FILE_SHARE_DELETE: u32 = 0x0000_0004;
    const FILE_FLAG_DELETE_ON_CLOSE: u32 = 0x0400_0000;

    const ERROR_ACCESS_DENIED: i32 = 5;
    const ERROR_SHARING_VIOLATION: i32 = 32;

    /// 共享模式冲突就是"别人正持有"，当作可重试的忙
    pub fn is_busy(err: &io::Error) -> bool {
        matches!(
            err.raw_os_error(),
            Some(ERROR_SHARING_VIOLATION) | Some(ERROR_ACCESS_DENIED)
        )
    }

    /// 独占：不允许任何其他句柄同时打开，文件在最后一个句柄关闭时删除
    pub fn open_exclusive(path: &Path) -> io::Result<File> {
        OpenOptions::new()
            .write(true)
            .access_mode(GENERIC_READ | GENERIC_WRITE | DELETE)
            .share_mode(0)
            .custom_flags(FILE_FLAG_DELETE_ON_CLOSE)
            .create(true)
            .open(path)
    }

    /// 共享：多个持有者可以同时打开；独占打开会因共享冲突失败。
    /// 文件不存在时先创建，再以共享模式打开（共享打开本身不带写权限）
    pub fn open_shared(path: &Path) -> io::Result<File> {
        let open = || {
            OpenOptions::new()
                .read(true)
                .access_mode(GENERIC_READ | DELETE)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_DELETE)
                .custom_flags(FILE_FLAG_DELETE_ON_CLOSE)
                .open(path)
        };

        match open() {
            Ok(file) => Ok(file),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                // 只建空文件，不持有任何锁；紧接着的共享打开才是加锁动作
                let _ = OpenOptions::new()
                    .create(true)
                    .truncate(false)
                    .write(true)
                    .open(path);
                open()
            }
            Err(e) => Err(e),
        }
    }
}

// ------------------------------------------------------------------- Unix

#[cfg(unix)]
mod platform {
    use std::fs::{File, OpenOptions};
    use std::io;
    use std::path::Path;

    pub fn is_busy(_err: &io::Error) -> bool {
        false
    }

    pub fn open_exclusive(path: &Path) -> io::Result<File> {
        OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
    }

    pub fn open_shared(path: &Path) -> io::Result<File> {
        open_exclusive(path)
    }

    /// 加锁成功后确认路径仍指向我们锁住的那个 inode。
    /// 不匹配说明别人在我们打开与加锁之间删掉重建了文件，必须重来，
    /// 否则会锁着一个已经没人引用的旧 inode，互斥就失效了
    pub fn still_owns_path(file: &File, path: &Path) -> bool {
        use std::os::unix::fs::MetadataExt;

        let (Ok(opened), Ok(current)) = (file.metadata(), std::fs::metadata(path)) else {
            return false;
        };
        opened.dev() == current.dev() && opened.ino() == current.ino()
    }
}

// ------------------------------------------------------------------- 通用

fn try_open(path: &Path, shared: bool) -> Result<Option<File>, LauncherError> {
    let result = if shared {
        platform::open_shared(path)
    } else {
        platform::open_exclusive(path)
    };

    match result {
        Ok(file) => Ok(Some(file)),
        Err(e) if platform::is_busy(&e) => Ok(None),
        #[cfg(windows)]
        // 独占持有者带 DELETE_ON_CLOSE 时，并发的打开可能报"文件不存在"
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(open_error(path, e)),
    }
}

/// Windows 上打开即加锁，Unix 上还要额外 flock 一次
#[cfg(windows)]
fn lock_file(_file: &File, _path: &Path, _shared: bool) -> Result<bool, LauncherError> {
    Ok(true)
}

#[cfg(unix)]
fn lock_file(file: &File, path: &Path, shared: bool) -> Result<bool, LauncherError> {
    use std::fs::TryLockError;

    let result = if shared {
        file.try_lock_shared()
    } else {
        file.try_lock()
    };

    match result {
        Ok(()) => Ok(platform::still_owns_path(file, path)),
        Err(TryLockError::WouldBlock) => Ok(false),
        Err(TryLockError::Error(e)) => Err(open_error(path, e)),
    }
}

fn acquire_with(path: &Path, timeout: Duration, shared: bool) -> Result<FileLock, LauncherError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let deadline = Instant::now() + timeout;
    loop {
        if let Some(file) = try_open(path, shared)?
            && lock_file(&file, path, shared)?
        {
            #[cfg(unix)]
            return Ok(FileLock {
                file,
                path: path.to_path_buf(),
            });
            #[cfg(windows)]
            return Ok(FileLock { file });
        }

        if Instant::now() >= deadline {
            return Err(busy_error(path, timeout));
        }
        std::thread::sleep(RETRY_INTERVAL);
    }
}

async fn acquire_async_with(
    path: &Path,
    timeout: Duration,
    shared: bool,
) -> Result<FileLock, LauncherError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let deadline = Instant::now() + timeout;
    loop {
        if let Some(file) = try_open(path, shared)?
            && lock_file(&file, path, shared)?
        {
            #[cfg(unix)]
            return Ok(FileLock {
                file,
                path: path.to_path_buf(),
            });
            #[cfg(windows)]
            return Ok(FileLock { file });
        }

        if Instant::now() >= deadline {
            return Err(busy_error(path, timeout));
        }
        tokio::time::sleep(RETRY_INTERVAL).await;
    }
}

/// 同步获取排他锁；配置写入这类短临界区用它
pub fn acquire(path: &Path, timeout: Duration) -> Result<FileLock, LauncherError> {
    acquire_with(path, timeout, false)
}

/// 异步获取排他锁；下载、token 刷新这类可能等较久的场景用它，避免阻塞 worker
pub async fn acquire_async(path: &Path, timeout: Duration) -> Result<FileLock, LauncherError> {
    acquire_async_with(path, timeout, false).await
}

/// 共享锁：多个持有者可以同时存在，排他锁必须等它们全部释放
pub async fn acquire_shared_async(
    path: &Path,
    timeout: Duration,
) -> Result<FileLock, LauncherError> {
    acquire_async_with(path, timeout, true).await
}

/// 同步共享锁；测试和需要短等待的场景用它
pub fn acquire_shared(path: &Path, timeout: Duration) -> Result<FileLock, LauncherError> {
    acquire_with(path, timeout, true)
}

/// 只用于旧版本遗留或异常中断留下的空壳。判断依据是"能否独占获取"：
/// 抢得到就说明确实没人持有，随即 drop 让内核删除；抢不到说明正被使用，必须原样保留。
///
/// 返回 true 表示文件已被清掉（或本来就不存在）
pub fn remove_if_unused(path: &Path) -> bool {
    if !path.exists() {
        return true;
    }
    match acquire(path, Duration::from_millis(20)) {
        Ok(lock) => {
            // 显式 drop 触发内核删除（Windows）或 unlink（Unix）
            drop(lock);
            true
        }
        Err(LauncherError::Busy(_)) => false,
        Err(_) => false,
    }
}

/// 批量清理目录下指定后缀的残留锁文件
pub fn remove_unused_in_dir(dir: &Path, suffix: &str) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.ends_with(suffix) {
            continue;
        }
        if remove_if_unused(&entry.path()) {
            log::debug!("已清理残留锁文件: {}", name);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{acquire, acquire_async, acquire_shared};
    use std::time::Duration;

    /// 多个共享持有者可以共存，且**任何一个释放都不能删掉别人的锁文件**
    ///
    /// 后半句是 Unix 特有的陷阱：锁挂在 inode 上，第一个持有者一 unlink，
    /// 其余持有者锁着的就变成一个已删除的 inode，之后新来的排他锁会在
    /// 新建的空文件上成功——正在跑的游戏会被当成"没人在跑"。
    /// 这条测试在 Linux CI 上跑才有意义，Windows 靠 DELETE_ON_CLOSE 天然成立
    #[test]
    fn test_multiple_shared_locks_can_coexist() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("run.lock");

        let first = acquire_shared(&path, Duration::from_millis(50)).unwrap();
        let second = acquire_shared(&path, Duration::from_millis(50)).unwrap();

        // 运行中的实例不能删除/重命名
        assert!(acquire(&path, Duration::from_millis(50)).is_err());

        drop(first);
        // 还有共享持有者，锁文件必须继续存在，否则新来的排他锁会以为没人用
        assert!(path.exists(), "还有持有者时锁文件不能消失");
        assert!(
            acquire(&path, Duration::from_millis(50)).is_err(),
            "残留的共享持有者必须继续挡住排他锁"
        );

        drop(second);
        assert!(!path.exists(), "最后一个持有者释放后锁文件必须消失");
        assert!(acquire(&path, Duration::from_millis(50)).is_ok());
    }

    #[test]
    fn test_shared_lock_cannot_bypass_exclusive_holder() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("run.lock");

        let exclusive = acquire(&path, Duration::from_millis(50)).unwrap();
        assert!(acquire_shared(&path, Duration::from_millis(50)).is_err());

        drop(exclusive);
        assert!(!path.exists(), "释放后不留锁文件");
        assert!(acquire_shared(&path, Duration::from_millis(50)).is_ok());
    }

    #[test]
    fn test_exclusive_lock_released_on_drop() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.lock");

        let first = acquire(&path, Duration::from_millis(50)).unwrap();
        let err = acquire(&path, Duration::from_millis(50)).unwrap_err();
        assert!(err.to_string().contains("另一个启动器实例"), "{err}");

        drop(first);
        assert!(!path.exists(), "释放后不留锁文件");
        assert!(acquire(&path, Duration::from_millis(50)).is_ok());
    }

    #[test]
    fn test_release_then_reacquire_leaves_no_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("repeat.lock");

        // 反复获取/释放不能攒下文件，也不能因为上次的文件残留而失败
        for _ in 0..5 {
            let lock = acquire(&path, Duration::from_millis(50)).unwrap();
            assert!(path.exists());
            drop(lock);
            assert!(!path.exists());
        }
    }

    #[tokio::test]
    async fn test_async_lock_waits_for_release() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.lock");

        let first = acquire(&path, Duration::from_millis(50)).unwrap();
        assert!(
            acquire_async(&path, Duration::from_millis(50))
                .await
                .is_err()
        );

        drop(first);
        assert!(
            acquire_async(&path, Duration::from_millis(50))
                .await
                .is_ok()
        );
    }

    #[test]
    fn test_lock_creates_missing_parent_directory() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("nested/deeper/test.lock");

        let lock = acquire(&path, Duration::from_millis(50)).unwrap();
        assert!(path.exists());
        drop(lock);
        assert!(!path.exists());
        // 目录本身保留，避免每次加锁都重建
        assert!(dir.path().join("nested/deeper").is_dir());
    }

    #[test]
    fn test_remove_if_unused_clears_orphan_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("orphan.lock");
        std::fs::write(&path, b"").unwrap();

        assert!(super::remove_if_unused(&path), "无人持有的锁应被清掉");
        assert!(!path.exists());
    }

    #[test]
    fn test_remove_if_unused_keeps_a_held_lock() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("held.lock");
        let held = acquire_shared(&path, Duration::from_millis(50)).unwrap();

        assert!(!super::remove_if_unused(&path), "正在使用的锁不能删");
        assert!(path.exists());

        drop(held);
        assert!(!path.exists());
    }

    #[test]
    fn test_remove_if_unused_accepts_a_missing_file() {
        let dir = tempfile::TempDir::new().unwrap();
        assert!(super::remove_if_unused(&dir.path().join("nope.lock")));
    }

    #[test]
    fn test_remove_unused_in_dir_only_touches_matching_suffix() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("a.lock"), b"").unwrap();
        std::fs::write(dir.path().join("b.lock"), b"").unwrap();
        std::fs::write(dir.path().join("keep.json"), b"").unwrap();

        let held = acquire_shared(&dir.path().join("b.lock"), Duration::from_millis(50)).unwrap();
        super::remove_unused_in_dir(dir.path(), ".lock");

        assert!(!dir.path().join("a.lock").exists());
        assert!(dir.path().join("b.lock").exists(), "被持有的锁要保留");
        assert!(dir.path().join("keep.json").exists(), "其他文件不受影响");

        drop(held);
    }
}
