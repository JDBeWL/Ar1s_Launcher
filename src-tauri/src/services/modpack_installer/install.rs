//! 执行安装
//!
//! 解压 → 铺 overrides → 补文件 → 装游戏与加载器 → 写 instance.json；
//! 中途失败或被取消时按 `finalize_install` 回滚这次产生的文件

use super::plan::{InstallTargets, ModpackPlan};
use super::progress::progress_emitter;
use super::{
    MAX_EXTRACTED_BYTES, ModpackInstallProgress, ModpackInstaller, check_cancelled, get_cancel_flag,
};
use crate::errors::LauncherError;
use crate::services::modpack_format::{self, ModrinthDependencies, ModrinthIndexFile};
use crate::services::{config, download, loaders};
use crate::utils::file_utils::{self};
use log::{info, warn};
use std::fs;
use std::path::Path;
use tauri::Emitter;

/// 解压 → 复制 overrides → 下载清单文件 → 装游戏与加载器 → 写 instance.json
impl ModpackInstaller {
    pub(super) async fn install_plan(
        &self,
        plan: &ModpackPlan,
        instance_name: &str,
        targets: &InstallTargets,
        window: &tauri::Window,
    ) -> Result<(), LauncherError> {
        let send_progress = progress_emitter(window);

        // 解压 overrides 和下载清单文件都要写盘；先按「还缺多少」预检一次，
        // 免得解压到一半才发现没空间，留下半成品目录
        let missing_bytes = plan
            .index
            .files
            .iter()
            .filter(|file| modpack_format::is_client_file(file))
            .map(|file| {
                let existing = fs::metadata(targets.content_root.join(&file.path))
                    .map(|meta| meta.len())
                    .unwrap_or(0);
                file.file_size.unwrap_or(0).saturating_sub(existing)
            })
            .sum::<u64>()
            .saturating_add(
                fs::metadata(&plan.archive_path)
                    .map(|meta| meta.len())
                    .unwrap_or(0),
            );
        crate::utils::disk_space::ensure_free_space(&targets.game_dir, missing_bytes)?;

        send_progress(35, "解压整合包...", false);
        check_cancelled()?;

        if targets.extract_dir.exists() {
            // 删除整个解压临时目录耗时不可控，不能放在 async worker 上
            let extract_dir_owned = targets.extract_dir.clone();
            tokio::task::spawn_blocking(move || fs::remove_dir_all(&extract_dir_owned))
                .await
                .map_err(|e| LauncherError::Custom(format!("清理旧目录失败: {}", e)))?
                .map_err(|e| LauncherError::Custom(format!("清理旧目录失败: {}", e)))?;
        }
        fs::create_dir_all(&targets.extract_dir)?;

        // 解压是纯同步阻塞活，几百 MB 的包会占住 async worker 几十秒：
        // 期间进度心跳和取消响应都会跟着卡住，必须挪到阻塞线程池
        let archive = plan.archive_path.clone();
        let extract_dir = targets.extract_dir.clone();
        tokio::task::spawn_blocking(move || {
            modpack_format::extract_archive(&archive, &extract_dir, MAX_EXTRACTED_BYTES)
        })
        .await
        .map_err(|e| LauncherError::Custom(format!("解压任务异常终止: {}", e)))?
        .map_err(|e| LauncherError::Custom(format!("解压整合包失败: {}", e)))?;

        send_progress(45, "处理整合包配置...", false);
        check_cancelled()?;

        fs::create_dir_all(&targets.instance_dir)?;

        send_progress(50, "复制整合包文件...", false);
        check_cancelled()?;

        let extract_dir = targets.extract_dir.clone();
        let content_root = targets.content_root.clone();
        tokio::task::spawn_blocking(move || copy_overrides(&extract_dir, &content_root))
            .await
            .map_err(|e| LauncherError::Custom(format!("复制任务异常终止: {}", e)))??;

        let client_files: Vec<ModrinthIndexFile> = plan
            .index
            .files
            .iter()
            .filter(|file| modpack_format::is_client_file(file))
            .cloned()
            .collect();

        send_progress(55, "下载模组文件与游戏核心...", false);
        check_cancelled()?;

        // 模组下载和「游戏核心 + 加载器」互不依赖，串行只是白等网络；两段各用自己的信号量限流。
        //
        // 进度区间必须错开：模组占 55..74，加载器阶段只在 74 上报不确定态，
        // 否则核心先装完会把进度推到 80，模组随后又报 62，进度条就倒退了
        let mods_future = self.download_modpack_files(&client_files, &targets.content_root, window);
        let core_future = self.install_game_and_loader(
            &plan.index.dependencies,
            instance_name,
            &targets.game_dir,
            window,
        );

        let (mods_result, core_result) = tokio::join!(mods_future, core_future);
        // 先报核心的错：模组缺失还能靠修复补，核心或加载器没装好是完全跑不起来的
        core_result?;
        mods_result?;

        send_progress(90, "创建实例配置...", false);
        check_cancelled()?;

        let instance_config = serde_json::json!({
            "id": instance_name,
            "name": plan.display_name,
            "type": "modpack",
            "source": plan.source,
            "modpack_id": plan.modpack_id,
            "modpack_version": plan.version_label,
            "minecraft": plan.index.dependencies.minecraft,
            "loader": plan.index.dependencies.loader_slug(),
            "icon_url": plan.icon_url,
            "loaders": plan.loaders,
            "created": chrono::Utc::now().to_rfc3339(),
        });

        let config_path = targets.instance_dir.join("instance.json");
        fs::write(config_path, serde_json::to_string_pretty(&instance_config)?)?;

        if targets.extract_dir.exists() {
            // 可能是几 GB 的 overrides 解压目录，删除耗时不可控，和上面的清理一样必须离开 async worker
            let extract_dir = targets.extract_dir.clone();
            let _ = tokio::task::spawn_blocking(move || fs::remove_dir_all(&extract_dir)).await;
        }

        send_progress(100, "整合包安装完成！", false);
        info!("整合包 {} 安装完成", instance_name);

        if let Ok(game_dir_str) = config::get_game_dir() {
            let _ = window.emit("game-dir-changed", &game_dir_str);
        }

        Ok(())
    }

