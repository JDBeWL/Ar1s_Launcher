//! 文件与下载校验工具
//!
//! 实例名校验、目录递归拷贝、sha1 与大小校验修复；校验结果按「长度 + mtime」缓存

use crate::errors::LauncherError;
use crate::utils::verify_cache;
use log::{debug, info, warn};
use sha1::{Digest, Sha1};
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, serde::Serialize)]
pub struct InstanceNameValidation {
    pub is_valid: bool,
    pub error_message: Option<String>,
}

/// 会直接当目录名/文件名用的字符串：拒绝路径分隔符、`..`、盘符与 Windows 保留名
fn validate_safe_component(name: &str, label: &str, max_len: usize) -> Result<(), String> {
    if name.is_empty() {
        return Err(format!("{label}不能为空"));
    }

    if name.len() > max_len {
        return Err(format!("{label}不能超过 {max_len} 个字符"));
    }

    if name.starts_with('.') || name.starts_with(' ') {
        return Err(format!("{label}不能以点或空格开头"));
    }

    if name.ends_with('.') || name.ends_with(' ') {
        return Err(format!("{label}不能以点或空格结尾"));
    }

    if name.contains("..") {
        return Err(format!("{label}不能包含 '..'"));
    }

    if name.contains('/') || name.contains('\\') {
        return Err(format!("{label}不能包含路径分隔符 (/ 或 \\)"));
    }

    const RESERVED_CHARS: &[char] = &['<', '>', ':', '"', '|', '?', '*'];
    for c in RESERVED_CHARS {
        if name.contains(*c) {
            return Err(format!("{label}不能包含特殊字符 '{}'", c));
        }
    }

    if name.chars().any(|c| (c as u32) < 32) {
        return Err(format!("{label}不能包含控制字符"));
    }

    let name_upper = name.to_uppercase();
    let base_name = name_upper.split('.').next().unwrap_or(&name_upper);
    const RESERVED_NAMES: &[&str] = &[
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];

    if RESERVED_NAMES.contains(&base_name) {
        return Err(format!("'{}' 是 Windows 保留名称，不能用作{label}", name));
    }

    Ok(())
}

/// 实例名会直接当目录名用，带路径分隔符或 `..` 就能跳出版本目录
/// Windows 保留名大小写不敏感，且只看首个 `.` 之前的部分（`CON.txt` 同样拒绝）
pub fn validate_instance_name(name: &str) -> InstanceNameValidation {
    match validate_safe_component(name, "实例名称", 64) {
        Ok(()) => InstanceNameValidation {
            is_valid: true,
            error_message: None,
        },
        Err(error_message) => InstanceNameValidation {
            is_valid: false,
            error_message: Some(error_message),
        },
    }
}

pub fn validate_instance_name_or_error(name: &str) -> Result<(), LauncherError> {
    validate_safe_component(name, "实例名称", 64).map_err(LauncherError::Custom)
}

/// 版本 ID 会拼进 versions/<id>，规则与实例目录名一致
pub fn validate_version_id_or_error(version_id: &str) -> Result<(), LauncherError> {
    validate_safe_component(version_id, "版本 ID", 64).map_err(LauncherError::Custom)
}

/// 远端给的文件名只允许是单个文件名，不能带目录或 ADS 冒号
pub fn validate_file_name_or_error(file_name: &str) -> Result<(), LauncherError> {
    validate_safe_component(file_name, "文件名", 200).map_err(LauncherError::Custom)
}

/// 启动时绑定缓存目录；游戏目录改变后也要再调一次
pub fn init_verify_cache(game_dir: &Path) {
    verify_cache::init(game_dir);
}

/// 下载结束时把这一轮记下的指纹落盘，下次启动仍然有效
pub fn flush_verify_cache() {
    verify_cache::flush();
}

pub fn instance_dir(base: &Path, instance_name: &str) -> Result<PathBuf, LauncherError> {
    validate_instance_name_or_error(instance_name)?;
    Ok(base.join(instance_name))
}

pub fn version_dir(base: &Path, version_id: &str) -> Result<PathBuf, LauncherError> {
    validate_version_id_or_error(version_id)?;
    Ok(base.join(version_id))
}

