//! 实例生命周期：删除、重命名、打开目录、启动

use super::scan::find_real_minecraft_version;
use super::{acquire_instance_lock, acquire_run_lock, get_dirs};
use crate::errors::LauncherError;
use crate::models::LaunchOptions;
use crate::services::{config, launcher};
use crate::utils::file_utils::{self, validate_instance_name_or_error};
use log::{info, warn};
use serde_json::Value;
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use tauri::Window;

pub async fn delete_instance(instance_name: String) -> Result<(), LauncherError> {
    let (game_dir, versions_dir) = get_dirs()?;
    let _instance_lock = acquire_instance_lock(&game_dir).await?;
    let instance_dir = file_utils::instance_dir(&versions_dir, &instance_name)?;

    if !instance_dir.exists() {
        return Err(LauncherError::Custom(format!(
            "实例 '{}' 不存在",
            instance_name
        )));
    }

    let run_lock = acquire_run_lock(&game_dir, &instance_name)
        .await
        .map_err(|e| match e {
            LauncherError::Busy(_) => LauncherError::Custom(format!(
                "实例 '{}' 正在运行或正在被修改，无法删除",
                instance_name
            )),
            other => other,
        })?;

    // 整个实例目录可能上 GB，删除会长时间占住线程，不能放在 async worker 上
    let dir_to_delete = instance_dir.clone();
    tokio::task::spawn_blocking(move || fs::remove_dir_all(&dir_to_delete))
        .await
        .map_err(|e| LauncherError::Custom(format!("删除实例失败: {}", e)))?
        .map_err(|e| LauncherError::Custom(format!("删除实例失败: {}", e)))?;

    let _ = config::remove_instance_last_played(&instance_name);
    drop(run_lock);

    info!("实例 {} 已删除", instance_name);
    Ok(())
}

