//! 资源包管理
//!
//! 扫描实例的 `resourcepacks` 目录，读取 `pack.mcmeta` 与 `pack.png` 做展示。
//! 启用状态只存在 `options.txt` 的 `resourcePacks` 列表里，不像模组靠改文件名，所以启动器不能另造一套状态。

use crate::errors::LauncherError;
use crate::services::{config, launcher};
use crate::utils::{file_utils, minecraft};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use log::{info, warn};
use serde::Serialize;
use serde_json::Value;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

/// 图标要转成 data URL 走 IPC，太大会拖慢列表刷新
const MAX_ICON_BYTES: u64 = 512 * 1024;
/// pack.mcmeta 正常只有几百字节，超过这个量说明不是资源包
const MAX_META_BYTES: u64 = 1024 * 1024;
/// 文件夹形式的资源包统计体积时的递归深度上限
const MAX_DIR_DEPTH: u8 = 4;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourcePackInfo {
    /// 磁盘上的名字：`.zip` 文件名或文件夹名，也是 `resourcePacks` 里 `file/` 后面那一段
    pub file_name: String,
    /// pack.mcmeta 里的描述；读不到时回退成文件名
    pub display_name: String,
    pub description: Option<String>,
    /// pack.mcmeta 声明的最低格式
    pub min_format: Option<i64>,
    /// pack.mcmeta 声明的最高格式
    pub max_format: Option<i64>,
    /// 相对当前实例资源包格式的判定：compatible / outdated / tooNew / unknown
    pub compatibility: &'static str,
    pub enabled: bool,
    pub size: u64,
    pub is_directory: bool,
    /// `pack.png` 的 data URL
    pub icon_data_url: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportSummary {
    pub imported: Vec<String>,
    pub skipped: Vec<String>,
}

/// pack.mcmeta 里跟兼容性相关的字段
#[derive(Debug, Default, PartialEq, Eq)]
struct PackMeta {
    description: Option<String>,
    min_format: Option<i64>,
    max_format: Option<i64>,
}

fn resourcepacks_dir(instance_name: &str) -> Result<(PathBuf, PathBuf), LauncherError> {
    file_utils::validate_instance_name_or_error(instance_name)?;
    let config = config::load_config()?;
    let game_dir = PathBuf::from(&config.game_dir);
    let root = launcher::isolation::content_root_for(&game_dir, instance_name);
    Ok((game_dir, root.join("resourcepacks")))
}

/// options.txt 与游戏工作目录同源：开隔离在实例目录，关隔离在根目录
fn options_path(game_dir: &Path, instance_name: &str) -> PathBuf {
    launcher::isolation::content_root_for(game_dir, instance_name).join("options.txt")
}

pub fn list_resource_packs(instance_name: &str) -> Result<Vec<ResourcePackInfo>, LauncherError> {
    let (game_dir, packs_dir) = resourcepacks_dir(instance_name)?;
    let enabled_ids = read_enabled_ids(&options_path(&game_dir, instance_name));
    let game_format = game_resource_pack_format(&game_dir, instance_name);

    if !packs_dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut packs = Vec::new();
    for entry in fs::read_dir(&packs_dir)? {
        let entry = entry?;
        let path = entry.path();
        let Some(file_name) = path
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_string)
        else {
            continue;
        };

        let is_directory = path.is_dir();
        if is_directory {
            // 只认带 pack.mcmeta 的目录，避免把随手建的文件夹也列出来
            if !path.join("pack.mcmeta").is_file() {
                continue;
            }
        } else if !file_name.to_ascii_lowercase().ends_with(".zip") {
            continue;
        }

        let (meta, icon_data_url) = read_pack_details(&path, is_directory);
        let compatibility = classify_compatibility(&meta, game_format);
        let display_name = meta
            .description
            .clone()
            .filter(|text| !text.trim().is_empty())
            .unwrap_or_else(|| file_name.clone());

        packs.push(ResourcePackInfo {
            enabled: is_enabled(&enabled_ids, &file_name),
            display_name,
            description: meta.description,
            min_format: meta.min_format,
            max_format: meta.max_format,
            compatibility,
            size: entry_size(&path, is_directory, 0),
            icon_data_url,
            is_directory,
            file_name,
        });
    }

    // 键只算一次：比较器里现算 to_lowercase 每次比较都要分配两个 String
    packs.sort_by_cached_key(|pack| {
        (
            std::cmp::Reverse(pack.enabled),
            pack.display_name.to_lowercase(),
        )
    });
    Ok(packs)
}

