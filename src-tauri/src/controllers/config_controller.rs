//! 配置 command 模块
//!
//! 设置页各项配置的 invoke 入口，含游戏目录、内存、JVM 参数预览与版本文件校验触发

use crate::errors::LauncherError;
use crate::models::*;
use crate::services::config;

use super::run_blocking;

#[tauri::command]
pub fn get_config() -> Result<GameConfig, LauncherError> {
    let mut config = (*config::load_config()?).clone();
    // token 与配置同存一个文件，但界面不需要；要登录态就查 get_auth_status
    config.ms_access_token = None;
    config.ms_refresh_token = None;
    Ok(config)
}

#[tauri::command]
pub fn load_config_key(key: String) -> Result<Option<String>, LauncherError> {
    config::load_config_key(key)
}

#[tauri::command]
pub async fn save_config_key(key: String, value: String) -> Result<(), LauncherError> {
    run_blocking(move || config::save_config_key(key, value)).await
}

#[tauri::command]
pub fn get_game_dir() -> Result<String, LauncherError> {
    config::get_game_dir()
}

#[tauri::command]
pub async fn get_game_dir_info() -> Result<GameDirInfo, LauncherError> {
    run_blocking(config::get_game_dir_info).await
}

#[tauri::command]
pub async fn set_game_dir(path: String, window: tauri::Window) -> Result<(), LauncherError> {
    run_blocking(move || config::set_game_dir(path, &window)).await
}

#[tauri::command]
pub fn get_download_threads() -> Result<u16, LauncherError> {
    config::get_download_threads()
}

#[tauri::command]
pub async fn set_download_threads(threads: u16) -> Result<(), LauncherError> {
    run_blocking(move || config::set_download_threads(threads)).await
}

#[tauri::command]
pub fn get_proxy_settings() -> Result<ProxySettings, LauncherError> {
    Ok(crate::services::proxy::current_settings())
}

#[tauri::command]
pub async fn set_proxy_settings(settings: ProxySettings) -> Result<ProxySettings, LauncherError> {
    run_blocking(move || crate::services::proxy::save_settings(settings)).await
}

#[tauri::command]
pub async fn test_proxy(settings: ProxySettings) -> Result<ProxyTestResult, LauncherError> {
    crate::services::proxy::test_connection(settings).await
}

/// 逐个检查版本 JSON、客户端 jar 与几百个库文件是否存在，全是同步 stat
#[tauri::command]
pub async fn validate_version_files(version_id: String) -> Result<Vec<String>, LauncherError> {
    run_blocking(move || {
        crate::services::file_verification::validate_version_files_sync(version_id)
    })
    .await
}

#[tauri::command]
pub async fn get_memory_stats() -> Result<crate::services::memory::MemoryStats, LauncherError> {
    run_blocking(|| Ok(crate::services::memory::get_system_memory())).await
}

#[tauri::command]
pub async fn check_memory_warning(memory_mb: u32) -> Result<Option<String>, LauncherError> {
    run_blocking(move || config::check_memory_warning(memory_mb)).await
}

#[tauri::command]
pub fn get_jvm_advanced_options() -> Result<JvmAdvancedOptions, LauncherError> {
    Ok(config::load_config()?.jvm_advanced.clone())
}

#[tauri::command]
pub async fn set_jvm_advanced_options(options: JvmAdvancedOptions) -> Result<(), LauncherError> {
    run_blocking(move || {
        config::update_config(|cfg| {
            cfg.jvm_advanced = options;
            Ok(())
        })
    })
    .await
}

#[tauri::command]
pub async fn get_effective_jvm_args(memory_mb: Option<u32>) -> Result<Vec<String>, LauncherError> {
    run_blocking(move || {
        let cfg = config::load_config()?;
        let memory = memory_mb.unwrap_or(cfg.max_memory);

        // Java 路径可能还没配置，拿不到版本就用默认值
        let java_version = crate::services::launcher::java::resolve_java_path(&cfg)
            .ok()
            .and_then(|path| crate::services::launcher::java::detect_java_version(&path));

        Ok(crate::services::memory::optimize_jvm_memory_args(
            memory,
            java_version,
            &cfg.jvm_advanced,
        ))
    })
    .await
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct WindowSettings {
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub fullscreen: bool,
}

#[tauri::command]
pub fn get_window_settings() -> Result<WindowSettings, LauncherError> {
    let config = config::load_config()?;
    Ok(WindowSettings {
        width: config.window_width,
        height: config.window_height,
        fullscreen: config.fullscreen,
    })
}

#[tauri::command]
pub async fn set_window_settings(
    width: Option<u32>,
    height: Option<u32>,
    fullscreen: bool,
) -> Result<(), LauncherError> {
    run_blocking(move || {
        config::update_config(|config| {
            config.window_width = width;
            config.window_height = height;
            config.fullscreen = fullscreen;
            Ok(())
        })
    })
    .await
}

#[tauri::command]
pub fn get_last_selected_version() -> Option<String> {
    config::get_last_selected_version()
}

#[tauri::command]
pub async fn set_last_selected_version(version: String) -> Result<(), LauncherError> {
    run_blocking(move || config::set_last_selected_version(&version)).await
}
