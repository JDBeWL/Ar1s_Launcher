//! 下载断点状态
//!
//! 以 URL 为键记录已完成与失败项并落盘，进行中的只留内存，dirty 决定何时写盘

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadState {
    pub completed_files: HashSet<String>,
    pub failed_files: HashSet<String>,
    /// URL -> 已下载字节数
    #[serde(default)]
    pub partial_downloads: HashMap<String, u64>,
    #[serde(skip)]
    pub active_downloads: HashMap<String, PathBuf>,
    #[serde(skip)]
    pub dirty: bool,
}

impl DownloadState {
    pub fn new() -> Self {
        Self {
            completed_files: HashSet::new(),
            failed_files: HashSet::new(),
            partial_downloads: HashMap::new(),
            active_downloads: HashMap::new(),
            dirty: false,
        }
    }

    pub fn load_from_file(path: &std::path::Path) -> Option<Self> {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|content| serde_json::from_str(&content).ok())
    }

    pub fn save_to_file(&mut self, path: &std::path::Path) -> Result<(), std::io::Error> {
        let content = self.to_json()?;
        std::fs::write(path, content)?;
        self.dirty = false;
        Ok(())
    }

    /// 只序列化、不落盘。调用方拿它配合 [`Self::mark_saved`]，先放开 `tokio::sync::Mutex`
    /// 再在阻塞线程里写文件——否则一次 `fs::write` 会把所有等着 `mark_completed` 的任务一起卡住
    pub fn to_json(&self) -> Result<String, std::io::Error> {
        serde_json::to_string(self).map_err(std::io::Error::other)
    }

    /// 落盘成功后调用，清掉 dirty 标记
    pub fn mark_saved(&mut self) {
        self.dirty = false;
    }

    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    pub fn mark_completed(&mut self, url: String) {
        self.partial_downloads.remove(&url);
        // 上次会话标记过失败的文件这次成功了，要把失败记录一并撤掉：
        // 否则收尾时会误报「部分失败」，状态文件也永远清不掉
        self.failed_files.remove(&url);
        self.completed_files.insert(url);
        self.mark_dirty();
    }

    pub fn mark_failed(&mut self, url: String) {
        self.failed_files.insert(url);
        self.mark_dirty();
    }

    #[allow(dead_code)]
    pub fn update_partial(&mut self, url: String, bytes: u64) {
        self.partial_downloads.insert(url, bytes);
        self.mark_dirty();
    }

    #[allow(dead_code)]
    pub fn get_partial_bytes(&self, url: &str) -> u64 {
        self.partial_downloads.get(url).copied().unwrap_or(0)
    }

    pub fn start_download(&mut self, url: String, path: PathBuf) {
        self.active_downloads.insert(url, path);
    }

    pub fn finish_download(&mut self, url: &str) {
        self.active_downloads.remove(url);
    }

    pub fn is_completed(&self, url: &str) -> bool {
        self.completed_files.contains(url)
    }

    #[allow(dead_code)]
    pub fn clear_failed(&mut self, url: &str) {
        self.failed_files.remove(url);
        self.mark_dirty();
    }

    #[allow(dead_code)]
    pub fn reset(&mut self) {
        self.completed_files.clear();
        self.failed_files.clear();
        self.partial_downloads.clear();
        self.active_downloads.clear();
        self.dirty = true;
    }
}

impl Default for DownloadState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_state_is_empty() {
        let state = DownloadState::new();
        assert!(state.completed_files.is_empty());
        assert!(state.failed_files.is_empty());
        assert!(state.partial_downloads.is_empty());
        assert!(state.active_downloads.is_empty());
        assert!(!state.dirty);
    }

    #[test]
    fn test_mark_completed() {
        let mut state = DownloadState::new();
        state.update_partial("http://example.com/a".into(), 512);

        state.mark_completed("http://example.com/a".into());

        assert!(state.is_completed("http://example.com/a"));
        assert!(!state.is_completed("http://example.com/b"));
        assert_eq!(state.get_partial_bytes("http://example.com/a"), 0);
        assert!(state.dirty);
    }

    #[test]
    fn test_mark_completed_idempotent() {
        let mut state = DownloadState::new();
        state.mark_completed("http://example.com/a".into());
        state.mark_completed("http://example.com/a".into());
        assert_eq!(state.completed_files.len(), 1);
    }

    #[test]
    fn test_mark_failed() {
        let mut state = DownloadState::new();
        state.mark_failed("http://example.com/a".into());
        assert!(state.failed_files.contains("http://example.com/a"));
        assert!(state.dirty);
    }

    #[test]
    fn test_clear_failed() {
        let mut state = DownloadState::new();
        state.mark_failed("http://example.com/a".into());
        state.mark_failed("http://example.com/b".into());

        state.clear_failed("http://example.com/a");
        assert!(!state.failed_files.contains("http://example.com/a"));
        assert!(state.failed_files.contains("http://example.com/b"));
    }

    #[test]
    fn test_start_and_finish_download() {
        let mut state = DownloadState::new();
        let path = PathBuf::from("/tmp/test.jar");
        state.start_download("http://example.com/a".into(), path.clone());
        assert_eq!(state.active_downloads.len(), 1);

        state.finish_download("http://example.com/a");
        assert!(state.active_downloads.is_empty());
    }

    #[test]
    fn test_reset() {
        let mut state = DownloadState::new();
        state.mark_completed("http://example.com/a".into());
        state.mark_failed("http://example.com/b".into());
        state.update_partial("http://example.com/c".into(), 1024);
        state.start_download("http://example.com/d".into(), PathBuf::from("/tmp/d"));

        state.reset();
        assert!(state.completed_files.is_empty());
        assert!(state.failed_files.is_empty());
        assert!(state.partial_downloads.is_empty());
        assert!(state.active_downloads.is_empty());
        assert!(state.dirty);
    }

    #[test]
    fn test_serialize_deserialize_roundtrip() {
        let mut state = DownloadState::new();
        state.mark_completed("http://example.com/a".into());
        state.mark_completed("http://example.com/b".into());
        state.mark_failed("http://example.com/c".into());
        state.update_partial("http://example.com/d".into(), 2048);

        let json = serde_json::to_string(&state).unwrap();
        let restored: DownloadState = serde_json::from_str(&json).unwrap();

        assert_eq!(restored.completed_files.len(), 2);
        assert!(restored.is_completed("http://example.com/a"));
        assert!(restored.is_completed("http://example.com/b"));
        assert!(restored.failed_files.contains("http://example.com/c"));
        assert_eq!(restored.get_partial_bytes("http://example.com/d"), 2048);
        assert!(restored.active_downloads.is_empty());
        assert!(!restored.dirty);
    }

    #[test]
    fn test_save_and_load_file() {
        let dir = std::env::temp_dir().join("ar1s_test_dl_state");
        let _ = std::fs::create_dir_all(&dir);
        let state_file = dir.join("test_state.json");

        let mut state = DownloadState::new();
        state.mark_completed("http://example.com/1".into());
        state.mark_failed("http://example.com/2".into());
        state.save_to_file(&state_file).unwrap();

        let loaded = DownloadState::load_from_file(&state_file).unwrap();
        assert!(loaded.is_completed("http://example.com/1"));
        assert!(loaded.failed_files.contains("http://example.com/2"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_load_from_nonexistent_file() {
        let result = DownloadState::load_from_file(std::path::Path::new("/nonexistent/state.json"));
        assert!(result.is_none());
    }
}
