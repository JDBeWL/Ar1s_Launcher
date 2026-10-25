//! 截图 command 模块

use tauri::Manager;

use crate::errors::LauncherError;
use crate::models::ScreenshotInfo;
use crate::services::screenshots;

use super::run_blocking;

#[tauri::command]
pub async fn get_instance_screenshots(
    app: tauri::AppHandle,
    instance_name: String,
) -> Result<Vec<ScreenshotInfo>, LauncherError> {
    run_blocking(move || {
        let dir = screenshots::screenshot_dir(&instance_name)?;
        if dir.is_dir() {
            app.asset_protocol_scope()
                .allow_directory(&dir, false)
                .map_err(|error| LauncherError::Custom(format!("无法授权截图目录: {error}")))?;
        }
        screenshots::list_screenshots_in(&dir)
    })
    .await
}

#[tauri::command]
pub async fn copy_instance_screenshot(
    instance_name: String,
    file_name: String,
) -> Result<(), LauncherError> {
    run_blocking(move || screenshots::copy_screenshot(&instance_name, &file_name)).await
}

#[tauri::command]
pub async fn open_screenshots_folder(instance_name: String) -> Result<(), LauncherError> {
    run_blocking(move || screenshots::open_screenshots_folder(&instance_name)).await
}
