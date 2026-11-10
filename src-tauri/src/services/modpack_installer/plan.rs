//! 安装计划
//!
//! 三种来源（Modrinth 在线、CurseForge 在线、本地压缩包）最终都收敛成一份 `ModpackPlan`，
//! 路径计算统一由 `build_targets` 负责，后面的阶段不再关心数据是怎么来的

use super::local::{read_archive_index, read_archive_manifest, trimmed_or};
use super::progress::progress_emitter;
use super::{ModpackInstallProgress, ModpackInstaller, check_cancelled, get_cancel_flag};
use crate::errors::LauncherError;
use crate::models::modpack::*;
use crate::services::cf_modpack_format::CfManifest;
use crate::services::curseforge::{CfManualFile, CfResolvedFile, CfResource};
use crate::services::launcher::isolation;
use crate::services::modpack_format::{self, ModrinthIndex, ModrinthIndexFile};
use crate::utils::file_utils::{self};
use log::info;
use std::fs;
use std::path::PathBuf;
use tauri::Emitter;

/// 一次安装涉及的所有落盘位置
pub(super) struct InstallTargets {
    pub(super) game_dir: PathBuf,
    pub(super) instance_dir: PathBuf,
    pub(super) content_root: PathBuf,
    pub(super) temp_dir: PathBuf,
    pub(super) extract_dir: PathBuf,
}

/// 待安装的包：压缩包本体 + 已经解析好的清单
pub(super) struct ModpackPlan {
    /// 展示用名称（Modrinth 项目名或清单里的 name）
    pub(super) display_name: String,
    /// 展示用版本号（Modrinth 版本号或清单里的 versionId）
    pub(super) version_label: String,
    /// instance.json 里的 source 字段
    pub(super) source: &'static str,
    pub(super) modpack_id: String,
    pub(super) icon_url: Option<String>,
    pub(super) loaders: Vec<String>,
    pub(super) index: ModrinthIndex,
    pub(super) archive_path: PathBuf,
    /// 在线安装下载到 temp 的包可以删；用户自己的本地文件绝不能删
    pub(super) remove_archive_after: bool,
}

pub(super) fn build_targets(
    config: &crate::models::GameConfig,
    instance_name: &str,
) -> Result<InstallTargets, LauncherError> {
    // 关掉隔离后整合包会直接覆盖所有实例共用的 mods / config / saves，
    // 失败时又没法区分哪些文件是这次写的，回滚会误删别人的内容：只能拒绝
    if !config.version_isolation {
        return Err(LauncherError::Custom(
            "版本隔离已关闭：整合包会写入所有实例共用的 mods / config / saves，安装失败也无法回滚。请先在设置中开启版本隔离再安装整合包。"
                .to_string(),
        ));
    }

    let game_dir = PathBuf::from(&config.game_dir);
    let instance_dir = game_dir.join("versions").join(instance_name);
    let content_root = isolation::content_root(config, &game_dir, instance_name);

    let temp_dir = game_dir.join("temp");
    let extract_dir = temp_dir.join(format!("{}_extract", instance_name));

    Ok(InstallTargets {
        game_dir,
        instance_dir,
        content_root,
        temp_dir,
        extract_dir,
    })
}

