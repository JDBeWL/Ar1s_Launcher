//! Minecraft 启动流程模块
//!
//! 串起 token 刷新、隔离目录、natives 解压、classpath、参数拼装、Java 匹配与进程监控

mod arguments;
mod classpath;
pub mod crash_diagnosis;
pub mod isolation;
pub mod java;
pub mod java_requirement;
mod natives;
mod process;
mod version_json;

use crate::errors::LauncherError;
use crate::models::LaunchOptions;
use crate::services::config::{
    acquire_auth_lock, load_config, reload_config_from_disk, update_config_async,
};
use crate::services::memory::{is_memory_setting_safe, optimize_jvm_memory_args, plan_auto_memory};
use crate::utils::file_utils;
use log::{info, warn};
use std::path::PathBuf;
use tauri::Emitter;

pub use classpath::find_library_jar;

/// 毫秒时间戳，与 config::update_instance_last_played 用的是同一套口径
fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub async fn launch_minecraft(
    options: LaunchOptions,
    window: tauri::Window,
) -> Result<(), LauncherError> {
    let emit = |event: &str, msg: String| {
        let _ = window.emit(event, msg);
    };

    let mut options = options;
    file_utils::validate_version_id_or_error(&options.version)?;

    if options.auth_type.as_deref() == Some("microsoft") {
        // token 刷新只走这一个入口：调用方手里的 token 可能早于别的进程刚完成的轮换
        options.access_token = None;

        // 多实例同时启动时只允许一个刷新，后到的实例直接复用轮换后的 token
        let auth_lock = match acquire_auth_lock().await {
            Ok(lock) => Some(lock),
            Err(e) => {
                warn!("获取 token 刷新锁失败: {}", e);
                None
            }
        };
        let cfg = reload_config_from_disk()?;

        if cfg.auth_type == crate::models::AuthType::Microsoft {
            let expires_at = cfg.ms_expires_at.unwrap_or(0);
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            let needs_refresh = now >= expires_at - 60;

            options.access_token = cfg.ms_access_token.clone();
            if options.uuid.is_none() {
                options.uuid = cfg.uuid.clone();
            }
            if let Some(username) = cfg.username.clone().filter(|name| !name.trim().is_empty()) {
                options.username = username;
            }

            if needs_refresh && auth_lock.is_some() {
                if let Some(refresh_token) = cfg.ms_refresh_token.clone() {
                    match crate::services::microsoft_auth::refresh_and_authenticate(&refresh_token)
                        .await
                    {
                        Ok(auth_result) => {
                            // 网络请求期间另一个实例可能已经刷新过；token 变了就不要覆盖
                            let applied = update_config_async({
                                let auth = auth_result.clone();
                                move |save_cfg| {
                                    if save_cfg.ms_refresh_token.as_deref()
                                        != Some(refresh_token.as_str())
                                    {
                                        return Ok(false);
                                    }
                                    save_cfg.ms_access_token = Some(auth.access_token.clone());
                                    save_cfg.ms_refresh_token = Some(auth.refresh_token.clone());
                                    save_cfg.ms_expires_at = Some(auth.expires_at);
                                    save_cfg.username = Some(auth.username.clone());
                                    save_cfg.uuid = Some(auth.uuid.clone());
                                    save_cfg.skin_url = auth.skin_url.clone();
                                    Ok(true)
                                }
                            })
                            .await?;

                            if applied {
                                options.access_token = Some(auth_result.access_token);
                                options.username = auth_result.username;
                                if options.uuid.is_none() {
                                    options.uuid = Some(auth_result.uuid);
                                }
                                info!("Microsoft token 刷新成功");
                            } else {
                                let current = reload_config_from_disk()?;
                                options.access_token = current.ms_access_token.clone();
                                if options.uuid.is_none() {
                                    options.uuid = current.uuid.clone();
                                }
                                if let Some(username) = current
                                    .username
                                    .clone()
                                    .filter(|name| !name.trim().is_empty())
                                {
                                    options.username = username;
                                }
                                info!("另一个实例已完成 Microsoft token 刷新");
                            }
                        }
                        Err(e) => {
                            warn!("Microsoft token 刷新失败: {}, 将使用旧 token", e);
                        }
                    }
                } else {
                    warn!("未找到 refresh_token，无法刷新 Microsoft token");
                }
            } else if needs_refresh {
                warn!("token 已过期，但刷新锁被占用，沿用旧 token 启动");
            }
        } else {
            warn!("配置中的登录方式不是 Microsoft，跳过 token 刷新");
        }
    }

    let uuid = if options.auth_type.as_deref() == Some("microsoft") {
        options
            .uuid
            .clone()
            .unwrap_or_else(|| java::generate_offline_uuid(&options.username))
    } else {
        java::generate_offline_uuid(&options.username)
    };

    // 用户名、UUID、上次启动时间与「上次选择的版本」都在同一个配置文件里，
    // 合成一次读改写：分三次 update_config 就是三次整文件读盘 + 三次全量写 + 三次 icacls 派生
    let launch_stamp = now_millis();
    let launch_version = options.version.clone();
    let launch_username = options.username.clone();
    let launch_uuid = uuid.clone();
    update_config_async(move |config| {
        config.username = Some(launch_username);
        config.uuid = Some(launch_uuid);
        config
            .instance_last_played
            .insert(launch_version.clone(), launch_stamp);
        config.last_selected_version = Some(launch_version);
        Ok(())
    })
    .await?;
    let config = (*load_config()?).clone();

    let game_dir = java::resolve_game_dir(&config.game_dir);
    let version_dir = file_utils::version_dir(&game_dir.join("versions"), &options.version)?;

    // 共享运行锁贯穿启动准备和游戏进程：同一实例可多开，但删除/重命名必须等全部退出
    let run_lock = crate::services::instance::acquire_run_shared_lock(&game_dir, &options.version)
        .await
        .map_err(|e| match e {
            LauncherError::Busy(_) => LauncherError::Custom(format!(
                "实例 '{}' 正在被删除或重命名，请稍后再启动",
                options.version
            )),
            other => other,
        })?;

    // Quilt 老版本自带的 ASM 读不了新游戏的类，这种实例放它启动只会秒崩；
    // 能就地升级就升级，升级失败也不拦启动，交给崩溃诊断去解释
    match crate::services::loaders::quilt::repair_if_unusable(&options.version, &game_dir, &window)
        .await
    {
        Ok(Some(message)) => emit("log-warning", message),
        Ok(None) => {}
        Err(e) => emit("log-warning", format!("检查 Quilt 加载器版本时出错: {}", e)),
    }

    emit("log-debug", format!("尝试启动版本: {}", options.version));
    emit("log-debug", format!("游戏目录: {}", game_dir.display()));

    // 版本 JSON 要沿 inheritsFrom 递归读盘解析，是纯同步活
    let version_json = {
        let game_dir_clone = game_dir.clone();
        let version = options.version.clone();
        tokio::task::spawn_blocking(move || {
            version_json::load_and_merge_version_json(&game_dir_clone, &version)
        })
        .await
        .map_err(|e| LauncherError::Custom(format!("读取版本 JSON 任务异常终止: {}", e)))??
    };

    let libraries_base_dir = game_dir.join("libraries");
    let assets_base_dir = game_dir.join("assets");

    emit(
        "log-debug",
        format!("库文件目录: {}", libraries_base_dir.display()),
    );
    emit(
        "log-debug",
        format!("资源文件目录: {}", assets_base_dir.display()),
    );

    // Minecraft 版本 JSON 用 "osx" 而非 Rust 的 "macos"
    let current_os = if std::env::consts::OS == "macos" {
        "osx"
    } else {
        std::env::consts::OS
    };

    let content_root = isolation::prepare_content_root(&config, &game_dir, &options.version)?;
    if !config.version_isolation {
        emit(
            "log-warning",
            format!(
                "版本隔离已关闭：游戏工作目录是 {}，所有实例共用同一份 mods / 存档 / 配置",
                content_root.display()
            ),
        );

        // 关掉隔离后实例目录里的内容不会再生效，明确指出来，免得以为模组丢了
        let stale_mods = version_dir.join("mods");
        let stale_count = java_requirement::list_mod_files(&stale_mods).len();
        if stale_count > 0 {
            emit(
                "log-warning",
                format!(
                    "实例目录下的 {} 个模组不会被加载（{}），需要的话请移到 {}/mods",
                    stale_count,
                    stale_mods.display(),
                    content_root.display()
                ),
            );
        }

        let stale_saves = version_dir.join("saves");
        let has_saves = std::fs::read_dir(&stale_saves)
            .map(|mut entries| entries.next().is_some())
            .unwrap_or(false);
        if has_saves {
            emit(
                "log-warning",
                format!(
                    "该实例的存档在 {}，关闭隔离后游戏读不到；请把世界文件夹移到 {}/saves",
                    stale_saves.display(),
                    content_root.display()
                ),
            );
        }
    }

    let natives_dir = natives::extract_natives(
        &version_json,
        &game_dir,
        &version_dir,
        &libraries_base_dir,
        current_os,
        &window,
    )
    .await?;

    // classpath 要为每个库 stat 一次，LaunchWrapper 预检还会整树递归找 jar，同样是同步活
    let (classpath, main_class) = {
        let version_json = version_json.clone();
        let libraries_base_dir = libraries_base_dir.clone();
        let version_dir = version_dir.clone();
        let version = options.version.clone();
        let window = window.clone();
        tokio::task::spawn_blocking(move || -> Result<(Vec<PathBuf>, String), LauncherError> {
            let emit = |event: &str, msg: String| {
                let _ = tauri::Emitter::emit(&window, event, msg);
            };
            let mut classpath = classpath::build_classpath(
                &version_json,
                &libraries_base_dir,
                &version_dir,
                &version,
                &emit,
            )?;

            let main_class = version_json["mainClass"]
                .as_str()
                .ok_or_else(|| LauncherError::Custom("无法在json中找到mainClass".to_string()))?
                .to_string();

            if main_class == "net.minecraft.launchwrapper.Launch" {
                classpath::precheck_launchwrapper_libraries(
                    &mut classpath,
                    &libraries_base_dir,
                    &emit,
                )?;
            }

            Ok((classpath, main_class))
        })
        .await
        .map_err(|e| LauncherError::Custom(format!("构建 classpath 任务异常终止: {}", e)))??
    };

    let main_class = main_class.as_str();

    let assets_index = version_json["assetIndex"]["id"]
        .as_str()
        .unwrap_or(&options.version);

    // build_arguments 会打开多个 jar 判 F3 实现、读 classpath 文件，也是同步活
    let (jvm_args, game_args_vec) = {
        let version_json = version_json.clone();
        let config = config.clone();
        let options_clone = LaunchOptions {
            version: options.version.clone(),
            username: options.username.clone(),
            memory: options.memory,
            window_width: options.window_width,
            window_height: options.window_height,
            fullscreen: options.fullscreen,
            auth_type: options.auth_type.clone(),
            access_token: options.access_token.clone(),
            uuid: options.uuid.clone(),
            override_java_path: options.override_java_path.clone(),
            force_java_mismatch: options.force_java_mismatch,
        };
        let uuid = uuid.clone();
        let version_dir = version_dir.clone();
        let game_dir = game_dir.clone();
        let assets_base_dir = assets_base_dir.clone();
        let assets_index = assets_index.to_string();
        let classpath = classpath.clone();
        let window = window.clone();
        tokio::task::spawn_blocking(move || {
            let emit = |event: &str, msg: String| {
                let _ = tauri::Emitter::emit(&window, event, msg);
            };
            arguments::build_arguments(
                &version_json,
                &config,
                &options_clone,
                &uuid,
                &version_dir,
                &game_dir,
                &assets_base_dir,
                &assets_index,
                &classpath,
                &emit,
            )
        })
        .await
        .map_err(|e| LauncherError::Custom(format!("构建启动参数任务异常终止: {}", e)))?
    };

    let profile = java::profile_for(&options.version, &game_dir);

    let mut java_path = if let Some(ref override_path) = options.override_java_path {
        emit(
            "log-info",
            format!("使用临时指定的 Java: {}", override_path),
        );
        override_path.clone()
    } else if config.auto_match_java {
        // 候选装配可能触发全盘扫描 + 起 JVM 子进程探测（冷缓存时秒级），不能占着 async worker
        let profile_for_match = profile.clone();
        let matched =
            tokio::task::spawn_blocking(move || java::auto_match_java(&profile_for_match))
                .await
                .map_err(|e| LauncherError::Custom(format!("Java 自动匹配任务异常终止: {}", e)))?;
        if let Some(matched) = matched {
            emit(
                "log-info",
                format!("自动匹配 Java: {} (版本 {})", matched.path, matched.version),
            );
            if let Some(ref warning) = matched.warning {
                emit("log-warning", warning.clone());
            }
            matched.path
        } else {
            emit(
                "log-warning",
                "自动匹配 Java 失败，使用配置中的 Java 路径".to_string(),
            );
            java::resolve_java_path(&config)?
        }
    } else {
        java::resolve_java_path(&config)?
    };

    // 单次 JVM 子进程探测，冷缓存约几百毫秒，进阻塞池
    let java_path_for_detect = java_path.clone();
    let mut java_version =
        tokio::task::spawn_blocking(move || java::detect_java_version(&java_path_for_detect))
            .await
            .map_err(|e| LauncherError::Custom(format!("Java 版本探测任务异常终止: {}", e)))?;

    // 三条分支选出来的 Java 都要过同一道判据；版本 JSON 读不到时不硬拦
    if profile.resolved
        && let Some(ver) = java_version
        && !java_requirement::java_allowed(&profile.requirement, ver)
    {
        let config_for_candidates = config.clone();
        let candidates =
            tokio::task::spawn_blocking(move || java::collect_candidates(&config_for_candidates))
                .await
                .map_err(|e| LauncherError::Custom(format!("Java 候选扫描任务异常终止: {}", e)))?;

        match java::resolve_gate(
            &profile,
            (java_path.as_str(), ver),
            options.force_java_mismatch,
            &candidates,
        ) {
            // 用户已经在弹窗里点过“仍用当前 Java”：这里再替他换一份，等于那个选项没生效
            java::JavaGate::KeepCurrent(report) => {
                emit(
                    "log-warning",
                    format!(
                        "按你的选择继续用 Java {} 启动（{}），风险自负：\n{}",
                        ver, java_path, report
                    ),
                );
            }
            java::JavaGate::SwitchTo(better) => {
                emit(
                    "log-warning",
                    format!(
                        "{} 实例要求 Java {}{}，当前 Java {} 不满足，已换成 Java {} ({})",
                        profile.loader.label(),
                        profile.requirement.min,
                        match profile.requirement.hard_max {
                            Some(max) => format!("~{}", max),
                            None => "+".to_string(),
                        },
                        ver,
                        better.version,
                        better.path
                    ),
                );
                java_path = better.path;
                java_version = Some(better.version);
            }
            java::JavaGate::Refuse(report) => {
                emit("log-error", report.clone());
                return Err(LauncherError::Custom(report));
            }
        }
    }

    emit("log-debug", format!("使用的Java路径: {}", java_path));
    if let Some(v) = java_version {
        emit("log-debug", format!("检测到Java版本: {}", v));
    } else {
        emit(
            "log-warning",
            "无法检测Java版本，将使用默认JVM参数".to_string(),
        );
    }

    let java_path_for_probe = java_path.clone();
    let java_is_32bit = tokio::task::spawn_blocking(move || {
        crate::services::java::probe_java(&java_path_for_probe)
            .is_some_and(|probe| probe.arch == crate::services::java::JavaArch::Bits32)
    })
    .await
    .map_err(|e| LauncherError::Custom(format!("Java 探测任务异常终止: {}", e)))?;

    let lwjgl_lib_path = natives_dir.to_string_lossy().to_string();
    let memory_mb = if config.auto_memory_enabled {
        let plan = plan_auto_memory(
            &options.version,
            profile.loader != java_requirement::LoaderKind::Vanilla,
            profile.mods.len(),
            java_is_32bit,
        );
        emit(
            "log-info",
            plan.describe(&options.version, profile.loader.label()),
        );
        // 只有实例需求都没满足才值得警告；低于 4GB 默认目标但够用时不打扰
        if plan.need_mb > plan.memory_mb {
            emit(
                "log-warning",
                format!(
                    "按实例规模约需 {} MB，但当前可用内存最多分配 {} MB；若卡顿或内存不足，请关闭其他程序或在设置中改用手动内存",
                    plan.need_mb, plan.memory_mb
                ),
            );
        }
        plan.memory_mb
    } else {
        options.memory.unwrap_or(2048)
    };

    if let Err(e) = is_memory_setting_safe(memory_mb) {
        emit("log-warning", format!("内存设置警告: {}", e));
    }

    let mut final_args = optimize_jvm_memory_args(memory_mb, java_version, &config.jvm_advanced);

    final_args.extend([
        format!("-Djava.library.path={}", lwjgl_lib_path),
        format!("-Dorg.lwjgl.librarypath={}", lwjgl_lib_path),
        "-Dfile.encoding=UTF-8".to_string(),
        "-Dorg.lwjgl.openal.mapping.use=false".to_string(),
    ]);
    final_args.extend(jvm_args);

    let classpath_str = classpath
        .iter()
        .map(|p| p.to_string_lossy())
        .collect::<Vec<_>>()
        .join(if cfg!(windows) { ";" } else { ":" });

    emit("log-debug", format!("最终Classpath: {}", classpath_str));

    final_args.push("-cp".to_string());
    final_args.push(classpath_str);
    final_args.push(main_class.to_string());
    final_args.extend(game_args_vec);

    // 版本 JSON 的 has_custom_resolution 规则可能已注入 --width/--height；Forge 只取第一个 --width，重复传参会让用户设置被占位符挤掉
    let has_arg = |flag: &str| final_args.iter().any(|a| a == flag);
    let width_present = has_arg("--width");
    let height_present = has_arg("--height");

    if !width_present && let Some(width) = options.window_width.or(config.window_width) {
        final_args.push("--width".to_string());
        final_args.push(width.to_string());
    }
    if !height_present && let Some(height) = options.window_height.or(config.window_height) {
        final_args.push("--height".to_string());
        final_args.push(height.to_string());
    }

    // 未替换的 ${...} 占位符会让游戏直接崩溃，这里提前告警
    if let Some(unresolved) = final_args.iter().find(|a| a.contains("${")) {
        emit(
            "log-warning",
            format!("启动参数中检测到未替换的占位符: {}", unresolved),
        );
    }
    if options.fullscreen.unwrap_or(config.fullscreen) {
        final_args.push("--fullscreen".to_string());
    }

    let working_dir = isolation::content_root_of(&config, &game_dir, &version_dir);

    if let Some(ref lang) = config.language
        && let Err(e) = crate::utils::minecraft::set_game_language(&working_dir, lang)
    {
        emit("log-warning", format!("无法设置游戏语言: {}", e));
    }

    let mods_root = java_requirement::mods_dir(
        &profile.game_dir,
        &options.version,
        config.version_isolation,
    );
    let launch_ctx = crash_diagnosis::LaunchContext {
        version_id: options.version.clone(),
        loader: profile.loader.label().to_string(),
        declared_java: profile.declared_java,
        used_java_path: java_path.clone(),
        used_java_version: java_version,
        memory_mb,
        mod_files: java_requirement::list_mod_files(&mods_root),
        runtime_secs: None,
    };

    process::spawn_and_monitor_process(
        &java_path,
        final_args,
        &working_dir,
        launch_ctx,
        window,
        run_lock,
    )
}
