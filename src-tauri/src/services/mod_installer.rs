//! 模组安装与更新
//!
//! 按实例的 Minecraft 版本与加载器在 Modrinth 上搜索、安装、检查更新。
//! 安装时按 `required` 依赖做广度优先补齐；更新时用文件 sha1 反查归属项目，
//! 这样即使玩家改过文件名也能认出是哪个模组。

use crate::errors::LauncherError;
use crate::models::DownloadJob;
use crate::models::modpack::*;
use crate::services::{config, download, instance, launcher, modrinth};
use crate::utils::file_utils;
use log::{info, warn};
use serde::Serialize;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64};

/// 一次安装最多补多少个依赖，防止作者写坏依赖链把磁盘塞满
const MAX_INSTALLED_MODS: usize = 64;

/// 一次安装/更新所在的实例环境
struct ModContext {
    game_dir: PathBuf,
    mods_dir: PathBuf,
    game_version: String,
    /// Modrinth 的加载器 id：forge / fabric / quilt / neoforge
    loader: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledMod {
    pub version_id: String,
    pub project_id: String,
    pub name: String,
    pub version_number: String,
    pub file_name: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModInstallResult {
    pub installed: Vec<InstalledMod>,
}

/// 一个本地文件对应的可更新版本
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModUpdate {
    /// 本地文件名（可能带 .disabled 后缀）
    pub file_name: String,
    pub version_id: String,
    pub project_id: String,
    pub version_number: String,
}

pub struct ModInstaller {
    modrinth: modrinth::ModrinthService,
}

static INSTALLER: std::sync::LazyLock<ModInstaller> = std::sync::LazyLock::new(ModInstaller::new);

pub fn get_installer() -> &'static ModInstaller {
    &INSTALLER
}

impl ModInstaller {
    fn new() -> Self {
        Self {
            modrinth: modrinth::ModrinthService::new(),
        }
    }

    pub async fn search_mods(
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
                "mod",
                query,
                Some(vec![context.game_version]),
                Some(vec![context.loader]),
                None,
                limit,
                offset,
                sort_by,
            )
            .await
    }

    pub async fn get_mod_versions(
        &self,
        instance_name: &str,
        project_id: &str,
    ) -> Result<Vec<ModrinthModpackVersion>, LauncherError> {
        let context = resolve_context(instance_name).await?;
        self.modrinth
            .get_project_versions(
                project_id,
                Some(vec![context.game_version]),
                Some(vec![context.loader]),
            )
            .await
    }

    pub async fn install_mod(
        &self,
        instance_name: String,
        project_id: String,
        version_id: Option<String>,
        install_dependencies: bool,
    ) -> Result<ModInstallResult, LauncherError> {
        let context = resolve_context(&instance_name).await?;
        fs::create_dir_all(&context.mods_dir)?;

        let root_version_id = match version_id.filter(|id| !id.trim().is_empty()) {
            Some(id) => id,
            None => self
                .modrinth
                .get_project_versions(
                    &project_id,
                    Some(vec![context.game_version.clone()]),
                    Some(vec![context.loader.clone()]),
                )
                .await?
                .first()
                .map(|version| version.id.clone())
                .ok_or_else(|| {
                    LauncherError::Custom("该模组没有适配此实例版本与加载器的文件".to_string())
                })?,
        };

        // 依赖解析分轮：同一轮里的取版本与下载互不依赖，可以并发；下一轮要等这一轮的结果才知道新依赖。
        // 串行时每个依赖一次 get_version + 一次下载，10 个依赖就是 20 个往返
        let mut frontier = vec![root_version_id];
        let mut visited: HashSet<String> = HashSet::new();
        let mut installed: Vec<InstalledMod> = Vec::new();

        while !frontier.is_empty() {
            let mut round: Vec<String> = Vec::new();
            for version_id in std::mem::take(&mut frontier) {
                if !visited.insert(version_id.clone()) {
                    continue;
                }
                if visited.len() > MAX_INSTALLED_MODS {
                    warn!("依赖数量超过 {}，停止继续解析", MAX_INSTALLED_MODS);
                    break;
                }
                round.push(version_id);
            }

            if round.is_empty() {
                break;
            }

            let results = resolve_and_download(&context, round, install_dependencies).await?;

            // 结果已按本轮序号排好，直接追加能保持「根 → 一级依赖 → 二级依赖」的顺序
            for (_, installed_mod, next) in results {
                installed.push(installed_mod);
                frontier.extend(next);
            }
        }

        info!(
            "实例 {} 安装模组 {} 个（含依赖）",
            instance_name,
            installed.len()
        );
        Ok(ModInstallResult { installed })
    }