impl ModpackInstaller {
    pub(super) async fn prepare_modrinth_plan(
        &self,
        options: &ModpackInstallOptions,
        targets: &InstallTargets,
        window: &tauri::Window,
    ) -> Result<ModpackPlan, LauncherError> {
        let send_progress = progress_emitter(window);

        send_progress(5, "检查实例目录...", false);
        check_cancelled()?;

        send_progress(10, "获取整合包信息...", false);
        check_cancelled()?;

        let modpack = self
            .modrinth_service
            .get_modpack(&options.modpack_id)
            .await
            .map_err(|e| LauncherError::Custom(format!("获取整合包信息失败: {}", e)))?;

        send_progress(15, "获取整合包版本...", false);
        check_cancelled()?;

        let versions = self
            .modrinth_service
            .get_modpack_versions(&options.modpack_id, None, None)
            .await
            .map_err(|e| LauncherError::Custom(format!("获取整合包版本失败: {}", e)))?;

        let selected_version = versions
            .iter()
            .find(|v| v.id == options.version_id)
            .ok_or_else(|| LauncherError::Custom("未找到指定的整合包版本".to_string()))?;

        send_progress(20, "下载整合包文件...", false);
        check_cancelled()?;

        let primary_file = selected_version
            .files
            .iter()
            .find(|f| f.primary)
            .or_else(|| selected_version.files.first())
            .ok_or_else(|| LauncherError::Custom("整合包没有可用的文件".to_string()))?;

        if !targets.temp_dir.exists() {
            fs::create_dir_all(&targets.temp_dir)?;
        }

        file_utils::validate_file_name_or_error(&primary_file.filename)?;
        let archive_path = targets.temp_dir.join(&primary_file.filename);
        let sha1 = (!primary_file.hashes.sha1.trim().is_empty())
            .then_some(primary_file.hashes.sha1.as_str());
        let cancel_flag = get_cancel_flag();

        let index_result = async {
            self.modrinth_service
                .download_modpack_file(
                    &primary_file.url,
                    &archive_path,
                    primary_file.size,
                    sha1,
                    &cancel_flag,
                )
                .await
                .map_err(|e| {
                    if e.is_cancelled() {
                        LauncherError::Cancelled
                    } else {
                        LauncherError::Custom(format!("下载整合包文件失败: {}", e))
                    }
                })?;

            send_progress(30, "读取整合包清单...", false);
            check_cancelled()?;

            read_archive_index(&archive_path).await
        }
        .await;

        // 半截包或坏包留在 temp 只会占盘（成功路径的清理由 remove_archive_after 负责）；
        // 清理失败不用阻断错误上报
        if index_result.is_err() {
            let _ = fs::remove_file(&archive_path);
        }

        let index = index_result?;

        Ok(ModpackPlan {
            display_name: modpack.title.clone(),
            version_label: selected_version.version_number.clone(),
            source: "modrinth",
            modpack_id: modpack.slug.clone(),
            icon_url: modpack.icon_url.clone(),
            loaders: selected_version.loaders.clone(),
            index,
            archive_path,
            remove_archive_after: true,
        })
    }

    /// CurseForge 在线安装的准备阶段：拉版本、下载 zip、读清单、在线解析文件直链，
    /// 最终合成一份 Modrinth 同构 index 交给统一流水线
    pub(super) async fn prepare_cf_plan(
        &self,
        options: &ModpackInstallOptions,
        targets: &InstallTargets,
        window: &tauri::Window,
    ) -> Result<(ModpackPlan, Vec<CfManualFile>), LauncherError> {
        let send_progress = progress_emitter(window);

        send_progress(5, "检查实例目录...", false);
        check_cancelled()?;

        send_progress(10, "获取整合包信息...", false);
        check_cancelled()?;

        let (title, icon_url) = self
            .cf_service
            .get_project_summary(CfResource::Modpacks, &options.modpack_id)
            .await
            .map_err(|e| LauncherError::Custom(format!("获取整合包信息失败: {}", e)))?;

        send_progress(15, "获取整合包版本...", false);
        check_cancelled()?;

        let files = self
            .cf_service
            .get_files(CfResource::Modpacks, &options.modpack_id, None)
            .await
            .map_err(|e| LauncherError::Custom(format!("获取整合包版本失败: {}", e)))?;

        let selected = files
            .iter()
            .find(|file| file.id.to_string() == options.version_id)
            .ok_or_else(|| LauncherError::Custom("未找到指定的整合包版本".to_string()))?;

        send_progress(20, "下载整合包文件...", false);
        check_cancelled()?;

        if !targets.temp_dir.exists() {
            fs::create_dir_all(&targets.temp_dir)?;
        }

        file_utils::validate_file_name_or_error(&selected.file_name)?;
        let archive_path = targets.temp_dir.join(&selected.file_name);
        let sha1 = (!selected.sha1.trim().is_empty()).then_some(selected.sha1.as_str());
        let cancel_flag = get_cancel_flag();

        let manifest_result = async {
            // 直链下载逻辑与 Modrinth 完全一致，复用同一个入口
            self.modrinth_service
                .download_modpack_file(
                    &selected.download_url,
                    &archive_path,
                    selected.file_length,
                    sha1,
                    &cancel_flag,
                )
                .await
                .map_err(|e| {
                    if e.is_cancelled() {
                        LauncherError::Cancelled
                    } else {
                        LauncherError::Custom(format!("下载整合包文件失败: {}", e))
                    }
                })?;

            send_progress(30, "读取整合包清单...", false);
            check_cancelled()?;

            read_archive_manifest(&archive_path).await
        }
        .await;

        if manifest_result.is_err() {
            let _ = fs::remove_file(&archive_path);
        }
        let manifest = manifest_result?;

        send_progress(32, "解析模组文件信息...", false);
        let (index, manual) = self.build_cf_index(&manifest, window, 32, 45).await?;

        let deps = index.dependencies.clone();
        let plan = ModpackPlan {
            display_name: trimmed_or(&title, &manifest.name),
            version_label: trimmed_or(&manifest.version, &selected.display_name),
            source: "curseforge",
            modpack_id: options.modpack_id.clone(),
            icon_url,
            loaders: deps
                .loader_slug()
                .map(|slug| vec![slug.to_string()])
                .unwrap_or_default(),
            index,
            archive_path,
            remove_archive_after: true,
        };
        Ok((plan, manual))
    }

