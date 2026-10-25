//! 日志中心数据模型

use serde::{Deserialize, Serialize};

pub const LAUNCH_HISTORY_LIMIT: usize = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LaunchOutcome {
    Running,
    Exited,
    Crashed,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchRecord {
    pub id: i64,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub duration_secs: Option<u64>,
    pub exit_code: Option<i32>,
    pub outcome: LaunchOutcome,
    pub log_file: Option<String>,
    pub crash_report: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogTail {
    pub lines: Vec<String>,
    pub truncated: bool,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CrashReportInfo {
    pub file_name: String,
    pub size: u64,
    pub modified_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CrashReportText {
    pub file_name: String,
    pub content: String,
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ContentTarget {
    LatestLog,
    LogsFolder,
    CrashReportsFolder,
}