    /// 用本地文件的 sha1 反查最新版本；已是最新的文件不会出现在结果里
    pub async fn check_updates(
        &self,
        instance_name: String,
    ) -> Result<Vec<ModUpdate>, LauncherError> {
        let context = resolve_context(&instance_name).await?;
        let files = list_local_mods(&context.mods_dir)?;
        if files.is_empty() {
            return Ok(Vec::new());
        }

        // 哈希是 CPU 密集活，扔到阻塞线程池并按核数并行分片；
        // 指纹没变的文件直接沿用上次算出的内容哈希，不必重读
        let (hashes, by_hash) =
            tokio::task::spawn_blocking(move || file_utils::sha1_index_cached(&files))
                .await
                .map_err(|e| LauncherError::Custom(format!("计算模组哈希失败: {}", e)))?;

        let versions = self
            .modrinth
            .check_updates(
                hashes,
                vec![context.game_version.clone()],
                vec![context.loader.clone()],
            )
            .await?;

        let mut updates = Vec::new();
        for (hash, version) in versions {
            let Some(file_name) = by_hash.get(&hash) else {
                continue;
            };

            // 已是最新的文件也会被返回，靠版本内的 sha1 判断是否真的需要更新
            let already_latest = version
                .files
                .iter()
                .any(|file| file.hashes.sha1.eq_ignore_ascii_case(hash.as_str()));
            if already_latest {
                continue;
            }

            updates.push(ModUpdate {
                file_name: file_name.clone(),
                version_id: version.id,
                project_id: version.project_id,
                version_number: version.version_number,
            });
        }

        updates.sort_by(|a, b| a.file_name.cmp(&b.file_name));
        Ok(updates)
    }

    pub async fn update_mod(
        &self,
        instance_name: String,
        file_name: String,
        version_id: String,
    ) -> Result<InstalledMod, LauncherError> {
        let context = resolve_context(&instance_name).await?;
        fs::create_dir_all(&context.mods_dir)?;

        file_utils::validate_file_name_or_error(&file_name)?;
        let old_path = context.mods_dir.join(&file_name);
        if !old_path.is_file() {
            return Err(LauncherError::Custom(format!(
                "模组文件不存在: {}",
                file_name
            )));
        }
        let was_disabled = file_name.ends_with(".disabled");

        let version = self.modrinth.get_version(&version_id).await?;

        // 先下到临时目录，成功后再替换；下载失败不会把旧版本删掉。
        let staging = context.game_dir.join("temp").join("mod-staging");
        fs::create_dir_all(&staging)?;
        let staged_name = download_version(&version, &staging).await?;
        let staged_path = staging.join(&staged_name);

        let final_name = if was_disabled {
            format!("{}.disabled", staged_name)
        } else {
            staged_name
        };
        let final_path = context.mods_dir.join(&final_name);

        // 替换旧文件走 replace_with_backup：先改名成 .bak 再换新的进来，失败会回滚
        file_utils::replace_file_atomically(&staged_path, &final_path)?;
        // 旧文件名可能与新文件名不同（版本换了、禁用状态变了），残留的旧文件要单独删
        if old_path != final_path {
            let _ = fs::remove_file(&old_path);
        }
        let _ = fs::remove_dir(&staging);

        info!("模组更新: {} -> {}", file_name, final_name);

        Ok(InstalledMod {
            version_id: version.id,
            project_id: version.project_id,
            name: version.name,
            version_number: version.version_number,
            file_name: final_name,
        })
    }
}

/// 依赖解析的并发上限：再多也只会把 Modrinth 的速率限制撞得更狠
const MAX_CONCURRENT_RESOLVES: usize = 8;

