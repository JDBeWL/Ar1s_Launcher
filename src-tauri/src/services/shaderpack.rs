//! 光影包管理
//!
//! 扫描实例的 `shaderpacks` 目录，并把「当前用哪个光影」写回游戏自己的配置：
//! Iris / Oculus 的 `config/iris.properties`（或 `oculus.properties`）与 OptiFine 的 `optionsshaders.txt`。
//! 光影包没有模组那样的 `.disabled` 约定，选中状态只由这两份配置决定，启动器不能另存一份。

use crate::errors::LauncherError;
use crate::services::{config, launcher};
use crate::utils::file_utils;
use log::{info, warn};
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};

/// 文件夹形式的光影包统计体积时的递归深度上限
const MAX_DIR_DEPTH: u8 = 4;
/// 目录形式的包往下找 `shaders` 时最多看多少个子目录，避免扫进巨大的解压目录
const MAX_DIR_PROBE: usize = 64;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShaderPackInfo {
    /// 磁盘上的名字：`.zip` 文件名或文件夹名，也是配置里 `shaderPack` 要写的值
    pub file_name: String,
    /// 展示名：压缩包去掉 `.zip`，文件夹保持原名
    pub display_name: String,
    pub size: u64,
    pub is_directory: bool,
    /// 包里有没有 `shaders` 目录；没有的话 Iris / OptiFine 都会拒绝加载
    pub valid: bool,
    pub selected: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShaderPackList {
    /// iris / oculus / optifine / unknown
    pub backend: &'static str,
    /// 只有 Iris / Oculus 有独立的总开关，OptiFine 靠 `shaderPack=OFF` 表示关闭
    pub supports_toggle: bool,
    pub enabled: bool,
    /// 配置里选中的包名；文件已经不在磁盘上时也会原样返回
    pub selected: Option<String>,
    pub packs: Vec<ShaderPackInfo>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportSummary {
    pub imported: Vec<String>,
    pub skipped: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Backend {
    Iris,
    Oculus,
    OptiFine,
    Unknown,
}

impl Backend {
    fn slug(self) -> &'static str {
        match self {
            Self::Iris => "iris",
            Self::Oculus => "oculus",
            Self::OptiFine => "optifine",
            Self::Unknown => "unknown",
        }
    }

    fn supports_toggle(self) -> bool {
        matches!(self, Self::Iris | Self::Oculus)
    }

    fn config_path(self, root: &Path) -> Option<PathBuf> {
        match self {
            Self::Iris => Some(root.join("config").join("iris.properties")),
            Self::Oculus => Some(root.join("config").join("oculus.properties")),
            Self::OptiFine => Some(root.join("optionsshaders.txt")),
            Self::Unknown => None,
        }
    }
}

struct ShaderContext {
    game_dir: PathBuf,
    packs_dir: PathBuf,
    backend: Backend,
    config_path: Option<PathBuf>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct ShaderState {
    enabled: bool,
    selected: Option<String>,
}

fn resolve_context(instance_name: &str) -> Result<ShaderContext, LauncherError> {
    file_utils::validate_instance_name_or_error(instance_name)?;
    let config = config::load_config()?;
    let game_dir = PathBuf::from(&config.game_dir);
    let root = launcher::isolation::content_root_for(&game_dir, instance_name);
    let backend = detect_backend(&root);
    Ok(ShaderContext {
        config_path: backend.config_path(&root),
        packs_dir: root.join("shaderpacks"),
        backend,
        game_dir,
    })
}

/// 先看 mods 里装的是哪套光影加载器，没有再退回到「游戏自己写过的配置文件」
fn detect_backend(root: &Path) -> Backend {
    let mut iris = false;
    let mut oculus = false;
    let mut optifine = false;

    if let Ok(entries) = fs::read_dir(root.join("mods")) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            // 禁用中的模组不参与判断，否则装了 Iris 又禁用会误判
            if !name.ends_with(".jar") {
                continue;
            }
            if name.contains("oculus") {
                oculus = true;
            } else if name.contains("iris") {
                iris = true;
            } else if name.contains("optifine") {
                optifine = true;
            }
        }
    }

    if oculus {
        return Backend::Oculus;
    }
    if iris {
        return Backend::Iris;
    }
    if optifine {
        return Backend::OptiFine;
    }

    if root.join("config/oculus.properties").is_file() {
        return Backend::Oculus;
    }
    if root.join("config/iris.properties").is_file() {
        return Backend::Iris;
    }
    if root.join("optionsshaders.txt").is_file() {
        return Backend::OptiFine;
    }
    Backend::Unknown
}

pub fn list_shader_packs(instance_name: &str) -> Result<ShaderPackList, LauncherError> {
    let context = resolve_context(instance_name)?;
    let state = read_state(&context);
    let mut packs = Vec::new();

    if context.packs_dir.is_dir() {
        for entry in fs::read_dir(&context.packs_dir)? {
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
            let lower = file_name.to_ascii_lowercase();
            if is_directory {
                // Iris 调试时会把展开后的 shader 存进 shaderpacks/debug，它不是光影包
                if lower == "debug" {
                    continue;
                }
            } else if !lower.ends_with(".zip") {
                continue;
            }

            let valid = if is_directory {
                dir_has_shaders(&path)
            } else {
                zip_has_shaders(&path)
            };
            let selected = state
                .selected
                .as_deref()
                .is_some_and(|name| name.eq_ignore_ascii_case(&file_name));

            packs.push(ShaderPackInfo {
                display_name: display_name(&file_name, is_directory),
                size: entry_size(&path, is_directory, 0),
                selected,
                is_directory,
                valid,
                file_name,
            });
        }
    }

    // 键只算一次：比较器里现算 to_lowercase 每次比较都要分配两个 String
    packs.sort_by_cached_key(|pack| {
        (
            std::cmp::Reverse(pack.selected),
            std::cmp::Reverse(pack.valid),
            pack.display_name.to_lowercase(),
        )
    });

    Ok(ShaderPackList {
        backend: context.backend.slug(),
        supports_toggle: context.backend.supports_toggle(),
        enabled: state.enabled,
        selected: state.selected,
        packs,
    })
}

/// `file_name` 为 None 表示关闭光影：Iris 只把 `enableShaders` 置 false，
/// 保留已选包；OptiFine 则写 `shaderPack=OFF`
pub async fn set_shader_pack(
    instance_name: String,
    file_name: Option<String>,
) -> Result<ShaderPackList, LauncherError> {
    let context = resolve_context(&instance_name)?;
    let _instance_lock =
        crate::services::instance::acquire_instance_lock(&context.game_dir).await?;

    let selected = file_name
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty());

    if let Some(name) = &selected {
        let path = resolve_pack_path(&context.packs_dir, name)?;
        if !path.exists() {
            return Err(LauncherError::Custom(format!("光影包不存在: {}", name)));
        }
    }

    let config_path = require_config_path(&context)?;
    let mut properties = PropertiesFile::load(&config_path)?;

    match context.backend {
        Backend::Iris | Backend::Oculus => match &selected {
            Some(name) => {
                // Iris 自己的交互就是「点一个包 = 启用光影」
                properties.set("shaderPack", name);
                properties.set("enableShaders", "true");
            }
            None => properties.set("enableShaders", "false"),
        },
        Backend::OptiFine => properties.set("shaderPack", selected.as_deref().unwrap_or("OFF")),
        Backend::Unknown => return Err(unknown_backend_error()),
    }

    properties.save()?;
    info!(
        "实例 {} 的光影选择: {}",
        instance_name,
        selected.as_deref().unwrap_or("关闭")
    );

    list_shader_packs(&instance_name)
}

