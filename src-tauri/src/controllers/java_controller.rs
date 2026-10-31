//! Java command 模块
//!
//! 本机 Java 扫描与自定义路径管理的 invoke 入口，兼容性判定前先把实例 ID 解析成真实 MC 版本

use crate::errors::LauncherError;
use crate::models::{JavaCompatibilityResult, JavaDownloadOption, JavaDownloadProgress};
use crate::services::config::load_config;
use crate::services::java_runtime;
use crate::services::launcher::java;
use crate::services::launcher::java_requirement;

#[tauri::command]
pub async fn find_java_installations_command() -> Result<Vec<String>, LauncherError> {
    crate::services::java::find_java_installations_command().await
}

#[tauri::command]
pub async fn refresh_java_installations() -> Result<Vec<String>, LauncherError> {
    crate::services::java::refresh_java_installations().await
}

#[tauri::command]
pub async fn set_java_path_command(path: String) -> Result<(), LauncherError> {
    crate::services::java::set_java_path_command(path).await
}

#[tauri::command]
pub async fn validate_java_path(path: String) -> Result<bool, LauncherError> {
    crate::services::java::validate_java_path(path).await
}

#[tauri::command]
pub async fn get_java_version(path: String) -> Result<String, LauncherError> {
    crate::services::java::get_java_version(path).await
}

#[tauri::command]
pub fn get_custom_java_paths() -> Vec<String> {
    crate::services::config::get_custom_java_paths()
}

#[tauri::command]
pub async fn add_custom_java_path(path: String) -> Result<(), LauncherError> {
    crate::controllers::run_blocking(move || crate::services::config::add_custom_java_path(&path))
        .await
}

#[tauri::command]
pub async fn remove_custom_java_path(path: String) -> Result<(), LauncherError> {
    crate::controllers::run_blocking(move || {
        crate::services::config::remove_custom_java_path(&path)
    })
    .await
}

#[tauri::command]
pub async fn check_java_compatibility(
    mc_version: String,
) -> Result<JavaCompatibilityResult, LauncherError> {
    tauri::async_runtime::spawn_blocking(move || {
        crate::utils::file_utils::validate_version_id_or_error(&mc_version)?;
        let config = load_config()?;

        // 传进来的可能是实例 ID，判据自己会沿 inheritsFrom 找到声明
        let game_dir = java::resolve_game_dir(&config.game_dir);
        let profile = java::profile_for(&mc_version, &game_dir);
        let req = &profile.requirement;
        log::info!(
            "检查 Java 兼容性: {} → {} 实例，要求 {}{}（优先 {}）",
            mc_version,
            profile.loader.label(),
            req.min,
            match req.hard_max {
                Some(max) => format!("~{}", max),
                None => "+".to_string(),
            },
            req.target
        );

        let instance_settings =
            crate::services::instance_settings::load(&mc_version).unwrap_or_default();
        let instance_java_path = instance_settings.java_path.clone();
        let current_java_path = instance_java_path
            .clone()
            .or_else(|| config.java_path.clone());
        let auto_match_enabled = config.auto_match_java && instance_settings.java_path.is_none();
        let current_java_version = current_java_path
            .as_deref()
            .and_then(java::detect_java_version);

        // 实例级固定 Java 是用户的明确选择，不再当作硬阻断；启动闸门也只警告不替换。
        // 探测不到版本时不放行，避免路径填错后连提示都没有
        let explicit_java = instance_java_path.is_some() && current_java_version.is_some();
        let allowed = current_java_version.is_some_and(|v| java_requirement::java_allowed(req, v));
        let compatible = !profile.resolved || allowed || explicit_java;
        let hard_block =
            !explicit_java && profile.resolved && current_java_version.is_some() && !allowed;

        if explicit_java && !allowed {
            log::warn!(
                "实例 {} 固定使用 Java {}，不满足判据（要求 Java {}{}）；按用户选择继续",
                mc_version,
                current_java_version.unwrap_or(0),
                req.min,
                match req.hard_max {
                    Some(max) => format!("~{}", max),
                    None => "+".to_string(),
                }
            );
        }

        let candidates = java::collect_candidates(&config);
        let match_result = java::choose_java(req, &candidates);
        let recommended_java_path = match_result.as_ref().map(|r| r.path.clone());
        let recommended_java_version = match_result.as_ref().map(|r| r.version);
        let recommended_java_optimal = match_result.as_ref().is_some_and(|r| r.is_optimal);
        let recommended_java_warning = match_result.as_ref().and_then(|r| r.warning.clone());

        let diagnostic = if hard_block {
            java_requirement::build_report(
                &profile,
                current_java_path.as_deref().zip(current_java_version),
                &candidates,
            )
        } else {
            String::new()
        };

        Ok(JavaCompatibilityResult {
            compatible,
            hard_block,
            diagnostic,
            current_java_version,
            required_java_version: req.min,
            recommended_min_java_version: req.min,
            recommended_max_java_version: req.hard_max.unwrap_or(java::NO_UPPER_BOUND),
            current_java_path,
            recommended_java_path,
            recommended_java_version,
            recommended_java_optimal,
            recommended_java_warning,
            auto_match_enabled,
        })
    })
    .await
    .map_err(|e| LauncherError::Custom(format!("后台任务执行失败: {}", e)))?
}

#[tauri::command]
pub async fn get_java_download_options() -> Result<Vec<JavaDownloadOption>, LauncherError> {
    java_runtime::options().await
}

#[tauri::command]
pub fn get_java_download_state() -> Option<JavaDownloadProgress> {
    java_runtime::current_progress()
}

#[tauri::command]
pub async fn download_java_runtime(
    app: tauri::AppHandle,
    feature: u32,
) -> Result<(), LauncherError> {
    java_runtime::download(app, feature).await
}

#[tauri::command]
pub fn cancel_java_download() {
    java_runtime::cancel();
}

#[tauri::command]
pub async fn delete_java_runtime(feature: u32) -> Result<(), LauncherError> {
    java_runtime::delete(feature).await
}
