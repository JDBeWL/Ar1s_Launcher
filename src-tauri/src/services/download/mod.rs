//! 下载模块
//!
//! 组装版本清单里的 jar、资源与库文件任务，并发下载、续传、校验并上报进度

pub mod batch;
mod file;
mod http;
mod manifest;
pub mod source;
mod state;
mod version;

pub use batch::download_all_files;
pub use file::{download_file, download_file_single};
pub(crate) use http::reload as reload_http_client;
pub use http::{get_http_client, get_java_download_client};
pub use manifest::get_versions;
pub use version::ensure_declared_libraries;
pub use version::ensure_version_core;
pub use version::process_and_download_version;
