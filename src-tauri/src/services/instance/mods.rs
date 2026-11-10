//! mods 目录：读取、启停、从 jar / mods.toml 里解析名称与版本

use super::{acquire_instance_lock, get_dirs};
use crate::errors::LauncherError;
use crate::services::launcher;
use crate::utils::file_utils::validate_instance_name_or_error;
use log::info;
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModInfo {
    /// 磁盘上的实际文件名，禁用时带 .disabled 后缀
    pub file_name: String,
    /// 展示名，读不到 jar 内元数据时回退成文件名
    pub display_name: String,
    pub version: Option<String>,
    pub enabled: bool,
    pub size: u64,
}

/// 定位 mods 目录下的某个文件，顺带挡住越界的文件名
pub(super) fn resolve_mod_path(
    instance_name: &str,
    file_name: &str,
) -> Result<(PathBuf, PathBuf), LauncherError> {
    let (game_dir, _) = get_dirs()?;
    let mods_dir = launcher::isolation::content_root_for(&game_dir, instance_name).join("mods");
    if !mods_dir.is_dir() {
        return Err(LauncherError::Custom(format!(
            "没有找到 mods 目录: {}",
            mods_dir.display()
        )));
    }

    // 前端只应该传文件名，带路径分隔符或 .. 就能跳出 mods 目录
    if file_name.is_empty()
        || file_name.contains('/')
        || file_name.contains('\\')
        || file_name.contains("..")
        || file_name.contains(':')
    {
        return Err(LauncherError::Custom("模组文件名无效".to_string()));
    }

    let path = mods_dir.join(file_name);
    if !path.is_file() {
        return Err(LauncherError::Custom(format!(
            "模组文件不存在: {}",
            file_name
        )));
    }
    Ok((mods_dir, path))
}

/// 从 jar 里读模组名和版本。Fabric / Quilt 走 JSON，Forge / NeoForge 走 mods.toml
///
/// 每次读都要打开 jar 解析中央目录；元数据只跟文件内容有关，「长度 + 修改时间」没变就返回上次结果
pub(super) fn read_mod_metadata(path: &Path) -> (Option<String>, Option<String>) {
    let meta = match fs::metadata(path) {
        Ok(meta) => meta,
        Err(_) => return (None, None),
    };
    let modified = meta.modified().ok();
    let len = meta.len();

    if let Some(cached) = cached_mod_metadata(path, len, modified) {
        return cached;
    }

    let resolved = read_mod_metadata_uncached(path);
    remember_mod_metadata(path, len, modified, &resolved);
    resolved
}

/// 一个模组的展示名与版本
type ModMetadata = (Option<String>, Option<String>);

/// 缓存条目：文件指纹（长度 + 修改时间）与当时读到的元数据
struct CachedModMetadata {
    len: u64,
    modified: Option<std::time::SystemTime>,
    name: Option<String>,
    version: Option<String>,
}

/// 模组元数据缓存：路径 -> 指纹与元数据
///
/// 只省重复的 zip 解析，命中与否都不影响结果；指纹对不上就重读一遍
static MOD_METADATA_CACHE: std::sync::LazyLock<
    std::sync::Mutex<HashMap<PathBuf, CachedModMetadata>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(HashMap::new()));

/// 条目上限：一个实例的模组数远小于这个值，超了说明换过很多次实例，整体丢弃即可
const MOD_METADATA_CACHE_MAX: usize = 4096;

pub(super) fn cached_mod_metadata(
    path: &Path,
    len: u64,
    modified: Option<std::time::SystemTime>,
) -> Option<ModMetadata> {
    let cache = MOD_METADATA_CACHE.lock().ok()?;
    let entry = cache.get(path)?;
    (entry.len == len && entry.modified == modified)
        .then(|| (entry.name.clone(), entry.version.clone()))
}

pub(super) fn remember_mod_metadata(
    path: &Path,
    len: u64,
    modified: Option<std::time::SystemTime>,
    value: &ModMetadata,
) {
    let Ok(mut cache) = MOD_METADATA_CACHE.lock() else {
        return;
    };
    if cache.len() >= MOD_METADATA_CACHE_MAX && !cache.contains_key(path) {
        cache.clear();
    }
    cache.insert(
        path.to_path_buf(),
        CachedModMetadata {
            len,
            modified,
            name: value.0.clone(),
            version: value.1.clone(),
        },
    );
}