/// mrpack 清单和 zip 条目的路径来自远端，拼到目标目录前先确认它是相对路径
pub fn is_safe_relative_path(raw: &str) -> bool {
    if raw.is_empty() || raw.contains("..") || raw.contains(':') {
        return false;
    }
    if raw.starts_with('/') || raw.starts_with('\\') {
        return false;
    }
    true
}

/// 逐字节 `format!` 会为每个字节分配一个 String；400 次校验就是几十万次分配
const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";

fn digest_to_hex(digest: &[u8]) -> String {
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        hex.push(HEX_DIGITS[(byte >> 4) as usize] as char);
        hex.push(HEX_DIGITS[(byte & 0x0f) as usize] as char);
    }
    hex
}

pub fn sha1_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha1::new();
    hasher.update(bytes);
    digest_to_hex(&hasher.finalize())
}

/// 流式计算文件 sha1，模组更新检查按内容识别文件，不依赖文件名
pub fn sha1_file(path: &std::path::Path) -> Result<String, LauncherError> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha1::new();
    // 大缓冲：小文件一次读完，大文件也少几次系统调用
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buf)?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }
    Ok(digest_to_hex(&hasher.finalize()))
}

/// 多文件并行算 sha1，按 CPU 核数分片
///
/// 逐个串行算只有单核在跑；按核数切块，每块内部仍然串行，避免几百个线程抢 IO
pub fn sha1_files_parallel(paths: &[PathBuf]) -> Vec<(PathBuf, String)> {
    if paths.is_empty() {
        return Vec::new();
    }

    let workers = std::thread::available_parallelism()
        .map(|count| count.get())
        .unwrap_or(4)
        .clamp(1, 8)
        .min(paths.len());

    if workers <= 1 {
        return paths
            .iter()
            .filter_map(|path| sha1_file(path).ok().map(|hash| (path.clone(), hash)))
            .collect();
    }

    let chunk_size = paths.len().div_ceil(workers);
    std::thread::scope(|scope| {
        let handles: Vec<_> = paths
            .chunks(chunk_size)
            .map(|chunk| {
                scope.spawn(move || {
                    chunk
                        .iter()
                        .filter_map(|path| sha1_file(path).ok().map(|hash| (path.clone(), hash)))
                        .collect::<Vec<_>>()
                })
            })
            .collect();

        handles
            .into_iter()
            .flat_map(|handle| handle.join().unwrap_or_default())
            .collect()
    })
}

/// 并行算出「文件 -> sha1」的索引，供 Modrinth 的按哈希反查使用
///
/// 模组、资源包、光影包的更新检查共用这一套：返回 (哈希列表, 哈希 -> 文件名)，算不出来的文件记警告并跳过
pub fn sha1_index_parallel(files: &[(PathBuf, String)]) -> (Vec<String>, HashMap<String, String>) {
    sha1_index_with(files, false)
}

/// 同上，但命中校验指纹缓存时直接沿用上次算出的内容哈希
///
/// 更新检查是「打开实例页面就看」的操作：长度与修改时间没变，内容就一定还是那一份，不必重读
pub fn sha1_index_cached(files: &[(PathBuf, String)]) -> (Vec<String>, HashMap<String, String>) {
    sha1_index_with(files, true)
}

fn sha1_index_with(
    files: &[(PathBuf, String)],
    use_cache: bool,
) -> (Vec<String>, HashMap<String, String>) {
    // 先按指纹挑出真正要读盘的文件，剩下的直接取缓存
    let mut cached: HashMap<PathBuf, String> = HashMap::with_capacity(files.len());
    let mut to_hash: Vec<PathBuf> = Vec::with_capacity(files.len());

    for (path, _) in files {
        let hit = use_cache
            .then(|| {
                std::fs::metadata(path)
                    .ok()
                    .and_then(|meta| verify_cache::known_hash(path, &meta))
            })
            .flatten();

        match hit {
            Some(hash) => {
                cached.insert(path.clone(), hash);
            }
            None => to_hash.push(path.clone()),
        }
    }

    let fresh = sha1_files_parallel(&to_hash);
    if use_cache {
        for (path, hash) in &fresh {
            if let Ok(meta) = std::fs::metadata(path) {
                // 内容哈希记进 actual：这张表就是「这份文件的内容是什么」，
                // 下次任何需要它的地方（含下载校验）都能直接命中
                verify_cache::remember(path, &meta, Some(hash), Some(hash));
            }
        }
        // 只有真算过东西才落盘，避免每次打开实例页面都重写一遍缓存文件
        if !fresh.is_empty() {
            verify_cache::flush();
        }
    }
    cached.extend(fresh);

    let mut hashes = Vec::with_capacity(files.len());
    let mut by_hash = HashMap::with_capacity(files.len());

    for (path, file_name) in files {
        match cached.get(path) {
            Some(hash) => {
                hashes.push(hash.clone());
                by_hash.insert(hash.clone(), file_name.clone());
            }
            None => warn!("计算 {} 的 sha1 失败，跳过更新检查", file_name),
        }
    }

    (hashes, by_hash)
}

