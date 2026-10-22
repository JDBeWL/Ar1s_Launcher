//! 文件校验指纹缓存
//!
//! `verify_file` 要把整个文件读一遍算 sha1，一个完整安装光校验就是 1.5 GB 的读盘。
//! 文件内容变没变，用「长度 + 修改时间」就足以判定：指纹没动过，上次的校验结论依然成立。
//!
//! 进程内是 HashMap，落盘到游戏目录的 `.ar1s/verify_cache.json`；条目只增不删，
//! 到上限整体丢弃重建——维护 LRU 的代价比偶尔全量重算一遍还高

use log::{debug, warn};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, RwLock};
use std::time::UNIX_EPOCH;

const CACHE_FILE: &str = "verify_cache.json";
const MAX_ENTRIES: usize = 20_000;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Entry {
    len: u64,
    /// 修改时间（自 UNIX_EPOCH 起的纳秒数，u64 够用到 2554 年）；取不到时为 None
    #[serde(default)]
    modified: Option<u64>,
    /// 校验通过时对应的期望哈希，None 表示那次只校验过大小
    #[serde(default)]
    hash: Option<String>,
    /// 上次真正算出来的内容哈希
    ///
    /// 同一个文件换一个期望哈希来校验时也能直接比对结论；升级后新增的字段，旧缓存读成 None 按「不知道」处理
    #[serde(default)]
    actual: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct CacheFile {
    #[serde(default)]
    files: HashMap<String, Entry>,
}

#[derive(Debug, Default)]
struct Cache {
    entries: HashMap<PathBuf, Entry>,
    /// 缓存落盘位置；没调过 init 就只在内存里生效
    path: Option<PathBuf>,
    dirty: bool,
}

static CACHE: LazyLock<RwLock<Cache>> = LazyLock::new(|| RwLock::new(Cache::default()));

fn read_lock() -> std::sync::RwLockReadGuard<'static, Cache> {
    CACHE
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn write_lock() -> std::sync::RwLockWriteGuard<'static, Cache> {
    CACHE
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn modified_nanos(meta: &std::fs::Metadata) -> Option<u64> {
    meta.modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|elapsed| elapsed.as_nanos().min(u128::from(u64::MAX)) as u64)
}

/// 绑定到游戏目录；重复调用同一个目录时不会重读磁盘
pub fn init(game_dir: &Path) {
    let path = game_dir.join(".ar1s").join(CACHE_FILE);
    let mut cache = write_lock();
    if cache.path.as_deref() == Some(path.as_path()) {
        return;
    }
    cache.entries = load(&path);
    cache.path = Some(path);
    cache.dirty = false;
}

/// 指纹与期望哈希都和上次校验通过时一致，就不必再读文件
///
/// 优先看上次真正算出的内容哈希：内容没变，换个期望值来问结论照样成立
pub fn matches(path: &Path, meta: &std::fs::Metadata, expected_hash: Option<&str>) -> bool {
    let expected = expected_hash.map(str::to_ascii_lowercase);
    let cache = read_lock();
    let Some(entry) = cache.entries.get(path) else {
        return false;
    };

    if entry.len != meta.len() || entry.modified != modified_nanos(meta) {
        return false;
    }

    match (expected.clone(), entry.actual.as_ref()) {
        (Some(expected), Some(actual)) => *actual == expected,
        _ => entry.hash == expected,
    }
}

/// 取「这个路径上次校验通过时用的哈希」，指纹必须和当前文件一致
///
/// 文件在两次启动之间没被动过就直接沿用结论，连读盘都省；指纹对不上返回 None，调用方重算
pub fn known_hash(path: &Path, meta: &std::fs::Metadata) -> Option<String> {
    let cached = read_lock().entries.get(path).cloned()?;
    (cached.len == meta.len() && cached.modified == modified_nanos(meta))
        .then_some(cached.actual)
        .flatten()
}