    /// CF 清单 → Modrinth 同构 index；解析进度映射进 start..end 区间
    pub(super) async fn build_cf_index(
        &self,
        manifest: &CfManifest,
        window: &tauri::Window,
        start: u8,
        end: u8,
    ) -> Result<(ModrinthIndex, Vec<CfManualFile>), LauncherError> {
        let refs = manifest.client_file_refs();
        let total_refs = refs.len();
        let window = window.clone();
        let (resolved, manual) = self
            .cf_service
            .resolve_manifest_files(&refs, move |done, total| {
                let span = (end - start) as f32;
                let progress = start + ((done as f32 / total.max(1) as f32) * span) as u8;
                let _ = window.emit(
                    "modpack-install-progress",
                    ModpackInstallProgress {
                        progress,
                        message: format!("解析模组文件 ({}/{})", done, total),
                        indeterminate: false,
                    },
                );
            })
            .await?;
        check_cancelled()?;

        let mut index_files = Vec::with_capacity(resolved.len());
        for entry in &resolved {
            index_files.push(cf_resolved_to_index_file(entry)?);
        }

        info!(
            "CF 清单解析完成：{} 个文件可自动下载，{} 个需手动处理（共 {} 条引用）",
            index_files.len(),
            manual.len(),
            total_refs
        );

        let index = ModrinthIndex {
            format_version: modpack_format::SUPPORTED_FORMAT_VERSION,
            game: "minecraft".to_string(),
            version_id: trimmed_or(&manifest.version, "1.0.0"),
            name: manifest.name.trim().to_string(),
            summary: None,
            files: index_files,
            dependencies: manifest.to_dependencies(),
        };
        modpack_format::validate_index(&index)?;
        Ok((index, manual))
    }
}

/// 解析出的 CF 文件 → 下载清单条目；CF 的 manifest 不带路径，惯例全部进 mods/
pub(super) fn cf_resolved_to_index_file(
    entry: &CfResolvedFile,
) -> Result<ModrinthIndexFile, LauncherError> {
    file_utils::validate_file_name_or_error(&entry.file.file_name)?;
    Ok(ModrinthIndexFile {
        path: format!("mods/{}", entry.file.file_name),
        hashes: modpack_format::ModrinthIndexHashes {
            sha1: entry.file.sha1.clone(),
            sha512: None,
        },
        downloads: vec![entry.file.download_url.clone()],
        file_size: (entry.file.file_length > 0).then_some(entry.file.file_length),
        env: None,
    })
}