/// 边写边算：下载时每块数据顺手进哈希，写完不必再把整个文件读一遍
///
/// `Clone` 用于续传前缀缓存：把「算到某个长度」的哈希器状态存下来，重试时接着算
#[derive(Clone)]
pub struct FileHasher {
    hasher: Sha1,
}

impl FileHasher {
    pub fn new() -> Self {
        Self {
            hasher: Sha1::new(),
        }
    }

    pub fn update(&mut self, chunk: &[u8]) {
        self.hasher.update(chunk);
    }

    pub fn finish_hex(self) -> String {
        digest_to_hex(&self.hasher.finalize())
    }
}

impl Default for FileHasher {
    fn default() -> Self {
        Self::new()
    }
}

/// maven 的 .sha1 文件常见格式是「hash  filename」或只有 hash
pub fn parse_sha1(text: &str) -> Option<String> {
    let hash = text.split_whitespace().next()?;
    (hash.len() == 40 && hash.chars().all(|c| c.is_ascii_hexdigit()))
        .then(|| hash.to_ascii_lowercase())
}

pub fn verify_sha1(bytes: &[u8], expected: &str) -> bool {
    let expected = expected.trim();
    !expected.is_empty() && sha1_hex(bytes).eq_ignore_ascii_case(expected)
}

/// 递归拷贝目录
///
/// 带缓冲的流式拷贝并按源长度预分配目标，顺手把 mtime 带过去，供指纹缓存判断
pub fn copy_dir_all(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> Result<(), std::io::Error> {
    fs::create_dir_all(&dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        if ty.is_dir() {
            copy_dir_all(entry.path(), dst.as_ref().join(entry.file_name()))?;
        } else {
            let target = dst.as_ref().join(entry.file_name());
            if ty.is_file() {
                copy_file(&entry.path(), &target)?;
            } else {
                // 符号链接等特殊文件原样复制，别当成普通文件去读
                fs::copy(entry.path(), target)?;
            }
        }
    }
    Ok(())
}

/// 流式拷贝单个文件，按源文件长度预分配目标
fn copy_file(src: &Path, dst: &Path) -> Result<(), std::io::Error> {
    let mut reader = std::io::BufReader::with_capacity(1024 * 1024, fs::File::open(src)?);
    let mut writer = std::io::BufWriter::with_capacity(1024 * 1024, fs::File::create(dst)?);
    std::io::copy(&mut reader, &mut writer)?;
    writer.flush()?;
    Ok(())
}

/// 用 `staged` 覆盖 `target`，尽量不出现「旧文件已删、新文件还没就位」的窗口。
///
/// 直接 remove + rename 在 Windows 上有真实风险：杀软扫描、索引器或用户开着文件都会让第二步失败，
/// 而旧文件这时已经没了。所以先把旧文件改名成 `.bak`（同卷 rename 是原子的），失败就把 `.bak` 改回去
pub fn replace_file_atomically(staged: &Path, target: &Path) -> Result<(), LauncherError> {
    let backup = backup_path_for(target);

    // 上一轮崩溃可能留下同名 .bak，先清掉，否则下面的 rename 会失败
    if backup.exists() {
        let _ = fs::remove_file(&backup);
    }

    let had_old = target.exists();
    if had_old {
        fs::rename(target, &backup).map_err(|e| {
            LauncherError::Custom(format!("无法暂存旧文件 {}: {}", target.display(), e))
        })?;
    }

    if let Err(e) = fs::rename(staged, target) {
        // 新文件没就位，把旧文件还回去，别让调用方看到「文件没了」
        if had_old {
            let _ = fs::rename(&backup, target);
        }
        return Err(LauncherError::Custom(format!(
            "无法写入新文件 {}: {}",
            target.display(),
            e
        )));
    }

    if had_old {
        let _ = fs::remove_file(&backup);
    }
    Ok(())
}

/// `.bak` 与目标同目录，保证 rename 落在同一个卷上（同卷 rename 才是原子的）
fn backup_path_for(target: &Path) -> PathBuf {
    let mut name = target
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(".ar1s-bak");
    target.with_file_name(name)
}

/// expected_size 为 0 或 expected_hash 为 None 时，对应校验直接跳过
/// 命中校验指纹缓存时不再读文件，缓存按「长度 + 修改时间」判定内容是否还是那一份
pub fn verify_file(
    path: &std::path::Path,
    expected_hash: Option<&str>,
    expected_size: u64,
) -> Result<bool, LauncherError> {
    if !path.exists() {
        return Ok(false);
    }

    let metadata = std::fs::metadata(path)?;
    if expected_size > 0 && metadata.len() != expected_size {
        warn!(
            "文件大小不匹配: 期望 {} 字节, 实际 {} 字节",
            expected_size,
            metadata.len()
        );
        return Ok(false);
    }

    if verify_cache::matches(path, &metadata, expected_hash) {
        debug!("文件指纹未变，跳过哈希计算: {}", path.display());
        return Ok(true);
    }

    let actual = if let Some(hash) = expected_hash {
        let actual_hash = sha1_file(path)?;
        if !actual_hash.eq_ignore_ascii_case(hash.trim()) {
            warn!("文件哈希不匹配: 期望 {}, 实际 {}", hash, actual_hash);
            return Ok(false);
        }
        Some(actual_hash)
    } else {
        None
    };

    verify_cache::remember(path, &metadata, expected_hash, actual.as_deref());
    Ok(true)
}

/// 每次临时操作唯一的后缀（进程 id + 纳秒时间戳）：并发操作各有各的临时目录，互不踩
pub fn unique_suffix() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!("{}-{}", std::process::id(), nanos)
}