/// Iris / Oculus 的总开关；OptiFine 没有这个概念，只能通过选包/写 OFF 表达
pub async fn set_shaders_enabled(
    instance_name: String,
    enabled: bool,
) -> Result<ShaderPackList, LauncherError> {
    let context = resolve_context(&instance_name)?;
    let _instance_lock =
        crate::services::instance::acquire_instance_lock(&context.game_dir).await?;

    match context.backend {
        Backend::Iris | Backend::Oculus => {
            let config_path = require_config_path(&context)?;
            let mut properties = PropertiesFile::load(&config_path)?;
            properties.set("enableShaders", if enabled { "true" } else { "false" });
            properties.save()?;
        }
        Backend::OptiFine => {
            if enabled {
                return Err(LauncherError::Custom(
                    "OptiFine 没有独立的光影开关，请在列表里选择一个光影包".to_string(),
                ));
            }
            let config_path = require_config_path(&context)?;
            let mut properties = PropertiesFile::load(&config_path)?;
            properties.set("shaderPack", "OFF");
            properties.save()?;
        }
        Backend::Unknown => return Err(unknown_backend_error()),
    }

    info!(
        "实例 {} 的光影总开关: {}",
        instance_name,
        if enabled { "开" } else { "关" }
    );
    list_shader_packs(&instance_name)
}