pub(super) fn read_mod_metadata_uncached(path: &Path) -> ModMetadata {
    let file = match fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return (None, None),
    };
    let mut archive = match zip::ZipArchive::new(file) {
        Ok(a) => a,
        Err(_) => return (None, None),
    };

    let mut best: Option<(u8, ModMetadata)> = None;
    for i in 0..archive.len() {
        let mut entry = match archive.by_index(i) {
            Ok(e) => e,
            Err(_) => continue,
        };
        let name = match entry.name() {
            Ok(n) => n.to_ascii_lowercase(),
            Err(_) => continue,
        };
        let Some(priority) = mod_metadata_priority(&name) else {
            continue;
        };

        let mut text = String::new();
        if std::io::Read::read_to_string(&mut entry, &mut text).is_err() {
            continue;
        }

        let parsed = parse_mod_metadata_file(&name, &text);
        if parsed.0.is_some() && best.as_ref().is_none_or(|(p, _)| priority < *p) {
            best = Some((priority, parsed));
        }
    }

    best.map(|(_, meta)| meta).unwrap_or((None, None))
}

/// NeoForge 1.21+ 只读 neoforge.mods.toml；同包残留的 META-INF/mods.toml
/// 可能是没跑资源处理的 Forge 模板
pub(super) fn mod_metadata_priority(name: &str) -> Option<u8> {
    if name.ends_with("neoforge.mods.toml") {
        Some(0)
    } else if name == "fabric.mod.json" || name == "quilt.mod.json" {
        Some(1)
    } else if name.ends_with("mods.toml") {
        Some(2)
    } else {
        None
    }
}

pub(super) fn parse_mod_metadata_file(name: &str, text: &str) -> ModMetadata {
    if name.ends_with(".json") {
        let Ok(json) = serde_json::from_str::<Value>(text) else {
            return (None, None);
        };
        // quilt.mod.json 把字段嵌在 quilt_metadata.mod_metadata 下
        let meta = json
            .get("quilt_metadata")
            .and_then(|m| m.get("mod_metadata"))
            .unwrap_or(&json);
        let display_name = meta
            .get("name")
            .and_then(Value::as_str)
            .filter(|value| !is_unresolved_placeholder(value))
            .map(str::to_string);
        let version = meta
            .get("version")
            .and_then(Value::as_str)
            .map(str::to_string);
        (display_name, version)
    } else {
        (parse_mods_toml_name(text), None)
    }
}

/// Gradle 没跑 processResources 时，值会保留成 `${mod_name}` 这类模板
pub(super) fn is_unresolved_placeholder(value: &str) -> bool {
    value.contains("${")
}