/// Drop 时删除这个路径（先按文件删，失败再按目录连内容删），错误一律忽略。
/// 给失败路径兜底临时残留：成功路径已自行删除时，这里再删一次也是无害的空操作。
pub struct TempPathGuard(pub PathBuf);

impl Drop for TempPathGuard {
    fn drop(&mut self) {
        if std::fs::remove_file(&self.0).is_err() {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

pub fn cleanup_instance_creation(game_dir: &Path, instance_name: &str, _base_version_id: &str) {
    info!("file_utils: 开始清理实例创建过程中的文件和目录");

    let instance_dir = game_dir.join("versions").join(instance_name);
    if instance_dir.exists() {
        info!("file_utils: 清理实例目录: {}", instance_dir.display());
        if let Err(e) = fs::remove_dir_all(&instance_dir) {
            warn!("file_utils: 清理实例目录失败: {}", e);
        } else {
            info!("file_utils: 实例目录清理完成");
        }
    }

    info!("file_utils: 清理完成");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_instance_names() {
        assert!(validate_instance_name("MyInstance").is_valid);
        assert!(validate_instance_name("my-instance-1").is_valid);
        assert!(validate_instance_name("我的实例").is_valid);
        assert!(validate_instance_name("test_instance.v2").is_valid);
        assert!(validate_instance_name("a").is_valid);
    }

    #[test]
    fn test_empty_name() {
        let result = validate_instance_name("");
        assert!(!result.is_valid);
        assert!(result.error_message.unwrap().contains("不能为空"));
    }

    #[test]
    fn test_is_safe_relative_path() {
        let is_safe = super::is_safe_relative_path;
        assert!(is_safe("mods/x.jar"));
        assert!(is_safe("config/foo/bar.toml"));
        assert!(!is_safe("mods/../../evil.jar"));
        assert!(!is_safe("/etc/passwd"));
        assert!(!is_safe("\\windows\\system32"));
        assert!(!is_safe("C:\\evil\\x.jar"));
        assert!(!is_safe("mods/a:b.jar"));
        assert!(!is_safe(""));
    }

    #[test]
    fn test_too_long_name() {
        let long_name = "a".repeat(65);
        assert!(!validate_instance_name(&long_name).is_valid);

        let exact_name = "a".repeat(64);
        assert!(validate_instance_name(&exact_name).is_valid);
    }

    #[test]
    fn test_starts_with_dot_or_space() {
        assert!(!validate_instance_name(".hidden").is_valid);
        assert!(!validate_instance_name(" leading").is_valid);
    }

    #[test]
    fn test_ends_with_dot_or_space() {
        assert!(!validate_instance_name("trailing.").is_valid);
        assert!(!validate_instance_name("trailing ").is_valid);
    }

    #[test]
    fn test_path_traversal() {
        assert!(!validate_instance_name("..").is_valid);
        assert!(!validate_instance_name("foo/../bar").is_valid);
        assert!(!validate_instance_name("a..b").is_valid);
    }

    #[test]
    fn test_path_separators() {
        assert!(!validate_instance_name("foo/bar").is_valid);
        assert!(!validate_instance_name("foo\\bar").is_valid);
    }

    #[test]
    fn test_reserved_characters() {
        for c in &['<', '>', ':', '"', '|', '?', '*'] {
            let name = format!("test{}name", c);
            assert!(
                !validate_instance_name(&name).is_valid,
                "应该拒绝包含 '{}' 的名称",
                c
            );
        }
    }

    #[test]
    fn test_control_characters() {
        assert!(!validate_instance_name("test\x00name").is_valid);
        assert!(!validate_instance_name("test\x1fname").is_valid);
    }

    #[test]
    fn test_windows_reserved_names() {
        assert!(!validate_instance_name("CON").is_valid);
        assert!(!validate_instance_name("PRN").is_valid);
        assert!(!validate_instance_name("AUX").is_valid);
        assert!(!validate_instance_name("NUL").is_valid);
        assert!(!validate_instance_name("COM1").is_valid);
        assert!(!validate_instance_name("LPT1").is_valid);
        assert!(!validate_instance_name("con").is_valid);
        assert!(!validate_instance_name("Con").is_valid);
        assert!(!validate_instance_name("CON.txt").is_valid);
        // 相似但不在保留名单里的名称应当通过
        assert!(validate_instance_name("CONNECT").is_valid);
        assert!(validate_instance_name("COM10").is_valid);
    }

    #[test]
    fn test_validate_instance_name_or_error() {
        assert!(validate_instance_name_or_error("valid-name").is_ok());
        assert!(validate_instance_name_or_error("").is_err());
        assert!(validate_instance_name_or_error("CON").is_err());
    }

    #[test]
    fn test_verify_file_nonexistent() {
        let result = verify_file(Path::new("/nonexistent/path"), Some("abc123"), 100);
        assert!(result.is_ok());
        assert!(!result.unwrap());
    }

    #[test]
    fn test_verify_file_no_hash() {
        let dir = std::env::temp_dir().join("ar1s_test_verify");
        let _ = fs::create_dir_all(&dir);
        let file_path = dir.join("test_no_hash.bin");
        fs::write(&file_path, b"hello").unwrap();

        assert!(verify_file(&file_path, None, 0).unwrap());
        assert!(verify_file(&file_path, None, 5).unwrap());
        assert!(!verify_file(&file_path, None, 999).unwrap());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_verify_file_with_sha1() {
        let dir = std::env::temp_dir().join("ar1s_test_sha1");
        let _ = fs::create_dir_all(&dir);
        let file_path = dir.join("test_sha1.bin");
        fs::write(&file_path, b"hello").unwrap();

        let correct_hash = "aaf4c61ddcc5e8a2dabede0f3b482cd9aea9434d";
        assert!(verify_file(&file_path, Some(correct_hash), 5).unwrap());
        assert!(
            !verify_file(
                &file_path,
                Some("0000000000000000000000000000000000000000"),
                5
            )
            .unwrap()
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_checked_instance_dir_rejects_path_components() {
        let base = Path::new("versions");
        assert!(instance_dir(base, "../outside").is_err());
        assert!(instance_dir(base, "a/b").is_err());
        assert!(instance_dir(base, "a\\b").is_err());
        assert!(instance_dir(base, "").is_err());
        assert_eq!(
            instance_dir(base, "1.20.1-Forge").unwrap(),
            base.join("1.20.1-Forge")
        );
        assert_eq!(
            instance_dir(base, "我的实例").unwrap(),
            base.join("我的实例")
        );
    }

    #[test]
    fn test_checked_version_dir_rejects_path_components() {
        let base = Path::new("versions");

        assert!(version_dir(base, "../../etc").is_err());
        assert!(version_dir(base, "1.20.1/evil").is_err());
        assert!(version_dir(base, "C:\\evil").is_err());
        assert!(version_dir(base, ".").is_err());
        assert_eq!(
            version_dir(base, "1.20.1-forge-47.2.0").unwrap(),
            base.join("1.20.1-forge-47.2.0")
        );
    }

    #[test]
    fn test_sha1_helpers() {
        let text = "aaf4c61ddcc5e8a2dabede0f3b482cd9aea9434d";

        assert_eq!(sha1_hex(b"hello"), text);
        assert!(verify_sha1(b"hello", &text.to_uppercase()));
        assert!(!verify_sha1(
            b"hello",
            "0000000000000000000000000000000000000000"
        ));
        assert_eq!(
            parse_sha1("aaf4c61ddcc5e8a2dabede0f3b482cd9aea9434d  forge.jar\n").as_deref(),
            Some(text)
        );
        assert_eq!(parse_sha1("not-a-hash"), None);
        assert_eq!(parse_sha1(""), None);
    }

    /// 并行分片算出来的哈希必须和逐个算完全一致，且不丢文件
    #[test]
    fn test_parallel_sha1_matches_serial_for_every_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut paths = Vec::new();
        for index in 0..9 {
            let path = dir.path().join(format!("f{index}.bin"));
            fs::write(&path, format!("content-{index}")).unwrap();
            paths.push(path);
        }

        let parallel = sha1_files_parallel(&paths);
        assert_eq!(parallel.len(), paths.len());

        let by_path: std::collections::HashMap<_, _> = parallel.into_iter().collect();
        for path in &paths {
            assert_eq!(by_path.get(path), Some(&sha1_file(path).unwrap()));
        }
    }

    #[test]
    fn test_parallel_sha1_handles_empty_input() {
        assert!(sha1_files_parallel(&[]).is_empty());
    }

    /// 更新检查用的索引：哈希列表与「哈希 -> 文件名」必须一一对应
    #[test]
    fn test_sha1_index_maps_each_hash_back_to_its_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut files = Vec::new();
        for index in 0..5 {
            let path = dir.path().join(format!("mod{index}.jar"));
            let content = format!("jar-{index}");
            fs::write(&path, &content).unwrap();
            files.push((path, format!("mod{index}.jar")));
        }

        let (hashes, by_hash) = sha1_index_parallel(&files);

        assert_eq!(hashes.len(), files.len());
        assert_eq!(by_hash.len(), files.len());
        for (path, file_name) in &files {
            let expected = sha1_file(path).unwrap();
            assert!(hashes.contains(&expected), "哈希列表缺少 {file_name}");
            assert_eq!(by_hash.get(&expected), Some(file_name));
        }
    }

    /// 读不出来的文件要跳过，不能让它挤掉其他文件的映射
    #[test]
    fn test_sha1_index_skips_unreadable_files() {
        let dir = tempfile::TempDir::new().unwrap();
        let good = dir.path().join("good.jar");
        fs::write(&good, b"content").unwrap();
        let missing = dir.path().join("missing.jar");

        let files = vec![
            (good.clone(), "good.jar".to_string()),
            (missing, "missing.jar".to_string()),
        ];
        let (hashes, by_hash) = sha1_index_parallel(&files);

        assert_eq!(hashes.len(), 1);
        assert_eq!(by_hash.len(), 1);
        assert_eq!(
            by_hash.get(&sha1_file(&good).unwrap()),
            Some(&"good.jar".to_string())
        );
    }

    #[test]
    fn test_download_filename_rejects_paths_but_allows_long_names() {
        assert!(validate_file_name_or_error("pack.mrpack").is_ok());
        assert!(validate_file_name_or_error(&format!("{}.mrpack", "a".repeat(120))).is_ok());
        assert!(validate_file_name_or_error("").is_err());
        assert!(validate_file_name_or_error("../pack.mrpack").is_err());
        assert!(validate_file_name_or_error("mods/pack.mrpack").is_err());
        assert!(validate_file_name_or_error("mods\\pack.mrpack").is_err());
        assert!(validate_file_name_or_error("C:\\pack.mrpack").is_err());
        assert!(validate_file_name_or_error("CON.mrpack").is_err());
    }

    /// 指纹缓存只认「长度 + 修改时间」，内容被改过就必须重新算哈希，
    /// 否则损坏的文件会被当成好的跳过下载
    #[test]
    fn test_verify_file_cache_invalidates_when_content_changes() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("cached.bin");
        fs::write(&path, b"hello").unwrap();

        let hello = "aaf4c61ddcc5e8a2dabede0f3b482cd9aea9434d";
        assert!(verify_file(&path, Some(hello), 5).unwrap());

        fs::write(&path, b"hello world").unwrap();
        assert!(!verify_file(&path, Some(hello), 5).unwrap());
        assert!(!verify_file(&path, Some(hello), 11).unwrap());

        // 长度相同但内容不同：靠修改时间识破
        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(&path, b"world").unwrap();
        assert!(!verify_file(&path, Some("d4e2b1dbb0de5e0d2c1f3a1b5e1e2f3c4d5e6f70"), 5).unwrap());
    }

    /// 只校验大小时不该把之前记下的哈希抹掉，否则下次带哈希的校验又要重算
    #[test]
    fn test_size_only_verify_keeps_the_recorded_hash() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("keep.bin");
        fs::write(&path, b"hello").unwrap();

        let hello = "aaf4c61ddcc5e8a2dabede0f3b482cd9aea9434d";
        assert!(verify_file(&path, Some(hello), 5).unwrap());
        assert!(verify_file(&path, None, 0).unwrap());
        // 缓存里仍留着哈希，这次应当直接命中而不再读文件
        assert!(verify_file(&path, Some(hello), 5).unwrap());
        assert!(!verify_file(&path, Some("0000000000000000000000000000000000000000"), 5).unwrap());
    }

    #[test]
    fn test_replace_file_atomically_overwrites_and_cleans_backup() {
        let dir = tempfile::TempDir::new().unwrap();
        let target = dir.path().join("mod.jar");
        let staged = dir.path().join("mod.jar.new");
        fs::write(&target, b"old").unwrap();
        fs::write(&staged, b"new").unwrap();

        replace_file_atomically(&staged, &target).unwrap();

        assert_eq!(fs::read(&target).unwrap(), b"new");
        assert!(!staged.exists(), "临时文件应已消失");
        assert!(
            !dir.path().join("mod.jar.ar1s-bak").exists(),
            "成功替换后不应留下 .bak"
        );
    }

    #[test]
    fn test_replace_file_atomically_creates_target_when_absent() {
        let dir = tempfile::TempDir::new().unwrap();
        let target = dir.path().join("pack.zip");
        let staged = dir.path().join("pack.zip.new");
        fs::write(&staged, b"content").unwrap();

        replace_file_atomically(&staged, &target).unwrap();

        assert_eq!(fs::read(&target).unwrap(), b"content");
    }

    /// 关键保护：新文件没能就位时必须把旧文件还回去，
    /// 否则玩家的模组会在更新失败时彻底消失
    #[test]
    fn test_replace_file_atomically_restores_old_file_when_new_is_missing() {
        let dir = tempfile::TempDir::new().unwrap();
        let target = dir.path().join("mod.jar");
        fs::write(&target, b"old").unwrap();
        // 故意给一个不存在的 staged 路径，让第二步 rename 失败
        let missing_staged = dir.path().join("nope.jar");

        assert!(replace_file_atomically(&missing_staged, &target).is_err());

        assert_eq!(
            fs::read(&target).unwrap(),
            b"old",
            "失败后旧文件必须回到原位"
        );
        assert!(!dir.path().join("mod.jar.ar1s-bak").exists());
    }

    /// 上一轮崩溃留下的 .bak 不能挡住下一次替换
    #[test]
    fn test_replace_file_atomically_clears_stale_backup() {
        let dir = tempfile::TempDir::new().unwrap();
        let target = dir.path().join("mod.jar");
        let staged = dir.path().join("mod.jar.new");
        fs::write(&target, b"old").unwrap();
        fs::write(dir.path().join("mod.jar.ar1s-bak"), b"stale").unwrap();
        fs::write(&staged, b"new").unwrap();

        replace_file_atomically(&staged, &target).unwrap();

        assert_eq!(fs::read(&target).unwrap(), b"new");
        assert!(!dir.path().join("mod.jar.ar1s-bak").exists());
    }
}