pub async fn delete_shader_pack(
    instance_name: String,
    file_name: String,
) -> Result<(), LauncherError> {
    let context = resolve_context(&instance_name)?;
    let _instance_lock =
        crate::services::instance::acquire_instance_lock(&context.game_dir).await?;

    let path = resolve_pack_path(&context.packs_dir, &file_name)?;
    if !path.exists() {
        return Err(LauncherError::Custom(format!(
            "光影包不存在: {}",
            file_name
        )));
    }

    // 删的正是配置里选中的包时，先把选择清掉，否则游戏会对着一个空名字报错
    if read_state(&context)
        .selected
        .as_deref()
        .is_some_and(|name| name.eq_ignore_ascii_case(&file_name))
        && let Some(config_path) = context.config_path.as_deref()
    {
        let mut properties = PropertiesFile::load(config_path)?;
        match context.backend {
            Backend::Iris | Backend::Oculus => properties.set("shaderPack", ""),
            Backend::OptiFine => properties.set("shaderPack", "OFF"),
            Backend::Unknown => {}
        }
        properties.save()?;
    }

    if path.is_dir() {
        fs::remove_dir_all(&path)?;
    } else {
        fs::remove_file(&path)?;
    }
    info!("已删除光影包: {}", path.display());
    Ok(())
}

pub async fn import_shader_packs(
    instance_name: String,
    source_paths: Vec<String>,
) -> Result<ImportSummary, LauncherError> {
    let context = resolve_context(&instance_name)?;
    if source_paths.is_empty() {
        return Ok(ImportSummary {
            imported: Vec::new(),
            skipped: Vec::new(),
        });
    }
    let _instance_lock =
        crate::services::instance::acquire_instance_lock(&context.game_dir).await?;
    fs::create_dir_all(&context.packs_dir)?;

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
            warn!("跳过文件名不合法的光影包: {}", source);
            summary.skipped.push(source.clone());
            continue;
        }

        let target = context.packs_dir.join(&name);
        if target.exists() {
            summary.skipped.push(name);
            continue;
        }

        let result = if source_path.is_dir() {
            if !dir_has_shaders(&source_path) {
                Err(LauncherError::Custom("不是光影包目录".to_string()))
            } else {
                // 递归复制整个光影包目录，文件多时会长时间占住线程
                let source_owned = source_path.clone();
                let target_owned = target.clone();
                tokio::task::spawn_blocking(move || {
                    file_utils::copy_dir_all(&source_owned, &target_owned)
                        .map_err(LauncherError::Io)
                })
                .await
                .map_err(|e| LauncherError::Custom(format!("导入光影包失败: {}", e)))?
            }
        } else if source_path.is_file() {
            if !name.to_ascii_lowercase().ends_with(".zip") || !zip_has_shaders(&source_path) {
                Err(LauncherError::Custom("不是光影包压缩包".to_string()))
            } else {
                fs::copy(&source_path, &target)
                    .map(|_| ())
                    .map_err(LauncherError::Io)
            }
        } else {
            Err(LauncherError::Custom("来源路径不存在".to_string()))
        };

        match result {
            Ok(()) => {
                info!("已导入光影包: {}", name);
                summary.imported.push(name);
            }
            Err(error) => {
                warn!("导入光影包 {} 失败: {}", name, error);
                summary.skipped.push(name);
            }
        }
    }

    Ok(summary)
}

