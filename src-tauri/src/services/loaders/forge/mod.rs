//! Forge / NeoForge 安装
//!
//! 新版本走官方 installer jar（跑 processors），老版本走手工拼库 + 改版本 JSON。
//! 拆开的几块互相只通过 `libraries_dir` / 版本号这类数据打交道：
//! - [`mirror`]：下载镜像与候选 URL 排序
//! - [`versions`]：版本目录识别与版本号比较
//! - [`java`]：安装器/processor 需要哪个 Java
//! - [`fetch`]：带重试、校验与镜像回退的库下载
//! - [`official`]：跑官方 installer 进程
//! - [`legacy`] / [`modern`]：两条手工安装路径
//! - [`processors`]：新版 Forge 的 processor 执行器

mod fetch;
mod java;
mod legacy;
mod mirror;
mod modern;
mod official;
mod processors;
mod versions;

#[cfg(test)]
mod tests;

// 子模块之间互相调用不写长路径：统一在这里重导出，各自 `use super::*` 即可
pub(crate) use fetch::*;
pub(crate) use java::*;
pub(crate) use legacy::*;
pub(crate) use mirror::*;
pub(crate) use modern::*;
pub(crate) use official::*;
pub(crate) use processors::*;
pub(crate) use versions::*;

use crate::errors::LauncherError;
use crate::services::config;
use crate::services::http_client;
use log::{info, warn};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

const INSTALLER_TIMEOUT: Duration = Duration::from_secs(600);
const PROCESSOR_TIMEOUT: Duration = Duration::from_secs(300);

const BMCL_API_BASE_URL: &str = "https://bmclapi2.bangbang93.com";
const MAVEN_FORGE: &str = "https://maven.minecraftforge.net";
const MAVEN_CENTRAL: &str = "https://repo1.maven.org/maven2";
const MAVEN_MINECRAFT: &str = "https://libraries.minecraft.net";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForgeVersion {
    pub version: String,
    pub mcversion: String,
    #[serde(default)]
    pub build: i32,
}

