//! 存档 command 模块

use crate::errors::LauncherError;
use crate::models::{SaveBackupInfo, SaveImportSummary, SaveInfo};
use crate::services::saves;

use super::run_blocking;

#[tauri::command]
pub async fn get_instance_saves(instance_name: String) -> Result<Vec<SaveInfo>, LauncherError> {
    run_blocking(move || saves::list_saves(&instance_name)).await
}

#[tauri::command]
pub async fn get_save_backups(instance_name: String) -> Result<Vec<SaveBackupInfo>, LauncherError> {
    run_blocking(move || saves::list_backups(&instance_name)).await
}

#[tauri::command]
pub async fn backup_save(
    instance_name: String,
    world: String,
) -> Result<SaveBackupInfo, LauncherError> {
    saves::backup_save(&instance_name, &world).await
}

#[tauri::command]
pub async fn restore_save_backup(
    instance_name: String,
    world: String,
    file_name: String,
) -> Result<(), LauncherError> {
    saves::restore_backup(&instance_name, &world, &file_name).await
}

#[tauri::command]
pub async fn delete_save_backup(
    instance_name: String,
    world: String,
    file_name: String,
) -> Result<(), LauncherError> {
    run_blocking(move || saves::delete_backup(&instance_name, &world, &file_name)).await
}

#[tauri::command]
pub async fn import_saves(
    instance_name: String,
    source_paths: Vec<String>,
) -> Result<SaveImportSummary, LauncherError> {
    saves::import_saves(&instance_name, &source_paths).await
}

#[tauri::command]
pub async fn open_saves_folder(instance_name: String) -> Result<(), LauncherError> {
    run_blocking(move || {
        let config = crate::services::config::load_config()?;
        let game_dir = std::path::PathBuf::from(&config.game_dir);
        let saves_dir =
            crate::services::launcher::isolation::content_root(&config, &game_dir, &instance_name)
                .join("saves");
        std::fs::create_dir_all(&saves_dir)?;
        opener::open(&saves_dir)
            .map_err(|error| LauncherError::Custom(format!("无法打开存档目录: {}", error)))
    })
    .await
}
