//! 存档与备份模型

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SaveInfo {
    pub folder: String,
    pub display_name: String,
    pub last_played: Option<i64>,
    pub size: u64,
    pub icon_data_url: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SaveBackupKind {
    Manual,
    Auto,
    PreRestore,
}

impl SaveBackupKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Auto => "auto",
            Self::PreRestore => "preRestore",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SaveBackupInfo {
    pub world: String,
    pub file_name: String,
    pub display_name: String,
    pub created_at: i64,
    pub size: u64,
    pub kind: SaveBackupKind,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SaveImportSummary {
    pub imported: Vec<String>,
    pub skipped: Vec<String>,
}
