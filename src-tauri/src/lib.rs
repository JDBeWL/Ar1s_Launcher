//! Ar1s Launcher 的 Tauri 后端
//!
//! crate 根：声明命令层与业务层，并把各 controller 注册成 Tauri command

pub mod controllers;
mod errors;
mod models;
pub mod services;
pub mod utils;
pub use errors::LauncherError;
pub use services::config::{load_config, save_config, update_config, update_config_async};
pub use services::launcher::launch_minecraft;
use utils::logger::setup_logger;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            controllers::download_controller::get_versions,
            controllers::download_controller::download_version,
            controllers::download_controller::cancel_download,
            controllers::config_controller::get_config,
            controllers::config_controller::get_game_dir,
            controllers::config_controller::get_game_dir_info,
            controllers::config_controller::set_game_dir,
            controllers::java_controller::find_java_installations_command,
            controllers::java_controller::refresh_java_installations,
            controllers::java_controller::set_java_path_command,
            controllers::config_controller::load_config_key,
            controllers::config_controller::save_config_key,
            controllers::java_controller::validate_java_path,
            controllers::java_controller::get_java_version,
            controllers::java_controller::get_custom_java_paths,
            controllers::java_controller::add_custom_java_path,
            controllers::java_controller::remove_custom_java_path,
            controllers::java_controller::check_java_compatibility,
            controllers::java_controller::get_java_download_options,
            controllers::java_controller::get_java_download_state,
            controllers::java_controller::download_java_runtime,
            controllers::java_controller::cancel_java_download,
            controllers::java_controller::delete_java_runtime,
            controllers::config_controller::get_download_threads,
            controllers::config_controller::set_download_threads,
            controllers::config_controller::get_proxy_settings,
            controllers::config_controller::set_proxy_settings,
            controllers::config_controller::test_proxy,
            controllers::config_controller::validate_version_files,
            controllers::auth_controller::get_saved_username,
            controllers::auth_controller::set_saved_username,
            controllers::auth_controller::get_saved_uuid,
            controllers::auth_controller::set_saved_uuid,
            controllers::auth_controller::get_auth_status,
            controllers::auth_controller::start_microsoft_login,
            controllers::auth_controller::complete_microsoft_login,
            controllers::auth_controller::start_microsoft_auth_code_login,
            controllers::auth_controller::complete_microsoft_auth_code_login,
            controllers::auth_controller::cancel_microsoft_login,
            controllers::auth_controller::refresh_microsoft_auth,
            controllers::auth_controller::refresh_skin_url,
            controllers::auth_controller::logout_microsoft,
            controllers::auth_controller::set_auth_type,
            controllers::config_controller::get_memory_stats,
            controllers::config_controller::check_memory_warning,
            controllers::config_controller::get_window_settings,
            controllers::config_controller::set_window_settings,
            controllers::config_controller::get_jvm_advanced_options,
            controllers::config_controller::set_jvm_advanced_options,
            controllers::config_controller::get_effective_jvm_args,
            controllers::config_controller::get_last_selected_version,
            controllers::config_controller::set_last_selected_version,
            controllers::instance_controller::validate_instance_name_cmd,
            controllers::instance_controller::check_instance_name_available,
            controllers::instance_controller::create_instance,
            controllers::instance_controller::get_instances,
            controllers::instance_controller::delete_instance,
            controllers::instance_controller::rename_instance,
            controllers::instance_controller::open_instance_folder,
            controllers::instance_controller::launch_instance,
            controllers::instance_controller::get_instance_settings,
            controllers::instance_controller::set_instance_settings,
            controllers::instance_controller::get_instance_mods,
            controllers::instance_controller::set_mod_enabled,
            controllers::instance_controller::delete_instance_mod,
            controllers::saves_controller::get_instance_saves,
            controllers::saves_controller::get_save_backups,
            controllers::saves_controller::backup_save,
            controllers::saves_controller::restore_save_backup,
            controllers::saves_controller::delete_save_backup,
            controllers::saves_controller::import_saves,
            controllers::saves_controller::open_saves_folder,
            controllers::logs_controller::get_launch_history,
            controllers::logs_controller::get_latest_log,
            controllers::logs_controller::get_launch_log,
            controllers::logs_controller::get_crash_reports,
            controllers::logs_controller::get_crash_report,
            controllers::logs_controller::open_instance_content,
            controllers::screenshots_controller::get_instance_screenshots,
            controllers::screenshots_controller::copy_instance_screenshot,
            controllers::screenshots_controller::open_screenshots_folder,
            controllers::mod_controller::search_modrinth_mods,
            controllers::mod_controller::get_modrinth_mod_versions,
            controllers::mod_controller::install_modrinth_mod,
            controllers::mod_controller::check_modrinth_mod_updates,
            controllers::mod_controller::update_modrinth_mod,
            controllers::diagnostics_controller::export_diagnostic_bundle,
            controllers::frontend_log_controller::log_frontend_error,
            controllers::loader_controller::get_forge_versions,
            controllers::loader_controller::get_fabric_versions,
            controllers::loader_controller::get_quilt_versions,
            controllers::loader_controller::get_neoforge_versions,
            controllers::loader_controller::get_available_loaders,
            controllers::modpack_controller::search_modrinth_modpacks,
            controllers::modpack_controller::get_modrinth_modpack_versions,
            controllers::modpack_controller::install_modrinth_modpack,
            controllers::modpack_controller::preview_local_modpack,
            controllers::modpack_controller::install_local_modpack,
            controllers::modpack_controller::export_modpack,
            controllers::modpack_controller::cancel_modpack_install,
            controllers::curseforge_controller::search_curseforge_mods,
            controllers::curseforge_controller::get_curseforge_mod_versions,
            controllers::curseforge_controller::install_curseforge_mod,
            controllers::curseforge_controller::search_curseforge_modpacks,
            controllers::curseforge_controller::get_curseforge_modpack_versions,
            controllers::curseforge_controller::get_curseforge_modpack_categories,
            controllers::curseforge_controller::resolve_curseforge_modpack,
            controllers::curseforge_controller::install_curseforge_modpack,
            controllers::resourcepack_controller::get_resource_packs,
            controllers::resourcepack_controller::set_resource_pack_enabled,
            controllers::resourcepack_controller::delete_instance_resource_pack,
            controllers::resourcepack_controller::import_resource_packs,
            controllers::resourcepack_controller::open_resourcepacks_folder,
            controllers::resourcepack_controller::search_modrinth_resource_packs,
            controllers::resourcepack_controller::get_modrinth_resource_pack_versions,
            controllers::resourcepack_controller::install_modrinth_resource_pack,
            controllers::resourcepack_controller::check_modrinth_resource_pack_updates,
            controllers::resourcepack_controller::update_modrinth_resource_pack,
            controllers::shaderpack_controller::get_shader_packs,
            controllers::shaderpack_controller::set_shader_pack,
            controllers::shaderpack_controller::set_shaders_enabled,
            controllers::shaderpack_controller::delete_instance_shader_pack,
            controllers::shaderpack_controller::import_shader_packs,
            controllers::shaderpack_controller::open_shaderpacks_folder,
            controllers::shaderpack_controller::search_modrinth_shader_packs,
            controllers::shaderpack_controller::get_modrinth_shader_pack_versions,
            controllers::shaderpack_controller::install_modrinth_shader_pack,
            controllers::shaderpack_controller::check_modrinth_shader_pack_updates,
            controllers::shaderpack_controller::update_modrinth_shader_pack
        ])
        .setup(|_app| {
            let storage_dir = services::config::init_storage();
            let log_dir = storage_dir.join("logs");
            if let Err(e) = setup_logger(&log_dir) {
                eprintln!("Error setting up logger: {}", e);
            }

            log::info!("[DEBUG] 程序启动");
            log::info!(
                "[DEBUG] Tauri应用初始化完成（存储目录: {}，日志目录: {}）",
                storage_dir.display(),
                log_dir.display()
            );

            if let Err(e) = services::config::preload_config() {
                log::error!("配置预加载失败: {}", e);
            }

            // 旧版本的下载状态与锁散落在别处，迁移一次即可，之后都走 .ar1s/
            if let Ok(config) = services::config::load_config() {
                let game_dir = std::path::PathBuf::from(&config.game_dir);
                services::instance::migrate_legacy_state_once(
                    &game_dir,
                    &game_dir.join("versions"),
                );
                // 校验指纹缓存：命中后二次安装不再整盘重算 sha1
                utils::file_utils::init_verify_cache(&game_dir);
            }

            // 预热 Java 检测缓存，放后台跑以免阻塞启动
            tauri::async_runtime::spawn(async {
                log::info!("后台预热 Java 检测缓存...");
                if let Err(e) = services::java::find_java_installations_command().await {
                    log::warn!("Java 缓存预热失败: {}", e);
                }
            });

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