pub async fn install_forge(
    mc_version: &str,
    forge_version: &str,
    instance_name: &str,
    game_dir: &Path,
    cancel: Option<&AtomicBool>,
) -> Result<(), LauncherError> {
    check_cancelled(cancel)?;
    info!(
        "安装 Forge: MC {} + Forge {} -> {}",
        mc_version, forge_version, instance_name
    );

    // Forge/NeoForge 直接写共享的 libraries/，和版本下载共用一把游戏目录锁
    let _download_lock = crate::utils::file_lock::acquire_async(
        &crate::services::instance::lock_dir(game_dir).join("download.lock"),
        Duration::from_secs(2),
    )
    .await?;

    let app_config = config::load_config()?;
    // 没在设置里指定 Java 也不该拦住安装：候选集里通常已经有能跑安装器的那份
    let java_path = app_config
        .java_path
        .clone()
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| "java".to_string());
    let java_candidates = crate::services::launcher::java::collect_candidates(&app_config);

    let forge_ver = ForgeVersion {
        version: forge_version.to_string(),
        mcversion: mc_version.to_string(),
        build: 0,
    };

    let installer_temp = download_forge_installer(&forge_ver).await?;
    let installer_path = installer_temp.as_ref();

    // 旧版安装器不会自动取这三个库，需预先落入 libraries
    if !is_new_forge(mc_version) {
        let libs_dir = game_dir.join("libraries");
        for result in [
            download_launchwrapper_library(&libs_dir, mc_version).await,
            download_asm_library(&libs_dir, mc_version).await,
            download_lzma_library(&libs_dir, mc_version).await,
        ] {
            if let Err(e) = result {
                warn!(
                    "Forge: 预置库没下来（还能继续，缺件会在安装结束时一并报）: {}",
                    e
                );
            }
        }
    }

    // 官方安装器缺这个文件会直接失败，先造个空骨架
    let profiles_path = game_dir.join("launcher_profiles.json");
    if !profiles_path.exists() {
        fs::write(&profiles_path, r#"{"profiles":{}}"#).ok();
    }

    let versions_dir = game_dir.join("versions");
    // 安装器写出的目录名不一定等于我们拼的 id（1.7.10 用的是 `1.7.10-Forge10.13.4.1614`），
    // 靠差集去认才不会漏
    let versions_before = snapshot_version_dirs(&versions_dir);

    // 安装器自己也是 jar，用跑不动它的 Java 去启动只是白等一轮
    let installer_java = choose_installer_java(installer_path, mc_version, &java_candidates);
    let install_result = match installer_java {
        Some(ref java) => {
            check_cancelled(cancel)?;
            info!("Forge: 尝试官方安装器（Java {}）", java);
            run_official_installer(installer_path, game_dir, java, cancel).await
        }
        None => Err(LauncherError::Custom(
            "本机没有能运行安装器的 Java".to_string(),
        )),
    };

    let forge_version_id = get_forge_version_id(mc_version, forge_version);

    match install_result {
        Ok(()) => {
            info!("Forge: 官方安装器成功");
        }
        Err(e) => {
            // 用户取消不是安装器失败，不能再转手动安装继续跑
            check_cancelled(cancel)?;
            warn!("Forge: 官方安装器失败: {}, 尝试手动安装", e);

            if is_new_forge(mc_version) {
                manual_install_new_forge(
                    installer_path,
                    game_dir,
                    &forge_ver,
                    &java_path,
                    &java_candidates,
                    cancel,
                )
                .await?;
            } else {
                check_cancelled(cancel)?;
                manual_install_old_forge(installer_path, game_dir, &forge_ver).await?;
            }
        }
    }

    drop(installer_temp);

    // 装完必须能在 versions 里找到加载器 JSON，否则实例会安静地退化成原版
    let installed_id = find_installed_loader_dir(
        &versions_dir,
        &versions_before,
        &forge_version_id,
        mc_version,
    )
    .ok_or_else(|| {
        LauncherError::Custom(format!(
            "Forge 安装器跑完了，但 versions 目录里没有它写出的版本 JSON（预期 {}）。这次安装没有生效，实例仍是原版。",
            forge_version_id
        ))
    })?;

    let loader_dir = versions_dir.join(&installed_id);
    let instance_dir = versions_dir.join(instance_name);

    if loader_dir.exists() && loader_dir != instance_dir {
        let loader_json_path = loader_dir.join(format!("{}.json", installed_id));
        if loader_json_path.exists() {
            let content = fs::read_to_string(&loader_json_path)?;
            let mut json: Value = serde_json::from_str(&content)?;

            if let Some(obj) = json.as_object_mut() {
                obj.insert("id".to_string(), serde_json::json!(instance_name));
            }

            fs::create_dir_all(&instance_dir)?;
            let instance_json_path = instance_dir.join(format!("{}.json", instance_name));
            fs::write(&instance_json_path, serde_json::to_string_pretty(&json)?)?;

            let _ = fs::remove_dir_all(&loader_dir);
        }
    }

    info!("Forge: 安装完成");
    Ok(())
}

pub async fn get_forge_versions(mc_version: &str) -> Result<Vec<ForgeVersion>, LauncherError> {
    let client = http_client::get_client();
    let url = format!("{}/forge/minecraft/{}", BMCL_API_BASE_URL, mc_version);

    info!("Forge: 获取版本列表: {}", url);
    let response = client.get(&url).send().await?;

    if !response.status().is_success() {
        return Err(LauncherError::Custom(format!(
            "获取 Forge 版本失败: {}",
            response.status()
        )));
    }

    let mut versions: Vec<ForgeVersion> = response.json().await?;

    // 最新版本在前
    versions.sort_by(|a, b| compare_forge_versions(&b.version, &a.version));

    Ok(versions)
}
