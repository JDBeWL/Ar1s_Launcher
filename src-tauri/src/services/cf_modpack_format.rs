//! CurseForge 整合包（zip + manifest.json）格式
//!
//! 只负责结构解析与「CF 清单 → Modrinth 同构清单」的字段换算；
//! 文件下载条目由 curseforge 服务在线解析，overrides 复用 mrpack 的解压逻辑

use crate::errors::LauncherError;
use crate::services::modpack_format::ModrinthDependencies;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::Read;
use std::path::Path;

pub const CF_MANIFEST_FILE: &str = "manifest.json";
/// 目前只有 manifestVersion 1，别的值宁可拒绝也不要瞎猜字段含义
pub const CF_SUPPORTED_MANIFEST_VERSION: u32 = 1;
/// manifest 本身不可能有这么大，超过就是恶意包
const MAX_MANIFEST_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CfManifest {
    pub minecraft: CfMinecraft,
    #[serde(rename = "manifestType")]
    pub manifest_type: String,
    #[serde(rename = "manifestVersion")]
    pub manifest_version: u32,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub files: Vec<CfManifestFile>,
    /// overrides 目录名，个别打包工具会改名，默认 overrides
    #[serde(default)]
    pub overrides: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CfMinecraft {
    pub version: String,
    #[serde(rename = "modLoaders")]
    pub mod_loaders: Vec<CfModLoader>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CfModLoader {
    /// 形如 `forge-47.2.0`、`fabric-loader-0.15.0`、`neoforge-20.4.237`
    pub id: String,
    #[serde(default)]
    pub primary: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CfManifestFile {
    #[serde(rename = "projectID")]
    pub project_id: u64,
    #[serde(rename = "fileID")]
    pub file_id: u64,
    #[serde(default = "default_true")]
    pub required: bool,
}

fn default_true() -> bool {
    true
}

impl CfManifestFile {
    pub fn as_ref(&self) -> crate::services::curseforge::CfManifestRef {
        crate::services::curseforge::CfManifestRef {
            project_id: self.project_id,
            file_id: self.file_id,
        }
    }
}

/// 只从压缩包里读 manifest，不解压整个包
pub fn read_manifest_from_archive(path: &Path) -> Result<CfManifest, LauncherError> {
    let file = File::open(path)?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| {
        LauncherError::Custom(format!("无法读取整合包（可能不是 zip 格式）: {}", e))
    })?;

    let entry = archive.by_name(CF_MANIFEST_FILE).map_err(|_| {
        LauncherError::Custom(format!(
            "整合包缺少 {}，可能不是 CurseForge 格式",
            CF_MANIFEST_FILE
        ))
    })?;

    if entry.size() > MAX_MANIFEST_BYTES {
        return Err(LauncherError::Custom(format!(
            "{} 超过 {} 字节上限，已拒绝",
            CF_MANIFEST_FILE, MAX_MANIFEST_BYTES
        )));
    }

    let mut text = String::new();
    // zip 头声明的 size 可以造假，按实际读到的字节流再限一次
    entry.take(MAX_MANIFEST_BYTES).read_to_string(&mut text)?;
    let manifest: CfManifest = serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .map_err(|e| LauncherError::Custom(format!("解析 {} 失败: {}", CF_MANIFEST_FILE, e)))?;

    validate_manifest(&manifest)?;
    Ok(manifest)
}

pub fn validate_manifest(manifest: &CfManifest) -> Result<(), LauncherError> {
    if manifest.manifest_version != CF_SUPPORTED_MANIFEST_VERSION {
        return Err(LauncherError::Custom(format!(
            "不支持的 CurseForge 清单版本 {}（仅支持 {}）",
            manifest.manifest_version, CF_SUPPORTED_MANIFEST_VERSION
        )));
    }
    if manifest.minecraft.version.trim().is_empty() {
        return Err(LauncherError::Custom(
            "整合包没有声明 Minecraft 版本".to_string(),
        ));
    }
    if let Some(bad) = manifest
        .files
        .iter()
        .find(|file| file.project_id == 0 || file.file_id == 0)
    {
        return Err(LauncherError::Custom(format!(
            "整合包清单包含非法文件引用，已拒绝安装: projectID {} fileID {}",
            bad.project_id, bad.file_id
        )));
    }
    Ok(())
}

/// 加载器标识 → (前缀, 剩余版本号)；`forge-47.2.0` → ("forge", "47.2.0")
///
/// `fabric-loader-0.15.0` 的前缀本身带连字符，所以按已知前缀最长优先匹配，
/// 而不是简单地在第一个 `-` 处切分
fn split_loader_id(loader_id: &str) -> Option<(&'static str, &str)> {
    const PREFIXES: [&str; 5] = [
        "fabric-loader",
        "quilt-loader",
        "neoforge",
        "quilt",
        "forge",
    ];
    let loader_id = loader_id.trim();
    for prefix in PREFIXES {
        if let Some(rest) = loader_id.strip_prefix(prefix)
            && let Some(version) = rest.strip_prefix('-')
            && !version.trim().is_empty()
        {
            return Some((prefix, version));
        }
    }
    None
}

impl CfManifest {
    /// CF 的 modLoaders[primary] → Modrinth dependencies，安装流水线只认后者
    pub fn to_dependencies(&self) -> ModrinthDependencies {
        let mut deps = ModrinthDependencies {
            minecraft: self.minecraft.version.trim().to_string(),
            ..Default::default()
        };

        let loader_id = self
            .minecraft
            .mod_loaders
            .iter()
            .find(|loader| loader.primary)
            .or_else(|| self.minecraft.mod_loaders.first())
            .map(|loader| loader.id.trim())
            .filter(|id| !id.is_empty());

        if let Some(loader_id) = loader_id
            && let Some((prefix, version)) = split_loader_id(loader_id)
        {
            match prefix {
                "forge" => deps.forge = Some(version.to_string()),
                "fabric-loader" => deps.fabric_loader = Some(version.to_string()),
                "quilt-loader" => deps.quilt_loader = Some(version.to_string()),
                "neoforge" => deps.neoforge = Some(version.to_string()),
                // PREFIXES 里没有单独的 fabric，保守起见归到 fabric
                _ => deps.fabric_loader = Some(version.to_string()),
            }
        }

        deps
    }

    /// manifest files 里客户端需要的条目；required=false 是可选文件，装了也无害但尊重作者意图跳过
    pub fn client_file_refs(&self) -> Vec<crate::services::curseforge::CfManifestRef> {
        self.files
            .iter()
            .filter(|file| file.required)
            .map(|file| file.as_ref())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CF_MANIFEST_FILE, CfManifest, read_manifest_from_archive, split_loader_id,
        validate_manifest,
    };
    use crate::services::loaders::LoaderType;
    use serde_json::json;
    use std::fs;
    use std::io::Write;
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    fn sample_manifest() -> CfManifest {
        serde_json::from_value(json!({
            "minecraft": {
                "version": "1.20.1",
                "modLoaders": [{ "id": "forge-47.2.0", "primary": true }]
            },
            "manifestType": "minecraftModpack",
            "manifestVersion": 1,
            "name": "Test Pack",
            "version": "1.2.3",
            "files": [
                { "projectID": 238222, "fileID": 9298210, "required": true },
                { "projectID": 999, "fileID": 1234, "required": false }
            ],
            "overrides": "overrides"
        }))
        .unwrap()
    }

    #[test]
    fn test_split_loader_id_matches_longest_prefix_first() {
        assert_eq!(
            split_loader_id("fabric-loader-0.15.0"),
            Some(("fabric-loader", "0.15.0"))
        );
        assert_eq!(split_loader_id("forge-47.2.0"), Some(("forge", "47.2.0")));
        assert_eq!(
            split_loader_id("neoforge-20.4.237"),
            Some(("neoforge", "20.4.237"))
        );
        assert_eq!(
            split_loader_id("quilt-loader-0.21.0"),
            Some(("quilt-loader", "0.21.0"))
        );
        // 不认识的或没带版本号的都不算
        assert_eq!(split_loader_id("rift-1.0"), None);
        assert_eq!(split_loader_id("forge-"), None);
        assert_eq!(split_loader_id("forge"), None);
    }

    #[test]
    fn test_to_dependencies_maps_primary_loader() {
        let deps = sample_manifest().to_dependencies();
        assert_eq!(deps.minecraft, "1.20.1");
        assert!(matches!(
            deps.loader(),
            Some(LoaderType::Forge { ref mc_version, ref loader_version })
                if mc_version == "1.20.1" && loader_version == "47.2.0"
        ));
    }

    #[test]
    fn test_to_dependencies_ignores_non_primary_loaders() {
        let manifest: CfManifest = serde_json::from_value(json!({
            "minecraft": {
                "version": "1.20.1",
                "modLoaders": [
                    { "id": "forge-47.2.0", "primary": false },
                    { "id": "fabric-loader-0.15.0", "primary": true }
                ]
            },
            "manifestType": "minecraftModpack",
            "manifestVersion": 1,
            "files": []
        }))
        .unwrap();
        let deps = manifest.to_dependencies();
        assert!(matches!(deps.loader(), Some(LoaderType::Fabric { .. })));
    }

    #[test]
    fn test_client_file_refs_skip_optional_entries() {
        let refs = sample_manifest().client_file_refs();
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].project_id, 238222);
        assert_eq!(refs[0].file_id, 9298210);
    }

    #[test]
    fn test_validate_manifest_rejects_bad_versions_and_refs() {
        let mut manifest = sample_manifest();
        assert!(validate_manifest(&manifest).is_ok());

        manifest.manifest_version = 2;
        assert!(validate_manifest(&manifest).is_err());
        manifest.manifest_version = 1;

        manifest.files[0].project_id = 0;
        assert!(validate_manifest(&manifest).is_err());
        manifest.files[0].project_id = 238222;

        manifest.minecraft.version = "  ".to_string();
        assert!(validate_manifest(&manifest).is_err());
    }

    #[test]
    fn test_read_manifest_from_archive_roundtrip() {
        let dir = tempfile::TempDir::new().unwrap();
        let zip_path = dir.path().join("pack.zip");
        let file = fs::File::create(&zip_path).unwrap();
        let mut zip = ZipWriter::new(file);
        zip.start_file(CF_MANIFEST_FILE, SimpleFileOptions::default())
            .unwrap();
        zip.write_all(&serde_json::to_vec(&sample_manifest()).unwrap())
            .unwrap();
        zip.start_file("overrides/config.toml", SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"key = 1").unwrap();
        zip.finish().unwrap();

        let manifest = read_manifest_from_archive(&zip_path).unwrap();
        assert_eq!(manifest.name, "Test Pack");
        assert_eq!(manifest.files.len(), 2);
    }

    #[test]
    fn test_read_manifest_rejects_zip_without_manifest() {
        let dir = tempfile::TempDir::new().unwrap();
        let zip_path = dir.path().join("pack.zip");
        let file = fs::File::create(&zip_path).unwrap();
        let mut zip = ZipWriter::new(file);
        zip.start_file("other.json", SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"{}").unwrap();
        zip.finish().unwrap();

        let error = read_manifest_from_archive(&zip_path).unwrap_err();
        assert!(error.to_string().contains("CurseForge"));
    }
}
