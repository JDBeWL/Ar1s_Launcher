use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ManagedJavaState {
    NotInstalled,
    Installed,
    Broken,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JavaDownloadOption {
    pub feature: u32,
    pub release_name: String,
    pub package_name: String,
    pub size: u64,
    pub state: ManagedJavaState,
    pub installed_version: Option<String>,
    /// `<root>/temurin-{n}/bin/java.exe`
    pub installed_path: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum JavaDownloadPhase {
    Downloading,
    Extracting,
    Completed,
    Cancelled,
    Error,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JavaDownloadProgress {
    pub feature: u32,
    pub phase: JavaDownloadPhase,
    pub bytes_downloaded: u64,
    pub total_bytes: u64,
    /// KB/s，与现有下载进度一致
    pub speed: f64,
    pub percent: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
