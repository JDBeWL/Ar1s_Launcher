//! 实例截图数据模型

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenshotInfo {
    pub file_name: String,
    pub path: String,
    pub captured_at: i64,
    pub size: u64,
    pub width: u32,
    pub height: u32,
}