pub async fn rename_instance(old_name: String, new_name: String) -> Result<(), LauncherError> {
    validate_instance_name_or_error(&new_name)?;

    let (game_dir, versions_dir) = get_dirs()?;
    let _instance_lock = acquire_instance_lock(&game_dir).await?;
    let old_dir = file_utils::instance_dir(&versions_dir, &old_name)?;
    let new_dir = file_utils::instance_dir(&versions_dir, &new_name)?;

    if !old_dir.exists() {
        return Err(LauncherError::Custom(format!(
            "原实例 '{}' 不存在",
            old_name
        )));
    }
    if new_dir.exists() {
        return Err(LauncherError::Custom(format!(
            "目标实例名 '{}' 已存在",
            new_name
        )));
    }

    let run_lock = acquire_run_lock(&game_dir, &old_name)
        .await
        .map_err(|e| match e {
            LauncherError::Busy(_) => LauncherError::Custom(format!(
                "实例 '{}' 正在运行或正在被修改，无法重命名",
                old_name
            )),
            other => other,
        })?;

    // 原版实例没有 inheritsFrom，一旦下面的 id 被改写就再也取不回真实 MC 版本，
    // 所以先固化下来
    let real_mc_version = fs::read_to_string(old_dir.join(format!("{}.json", old_name)))
        .ok()
        .and_then(|c| serde_json::from_str::<Value>(&c).ok())
        .and_then(|json| find_real_minecraft_version(&old_name, &json, &versions_dir));

    // 改名分两步：先在原目录里把 <old>.json / <old>.jar 改成新名字并改写 id，
    // 全部成功后才改名目录本身。反过来做的话目录已叫 new_name、里面却还是 old_name.json，
    // 失败时既启动不了也改不回去。这几步不能吞错：静默跳过 id 改写会留下「重命名成功但启动不了」
    let old_json = old_dir.join(format!("{}.json", old_name));
    let renamed_json = old_dir.join(format!("{}.json", new_name));
    let old_jar = old_dir.join(format!("{}.jar", old_name));

    let mut json_renamed = false;
    if old_json.exists() {
        fs::rename(&old_json, &renamed_json)
            .map_err(|e| LauncherError::Custom(format!("重命名版本 JSON 失败: {}", e)))?;
        json_renamed = true;
    }

    let mut jar_renamed = false;
    if old_jar.exists() {
        match fs::rename(&old_jar, old_dir.join(format!("{}.jar", new_name))) {
            Ok(()) => jar_renamed = true,
            // jar 没改成，就把 JSON 名字改回去，尽量别留下半改状态
            Err(e) => {
                if json_renamed {
                    let _ = fs::rename(&renamed_json, &old_json);
                }
                return Err(LauncherError::Custom(format!(
                    "重命名客户端 jar 失败: {}",
                    e
                )));
            }
        }
    }

    // 只有 JSON 真就位了才改写 id；解析失败直接报错，不再静默跳过
    let mut id_rewritten = false;
    if renamed_json.exists() {
        let content = fs::read_to_string(&renamed_json)?;
        let mut json: Value = serde_json::from_str(&content)
            .map_err(|e| LauncherError::Custom(format!("版本 JSON 解析失败: {}", e)))?;
        json["id"] = Value::String(new_name.clone());
        fs::write(&renamed_json, serde_json::to_string_pretty(&json)?)?;
        id_rewritten = true;
    }

    if let Err(e) = fs::rename(&old_dir, &new_dir) {
        // 目录改名是最后一步：失败时把已改的文件名与 id 全部还原，
        // 否则目录仍叫旧名、里面却已是 new_name.json，实例会直接从列表里消失
        warn!("重命名实例目录失败，正在回滚文件名与 id: {}", e);
        if id_rewritten
            && let Some(mut json) = fs::read_to_string(&renamed_json)
                .ok()
                .and_then(|content| serde_json::from_str::<Value>(&content).ok())
        {
            json["id"] = Value::String(old_name.clone());
            if let Ok(pretty) = serde_json::to_string_pretty(&json) {
                let _ = fs::write(&renamed_json, pretty);
            }
        }
        if json_renamed {
            let _ = fs::rename(&renamed_json, &old_json);
        }
        if jar_renamed {
            let _ = fs::rename(old_dir.join(format!("{}.jar", new_name)), &old_jar);
        }
        return Err(LauncherError::Custom(format!("重命名目录失败: {}", e)));
    }

    let _ = config::rename_instance_last_played(&old_name, &new_name);

    // 把固化下来的版本写进 instance.json，读取端会优先采用它
    let meta_path = new_dir.join("instance.json");
    if !meta_path.exists()
        && let Some(mc) = real_mc_version
    {
        let meta = serde_json::json!({ "minecraft": mc });
        fs::write(&meta_path, serde_json::to_string_pretty(&meta)?)?;
    }

    // 运行锁固定一个文件，改名后不需要再清理旧名字的锁
    drop(run_lock);

    Ok(())
}

pub async fn open_instance_folder(instance_name: String) -> Result<(), LauncherError> {
    let (_, versions_dir) = get_dirs()?;
    let instance_dir = file_utils::instance_dir(&versions_dir, &instance_name)?;

    if !instance_dir.exists() {
        return Err(LauncherError::Custom(format!(
            "实例 '{}' 不存在",
            instance_name
        )));
    }

    opener::open(&instance_dir)
        .map_err(|e| LauncherError::Custom(format!("无法打开文件夹: {}", e)))?;

    Ok(())
}

