//! Modrinth 整合包（.mrpack）格式
//!
//! `modrinth.index.json` 的结构、校验与解压；在线安装、本地导入与导出共用这一份定义

use crate::errors::LauncherError;
use crate::services::loaders::LoaderType;
use crate::utils::file_utils;
use log::warn;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{Read, Write};
use std::path::Path;

pub const INDEX_FILE: &str = "modrinth.index.json";
pub const OVERRIDES_DIR: &str = "overrides";
pub const CLIENT_OVERRIDES_DIR: &str = "client-overrides";
/// 目前只有 formatVersion 1，别的值宁可拒绝也不要瞎猜字段含义
pub const SUPPORTED_FORMAT_VERSION: u32 = 1;
/// index 本身不可能有这么大，超过就是恶意包
const MAX_INDEX_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModrinthIndex {
    #[serde(rename = "formatVersion")]
    pub format_version: u32,
    pub game: String,
    #[serde(rename = "versionId")]
    pub version_id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default)]
    pub files: Vec<ModrinthIndexFile>,
    pub dependencies: ModrinthDependencies,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModrinthIndexFile {
    pub path: String,
    pub hashes: ModrinthIndexHashes,
    #[serde(default)]
    pub downloads: Vec<String>,
    #[serde(rename = "fileSize")]
    #[serde(default)]
    pub file_size: Option<u64>,
    /// 客户端/服务端可用性，缺省视为两端都需要
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<ModrinthFileEnv>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModrinthIndexHashes {
    #[serde(default)]
    pub sha1: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha512: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModrinthFileEnv {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModrinthDependencies {
    #[serde(default)]
    pub minecraft: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forge: Option<String>,
    /// 早期导出工具写的是 `fabric`，规范里是 `fabric-loader`，两个都认
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fabric: Option<String>,
    #[serde(
        rename = "fabric-loader",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub fabric_loader: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quilt: Option<String>,
    #[serde(
        rename = "quilt-loader",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub quilt_loader: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub neoforge: Option<String>,
}

impl ModrinthDependencies {
    /// 按 Forge → Fabric → Quilt → NeoForge 的顺序取第一个声明的加载器
    pub fn loader(&self) -> Option<LoaderType> {
        let mc_version = self.minecraft.trim();
        if mc_version.is_empty() {
            return None;
        }

        let version = |value: &Option<String>| {
            value
                .as_deref()
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_string)
        };

        if let Some(loader_version) = version(&self.forge) {
            return Some(LoaderType::Forge {
                mc_version: mc_version.to_string(),
                loader_version,
            });
        }
        if let Some(loader_version) = version(&self.fabric_loader).or_else(|| version(&self.fabric))
        {
            return Some(LoaderType::Fabric {
                mc_version: mc_version.to_string(),
                loader_version,
            });
        }
        if let Some(loader_version) = version(&self.quilt_loader).or_else(|| version(&self.quilt)) {
            return Some(LoaderType::Quilt {
                mc_version: mc_version.to_string(),
                loader_version,
            });
        }
        if let Some(loader_version) = version(&self.neoforge) {
            return Some(LoaderType::NeoForge {
                mc_version: mc_version.to_string(),
                loader_version,
            });
        }

        None
    }

    /// instance.json 里记的加载器标识，取值与前端 LoaderType 一致
    pub fn loader_slug(&self) -> Option<&'static str> {
        match self.loader()? {
            LoaderType::Forge { .. } => Some("forge"),
            LoaderType::Fabric { .. } => Some("fabric"),
            LoaderType::Quilt { .. } => Some("quilt"),
            LoaderType::NeoForge { .. } => Some("neoforge"),
        }
    }
}

/// `env.client` 为 unsupported 的文件是服务端专用，客户端装了纯粹占空间
pub fn is_client_file(file: &ModrinthIndexFile) -> bool {
    file.env.as_ref().and_then(|env| env.client.as_deref()) != Some("unsupported")
}

/// 只从压缩包里读 index，不解压整个包 —— 预览和安装前校验都走这里
pub fn read_index_from_archive(path: &Path) -> Result<ModrinthIndex, LauncherError> {
    let file = fs::File::open(path)?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| {
        LauncherError::Custom(format!("无法读取整合包（可能不是 zip 格式）: {}", e))
    })?;

    let mut entry = archive.by_name(INDEX_FILE).map_err(|_| {
        LauncherError::Custom(format!("整合包缺少 {}，可能不是 Modrinth 格式", INDEX_FILE))
    })?;

    if entry.size() > MAX_INDEX_BYTES {
        return Err(LauncherError::Custom(format!(
            "{} 超过 {} 字节上限，已拒绝",
            INDEX_FILE, MAX_INDEX_BYTES
        )));
    }

    let mut text = String::new();
    entry.read_to_string(&mut text)?;
    let index: ModrinthIndex = serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .map_err(|e| LauncherError::Custom(format!("解析 {} 失败: {}", INDEX_FILE, e)))?;

    validate_index(&index)?;
    Ok(index)
}

