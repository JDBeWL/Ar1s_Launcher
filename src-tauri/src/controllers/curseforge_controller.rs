//! CurseForge command 模块
//!
//! 麦块开放 API 中转的模组与整合包搜索、版本列表、安装入口；
//! 响应结构与 Modrinth 命令同构，前端按平台选择调用目标

use crate::errors::LauncherError;
use crate::models::modpack::{
    ModpackInstallOptions, ModrinthModpackVersion, ModrinthSearchResponse,
};
use crate::services::curseforge::{
    CfCategory, CfManualFile, CfProjectRef, CfResource, CurseForgeService,
};
use crate::services::mod_installer::{self, ModInstallResult};
use crate::services::modpack_installer;

/// CurseForge 整合包的分类标签；与 Modrinth 的 taxonomy 完全不同，前端按源分别取
#[tauri::command]
pub async fn get_curseforge_modpack_categories() -> Result<Vec<CfCategory>, LauncherError> {
    CurseForgeService::new()
        .get_categories(CfResource::Modpacks)
        .await
}

/// 把 CurseForge 链接 / slug / 数字 ID 解析成项目引用。
/// 搜索库未收录的项目（新包常见）也能借此拿到数字 ID 直接进版本列表
#[tauri::command]
pub async fn resolve_curseforge_modpack(input: String) -> Result<CfProjectRef, LauncherError> {
    CurseForgeService::new()
        .resolve_project(CfResource::Modpacks, &input)
        .await
}

#[tauri::command]
pub async fn search_curseforge_mods(
    instance_name: String,
    query: Option<String>,
    limit: Option<u32>,
    offset: Option<u32>,
    sort_by: Option<String>,
) -> Result<ModrinthSearchResponse, LauncherError> {
    mod_installer::get_installer()
        .search_cf_mods(&instance_name, query, limit, offset, sort_by)
        .await
}

#[tauri::command]
pub async fn get_curseforge_mod_versions(
    instance_name: String,
    project_id: String,
) -> Result<Vec<ModrinthModpackVersion>, LauncherError> {
    mod_installer::get_installer()
        .get_cf_mod_versions(&instance_name, &project_id)
        .await
}

#[tauri::command]
pub async fn install_curseforge_mod(
    instance_name: String,
    project_id: String,
    file_id: Option<String>,
) -> Result<ModInstallResult, LauncherError> {
    mod_installer::get_installer()
        .install_cf_mod(instance_name, project_id, file_id)
        .await
}

#[tauri::command]
pub async fn search_curseforge_modpacks(
    query: Option<String>,
    game_versions: Option<Vec<String>>,
    loaders: Option<Vec<String>>,
    categories: Option<Vec<String>>,
    limit: Option<u32>,
    offset: Option<u32>,
    sort_by: Option<String>,
) -> Result<ModrinthSearchResponse, LauncherError> {
    modpack_installer::get_installer()
        .search_cf_modpacks(
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
pub async fn get_curseforge_modpack_versions(
    project_id: String,
) -> Result<Vec<ModrinthModpackVersion>, LauncherError> {
    modpack_installer::get_installer()
        .get_cf_modpack_versions(&project_id)
        .await
}

/// 返回值是需手动下载的受限文件清单；安装本体成功时清单可以为空
#[tauri::command]
pub async fn install_curseforge_modpack(
    options: ModpackInstallOptions,
    window: tauri::Window,
) -> Result<Vec<CfManualFile>, LauncherError> {
    modpack_installer::get_installer()
        .install_curseforge_modpack(options, &window)
        .await
}