pub async fn open_shaderpacks_folder(instance_name: String) -> Result<(), LauncherError> {
    let context = resolve_context(&instance_name)?;
    fs::create_dir_all(&context.packs_dir)?;
    opener::open(&context.packs_dir)
        .map_err(|e| LauncherError::Custom(format!("无法打开光影包文件夹: {}", e)))?;
    Ok(())
}

/// 只允许传单个名字；带路径分隔符或 `..` 就能跳出 shaderpacks 目录
fn resolve_pack_path(packs_dir: &Path, file_name: &str) -> Result<PathBuf, LauncherError> {
    if file_name.is_empty()
        || file_name.contains('/')
        || file_name.contains('\\')
        || file_name.contains("..")
        || file_name.contains(':')
    {
        return Err(LauncherError::Custom("光影包文件名无效".to_string()));
    }
    Ok(packs_dir.join(file_name))
}

fn require_config_path(context: &ShaderContext) -> Result<PathBuf, LauncherError> {
    context
        .config_path
        .clone()
        .ok_or_else(unknown_backend_error)
}

fn unknown_backend_error() -> LauncherError {
    LauncherError::Custom(
        "这个实例没有安装 Iris / Oculus 或 OptiFine，光影包不会被加载；请先在「模组管理」里安装一个光影加载器"
            .to_string(),
    )
}

fn read_state(context: &ShaderContext) -> ShaderState {
    let Some(config_path) = context.config_path.as_deref() else {
        return ShaderState::default();
    };
    // 游戏还没写过配置：当作「没启用」，界面上也就不用凭空点亮一个总开关
    if !config_path.is_file() {
        return ShaderState::default();
    }
    let Ok(properties) = PropertiesFile::load(config_path) else {
        return ShaderState::default();
    };

    match context.backend {
        Backend::Iris | Backend::Oculus => {
            let selected = properties
                .get("shaderPack")
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty() && value != "(internal)");
            // Iris 的默认值是 true：只有显式写了 false 才算关
            let enabled = properties.get("enableShaders").as_deref() != Some("false");
            ShaderState { enabled, selected }
        }
        Backend::OptiFine => {
            let selected = properties
                .get("shaderPack")
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty() && value != "OFF" && value != "(internal)");
            let enabled = selected.is_some();
            ShaderState { enabled, selected }
        }
        Backend::Unknown => ShaderState::default(),
    }
}

fn display_name(file_name: &str, is_directory: bool) -> String {
    if is_directory {
        return file_name.to_string();
    }
    file_name
        .strip_suffix(".zip")
        .or_else(|| file_name.strip_suffix(".ZIP"))
        .unwrap_or(file_name)
        .to_string()
}

/// Iris 认的是「任意一层目录名为 shaders」，所以压缩包里出现 shaders 目录就算有效
fn zip_has_shaders(path: &Path) -> bool {
    let Ok(file) = fs::File::open(path) else {
        return false;
    };
    let Ok(archive) = zip::ZipArchive::new(file) else {
        return false;
    };
    has_shaders_component(archive.file_names().flatten())
}

fn has_shaders_component<'a>(names: impl Iterator<Item = std::borrow::Cow<'a, str>>) -> bool {
    names.into_iter().any(|name| {
        name.split(['/', '\\'])
            .any(|part| part.eq_ignore_ascii_case("shaders"))
    })
}

