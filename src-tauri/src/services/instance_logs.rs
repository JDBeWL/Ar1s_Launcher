//! 启动历史、日志归档与日志/崩溃报告读取

use std::collections::{HashSet, VecDeque};
use std::fs::{self, File};
use std::io::{self, BufRead, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use serde::{Deserialize, Serialize};

use crate::errors::LauncherError;
use crate::models::{
    ContentTarget, CrashReportInfo, CrashReportText, LAUNCH_HISTORY_LIMIT, LaunchOutcome,
    LaunchRecord, LogTail,
};
use crate::services::config;
use crate::services::launcher::isolation;
use crate::utils::{file_lock, file_utils};

const HISTORY_FILE_NAME: &str = "launch-history.json";
const HISTORY_LOCK_NAME: &str = "launch-history.lock";
const HISTORY_FORMAT: u32 = 1;
const ARCHIVE_DIR_NAME: &str = "launch-logs";
const HISTORY_LOCK_TIMEOUT: Duration = Duration::from_secs(2);
const LOG_TAIL_LINES: usize = 5000;
const LOG_TAIL_BYTES: u64 = 2 * 1024 * 1024;
const LOG_LINE_MAX_CHARS: usize = 4000;
const CRASH_TEXT_BYTES: u64 = 1024 * 1024;
const CRASH_REPORT_LIMIT: usize = 100;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LaunchHistoryFile {
    format: u32,
    records: Vec<LaunchRecord>,
}

impl Default for LaunchHistoryFile {
    fn default() -> Self {
        Self {
            format: HISTORY_FORMAT,
            records: Vec::new(),
        }
    }
}

struct LogContext {
    history_path: PathBuf,
    archive_dir: PathBuf,
    logs_dir: PathBuf,
    crash_dir: PathBuf,
}

impl LogContext {
    fn new(instance_name: &str) -> Result<Self, LauncherError> {
        let config = config::load_config()?;
        let game_dir = PathBuf::from(&config.game_dir);
        let instance_dir = file_utils::instance_dir(&game_dir.join("versions"), instance_name)?;
        let content_root = isolation::content_root(&config, &game_dir, instance_name);
        let state_dir = instance_dir.join(".ar1s");
        Ok(Self {
            history_path: state_dir.join(HISTORY_FILE_NAME),
            archive_dir: state_dir.join(ARCHIVE_DIR_NAME),
            logs_dir: content_root.join("logs"),
            crash_dir: content_root.join("crash-reports"),
        })
    }
}

fn backup_corrupt(path: &Path, reason: &str) {
    let backup = path.with_extension("json.bak");
    let _ = fs::copy(path, &backup);
    log::warn!(
        "启动历史文件无法解析，已备份到 {} 并按空历史处理: {}",
        backup.display(),
        reason
    );
}

fn read_history_file(path: &Path) -> LaunchHistoryFile {
    if !path.exists() {
        return LaunchHistoryFile::default();
    }

    let parsed = fs::read_to_string(path)
        .map_err(|error| error.to_string())
        .and_then(|text| {
            serde_json::from_str::<LaunchHistoryFile>(&text).map_err(|error| error.to_string())
        });

    match parsed {
        Ok(file) if file.format == HISTORY_FORMAT => file,
        Ok(file) => {
            backup_corrupt(path, &format!("未知格式 {}", file.format));
            LaunchHistoryFile::default()
        }
        Err(error) => {
            backup_corrupt(path, &error);
            LaunchHistoryFile::default()
        }
    }
}

fn write_history_file(
    state_dir: &Path,
    path: &Path,
    history: &LaunchHistoryFile,
) -> Result<(), LauncherError> {
    fs::create_dir_all(state_dir)?;
    let json = serde_json::to_string_pretty(history)?;
    let mut tmp = tempfile::NamedTempFile::new_in(state_dir)?;
    tmp.write_all(json.as_bytes())?;
    tmp.flush()?;
    tmp.persist(path).map_err(|error| error.error)?;
    Ok(())
}

fn is_archive_name(name: &str) -> bool {
    name.strip_suffix(".log.gz")
        .is_some_and(|stem| !stem.is_empty() && stem.bytes().all(|byte| byte.is_ascii_digit()))
}

fn prune_records(records: &mut Vec<LaunchRecord>, archive_dir: &Path) {
    records.sort_by_key(|record| std::cmp::Reverse(record.started_at));
    records.truncate(LAUNCH_HISTORY_LIMIT);

    let keep: HashSet<String> = records
        .iter()
        .filter_map(|record| record.log_file.clone())
        .collect();

    let Ok(entries) = fs::read_dir(archive_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if is_archive_name(name)
            && !keep.contains(name)
            && let Err(error) = fs::remove_file(&path)
        {
            log::warn!("清理日志归档失败 {}: {}", path.display(), error);
        }
    }
}

pub(crate) fn record_launch_started(instance_dir: &Path, now_ms: i64) -> Option<i64> {
    let state_dir = instance_dir.join(".ar1s");
    if let Err(error) = fs::create_dir_all(&state_dir) {
        log::warn!("创建实例状态目录失败，本次不记录启动: {}", error);
        return None;
    }

    let _lock = match file_lock::acquire(&state_dir.join(HISTORY_LOCK_NAME), HISTORY_LOCK_TIMEOUT) {
        Ok(lock) => lock,
        Err(error) => {
            log::warn!("获取启动历史写锁失败，本次不记录启动: {}", error);
            return None;
        }
    };

    let history_path = state_dir.join(HISTORY_FILE_NAME);
    let mut history = read_history_file(&history_path);
    let id = history
        .records
        .iter()
        .map(|record| record.id)
        .max()
        .map_or(now_ms, |latest| latest.max(now_ms - 1) + 1);

    history.records.push(LaunchRecord {
        id,
        started_at: now_ms,
        ended_at: None,
        duration_secs: None,
        exit_code: None,
        outcome: LaunchOutcome::Running,
        log_file: None,
        crash_report: None,
    });
    prune_records(&mut history.records, &state_dir.join(ARCHIVE_DIR_NAME));

    if let Err(error) = write_history_file(&state_dir, &history_path, &history) {
        log::warn!("写入启动历史失败: {}", error);
        return None;
    }

    Some(id)
}

pub(crate) struct LaunchFinish {
    pub(crate) instance_dir: PathBuf,
    pub(crate) content_root: PathBuf,
    pub(crate) history_id: i64,
    pub(crate) started_at_ms: i64,
    pub(crate) exit_code: Option<i32>,
    pub(crate) duration_secs: u64,
    pub(crate) crash_report: Option<String>,
    pub(crate) archive_log: bool,
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

pub(crate) fn outcome_for(exit_code: Option<i32>) -> LaunchOutcome {
    match exit_code {
        Some(0) => LaunchOutcome::Exited,
        Some(_) => LaunchOutcome::Crashed,
        None => LaunchOutcome::Unknown,
    }
}

fn archive_session_log(
    content_root: &Path,
    archive_dir: &Path,
    id: i64,
    started_at_ms: i64,
) -> Result<Option<String>, LauncherError> {
    let source = content_root.join("logs").join("latest.log");
    let Ok(metadata) = fs::metadata(&source) else {
        return Ok(None);
    };

    let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
    let started_at = SystemTime::UNIX_EPOCH + Duration::from_millis(started_at_ms.max(0) as u64);
    // 文件系统 mtime 精度不一致，留 1 秒余量，避免把上一次会话的日志归档进来
    if modified + Duration::from_secs(1) < started_at {
        return Ok(None);
    }

    fs::create_dir_all(archive_dir)?;
    let file_name = format!("{}.log.gz", id);
    let target = archive_dir.join(&file_name);
    let mut reader = File::open(&source)?;
    let temp = tempfile::NamedTempFile::new_in(archive_dir)?;
    {
        let mut encoder = GzEncoder::new(temp.as_file(), Compression::default());
        io::copy(&mut reader, &mut encoder)?;
        encoder.finish()?;
    }
    temp.persist(&target).map_err(|error| error.error)?;
    Ok(Some(file_name))
}

pub(crate) fn record_launch_finished(finish: LaunchFinish) {
    let state_dir = finish.instance_dir.join(".ar1s");
    if let Err(error) = fs::create_dir_all(&state_dir) {
        log::warn!("创建实例状态目录失败，本次退出未记录: {}", error);
        return;
    }

    let archive_dir = state_dir.join(ARCHIVE_DIR_NAME);
    let _lock = match file_lock::acquire(&state_dir.join(HISTORY_LOCK_NAME), HISTORY_LOCK_TIMEOUT) {
        Ok(lock) => lock,
        Err(error) => {
            log::warn!("获取启动历史写锁失败，本次退出未记录: {}", error);
            return;
        }
    };

    let history_path = state_dir.join(HISTORY_FILE_NAME);
    let mut history = read_history_file(&history_path);
    let Some(record) = history
        .records
        .iter_mut()
        .find(|record| record.id == finish.history_id)
    else {
        log::warn!(
            "启动历史里找不到本次记录 {}，跳过退出收尾",
            finish.history_id
        );
        return;
    };

    if finish.archive_log {
        match archive_session_log(
            &finish.content_root,
            &archive_dir,
            finish.history_id,
            finish.started_at_ms,
        ) {
            Ok(Some(name)) => record.log_file = Some(name),
            Ok(None) => {}
            Err(error) => log::warn!("归档本次游戏日志失败: {}", error),
        }
    }

    record.ended_at = Some(now_millis());
    record.duration_secs = Some(finish.duration_secs);
    record.exit_code = finish.exit_code;
    record.outcome = outcome_for(finish.exit_code);
    record.crash_report = finish.crash_report;

    prune_records(&mut history.records, &archive_dir);
    if let Err(error) = write_history_file(&state_dir, &history_path, &history) {
        log::warn!("写入启动历史失败: {}", error);
    }
}

pub fn list_history(instance_name: &str) -> Result<Vec<LaunchRecord>, LauncherError> {
    let context = LogContext::new(instance_name)?;
    let mut history = read_history_file(&context.history_path);
    history
        .records
        .sort_by_key(|record| std::cmp::Reverse(record.started_at));
    history.records.truncate(LAUNCH_HISTORY_LIMIT);
    Ok(history.records)
}

fn truncate_line(line: &str) -> String {
    if line.chars().count() <= LOG_LINE_MAX_CHARS {
        return line.to_string();
    }
    let mut shortened: String = line.chars().take(LOG_LINE_MAX_CHARS).collect();
    shortened.push('…');
    shortened
}

fn read_plain_tail(
    path: &Path,
    max_lines: usize,
    max_bytes: u64,
) -> io::Result<(Vec<String>, bool)> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    let start = len.saturating_sub(max_bytes);
    let mut bytes = Vec::with_capacity((len - start) as usize);
    file.seek(SeekFrom::Start(start))?;
    file.read_to_end(&mut bytes)?;

    let mut truncated = start > 0;
    if start > 0 {
        if let Some(position) = bytes.iter().position(|byte| *byte == b'\n') {
            bytes.drain(..=position);
        } else {
            bytes.clear();
        }
    }

    let text = String::from_utf8_lossy(&bytes);
    let mut lines: Vec<String> = text
        .lines()
        .map(|line| truncate_line(line.trim_end_matches('\r')))
        .collect();
    if lines.len() > max_lines {
        lines.drain(..lines.len() - max_lines);
        truncated = true;
    }
    Ok((lines, truncated))
}

fn read_gzip_tail(path: &Path, max_lines: usize) -> io::Result<(Vec<String>, bool)> {
    let file = File::open(path)?;
    let mut decoder = io::BufReader::new(GzDecoder::new(file));
    let mut ring: VecDeque<String> = VecDeque::with_capacity(max_lines);
    let mut buffer = Vec::new();
    let mut total = 0usize;

    loop {
        buffer.clear();
        let read = decoder.read_until(b'\n', &mut buffer)?;
        if read == 0 {
            break;
        }
        total += 1;
        if buffer.ends_with(b"\n") {
            buffer.pop();
        }
        if buffer.ends_with(b"\r") {
            buffer.pop();
        }
        let line = String::from_utf8_lossy(&buffer);
        if ring.len() == max_lines {
            ring.pop_front();
        }
        ring.push_back(truncate_line(&line));
    }

    Ok((ring.into_iter().collect(), total > max_lines))
}

pub fn read_latest_log(instance_name: &str) -> Result<LogTail, LauncherError> {
    let context = LogContext::new(instance_name)?;
    let path = context.logs_dir.join("latest.log");
    if !path.exists() {
        return Ok(LogTail {
            lines: Vec::new(),
            truncated: false,
            path: path.to_string_lossy().to_string(),
        });
    }

    let (lines, truncated) = read_plain_tail(&path, LOG_TAIL_LINES, LOG_TAIL_BYTES)?;
    Ok(LogTail {
        lines,
        truncated,
        path: path.to_string_lossy().to_string(),
    })
}

pub fn read_launch_log(instance_name: &str, launch_id: i64) -> Result<LogTail, LauncherError> {
    let context = LogContext::new(instance_name)?;
    let history = read_history_file(&context.history_path);
    let record = history
        .records
        .iter()
        .find(|record| record.id == launch_id)
        .ok_or_else(|| LauncherError::Custom("找不到这次启动记录".to_string()))?;
    let Some(file_name) = record.log_file.as_deref() else {
        return Err(LauncherError::Custom("这次启动没有日志归档".to_string()));
    };
    if !is_archive_name(file_name) {
        return Err(LauncherError::Custom("日志归档文件名无效".to_string()));
    }

    let path = context.archive_dir.join(file_name);
    if !path.is_file() {
        return Err(LauncherError::Custom("日志归档文件已丢失".to_string()));
    }

    let (lines, truncated) = read_gzip_tail(&path, LOG_TAIL_LINES)?;
    Ok(LogTail {
        lines,
        truncated,
        path: path.to_string_lossy().to_string(),
    })
}

fn is_crash_report_name(name: &str) -> bool {
    name.starts_with("crash-")
        && name.ends_with(".txt")
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains(':')
        && !name.contains("..")
}

fn system_time_millis(time: SystemTime) -> i64 {
    time.duration_since(SystemTime::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

fn list_crash_reports_in(dir: &Path) -> Vec<CrashReportInfo> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };

    let mut reports: Vec<CrashReportInfo> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let file_name = path.file_name()?.to_str()?.to_string();
            if !is_crash_report_name(&file_name) {
                return None;
            }
            let metadata = entry.metadata().ok()?;
            if !metadata.is_file() {
                return None;
            }
            Some(CrashReportInfo {
                file_name,
                size: metadata.len(),
                modified_at: system_time_millis(metadata.modified().ok()?),
            })
        })
        .collect();

    reports.sort_by_key(|report| std::cmp::Reverse(report.modified_at));
    reports.truncate(CRASH_REPORT_LIMIT);
    reports
}

