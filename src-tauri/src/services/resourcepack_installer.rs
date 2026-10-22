//! 资源包安装与更新
//!
//! 与模组安装同源，但资源包没有加载器概念：Modrinth 上它们统一挂在 `minecraft` 这个 loader 下，
//! 也不存在依赖补齐。落盘位置是实例的 `resourcepacks` 目录。

use crate::errors::LauncherError;
use crate::models::DownloadJob;
use crate::models::modpack::*;
use crate::services::{config, download, launcher, modrinth};
use crate::utils::file_utils;
use log::info;
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64};

/// Modrinth 把资源包归在 `minecraft` 这个 loader 下
const RESOURCEPACK_LOADER: &str = "minecraft";

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledResourcePack {
    pub version_id: String,
    pub project_id: String,
    pub name: String,
    pub version_number: String,
    pub file_name: String,
}

/// 一个本地文件对应的可更新版本
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourcePackUpdate {
    pub file_name: String,
    pub version_id: String,
    pub project_id: String,
    pub version_number: String,
}

struct PackContext {
    game_dir: PathBuf,
    packs_dir: PathBuf,
    game_version: String,
}

pub struct ResourcePackInstaller {
    modrinth: modrinth::ModrinthService,
}

static INSTALLER: std::sync::LazyLock<ResourcePackInstaller> =
    std::sync::LazyLock::new(ResourcePackInstaller::new);

pub fn get_installer() -> &'static ResourcePackInstaller {
    &INSTALLER
}

impl ResourcePackInstaller {
    fn new() -> Self {
        Self {
            modrinth: modrinth::ModrinthService::new(),
        }
    }

    pub async fn search(
        &self,
        instance_name: &str,
        query: Option<String>,
        limit: Option<u32>,
        offset: Option<u32>,
        sort_by: Option<String>,
    ) -> Result<ModrinthSearchResponse, LauncherError> {
        let context = resolve_context(instance_name).await?;
        self.modrinth
            .search_projects(
                "resourcepack",
                query,
                Some(vec![context.game_version]),
                Some(vec![RESOURCEPACK_LOADER.to_string()]),
                None,
                limit,
                offset,
                sort_by,
            )
            .await
    }

    pub async fn get_versions(
        &self,
        instance_name: &str,
        project_id: &str,
    ) -> Result<Vec<ModrinthModpackVersion>, LauncherError> {
        let context = resolve_context(instance_name).await?;
        self.modrinth
            .get_project_versions(
                project_id,
                Some(vec![context.game_version]),
                Some(vec![RESOURCEPACK_LOADER.to_string()]),
            )
            .await
    }

    pub async fn install(
        &self,
        instance_name: String,
        project_id: String,
        version_id: Option<String>,
    ) -> Result<InstalledResourcePack, LauncherError> {
        let context = resolve_context(&instance_name).await?;
        fs::create_dir_all(&context.packs_dir)?;

        let version = match version_id.filter(|id| !id.trim().is_empty()) {
            Some(id) => self.modrinth.get_version(&id).await?,
            None => self
                .modrinth
                .get_project_versions(
                    &project_id,
                    Some(vec![context.game_version.clone()]),
                    Some(vec![RESOURCEPACK_LOADER.to_string()]),
                )
                .await?
                .into_iter()
                .next()
                .ok_or_else(|| {
                    LauncherError::Custom("该资源包没有适配此实例版本的文件".to_string())
                })?,
        };

        let file_name = download_version(&version, &context.packs_dir).await?;
        info!(
            "实例 {} 安装资源包 {} ({})",
            instance_name, version.name, file_name
        );

        Ok(InstalledResourcePack {
            version_id: version.id,
            project_id: version.project_id,
            name: version.name,
            version_number: version.version_number,
            file_name,
        })
    }

    /// 用本地文件的 sha1 反查最新版本；已是最新的文件不会出现在结果里
    pub async fn check_updates(
        &self,
        instance_name: String,
    ) -> Result<Vec<ResourcePackUpdate>, LauncherError> {
        let context = resolve_context(&instance_name).await?;
        let files = list_local_packs(&context.packs_dir);
        if files.is_empty() {
            return Ok(Vec::new());
        }

        // 并行分片算哈希：串行循环只用得上一个核；指纹没变的文件沿用上次结论
        let (hashes, by_hash) =
            tokio::task::spawn_blocking(move || file_utils::sha1_index_cached(&files))
                .await
                .map_err(|e| LauncherError::Custom(format!("计算资源包哈希失败: {}", e)))?;

        let versions = self
            .modrinth
            .check_updates(
                hashes,
                vec![context.game_version.clone()],
                vec![RESOURCEPACK_LOADER.to_string()],
            )
            .await?;

        let mut updates = Vec::new();
        for (hash, version) in versions {
            let Some(file_name) = by_hash.get(&hash) else {
                continue;
            };
            let already_latest = version
                .files
                .iter()
                .any(|file| file.hashes.sha1.eq_ignore_ascii_case(hash.as_str()));
            if already_latest {
                continue;
            }
            updates.push(ResourcePackUpdate {
                file_name: file_name.clone(),
                version_id: version.id,
                project_id: version.project_id,
                version_number: version.version_number,
            });
        }

        updates.sort_by(|a, b| a.file_name.cmp(&b.file_name));
        Ok(updates)
    }