/// 文件夹包：自己就是包根（`<包>/shaders`），或解压时多套了一层（`<包>/<包>/shaders`）
fn dir_has_shaders(path: &Path) -> bool {
    if path.join("shaders").is_dir() {
        return true;
    }
    let Ok(entries) = fs::read_dir(path) else {
        return false;
    };
    entries
        .flatten()
        .take(MAX_DIR_PROBE)
        .any(|entry| entry.path().is_dir() && entry.path().join("shaders").is_dir())
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

/// Java `.properties` 的最小实现。
///
/// 两份配置的编码约定并不一致：OptiFine 用默认字符集的 `FileWriter` 写，Iris 按
/// ISO-8859-1 读。为了两边都不写坏，未改动的行按原始字节保留，改写时非 ASCII
/// 字符一律写成 `\uXXXX` 转义 —— `Properties.load` 对两种入口都会解码转义。
struct PropertiesFile {
    path: PathBuf,
    /// 每行原始字节，末尾不含换行符
    lines: Vec<Vec<u8>>,
    crlf: bool,
}

impl PropertiesFile {
    fn load(path: &Path) -> Result<Self, LauncherError> {
        if !path.exists() {
            return Ok(Self {
                path: path.to_path_buf(),
                lines: Vec::new(),
                crlf: false,
            });
        }
        let bytes = fs::read(path)?;
        let crlf = bytes.windows(2).any(|pair| pair == b"\r\n");
        let mut lines: Vec<Vec<u8>> = bytes
            .split(|byte| *byte == b'\n')
            .map(|line| {
                let mut line = line.to_vec();
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                line
            })
            .collect();
        // 文件末尾的换行会 split 出一个空行；丢掉它，否则每次保存都多一个空行
        while lines.last().is_some_and(|line| line.is_empty()) {
            lines.pop();
        }
        Ok(Self {
            path: path.to_path_buf(),
            lines,
            crlf,
        })
    }

    fn get(&self, key: &str) -> Option<String> {
        self.lines.iter().find_map(|line| {
            let text = decode_line(line);
            let (found, value) = split_property(&text)?;
            (found == key).then_some(value)
        })
    }

    fn set(&mut self, key: &str, value: &str) {
        let new_line = format!("{}={}", escape_property(key), escape_property(value)).into_bytes();
        let prefix = format!("{}=", escape_property(key));
        let colon_prefix = format!("{}:", escape_property(key));
        for line in &mut self.lines {
            let text = decode_line(line);
            if text.starts_with(&prefix) || text.starts_with(&colon_prefix) {
                *line = new_line;
                return;
            }
            if let Some((found, _)) = split_property(&text)
                && found == key
            {
                *line = new_line;
                return;
            }
        }
        self.lines.push(new_line);
    }

    fn save(&self) -> Result<(), LauncherError> {
        let separator: &[u8] = if self.crlf { b"\r\n" } else { b"\n" };
        let mut bytes = Vec::new();
        for (index, line) in self.lines.iter().enumerate() {
            if index > 0 {
                bytes.extend_from_slice(separator);
            }
            bytes.extend_from_slice(line);
        }
        // 游戏自己写的时候文件末尾有换行，跟着保留
        if !self.lines.is_empty() {
            bytes.extend_from_slice(separator);
        }

        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        // 先写临时文件再走 .bak 回滚替换：中途失败不会留下半截配置
        let file_name = self
            .path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "properties".to_string());
        let temp = self.path.with_file_name(format!("{}.ar1s-tmp", file_name));
        fs::write(&temp, bytes)?;
        crate::utils::file_utils::replace_file_atomically(&temp, &self.path)?;
        Ok(())
    }
}

/// 把一行原始字节按 `\uXXXX` 与常见转义还原成文本；非转义部分按 UTF-8 容错解码
fn decode_line(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('u') => {
                let mut hex = String::new();
                while hex.len() < 4 {
                    match chars.peek() {
                        Some(next) if next.is_ascii_hexdigit() => {
                            hex.push(*next);
                            chars.next();
                        }
                        _ => break,
                    }
                }
                match u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                    Some(decoded) => out.push(decoded),
                    // 不完整的 \u 转义原样保留，别把玩家的配置吃掉
                    None => {
                        out.push('\\');
                        out.push('u');
                        out.push_str(&hex);
                    }
                }
            }
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            // `\\`、`\=`、`\:`、`\ ` 等都还原成字符本身
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

/// 按 Java 的规则拆出键与值：第一个未转义的 `=`、`:` 或空白是分隔符
fn split_property(text: &str) -> Option<(String, String)> {
    if text.trim().is_empty() || text.trim_start().starts_with(['#', '!']) {
        return None;
    }
    let mut escaped = false;
    for (index, ch) in text.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == '=' || ch == ':' || ch == ' ' || ch == '\t' {
            let key = decode_line(&text.as_bytes()[..index]);
            let rest = &text[index..];
            let value = rest
                .trim_start_matches([' ', '\t'])
                .strip_prefix(['=', ':'])
                .unwrap_or(rest.trim_start_matches([' ', '\t']))
                .trim_start_matches([' ', '\t']);
            return Some((key, decode_line(value.as_bytes())));
        }
    }
    Some((decode_line(text.as_bytes()), String::new()))
}