/// 并发取版本并下载一轮依赖，返回 (序号, 安装结果, 新发现的依赖)
///
/// 同一轮内各模组互不依赖；并发上限跟下载线程数走，结果按入参顺序返回，保证安装列表顺序稳定
async fn resolve_and_download(
    context: &ModContext,
    version_ids: Vec<String>,
    install_dependencies: bool,
) -> Result<Vec<(usize, InstalledMod, Vec<String>)>, LauncherError> {
    let concurrency = config::load_config()
        .map(|cfg| {
            (cfg.download_threads as usize).clamp(
                config::MIN_DOWNLOAD_THREADS as usize,
                MAX_CONCURRENT_RESOLVES,
            )
        })
        .unwrap_or(4)
        .max(1);

    let semaphore = Arc::new(tokio::sync::Semaphore::new(concurrency));
    let mut handles = Vec::with_capacity(version_ids.len());

    for (index, version_id) in version_ids.into_iter().enumerate() {
        let mods_dir = context.mods_dir.clone();
        let game_version = context.game_version.clone();
        let loader = context.loader.clone();
        let permit = semaphore.clone();

        handles.push((
            index,
            tokio::spawn(async move {
                let _permit = permit
                    .acquire_owned()
                    .await
                    .map_err(|_| LauncherError::Custom("依赖解析被中止".to_string()))?;

                let service = modrinth::ModrinthService::new();
                let version = service.get_version(&version_id).await?;
                let file_name = download_version(&version, &mods_dir).await?;

                let mut next = Vec::new();
                if install_dependencies {
                    for dependency in &version.dependencies {
                        if dependency.dependency_type != "required" {
                            continue;
                        }
                        if let Some(dependency_version) = dependency.version_id.as_ref() {
                            next.push(dependency_version.clone());
                        } else if let Some(dependency_project) = dependency.project_id.as_ref() {
                            // 依赖只给了项目：按实例版本与加载器挑最新的一个
                            if let Ok(versions) = service
                                .get_project_versions(
                                    dependency_project,
                                    Some(vec![game_version.clone()]),
                                    Some(vec![loader.clone()]),
                                )
                                .await
                                && let Some(first) = versions.first()
                            {
                                next.push(first.id.clone());
                            }
                        }
                    }
                }

                Ok::<_, LauncherError>((
                    InstalledMod {
                        version_id: version.id.clone(),
                        project_id: version.project_id.clone(),
                        name: version.name.clone(),
                        version_number: version.version_number.clone(),
                        file_name,
                    },
                    next,
                ))
            }),
        ));
    }

    let mut results = Vec::with_capacity(handles.len());
    for (index, handle) in handles {
        let (installed_mod, next) = handle
            .await
            .map_err(|e| LauncherError::Custom(format!("模组下载任务异常终止: {}", e)))??;
        results.push((index, installed_mod, next));
    }

    // 并发完成顺序不可控，按序号还原成本轮入参顺序
    results.sort_by_key(|(index, _, _)| *index);
    Ok(results)
}

/// 从实例元数据里取 MC 版本与加载器，原版实例直接拒绝
async fn resolve_context(instance_name: &str) -> Result<ModContext, LauncherError> {
    let info = instance::get_instance_info(instance_name).await?;

    let loader = loader_slug(info.loader_type.as_deref()).ok_or_else(|| {
        LauncherError::Custom(
            "该实例没有安装模组加载器（Forge / Fabric / Quilt / NeoForge），无法管理模组"
                .to_string(),
        )
    })?;

    let game_version = info
        .game_version
        .filter(|version| !version.is_empty())
        .ok_or_else(|| LauncherError::Custom("无法确定实例的 Minecraft 版本".to_string()))?;

    let config = config::load_config()?;
    let game_dir = PathBuf::from(&config.game_dir);
    let mods_dir = launcher::isolation::content_root_for(&game_dir, instance_name).join("mods");

    Ok(ModContext {
        game_dir,
        mods_dir,
        game_version,
        loader: loader.to_string(),
    })
}

fn loader_slug(loader_type: Option<&str>) -> Option<&'static str> {
    match loader_type? {
        "Forge" => Some("forge"),
        "Fabric" => Some("fabric"),
        "Quilt" => Some("quilt"),
        "NeoForge" => Some("neoforge"),
        _ => None,
    }
}

/// 下载版本的主文件，返回落盘后的文件名
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
        fallback_urls: Vec::new(),
        path: target_dir.join(&file.filename),
        size: file.size,
        // 空 sha1 表示上游没给哈希，交给大小与后续校验兜底
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

/// mods 目录下的 jar（含 .disabled），返回绝对路径与文件名
fn list_local_mods(mods_dir: &Path) -> Result<Vec<(PathBuf, String)>, LauncherError> {
    if !mods_dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut files = Vec::new();
    for entry in fs::read_dir(mods_dir)? {
        let path = entry?.path();
        if !path.is_file() {
            continue;
        }
        // 先转成 String 再 move path，否则 file_name 还借着 path 的字符串切片
        let Some(file_name) = path
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_string)
        else {
            continue;
        };
        if !(file_name.ends_with(".jar") || file_name.ends_with(".jar.disabled")) {
            continue;
        }
        files.push((path, file_name));
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::{list_local_mods, loader_slug};
    use std::fs;

    #[test]
    fn test_loader_slug_accepts_known_loaders_only() {
        assert_eq!(loader_slug(Some("Forge")), Some("forge"));
        assert_eq!(loader_slug(Some("NeoForge")), Some("neoforge"));
        assert_eq!(loader_slug(Some("None")), None);
        assert_eq!(loader_slug(None), None);
    }

    #[test]
    fn test_list_local_mods_only_returns_jars() {
        let dir = tempfile::TempDir::new().unwrap();
        fs::write(dir.path().join("a.jar"), b"a").unwrap();
        fs::write(dir.path().join("b.jar.disabled"), b"b").unwrap();
        fs::write(dir.path().join("config.toml"), b"c").unwrap();
        fs::create_dir(dir.path().join("subdir")).unwrap();

        let mut names: Vec<String> = list_local_mods(dir.path())
            .unwrap()
            .into_iter()
            .map(|(_, name)| name)
            .collect();
        names.sort();

        assert_eq!(
            names,
            vec!["a.jar".to_string(), "b.jar.disabled".to_string()]
        );
    }
}
