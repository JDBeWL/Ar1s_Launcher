//! Tauri command 模块集合
//!
//! 前端 invoke 的后端入口，只做参数接收与 services 层转发

use crate::errors::LauncherError;

/// 把同步重活挪到阻塞线程池
///
/// 非 async 的 `#[tauri::command]` 会在主线程执行，任何读盘、解压、遍历目录都会
/// 卡住整个窗口；这类命令要么写成 `async` 再用本函数包一层，要么只做纯内存操作。
pub(crate) async fn run_blocking<T, F>(task: F) -> Result<T, LauncherError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, LauncherError> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(task)
        .await
        .map_err(|e| LauncherError::Custom(format!("后台任务执行失败: {}", e)))?
}

pub mod auth_controller;
pub mod config_controller;
pub mod curseforge_controller;
pub mod diagnostics_controller;
pub mod download_controller;
pub mod frontend_log_controller;
pub mod instance_controller;
pub mod java_controller;
pub mod loader_controller;
pub mod mod_controller;
pub mod modpack_controller;
pub mod resourcepack_controller;
pub mod shaderpack_controller;
