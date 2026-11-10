//! 整合包安装模块
//!
//! 下载 Modrinth 整合包或直接读取本地 .mrpack，按 modrinth.index.json 并发补下模组与依赖加载器，
//! 失败或取消时回滚实例。在线安装与本地导入共用同一条流水线，区别只在压缩包与展示信息从哪来。
//!
//! 拆成几个阶段，互相之间只通过 `ModpackPlan` / `InstallTargets` 传递数据：
//! - [`progress`]：进度采样与节流上报
//! - [`plan`]：把在线/本地/CF 三种来源统一成一份待安装计划
//! - [`files`]：按清单并发补文件
//! - [`install`]：解压、覆盖、装游戏与加载器、失败回滚
//! - [`local`]：本地压缩包识别与名称规整

mod files;
mod install;
mod local;
mod plan;
mod progress;

#[cfg(test)]
mod tests;

pub use progress::{ModpackActiveFile, ModpackFilesProgress, ModpackInstallProgress};

use self::install::{acquire_install_lock, ensure_instance_free, finalize_install};
use self::local::LocalArchiveKind;
use self::local::{
    detect_local_archive_kind, detect_local_archive_kind_sync, file_stem_of, read_archive_index,
    read_archive_manifest, sanitize_instance_name, trimmed_or, validate_local_archive,
};
use self::plan::{ModpackPlan, build_targets};

use crate::errors::LauncherError;
use crate::models::modpack::*;
use crate::services::cf_modpack_format::{self};
use crate::services::curseforge::{CfManualFile, CfResource, CurseForgeService};
use crate::services::modpack_format::{self, ModrinthIndexFile};
use crate::services::{config, modrinth};
use crate::utils::file_utils::validate_instance_name_or_error;
use log::warn;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

static MODPACK_CANCEL_FLAG: std::sync::OnceLock<Arc<AtomicBool>> = std::sync::OnceLock::new();

pub(super) const MAX_EXTRACTED_BYTES: u64 = 8 * 1024 * 1024 * 1024;

/// 单个整合包的压缩包上限，防止误选超大文件把临时目录塞满
pub(super) const MAX_MODPACK_ARCHIVE_BYTES: u64 = 4 * 1024 * 1024 * 1024;

pub(super) fn get_cancel_flag() -> Arc<AtomicBool> {
    MODPACK_CANCEL_FLAG
        .get_or_init(|| Arc::new(AtomicBool::new(false)))
        .clone()
}

pub fn reset_modpack_cancel_flag() {
    get_cancel_flag().store(false, Ordering::SeqCst);
}

pub fn set_modpack_cancel_flag() {
    get_cancel_flag().store(true, Ordering::SeqCst);
    // 原版下载走独立的取消标志；不联动的话取消要等整轮下载跑完才生效
    crate::services::download::batch::set_cancel_flag();
}

pub(super) fn is_cancelled() -> bool {
    get_cancel_flag().load(Ordering::SeqCst)
}

pub(super) fn check_cancelled() -> Result<(), LauncherError> {
    if is_cancelled() {
        Err(LauncherError::Cancelled)
    } else {
        Ok(())
    }
}

/// 本地 .mrpack 的预览信息，用于安装前展示与默认实例名
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalModpackPreview {
    pub path: String,
    pub name: String,
    pub version_id: String,
    pub summary: Option<String>,
    pub minecraft: String,
    /// Modrinth 加载器标识：forge / fabric / quilt / neoforge，原版为空
    pub loader: Option<String>,
    pub loader_version: Option<String>,
    /// 需要下载的文件数（已排除服务端专用文件）
    pub file_count: usize,
    pub total_size: u64,
    pub suggested_instance_name: String,
}

pub struct ModpackInstaller {
    pub(super) modrinth_service: modrinth::ModrinthService,
    pub(super) cf_service: CurseForgeService,
}

static INSTALLER: std::sync::LazyLock<ModpackInstaller> =
    std::sync::LazyLock::new(ModpackInstaller::new);

pub fn get_installer() -> &'static ModpackInstaller {
    &INSTALLER
}

impl ModpackInstaller {
    fn new() -> Self {
        Self {
            modrinth_service: modrinth::ModrinthService::new(),
            cf_service: CurseForgeService::new(),
        }
    }

