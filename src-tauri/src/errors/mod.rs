//! 错误类型模块
//!
//! 唯一的 LauncherError 枚举与各底层错误的 From 转换，供 `?` 冒泡到 command 返回值。
//! `code()` 是给前端做控制流判断与本地化的稳定标识；`message` 只用于日志和兜底展示

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
    /// 需要前端本地化的错误
    ///
    /// `code` 是稳定标识，前端按 `errors.backend.<code>` 查表并用 `params` 按位置填充；
    /// `message` 是不查表时的中文兜底，同时保证日志里能看懂发生了什么。
    #[error("{message}")]
    Localized {
        code: &'static str,
        params: Vec<String>,
        message: String,
    },
}

impl LauncherError {
    /// 构造一条可本地化错误。`params` 的顺序必须和文案里的 `{0}`、`{1}` 对应
    pub fn localized(code: &'static str, message: impl Into<String>, params: Vec<String>) -> Self {
        Self::Localized {
            code,
            params,
            message: message.into(),
        }
    }

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
            Self::Localized { code, .. } => code,
        }
    }

    /// 前端本地化文案的填充参数；粗粒度包装错误把内层原因作为唯一参数
    pub fn params(&self) -> Vec<String> {
        match self {
            Self::Io(error) => vec![error.to_string()],
            Self::Http(error) => vec![error.to_string()],
            Self::Json(error) => vec![error.to_string()],
            Self::Zip(error) => vec![error.to_string()],
            Self::Tauri(error) => vec![error.to_string()],
            Self::Busy(resource) => vec![resource.clone()],
            Self::Localized { params, .. } => params.clone(),
            Self::Cancelled | Self::AuthDenied(_) | Self::AuthExpired(_) | Self::Custom(_) => {
                Vec::new()
            }
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
        let mut state = serializer.serialize_struct("LauncherError", 3)?;
        state.serialize_field("code", self.code())?;
        state.serialize_field("message", &self.to_string())?;
        state.serialize_field("params", &self.params())?;
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

    #[test]
    fn test_wrapper_errors_expose_inner_reason_as_param() {
        let busy = LauncherError::Busy("下载锁".to_string());
        assert_eq!(busy.params(), vec!["下载锁".to_string()]);

        let io = LauncherError::Io(std::io::Error::other("磁盘只读"));
        assert_eq!(io.params(), vec!["磁盘只读".to_string()]);

        // 包装类错误的 message 带中文前缀，前端只用 params 里的内层原因
        assert!(io.to_string().contains("磁盘只读"));
    }

    #[test]
    fn test_localized_error_keeps_code_params_and_fallback() {
        let error = LauncherError::localized(
            "disk_insufficient",
            "磁盘空间不足：需要约 1.0 GB，可用 0.2 GB",
            vec!["1.0 GB".to_string(), "0.2 GB".to_string()],
        );

        assert_eq!(error.code(), "disk_insufficient");
        assert_eq!(error.params(), vec!["1.0 GB", "0.2 GB"]);
        assert_eq!(
            error.to_string(),
            "磁盘空间不足：需要约 1.0 GB，可用 0.2 GB"
        );

        let json = serde_json::to_value(&error).unwrap();
        assert_eq!(json["code"], "disk_insufficient");
        assert_eq!(json["params"][1], "0.2 GB");
    }
}