/// 非 ASCII 与控制字符写成 `\uXXXX`，保证 OptiFine 与 Iris 用哪种字符集都能读回来
fn escape_property(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for (index, ch) in value.chars().enumerate() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '=' => out.push_str("\\="),
            ':' => out.push_str("\\:"),
            ' ' if index == 0 => out.push_str("\\ "),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if (' '..='~').contains(&ch) => out.push(ch),
            ch => {
                let mut units = [0u16; 2];
                for unit in ch.encode_utf16(&mut units) {
                    out.push_str(&format!("\\u{:04x}", unit));
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{
        Backend, PropertiesFile, ShaderContext, ShaderState, decode_line, dir_has_shaders,
        display_name, escape_property, has_shaders_component, read_state, split_property,
    };
    use std::borrow::Cow;
    use std::fs;
    use std::io::Write;
    use std::path::PathBuf;
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    fn write_zip(path: &std::path::Path, entries: &[&str]) {
        let file = fs::File::create(path).unwrap();
        let mut zip = ZipWriter::new(file);
        let options = SimpleFileOptions::default();
        for entry in entries {
            zip.start_file(*entry, options).unwrap();
            zip.write_all(b"// shader").unwrap();
        }
        zip.finish().unwrap();
    }

    fn context(root: &std::path::Path, backend: Backend) -> ShaderContext {
        ShaderContext {
            game_dir: root.to_path_buf(),
            packs_dir: root.join("shaderpacks"),
            backend,
            config_path: backend.config_path(root),
        }
    }

    #[test]
    fn test_properties_round_trip_keeps_other_keys() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("optionsshaders.txt");
        fs::write(&path, "shaderPack=OFF\r\nshadowResMul=1.0\r\n").unwrap();

        let mut properties = PropertiesFile::load(&path).unwrap();
        assert_eq!(properties.get("shaderPack").as_deref(), Some("OFF"));
        properties.set("shaderPack", "ComplementaryReimagined_r5.9.3.zip");
        properties.save().unwrap();

        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("shaderPack=ComplementaryReimagined_r5.9.3.zip"));
        assert!(text.contains("shadowResMul=1.0\r\n"));
        // 原来就是 CRLF，写回不能混进裸 \n
        assert!(!text.replace("\r\n", "").contains('\n'));
    }

    #[test]
    fn test_properties_appends_missing_key() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("iris.properties");
        fs::write(&path, "shaderPack=\nenableShaders=true\n").unwrap();

        let mut properties = PropertiesFile::load(&path).unwrap();
        properties.set("shaderPack", "BSL_v8.2.zip");
        properties.save().unwrap();

        let properties = PropertiesFile::load(&path).unwrap();
        assert_eq!(
            properties.get("shaderPack").as_deref(),
            Some("BSL_v8.2.zip")
        );
        assert_eq!(properties.get("enableShaders").as_deref(), Some("true"));
    }

    #[test]
    fn test_properties_rewrite_does_not_grow_blank_lines() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("optionsshaders.txt");
        fs::write(&path, "shaderPack=OFF\nshadowResMul=1.0\n").unwrap();

        for name in ["a.zip", "b.zip"] {
            let mut properties = PropertiesFile::load(&path).unwrap();
            properties.set("shaderPack", name);
            properties.save().unwrap();
        }

        // 多轮读写后行数保持不变：末尾换行不会被当成一条空配置越积越多
        let text = fs::read_to_string(&path).unwrap();
        assert_eq!(text, "shaderPack=b.zip\nshadowResMul=1.0\n");
    }

    #[test]
    fn test_escape_property_is_ascii_only_and_decodes_back() {
        let escaped = escape_property("我的 光影:包=1\\2");
        assert!(escaped.is_ascii());
        assert_eq!(decode_line(escaped.as_bytes()), "我的 光影:包=1\\2");
        assert!(escaped.contains("\\u"));
    }

    #[test]
    fn test_split_property_handles_colon_and_escaped_separator() {
        assert_eq!(
            split_property("shaderPack:BSL.zip"),
            Some(("shaderPack".to_string(), "BSL.zip".to_string()))
        );
        assert_eq!(
            split_property("shaderPack=foo\\=bar.zip"),
            Some(("shaderPack".to_string(), "foo=bar.zip".to_string()))
        );
        assert_eq!(split_property("# comment"), None);
        assert_eq!(split_property(""), None);
    }

    #[test]
    fn test_read_state_defaults_to_enabled_for_iris() {
        let dir = tempfile::TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("config")).unwrap();
        fs::write(
            dir.path().join("config/iris.properties"),
            "shaderPack=BSL_v8.2.zip\nenableShaders=true\n",
        )
        .unwrap();

        let state = read_state(&context(dir.path(), Backend::Iris));
        assert_eq!(
            state,
            ShaderState {
                enabled: true,
                selected: Some("BSL_v8.2.zip".to_string()),
            }
        );
    }

    #[test]
    fn test_read_state_treats_optifine_off_as_disabled() {
        let dir = tempfile::TempDir::new().unwrap();
        fs::write(dir.path().join("optionsshaders.txt"), "shaderPack=OFF\n").unwrap();

        let state = read_state(&context(dir.path(), Backend::OptiFine));
        assert_eq!(state, ShaderState::default());

        fs::write(
            dir.path().join("optionsshaders.txt"),
            "shaderPack=Complementary.zip\n",
        )
        .unwrap();
        let state = read_state(&context(dir.path(), Backend::OptiFine));
        assert!(state.enabled);
        assert_eq!(state.selected.as_deref(), Some("Complementary.zip"));
    }

    #[test]
    fn test_read_state_ignores_missing_config() {
        let dir = tempfile::TempDir::new().unwrap();
        assert_eq!(
            read_state(&context(dir.path(), Backend::Iris)),
            ShaderState::default()
        );
        assert_eq!(
            read_state(&context(dir.path(), Backend::Unknown)),
            ShaderState::default()
        );
    }

    #[test]
    fn test_has_shaders_component_accepts_nested_layout() {
        let names = [
            Cow::Borrowed("readme.txt"),
            Cow::Borrowed("Pack/shaders/gbuffers_basic.fsh"),
        ];
        assert!(has_shaders_component(names.into_iter()));
        let names = [Cow::Borrowed("Pack/textures/a.png")];
        assert!(!has_shaders_component(names.into_iter()));
    }

    #[test]
    fn test_zip_has_shaders_reads_real_archive() {
        let dir = tempfile::TempDir::new().unwrap();
        let good = dir.path().join("good.zip");
        write_zip(&good, &["shaders/gbuffers_basic.fsh"]);
        assert!(super::zip_has_shaders(&good));

        let bad = dir.path().join("bad.zip");
        write_zip(&bad, &["readme.txt"]);
        assert!(!super::zip_has_shaders(&bad));

        // 坏文件不应该 panic，只当作无效
        let broken = dir.path().join("broken.zip");
        fs::write(&broken, b"not a zip").unwrap();
        assert!(!super::zip_has_shaders(&broken));
    }

    #[test]
    fn test_dir_has_shaders_supports_nested_and_rejects_plain_folder() {
        let dir = tempfile::TempDir::new().unwrap();
        let flat = dir.path().join("flat");
        fs::create_dir_all(flat.join("shaders")).unwrap();
        assert!(dir_has_shaders(&flat));

        let nested = dir.path().join("nested");
        fs::create_dir_all(nested.join("nested/shaders")).unwrap();
        assert!(dir_has_shaders(&nested));

        let plain = dir.path().join("plain");
        fs::create_dir_all(plain.join("textures")).unwrap();
        assert!(!dir_has_shaders(&plain));
        assert!(!dir_has_shaders(&PathBuf::from("does-not-exist")));
    }

    #[test]
    fn test_display_name_strips_zip_suffix_only_for_files() {
        assert_eq!(display_name("BSL_v8.2.zip", false), "BSL_v8.2");
        assert_eq!(display_name("BSL_v8.2.ZIP", false), "BSL_v8.2");
        assert_eq!(display_name("MyPack", true), "MyPack");
    }
}
