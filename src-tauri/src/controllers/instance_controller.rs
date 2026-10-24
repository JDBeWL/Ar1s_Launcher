//! 实例 command 模块
//!
//! 实例增删改查、启动与实例内 Mod 管理的 invoke 入口

use crate::errors::LauncherError;
use crate::models::InstanceInfo;
use crate::models::InstanceSettings;
use crate::services::instance::{self, ModInfo};
use crate::services::instance_settings;
use crate::services::loaders::LoaderType;
use crate::utils::file_utils::{InstanceNameValidation, validate_instance_name};

use super::run_blocking;

#[tauri::command]
pub fn validate_instance_name_cmd(name: String) -> InstanceNameValidation {
    validate_instance_name(&name)
}

/// 除格式外还要判断同名实例是否已存在
#[tauri::command]
pub fn check_instance_name_available(name: String) -> InstanceNameValidation {
    instance::check_instance_name_available(&name)
}

#[tauri::command]
pub async fn create_instance(
    new_instance_name: String,
    base_version_id: String,
    loader: Option<LoaderType>,
    window: tauri::Window,
) -> Result<(), LauncherError> {
    instance::create_instance(new_instance_name, base_version_id, loader, &window).await
}

#[tauri::command]
pub async fn get_instances() -> Result<Vec<InstanceInfo>, LauncherError> {
    instance::get_instances().await
}

#[tauri::command]
pub async fn delete_instance(instance_name: String) -> Result<(), LauncherError> {
    instance::delete_instance(instance_name).await
}

#[tauri::command]
pub async fn rename_instance(old_name: String, new_name: String) -> Result<(), LauncherError> {
    instance::rename_instance(old_name, new_name).await
}

#[tauri::command]
pub async fn open_instance_folder(instance_name: String) -> Result<(), LauncherError> {
    instance::open_instance_folder(instance_name).await
}

#[tauri::command]
pub async fn launch_instance(
    instance_name: String,
    override_java_path: Option<String>,
    force_java_mismatch: Option<bool>,
    window: tauri::Window,
) -> Result<(), LauncherError> {
    instance::launch_instance(
        instance_name,
        override_java_path,
        force_java_mismatch.unwrap_or(false),
        window,
    )
    .await
}

#[tauri::command]
pub fn get_instance_settings(instance_name: String) -> Result<InstanceSettings, LauncherError> {
    instance_settings::load(&instance_name)
}

#[tauri::command]
pub async fn set_instance_settings(
    instance_name: String,
    settings: InstanceSettings,
) -> Result<(), LauncherError> {
    run_blocking(move || instance_settings::save(&instance_name, settings)).await
}

/// 列出实例模组：每个 jar 都要打开读 fabric.mod.json / mods.toml，几百个模组时
/// 留在 async worker 上会把同一时间的其他命令一起拖住
#[tauri::command]
pub async fn get_instance_mods(instance_name: String) -> Result<Vec<ModInfo>, LauncherError> {
    run_blocking(move || instance::get_instance_mods(instance_name)).await
}

#[tauri::command]
pub async fn set_mod_enabled(
    instance_name: String,
    file_name: String,
    enabled: bool,
) -> Result<(), LauncherError> {
    instance::set_mod_enabled(instance_name, file_name, enabled).await
}

#[tauri::command]
pub async fn delete_instance_mod(
    instance_name: String,
    file_name: String,
) -> Result<(), LauncherError> {
    instance::delete_instance_mod(instance_name, file_name).await
}