pub async fn launch_instance(
    instance_name: String,
    override_java_path: Option<String>,
    force_java_mismatch: bool,
    window: Window,
) -> Result<(), LauncherError> {
    let config = config::load_config()?;
    let (_, versions_dir) = get_dirs()?;
    let instance_dir = file_utils::instance_dir(&versions_dir, &instance_name)?;

    let json_path = instance_dir.join(format!("{}.json", instance_name));
    if !json_path.exists() {
        return Err(LauncherError::Custom(format!(
            "实例 '{}' 的配置文件不存在",
            instance_name
        )));
    }

    let json_content = fs::read_to_string(&json_path)?;
    let json: Value = serde_json::from_str(&json_content).map_err(|e| {
        LauncherError::Custom(format!(
            "实例 '{}' 的配置文件格式无效: {}",
            instance_name, e
        ))
    })?;

    if !json.get("id").and_then(|v| v.as_str()).is_some() {
        return Err(LauncherError::Custom(format!(
            "实例 '{}' 的配置文件缺少 'id' 字段",
            instance_name
        )));
    }
    if !json.get("mainClass").and_then(|v| v.as_str()).is_some() {
        return Err(LauncherError::Custom(format!(
            "实例 '{}' 的配置文件缺少 'mainClass' 字段",
            instance_name
        )));
    }

    let config_last_played = config.instance_last_played.get(&instance_name).copied();
    if let Some(ts) = config_last_played
        && let Some(json_played) = json.get("lastPlayed").and_then(|v| v.as_i64())
        && json_played != ts
    {
        warn!(
            "实例 '{}' 的 lastPlayed 不一致 (config: {}, json: {}), 使用 config 值",
            instance_name, ts, json_played
        );
    }

    // lastPlayed 已由 launcher::launch_minecraft 更新，此处不重复

    // Microsoft token 的读取与刷新统一由 launcher 在认证锁内完成，
    // 这里只告诉它用哪种登录方式，避免两个入口各自轮换 refresh_token
    let auth_type = match config.auth_type {
        crate::models::AuthType::Microsoft => Some("microsoft".to_string()),
        crate::models::AuthType::Offline => None,
    };

    let instance_settings = crate::services::instance_settings::load(&instance_name).ok();

    let launch_options = LaunchOptions {
        version: instance_name,
        username: config
            .username
            .clone()
            .unwrap_or_else(|| "Player".to_string()),
        memory: Some(config.max_memory),
        window_width: config.window_width,
        window_height: config.window_height,
        fullscreen: Some(config.fullscreen),
        auth_type,
        access_token: None,
        uuid: None,
        override_java_path,
        instance_settings,
        force_java_mismatch,
    };

    launcher::launch_minecraft(launch_options, window).await
}

pub(super) fn merge_forge_instance_json(
    instance_id: &str,
    target_json_path: &Path,
    base_json_path: &Path,
    forge_json_path: &Path,
) -> Result<(), LauncherError> {
    let base_content = fs::read_to_string(base_json_path)?;
    let forge_content = fs::read_to_string(forge_json_path)?;

    let base_json: Value =
        serde_json::from_str(&base_content).map_err(|e| LauncherError::Custom(e.to_string()))?;
    let forge_json: Value =
        serde_json::from_str(&forge_content).map_err(|e| LauncherError::Custom(e.to_string()))?;

    let mut merged = forge_json.clone();
    merged["id"] = Value::String(instance_id.to_string());

    if merged["mainClass"].is_null() {
        merged["mainClass"] = base_json["mainClass"].clone();
    }

    if merged["arguments"].is_null() {
        if let Some(forge_args) = forge_json["minecraftArguments"].as_str() {
            let args_array: Vec<Value> = forge_args
                .split_whitespace()
                .map(|s| Value::String(s.to_string()))
                .collect();
            merged["arguments"] = serde_json::json!({ "game": args_array });
        } else if !base_json["arguments"].is_null() {
            merged["arguments"] = base_json["arguments"].clone();
        } else if let Some(base_args) = base_json["minecraftArguments"].as_str() {
            let args_array: Vec<Value> = base_args
                .split_whitespace()
                .map(|s| Value::String(s.to_string()))
                .collect();
            merged["arguments"] = serde_json::json!({ "game": args_array });
        }
    }

    let mut final_libs = Vec::new();
    let mut seen_libs = HashSet::new();

    if let Some(libs) = forge_json["libraries"].as_array() {
        for lib in libs {
            if let Some(name) = lib["name"].as_str() {
                seen_libs.insert(name.to_string());
            }
            final_libs.push(lib.clone());
        }
    }

    if let Some(libs) = base_json["libraries"].as_array() {
        for lib in libs {
            if let Some(name) = lib["name"].as_str() {
                if !seen_libs.contains(name) {
                    final_libs.push(lib.clone());
                }
            } else {
                final_libs.push(lib.clone());
            }
        }
    }
    merged["libraries"] = Value::Array(final_libs);

    if let Some(obj) = base_json.as_object() {
        for (k, v) in obj {
            if merged[k].is_null() {
                merged[k] = v.clone();
            }
        }
    }

    fs::write(target_json_path, serde_json::to_string_pretty(&merged)?)?;

    Ok(())
}
