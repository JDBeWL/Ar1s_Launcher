//! 实例管理模块
//!
//! 创建、重命名、删除实例，从版本 JSON 逐级推断加载器与游戏版本，兼管 mods 目录的 Mod 启停。
//!
//! 拆成几块，共享的只有路径与运行锁：
//! - [`create`]：按版本 JSON 装出一个新实例
//! - [`scan`]：扫描 versions 目录并推断加载器/版本
//! - [`descriptor`]：解析版本 JSON 里的 loader 声明
//! - [`mods`]：mods 目录的读取、启停与元数据解析
//! - [`lifecycle`]：删除/重命名/打开目录/启动
//! - [`maintenance`]：迁移旧状态、清理残留运行锁

mod create;
mod descriptor;
mod lifecycle;
mod maintenance;
mod mods;
mod scan;

#[cfg(test)]
mod tests;

pub use create::create_instance;
pub use lifecycle::{delete_instance, launch_instance, open_instance_folder, rename_instance};
pub use maintenance::{migrate_legacy_state, migrate_legacy_state_once, prune_stale_run_locks};
pub use mods::{ModInfo, delete_instance_mod, get_instance_mods, set_mod_enabled};
pub use scan::{get_instance_info, get_instances};

use crate::errors::LauncherError;
use crate::services::config;
use crate::utils::file_utils::{InstanceNameValidation, validate_instance_name};
use std::path::{Path, PathBuf};
use std::time::Duration;

const INSTANCE_LOCK_TIMEOUT: Duration = Duration::from_secs(2);

fn get_dirs() -> Result<(PathBuf, PathBuf), LauncherError> {
    let config = config::load_config()?;
    let game_dir = PathBuf::from(&config.game_dir);
    let versions_dir = game_dir.join("versions");
    Ok((game_dir, versions_dir))
}

/// 所有跨进程锁都放在游戏目录下的 `.ar1s/`
pub(crate) fn lock_dir(game_dir: &Path) -> PathBuf {
    game_dir.join(".ar1s")
}

/// 实例目录的创建/删除/重命名/整合包安装跨进程串行；嵌套下载另用 download.lock
pub(crate) async fn acquire_instance_lock(
    game_dir: &Path,
) -> Result<crate::utils::file_lock::FileLock, LauncherError> {
    crate::utils::file_lock::acquire_async(
        &lock_dir(game_dir).join("instances.lock"),
        INSTANCE_LOCK_TIMEOUT,
    )
    .await
}

/// 运行锁不能放进实例目录：Windows 上目录里有打开的文件就重命名不了，
/// 而删除/重命名实例必须先拿到这把锁，会自己把自己锁死
fn run_lock_path(game_dir: &Path, instance_name: &str) -> PathBuf {
    lock_dir(game_dir).join(format!("run-{}.lock", instance_name))
}

/// 游戏进程运行期间由监控线程持有；共享锁允许同一实例同时启动多次
pub(crate) async fn acquire_run_shared_lock(
    game_dir: &Path,
    instance_name: &str,
) -> Result<crate::utils::file_lock::FileLock, LauncherError> {
    crate::utils::file_lock::acquire_shared_async(
        &run_lock_path(game_dir, instance_name),
        INSTANCE_LOCK_TIMEOUT,
    )
    .await
}

/// 删除/重命名独占运行锁，确认没有游戏进程还在跑
pub(crate) async fn acquire_run_lock(
    game_dir: &Path,
    instance_name: &str,
) -> Result<crate::utils::file_lock::FileLock, LauncherError> {
    crate::utils::file_lock::acquire_async(
        &run_lock_path(game_dir, instance_name),
        INSTANCE_LOCK_TIMEOUT,
    )
    .await
}

pub fn check_instance_name_available(name: &str) -> InstanceNameValidation {
    let validation = validate_instance_name(name);
    if !validation.is_valid {
        return validation;
    }

    if let Ok((_, versions_dir)) = get_dirs() {
        let instance_dir = versions_dir.join(name);
        if instance_dir.exists() {
            return InstanceNameValidation {
                is_valid: false,
                error_message: Some(format!("名为 '{}' 的实例已存在，请使用其他名称", name)),
            };
        }
    }

    InstanceNameValidation {
        is_valid: true,
        error_message: None,
    }
}