pub async fn set_resource_pack_enabled(
    instance_name: String,
    file_name: String,
    enabled: bool,
) -> Result<(), LauncherError> {
    let (game_dir, packs_dir) = resourcepacks_dir(&instance_name)?;
    let _instance_lock = crate::services::instance::acquire_instance_lock(&game_dir).await?;

    let pack_path = resolve_pack_path(&packs_dir, &file_name)?;
    if !pack_path.exists() {
        return Err(LauncherError::Custom(format!(
            "资源包不存在: {}",
            file_name
        )));
    }

    let path = options_path(&game_dir, &instance_name);
    let (mut lines, crlf) = read_options(&path)?;
    let raw = find_option(&lines, "resourcePacks").unwrap_or_else(|| "[]".to_string());
    let mut ids = parse_pack_list(&raw);
    let before = ids.clone();

    if enabled {
        if !is_enabled(&ids, &file_name) {
            // 游戏自己写进去的就是 `file/` 前缀形态，跟着写才能被认出来
            ids.push(format!("file/{}", file_name));
        }
    } else {
        let target = pack_key(&file_name);
        ids.retain(|id| pack_key(id) != target);
    }

    // 状态没变就别碰 options.txt：写一次就多一次把玩家设置写坏的机会
    if ids != before {
        let serialized = serde_json::to_string(&ids)?;
        set_option(&mut lines, "resourcePacks", &serialized);
        write_options(&path, &lines, crlf)?;
    }

    info!(
        "资源包 {} 已{}（实例 {}）",
        file_name,
        if enabled { "启用" } else { "禁用" },
        instance_name
    );
    Ok(())
}

pub async fn delete_resource_pack(
    instance_name: String,
    file_name: String,
) -> Result<(), LauncherError> {
    let (game_dir, packs_dir) = resourcepacks_dir(&instance_name)?;
    let _instance_lock = crate::services::instance::acquire_instance_lock(&game_dir).await?;

    let path = resolve_pack_path(&packs_dir, &file_name)?;
    if !path.exists() {
        return Err(LauncherError::Custom(format!(
            "资源包不存在: {}",
            file_name
        )));
    }

    // 先把它从启用列表里摘掉，否则 options.txt 会留一条指向空文件名的记录
    let options = options_path(&game_dir, &instance_name);
    let (mut lines, crlf) = read_options(&options)?;
    if let Some(raw) = find_option(&lines, "resourcePacks") {
        let mut ids = parse_pack_list(&raw);
        let target = pack_key(&file_name);
        let before = ids.len();
        ids.retain(|id| pack_key(id) != target);
        if ids.len() != before {
            set_option(&mut lines, "resourcePacks", &serde_json::to_string(&ids)?);
            write_options(&options, &lines, crlf)?;
        }
    }

    if path.is_dir() {
        fs::remove_dir_all(&path)?;
    } else {
        fs::remove_file(&path)?;
    }
    info!("已删除资源包: {}", path.display());
    Ok(())
}