    /// 在线安装：从 Modrinth 拉项目信息与版本，下载 .mrpack 后走统一流水线
    pub async fn install_modrinth_modpack(
        &self,
        options: ModpackInstallOptions,
        window: &tauri::Window,
    ) -> Result<(), LauncherError> {
        reset_modpack_cancel_flag();
        validate_instance_name_or_error(&options.instance_name)?;

        let config = config::load_config()?;
        let targets = build_targets(&config, &options.instance_name)?;
        let _install_lock = acquire_install_lock(&targets).await?;
        ensure_instance_free(&targets, &options.instance_name)?;

        let result = async {
            let plan = self
                .prepare_modrinth_plan(&options, &targets, window)
                .await?;
            let outcome = self
                .install_plan(&plan, &options.instance_name, &targets, window)
                .await;

            if plan.remove_archive_after {
                let _ = fs::remove_file(&plan.archive_path);
            }
            outcome
        }
        .await;

        finalize_install(&targets, result).await
    }

    /// CurseForge 在线安装：解析出的直链文件走统一流水线，
    /// 受限文件拿不到直链，返回清单让前端引导用户手动下载
    pub async fn install_curseforge_modpack(
        &self,
        options: ModpackInstallOptions,
        window: &tauri::Window,
    ) -> Result<Vec<CfManualFile>, LauncherError> {
        reset_modpack_cancel_flag();
        validate_instance_name_or_error(&options.instance_name)?;

        let config = config::load_config()?;
        let targets = build_targets(&config, &options.instance_name)?;
        let _install_lock = acquire_install_lock(&targets).await?;
        ensure_instance_free(&targets, &options.instance_name)?;

        let result = async {
            let (plan, manual) = self.prepare_cf_plan(&options, &targets, window).await?;
            let outcome = self
                .install_plan(&plan, &options.instance_name, &targets, window)
                .await;

            if plan.remove_archive_after {
                let _ = fs::remove_file(&plan.archive_path);
            }
            outcome?;

            if !manual.is_empty() {
                warn!(
                    "整合包 {} 有 {} 个文件需手动下载",
                    options.instance_name,
                    manual.len()
                );
            }
            Ok(manual)
        }
        .await;

        finalize_install(&targets, result).await
    }

    /// 本地导入：自动识别 .mrpack 与 CurseForge zip，其余流程与在线安装一致
    pub async fn install_local_modpack(
        &self,
        options: LocalModpackInstallOptions,
        window: &tauri::Window,
    ) -> Result<Vec<CfManualFile>, LauncherError> {
        reset_modpack_cancel_flag();
        validate_instance_name_or_error(&options.instance_name)?;

        let archive_path = PathBuf::from(&options.file_path);
        validate_local_archive(&archive_path)?;

        let config = config::load_config()?;
        let targets = build_targets(&config, &options.instance_name)?;
        let _install_lock = acquire_install_lock(&targets).await?;
        ensure_instance_free(&targets, &options.instance_name)?;

        let result = async {
            let file_stem = file_stem_of(&archive_path);
            match detect_local_archive_kind(&archive_path).await? {
                LocalArchiveKind::CurseForge => {
                    let manifest = read_archive_manifest(&archive_path).await?;
                    let (index, manual) = self.build_cf_index(&manifest, window, 30, 45).await?;
                    let deps = index.dependencies.clone();
                    let plan = ModpackPlan {
                        display_name: trimmed_or(&manifest.name, &file_stem),
                        version_label: trimmed_or(&manifest.version, "1.0.0"),
                        source: "curseforge",
                        modpack_id: file_stem,
                        icon_url: None,
                        loaders: deps
                            .loader_slug()
                            .map(|slug| vec![slug.to_string()])
                            .unwrap_or_default(),
                        index,
                        archive_path,
                        remove_archive_after: false,
                    };
                    self.install_plan(&plan, &options.instance_name, &targets, window)
                        .await?;
                    Ok(manual)
                }
                LocalArchiveKind::Modrinth => {
                    let index = read_archive_index(&archive_path).await?;
                    let loader = index.dependencies.loader_slug();
                    let plan = ModpackPlan {
                        display_name: trimmed_or(&index.name, &file_stem),
                        version_label: trimmed_or(&index.version_id, "1.0.0"),
                        source: "local",
                        modpack_id: file_stem,
                        icon_url: None,
                        loaders: loader
                            .map(|slug| vec![slug.to_string()])
                            .unwrap_or_default(),
                        index,
                        archive_path,
                        remove_archive_after: false,
                    };
                    self.install_plan(&plan, &options.instance_name, &targets, window)
                        .await?;
                    Ok(Vec::new())
                }
            }
        }
        .await;

        finalize_install(&targets, result).await
    }