/// 取 mods.toml 里第一条 displayName，缺省退化到 modId
pub(super) fn parse_mods_toml_name(text: &str) -> Option<String> {
    let mut mod_id = None;
    // description 这类多行字符串的正文里可能正好有 `modId = "..."` 这样的行，
    // 所以整块正文要跳过，不能逐行当键值对解析
    let mut in_multiline: Option<char> = None;
    for line in text.lines() {
        let line = line.trim();
        if let Some(quote) = in_multiline {
            if line.contains(multiline_delimiter(quote)) {
                in_multiline = None;
            }
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        // mods.toml 里满是 [mods] 这类没有 '=' 的表头行，不能用 ? 退出整个函数
        let Some((key, rest)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim().trim_matches('"').trim();
        if key.is_empty() {
            continue;
        }
        let rest = rest.trim_start();
        // 多行字符串：同一行能闭合就取首行内容，否则跳过整块正文
        if let Some(quote) = multiline_delimiter_at(rest) {
            let delimiter = multiline_delimiter(quote);
            let body = &rest[delimiter.len()..];
            let closed = body.find(delimiter);
            if closed.is_none() {
                in_multiline = Some(quote);
            }
            let body = match closed {
                Some(end) => &body[..end],
                None => body,
            };
            if key == "displayName" {
                let value = body.lines().next().unwrap_or_default().trim();
                if !value.is_empty() && !is_unresolved_placeholder(value) {
                    return Some(value.to_string());
                }
            }
            continue;
        }
        // 值可能是 `"名字" #mandatory` 这种带行内注释的写法，注释不属于值
        let Some(value) = toml_string_value(rest) else {
            continue;
        };
        match key {
            "displayName" => {
                if !is_unresolved_placeholder(&value) {
                    return Some(value);
                }
            }
            "modId" if mod_id.is_none() && !is_unresolved_placeholder(&value) => {
                mod_id = Some(value);
            }
            _ => {}
        }
    }
    mod_id
}

/// 多行字符串的定界符：三个双引号或三个单引号
pub(super) fn multiline_delimiter(quote: char) -> &'static str {
    if quote == '"' { "\"\"\"" } else { "'''" }
}

/// 值是否以多行字符串的起始定界符开头
pub(super) fn multiline_delimiter_at(value: &str) -> Option<char> {
    if value.starts_with("\"\"\"") {
        Some('"')
    } else if value.starts_with("'''") {
        Some('\'')
    } else {
        None
    }
}

/// 解析 TOML 值里的字符串：支持双引号/单引号、转义，
/// 引号外的 `#` 之后是注释。空串或解析不出内容时返回 None。
/// 多行字符串由调用方单独处理。
pub(super) fn toml_string_value(rest: &str) -> Option<String> {
    let mut chars = rest.chars();
    match chars.next() {
        Some(quote @ ('"' | '\'')) => toml_quoted_string(chars.as_str(), quote),
        // 裸值（true、1.0 之类）同样按行内注释切
        _ => {
            let raw = strip_toml_comment(rest).trim();
            if raw.is_empty() {
                None
            } else {
                Some(raw.to_string())
            }
        }
    }
}

/// 读到与开引号配对的闭引号；引号内的 `#` 是正文，不是注释
pub(super) fn toml_quoted_string(rest: &str, quote: char) -> Option<String> {
    let mut out = String::new();
    let mut chars = rest.chars();
    while let Some(c) = chars.next() {
        if quote == '"' && c == '\\' {
            let escaped = chars.next()?;
            match escaped {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'r' => out.push('\r'),
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                'u' | 'U' => {
                    let len = if escaped == 'u' { 4 } else { 8 };
                    let hex: String = chars.by_ref().take(len).collect();
                    let code = u32::from_str_radix(&hex, 16).ok()?;
                    out.push(char::from_u32(code)?);
                }
                // 其余转义（\b、\f 等）对展示名没有意义，原样保留
                other => {
                    out.push('\\');
                    out.push(other);
                }
            }
            continue;
        }
        if c == quote {
            return if out.is_empty() { None } else { Some(out) };
        }
        out.push(c);
    }
    // 没有闭合引号：多半是残缺行，宁可回退到 modId 也不吐出垃圾
    None
}

/// 切掉裸值后面的行内注释，引号内的 `#` 不算
pub(super) fn strip_toml_comment(value: &str) -> &str {
    let mut in_single = false;
    let mut in_double = false;
    for (index, c) in value.char_indices() {
        match c {
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            '#' if !in_single && !in_double => return &value[..index],
            _ => {}
        }
    }
    value
}

/// 列出实例的模组，含被禁用的 *.jar.disabled
pub fn get_instance_mods(instance_name: String) -> Result<Vec<ModInfo>, LauncherError> {
    validate_instance_name_or_error(&instance_name)?;

    let (game_dir, _) = get_dirs()?;
    let mods_dir = launcher::isolation::content_root_for(&game_dir, &instance_name).join("mods");
    if !mods_dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut mods = Vec::new();
    for entry in fs::read_dir(&mods_dir)? {
        let path = entry?.path();
        if !path.is_file() {
            continue;
        }
        let file_name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        let enabled = file_name.ends_with(".jar");
        if !enabled && !file_name.ends_with(".jar.disabled") {
            continue;
        }

        let stem = file_name
            .strip_suffix(".disabled")
            .unwrap_or(&file_name)
            .strip_suffix(".jar")
            .unwrap_or(&file_name);
        let (meta_name, version) = read_mod_metadata(&path);
        let display_name = meta_name.unwrap_or_else(|| stem.to_string());
        mods.push(ModInfo {
            file_name,
            display_name,
            version,
            enabled,
            size: path.metadata().map(|m| m.len()).unwrap_or(0),
        });
    }

    // 比较器里每次都 to_lowercase 会为大列表分配上万个临时 String；
    // sort_by_cached_key 只算一次小写键
    mods.sort_by_cached_key(|m| m.display_name.to_lowercase());
    Ok(mods)
}

/// 启用/禁用模组：改名 *.jar <-> *.jar.disabled，这是各加载器的通用约定
pub async fn set_mod_enabled(
    instance_name: String,
    file_name: String,
    enabled: bool,
) -> Result<(), LauncherError> {
    validate_instance_name_or_error(&instance_name)?;
    let (game_dir, _) = get_dirs()?;
    let _instance_lock = acquire_instance_lock(&game_dir).await?;
    let (mods_dir, path) = resolve_mod_path(&instance_name, &file_name)?;

    let target_name = if enabled {
        match file_name.strip_suffix(".disabled") {
            Some(stripped) => stripped.to_string(),
            None => return Ok(()),
        }
    } else {
        match file_name.strip_suffix(".jar") {
            Some(_) => format!("{}.disabled", file_name),
            None => return Ok(()),
        }
    };

    let target = mods_dir.join(&target_name);
    if target.exists() {
        return Err(LauncherError::Custom(format!(
            "目标文件已存在: {}",
            target_name
        )));
    }
    fs::rename(&path, &target)?;
    info!("模组 {} -> {}", file_name, target_name);
    Ok(())
}

pub async fn delete_instance_mod(
    instance_name: String,
    file_name: String,
) -> Result<(), LauncherError> {
    validate_instance_name_or_error(&instance_name)?;
    let (game_dir, _) = get_dirs()?;
    let _instance_lock = acquire_instance_lock(&game_dir).await?;
    let (_, path) = resolve_mod_path(&instance_name, &file_name)?;
    fs::remove_file(&path)?;
    info!("已删除模组: {}", path.display());
    Ok(())
}