    pub async fn update(
        &self,
        instance_name: String,
        file_name: String,
        version_id: String,
    ) -> Result<InstalledResourcePack, LauncherError> {
        let context = resolve_context(&instance_name).await?;
        fs::create_dir_all(&context.packs_dir)?;

        file_utils::validate_file_name_or_error(&file_name)?;
        let old_path = context.packs_dir.join(&file_name);
        if !old_path.is_file() {
            return Err(LauncherError::Custom(format!(
                "资源包文件不存在: {}",
                file_name
            )));
        }

        let version = self.modrinth.get_version(&version_id).await?;

        // 先下到临时目录，成功后再替换；下载失败不会把旧版本删掉
        let staging = context.game_dir.join("temp").join("resourcepack-staging");
        fs::create_dir_all(&staging)?;
        let staged_name = download_version(&version, &staging).await?;
        let staged_path = staging.join(&staged_name);
        let final_path = context.packs_dir.join(&staged_name);

        file_utils::replace_file_atomically(&staged_path, &final_path)?;
        // 新旧文件名不同时，残留的旧文件单独删
        if old_path != final_path {
            let _ = fs::remove_file(&old_path);
        }
        let _ = fs::remove_dir(&staging);

        info!("资源包更新: {} -> {}", file_name, staged_name);

        Ok(InstalledResourcePack {
            version_id: version.id,
            project_id: version.project_id,
            name: version.name,
            version_number: version.version_number,
            file_name: staged_name,
        })
    }
}

/// 资源包不需要加载器，原版实例也能装；只要知道 MC 版本就能筛选
async fn resolve_context(instance_name: &str) -> Result<PackContext, LauncherError> {
    let info = crate::services::instance::get_instance_info(instance_name).await?;

    let game_version = info
        .game_version
        .filter(|version| !version.is_empty())
        .ok_or_else(|| LauncherError::Custom("无法确定实例的 Minecraft 版本".to_string()))?;

    let config = config::load_config()?;
    let game_dir = PathBuf::from(&config.game_dir);
    let packs_dir =
        launcher::isolation::content_root_for(&game_dir, instance_name).join("resourcepacks");

    Ok(PackContext {
        game_dir,
        packs_dir,
        game_version,
    })
}

async fn download_version(
    version: &ModrinthModpackVersion,
    target_dir: &Path,
) -> Result<String, LauncherError> {
    let file = version
        .files
        .iter()
        .find(|file| file.primary)
        .or_else(|| version.files.first())
        .ok_or_else(|| {
            LauncherError::Custom(format!("版本 {} 没有可下载的文件", version.version_number))
        })?;

    file_utils::validate_file_name_or_error(&file.filename)?;

    let job = DownloadJob {
        url: file.url.clone(),
        fallback_url: None,
        path: target_dir.join(&file.filename),
        size: file.size,
        hash: (!file.hashes.sha1.trim().is_empty()).then(|| file.hashes.sha1.clone()),
    };

    let http = download::get_http_client();
    let state = Arc::new(AtomicBool::new(true));
    let cancel = Arc::new(AtomicBool::new(false));
    let downloaded = Arc::new(AtomicU64::new(0));
    let since_last = Arc::new(AtomicU64::new(0));

    download::download_file(
        &http,
        &job,
        &file.url,
        &state,
        &cancel,
        &downloaded,
        &since_last,
    )
    .await?;

    Ok(file.filename.clone())
}

/// resourcepacks 目录下的 zip，返回绝对路径与文件名
fn list_local_packs(packs_dir: &Path) -> Vec<(PathBuf, String)> {
    let Ok(entries) = fs::read_dir(packs_dir) else {
        return Vec::new();
    };

    entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if !path.is_file() {
                return None;
            }
            let file_name = path.file_name()?.to_str()?.to_string();
            file_name
                .to_ascii_lowercase()
                .ends_with(".zip")
                .then_some((path, file_name))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::list_local_packs;
    use std::fs;

    #[test]
    fn test_list_local_packs_only_returns_zips() {
        let dir = tempfile::TempDir::new().unwrap();
        fs::write(dir.path().join("a.zip"), b"a").unwrap();
        fs::write(dir.path().join("B.ZIP"), b"b").unwrap();
        fs::write(dir.path().join("notes.txt"), b"c").unwrap();
        fs::create_dir(dir.path().join("unzipped")).unwrap();

        let mut names: Vec<String> = list_local_packs(dir.path())
            .into_iter()
            .map(|(_, name)| name)
            .collect();
        names.sort();
        assert_eq!(names, vec!["B.ZIP".to_string(), "a.zip".to_string()]);
    }

    #[test]
    fn test_list_local_packs_tolerates_missing_directory() {
        assert!(list_local_packs(std::path::Path::new("does-not-exist")).is_empty());
    }
}