pub fn list_crash_reports(instance_name: &str) -> Result<Vec<CrashReportInfo>, LauncherError> {
    let context = LogContext::new(instance_name)?;
    Ok(list_crash_reports_in(&context.crash_dir))
}

fn decode_crash_text(path: &Path, file_name: &str) -> Result<CrashReportText, LauncherError> {
    let file = File::open(path)?;
    let mut bytes = Vec::new();
    file.take(CRASH_TEXT_BYTES + 1).read_to_end(&mut bytes)?;
    let truncated = bytes.len() as u64 > CRASH_TEXT_BYTES;
    if truncated {
        bytes.truncate(CRASH_TEXT_BYTES as usize);
    }

    Ok(CrashReportText {
        file_name: file_name.to_string(),
        content: String::from_utf8_lossy(&bytes).to_string(),
        truncated,
    })
}

pub fn read_crash_report(
    instance_name: &str,
    file_name: &str,
) -> Result<CrashReportText, LauncherError> {
    if !is_crash_report_name(file_name) {
        return Err(LauncherError::Custom("崩溃报告文件名无效".to_string()));
    }
    file_utils::validate_file_name_or_error(file_name)?;

    let context = LogContext::new(instance_name)?;
    decode_crash_text(&context.crash_dir.join(file_name), file_name)
}