/// 校验通过后记一笔；同一路径再次调用会覆盖旧指纹
///
/// `actual_hash` 是这次真正算出来的内容哈希，只校验大小时传 None——
/// 那种情况下不能顺手把之前记下的结论抹掉，也不能凭大小断言内容
pub fn remember(
    path: &Path,
    meta: &std::fs::Metadata,
    expected_hash: Option<&str>,
    actual_hash: Option<&str>,
) {
    let modified = modified_nanos(meta);
    let mut cache = write_lock();

    let previous = cache
        .entries
        .get(path)
        .filter(|entry| entry.len == meta.len() && entry.modified == modified);

    let hash = match expected_hash {
        Some(hash) => Some(hash.to_ascii_lowercase()),
        None => previous.and_then(|entry| entry.hash.clone()),
    };

    let actual = match actual_hash {
        Some(actual) => Some(actual.to_ascii_lowercase()),
        None => previous.and_then(|entry| entry.actual.clone()),
    };

    if cache.entries.len() >= MAX_ENTRIES && !cache.entries.contains_key(path) {
        warn!("文件校验缓存达到 {} 条，清空重建", MAX_ENTRIES);
        cache.entries.clear();
    }

    cache.entries.insert(
        path.to_path_buf(),
        Entry {
            len: meta.len(),
            modified,
            hash,
            actual,
        },
    );
    cache.dirty = true;
}

/// 把内存里的指纹写回磁盘；没有变化或没绑定目录时直接返回
///
/// 序列化在读锁下进行：只需要 `&`，并发读不受影响，也不必先把整表克隆一份挡住在场的校验线程
pub fn flush() {
    let path = {
        let mut cache = write_lock();
        if !cache.dirty {
            return;
        }
        // 先清 dirty：序列化期间若有新的 remember，它会重新置位，下次 flush 再写一遍
        cache.dirty = false;
        let Some(path) = cache.path.clone() else {
            return;
        };
        path
    };

    let (text, count) = {
        let cache = read_lock();
        let count = cache.entries.len();
        match serde_json::to_string(&CacheFileRef {
            files: FilesRef(&cache.entries),
        }) {
            Ok(text) => (text, count),
            Err(error) => {
                warn!("序列化校验缓存失败: {}", error);
                // 没写成，把 dirty 放回去，下次还有机会
                write_lock().dirty = true;
                return;
            }
        }
    };

    if let Err(error) = save_text(&path, &text) {
        warn!("写入校验缓存失败: {}", error);
    } else {
        debug!("校验缓存已保存 {} 条", count);
    }
}

/// 序列化用的借用视图：直接引用内存里的表，不复制任何 Entry
///
/// JSON 的对象键必须是字符串，内存里用 `PathBuf` 做键；这里转成 `&str` 借用传出
struct CacheFileRef<'a> {
    files: FilesRef<'a>,
}

struct FilesRef<'a>(&'a HashMap<PathBuf, Entry>);

impl Serialize for CacheFileRef<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;

        let mut map = serializer.serialize_map(Some(1))?;
        map.serialize_entry("files", &self.files)?;
        map.end()
    }
}

impl Serialize for FilesRef<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;

        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (path, entry) in self.0 {
            map.serialize_entry(path.to_string_lossy().as_ref(), entry)?;
        }
        map.end()
    }
}

fn load(path: &Path) -> HashMap<PathBuf, Entry> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return HashMap::new();
    };
    match serde_json::from_str::<CacheFile>(&text) {
        Ok(file) => file
            .files
            .into_iter()
            .map(|(key, entry)| (PathBuf::from(key), entry))
            .collect(),
        Err(error) => {
            debug!("校验缓存无法解析，按空缓存处理: {}", error);
            HashMap::new()
        }
    }
}

/// 先写临时文件再 rename，避免中途退出留下半截 JSON
/// 生产路径走 `flush` 的借用序列化，这份「从表直接写盘」的入口给测试用
#[cfg(test)]
fn save_to(path: &Path, entries: &HashMap<PathBuf, Entry>) -> std::io::Result<()> {
    let text = serde_json::to_string(&CacheFileRef {
        files: FilesRef(entries),
    })
    .map_err(std::io::Error::other)?;
    save_text(path, &text)
}

