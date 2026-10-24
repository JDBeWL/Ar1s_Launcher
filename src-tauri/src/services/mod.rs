//! 服务层模块声明
//!
//! 挂出配置、下载、实例、Java、内存、启动、加载器与正版登录等业务子模块

pub mod config;
pub mod cf_modpack_format;
pub mod curseforge;
pub mod diagnostics;
pub mod download;
pub mod file_verification;
pub mod http_client;
pub mod instance;
pub mod java;
pub mod launcher;
pub mod loaders;
pub mod memory;
pub mod microsoft_auth;
pub mod mod_installer;
pub mod modpack_exporter;
pub mod modpack_format;
pub mod modpack_installer;
pub mod modrinth;
pub mod proxy;
pub mod resourcepack;
pub mod resourcepack_installer;
pub mod shaderpack;
pub mod shaderpack_installer;
