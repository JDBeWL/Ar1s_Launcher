//! 扫描 versions 目录
//!
//! 实例目录名不一定等于 MC 版本，加载器信息也可能只写在 libraries 里，所以这里逐级往回推

use super::descriptor::{parse_fml_arguments, parse_loader_from_libraries, parse_patches};
use super::get_dirs;
use super::maintenance::prune_stale_run_locks;
use crate::errors::LauncherError;
use crate::models::InstanceInfo;
use crate::services::{
    config,
    memory::{compute_auto_memory, get_system_memory},
};
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

pub async fn get_instances() -> Result<Vec<InstanceInfo>, LauncherError> {
    let (game_dir, versions_dir) = get_dirs()?;
    // 关闭隔离时所有实例共用根目录的 mods，计数要按共享目录来
    let isolated = config::is_version_isolated();

    // 目录遍历 + JSON 解析是同步活；清残留运行锁要逐个探测锁文件、抢不到时退让重试，
    // 同样不能在 async worker 上等，一并塞进同一个阻塞任务
    let instances = tokio::task::spawn_blocking(move || {
        prune_stale_run_locks(&game_dir, &versions_dir);
        get_instances_sync(&versions_dir, isolated)
    })
    .await
    .map_err(|e| LauncherError::Custom(format!("获取实例列表失败: {}", e)))??;

    Ok(instances)
}

/// 只解析一个实例的元数据，省掉对其余所有版本 JSON 的解析
pub async fn get_instance_info(instance_name: &str) -> Result<InstanceInfo, LauncherError> {
    let (game_dir, versions_dir) = get_dirs()?;
    let isolated = config::is_version_isolated();
    let wanted = instance_name.to_string();

    tokio::task::spawn_blocking(move || {
        prune_stale_run_locks(&game_dir, &versions_dir);
        get_instances_filtered_sync(&versions_dir, isolated, Some(&wanted))
            .ok()
            .and_then(|mut list| list.pop())
            .ok_or_else(|| LauncherError::Custom(format!("实例 '{}' 不存在", wanted)))
    })
    .await
    .map_err(|e| LauncherError::Custom(format!("获取实例信息失败: {}", e)))?
}

/// instance.json 里记录的整合包元数据
pub(super) struct InstanceMetadata {
    minecraft: String,
    loader: Option<String>,
    icon_url: Option<String>,
}

pub(super) fn read_instance_metadata(instance_dir: &Path) -> Option<InstanceMetadata> {
    let meta_path = instance_dir.join("instance.json");
    if !meta_path.exists() {
        return None;
    }
    let content = fs::read_to_string(&meta_path).ok()?;
    let json: Value = serde_json::from_str(&content).ok()?;

    let mc_version = json["minecraft"].as_str().map(String::from)?;

    Some(InstanceMetadata {
        minecraft: mc_version,
        loader: json["loader"].as_str().map(String::from),
        icon_url: json["icon_url"].as_str().map(String::from),
    })
}

/// 沿 inheritsFrom 链解析真实 MC 版本：id 本身是版本号就直接用，否则读父 JSON 递归
pub(super) fn find_real_minecraft_version(
    version_id: &str,
    json_value: &Value,
    versions_dir: &Path,
) -> Option<String> {
    find_real_minecraft_version_at_depth(version_id, json_value, versions_dir, 0)
}

pub(super) fn find_real_minecraft_version_at_depth(
    version_id: &str,
    json_value: &Value,
    versions_dir: &Path,
    depth: usize,
) -> Option<String> {
    // 只用来填 instance.json 的元数据，遇到环形继承链时放弃即可
    if depth > 16 {
        return None;
    }
    if let Some(inherits) = json_value["inheritsFrom"].as_str() {
        if crate::utils::minecraft::parse_mc_version(inherits).is_some() {
            return Some(inherits.to_string());
        }
        let parent_json_path = versions_dir
            .join(inherits)
            .join(format!("{}.json", inherits));
        if parent_json_path.exists()
            && let Ok(parent_content) = fs::read_to_string(&parent_json_path)
            && let Ok(parent_json) = serde_json::from_str::<Value>(&parent_content)
        {
            return find_real_minecraft_version_at_depth(
                inherits,
                &parent_json,
                versions_dir,
                depth + 1,
            );
        }
        Some(inherits.to_string())
    } else {
        Some(version_id.to_string())
    }
}

