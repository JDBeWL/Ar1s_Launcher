//! 整合包 command 模块
//!
//! Modrinth 搜索、版本列表、在线安装与取消，以及本地 .mrpack 的预览、导入与实例导出

use crate::errors::LauncherError;
use crate::models::modpack::*;
use crate::services::curseforge::CfManualFile;
use crate::services::modpack_exporter;
use crate::services::modpack_installer::{self, LocalModpackPreview};

#[tauri::command]
pub async fn search_modrinth_modpacks(
    query: Option<String>,
    game_versions: Option<Vec<String>>,
    loaders: Option<Vec<String>>,
    categories: Option<Vec<String>>,
    limit: Option<u32>,
    offset: Option<u32>,
    sort_by: Option<String>,
) -> Result<ModrinthSearchResponse, LauncherError> {
    modpack_installer::get_installer()
        .search_modpacks(
            query,
            game_versions,
            loaders,
            categories,
            limit,
            offset,
            sort_by,
        )
        .await
}

#[tauri::command]
pub async fn get_modrinth_modpack_versions(
    project_id: String,
    game_versions: Option<Vec<String>>,
    loaders: Option<Vec<String>>,
) -> Result<Vec<ModrinthModpackVersion>, LauncherError> {
    modpack_installer::get_installer()
        .get_modpack_versions(&project_id, game_versions, loaders)
        .await
}

#[tauri::command]
pub async fn install_modrinth_modpack(
    options: ModpackInstallOptions,
    window: tauri::Window,
) -> Result<(), LauncherError> {
    modpack_installer::get_installer()
        .install_modrinth_modpack(options, &window)
        .await
}

/// 读取本地 .mrpack 的清单，安装页在用户选完文件后先展示这些信息
#[tauri::command]
pub async fn preview_local_modpack(
    file_path: String,
) -> Result<LocalModpackPreview, LauncherError> {
    modpack_installer::get_installer()
        .preview_local_modpack(file_path)
        .await
}

#[tauri::command]
pub async fn install_local_modpack(
    options: LocalModpackInstallOptions,
    window: tauri::Window,
) -> Result<Vec<CfManualFile>, LauncherError> {
    modpack_installer::get_installer()
        .install_local_modpack(options, &window)
        .await
}

#[tauri::command]
pub async fn export_modpack(
    instance_name: String,
    options: ModpackExportOptions,
) -> Result<ModpackExportResult, LauncherError> {
    modpack_exporter::export_modpack(instance_name, options).await
}

#[tauri::command]
pub fn cancel_modpack_install() -> Result<(), LauncherError> {
    modpack_installer::set_modpack_cancel_flag();
    Ok(())
}
