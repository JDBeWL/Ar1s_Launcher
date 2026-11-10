//! 维护：迁移旧版本留下的状态、清理异常中断留下的运行锁

use super::lock_dir;
use log::{info, warn};
use std::fs;
use std::path::Path;

/// 只兜底清理旧版本或异常中断留下的空壳，判断依据见 `remove_if_unused`
///
/// 不能直接 `remove_file`：共享锁允许 FILE_SHARE_DELETE，直接删会把正在运行的游戏
/// 的锁文件删掉，新来的排他锁就会误判"没有实例在跑"
pub fn prune_stale_run_locks(game_dir: &Path, versions_dir: &Path) {
    let Ok(entries) = fs::read_dir(lock_dir(game_dir)) else {
        return;
    };

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(instance) = name
            .strip_prefix("run-")
            .and_then(|rest| rest.strip_suffix(".lock"))
        else {
            continue;
        };
        // 实例目录还在就说明可能真在运行，交给持有者自己清理
        if versions_dir.join(instance).is_dir() {
            continue;
        }
        if crate::utils::file_lock::remove_if_unused(&entry.path()) {
            info!("清理残留的运行锁: {}", name);
        }
    }
}

/// 旧版本把下载状态和锁放在 `.download_state/`、natives 锁放在版本目录里。
/// 状态文件关系到断点续传，必须搬过来；旧锁文件直接删（进程退出后本来就没有意义）
pub fn migrate_legacy_state(game_dir: &Path, versions_dir: &Path) {
    let lock_root = lock_dir(game_dir);
    let legacy_state = game_dir.join(".download_state");

    if legacy_state.is_dir() {
        let new_state = lock_root.join("download_state");
        if let Err(e) = fs::create_dir_all(&new_state) {
            warn!("创建下载状态目录失败: {}", e);
        } else if let Ok(entries) = fs::read_dir(&legacy_state) {
            for entry in entries.flatten() {
                let path = entry.path();
                let name = entry.file_name();
                // 只搬续传状态，旧锁文件留着没有用
                let is_lock = path.extension().is_some_and(|ext| ext == "lock");
                let target = new_state.join(&name);
                if !is_lock && path.is_file() && !target.exists() {
                    let _ = fs::rename(&path, &target);
                }
            }
        }
        // 只剩空目录或旧锁文件时整个删掉
        let _ = fs::remove_dir_all(&legacy_state);
    }

    // 每个实例目录下的 .natives.lock 也搬到统一锁目录
    if let Ok(entries) = fs::read_dir(versions_dir) {
        for entry in entries.flatten() {
            let stale = entry.path().join(".natives.lock");
            if stale.exists() {
                let _ = fs::remove_file(&stale);
            }
        }
    }
}

/// 已经启动过一次就不再重复扫描；迁移只在启动时做一次
pub fn migrate_legacy_state_once(game_dir: &Path, versions_dir: &Path) {
    static MIGRATED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if MIGRATED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    migrate_legacy_state(game_dir, versions_dir);
}