pub async fn import_resource_packs(
    instance_name: String,
    source_paths: Vec<String>,
) -> Result<ImportSummary, LauncherError> {
    let (game_dir, packs_dir) = resourcepacks_dir(&instance_name)?;
    if source_paths.is_empty() {
        return Ok(ImportSummary {
            imported: Vec::new(),
            skipped: Vec::new(),
        });
    }
    let _instance_lock = crate::services::instance::acquire_instance_lock(&game_dir).await?;
    fs::create_dir_all(&packs_dir)?;

    let mut summary = ImportSummary {
        imported: Vec::new(),
        skipped: Vec::new(),
    };

    for source in source_paths {
        let source_path = PathBuf::from(&source);
        let Some(name) = source_path
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_string)
        else {
            summary.skipped.push(source.clone());
            continue;
        };

        // 来源是用户随手挑的路径，落盘前仍按文件名规则过一遍
        if file_utils::validate_file_name_or_error(&name).is_err() {
            warn!("跳过文件名不合法的资源包: {}", source);
            summary.skipped.push(source.clone());
            continue;
        }

        let target = packs_dir.join(&name);
        if target.exists() {
            summary.skipped.push(name);
            continue;
        }

        let result = if source_path.is_dir() {
            if !source_path.join("pack.mcmeta").is_file() {
                Err(LauncherError::Custom("不是资源包目录".to_string()))
            } else {
                // 递归复制整个资源包目录，文件多时会长时间占住线程
                let source_owned = source_path.clone();
                let target_owned = target.clone();
                tokio::task::spawn_blocking(move || {
                    file_utils::copy_dir_all(&source_owned, &target_owned)
                        .map_err(LauncherError::Io)
                })
                .await
                .map_err(|e| LauncherError::Custom(format!("导入资源包失败: {}", e)))?
            }
        } else if source_path.is_file() {
            fs::copy(&source_path, &target)
                .map(|_| ())
                .map_err(LauncherError::Io)
        } else {
            Err(LauncherError::Custom("来源路径不存在".to_string()))
        };

        match result {
            Ok(()) => {
                info!("已导入资源包: {}", name);
                summary.imported.push(name);
            }
            Err(error) => {
                warn!("导入资源包 {} 失败: {}", name, error);
                summary.skipped.push(name);
            }
        }
    }

    Ok(summary)
}

pub async fn open_resourcepacks_folder(instance_name: String) -> Result<(), LauncherError> {
    let (_, packs_dir) = resourcepacks_dir(&instance_name)?;
    fs::create_dir_all(&packs_dir)?;
    opener::open(&packs_dir)
        .map_err(|e| LauncherError::Custom(format!("无法打开资源包文件夹: {}", e)))?;
    Ok(())
}

/// 只允许传单个名字；带路径分隔符或 `..` 就能跳出 resourcepacks 目录
fn resolve_pack_path(packs_dir: &Path, file_name: &str) -> Result<PathBuf, LauncherError> {
    if file_name.is_empty()
        || file_name.contains('/')
        || file_name.contains('\\')
        || file_name.contains("..")
        || file_name.contains(':')
    {
        return Err(LauncherError::Custom("资源包文件名无效".to_string()));
    }
    Ok(packs_dir.join(file_name))
}

fn entry_size(path: &Path, is_directory: bool, depth: u8) -> u64 {
    if !is_directory {
        return path.metadata().map(|meta| meta.len()).unwrap_or(0);
    }
    if depth >= MAX_DIR_DEPTH {
        return 0;
    }
    let Ok(entries) = fs::read_dir(path) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| {
            let child = entry.path();
            entry_size(&child, child.is_dir(), depth + 1)
        })
        .fold(0u64, u64::saturating_add)
}

/// 一次打开 zip 同时取 metadata 与图标
/// 同一个包的中央目录只解析一遍，50 个包不再变成 100 次打开
fn read_pack_details(path: &Path, is_directory: bool) -> (PackMeta, Option<String>) {
    if is_directory {
        let meta = fs::read(path.join("pack.mcmeta"))
            .ok()
            .map(|bytes| parse_pack_mcmeta(&String::from_utf8_lossy(&bytes)))
            .unwrap_or_default();

        let icon = path.join("pack.png");
        let icon = icon
            .metadata()
            .ok()
            .filter(|meta| meta.len() <= MAX_ICON_BYTES)
            .and_then(|_| fs::read(&icon).ok())
            .and_then(encode_icon);

        return (meta, icon);
    }

    let Ok(file) = fs::File::open(path) else {
        return (PackMeta::default(), None);
    };
    let Ok(mut archive) = zip::ZipArchive::new(file) else {
        return (PackMeta::default(), None);
    };

    let meta = read_zip_entry(&mut archive, "pack.mcmeta")
        .map(|bytes| parse_pack_mcmeta(&String::from_utf8_lossy(&bytes)))
        .unwrap_or_default();
    let icon = read_zip_entry(&mut archive, "pack.png").and_then(encode_icon);

    (meta, icon)
}

