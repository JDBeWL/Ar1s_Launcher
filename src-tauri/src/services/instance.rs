//! 实例管理模块
//!
//! 创建、重命名、删除实例，从版本 JSON 逐级推断加载器与游戏版本，兼管 mods 目录的 Mod 启停

use crate::errors::LauncherError;
use crate::models::{InstanceInfo, LaunchOptions};
use crate::services::{
    config, download, launcher,
    loaders::{self, LoaderType},
    memory::{compute_auto_memory, get_system_memory},
};
use crate::utils::file_utils::{
    self, InstanceNameValidation, validate_instance_name, validate_instance_name_or_error,
};
use log::{info, warn};
use serde::Serialize;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tauri::{Emitter, Window};

const INSTANCE_LOCK_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone, Serialize)]
struct InstallProgress {
    progress: u8,
    message: String,
    indeterminate: bool,
}

fn get_dirs() -> Result<(PathBuf, PathBuf), LauncherError> {
    let config = config::load_config()?;
    let game_dir = PathBuf::from(&config.game_dir);
    let versions_dir = game_dir.join("versions");
    Ok((game_dir, versions_dir))
}

/// 所有跨进程锁都放在游戏目录下的 `.ar1s/`
pub(crate) fn lock_dir(game_dir: &Path) -> PathBuf {
    game_dir.join(".ar1s")
}

/// 实例目录的创建/删除/重命名/整合包安装跨进程串行；嵌套下载另用 download.lock
pub(crate) async fn acquire_instance_lock(
    game_dir: &Path,
) -> Result<crate::utils::file_lock::FileLock, LauncherError> {
    crate::utils::file_lock::acquire_async(
        &lock_dir(game_dir).join("instances.lock"),
        INSTANCE_LOCK_TIMEOUT,
    )
    .await
}

/// 运行锁不能放进实例目录：Windows 上目录里有打开的文件就重命名不了，
/// 而删除/重命名实例必须先拿到这把锁，会自己把自己锁死
fn run_lock_path(game_dir: &Path, instance_name: &str) -> PathBuf {
    lock_dir(game_dir).join(format!("run-{}.lock", instance_name))
}

/// 游戏进程运行期间由监控线程持有；共享锁允许同一实例同时启动多次
pub(crate) async fn acquire_run_shared_lock(
    game_dir: &Path,
    instance_name: &str,
) -> Result<crate::utils::file_lock::FileLock, LauncherError> {
    crate::utils::file_lock::acquire_shared_async(
        &run_lock_path(game_dir, instance_name),
        INSTANCE_LOCK_TIMEOUT,
    )
    .await
}

/// 删除/重命名独占运行锁，确认没有游戏进程还在跑
pub(crate) async fn acquire_run_lock(
    game_dir: &Path,
    instance_name: &str,
) -> Result<crate::utils::file_lock::FileLock, LauncherError> {
    crate::utils::file_lock::acquire_async(
        &run_lock_path(game_dir, instance_name),
        INSTANCE_LOCK_TIMEOUT,
    )
    .await
}

/// 只兜底清理旧版本或异常中断留下的空壳，判断依据见 `remove_if_unused`
///
/// 不能直接 `remove_file`：共享锁允许 FILE_SHARE_DELETE，直接删会把正在运行的游戏
/// 的锁文件删掉，新来的排他锁就会误判"没有实例在跑"
pub fn prune_stale_run_locks(game_dir: &Path, versions_dir: &Path) {
    let Ok(entries) = fs::read_dir(lock_dir(game_dir)) else {
        return;
    };

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(instance) = name
            .strip_prefix("run-")
            .and_then(|rest| rest.strip_suffix(".lock"))
        else {
            continue;
        };
        // 实例目录还在就说明可能真在运行，交给持有者自己清理
        if versions_dir.join(instance).is_dir() {
            continue;
        }
        if crate::utils::file_lock::remove_if_unused(&entry.path()) {
            info!("清理残留的运行锁: {}", name);
        }
    }
}

/// 旧版本把下载状态和锁放在 `.download_state/`、natives 锁放在版本目录里。
/// 状态文件关系到断点续传，必须搬过来；旧锁文件直接删（进程退出后本来就没有意义）
pub fn migrate_legacy_state(game_dir: &Path, versions_dir: &Path) {
    let lock_root = lock_dir(game_dir);
    let legacy_state = game_dir.join(".download_state");

    if legacy_state.is_dir() {
        let new_state = lock_root.join("download_state");
        if let Err(e) = fs::create_dir_all(&new_state) {
            warn!("创建下载状态目录失败: {}", e);
        } else if let Ok(entries) = fs::read_dir(&legacy_state) {
            for entry in entries.flatten() {
                let path = entry.path();
                let name = entry.file_name();
                // 只搬续传状态，旧锁文件留着没有用
                let is_lock = path.extension().is_some_and(|ext| ext == "lock");
                let target = new_state.join(&name);
                if !is_lock && path.is_file() && !target.exists() {
                    let _ = fs::rename(&path, &target);
                }
            }
        }
        // 只剩空目录或旧锁文件时整个删掉
        let _ = fs::remove_dir_all(&legacy_state);
    }

    // 每个实例目录下的 .natives.lock 也搬到统一锁目录
    if let Ok(entries) = fs::read_dir(versions_dir) {
        for entry in entries.flatten() {
            let stale = entry.path().join(".natives.lock");
            if stale.exists() {
                let _ = fs::remove_file(&stale);
            }
        }
    }
}