pub(super) fn get_instances_sync(
    versions_dir: &Path,
    isolated: bool,
) -> Result<Vec<InstanceInfo>, LauncherError> {
    get_instances_filtered_sync(versions_dir, isolated, None)
}

/// `only` 只取指定实例时，跳过其余版本目录，不做 JSON 解析
/// 装模组、装资源包、导出整合包都只需要一个实例的信息，不该每次都把全表重解析一遍
pub(super) fn get_instances_filtered_sync(
    versions_dir: &Path,
    isolated: bool,
    only: Option<&str>,
) -> Result<Vec<InstanceInfo>, LauncherError> {
    let mut instances = Vec::new();

    if !versions_dir.exists() {
        return Ok(instances);
    }

    // 关闭隔离时内容是共享的，各实例的模组数都来自这里
    let shared_root = versions_dir
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| versions_dir.to_path_buf());

    // 自动内存开启时给每张实例卡片一个启动时的预计值；可用内存只采样一次
    let config = config::load_config().ok();
    let auto_memory_available_mb = config
        .as_ref()
        .filter(|cfg| cfg.auto_memory_enabled)
        .map(|_| get_system_memory().available_memory_mb as u32);

    // 上次启动时间整表取一次：逐实例调用会重复 stat 配置文件 + 抢锁，而内容其实是同一份
    let last_played_map = config
        .as_ref()
        .map(|cfg| cfg.instance_last_played.clone())
        .unwrap_or_default();

    // 关闭隔离时所有实例共用根目录的 mods：20 个实例 × 500 个模组 = 1 万次完全重复的遍历
    let mut mod_count_cache: HashMap<PathBuf, Option<u32>> = HashMap::new();

    if let Ok(entries) = fs::read_dir(versions_dir) {
        for entry in entries.flatten() {
            if let Ok(file_type) = entry.file_type()
                && file_type.is_dir()
            {
                let name = entry.file_name().to_string_lossy().to_string();
                // 只要一个实例时，别人的目录连 stat 都不必做
                if only.is_some_and(|wanted| wanted != name) {
                    continue;
                }
                let path = entry.path();
                let json_path = path.join(format!("{}.json", name));

                if json_path.exists() {
                    let json_content = fs::read_to_string(&json_path).ok();
                    let json_value = json_content
                        .as_ref()
                        .and_then(|c| serde_json::from_str::<Value>(c).ok());

                    let version_id = json_value
                        .as_ref()
                        .and_then(|v| v["id"].as_str().map(String::from))
                        .unwrap_or_else(|| name.clone());

                    let content_dir = if isolated { &path } else { &shared_root };
                    let mod_count = *mod_count_cache
                        .entry(content_dir.clone())
                        .or_insert_with(|| count_mods(content_dir));

                    let main_class = json_value
                        .as_ref()
                        .and_then(|v| v["mainClass"].as_str())
                        .unwrap_or("");

                    let mut loader_type: Option<String> = None;
                    let mut loader_version: Option<String> = None;
                    let mut game_version: Option<String> = None;
                    let mut icon_url: Option<String> = None;

                    if let Some(ref json) = json_value
                        && let (Some(lt), fv, mv) = parse_fml_arguments(json)
                    {
                        loader_type = Some(lt);
                        loader_version = fv;
                        game_version = mv;
                    }

                    if let Some(ref json) = json_value {
                        let (plt, plv, pgv) = parse_patches(json);
                        if plt.is_some() {
                            if loader_type.is_none() {
                                loader_type = plt;
                            }
                            if loader_version.is_none() {
                                loader_version = plv;
                            }
                            if game_version.is_none() {
                                game_version = pgv;
                            }
                        }
                    }

                    if let Some(meta) = read_instance_metadata(&path) {
                        if game_version.is_none() {
                            game_version = Some(meta.minecraft);
                        }
                        if icon_url.is_none() {
                            icon_url = meta.icon_url;
                        }
                        if loader_type.is_none()
                            && let Some(lt) = meta.loader
                        {
                            loader_type = Some(match lt.as_str() {
                                "forge" => "Forge".to_string(),
                                "fabric" => "Fabric".to_string(),
                                "quilt" => "Quilt".to_string(),
                                "neoforge" => "NeoForge".to_string(),
                                other => {
                                    let mut c = other.chars();
                                    match c.next() {
                                        Some(f) => {
                                            f.to_uppercase().collect::<String>() + c.as_str()
                                        }
                                        None => other.to_string(),
                                    }
                                }
                            });
                        }
                    }

                    if let Some(ref json) = json_value
                        && let (Some(lt), Some(lv)) = parse_loader_from_libraries(json)
                    {
                        if loader_type.is_none() {
                            loader_type = Some(lt);
                        }
                        if loader_version.is_none() {
                            loader_version = Some(lv);
                        }
                    }

                    if loader_type.is_none() {
                        if main_class.contains("forge")
                            || main_class.contains("LaunchWrapper")
                            || main_class.contains("modlauncher")
                        {
                            loader_type = Some("Forge".to_string());
                        } else if main_class.contains("fabricmc")
                            || (main_class.contains("knot")
                                && main_class.to_lowercase().contains("fabric"))
                        {
                            loader_type = Some("Fabric".to_string());
                        } else if main_class.contains("quiltmc")
                            || (main_class.contains("knot")
                                && main_class.to_lowercase().contains("quilt"))
                        {
                            loader_type = Some("Quilt".to_string());
                        } else if main_class.contains("neoforge") {
                            loader_type = Some("NeoForge".to_string());
                        }
                    }

                    if game_version.is_none() {
                        if let Some(json) = json_value.as_ref() {
                            game_version =
                                find_real_minecraft_version(&version_id, json, versions_dir);
                        } else {
                            game_version = Some(version_id.clone());
                        }
                    }

                    if loader_type.is_none() {
                        if json_value
                            .as_ref()
                            .and_then(|v| v["inheritsFrom"].as_str())
                            .is_some()
                        {
                            loader_type = Some("Unknown".to_string());
                        } else if mod_count.unwrap_or(0) > 0 {
                            loader_type = Some("Modded".to_string());
                        } else {
                            loader_type = Some("None".to_string());
                        }
                    }

                    // 实例自己指定了内存时，卡片上的“自动内存”不再适用
                    let has_memory_override = auto_memory_available_mb.is_some()
                        && crate::services::instance_settings::read_from_instance_dir(&path)
                            .map(|settings| settings.memory_mb.is_some())
                            .unwrap_or(false);
                    let auto_memory_mb = if has_memory_override {
                        None
                    } else {
                        auto_memory_available_mb.map(|available| {
                            compute_auto_memory(
                                game_version.as_deref().unwrap_or(&version_id),
                                loader_type.as_deref().is_some_and(|lt| lt != "None"),
                                mod_count.unwrap_or(0) as usize,
                                available,
                                false,
                            )
                            .memory_mb
                        })
                    };

                    let created = entry
                        .metadata()
                        .and_then(|m| m.created())
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_secs().to_string());

                    let config_last_played = last_played_map.get(&name).copied();
                    let json_last_played =
                        json_value.as_ref().and_then(|v| v["lastPlayed"].as_i64());
                    let last_played = match (config_last_played, json_last_played) {
                        (Some(c), Some(j)) => Some(c.max(j)),
                        (Some(c), None) => Some(c),
                        (None, Some(j)) => Some(j),
                        (None, None) => None,
                    };

                    instances.push(InstanceInfo {
                        id: name.clone(),
                        name: name.clone(),
                        version: version_id,
                        path: path.to_string_lossy().to_string(),
                        created_time: created,
                        loader_type,
                        loader_version,
                        game_version,
                        last_played,
                        mod_count,
                        icon_url,
                        auto_memory_mb,
                    });
                }
            }
        }
    }
    Ok(instances)
}

/// 内容目录下的模组数量（隔离时是实例目录，否则是共享根目录）
pub(super) fn count_mods(content_dir: &Path) -> Option<u32> {
    let mods_dir = content_dir.join("mods");
    if !mods_dir.is_dir() {
        return None;
    }
    // 只取文件名判后缀：`e.path()` 会为每个条目分配一个 PathBuf，几百个模组就是几百次纯浪费
    let count = fs::read_dir(&mods_dir)
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name()
                .to_str()
                .is_some_and(|name| name.ends_with(".jar"))
        })
        .count() as u32;
    if count == 0 { None } else { Some(count) }
}
