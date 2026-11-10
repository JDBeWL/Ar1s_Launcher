//! 本地压缩包
//!
//! 只认 .mrpack 与 CurseForge zip 两种格式：先按文件名/大小筛，再打开 zip 看清单文件，
//! 不做启发式猜测——猜错会把用户的整合包解成垃圾

use super::MAX_MODPACK_ARCHIVE_BYTES;
use crate::errors::LauncherError;
use crate::services::cf_modpack_format::{self, CfManifest};
use crate::services::modpack_format::{self, ModrinthIndex};
use crate::utils::file_utils::{self};
use std::fs;
use std::path::Path;

pub(super) fn validate_local_archive(path: &Path) -> Result<(), LauncherError> {
    if !path.is_file() {
        return Err(LauncherError::Custom(format!(
            "整合包文件不存在: {}",
            path.display()
        )));
    }

    let size = fs::metadata(path)?.len();
    if size == 0 {
        return Err(LauncherError::Custom("整合包文件是空的".to_string()));
    }
    if size > MAX_MODPACK_ARCHIVE_BYTES {
        return Err(LauncherError::Custom(format!(
            "整合包文件超过 {} 字节上限",
            MAX_MODPACK_ARCHIVE_BYTES
        )));
    }
    Ok(())
}

pub(super) async fn read_archive_index(path: &Path) -> Result<ModrinthIndex, LauncherError> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || modpack_format::read_index_from_archive(&path))
        .await
        .map_err(|e| LauncherError::Custom(format!("读取整合包失败: {}", e)))?
}

pub(super) async fn read_archive_manifest(path: &Path) -> Result<CfManifest, LauncherError> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || cf_modpack_format::read_manifest_from_archive(&path))
        .await
        .map_err(|e| LauncherError::Custom(format!("读取整合包失败: {}", e)))?
}

/// 本地压缩包的格式识别
pub(super) enum LocalArchiveKind {
    Modrinth,
    CurseForge,
}

/// 打开 zip 只看清单文件名，不解析内容
pub(super) async fn detect_local_archive_kind(
    path: &Path,
) -> Result<LocalArchiveKind, LauncherError> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || detect_local_archive_kind_sync(&path))
        .await
        .map_err(|e| LauncherError::Custom(format!("读取整合包失败: {}", e)))?
}

pub(super) fn detect_local_archive_kind_sync(
    path: &Path,
) -> Result<LocalArchiveKind, LauncherError> {
    let file = fs::File::open(path)?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| {
        LauncherError::Custom(format!("无法读取整合包（可能不是 zip 格式）: {}", e))
    })?;

    if archive.by_name(modpack_format::INDEX_FILE).is_ok() {
        return Ok(LocalArchiveKind::Modrinth);
    }
    if archive.by_name(cf_modpack_format::CF_MANIFEST_FILE).is_ok() {
        return Ok(LocalArchiveKind::CurseForge);
    }
    Err(LauncherError::Custom(
        "压缩包里既没有 modrinth.index.json 也没有 manifest.json，无法识别整合包格式".to_string(),
    ))
}

pub(super) fn trimmed_or(value: &str, fallback: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        fallback.to_string()
    } else {
        value.to_string()
    }
}

pub(super) fn file_stem_of(path: &Path) -> String {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .filter(|stem| !stem.trim().is_empty())
        .unwrap_or_else(|| "modpack".to_string())
}

/// 把清单里的包名收敛成合法的实例目录名；实在救不回来就用固定兜底
pub(super) fn sanitize_instance_name(raw: &str) -> String {
    let mut cleaned = String::with_capacity(raw.len());
    for ch in raw.chars() {
        let reserved = matches!(ch, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*');
        cleaned.push(if reserved || (ch as u32) < 32 {
            '-'
        } else {
            ch
        });
    }

    // 实例名上限 64，校验函数量的是字节数，所以先按字符裁再按字节收缩
    let mut candidate: String = cleaned.chars().take(64).collect();
    candidate = candidate
        .trim_matches(|c: char| c == ' ' || c == '.')
        .to_string();
    while candidate.len() > 64 {
        candidate.pop();
    }
    let candidate = candidate.trim_end_matches([' ', '.']).to_string();

    // 长度之外还不合法（CON、NUL 之类的保留名）就没救了，直接用兜底名
    if candidate.is_empty() || !file_utils::validate_instance_name(&candidate).is_valid {
        return "modpack".to_string();
    }

    candidate
}