    async fn install_game_and_loader(
        &self,
        deps: &ModrinthDependencies,
        instance_name: &str,
        game_dir: &Path,
        window: &tauri::Window,
    ) -> Result<(), LauncherError> {
        let mc_version = &deps.minecraft;
        info!("安装 Minecraft {}", mc_version);
        let cancel = get_cancel_flag();

        let config = config::load_config()?;
        let core_result = download::ensure_version_core(
            mc_version.clone(),
            config.download_mirror.clone(),
            window,
        )
        .await;
        // 下载被取消时错误文案来自下载模块，这里统一成安装取消
        check_cancelled()?;
        core_result?;

        let loader = deps.loader();
        if loader.is_some() {
            // 74 而不是 80：和并行跑的模组下载（55..74）错开，进度条才不会倒退
            let _ = window.emit(
                "modpack-install-progress",
                ModpackInstallProgress {
                    progress: 74,
                    message: "安装加载器...".to_string(),
                    indeterminate: true,
                },
            );
        }

        match loader {
            Some(loaded) => {
                info!("安装 {} {}", loaded.name(), loaded.loader_version());
                let result = loaders::install_loader(
                    &loaded,
                    instance_name,
                    game_dir,
                    Some(cancel.as_ref()),
                )
                .await;
                check_cancelled()?;
                result?;
            }
            // 无加载器依赖时手动补一份版本 JSON
            None => self.create_vanilla_version_json(mc_version, instance_name, game_dir)?,
        }

        // 加载器安装器只负责自己的文件；Fabric/Quilt 的 profile 库需要在这里补
        let instance_json = fs::read_to_string(
            game_dir
                .join("versions")
                .join(instance_name)
                .join(format!("{}.json", instance_name)),
        )
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok());
        if let Some(json) = instance_json
            && let Err(e) = download::ensure_declared_libraries(&json, window).await
        {
            if e.is_cancelled() {
                return Err(e);
            }
            warn!("加载器库文件没补全，首次启动前可能需要修复: {}", e);
        }
        // download_all_files 会重置自己的取消标志，这里再按整合包取消标志收口
        check_cancelled()?;