/// 图标上限之外的直接丢掉：列表里塞一个几 MB 的 data URL 只会拖慢渲染
fn encode_icon(bytes: Vec<u8>) -> Option<String> {
    if bytes.len() as u64 > MAX_ICON_BYTES {
        return None;
    }
    Some(format!("data:image/png;base64,{}", STANDARD.encode(bytes)))
}

fn read_zip_entry(archive: &mut zip::ZipArchive<fs::File>, name: &str) -> Option<Vec<u8>> {
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).ok()?;
        let entry_name = entry.name().ok()?.to_string();
        if !entry_name.eq_ignore_ascii_case(name) {
            continue;
        }
        if entry.size() > MAX_META_BYTES {
            return None;
        }
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).ok()?;
        return Some(bytes);
    }
    None
}

/// `pack.mcmeta` 的格式字段有四种写法：单个数字、`[major, minor]`、
/// `min_format`/`max_format` 一对，以及旧的 `supported_formats` 区间
fn parse_pack_mcmeta(text: &str) -> PackMeta {
    let Ok(json) = serde_json::from_str::<Value>(text) else {
        return PackMeta::default();
    };
    let pack = json.get("pack").unwrap_or(&json);

    let description = pack.get("description").map(text_component_to_string);
    let mut min_format = pack.get("min_format").and_then(format_value);
    let mut max_format = pack.get("max_format").and_then(format_value);

    if let Some(supported) = pack.get("supported_formats") {
        min_format = min_format.or_else(|| supported.get("min_inclusive").and_then(format_value));
        max_format = max_format.or_else(|| supported.get("max_inclusive").and_then(format_value));
        if supported.is_array() {
            let values: Vec<i64> = supported
                .as_array()
                .map(|items| items.iter().filter_map(format_value).collect())
                .unwrap_or_default();
            if let (Some(first), Some(last)) = (values.first(), values.last()) {
                min_format = min_format.or(Some(*first));
                max_format = max_format.or(Some(*last));
            }
        }
    }

    if let Some(legacy) = pack.get("pack_format").and_then(format_value) {
        min_format = min_format.or(Some(legacy));
        max_format = max_format.or(Some(legacy));
    }

    PackMeta {
        description,
        min_format,
        max_format,
    }
}

/// `pack_format` 可以是整数，也可以是 `[major, minor]`；带 minor 时取主版本
fn format_value(value: &Value) -> Option<i64> {
    if let Some(number) = value.as_i64() {
        return Some(number);
    }
    if let Some(array) = value.as_array() {
        return array.first().and_then(Value::as_i64);
    }
    value.get("major").and_then(Value::as_i64)
}

/// 描述可能是纯字符串，也可能是文本组件
fn text_component_to_string(value: &Value) -> String {
    if let Some(text) = value.as_str() {
        return text.to_string();
    }
    if let Some(text) = value.get("text").and_then(Value::as_str) {
        return text.to_string();
    }
    if let Some(key) = value.get("translate").and_then(Value::as_str) {
        return key.to_string();
    }
    if let Some(extra) = value.get("extra").and_then(Value::as_array) {
        return extra
            .iter()
            .map(text_component_to_string)
            .collect::<Vec<_>>()
            .join("");
    }
    String::new()
}

fn classify_compatibility(meta: &PackMeta, game_format: Option<i64>) -> &'static str {
    let (Some(game), Some(min), Some(max)) = (game_format, meta.min_format, meta.max_format) else {
        return "unknown";
    };
    if max < game {
        "outdated"
    } else if min > game {
        "tooNew"
    } else {
        "compatible"
    }
}

/// 当前实例游戏本体支持的资源包格式；从 client jar 里的 `version.json` 读，读不到就不下结论
fn game_resource_pack_format(game_dir: &Path, instance_name: &str) -> Option<i64> {
    let json_path = game_dir
        .join("versions")
        .join(instance_name)
        .join(format!("{}.json", instance_name));
    let json: Value = serde_json::from_str(&fs::read_to_string(json_path).ok()?).ok()?;
    let jar_version = minecraft::find_jar_version(&json, game_dir).ok()?;
    let jar_path = game_dir
        .join("versions")
        .join(&jar_version)
        .join(format!("{}.jar", jar_version));

    let file = fs::File::open(jar_path).ok()?;
    let mut archive = zip::ZipArchive::new(file).ok()?;
    let bytes = read_zip_entry(&mut archive, "version.json")?;
    let version_json: Value = serde_json::from_slice(&bytes).ok()?;
    let pack_version = version_json.get("pack_version")?;
    pack_version
        .get("resource")
        .and_then(format_value)
        .or_else(|| format_value(pack_version))
}