/// 解压后的 index，路径校验与结构校验和压缩包里读到的一致
pub fn read_index_file(path: &Path) -> Result<ModrinthIndex, LauncherError> {
    let text = fs::read_to_string(path)?;
    let index: ModrinthIndex = serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .map_err(|e| LauncherError::Custom(format!("解析 {} 失败: {}", INDEX_FILE, e)))?;
    validate_index(&index)?;
    Ok(index)
}

pub fn validate_index(index: &ModrinthIndex) -> Result<(), LauncherError> {
    if index.format_version != SUPPORTED_FORMAT_VERSION {
        return Err(LauncherError::Custom(format!(
            "不支持的整合包格式版本 {}（仅支持 {}）",
            index.format_version, SUPPORTED_FORMAT_VERSION
        )));
    }
    if index.game != "minecraft" {
        return Err(LauncherError::Custom(format!(
            "不支持的整合包类型: {}",
            index.game
        )));
    }
    if index.dependencies.minecraft.trim().is_empty() {
        return Err(LauncherError::Custom(
            "整合包没有声明 Minecraft 版本".to_string(),
        ));
    }
    // 清单里的 path 由远端给；出现遍历路径说明包本身有问题，整包拒绝而不是装一半
    if let Some(bad) = index
        .files
        .iter()
        .find(|file| !file_utils::is_safe_relative_path(&file.path))
    {
        return Err(LauncherError::Custom(format!(
            "整合包清单包含非法文件路径，已拒绝安装: {}",
            bad.path
        )));
    }
    Ok(())
}

