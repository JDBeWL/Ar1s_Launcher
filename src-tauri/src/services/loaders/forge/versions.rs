//! 版本目录识别与版本号比较

use serde_json::Value;
use std::collections::HashSet;
use std::fs;
use std::path::Path;

pub(crate) fn snapshot_version_dirs(versions_dir: &Path) -> HashSet<String> {
    fs::read_dir(versions_dir)
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| e.path().is_dir())
                .map(|e| e.file_name().to_string_lossy().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// 先认预期 id，再认“安装后新出现的、以本版本为父的 Forge 目录”
pub(crate) fn find_installed_loader_dir(
    versions_dir: &Path,
    before: &HashSet<String>,
    expected_id: &str,
    mc_version: &str,
) -> Option<String> {
    let expected_json = versions_dir
        .join(expected_id)
        .join(format!("{}.json", expected_id));
    if expected_json.exists() {
        return Some(expected_id.to_string());
    }

    let entries = fs::read_dir(versions_dir).ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if before.contains(&name) {
            continue;
        }

        let Ok(content) = fs::read_to_string(entry.path().join(format!("{}.json", name))) else {
            continue;
        };
        let Ok(json) = serde_json::from_str::<Value>(&content) else {
            continue;
        };

        let same_base = json["inheritsFrom"].as_str() == Some(mc_version)
            || json["id"]
                .as_str()
                .is_some_and(|id| id.starts_with(mc_version));
        let is_forge = json["libraries"].as_array().is_some_and(|libs| {
            libs.iter().any(|lib| {
                lib["name"]
                    .as_str()
                    .is_some_and(|name| name.starts_with("net.minecraftforge"))
            })
        });

        if same_base && is_forge {
            return Some(name);
        }
    }

    None
}

pub(crate) fn is_new_forge(mc_version: &str) -> bool {
    crate::utils::minecraft::parse_mc_version(mc_version)
        .is_some_and(|(major, minor)| major == 1 && minor >= 13)
}

/// 安装器写入 versions 目录的 id，后续目录重命名依赖它
pub(crate) fn get_forge_version_id(mc_version: &str, forge_version: &str) -> String {
    format!("{}-forge-{}", mc_version, forge_version)
}

pub(crate) fn compare_forge_versions(a: &str, b: &str) -> std::cmp::Ordering {
    let parse = |s: &str| -> Vec<u32> { s.split('.').filter_map(|p| p.parse().ok()).collect() };

    let a_parts = parse(a);
    let b_parts = parse(b);

    for i in 0..std::cmp::max(a_parts.len(), b_parts.len()) {
        let a_num = a_parts.get(i).unwrap_or(&0);
        let b_num = b_parts.get(i).unwrap_or(&0);
        match a_num.cmp(b_num) {
            std::cmp::Ordering::Equal => continue,
            ord => return ord,
        }
    }
    std::cmp::Ordering::Equal
}