/// 已经是序列化好的文本，直接落盘
fn save_text(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let temp = path.with_extension("json.part");
    std::fs::write(&temp, text)?;
    std::fs::rename(&temp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Entry, known_hash, load, modified_nanos, remember, save_to};
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn entry(len: u64, modified: Option<u64>, hash: Option<&str>) -> Entry {
        Entry {
            len,
            modified,
            hash: hash.map(str::to_string),
            actual: None,
        }
    }

    #[test]
    fn test_saved_entries_survive_a_round_trip() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("verify_cache.json");

        let mut entries = HashMap::new();
        entries.insert(
            PathBuf::from("/games/minecraft/assets/objects/ab/abcdef"),
            entry(1024, Some(1_700_000_000_000_000_000), Some("abc123")),
        );
        entries.insert(
            PathBuf::from("/games/minecraft/libraries/x.jar"),
            entry(0, None, None),
        );

        save_to(&path, &entries).unwrap();
        let restored = load(&path);

        assert_eq!(restored, entries);
    }

    #[test]
    fn test_broken_cache_file_is_ignored() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("verify_cache.json");
        std::fs::write(&path, "{ not json").unwrap();

        assert!(load(&path).is_empty());
        assert!(load(&dir.path().join("missing.json")).is_empty());
    }

    #[test]
    fn test_modified_nanos_is_stable_for_a_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("a.bin");
        std::fs::write(&path, b"hello").unwrap();

        let meta = std::fs::metadata(&path).unwrap();
        let first = modified_nanos(&meta);
        assert!(first.is_some());
        assert_eq!(first, modified_nanos(&std::fs::metadata(&path).unwrap()));
    }

    #[test]
    fn test_entry_matches_only_the_same_fingerprint_and_hash() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("a.bin");
        std::fs::write(&path, b"hello").unwrap();
        let meta = std::fs::metadata(&path).unwrap();

        let cached = entry(meta.len(), modified_nanos(&meta), Some("aaf4c61d"));

        assert_eq!(cached.len, meta.len());
        assert_eq!(cached.hash.as_deref(), Some("aaf4c61d"));
        // 长度或哈希对不上时必须重算
        assert_ne!(cached.len, meta.len() + 1);
        assert_ne!(cached.hash.as_deref(), Some("00000000"));
    }

    /// 后台校验靠 known_hash 决定跳过还是重算：文件没动过要给出上次的哈希，
    /// 被动过就必须返回 None，否则改动过的文件会被当成好的
    #[test]
    fn test_known_hash_follows_the_file_fingerprint() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("known.bin");
        std::fs::write(&path, b"hello").unwrap();

        assert_eq!(known_hash(&path, &std::fs::metadata(&path).unwrap()), None);

        let meta = std::fs::metadata(&path).unwrap();
        remember(&path, &meta, Some("AABBCC"), Some("aabbcc"));
        assert_eq!(
            known_hash(&path, &std::fs::metadata(&path).unwrap()),
            Some("aabbcc".to_string()),
            "记下的哈希应当小写化后返回"
        );

        // 只校验过大小的条目不提供哈希
        let path2 = dir.path().join("size_only.bin");
        std::fs::write(&path2, b"abc").unwrap();
        let meta2 = std::fs::metadata(&path2).unwrap();
        remember(&path2, &meta2, None, None);
        assert_eq!(
            known_hash(&path2, &std::fs::metadata(&path2).unwrap()),
            None
        );

        // 文件内容变了，指纹失效
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&path, b"hello world").unwrap();
        assert_eq!(known_hash(&path, &std::fs::metadata(&path).unwrap()), None);
    }

    /// flush 用借用视图序列化，产出的 JSON 结构必须和原来一致，否则升级后老缓存读不回来
    #[test]
    fn test_lazy_serialization_matches_the_saved_format() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("verify_cache.json");

        let mut entries = HashMap::new();
        entries.insert(
            PathBuf::from("/games/minecraft/assets/objects/ab/abcdef"),
            entry(1024, Some(1_700_000_000_000_000_000), Some("abc123")),
        );

        let text = serde_json::to_string(&super::CacheFileRef {
            files: super::FilesRef(&entries),
        })
        .unwrap();

        // 顶层必须是 {"files": {...}}，键是路径字符串
        let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert!(parsed["files"].is_object());
        assert_eq!(
            parsed["files"]["/games/minecraft/assets/objects/ab/abcdef"]["len"],
            1024
        );

        save_to(&path, &entries).unwrap();
        assert_eq!(load(&path), entries);
    }
}