/// 已经启动过一次就不再重复扫描；迁移只在启动时做一次
pub fn migrate_legacy_state_once(game_dir: &Path, versions_dir: &Path) {
    static MIGRATED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if MIGRATED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    migrate_legacy_state(game_dir, versions_dir);
}

pub fn check_instance_name_available(name: &str) -> InstanceNameValidation {
    let validation = validate_instance_name(name);
    if !validation.is_valid {
        return validation;
    }

    if let Ok((_, versions_dir)) = get_dirs() {
        let instance_dir = versions_dir.join(name);
        if instance_dir.exists() {
            return InstanceNameValidation {
                is_valid: false,
                error_message: Some(format!("名为 '{}' 的实例已存在，请使用其他名称", name)),
            };
        }
    }

    InstanceNameValidation {
        is_valid: true,
        error_message: None,
    }
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
        return Err(LauncherError::Custom(format!(
            "实例 '{}' 已存在",
            new_instance_name
        )));
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

/// Forge/NeoForge 把加载器信息写在 arguments.game 的 `--fml.*` 参数里，
/// 旧版 JSON 则在 minecraftArguments 字符串中
fn parse_fml_arguments(json_value: &Value) -> (Option<String>, Option<String>, Option<String>) {
    let args = match json_value
        .get("arguments")
        .and_then(|a| a.get("game"))
        .and_then(|g| g.as_array())
    {
        Some(arr) => arr,
        None => {
            let raw_args = match json_value["minecraftArguments"].as_str() {
                Some(s) => s,
                None => return (None, None, None),
            };
            let tokens: Vec<&str> = raw_args.split_whitespace().collect();
            let mut forge_ver: Option<String> = None;
            let mut mc_ver: Option<String> = None;
            let mut is_neoforge = false;

            for (i, token) in tokens.iter().enumerate() {
                match *token {
                    "--fml.forgeVersion" => {
                        if i + 1 < tokens.len() {
                            forge_ver = Some(tokens[i + 1].to_string());
                        }
                    }
                    "--fml.neoForgeVersion" => {
                        if i + 1 < tokens.len() {
                            forge_ver = Some(tokens[i + 1].to_string());
                            is_neoforge = true;
                        }
                    }
                    "--fml.mcVersion" if i + 1 < tokens.len() => {
                        mc_ver = Some(tokens[i + 1].to_string());
                    }
                    _ => {}
                }
            }

            if forge_ver.is_some() || mc_ver.is_some() {
                let lt = if is_neoforge { "NeoForge" } else { "Forge" };
                return (Some(lt.to_string()), forge_ver, mc_ver);
            }
            return (None, None, None);
        }
    };

    let mut forge_ver: Option<String> = None;
    let mut mc_ver: Option<String> = None;
    let mut is_neoforge = false;

    for (i, arg) in args.iter().enumerate() {
        let token = match arg.as_str() {
            Some(s) => s,
            None => continue,
        };

        match token {
            "--fml.forgeVersion" => {
                if let Some(next) = args.get(i + 1).and_then(|v| v.as_str()) {
                    forge_ver = Some(next.to_string());
                }
            }
            "--fml.neoForgeVersion" => {
                if let Some(next) = args.get(i + 1).and_then(|v| v.as_str()) {
                    forge_ver = Some(next.to_string());
                    is_neoforge = true;
                }
            }
            "--fml.mcVersion" => {
                if let Some(next) = args.get(i + 1).and_then(|v| v.as_str()) {
                    mc_ver = Some(next.to_string());
                }
            }
            _ => {}
        }
    }

    if forge_ver.is_some() || mc_ver.is_some() {
        let lt = if is_neoforge { "NeoForge" } else { "Forge" };
        return (Some(lt.to_string()), forge_ver, mc_ver);
    }

    (None, None, None)
}

/// Fabric/Quilt 的版本 JSON 用 patches 分别记录 game 与加载器条目
fn parse_patches(json_value: &Value) -> (Option<String>, Option<String>, Option<String>) {
    let patches = match json_value["patches"].as_array() {
        Some(arr) => arr,
        None => return (None, None, None),
    };

    let mut game_version: Option<String> = None;
    let mut loader_type: Option<String> = None;
    let mut loader_version: Option<String> = None;

    for patch in patches {
        let id = match patch["id"].as_str() {
            Some(s) => s,
            None => continue,
        };
        let version = patch["version"].as_str().map(String::from);

        match id {
            "game" => {
                game_version = version;
            }
            "fabric" => {
                loader_type = Some("Fabric".to_string());
                loader_version = version;
            }
            "quilt" => {
                loader_type = Some("Quilt".to_string());
                loader_version = version;
            }
            "forge" => {
                loader_type = Some("Forge".to_string());
                loader_version = version;
            }
            "neoforge" => {
                loader_type = Some("NeoForge".to_string());
                loader_version = version;
            }
            _ => {}
        }
    }

    if loader_type.is_some() || game_version.is_some() {
        return (loader_type, loader_version, game_version);
    }

    (None, None, None)
}

/// 按 Maven 坐标的 group/artifact 识别各加载器
fn parse_loader_from_libraries(json_value: &Value) -> (Option<String>, Option<String>) {
    let libs = match json_value["libraries"].as_array() {
        Some(arr) => arr,
        None => return (None, None),
    };

    for lib in libs {
        let name = match lib["name"].as_str() {
            Some(n) => n,
            None => continue,
        };

        let parts: Vec<&str> = name.split(':').collect();
        if parts.len() < 3 {
            continue;
        }

        let group = parts[0];
        let artifact = parts[1];
        let version = parts[2];

        if group == "net.minecraftforge" && artifact == "forge" {
            let loader_ver = version
                .split('-')
                .next_back()
                .unwrap_or(version)
                .to_string();
            return (Some("Forge".to_string()), Some(loader_ver));
        }

        if group == "net.minecraftforge" && artifact == "fmlloader" {
            let loader_ver = version
                .split('-')
                .next_back()
                .unwrap_or(version)
                .to_string();
            return (Some("Forge".to_string()), Some(loader_ver));
        }

        if group == "net.fabricmc" && artifact == "fabric-loader" {
            return (Some("Fabric".to_string()), Some(version.to_string()));
        }

        if group == "org.quiltmc" && artifact == "quilt-loader" {
            return (Some("Quilt".to_string()), Some(version.to_string()));
        }

        if group == "net.neoforged" && (artifact == "neoforge" || artifact == "fml") {
            let loader_ver = version
                .split('-')
                .next_back()
                .unwrap_or(version)
                .to_string();
            return (Some("NeoForge".to_string()), Some(loader_ver));
        }
    }

    (None, None)
}

/// instance.json 里记录的整合包元数据
struct InstanceMetadata {
    minecraft: String,
    loader: Option<String>,
    icon_url: Option<String>,
}

fn read_instance_metadata(instance_dir: &Path) -> Option<InstanceMetadata> {
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
fn find_real_minecraft_version(
    version_id: &str,
    json_value: &Value,
    versions_dir: &Path,
) -> Option<String> {
    find_real_minecraft_version_at_depth(version_id, json_value, versions_dir, 0)
}

fn find_real_minecraft_version_at_depth(
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

fn get_instances_sync(
    versions_dir: &Path,
    isolated: bool,
) -> Result<Vec<InstanceInfo>, LauncherError> {
    get_instances_filtered_sync(versions_dir, isolated, None)
}

/// `only` 只取指定实例时，跳过其余版本目录，不做 JSON 解析
/// 装模组、装资源包、导出整合包都只需要一个实例的信息，不该每次都把全表重解析一遍
fn get_instances_filtered_sync(
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

                    let auto_memory_mb = auto_memory_available_mb.map(|available| {
                        compute_auto_memory(
                            game_version.as_deref().unwrap_or(&version_id),
                            loader_type.as_deref().is_some_and(|lt| lt != "None"),
                            mod_count.unwrap_or(0) as usize,
                            available,
                            false,
                        )
                        .memory_mb
                    });

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
fn count_mods(content_dir: &Path) -> Option<u32> {
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

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModInfo {
    /// 磁盘上的实际文件名，禁用时带 .disabled 后缀
    pub file_name: String,
    /// 展示名，读不到 jar 内元数据时回退成文件名
    pub display_name: String,
    pub version: Option<String>,
    pub enabled: bool,
    pub size: u64,
}

/// 定位 mods 目录下的某个文件，顺带挡住越界的文件名
fn resolve_mod_path(
    instance_name: &str,
    file_name: &str,
) -> Result<(PathBuf, PathBuf), LauncherError> {
    let (game_dir, _) = get_dirs()?;
    let mods_dir = launcher::isolation::content_root_for(&game_dir, instance_name).join("mods");
    if !mods_dir.is_dir() {
        return Err(LauncherError::Custom(format!(
            "没有找到 mods 目录: {}",
            mods_dir.display()
        )));
    }

    // 前端只应该传文件名，带路径分隔符或 .. 就能跳出 mods 目录
    if file_name.is_empty()
        || file_name.contains('/')
        || file_name.contains('\\')
        || file_name.contains("..")
        || file_name.contains(':')
    {
        return Err(LauncherError::Custom("模组文件名无效".to_string()));
    }

    let path = mods_dir.join(file_name);
    if !path.is_file() {
        return Err(LauncherError::Custom(format!(
            "模组文件不存在: {}",
            file_name
        )));
    }
    Ok((mods_dir, path))
}

/// 从 jar 里读模组名和版本。Fabric / Quilt 走 JSON，Forge / NeoForge 走 mods.toml
///
/// 每次读都要打开 jar 解析中央目录；元数据只跟文件内容有关，「长度 + 修改时间」没变就返回上次结果
fn read_mod_metadata(path: &Path) -> (Option<String>, Option<String>) {
    let meta = match fs::metadata(path) {
        Ok(meta) => meta,
        Err(_) => return (None, None),
    };
    let modified = meta.modified().ok();
    let len = meta.len();

    if let Some(cached) = cached_mod_metadata(path, len, modified) {
        return cached;
    }

    let resolved = read_mod_metadata_uncached(path);
    remember_mod_metadata(path, len, modified, &resolved);
    resolved
}

/// 一个模组的展示名与版本
type ModMetadata = (Option<String>, Option<String>);

/// 缓存条目：文件指纹（长度 + 修改时间）与当时读到的元数据
struct CachedModMetadata {
    len: u64,
    modified: Option<std::time::SystemTime>,
    name: Option<String>,
    version: Option<String>,
}

/// 模组元数据缓存：路径 -> 指纹与元数据
///
/// 只省重复的 zip 解析，命中与否都不影响结果；指纹对不上就重读一遍
static MOD_METADATA_CACHE: std::sync::LazyLock<
    std::sync::Mutex<HashMap<PathBuf, CachedModMetadata>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(HashMap::new()));

/// 条目上限：一个实例的模组数远小于这个值，超了说明换过很多次实例，整体丢弃即可
const MOD_METADATA_CACHE_MAX: usize = 4096;

fn cached_mod_metadata(
    path: &Path,
    len: u64,
    modified: Option<std::time::SystemTime>,
) -> Option<ModMetadata> {
    let cache = MOD_METADATA_CACHE.lock().ok()?;
    let entry = cache.get(path)?;
    (entry.len == len && entry.modified == modified)
        .then(|| (entry.name.clone(), entry.version.clone()))
}

fn remember_mod_metadata(
    path: &Path,
    len: u64,
    modified: Option<std::time::SystemTime>,
    value: &ModMetadata,
) {
    let Ok(mut cache) = MOD_METADATA_CACHE.lock() else {
        return;
    };
    if cache.len() >= MOD_METADATA_CACHE_MAX && !cache.contains_key(path) {
        cache.clear();
    }
    cache.insert(
        path.to_path_buf(),
        CachedModMetadata {
            len,
            modified,
            name: value.0.clone(),
            version: value.1.clone(),
        },
    );
}

fn read_mod_metadata_uncached(path: &Path) -> ModMetadata {
    let file = match fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return (None, None),
    };
    let mut archive = match zip::ZipArchive::new(file) {
        Ok(a) => a,
        Err(_) => return (None, None),
    };

    for i in 0..archive.len() {
        let mut entry = match archive.by_index(i) {
            Ok(e) => e,
            Err(_) => continue,
        };
        let name = match entry.name() {
            Ok(n) => n.to_ascii_lowercase(),
            Err(_) => continue,
        };
        if name != "fabric.mod.json" && name != "quilt.mod.json" && !name.ends_with("mods.toml") {
            continue;
        }

        let mut text = String::new();
        if std::io::Read::read_to_string(&mut entry, &mut text).is_err() {
            continue;
        }

        if name.ends_with(".json") {
            if let Ok(json) = serde_json::from_str::<Value>(&text) {
                // quilt.mod.json 把字段嵌在 quilt_metadata.mod_metadata 下
                let meta = json
                    .get("quilt_metadata")
                    .and_then(|m| m.get("mod_metadata"))
                    .unwrap_or(&json);
                let display_name = meta.get("name").and_then(Value::as_str).map(str::to_string);
                let version = meta
                    .get("version")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                if display_name.is_some() {
                    return (display_name, version);
                }
            }
        } else if let Some(display_name) = parse_mods_toml_name(&text) {
            return (Some(display_name), None);
        }
    }

    (None, None)
}

/// 取 mods.toml 里第一条 displayName，缺省退化到 modId
fn parse_mods_toml_name(text: &str) -> Option<String> {
    let mut mod_id = None;
    // description 这类多行字符串的正文里可能正好有 `modId = "..."` 这样的行，
    // 所以整块正文要跳过，不能逐行当键值对解析
    let mut in_multiline: Option<char> = None;
    for line in text.lines() {
        let line = line.trim();
        if let Some(quote) = in_multiline {
            if line.contains(multiline_delimiter(quote)) {
                in_multiline = None;
            }
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        // mods.toml 里满是 [mods] 这类没有 '=' 的表头行，不能用 ? 退出整个函数
        let Some((key, rest)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim().trim_matches('"').trim();
        if key.is_empty() {
            continue;
        }
        let rest = rest.trim_start();
        // 多行字符串：同一行能闭合就取首行内容，否则跳过整块正文
        if let Some(quote) = multiline_delimiter_at(rest) {
            let delimiter = multiline_delimiter(quote);
            let body = &rest[delimiter.len()..];
            let closed = body.find(delimiter);
            if closed.is_none() {
                in_multiline = Some(quote);
            }
            let body = match closed {
                Some(end) => &body[..end],
                None => body,
            };
            if key == "displayName" {
                let value = body.lines().next().unwrap_or_default().trim();
                return if value.is_empty() {
                    None
                } else {
                    Some(value.to_string())
                };
            }
            continue;
        }
        // 值可能是 `"名字" #mandatory` 这种带行内注释的写法，注释不属于值
        let Some(value) = toml_string_value(rest) else {
            continue;
        };
        match key {
            "displayName" => return Some(value),
            "modId" if mod_id.is_none() => mod_id = Some(value),
            _ => {}
        }
    }
    mod_id
}

/// 多行字符串的定界符：三个双引号或三个单引号
fn multiline_delimiter(quote: char) -> &'static str {
    if quote == '"' { "\"\"\"" } else { "'''" }
}

/// 值是否以多行字符串的起始定界符开头
fn multiline_delimiter_at(value: &str) -> Option<char> {
    if value.starts_with("\"\"\"") {
        Some('"')
    } else if value.starts_with("'''") {
        Some('\'')
    } else {
        None
    }
}

/// 解析 TOML 值里的字符串：支持双引号/单引号、转义，
/// 引号外的 `#` 之后是注释。空串或解析不出内容时返回 None。
/// 多行字符串由调用方单独处理。
fn toml_string_value(rest: &str) -> Option<String> {
    let mut chars = rest.chars();
    match chars.next() {
        Some(quote @ ('"' | '\'')) => toml_quoted_string(chars.as_str(), quote),
        // 裸值（true、1.0 之类）同样按行内注释切
        _ => {
            let raw = strip_toml_comment(rest).trim();
            if raw.is_empty() {
                None
            } else {
                Some(raw.to_string())
            }
        }
    }
}

/// 读到与开引号配对的闭引号；引号内的 `#` 是正文，不是注释
fn toml_quoted_string(rest: &str, quote: char) -> Option<String> {
    let mut out = String::new();
    let mut chars = rest.chars();
    while let Some(c) = chars.next() {
        if quote == '"' && c == '\\' {
            let escaped = chars.next()?;
            match escaped {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'r' => out.push('\r'),
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                'u' | 'U' => {
                    let len = if escaped == 'u' { 4 } else { 8 };
                    let hex: String = chars.by_ref().take(len).collect();
                    let code = u32::from_str_radix(&hex, 16).ok()?;
                    out.push(char::from_u32(code)?);
                }
                // 其余转义（\b、\f 等）对展示名没有意义，原样保留
                other => {
                    out.push('\\');
                    out.push(other);
                }
            }
            continue;
        }
        if c == quote {
            return if out.is_empty() { None } else { Some(out) };
        }
        out.push(c);
    }
    // 没有闭合引号：多半是残缺行，宁可回退到 modId 也不吐出垃圾
    None
}

/// 切掉裸值后面的行内注释，引号内的 `#` 不算
fn strip_toml_comment(value: &str) -> &str {
    let mut in_single = false;
    let mut in_double = false;
    for (index, c) in value.char_indices() {
        match c {
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            '#' if !in_single && !in_double => return &value[..index],
            _ => {}
        }
    }
    value
}

/// 列出实例的模组，含被禁用的 *.jar.disabled
pub fn get_instance_mods(instance_name: String) -> Result<Vec<ModInfo>, LauncherError> {
    validate_instance_name_or_error(&instance_name)?;

    let (game_dir, _) = get_dirs()?;
    let mods_dir = launcher::isolation::content_root_for(&game_dir, &instance_name).join("mods");
    if !mods_dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut mods = Vec::new();
    for entry in fs::read_dir(&mods_dir)? {
        let path = entry?.path();
        if !path.is_file() {
            continue;
        }
        let file_name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        let enabled = file_name.ends_with(".jar");
        if !enabled && !file_name.ends_with(".jar.disabled") {
            continue;
        }

        let stem = file_name
            .strip_suffix(".disabled")
            .unwrap_or(&file_name)
            .strip_suffix(".jar")
            .unwrap_or(&file_name);
        let (meta_name, version) = read_mod_metadata(&path);
        let display_name = meta_name.unwrap_or_else(|| stem.to_string());
        mods.push(ModInfo {
            file_name,
            display_name,
            version,
            enabled,
            size: path.metadata().map(|m| m.len()).unwrap_or(0),
        });
    }

    // 比较器里每次都 to_lowercase 会为大列表分配上万个临时 String；
    // sort_by_cached_key 只算一次小写键
    mods.sort_by_cached_key(|m| m.display_name.to_lowercase());
    Ok(mods)
}

/// 启用/禁用模组：改名 *.jar <-> *.jar.disabled，这是各加载器的通用约定
pub async fn set_mod_enabled(
    instance_name: String,
    file_name: String,
    enabled: bool,
) -> Result<(), LauncherError> {
    validate_instance_name_or_error(&instance_name)?;
    let (game_dir, _) = get_dirs()?;
    let _instance_lock = acquire_instance_lock(&game_dir).await?;
    let (mods_dir, path) = resolve_mod_path(&instance_name, &file_name)?;

    let target_name = if enabled {
        match file_name.strip_suffix(".disabled") {
            Some(stripped) => stripped.to_string(),
            None => return Ok(()),
        }
    } else {
        match file_name.strip_suffix(".jar") {
            Some(_) => format!("{}.disabled", file_name),
            None => return Ok(()),
        }
    };

    let target = mods_dir.join(&target_name);
    if target.exists() {
        return Err(LauncherError::Custom(format!(
            "目标文件已存在: {}",
            target_name
        )));
    }
    fs::rename(&path, &target)?;
    info!("模组 {} -> {}", file_name, target_name);
    Ok(())
}

pub async fn delete_instance_mod(
    instance_name: String,
    file_name: String,
) -> Result<(), LauncherError> {
    validate_instance_name_or_error(&instance_name)?;
    let (game_dir, _) = get_dirs()?;
    let _instance_lock = acquire_instance_lock(&game_dir).await?;
    let (_, path) = resolve_mod_path(&instance_name, &file_name)?;
    fs::remove_file(&path)?;
    info!("已删除模组: {}", path.display());
    Ok(())
}

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

    let (auth_type, access_token, uuid) = match config.auth_type {
        crate::models::AuthType::Microsoft => {
            let at = "microsoft".to_string();
            let mut token = config.ms_access_token.clone();
            let uid = config.uuid.clone();
            let expires_at = config.ms_expires_at.unwrap_or(0);

            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);

            if now >= expires_at - 60 {
                info!("Microsoft token 已过期或即将过期，正在刷新...");
                if let Some(refresh_token) = config.ms_refresh_token.clone() {
                    match crate::services::microsoft_auth::refresh_and_authenticate(&refresh_token)
                        .await
                    {
                        Ok(auth_result) => {
                            config::update_config_async({
                                let auth = auth_result.clone();
                                move |cfg| {
                                    cfg.ms_access_token = Some(auth.access_token.clone());
                                    cfg.ms_refresh_token = Some(auth.refresh_token.clone());
                                    cfg.ms_expires_at = Some(auth.expires_at);
                                    cfg.username = Some(auth.username.clone());
                                    cfg.uuid = Some(auth.uuid.clone());
                                    Ok(())
                                }
                            })
                            .await?;

                            token = Some(auth_result.access_token);
                            info!("Microsoft token 刷新成功");
                        }
                        Err(e) => {
                            warn!("Microsoft token 刷新失败: {}, 将使用旧 token 启动", e);
                        }
                    }
                } else {
                    warn!("未找到 refresh_token，无法刷新 Microsoft token");
                }
            }

            (Some(at), token, uid)
        }
        crate::models::AuthType::Offline => (None, None, None),
    };

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
        access_token,
        uuid,
        override_java_path,
        force_java_mismatch,
    };

    launcher::launch_minecraft(launch_options, window).await
}

fn merge_forge_instance_json(
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

#[cfg(test)]
mod tests {
    use super::{get_instances_sync, lock_dir, parse_mods_toml_name, prune_stale_run_locks};
    use std::fs;

    /// 锁必须放在实例目录之外，否则 Windows 上目录内有打开的文件就无法重命名/删除实例
    #[test]
    fn test_run_locks_live_outside_instance_dirs() {
        let game = tempfile::TempDir::new().unwrap();
        let versions = game.path().join("versions");
        let instance = versions.join("1.20.1");
        fs::create_dir_all(&instance).unwrap();

        let lock_root = lock_dir(game.path());
        assert_eq!(lock_root, game.path().join(".ar1s"));
        assert!(!lock_root.starts_with(&versions));
        assert!(!instance.join(".natives.lock").exists());
    }

    #[test]
    fn test_prune_removes_run_locks_of_deleted_instances_only() {
        let game = tempfile::TempDir::new().unwrap();
        let versions = game.path().join("versions");
        fs::create_dir_all(versions.join("still-here")).unwrap();

        let lock_root = lock_dir(game.path());
        fs::create_dir_all(&lock_root).unwrap();
        fs::write(lock_root.join("run-still-here.lock"), b"").unwrap();
        fs::write(lock_root.join("run-gone.lock"), b"").unwrap();
        fs::write(lock_root.join("instances.lock"), b"").unwrap();

        prune_stale_run_locks(game.path(), &versions);

        assert!(lock_root.join("run-still-here.lock").exists());
        assert!(!lock_root.join("run-gone.lock").exists());
        assert!(lock_root.join("instances.lock").exists());
    }

    /// 有游戏进程持有共享锁时，清理逻辑绝不能把它的锁文件删掉
    #[test]
    fn test_prune_never_removes_a_held_run_lock() {
        let game = tempfile::TempDir::new().unwrap();
        let versions = game.path().join("versions");
        fs::create_dir_all(&versions).unwrap();
        let lock_root = lock_dir(game.path());
        fs::create_dir_all(&lock_root).unwrap();

        let path = lock_root.join("run-orphan.lock");
        let held =
            crate::utils::file_lock::acquire_shared(&path, std::time::Duration::from_millis(50))
                .expect("持有共享锁");

        // 实例目录不存在，但锁仍被持有：不能删
        prune_stale_run_locks(game.path(), &versions);
        assert!(path.exists(), "正在运行的实例的锁不能被清掉");

        drop(held);
        assert!(!path.exists(), "持有者释放后文件自动消失");
    }

    #[test]
    fn test_prune_is_a_noop_without_lock_dir() {
        let game = tempfile::TempDir::new().unwrap();
        let versions = game.path().join("versions");
        fs::create_dir_all(&versions).unwrap();

        prune_stale_run_locks(game.path(), &versions);
    }

    #[test]
    fn test_migrate_moves_resume_state_and_drops_old_locks() {
        let game = tempfile::TempDir::new().unwrap();
        let versions = game.path().join("versions");
        let instance = versions.join("1.20.1");
        fs::create_dir_all(&instance).unwrap();
        fs::write(instance.join(".natives.lock"), b"").unwrap();

        let legacy = game.path().join(".download_state");
        fs::create_dir_all(&legacy).unwrap();
        fs::write(legacy.join("1.20.1.json"), r#"{"completed_files":{}}"#).unwrap();
        fs::write(legacy.join("download.lock"), b"").unwrap();
        fs::write(legacy.join("run-1.20.1.lock"), b"").unwrap();

        super::migrate_legacy_state(game.path(), &versions);

        // 续传状态要搬过去，否则已经下好的文件会重下
        let migrated = lock_dir(game.path()).join("download_state/1.20.1.json");
        assert!(migrated.exists());
        assert!(!legacy.exists());
        assert!(!instance.join(".natives.lock").exists());
    }

    /// 关闭隔离后，实例卡片上的模组数必须来自共享根目录，
    /// 而不是实例目录里那些根本不会被加载的旧文件
    #[test]
    fn test_shared_mode_counts_mods_from_game_root() {
        let game = tempfile::TempDir::new().unwrap();
        let versions = game.path().join("versions");
        let instance = versions.join("1.20.1-Forge");
        fs::create_dir_all(&instance).unwrap();
        fs::write(
            instance.join("1.20.1-Forge.json"),
            r#"{"id":"1.20.1-Forge","inheritsFrom":"1.20.1","mainClass":"cpw.mods.bootstraplauncher.BootstrapLauncher"}"#,
        )
        .unwrap();

        // 实例目录里残留一个模组，共享目录里有两个
        fs::create_dir_all(instance.join("mods")).unwrap();
        fs::write(instance.join("mods/stale.jar"), b"x").unwrap();
        fs::create_dir_all(game.path().join("mods")).unwrap();
        for name in ["a.jar", "b.jar"] {
            fs::write(game.path().join("mods").join(name), b"x").unwrap();
        }

        let isolated = get_instances_sync(&versions, true).unwrap();
        assert_eq!(isolated[0].mod_count, Some(1), "隔离时应看实例目录");

        let shared = get_instances_sync(&versions, false).unwrap();
        assert_eq!(shared[0].mod_count, Some(2), "关闭隔离时应看根目录");
    }

    #[test]
    fn test_base_and_loader_versions_are_both_listed() {
        let game = tempfile::TempDir::new().unwrap();
        let versions = game.path().join("versions");

        fs::create_dir_all(versions.join("test-base")).unwrap();
        fs::write(
            versions.join("test-base/test-base.json"),
            r#"{"id":"test-base","mainClass":"net.minecraft.client.main.Main"}"#,
        )
        .unwrap();

        fs::create_dir_all(versions.join("test-forge")).unwrap();
        fs::write(
            versions.join("test-forge/test-forge.json"),
            r#"{"id":"test-forge","inheritsFrom":"test-base","mainClass":"cpw.mods.bootstraplauncher.BootstrapLauncher"}"#,
        )
        .unwrap();

        let list = get_instances_sync(&versions, true).unwrap();

        assert_eq!(list.len(), 2);
        assert!(list.iter().any(|instance| instance.id == "test-base"));
        assert!(list.iter().any(|instance| instance.id == "test-forge"));
    }

    #[test]
    fn test_mods_toml_name_survives_table_headers() {
        let text = "modLoader=\"javafml\"\nloaderVersion=\"[47,)\"\nlicense=\"MIT\"\n\n[[mods]]\nmodId=\"examplemod\"\nversion=\"1.0\"\ndisplayName=\"示例模组\"\n";
        assert_eq!(parse_mods_toml_name(text).as_deref(), Some("示例模组"));
    }

    #[test]
    fn test_mods_toml_name_falls_back_to_mod_id() {
        let text = "[[mods]]\nmodId=\"examplemod\"\n";
        assert_eq!(parse_mods_toml_name(text).as_deref(), Some("examplemod"));
    }

    #[test]
    fn test_mods_toml_name_skips_empty_display_name() {
        let text = "[[mods]]\nmodId=\"examplemod\"\ndisplayName=\"\"\n";
        assert_eq!(parse_mods_toml_name(text).as_deref(), Some("examplemod"));
    }

    /// Forge MDK 模板到处是 `#mandatory`，行内注释不能被当成名字的一部分
    #[test]
    fn test_mods_toml_name_strips_inline_comment() {
        let text = "[[mods]] #mandatory\nmodId = \"opack2reload\" #mandatory\ndisplayName = \"0Pack2Reload\" #mandatory\n";
        assert_eq!(parse_mods_toml_name(text).as_deref(), Some("0Pack2Reload"));
    }

    #[test]
    fn test_mods_toml_name_keeps_hash_inside_quotes() {
        let text = "[[mods]]\nmodId=\"examplemod\"\ndisplayName=\"Mod #1\" #mandatory\n";
        assert_eq!(parse_mods_toml_name(text).as_deref(), Some("Mod #1"));
    }

    #[test]
    fn test_mods_toml_name_reads_literal_strings_and_escapes() {
        // 单引号是字面串，不处理转义
        let text = "[[mods]]\nmodId='examplemod'\ndisplayName='Literal \\\" Name'\n";
        assert_eq!(
            parse_mods_toml_name(text).as_deref(),
            Some("Literal \\\" Name")
        );

        // 双引号串里的转义要还原
        let text = "[[mods]]\nmodId=\"examplemod\"\ndisplayName=\"A \\\"quoted\\\" mod\"\n";
        assert_eq!(
            parse_mods_toml_name(text).as_deref(),
            Some("A \"quoted\" mod")
        );
    }

    #[test]
    fn test_mods_toml_name_reads_single_line_multiline_string() {
        let text = "[[mods]]\nmodId=\"examplemod\"\ndisplayName=\"\"\"Example\"\"\"\n";
        assert_eq!(parse_mods_toml_name(text).as_deref(), Some("Example"));
    }

    /// description 的多行正文里可能写着看起来像键值的行，不能拿来当名字
    #[test]
    fn test_mods_toml_name_ignores_multiline_description_body() {
        let text = "[[mods]]\nmodId=\"examplemod\"\ndisplayName=\"Example\"\ndescription='''\nmodId=\"bogus\"\ndisplayName=\"Bogus\"\n'''\n";
        assert_eq!(parse_mods_toml_name(text).as_deref(), Some("Example"));
    }

    #[test]
    fn test_mods_toml_name_skips_commented_out_display_name() {
        let text = "[[mods]]\nmodId=\"examplemod\"\n#displayName=\"Bogus\" #optional\n";
        assert_eq!(parse_mods_toml_name(text).as_deref(), Some("examplemod"));
    }
}