    /// 只读预览本地包，供安装页在真正落盘前展示信息；.mrpack 与 CF zip 都认
    pub async fn preview_local_modpack(
        &self,
        file_path: String,
    ) -> Result<LocalModpackPreview, LauncherError> {
        let archive_path = PathBuf::from(&file_path);
        validate_local_archive(&archive_path)?;

        tokio::task::spawn_blocking(move || -> Result<LocalModpackPreview, LauncherError> {
            let stem = file_stem_of(&archive_path);

            let (
                name,
                version_id,
                summary,
                minecraft,
                loader,
                loader_version,
                file_count,
                total_size,
            ) = match detect_local_archive_kind_sync(&archive_path)? {
                LocalArchiveKind::CurseForge => {
                    let manifest = cf_modpack_format::read_manifest_from_archive(&archive_path)?;
                    let deps = manifest.to_dependencies();
                    let loader = deps.loader();
                    (
                        trimmed_or(&manifest.name, &stem),
                        trimmed_or(&manifest.version, "1.0.0"),
                        None,
                        deps.minecraft.clone(),
                        deps.loader_slug().map(str::to_string),
                        loader.as_ref().map(|l| l.loader_version().to_string()),
                        // CF 清单不带文件体积，装进实例前要在线解析才知道，预览阶段只能给个数
                        manifest.client_file_refs().len(),
                        0u64,
                    )
                }
                LocalArchiveKind::Modrinth => {
                    let index = modpack_format::read_index_from_archive(&archive_path)?;
                    let client_files: Vec<&ModrinthIndexFile> = index
                        .files
                        .iter()
                        .filter(|file| modpack_format::is_client_file(file))
                        .collect();
                    let total_size = client_files
                        .iter()
                        .map(|file| file.file_size.unwrap_or(0))
                        .sum();
                    let loader = index.dependencies.loader();
                    (
                        trimmed_or(&index.name, &stem),
                        trimmed_or(&index.version_id, "1.0.0"),
                        index
                            .summary
                            .as_deref()
                            .map(str::trim)
                            .filter(|text| !text.is_empty())
                            .map(str::to_string),
                        index.dependencies.minecraft.clone(),
                        index.dependencies.loader_slug().map(str::to_string),
                        loader
                            .as_ref()
                            .map(|loaded| loaded.loader_version().to_string()),
                        client_files.len(),
                        total_size,
                    )
                }
            };

            let suggested_instance_name = sanitize_instance_name(&name);
            Ok(LocalModpackPreview {
                path: archive_path.to_string_lossy().to_string(),
                name,
                version_id,
                summary,
                minecraft,
                loader,
                loader_version,
                file_count,
                total_size,
                suggested_instance_name,
            })
        })
        .await
        .map_err(|e| LauncherError::Custom(format!("读取整合包失败: {}", e)))?
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn search_modpacks(
        &self,
        query: Option<String>,
        game_versions: Option<Vec<String>>,
        loaders: Option<Vec<String>>,
        categories: Option<Vec<String>>,
        limit: Option<u32>,
        offset: Option<u32>,
        sort_by: Option<String>,
    ) -> Result<ModrinthSearchResponse, LauncherError> {
        self.modrinth_service
            .search_modpacks(
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

    pub async fn get_modpack_versions(
        &self,
        project_id: &str,
        game_versions: Option<Vec<String>>,
        loaders: Option<Vec<String>>,
    ) -> Result<Vec<ModrinthModpackVersion>, LauncherError> {
        self.modrinth_service
            .get_modpack_versions(project_id, game_versions, loaders)
            .await
    }

    /// CurseForge 整合包搜索
    #[allow(clippy::too_many_arguments)]
    pub async fn search_cf_modpacks(
        &self,
        query: Option<String>,
        game_versions: Option<Vec<String>>,
        loaders: Option<Vec<String>>,
        categories: Option<Vec<String>>,
        limit: Option<u32>,
        offset: Option<u32>,
        sort_by: Option<String>,
    ) -> Result<ModrinthSearchResponse, LauncherError> {
        self.cf_service
            .search(
                CfResource::Modpacks,
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

    /// CurseForge 整合包的文件列表（文件即版本）
    pub async fn get_cf_modpack_versions(
        &self,
        project_id: &str,
    ) -> Result<Vec<ModrinthModpackVersion>, LauncherError> {
        let files = self
            .cf_service
            .get_files(CfResource::Modpacks, project_id, None)
            .await?;
        Ok(files
            .iter()
            .map(CurseForgeService::file_to_modrinth_version)
            .collect())
    }
}