        Ok(())
    }

    fn create_loader_version_json(
        &self,
        instance_name: &str,
        inherits_from: &str,
        game_dir: &Path,
    ) -> Result<(), LauncherError> {
        let version_dir = game_dir.join("versions").join(instance_name);
        fs::create_dir_all(&version_dir)?;

        let json_path = version_dir.join(format!("{}.json", instance_name));

        if json_path.exists() {
            return Ok(());
        }

        let version_json = serde_json::json!({
            "id": instance_name,
            "inheritsFrom": inherits_from,
            "type": "release"
        });

        fs::write(&json_path, serde_json::to_string_pretty(&version_json)?)?;
        info!("创建版本 JSON: {}", json_path.display());

        Ok(())
    }

    fn create_vanilla_version_json(
        &self,
        mc_version: &str,
        instance_name: &str,
        game_dir: &Path,
    ) -> Result<(), LauncherError> {
        self.create_loader_version_json(instance_name, mc_version, game_dir)
    }
}

/// 与实例创建/删除/重命名共用一把锁；失败清理会删实例目录，不能和另一实例互相踩
pub(super) async fn acquire_install_lock(
    targets: &InstallTargets,
) -> Result<crate::utils::file_lock::FileLock, LauncherError> {
    crate::utils::file_lock::acquire_async(
        &crate::services::instance::lock_dir(&targets.game_dir).join("instances.lock"),
        std::time::Duration::from_secs(2),
    )
    .await
}

pub(super) fn ensure_instance_free(
    targets: &InstallTargets,
    instance_name: &str,
) -> Result<(), LauncherError> {
    if targets.instance_dir.exists() {
        return Err(LauncherError::Custom(format!(
            "名为 '{}' 的实例已存在，请使用其他名称",
            instance_name
        )));
    }
    Ok(())
}

/// 失败或被取消时回滚这次安装产生的文件
pub(super) async fn finalize_install<T>(
    targets: &InstallTargets,
    result: Result<T, LauncherError>,
) -> Result<T, LauncherError> {
    if result.is_err() {
        info!("安装失败或被取消，清理已创建的文件...");

        if targets.instance_dir.exists() {
            // 整个实例目录（可能含 GB 级整合包内容）的删除耗时不可控，不能占着 async worker
            let instance_dir = targets.instance_dir.clone();
            match tokio::task::spawn_blocking(move || fs::remove_dir_all(&instance_dir)).await {
                Ok(Ok(())) => info!("已清理实例目录: {}", targets.instance_dir.display()),
                Ok(Err(e)) => warn!("清理实例目录失败: {}", e),
                Err(e) => warn!("清理实例目录任务异常终止: {}", e),
            }
        }

        if targets.extract_dir.exists() {
            let extract_dir = targets.extract_dir.clone();
            let _ = tokio::task::spawn_blocking(move || fs::remove_dir_all(&extract_dir)).await;
        }
    }

    result
}

/// overrides 与 client-overrides 都是要原样铺到实例内容目录的
fn copy_overrides(extract_dir: &Path, content_root: &Path) -> Result<(), LauncherError> {
    for name in [
        modpack_format::OVERRIDES_DIR,
        modpack_format::CLIENT_OVERRIDES_DIR,
    ] {
        let dir = extract_dir.join(name);
        if dir.exists() {
            info!("复制 {} 目录到实例", name);
            file_utils::copy_dir_all(&dir, content_root)?;
        }
    }
    Ok(())
}