/// 读不到文件时返回空列表（游戏还没写过 options.txt）；
/// 读得到但解析不了的内容原样保留，只当没有 resourcePacks 这一行处理
fn read_options(path: &Path) -> Result<(Vec<String>, bool), LauncherError> {
    if !path.exists() {
        return Ok((Vec::new(), false));
    }
    let content = fs::read_to_string(path)?;
    // options.txt 由游戏写，行尾可能是 CRLF；读进来时统一剥掉，写回时再还原
    let crlf = content.contains("\r\n");
    let lines = content
        .split('\n')
        .map(|line| line.trim_end_matches('\r').to_string())
        .collect();
    Ok((lines, crlf))
}

fn write_options(path: &Path, lines: &[String], crlf: bool) -> Result<(), LauncherError> {
    let separator = if crlf { "\r\n" } else { "\n" };
    let mut text = lines.join(separator);
    // 保留文件末尾的换行，游戏自己也是这么写的
    if !text.is_empty() && !text.ends_with(separator) {
        text.push_str(separator);
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    // 先写临时文件再走 .bak 回滚替换：中途失败不会留下半截 options.txt，把玩家的设置全弄丢
    let temp = path.with_extension("txt.ar1s-tmp");
    fs::write(&temp, text)?;
    crate::utils::file_utils::replace_file_atomically(&temp, path)?;
    Ok(())
}

fn find_option(lines: &[String], key: &str) -> Option<String> {
    let prefix = format!("{}:", key);
    lines
        .iter()
        .find_map(|line| line.strip_prefix(&prefix).map(str::to_string))
}

fn set_option(lines: &mut Vec<String>, key: &str, value: &str) {
    let prefix = format!("{}:", key);
    let new_line = format!("{}{}", prefix, value);
    match lines.iter().position(|line| line.starts_with(&prefix)) {
        Some(index) => lines[index] = new_line,
        None => lines.push(new_line),
    }
}

/// 游戏把选中项写成 `["vanilla","file/xxx.zip"]`；解析失败时按逗号硬拆，别让一条坏数据锁死整个面板
fn parse_pack_list(raw: &str) -> Vec<String> {
    let trimmed = raw.trim();
    if let Ok(Value::Array(items)) = serde_json::from_str::<Value>(trimmed) {
        return items
            .iter()
            .filter_map(|item| item.as_str().map(str::to_string))
            .collect();
    }

    let inner = trimmed.trim_start_matches('[').trim_end_matches(']').trim();
    if inner.is_empty() {
        return Vec::new();
    }
    inner
        .split(',')
        .map(|item| item.trim().trim_matches('"').trim_matches('\'').to_string())
        .filter(|item| !item.is_empty())
        .collect()
}

/// 比较启用状态时忽略 `file/` 前缀与大小写：不同版本写进去的形态并不完全一致
fn pack_key(id: &str) -> String {
    let trimmed = id.trim().trim_matches('"');
    let without_prefix = trimmed.strip_prefix("file/").unwrap_or(trimmed);
    without_prefix.to_lowercase()
}

fn is_enabled(ids: &[String], file_name: &str) -> bool {
    let target = pack_key(file_name);
    ids.iter().any(|id| pack_key(id) == target)
}

/// options.txt 里已经选中的资源包 id 列表；读不出来时按「一个都没启用」展示，
/// 但不影响写回（写回路径会自己再读一次，失败就报错而不是覆盖）
fn read_enabled_ids(options: &Path) -> Vec<String> {
    read_options(options)
        .ok()
        .and_then(|(lines, _)| find_option(&lines, "resourcePacks"))
        .map(|raw| parse_pack_list(&raw))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{
        PackMeta, classify_compatibility, find_option, is_enabled, pack_key, parse_pack_list,
        parse_pack_mcmeta, read_options, set_option, write_options,
    };
    use std::fs;
    use std::io::Write;
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    /// 造一个带 pack.mcmeta / pack.png 的真实 zip，验证从压缩包里读元数据那条路径
    fn write_pack_zip(path: &std::path::Path, mcmeta: &str, png: Option<&[u8]>) {
        let file = fs::File::create(path).unwrap();
        let mut zip = ZipWriter::new(file);
        let options = SimpleFileOptions::default();

        zip.start_file("pack.mcmeta", options).unwrap();
        zip.write_all(mcmeta.as_bytes()).unwrap();

        if let Some(bytes) = png {
            zip.start_file("pack.png", options).unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap();
    }

    #[test]
    fn test_parse_pack_list_handles_json_and_broken_data() {
        assert_eq!(
            parse_pack_list(r#"["vanilla","file/a.zip"]"#),
            vec!["vanilla".to_string(), "file/a.zip".to_string()]
        );
        // 引号丢失时仍然能读出来
        assert_eq!(
            parse_pack_list("[vanilla, file/a.zip]"),
            vec!["vanilla".to_string(), "file/a.zip".to_string()]
        );
        assert!(parse_pack_list("[]").is_empty());
        assert!(parse_pack_list("").is_empty());
    }

    #[test]
    fn test_pack_key_ignores_file_prefix_and_case() {
        assert_eq!(pack_key("file/Foo.ZIP"), pack_key("foo.zip"));
        assert!(is_enabled(
            &["vanilla".to_string(), "file/Foo.ZIP".to_string()],
            "foo.zip"
        ));
        assert!(!is_enabled(&["vanilla".to_string()], "foo.zip"));
    }

    #[test]
    fn test_parse_pack_mcmeta_legacy_and_new_formats() {
        let legacy = parse_pack_mcmeta(r#"{"pack":{"pack_format":15,"description":"旧格式"}}"#);
        assert_eq!(legacy.min_format, Some(15));
        assert_eq!(legacy.max_format, Some(15));
        assert_eq!(legacy.description.as_deref(), Some("旧格式"));

        let modern = parse_pack_mcmeta(r#"{"pack":{"min_format":[88,0],"max_format":[88,2]}}"#);
        assert_eq!(modern.min_format, Some(88));
        assert_eq!(modern.max_format, Some(88));

        let supported = parse_pack_mcmeta(
            r#"{"pack":{"pack_format":15,"supported_formats":{"min_inclusive":15,"max_inclusive":18}}}"#,
        );
        assert_eq!(supported.min_format, Some(15));
        assert_eq!(supported.max_format, Some(18));
    }

    #[test]
    fn test_parse_pack_mcmeta_reads_text_component_description() {
        let meta =
            parse_pack_mcmeta(r#"{"pack":{"pack_format":34,"description":{"text":"你好"}}}"#);
        assert_eq!(meta.description.as_deref(), Some("你好"));
    }

    #[test]
    fn test_parse_pack_mcmeta_tolerates_garbage() {
        assert_eq!(parse_pack_mcmeta("not json"), PackMeta::default());
        assert_eq!(parse_pack_mcmeta("{}"), PackMeta::default());
    }

    #[test]
    fn test_classify_compatibility() {
        let meta = PackMeta {
            min_format: Some(15),
            max_format: Some(18),
            ..PackMeta::default()
        };
        assert_eq!(classify_compatibility(&meta, Some(15)), "compatible");
        assert_eq!(classify_compatibility(&meta, Some(18)), "compatible");
        assert_eq!(classify_compatibility(&meta, Some(22)), "outdated");
        assert_eq!(classify_compatibility(&meta, Some(12)), "tooNew");
        // 读不到游戏格式时不下结论，界面不显示提示
        assert_eq!(classify_compatibility(&meta, None), "unknown");
    }

    #[test]
    fn test_option_round_trip_keeps_other_lines() {
        let mut lines = vec![
            "lang:zh_cn".to_string(),
            "resourcePacks:[\"vanilla\"]".to_string(),
            "fov:70".to_string(),
        ];

        assert_eq!(
            find_option(&lines, "resourcePacks").as_deref(),
            Some("[\"vanilla\"]")
        );

        set_option(&mut lines, "resourcePacks", "[\"vanilla\",\"file/a.zip\"]");
        assert_eq!(lines.len(), 3);
        assert_eq!(
            find_option(&lines, "resourcePacks").as_deref(),
            Some("[\"vanilla\",\"file/a.zip\"]")
        );

        set_option(&mut lines, "renderDistance", "12");
        assert_eq!(lines.last().map(String::as_str), Some("renderDistance:12"));
    }

    #[test]
    fn test_read_options_treats_missing_file_as_empty() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("options.txt");

        let (lines, crlf) = read_options(&path).unwrap();
        assert!(lines.is_empty());
        assert!(!crlf);
    }

    #[test]
    fn test_write_options_keeps_crlf_and_other_settings() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("options.txt");
        fs::write(&path, "lang:zh_cn\r\nfov:70\r\n").unwrap();

        let (mut lines, crlf) = read_options(&path).unwrap();
        assert!(crlf);
        set_option(&mut lines, "resourcePacks", "[\"file/a.zip\"]");
        write_options(&path, &lines, crlf).unwrap();

        let written = fs::read_to_string(&path).unwrap();
        assert!(written.contains("lang:zh_cn\r\n"));
        assert!(written.contains("fov:70\r\n"));
        assert!(written.contains("resourcePacks:[\"file/a.zip\"]"));
        // 游戏读的是 CRLF 文件，写回时不能混进裸 \n
        assert!(!written.replace("\r\n", "").contains('\n'));
    }

    #[test]
    fn test_read_pack_details_from_zip_and_directory() {
        let dir = tempfile::TempDir::new().unwrap();

        let zip_path = dir.path().join("pack.zip");
        write_pack_zip(
            &zip_path,
            r#"{"pack":{"pack_format":34,"description":"压缩包资源包"}}"#,
            None,
        );
        let (meta, icon) = super::read_pack_details(&zip_path, false);
        assert_eq!(meta.description.as_deref(), Some("压缩包资源包"));
        assert_eq!(meta.min_format, Some(34));
        assert!(icon.is_none());

        // 文件夹形式的资源包走的是直接读文件那条路
        let folder = dir.path().join("pack-folder");
        fs::create_dir_all(&folder).unwrap();
        fs::write(
            folder.join("pack.mcmeta"),
            r#"{"pack":{"min_format":88,"max_format":88,"description":"文件夹资源包"}}"#,
        )
        .unwrap();
        let (folder_meta, _) = super::read_pack_details(&folder, true);
        assert_eq!(folder_meta.description.as_deref(), Some("文件夹资源包"));
        assert_eq!(folder_meta.min_format, Some(88));
        assert_eq!(folder_meta.max_format, Some(88));
    }

    #[test]
    fn test_read_pack_details_falls_back_to_defaults_without_mcmeta() {
        let dir = tempfile::TempDir::new().unwrap();
        let zip_path = dir.path().join("not-a-pack.zip");
        write_pack_zip(&zip_path, "", None);
        // 空文件不是合法 JSON，解析不出元数据但也不该 panic
        let (meta, _) = super::read_pack_details(&zip_path, false);
        assert_eq!(meta, PackMeta::default());
    }

    #[test]
    fn test_read_pack_details_encodes_icon_as_data_url() {
        let dir = tempfile::TempDir::new().unwrap();
        let zip_path = dir.path().join("icon.zip");
        let png = b"\x89PNG\r\n\x1a\nfake";
        write_pack_zip(&zip_path, r#"{"pack":{"pack_format":34}}"#, Some(png));

        let (meta, icon) = super::read_pack_details(&zip_path, false);
        assert_eq!(meta.min_format, Some(34));
        let icon = icon.unwrap();
        assert!(icon.starts_with("data:image/png;base64,"));

        // 没有 pack.png 时返回 None，界面回退到默认图标
        let plain = dir.path().join("plain.zip");
        write_pack_zip(&plain, r#"{"pack":{"pack_format":34}}"#, None);
        assert!(super::read_pack_details(&plain, false).1.is_none());
    }
}