pub fn open_content(instance_name: &str, target: ContentTarget) -> Result<(), LauncherError> {
    let context = LogContext::new(instance_name)?;
    let path = match target {
        ContentTarget::LatestLog => {
            let path = context.logs_dir.join("latest.log");
            if !path.is_file() {
                return Err(LauncherError::Custom(
                    "还没有游戏日志，先启动一次游戏".to_string(),
                ));
            }
            path
        }
        ContentTarget::LogsFolder => {
            fs::create_dir_all(&context.logs_dir)?;
            context.logs_dir
        }
        ContentTarget::CrashReportsFolder => {
            fs::create_dir_all(&context.crash_dir)?;
            context.crash_dir
        }
    };

    opener::open(&path)
        .map_err(|error| LauncherError::Custom(format!("无法打开 {}: {}", path.display(), error)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{LAUNCH_HISTORY_LIMIT, LaunchOutcome};
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use std::io::Write;
    use tempfile::TempDir;

    fn instance_dir(dir: &TempDir) -> std::path::PathBuf {
        dir.path().join("versions").join("Test Instance")
    }

    fn read_records(dir: &TempDir) -> Vec<crate::models::LaunchRecord> {
        read_history_file(&instance_dir(dir).join(".ar1s").join(HISTORY_FILE_NAME)).records
    }

    fn sample_instance(dir: &TempDir) -> (std::path::PathBuf, std::path::PathBuf) {
        let instance_dir = dir.path().join("versions").join("Test Instance");
        let content_root = instance_dir.clone();
        std::fs::create_dir_all(content_root.join("logs")).unwrap();
        (instance_dir, content_root)
    }

    #[test]
    fn start_writes_running_record() {
        let dir = TempDir::new().unwrap();
        let id = record_launch_started(&instance_dir(&dir), 1_760_000_000_000).unwrap();

        let records = read_records(&dir);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].id, id);
        assert_eq!(records[0].started_at, 1_760_000_000_000);
        assert_eq!(records[0].outcome, LaunchOutcome::Running);
        assert!(records[0].ended_at.is_none());
        assert!(records[0].log_file.is_none());
    }

    #[test]
    fn record_ids_stay_unique_when_clock_stands_still() {
        let dir = TempDir::new().unwrap();
        let first = record_launch_started(&instance_dir(&dir), 1_760_000_000_000).unwrap();
        let second = record_launch_started(&instance_dir(&dir), 1_760_000_000_000).unwrap();

        assert!(second > first);
    }

    #[test]
    fn history_is_pruned_to_limit_and_orphan_archives_are_removed() {
        let dir = TempDir::new().unwrap();
        let instance = instance_dir(&dir);
        let first_id = record_launch_started(&instance, 1_760_000_000_000).unwrap();

        let archive_dir = instance.join(".ar1s").join(ARCHIVE_DIR_NAME);
        std::fs::create_dir_all(&archive_dir).unwrap();
        let orphan = archive_dir.join(format!("{}.log.gz", first_id));
        std::fs::write(&orphan, b"x").unwrap();

        for index in 1..LAUNCH_HISTORY_LIMIT as i64 + 1 {
            record_launch_started(&instance, 1_760_000_000_000 + index).unwrap();
        }

        let records = read_records(&dir);
        assert_eq!(records.len(), LAUNCH_HISTORY_LIMIT);
        assert!(records.iter().all(|record| record.id != first_id));
        assert!(!orphan.exists());
    }

    #[test]
    fn corrupt_history_is_backed_up_and_treated_as_empty() {
        let dir = TempDir::new().unwrap();
        let instance = instance_dir(&dir);
        let state = instance.join(".ar1s");
        std::fs::create_dir_all(&state).unwrap();
        let path = state.join(HISTORY_FILE_NAME);
        std::fs::write(&path, b"not json").unwrap();

        let history = read_history_file(&path);
        assert!(history.records.is_empty());
        assert!(state.join("launch-history.json.bak").exists());
    }

    #[test]
    fn finish_writes_outcome_and_archives_log() {
        let dir = TempDir::new().unwrap();
        let (instance_dir, content_root) = sample_instance(&dir);
        std::fs::write(
            content_root.join("logs").join("latest.log"),
            "hello\nworld\n",
        )
        .unwrap();

        let started = now_millis() - 5_000;
        let id = record_launch_started(&instance_dir, started).unwrap();
        record_launch_finished(LaunchFinish {
            instance_dir: instance_dir.clone(),
            content_root: content_root.clone(),
            history_id: id,
            started_at_ms: started,
            exit_code: Some(1),
            duration_secs: 5,
            crash_report: Some("crash-1.txt".to_string()),
            archive_log: true,
        });

        let history = read_history_file(&instance_dir.join(".ar1s").join(HISTORY_FILE_NAME));
        let record = &history.records[0];
        assert_eq!(record.outcome, LaunchOutcome::Crashed);
        assert_eq!(record.exit_code, Some(1));
        assert_eq!(record.duration_secs, Some(5));
        assert_eq!(record.crash_report.as_deref(), Some("crash-1.txt"));

        let log_file = record.log_file.clone().expect("应当归档日志");
        assert!(is_archive_name(&log_file));
        let archive = instance_dir
            .join(".ar1s")
            .join(ARCHIVE_DIR_NAME)
            .join(&log_file);
        assert!(archive.is_file());
        let (lines, truncated) = read_gzip_tail(&archive, 5000).unwrap();
        assert_eq!(lines, vec!["hello", "world"]);
        assert!(!truncated);
    }

    #[test]
    fn finish_skips_archive_when_latest_log_is_stale() {
        let dir = TempDir::new().unwrap();
        let (instance_dir, content_root) = sample_instance(&dir);
        std::fs::write(content_root.join("logs").join("latest.log"), "old\n").unwrap();

        let started = now_millis() + 3_600_000;
        let id = record_launch_started(&instance_dir, started).unwrap();
        record_launch_finished(LaunchFinish {
            instance_dir: instance_dir.clone(),
            content_root,
            history_id: id,
            started_at_ms: started,
            exit_code: Some(0),
            duration_secs: 1,
            crash_report: None,
            archive_log: true,
        });

        let history = read_history_file(&instance_dir.join(".ar1s").join(HISTORY_FILE_NAME));
        assert!(history.records[0].log_file.is_none());
    }

    #[test]
    fn plain_tail_keeps_last_lines_and_caps_long_lines() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("latest.log");
        let content: String = (0..6000).map(|index| format!("line {}\n", index)).collect();
        std::fs::write(&path, content).unwrap();

        let (lines, truncated) = read_plain_tail(&path, 5000, 2 * 1024 * 1024).unwrap();
        assert_eq!(lines.len(), 5000);
        assert_eq!(lines.first().unwrap(), "line 1000");
        assert!(truncated);

        let long_line = "x".repeat(LOG_LINE_MAX_CHARS + 10);
        std::fs::write(&path, format!("{}\n", long_line)).unwrap();
        let (lines, _) = read_plain_tail(&path, 10, 2 * 1024 * 1024).unwrap();
        assert_eq!(lines[0].chars().count(), LOG_LINE_MAX_CHARS + 1);
        assert!(lines[0].ends_with('…'));
    }

    #[test]
    fn plain_tail_respects_byte_cap() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("latest.log");
        std::fs::write(&path, "line 0\nline 1\nline 2\n").unwrap();

        let (lines, truncated) = read_plain_tail(&path, 10, 12).unwrap();
        assert!(truncated);
        assert_eq!(lines, vec!["line 2"]);
    }

    #[test]
    fn gzip_tail_returns_last_lines() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("session.log.gz");
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        for index in 0..10 {
            writeln!(encoder, "line {}", index).unwrap();
        }
        std::fs::write(&path, encoder.finish().unwrap()).unwrap();

        let (lines, truncated) = read_gzip_tail(&path, 5).unwrap();
        assert_eq!(
            lines,
            vec!["line 5", "line 6", "line 7", "line 8", "line 9"]
        );
        assert!(truncated);
    }

    #[test]
    fn corrupt_gzip_returns_error() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("broken.log.gz");
        std::fs::write(&path, b"not gzip").unwrap();

        assert!(read_gzip_tail(&path, 10).is_err());
    }

    #[test]
    fn crash_reports_only_accept_crash_txt() {
        let dir = TempDir::new().unwrap();
        let crash_dir = dir.path().join("crash-reports");
        std::fs::create_dir_all(&crash_dir).unwrap();
        std::fs::write(crash_dir.join("crash-a.txt"), "a").unwrap();
        std::fs::write(crash_dir.join("crash-b.txt"), "b").unwrap();
        std::fs::write(crash_dir.join("notes.txt"), "n").unwrap();

        let reports = list_crash_reports_in(&crash_dir);
        assert_eq!(reports.len(), 2);
        assert!(
            reports
                .iter()
                .all(|report| is_crash_report_name(&report.file_name))
        );

        assert!(!is_crash_report_name("../crash-a.txt"));
        assert!(!is_crash_report_name("crash-a.exe"));
        assert!(is_crash_report_name("crash-2026-10-24_12.00.00-client.txt"));
    }

    #[test]
    fn crash_text_is_capped() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("crash-big.txt");
        std::fs::write(&path, vec![b'a'; CRASH_TEXT_BYTES as usize + 10]).unwrap();

        let text = decode_crash_text(&path, "crash-big.txt").unwrap();
        assert!(text.truncated);
        assert_eq!(text.content.len(), CRASH_TEXT_BYTES as usize);
    }

    #[test]
    fn outcome_for_maps_exit_codes() {
        assert_eq!(outcome_for(Some(0)), LaunchOutcome::Exited);
        assert_eq!(outcome_for(Some(1)), LaunchOutcome::Crashed);
        assert_eq!(outcome_for(None), LaunchOutcome::Unknown);
    }
}
