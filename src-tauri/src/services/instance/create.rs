//! 创建实例：按版本 JSON 落盘、下载依赖、同步加载器

use super::lifecycle::merge_forge_instance_json;
use super::{acquire_instance_lock, get_dirs};
use crate::errors::LauncherError;
use crate::services::{
    config, download,
    loaders::{self, LoaderType},
};
use crate::utils::file_utils::{self, validate_instance_name_or_error};
use log::warn;
use serde::Serialize;
use serde_json::Value;
use std::fs;
use tauri::{Emitter, Window};

#[derive(Clone, Serialize)]
struct InstallProgress {
    progress: u8,
    message: String,
    indeterminate: bool,
}

pub async fn create_instance(
    new_instance_name: String,
    base_version_id: String,
    loader: Option<LoaderType>,
    window: &Window,
) -> Result<(), LauncherError> {
    validate_instance_name_or_error(&new_instance_name)?;

    let (game_dir, versions_dir) = get_dirs()?;
    let _instance_lock = acquire_instance_lock(&game_dir).await?;
    let source_dir = file_utils::version_dir(&versions_dir, &base_version_id)?;
    let dest_dir = file_utils::instance_dir(&versions_dir, &new_instance_name)?;

    let send_progress = |progress: u8, message: &str, indeterminate: bool| {
        let _ = window.emit(
            "instance-install-progress",
            InstallProgress {
                progress,
                message: message.to_string(),
                indeterminate,
            },
        );
    };

    if dest_dir.exists() {
        return Err(LauncherError::localized(
            "instance_exists",
            format!("实例 '{}' 已存在", new_instance_name),
            vec![new_instance_name.clone()],
        ));
    }

    send_progress(5, "检查基础版本...", false);

    // 基础版本目录存在不代表文件齐全；每次先保证 JSON + client jar，
    // 原版库/assets 交给创建完成后的自动补齐
    send_progress(10, "准备基础版本核心...", true);
    let config = config::load_config()?;
    download::ensure_version_core(
        base_version_id.clone(),
        config.download_mirror.clone(),
        window,
    )
    .await?;

    if !source_dir.exists() {
        return Err(LauncherError::Custom(format!(
            "基础版本 '{}' 准备后仍未找到",
            base_version_id
        )));
    }

    let cleanup = || {
        warn!("安装失败，正在清理实例目录: {}", dest_dir.display());
        file_utils::cleanup_instance_creation(&game_dir, &new_instance_name, &base_version_id);
    };

    send_progress(30, "复制基础文件...", false);

    // 递归拷贝整个版本目录（可能几百 MB）：留在 async worker 上会拖住这段时间里的其他命令
    let copy_result = {
        let source_dir = source_dir.clone();
        let dest_dir = dest_dir.clone();
        tokio::task::spawn_blocking(move || file_utils::copy_dir_all(&source_dir, &dest_dir))
            .await
            .map_err(|e| LauncherError::Custom(format!("复制任务异常终止: {}", e)))?
    };
    if let Err(e) = copy_result {
        cleanup();
        return Err(e.into());
    }

    // 基础版本如果本身带实例设置，新实例不应继承
    let _ = fs::remove_file(dest_dir.join(crate::services::instance_settings::SETTINGS_FILE_NAME));

    send_progress(40, "配置实例...", false);

    let old_json_path = dest_dir.join(format!("{}.json", base_version_id));
    let new_json_path = dest_dir.join(format!("{}.json", new_instance_name));
    let old_jar_path = dest_dir.join(format!("{}.jar", base_version_id));
    let new_jar_path = dest_dir.join(format!("{}.jar", new_instance_name));

    if let Err(e) = fs::rename(&old_json_path, &new_json_path) {
        cleanup();
        return Err(LauncherError::Custom(format!("重命名 JSON 失败: {}", e)));
    }

    if old_jar_path.exists()
        && let Err(e) = fs::rename(&old_jar_path, &new_jar_path)
    {
        cleanup();
        return Err(LauncherError::Custom(format!("重命名 JAR 失败: {}", e)));
    }

    let update_json_id = || -> Result<(), LauncherError> {
        let json_str = fs::read_to_string(&new_json_path)?;
        let mut json: Value = serde_json::from_str(&json_str)
            .map_err(|e| LauncherError::Custom(format!("解析 JSON 失败: {}", e)))?;
        json["id"] = Value::String(new_instance_name.clone());
        fs::write(&new_json_path, serde_json::to_string_pretty(&json)?)?;
        Ok(())
    };

    if let Err(e) = update_json_id() {
        cleanup();
        return Err(e);
    }

    if let Some(ref loader_type) = loader {
        send_progress(60, &format!("安装 {} 加载器...", loader_type.name()), true);

        if let Err(e) =
            loaders::install_loader(loader_type, &new_instance_name, &game_dir, None).await
        {
            cleanup();
            return Err(e);
        }

        // 装完就得能直接启动：Fabric/Quilt 只写了一份 profile，库文件只能在这里补
        send_progress(65, "补全加载器文件...", true);
        match fs::read_to_string(&new_json_path)
            .ok()
            .and_then(|content| serde_json::from_str::<Value>(&content).ok())
        {
            Some(json) => {
                if let Err(e) = download::ensure_declared_libraries(&json, window).await {
                    warn!("加载器库文件没补全，首次启动前可能需要「修复实例」: {}", e);
                }
            }
            None => warn!("读不到实例 JSON，跳过加载器库补全"),
        }

        if let LoaderType::Forge {
            mc_version,
            loader_version,
        } = loader_type
        {
            let forge_id_prefix = format!("{}-forge", mc_version);
            let forge_id_exact = format!("{}-forge-{}", mc_version, loader_version);

            let found_forge_id = fs::read_dir(&versions_dir).ok().and_then(|entries| {
                entries
                    .flatten()
                    .filter_map(|e| e.file_name().to_str().map(String::from))
                    .find(|name| name == &forge_id_exact || name.starts_with(&forge_id_prefix))
            });

            if let Some(fid) = found_forge_id {
                let forge_json_path = versions_dir.join(&fid).join(format!("{}.json", fid));
                let base_json_path = file_utils::version_dir(&versions_dir, &base_version_id)?
                    .join(format!("{}.json", base_version_id));

                if forge_json_path.exists() && base_json_path.exists() {
                    send_progress(70, "合并配置...", false);

                    if let Err(e) = merge_forge_instance_json(
                        &new_instance_name,
                        &new_json_path,
                        &base_json_path,
                        &forge_json_path,
                    ) {
                        cleanup();
                        return Err(e);
                    }

                    let forge_dir = versions_dir.join(&fid);
                    if forge_dir.exists() && forge_dir != dest_dir {
                        let _ = fs::remove_dir_all(forge_dir);
                    }
                } else {
                    warn!("未找到 Forge 或 基础版本的 JSON 文件，跳过合并");
                }
            } else {
                warn!("未找到安装后的 Forge 目录");
            }
        }
    }

    send_progress(100, "实例创建完成！", false);

    if let Ok(game_dir_str) = config::get_game_dir() {
        let _ = window.emit("game-dir-changed", &game_dir_str);
    }

    Ok(())
}