/// 解压整合包，防止 zip 路径遍历与 zip bomb
pub fn extract_archive(
    archive_path: &Path,
    extract_dir: &Path,
    max_bytes: u64,
) -> Result<(), LauncherError> {
    fs::create_dir_all(extract_dir)?;

    // 先按 central directory 声明的未压缩大小做快速上限检查
    let file = fs::File::open(archive_path)?;
    let mut archive = zip::ZipArchive::new(file)?;
    let mut declared_total = 0u64;
    for i in 0..archive.len() {
        let entry = archive.by_index(i)?;
        let name = entry.name()?.to_string();
        if name.ends_with('/') || !file_utils::is_safe_relative_path(&name) {
            continue;
        }
        declared_total = declared_total.saturating_add(entry.size());
        if declared_total > max_bytes {
            return Err(LauncherError::Custom(format!(
                "整合包解压后超过 {} 字节上限，已中止",
                max_bytes
            )));
        }
    }
    drop(archive);

    let file = fs::File::open(archive_path)?;
    let mut archive = zip::ZipArchive::new(file)?;
    let mut actual_total = 0u64;

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let file_name = entry.name()?.to_string();

        if !file_utils::is_safe_relative_path(&file_name) {
            warn!("跳过可疑的 zip 条目: {}", file_name);
            continue;
        }

        let outpath = extract_dir.join(&file_name);

        if file_name.ends_with('/') {
            fs::create_dir_all(&outpath)?;
            continue;
        }

        if let Some(parent) = outpath.parent() {
            fs::create_dir_all(parent)?;
        }

        let mut outfile = fs::File::create(&outpath)?;
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let read = entry.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            actual_total = actual_total.saturating_add(read as u64);
            if actual_total > max_bytes {
                return Err(LauncherError::Custom(format!(
                    "整合包解压后超过 {} 字节上限，已中止",
                    max_bytes
                )));
            }
            outfile.write_all(&buffer[..read])?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        ModrinthDependencies, ModrinthFileEnv, ModrinthIndexFile, ModrinthIndexHashes,
        extract_archive, is_client_file, validate_index,
    };
    use crate::services::loaders::LoaderType;
    use serde_json::json;
    use std::fs;
    use std::io::Write;
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    fn write_zip(path: &std::path::Path, entries: &[(&str, &[u8])]) {
        let file = fs::File::create(path).unwrap();
        let mut zip = ZipWriter::new(file);
        for (name, content) in entries {
            zip.start_file(*name, SimpleFileOptions::default()).unwrap();
            zip.write_all(content).unwrap();
        }
        zip.finish().unwrap();
    }

    #[test]
    fn test_extract_zip_enforces_uncompressed_limit() {
        let dir = tempfile::TempDir::new().unwrap();
        let zip_path = dir.path().join("pack.zip");
        let extract_dir = dir.path().join("extract");
        fs::create_dir_all(&extract_dir).unwrap();
        write_zip(&zip_path, &[("a.txt", b"0123456789")]);

        let error = extract_archive(&zip_path, &extract_dir, 5).unwrap_err();
        assert!(error.to_string().contains("上限"));

        assert!(extract_archive(&zip_path, &extract_dir, 10).is_ok());
    }

    #[test]
    fn test_extract_zip_skips_path_traversal() {
        let dir = tempfile::TempDir::new().unwrap();
        let zip_path = dir.path().join("pack.zip");
        let extract_dir = dir.path().join("extract");
        fs::create_dir_all(&extract_dir).unwrap();
        write_zip(&zip_path, &[("ok.txt", b"ok"), ("../evil.txt", b"evil")]);

        extract_archive(&zip_path, &extract_dir, 1024).unwrap();

        assert!(extract_dir.join("ok.txt").exists());
        assert!(!dir.path().join("evil.txt").exists());
    }

    fn deps(value: serde_json::Value) -> ModrinthDependencies {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn test_dependency_loader_accepts_both_fabric_keys() {
        let loader = deps(json!({ "minecraft": "1.20.1", "fabric-loader": "0.15.0" }))
            .loader()
            .unwrap();
        assert!(matches!(
            loader,
            LoaderType::Fabric {
                ref mc_version,
                ref loader_version
            } if mc_version == "1.20.1" && loader_version == "0.15.0"
        ));

        let legacy = deps(json!({ "minecraft": "1.20.1", "fabric": "0.15.0" }))
            .loader()
            .unwrap();
        assert!(matches!(legacy, LoaderType::Fabric { .. }));
    }

    #[test]
    fn test_dependency_loader_prefers_forge_and_ignores_blank_versions() {
        let loaded = deps(json!({
            "minecraft": "1.19.2",
            "forge": "43.2.0",
            "fabric-loader": "0.15.0"
        }));
        assert!(matches!(
            loaded.loader().unwrap(),
            LoaderType::Forge { ref loader_version, .. } if loader_version == "43.2.0"
        ));
        assert_eq!(loaded.loader_slug(), Some("forge"));

        // 空串等同于没写，不能当成 "0 版本" 去装
        let blank = deps(json!({ "minecraft": "1.20.1", "fabric-loader": "  " }));
        assert!(blank.loader().is_none());
        assert!(blank.loader_slug().is_none());
    }

    #[test]
    fn test_server_only_files_are_not_client_files() {
        let mut file = ModrinthIndexFile {
            path: "mods/x.jar".to_string(),
            hashes: ModrinthIndexHashes::default(),
            downloads: vec![],
            file_size: None,
            env: None,
        };
        assert!(is_client_file(&file));

        file.env = Some(ModrinthFileEnv {
            client: Some("unsupported".to_string()),
            server: Some("required".to_string()),
        });
        assert!(!is_client_file(&file));
    }

    #[test]
    fn test_validate_index_rejects_unsafe_paths_and_wrong_format() {
        let mut index: super::ModrinthIndex = serde_json::from_value(json!({
            "formatVersion": 1,
            "game": "minecraft",
            "versionId": "1.0.0",
            "name": "Test",
            "files": [{ "path": "mods/a.jar", "hashes": { "sha1": "abc" }, "downloads": ["https://x"] }],
            "dependencies": { "minecraft": "1.20.1" }
        }))
        .unwrap();
        assert!(validate_index(&index).is_ok());

        index.files[0].path = "../evil.jar".to_string();
        assert!(validate_index(&index).is_err());
        index.files[0].path = "mods/a.jar".to_string();

        index.format_version = 2;
        assert!(validate_index(&index).is_err());
        index.format_version = 1;

        index.game = "other".to_string();
        assert!(validate_index(&index).is_err());
    }
}
