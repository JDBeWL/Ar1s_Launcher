//! 错误类型模块
//!
//! 唯一的 LauncherError 枚举与各底层错误的 From 转换，供 `?` 冒泡到 command 返回值。
//! `code()` 是给前端做控制流判断的稳定标识；`message` 只用于展示

use std::io;
use thiserror::Error;
use tokio::task::JoinError;

#[derive(Error, Debug)]
pub enum LauncherError {
    #[error("IO 错误: {0}")]
    Io(#[from] io::Error),
    #[error("网络请求错误: {0}")]
    Http(#[from] reqwest::Error),
    #[error("JSON 解析错误: {0}")]
    Json(#[from] serde_json::Error),
    #[error("压缩包错误: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("Tauri 错误: {0}")]
    Tauri(#[from] tauri::Error),
    #[error("操作已取消")]
    Cancelled,
    #[error("另一个启动器实例正在占用 {0}")]
    Busy(String),
    #[error("{0}")]
    /// 用户在 OAuth 流程里拒绝了授权；前端据此静默，不当错误弹窗
    AuthDenied(String),
    #[error("{0}")]
    /// 设备代码等授权凭据过期；前端据此静默并引导重新发起登录
    AuthExpired(String),
    #[error("{0}")]
    Custom(String),
}

impl LauncherError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Io(_) => "io",
            Self::Http(_) => "http",
            Self::Json(_) => "json",
            Self::Zip(_) => "zip",
            Self::Tauri(_) => "tauri",
            Self::Cancelled => "cancelled",
            Self::Busy(_) => "busy",
            Self::AuthDenied(_) => "auth_denied",
            Self::AuthExpired(_) => "auth_expired",
            Self::Custom(_) => "custom",
        }
    }

    pub fn is_cancelled(&self) -> bool {
        matches!(self, Self::Cancelled)
    }
}

impl serde::Serialize for LauncherError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::ser::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("LauncherError", 2)?;
        state.serialize_field("code", self.code())?;
        state.serialize_field("message", &self.to_string())?;
        state.end()
    }
}

impl From<JoinError> for LauncherError {
    fn from(err: JoinError) -> Self {
        LauncherError::Custom(format!("任务执行错误: {}", err))
    }
}

#[cfg(test)]
mod tests {
    use super::LauncherError;

    #[test]
    fn test_error_serializes_stable_code_and_message() {
        let cancelled = serde_json::to_value(LauncherError::Cancelled).unwrap();
        assert_eq!(cancelled["code"], "cancelled");
        assert_eq!(cancelled["message"], "操作已取消");
        assert!(LauncherError::Cancelled.is_cancelled());

        let custom = serde_json::to_value(LauncherError::Custom("出错了".to_string())).unwrap();
        assert_eq!(custom["code"], "custom");
        assert_eq!(custom["message"], "出错了");

        assert_eq!(LauncherError::Busy("下载锁".to_string()).code(), "busy");
    }
}
