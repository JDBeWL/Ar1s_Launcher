//! 日志中心 command 模块

use crate::errors::LauncherError;
use crate::models::{ContentTarget, CrashReportInfo, CrashReportText, LaunchRecord, LogTail};
use crate::services::instance_logs;

use super::run_blocking;

#[tauri::command]
pub async fn get_launch_history(instance_name: String) -> Result<Vec<LaunchRecord>, LauncherError> {
    run_blocking(move || instance_logs::list_history(&instance_name)).await
}

#[tauri::command]
pub async fn get_latest_log(instance_name: String) -> Result<LogTail, LauncherError> {
    run_blocking(move || instance_logs::read_latest_log(&instance_name)).await
}

#[tauri::command]
pub async fn get_launch_log(
    instance_name: String,
    launch_id: i64,
) -> Result<LogTail, LauncherError> {
    run_blocking(move || instance_logs::read_launch_log(&instance_name, launch_id)).await
}

#[tauri::command]
pub async fn get_crash_reports(
    instance_name: String,
) -> Result<Vec<CrashReportInfo>, LauncherError> {
    run_blocking(move || instance_logs::list_crash_reports(&instance_name)).await
}

#[tauri::command]
pub async fn get_crash_report(
    instance_name: String,
    file_name: String,
) -> Result<CrashReportText, LauncherError> {
    run_blocking(move || instance_logs::read_crash_report(&instance_name, &file_name)).await
}

#[tauri::command]
pub async fn open_instance_content(
    instance_name: String,
    target: ContentTarget,
) -> Result<(), LauncherError> {
    run_blocking(move || instance_logs::open_content(&instance_name, target)).await
}
