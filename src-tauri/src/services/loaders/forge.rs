//! Forge 加载器安装
//!
//! 先跑官方安装器，失败时按 MC 1.13 分界转手动安装，最后改写 id 落到实例版本目录

use crate::errors::LauncherError;
use crate::services::config;
use crate::services::download;
use crate::services::download::source::{self as download_sources, DownloadSource};
use crate::services::http_client;
use crate::services::launcher::java::JavaCandidate;
use crate::utils::file_utils;
use log::{debug, info, warn};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::process::Command;
use zip::ZipArchive;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

const INSTALLER_TIMEOUT: Duration = Duration::from_secs(600);
const PROCESSOR_TIMEOUT: Duration = Duration::from_secs(300);

const BMCL_API_BASE_URL: &str = "https://bmclapi2.bangbang93.com";
const MAVEN_FORGE: &str = "https://maven.minecraftforge.net";
const MAVEN_CENTRAL: &str = "https://repo1.maven.org/maven2";
const MAVEN_MINECRAFT: &str = "https://libraries.minecraft.net";

/// 官方 maven 地址 → 镜像内路径
///
/// 只映射镜像确实覆盖了的域名；镜像统一用 `/maven/{path}`（BMCL 与 MC 光速都支持），
/// 不区分 Mojang 库与 Forge/NeoForge 库。
fn maven_mirror_path(url: &str) -> Option<String> {
    let path = [
        MAVEN_MINECRAFT,
        MAVEN_FORGE,
        "https://maven.neoforged.net/releases",
        MAVEN_CENTRAL,
    ]
    .iter()
    .find_map(|official| {
        url.strip_prefix(official)
            .map(|rest| rest.trim_start_matches('/').to_string())
    })?;

    Some(format!("/maven/{path}"))
}

/// 官方 maven 地址在当前下载源下的候选镜像地址
fn mirror_urls(url: &str, source: DownloadSource) -> Vec<String> {
    if let Some(path) = download_sources::mirror_path(url) {
        return download_sources::build_mirror_urls(source, &path);
    }

    maven_mirror_path(url)
        .map(|path| download_sources::build_mirror_urls(source, &path))
        .unwrap_or_default()
}

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

fn snapshot_version_dirs(versions_dir: &Path) -> HashSet<String> {
    fs::read_dir(versions_dir)
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| e.path().is_dir())
                .map(|e| e.file_name().to_string_lossy().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// 先认预期 id，再认“安装后新出现的、以本版本为父的 Forge 目录”
fn find_installed_loader_dir(
    versions_dir: &Path,
    before: &HashSet<String>,
    expected_id: &str,
    mc_version: &str,
) -> Option<String> {
    let expected_json = versions_dir
        .join(expected_id)
        .join(format!("{}.json", expected_id));
    if expected_json.exists() {
        return Some(expected_id.to_string());
    }

    let entries = fs::read_dir(versions_dir).ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if before.contains(&name) {
            continue;
        }

        let Ok(content) = fs::read_to_string(entry.path().join(format!("{}.json", name))) else {
            continue;
        };
        let Ok(json) = serde_json::from_str::<Value>(&content) else {
            continue;
        };

        let same_base = json["inheritsFrom"].as_str() == Some(mc_version)
            || json["id"]
                .as_str()
                .is_some_and(|id| id.starts_with(mc_version));
        let is_forge = json["libraries"].as_array().is_some_and(|libs| {
            libs.iter().any(|lib| {
                lib["name"]
                    .as_str()
                    .is_some_and(|name| name.starts_with("net.minecraftforge"))
            })
        });

        if same_base && is_forge {
            return Some(name);
        }
    }

    None
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

fn is_new_forge(mc_version: &str) -> bool {
    crate::utils::minecraft::parse_mc_version(mc_version)
        .is_some_and(|(major, minor)| major == 1 && minor >= 13)
}

/// 安装器写入 versions 目录的 id，后续目录重命名依赖它
fn get_forge_version_id(mc_version: &str, forge_version: &str) -> String {
    format!("{}-forge-{}", mc_version, forge_version)
}

fn compare_forge_versions(a: &str, b: &str) -> std::cmp::Ordering {
    let parse = |s: &str| -> Vec<u32> { s.split('.').filter_map(|p| p.parse().ok()).collect() };

    let a_parts = parse(a);
    let b_parts = parse(b);

    for i in 0..std::cmp::max(a_parts.len(), b_parts.len()) {
        let a_num = a_parts.get(i).unwrap_or(&0);
        let b_num = b_parts.get(i).unwrap_or(&0);
        match a_num.cmp(b_num) {
            std::cmp::Ordering::Equal => continue,
            ord => return ord,
        }
    }
    std::cmp::Ordering::Equal
}

/// Forge 安装器的候选下载地址。
/// maven 坐标在 1.8 与 1.9 之间换过形式（≤1.8 带 -{mc} 后缀，≥1.9 不带），
/// 所以两种形式都进候选表，版本判断只决定先试哪个；只靠判断挑一种，漏掉的版本就永远装不上
fn forge_installer_sources(mc: &str, v: &str) -> Vec<String> {
    let suffixed = format!("{mc}-{v}-{mc}");
    let plain = format!("{mc}-{v}");

    let prefer_suffixed = crate::utils::minecraft::parse_mc_version(mc)
        .is_some_and(|(major, minor)| major == 1 && minor <= 8);

    let coords = if prefer_suffixed {
        vec![suffixed, plain]
    } else {
        vec![plain, suffixed]
    };

    let mut sources = Vec::with_capacity(coords.len() * 4);
    let source = DownloadSource::current();

    for coord in &coords {
        let path = format!("net/minecraftforge/forge/{coord}/forge-{coord}-installer.jar");
        sources.extend(download_sources::build_mirror_urls(
            source,
            &format!("/maven/{path}"),
        ));
        sources.push(format!("{MAVEN_FORGE}/{path}"));
    }

    download_sources::dedup_urls(sources)
}

/// 流式把 URL 写到目标文件，边写边算 sha1，可选校验 ZIP 头
///
/// `resp.bytes()` 会把整个 jar 读进内存；库文件走同一条路径且并发下载，
/// 几个大 jar 同时进来就是几十 MB 峰值。流式之后内存占用与文件大小无关。
/// `validate` 拿到「ZIP 头是否合法 + 实际 sha1」，返回 false 表示这份内容不可用
pub(crate) async fn download_to_file(
    url: &str,
    client: &Client,
    target: &Path,
    expected_sha1: Option<&str>,
    require_zip: bool,
) -> Result<bool, LauncherError> {
    let response = download_with_retry(url, client, 2).await?;

    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).ok();
    }

    // 先写临时文件再改名：校验失败时不能留下半截 jar 冒充已下载
    let temp = target.with_extension("download.tmp");
    let mut file = fs::File::create(&temp)?;
    let mut hasher = file_utils::FileHasher::new();
    let mut head = [0u8; 4];
    let mut head_len = 0usize;
    let mut total: u64 = 0;

    let mut response = response;
    let written = async {
        while let Some(chunk) = response.chunk().await? {
            if head_len < head.len() {
                let take = (head.len() - head_len).min(chunk.len());
                head[head_len..head_len + take].copy_from_slice(&chunk[..take]);
                head_len += take;
            }
            file.write_all(&chunk)?;
            hasher.update(&chunk);
            total += chunk.len() as u64;
        }
        Ok::<(), LauncherError>(())
    }
    .await;

    if let Err(error) = written {
        drop(file);
        let _ = fs::remove_file(&temp);
        return Err(error);
    }
    drop(file);

    let is_zip = head_len >= 4 && head == [0x50, 0x4B, 0x03, 0x04];
    let actual = hasher.finish_hex();

    let hash_ok = match expected_sha1.map(str::trim).filter(|h| !h.is_empty()) {
        Some(expected) => {
            let ok = actual.eq_ignore_ascii_case(expected);
            if !ok {
                warn!("Forge: sha1 校验失败: {}", target.display());
            }
            ok
        }
        None => true,
    };

    let zip_ok = !require_zip || is_zip;
    if require_zip && !is_zip {
        warn!("Forge: 下载内容不是 ZIP: {}", target.display());
    }

    // 空文件/超小文件一律当作失败：多半是镜像返回的错误页
    if !hash_ok || !zip_ok || total < 100 {
        let _ = fs::remove_file(&temp);
        return Ok(false);
    }

    if let Err(error) = fs::rename(&temp, target) {
        let _ = fs::remove_file(&temp);
        return Err(error.into());
    }

    Ok(true)
}

async fn download_forge_installer(
    forge_version: &ForgeVersion,
) -> Result<tempfile::TempPath, LauncherError> {
    let installer_filename = format!(
        "forge-{}-{}-installer.jar",
        forge_version.mcversion, forge_version.version
    );

    let sources = forge_installer_sources(&forge_version.mcversion, &forge_version.version);

    // 安装器是实打实的大文件下载，用下载客户端（只设读超时）而不是 30 秒总超时的 API 客户端
    let download_client = download::get_http_client();
    let api_client = http_client::get_client();

    for url in &sources {
        info!("Forge: 尝试下载: {}", url);

        let tmp = tempfile::Builder::new()
            .prefix(&installer_filename)
            .suffix(".jar")
            .tempfile()
            .map_err(|e| LauncherError::Custom(format!("创建临时文件失败: {}", e)))?;
        let temp_path = tmp.into_temp_path();

        let expected = download_sha1(&api_client, url).await;
        if expected.is_none() {
            warn!("Forge: 未取到安装器 sha1，仅校验 ZIP 头: {}", url);
        }

        match download_to_file(url, &download_client, &temp_path, expected.as_deref(), true).await {
            Ok(true) => {
                if expected.is_some() {
                    info!("Forge: 安装器 sha1 校验通过");
                }
                info!("Forge: 安装器已下载");
                return Ok(temp_path);
            }
            Ok(false) => continue,
            Err(error) => {
                warn!("Forge: 下载安装器失败: {}", error);
                continue;
            }
        }
    }

    Err(LauncherError::Custom("安装器下载失败".to_string()))
}

/// 安装器自己也是 jar：它编译到的 class 版本、以及该 MC 版本的常规 Java，
/// 两者取高——1.20.1 的安装器字节码可能是 8，但运行时按 17 才稳
fn installer_requirement(installer_path: &Path, mc_version: &str) -> u32 {
    let by_class =
        crate::services::launcher::java_requirement::jar_java_version(installer_path).unwrap_or(0);
    let by_mc = crate::services::launcher::java_requirement::infer_java_for_version(mc_version)
        .unwrap_or(0);

    by_class.max(by_mc).max(8)
}

/// 挑能跑安装器的那份：优先精确命中，其次够用的最低版。
/// 旧 Forge 安装器在 Java 9+ 上直接 UnsupportedClassVersionError，不能随手往上挑
fn choose_installer_java(
    installer_path: &Path,
    mc_version: &str,
    candidates: &[JavaCandidate],
) -> Option<String> {
    let required = installer_requirement(installer_path, mc_version);

    // Java 8 时代的安装器在 9+ 上必然 UnsupportedClassVersionError / 找不到 Nashorn，
    // 与其白跑一轮（还可能弹个 GUI），不如直接交给手动安装
    if required <= 8 {
        return candidates
            .iter()
            .find(|candidate| candidate.version == 8)
            .map(|candidate| candidate.path.clone());
    }

    if let Some(exact) = candidates.iter().find(|c| c.version == required) {
        return Some(exact.path.clone());
    }

    candidates
        .iter()
        .filter(|c| c.version > required)
        .min_by(|a, b| {
            a.version
                .cmp(&b.version)
                .then_with(|| a.arch.rank().cmp(&b.arch.rank()))
                .then_with(|| a.path.cmp(&b.path))
        })
        .map(|c| c.path.clone())
}

pub(crate) fn check_cancelled(cancel: Option<&AtomicBool>) -> Result<(), LauncherError> {
    if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
        return Err(LauncherError::Cancelled);
    }
    Ok(())
}

async fn wait_for_cancel(flag: &AtomicBool) {
    while !flag.load(Ordering::Relaxed) {
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// 安装器和 processor 都不能无限挂起；取消或超时会 drop 子进程句柄，
/// 配合 kill_on_drop 把 JVM 一起收掉
async fn run_limited_command(
    cmd: &mut Command,
    label: &str,
    timeout: Duration,
    cancel: Option<&AtomicBool>,
) -> Result<std::process::Output, LauncherError> {
    cmd.kill_on_drop(true);
    let run = tokio::time::timeout(timeout, cmd.output());

    let result = match cancel {
        Some(flag) => {
            tokio::select! {
                result = run => result,
                _ = wait_for_cancel(flag) => {
                    return Err(LauncherError::Cancelled);
                }
            }
        }
        None => run.await,
    };

    match result {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(e)) => Err(LauncherError::Custom(format!("{}执行失败: {}", label, e))),
        Err(_) => Err(LauncherError::Custom(format!(
            "{}执行超时（{} 秒）",
            label,
            timeout.as_secs()
        ))),
    }
}

async fn run_official_installer(
    installer_path: &Path,
    game_dir: &Path,
    java_path: &str,
    cancel: Option<&AtomicBool>,
) -> Result<(), LauncherError> {
    let mut cmd = Command::new(java_path);
    cmd.current_dir(game_dir)
        .arg("-jar")
        .arg(installer_path)
        .arg("--installClient");

    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);

    let output = run_limited_command(&mut cmd, "Forge 安装器", INSTALLER_TIMEOUT, cancel).await?;

    let stderr = String::from_utf8_lossy(&output.stderr);

    if stderr.contains("not a recognized option") || stderr.contains("UnrecognizedOptionException")
    {
        let mut cmd2 = Command::new(java_path);
        cmd2.current_dir(game_dir)
            .arg("-Djava.awt.headless=true")
            .arg("-jar")
            .arg(installer_path);

        #[cfg(windows)]
        cmd2.creation_flags(CREATE_NO_WINDOW);

        let output2 =
            run_limited_command(&mut cmd2, "Forge 安装器", INSTALLER_TIMEOUT, cancel).await?;

        if !output2.status.success() {
            let stderr2 = String::from_utf8_lossy(&output2.stderr);
            if stderr2.contains("HeadlessException") {
                return Err(LauncherError::Custom(
                    "安装器需要 GUI，切换到手动安装".to_string(),
                ));
            }
            return Err(LauncherError::Custom(format!("安装器失败: {}", stderr2)));
        }
    } else if !output.status.success() {
        return Err(LauncherError::Custom(format!("安装器失败: {}", stderr)));
    }

    Ok(())
}

pub(crate) async fn download_sha1(client: &Client, url: &str) -> Option<String> {
    let response = download_with_retry(&format!("{url}.sha1"), client, 0)
        .await
        .ok()?;
    let text = response.text().await.ok()?;
    file_utils::parse_sha1(&text)
}

async fn download_with_retry(
    url: &str,
    client: &Client,
    max_retries: usize,
) -> Result<reqwest::Response, LauncherError> {
    let mut retry_count = 0;

    while retry_count <= max_retries {
        retry_count += 1;
        debug!("Forge: 下载尝试第{}次: {}", retry_count, url);

        if retry_count > 1 {
            let delay = std::cmp::min(2u64.pow(retry_count as u32 - 1), 10);
            tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
        }

        match client.get(url).send().await {
            Ok(response) if response.status().is_success() => {
                return Ok(response);
            }
            Ok(response) => {
                warn!("Forge: 下载失败，状态: {}", response.status());
            }
            Err(e) => {
                warn!("Forge: 网络错误: {}", e);
            }
        }
    }

    Err(LauncherError::Custom(format!(
        "下载失败: 超过最大重试次数 {}",
        url
    )))
}

async fn download_library(
    libraries_dir: &Path,
    rel_path: &str,
    sources: Vec<String>,
    lib_name: &str,
) -> Result<(), LauncherError> {
    let target_path = libraries_dir.join(rel_path);

    if target_path.exists() {
        return Ok(());
    }

    if let Some(parent) = target_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let client = download::get_http_client();
    for url in &sources {
        match download_to_file(url, &client, &target_path, None, true).await {
            Ok(true) => {
                info!("Forge: {} 下载成功", lib_name);
                return Ok(());
            }
            Ok(false) => continue,
            Err(error) => {
                warn!("Forge: {} 下载失败: {}", lib_name, error);
                continue;
            }
        }
    }

    Err(LauncherError::Custom(format!(
        "所有 {} 下载源均失败",
        lib_name
    )))
}

/// 老版本 Forge 预置库的候选地址：镜像 `/maven/{path}` 在前，官方 maven 兜底
fn preloaded_library_sources(path: &str, official_urls: Vec<String>) -> Vec<String> {
    let source = DownloadSource::current();
    let mut urls = download_sources::build_mirror_urls(source, &format!("/maven/{path}"));
    urls.extend(official_urls);
    download_sources::dedup_urls(urls)
}

async fn download_launchwrapper_library(
    libraries_dir: &Path,
    _mc_version: &str,
) -> Result<(), LauncherError> {
    let path = "net/minecraft/launchwrapper/1.12/launchwrapper-1.12.jar";
    download_library(
        libraries_dir,
        path,
        preloaded_library_sources(
            path,
            vec![
                format!("{}/{}", MAVEN_MINECRAFT, path),
                format!("{}/{}", MAVEN_CENTRAL, path),
            ],
        ),
        "LaunchWrapper",
    )
    .await
}

async fn download_asm_library(libraries_dir: &Path, mc_version: &str) -> Result<(), LauncherError> {
    let version = if mc_version.starts_with("1.7") {
        "5.0.3"
    } else if mc_version.starts_with("1.8")
        || mc_version.starts_with("1.9")
        || mc_version.starts_with("1.10")
        || mc_version.starts_with("1.11")
    {
        "5.0.4"
    } else {
        "5.2"
    };

    let path = format!("org/ow2/asm/asm-all/{}/asm-all-{}.jar", version, version);
    download_library(
        libraries_dir,
        &path,
        preloaded_library_sources(&path, vec![format!("{}/{}", MAVEN_CENTRAL, path)]),
        "ASM",
    )
    .await
}

async fn download_lzma_library(
    libraries_dir: &Path,
    mc_version: &str,
) -> Result<(), LauncherError> {
    let path = if mc_version.starts_with("1.7") {
        "lzma/lzma/0.0.1/lzma-0.0.1.jar"
    } else {
        "org/tukaani/xz/1.8/xz-1.8.jar"
    };
    download_library(
        libraries_dir,
        path,
        preloaded_library_sources(path, vec![format!("{}/{}", MAVEN_CENTRAL, path)]),
        "LZMA/XZ",
    )
    .await
}

use crate::utils::minecraft::maven_name_to_path;

pub(crate) async fn download_library_from_profile(
    library: &Value,
    libraries_dir: &Path,
    client: &Client,
) -> Result<(), LauncherError> {
    let name = match library["name"].as_str() {
        Some(n) => n,
        None => return Ok(()),
    };

    if let Some(false) = library.get("clientreq").and_then(|v| v.as_bool()) {
        return Ok(());
    }

    let artifact_sha1 = library
        .get("downloads")
        .and_then(|d| d.get("artifact"))
        .and_then(|a| a.get("sha1"))
        .and_then(|v| v.as_str())
        .filter(|value| !value.trim().is_empty());
    let checksum_sha1 = library
        .get("checksums")
        .and_then(|v| v.as_array())
        .and_then(|checksums| {
            checksums
                .iter()
                .filter_map(|value| value.as_str())
                .find(|value| file_utils::parse_sha1(value).is_some())
        });
    let expected_sha1 = artifact_sha1.or(checksum_sha1);

    if let Some(artifact) = library.get("downloads").and_then(|d| d.get("artifact"))
        && let Some(path) = artifact.get("path").and_then(|p| p.as_str())
    {
        let target_path = libraries_dir.join(path);
        if target_path.exists() {
            return Ok(());
        }

        if let Some(parent) = target_path.parent() {
            fs::create_dir_all(parent).ok();
        }

        let source = DownloadSource::current();
        let mut sources = Vec::new();
        if let Some(url) = artifact.get("url").and_then(|u| u.as_str()) {
            // 镜像优先：官方 maven 在国内可能整段超时，先试镜像能把等待从几十秒压到几百毫秒
            sources.extend(mirror_urls(url, source));
            sources.push(url.to_string());
        }
        sources.extend(download_sources::build_mirror_urls(
            source,
            &format!("/maven/{path}"),
        ));
        sources.push(format!("{}/{}", MAVEN_MINECRAFT, path));
        sources.push(format!("{}/{}", MAVEN_FORGE, path));
        let sources = download_sources::dedup_urls(sources);

        for url in &sources {
            match download_to_file(url, client, &target_path, expected_sha1, true).await {
                Ok(true) => return Ok(()),
                Ok(false) => continue,
                Err(error) => {
                    warn!("Forge: 下载 {} 失败: {}", target_path.display(), error);
                    continue;
                }
            }
        }
    }

    if let Some(maven_path) = maven_name_to_path(name, None, "jar") {
        let target_path = libraries_dir.join(&maven_path);
        if target_path.exists() {
            return Ok(());
        }

        let source = DownloadSource::current();
        let sources = download_sources::dedup_urls(
            [
                download_sources::build_mirror_urls(source, &format!("/maven/{maven_path}")),
                vec![
                    format!("{}/{}", MAVEN_MINECRAFT, maven_path),
                    format!("{}/{}", MAVEN_FORGE, maven_path),
                    format!("{}/{}", MAVEN_CENTRAL, maven_path),
                ],
            ]
            .concat(),
        );

        let client = download::get_http_client();
        for url in &sources {
            match download_to_file(url, &client, &target_path, expected_sha1, true).await {
                Ok(true) => return Ok(()),
                Ok(false) => continue,
                Err(error) => {
                    warn!("Forge: 下载 {} 失败: {}", target_path.display(), error);
                    continue;
                }
            }
        }
    }

    Err(LauncherError::Custom(format!(
        "库 {} 没下载下来，最后几个源都没给出可用的 jar（网络问题或该坐标在上游不存在）",
        name
    )))
}

async fn manual_install_old_forge(
    installer_path: &Path,
    game_dir: &Path,
    forge_version: &ForgeVersion,
) -> Result<(), LauncherError> {
    info!("Forge: 开始手动安装旧版本 Forge");

    let file = fs::File::open(installer_path)?;
    let mut archive = ZipArchive::new(file)?;

    let profile: Value = {
        let mut content = String::new();
        archive
            .by_name("install_profile.json")
            .map_err(|_| LauncherError::Custom("未找到 install_profile.json".to_string()))?
            .read_to_string(&mut content)?;
        serde_json::from_str(&content)?
    };

    let libraries_dir = game_dir.join("libraries");
    let client = http_client::get_client();

    // 旧 Forge 的 profile 里混着 pom-only 条目，单条失败不算致命，
    // 但全静默会装出一个缺 jar 的实例，留给崩溃日志去反推
    let mut library_failures: Vec<String> = Vec::new();
    if let Some(libs) = profile
        .get("versionInfo")
        .and_then(|v| v.get("libraries"))
        .and_then(|l| l.as_array())
    {
        for lib in libs {
            if let Err(e) = download_library_from_profile(lib, &libraries_dir, &client).await {
                library_failures.push(e.to_string());
            }
        }
    }

    let version_id = get_forge_version_id(&forge_version.mcversion, &forge_version.version);
    let version_dir = game_dir.join("versions").join(&version_id);
    fs::create_dir_all(&version_dir)?;

    let mut version_info = profile
        .get("versionInfo")
        .ok_or_else(|| LauncherError::Custom("缺少 versionInfo".to_string()))?
        .clone();

    if let Value::Object(ref mut obj) = version_info {
        obj.insert("id".to_string(), serde_json::json!(version_id));
        obj.insert(
            "inheritsFrom".to_string(),
            serde_json::json!(forge_version.mcversion),
        );
        obj.insert(
            "jar".to_string(),
            serde_json::json!(forge_version.mcversion),
        );
    }

    let version_json_path = version_dir.join(format!("{}.json", version_id));
    fs::write(
        &version_json_path,
        serde_json::to_string_pretty(&version_info)?,
    )?;

    let file = fs::File::open(installer_path)?;
    let mut archive = ZipArchive::new(file)?;

    for i in 0..archive.len() {
        let mut file = match archive.by_index(i) {
            Ok(f) => f,
            Err(_) => continue,
        };
        let name = file.name()?.to_string();

        if name.contains("..") || name.starts_with('/') || name.starts_with('\\') {
            log::warn!("跳过可疑的 zip 条目: {}", name);
            continue;
        }

        if name.starts_with("maven/") && !name.ends_with('/') {
            if let Some(rel) = name.strip_prefix("maven/") {
                if rel.contains("..") {
                    log::warn!("跳过可疑的 maven 路径: {}", name);
                    continue;
                }
                let target = libraries_dir.join(rel);
                if let Some(p) = target.parent() {
                    fs::create_dir_all(p).ok();
                }
                let mut buf = Vec::new();
                if file.read_to_end(&mut buf).is_ok() {
                    fs::write(&target, &buf).ok();
                }
            }
        } else if name.ends_with("-universal.jar") && !name.contains('/') {
            let mut buf = Vec::new();
            if file.read_to_end(&mut buf).is_ok() {
                let forge_lib = format!(
                    "net/minecraftforge/forge/{mc}-{v}/forge-{mc}-{v}-universal.jar",
                    mc = forge_version.mcversion,
                    v = forge_version.version
                );
                let target = libraries_dir.join(&forge_lib);
                if let Some(p) = target.parent() {
                    fs::create_dir_all(p).ok();
                }
                fs::write(&target, &buf).ok();
            }
        }
    }

    // 加载器本体是启动的必要条件，缺了就别报成功。
    // 坐标在 1.7/1.8 上多一段 `-{mc}` 后缀（`1.7.10-10.13.4.1614-1.7.10`），所以按前缀找目录而不是自己拼
    if !loader_jar_present(
        &libraries_dir,
        &forge_version.mcversion,
        &forge_version.version,
    ) {
        let detail = if library_failures.is_empty() {
            "安装器里既没有加载器本体，也没从任何源下到".to_string()
        } else {
            library_failures.join("；")
        };
        return Err(LauncherError::Custom(format!(
            "旧版 Forge 手动安装没拿到加载器本体（libraries/net/minecraftforge/forge 下没有 {} 的 jar）：{}",
            forge_version.version, detail
        )));
    }

    info!("Forge: 手动安装完成");
    Ok(())
}

/// `libraries/net/minecraftforge/forge/` 下是否存在该版本的加载器 jar
fn loader_jar_present(libraries_dir: &Path, mc_version: &str, loader_version: &str) -> bool {
    let prefix = format!("{}-{}", mc_version, loader_version);
    let Ok(entries) = fs::read_dir(libraries_dir.join("net/minecraftforge/forge")) else {
        return false;
    };

    for entry in entries.flatten() {
        if !entry.path().is_dir() || !entry.file_name().to_string_lossy().starts_with(&prefix) {
            continue;
        }

        let Ok(files) = fs::read_dir(entry.path()) else {
            continue;
        };
        if files.flatten().any(|file| {
            file.file_name()
                .to_string_lossy()
                .to_lowercase()
                .ends_with(".jar")
        }) {
            return true;
        }
    }

    false
}

async fn manual_install_new_forge(
    installer_path: &Path,
    game_dir: &Path,
    forge_version: &ForgeVersion,
    java_path: &str,
    java_candidates: &[JavaCandidate],
    cancel: Option<&AtomicBool>,
) -> Result<(), LauncherError> {
    info!("Forge: 开始手动安装新版 Forge (1.13+)");

    let file = fs::File::open(installer_path)?;
    let mut archive = ZipArchive::new(file)?;

    let profile: Value = {
        let mut content = String::new();
        archive
            .by_name("install_profile.json")
            .map_err(|_| LauncherError::Custom("未找到 install_profile.json".to_string()))?
            .read_to_string(&mut content)?;
        serde_json::from_str(&content)?
    };

    let version_json: Value = {
        let mut content = String::new();
        if let Ok(mut f) = archive.by_name("version.json") {
            f.read_to_string(&mut content).ok();
        }
        if content.is_empty() {
            serde_json::json!({})
        } else {
            serde_json::from_str(&content).unwrap_or(serde_json::json!({}))
        }
    };

    let libraries_dir = game_dir.join("libraries");
    let client = http_client::get_client();

    // 新版 Forge 的库下载同样单条失败不致命（pom-only 条目本来就没有 jar），
    // 但全静默会装出缺件的实例只能靠崩溃日志反推，结尾汇总一条 warn 留排查线索
    let mut library_failures: Vec<String> = Vec::new();
    if let Some(libs) = profile.get("libraries").and_then(|l| l.as_array()) {
        for lib in libs {
            if let Err(e) = download_library_from_profile(lib, &libraries_dir, &client).await {
                library_failures.push(e.to_string());
            }
        }
    }
    if let Some(libs) = version_json.get("libraries").and_then(|l| l.as_array()) {
        for lib in libs {
            if let Err(e) = download_library_from_profile(lib, &libraries_dir, &client).await {
                library_failures.push(e.to_string());
            }
        }
    }

    let file = fs::File::open(installer_path)?;
    let mut archive = ZipArchive::new(file)?;

    for i in 0..archive.len() {
        let mut file = match archive.by_index(i) {
            Ok(f) => f,
            Err(_) => continue,
        };
        let name = file.name()?.to_string();

        if name.contains("..") || name.starts_with('/') || name.starts_with('\\') {
            log::warn!("跳过可疑的 zip 条目: {}", name);
            continue;
        }

        if name.starts_with("maven/") && !name.ends_with('/') {
            if let Some(rel) = name.strip_prefix("maven/") {
                if rel.contains("..") {
                    log::warn!("跳过可疑的 maven 路径: {}", name);
                    continue;
                }
                let target = libraries_dir.join(rel);
                if let Some(p) = target.parent() {
                    fs::create_dir_all(p).ok();
                }
                let mut buf = Vec::new();
                if file.read_to_end(&mut buf).is_ok() {
                    fs::write(&target, &buf).ok();
                }
            }
        } else if name.starts_with("data/")
            && !name.ends_with('/')
            && let Some(rel) = name.strip_prefix("data/")
        {
            if rel.contains("..") {
                log::warn!("跳过可疑的 data 路径: {}", name);
                continue;
            }
            let target = libraries_dir
                .join("net/minecraftforge/forge")
                .join(format!(
                    "{}-{}",
                    forge_version.mcversion, forge_version.version
                ))
                .join(rel);
            if let Some(p) = target.parent() {
                fs::create_dir_all(p).ok();
            }
            let mut buf = Vec::new();
            if file.read_to_end(&mut buf).is_ok() {
                fs::write(&target, &buf).ok();
            }
        }
    }

    let forge_lib_dir = libraries_dir.join("net/minecraftforge/forge").join(format!(
        "{}-{}",
        forge_version.mcversion, forge_version.version
    ));

    run_processors(
        &profile,
        &ProcessorVars {
            game_dir,
            libraries_dir: &libraries_dir,
            installer: installer_path,
            minecraft_jar: game_dir
                .join("versions")
                .join(&forge_version.mcversion)
                .join(format!("{}.jar", forge_version.mcversion)),
            data_dir: forge_lib_dir,
            mc_version: &forge_version.mcversion,
            loader_version: &forge_version.version,
            java_fallback: java_path,
            java_candidates,
        },
        cancel,
    )
    .await?;

    let version_id = get_forge_version_id(&forge_version.mcversion, &forge_version.version);
    let version_dir = game_dir.join("versions").join(&version_id);
    fs::create_dir_all(&version_dir)?;

    let mut final_version = version_json.clone();
    if let Value::Object(ref mut obj) = final_version {
        obj.insert("id".to_string(), serde_json::json!(version_id));
        obj.insert(
            "inheritsFrom".to_string(),
            serde_json::json!(forge_version.mcversion),
        );
    }

    let version_json_path = version_dir.join(format!("{}.json", version_id));
    fs::write(
        &version_json_path,
        serde_json::to_string_pretty(&final_version)?,
    )?;

    if !library_failures.is_empty() {
        // profile 与 version_json 的库列表大面积重叠，去重后再汇总
        library_failures.sort();
        library_failures.dedup();
        warn!(
            "Forge: 新版手动安装有 {} 个库下载失败，缺件可能导致启动失败: {}",
            library_failures.len(),
            library_failures.join("；")
        );
    }

    info!("Forge: 新版手动安装完成");
    Ok(())
}

/// processor 是否适用于客户端安装：没有 `sides` 表示两边都要
fn runs_on_client(processor: &Value) -> bool {
    match processor.get("sides").and_then(|s| s.as_array()) {
        Some(sides) => sides.iter().any(|side| side.as_str() == Some("client")),
        None => true,
    }
}

pub(crate) async fn run_processors(
    profile: &Value,
    vars: &ProcessorVars<'_>,
    cancel: Option<&AtomicBool>,
) -> Result<(), LauncherError> {
    let processors = match profile.get("processors").and_then(|p| p.as_array()) {
        Some(p) => p,
        None => return Ok(()),
    };

    let libraries_dir = vars.libraries_dir;
    let data = profile.get("data").and_then(|d| d.as_object());

    // 清单里常有只服务服务端的条目（NeoForge 21.11 的第一条是 sides: ["server"]），客户端安装要跳过，
    // 所以报「总数 / 实际要跑的条数」而不是单个总数
    let runnable = processors
        .iter()
        .filter(|processor| runs_on_client(processor))
        .count();
    info!(
        "安装器: 共 {} 个 processors，客户端需执行 {} 个",
        processors.len(),
        runnable
    );

    let mut executed = 0;
    for (idx, processor) in processors.iter().enumerate() {
        check_cancelled(cancel)?;
        if !runs_on_client(processor) {
            debug!(
                "安装器: 跳过 {}/{}（声明为服务端专用）",
                idx + 1,
                processors.len()
            );
            continue;
        }

        let jar_name = match processor.get("jar").and_then(|j| j.as_str()) {
            Some(j) => j,
            None => continue,
        };

        let jar_path = match maven_name_to_path(jar_name, None, "jar") {
            Some(p) => libraries_dir.join(p),
            None => continue,
        };

        if !jar_path.exists() {
            // 静默跳过 = 装出一个缺 jar 的实例，之后只能靠崩日志反推
            return Err(LauncherError::Custom(format!(
                "processor {} 的 jar 不在 libraries 里: {}",
                jar_name,
                jar_path.display()
            )));
        }

        let mut classpath = vec![jar_path.to_string_lossy().to_string()];
        if let Some(cp) = processor.get("classpath").and_then(|c| c.as_array()) {
            for lib in cp {
                if let Some(lib_name) = lib.as_str()
                    && let Some(lib_path) = maven_name_to_path(lib_name, None, "jar")
                {
                    let full_path = libraries_dir.join(&lib_path);
                    if full_path.exists() {
                        classpath.push(full_path.to_string_lossy().to_string());
                    }
                }
            }
        }

        let mut args: Vec<String> = Vec::new();
        if let Some(proc_args) = processor.get("args").and_then(|a| a.as_array()) {
            for arg in proc_args {
                if let Some(arg_str) = arg.as_str() {
                    args.push(resolve_processor_arg(arg_str, data, vars));
                }
            }
        }

        let main_class = get_jar_main_class(&jar_path)?;

        // processor 是外部 jar，它编译到的 class 版本决定要哪份 Java：
        // 装 NeoForge 26.x 时配置里那份 17 跑不动 major 69 的 loader
        let required =
            crate::services::launcher::java_requirement::jar_java_version(&jar_path).unwrap_or(0);
        let fallback_version =
            crate::services::launcher::java::detect_java_version(vars.java_fallback);
        let java_path = choose_processor_java(
            required,
            (vars.java_fallback, fallback_version),
            vars.java_candidates,
        )
        .map_err(LauncherError::Custom)?;

        executed += 1;
        let requirement = if required > 0 {
            format!("最低需要 Java {}", required)
        } else {
            "无法识别所需的 Java 版本".to_string()
        };
        info!(
            "安装器: 执行 processor {}/{}: {}（用 Java {} 运行，{}）",
            executed, runnable, main_class, java_path, requirement
        );

        let cp_separator = if cfg!(windows) { ";" } else { ":" };
        let cp_string = classpath.join(cp_separator);

        let mut cmd = Command::new(java_path);
        cmd.current_dir(vars.game_dir)
            .arg("-cp")
            .arg(&cp_string)
            .arg(&main_class)
            .args(&args);

        #[cfg(windows)]
        cmd.creation_flags(CREATE_NO_WINDOW);

        let output = run_limited_command(
            &mut cmd,
            &format!("processor {}/{}", idx + 1, processors.len()),
            PROCESSOR_TIMEOUT,
            cancel,
        )
        .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(LauncherError::Custom(format!(
                "processor {}/{} 执行失败（{}）:\n{}",
                idx + 1,
                processors.len(),
                jar_name,
                stderr.trim()
            )));
        }
    }

    Ok(())
}

/// processor 参数里那些隐式变量的取值来源
pub(crate) struct ProcessorVars<'a> {
    pub game_dir: &'a Path,
    pub libraries_dir: &'a Path,
    /// installer 自身；`{INSTALLER}` 与它内部的 data/ 都靠这个
    pub installer: &'a Path,
    pub minecraft_jar: PathBuf,
    /// 从 installer 抽出来的 data/ 落在哪，`/data/x` 按它解析
    pub data_dir: PathBuf,
    pub mc_version: &'a str,
    pub loader_version: &'a str,
    /// 配置里那份 Java；没配时是 PATH 上的 `java`
    pub java_fallback: &'a str,
    /// 本机 Java 候选，fallback 不够时从这里挑
    pub java_candidates: &'a [JavaCandidate],
}

fn resolve_processor_arg(
    arg: &str,
    data: Option<&serde_json::Map<String, Value>>,
    vars: &ProcessorVars,
) -> String {
    if arg.starts_with('{') && arg.ends_with('}') {
        let key = &arg[1..arg.len() - 1];
        if let Some(data_map) = data
            && let Some(value) = data_map.get(key)
        {
            let client_val = match value {
                Value::Object(obj) => obj.get("client").and_then(|v| v.as_str()),
                other => other.as_str(),
            };
            if let Some(client_val) = client_val {
                return resolve_data_value(client_val, vars);
            }
        }
    }

    // 不在 data 里的隐式变量：{ROOT}/libraries/{INSTALLER}/{MINECRAFT_JAR} 可能出现在
    // 参数的任意位置（NeoForge 用 `{ROOT}/libraries/`，Forge 用整段 `{INSTALLER}`）
    if arg.contains('{') {
        return arg
            // 只装客户端；`--side {SIDE}` 不换掉的话，installertools 会拿 "{SIDE}" 去查映射
            .replace("{SIDE}", "client")
            .replace("{ROOT}", &vars.game_dir.to_string_lossy())
            .replace("{LIBRARY_DIR}", &vars.libraries_dir.to_string_lossy())
            .replace("{INSTALLER}", &vars.installer.to_string_lossy())
            .replace(
                "{MINECRAFT_JAR}",
                vars.minecraft_jar.to_string_lossy().as_ref(),
            )
            .replace("{MINECRAFT_VERSION}", vars.mc_version)
            .replace("{FORGE_VERSION}", vars.loader_version);
    }

    if arg.starts_with('[') && arg.ends_with(']') {
        let artifact = &arg[1..arg.len() - 1];
        if let Some(path) = maven_name_to_path(artifact, None, "jar") {
            return vars.libraries_dir.join(path).to_string_lossy().to_string();
        }
    }

    arg.to_string()
}

fn resolve_data_value(value: &str, vars: &ProcessorVars) -> String {
    if value.starts_with('[') && value.ends_with(']') {
        let artifact = &value[1..value.len() - 1];
        if let Some(path) = maven_name_to_path(artifact, None, "jar") {
            return vars.libraries_dir.join(path).to_string_lossy().to_string();
        }
    }

    // `/data/x` 指的是 installer 内部那份，我们已经抽到 data_dir；
    // 其它 `/x` 才是相对游戏根目录
    if let Some(rel) = value.strip_prefix("/data/") {
        return vars.data_dir.join(rel).to_string_lossy().to_string();
    }
    if let Some(rel) = value.strip_prefix('/') {
        return vars.game_dir.join(rel).to_string_lossy().to_string();
    }

    resolve_processor_arg(value, None, vars)
}

fn get_jar_main_class(jar_path: &Path) -> Result<String, LauncherError> {
    let file = fs::File::open(jar_path)?;
    let mut archive = ZipArchive::new(file)?;

    let mut manifest = archive
        .by_name("META-INF/MANIFEST.MF")
        .map_err(|_| LauncherError::Custom("JAR 中没有 MANIFEST.MF".to_string()))?;

    let mut content = String::new();
    manifest.read_to_string(&mut content)?;

    for line in content.lines() {
        if line.starts_with("Main-Class:") {
            return Ok(line.trim_start_matches("Main-Class:").trim().to_string());
        }
    }

    Err(LauncherError::Custom(
        "MANIFEST 中没有 Main-Class".to_string(),
    ))
}

/// processor 是外部 jar，它自己编译到的 class 版本决定了能跑它的 Java：
/// 配置里那份够用就不动，否则从候选里挑够用的最低版
fn choose_processor_java(
    required: u32,
    fallback: (&str, Option<u32>),
    candidates: &[JavaCandidate],
) -> Result<String, String> {
    if fallback.1.is_some_and(|ver| ver >= required) {
        return Ok(fallback.0.to_string());
    }

    let mut usable: Vec<&JavaCandidate> = candidates
        .iter()
        .filter(|candidate| candidate.version >= required)
        .collect();

    if usable.is_empty() {
        let found = if candidates.is_empty() {
            "没发现任何可用的 Java".to_string()
        } else {
            let mut versions: Vec<u32> = candidates.iter().map(|c| c.version).collect();
            versions.sort_unstable();
            versions.dedup();
            format!(
                "本机只有 Java {}",
                versions
                    .iter()
                    .map(|v| v.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        return Err(format!(
            "安装器需要 Java {} 来跑 processor，{}；请在设置里补一份再装",
            required, found
        ));
    }

    usable.sort_by(|a, b| {
        a.version
            .cmp(&b.version)
            .then_with(|| a.arch.rank().cmp(&b.arch.rank()))
            .then_with(|| a.path.cmp(&b.path))
    });

    Ok(usable[0].path.clone())
}

#[cfg(test)]
mod tests {
    use super::{
        ProcessorVars, choose_installer_java, choose_processor_java, find_installed_loader_dir,
        forge_installer_sources, loader_jar_present, mirror_urls, resolve_processor_arg,
        run_limited_command, runs_on_client, snapshot_version_dirs,
    };
    use crate::services::download::source::{BMCL_BASE, DownloadSource, HKMC_MAIN_BASE};
    use crate::services::launcher::java::{JavaArch, JavaCandidate};
    use std::sync::atomic::AtomicBool;
    use std::time::Duration;
    use tokio::process::Command;

    #[cfg(windows)]
    use super::CREATE_NO_WINDOW;

    fn slow_command() -> Command {
        #[cfg(windows)]
        {
            let mut cmd = Command::new("ping");
            cmd.args(["-n", "10", "127.0.0.1"]);
            cmd.creation_flags(CREATE_NO_WINDOW);
            cmd
        }
        #[cfg(not(windows))]
        {
            let mut cmd = Command::new("sleep");
            cmd.arg("5");
            cmd
        }
    }

    fn cand(path: &str, version: u32) -> JavaCandidate {
        JavaCandidate {
            path: path.to_string(),
            version,
            arch: JavaArch::Bits64,
            is_ea: false,
            from_config: false,
        }
    }

    #[test]
    fn test_find_installed_loader_dir_accepts_the_expected_id() {
        let tmp = tempfile::tempdir().unwrap();
        let versions = tmp.path().join("versions");
        let dir = versions.join("1.20.1-forge-47.2.0");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("1.20.1-forge-47.2.0.json"), "{}").unwrap();

        let before = snapshot_version_dirs(&versions);
        assert_eq!(
            find_installed_loader_dir(&versions, &before, "1.20.1-forge-47.2.0", "1.20.1")
                .as_deref(),
            Some("1.20.1-forge-47.2.0")
        );
    }

    /// 1.7.10 的安装器把 id 写成大小写不同的形式，认不出来实例就退化成原版
    #[test]
    fn test_find_installed_loader_dir_falls_back_to_new_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let versions = tmp.path().join("versions");
        std::fs::create_dir_all(versions.join("1.7.10")).unwrap();

        let before = snapshot_version_dirs(&versions);

        let installed = versions.join("1.7.10-Forge10.13.4.1614");
        std::fs::create_dir_all(&installed).unwrap();
        std::fs::write(
            installed.join("1.7.10-Forge10.13.4.1614.json"),
            serde_json::json!({
                "id": "1.7.10-Forge10.13.4.1614",
                "inheritsFrom": "1.7.10",
                "libraries": [{ "name": "net.minecraftforge:forge:1.7.10-10.13.4.1614" }]
            })
            .to_string(),
        )
        .unwrap();

        assert_eq!(
            find_installed_loader_dir(&versions, &before, "1.7.10-forge-10.13.4.1614", "1.7.10")
                .as_deref(),
            Some("1.7.10-Forge10.13.4.1614")
        );
    }

    #[test]
    fn test_find_installed_loader_dir_ignores_unrelated_new_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let versions = tmp.path().join("versions");
        std::fs::create_dir_all(&versions).unwrap();
        let before = snapshot_version_dirs(&versions);

        // 装 Forge 期间可能顺带装出来的原版目录，不该被认成加载器
        let vanilla = versions.join("1.20.1");
        std::fs::create_dir_all(&vanilla).unwrap();
        std::fs::write(
            vanilla.join("1.20.1.json"),
            serde_json::json!({ "id": "1.20.1", "libraries": [] }).to_string(),
        )
        .unwrap();

        assert_eq!(
            find_installed_loader_dir(&versions, &before, "1.20.1-forge-47.2.0", "1.20.1"),
            None
        );
    }

    #[test]
    fn test_installer_java_prefers_the_exact_version() {
        let installer = std::path::Path::new("C:/nonexistent/installer.jar");
        let candidates = vec![
            cand("C:/jdk8/bin/java.exe", 8),
            cand("C:/jdk17/bin/java.exe", 17),
        ];

        // 1.12.2 的安装器只能在 8 上跑，17 会 UnsupportedClassVersionError
        assert_eq!(
            choose_installer_java(installer, "1.12.2", &candidates).as_deref(),
            Some("C:/jdk8/bin/java.exe")
        );

        // 1.20.1 要 17；没有 17 时只能往上取最近的一份
        assert_eq!(
            choose_installer_java(installer, "1.20.1", &candidates).as_deref(),
            Some("C:/jdk17/bin/java.exe")
        );
        let only_newer = vec![
            cand("C:/jdk8/bin/java.exe", 8),
            cand("C:/jdk21/bin/java.exe", 21),
        ];
        assert_eq!(
            choose_installer_java(installer, "1.20.1", &only_newer).as_deref(),
            Some("C:/jdk21/bin/java.exe")
        );
    }

    #[test]
    fn test_installer_java_is_none_when_nothing_is_new_enough() {
        let installer = std::path::Path::new("C:/nonexistent/installer.jar");
        // 1.21.4 的安装器要 21，本机只有 8 和 17，跑不了就直接走手动安装
        let candidates = vec![
            cand("C:/jdk8/bin/java.exe", 8),
            cand("C:/jdk17/bin/java.exe", 17),
        ];

        assert_eq!(
            choose_installer_java(installer, "1.21.4", &candidates),
            None
        );
    }

    #[test]
    fn test_loader_jar_present_accepts_both_coordinate_forms() {
        let tmp = tempfile::tempdir().unwrap();
        let libs = tmp.path().join("libraries");

        // 1.7.10 的坐标多一段 -{mc} 后缀，按自己拼的路径去查会漏
        let legacy = libs.join("net/minecraftforge/forge/1.7.10-10.13.4.1614-1.7.10");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(
            legacy.join("forge-1.7.10-10.13.4.1614-1.7.10-universal.jar"),
            b"x",
        )
        .unwrap();

        assert!(loader_jar_present(&libs, "1.7.10", "10.13.4.1614"));
        assert!(!loader_jar_present(&libs, "1.7.10", "10.13.4.1615"));
        assert!(!loader_jar_present(&libs, "1.12.2", "14.23.5.2860"));
    }

    #[test]
    fn test_installer_sources_cover_both_coordinate_forms() {
        // 实测边界：1.7.10 / 1.8.9 在 maven 上只有带 -{mc} 后缀的坐标
        let old = forge_installer_sources("1.8.9", "11.15.1.2318");
        assert!(
            old.iter().any(|u| u
                .contains("1.8.9-11.15.1.2318-1.8.9/forge-1.8.9-11.15.1.2318-1.8.9-installer.jar")),
            "1.8.9 必须有带后缀的候选：{:?}",
            old
        );

        // 1.9 起改成不带后缀
        let new = forge_installer_sources("1.12.2", "14.23.5.2861");
        assert!(
            new.iter()
                .any(|u| u.ends_with("1.12.2-14.23.5.2861/forge-1.12.2-14.23.5.2861-installer.jar")),
            "1.12.2 必须有不带后缀的候选：{:?}",
            new
        );

        // 先试实测有效的那种，省掉整轮无谓重试
        assert!(
            old[0]
                .contains("1.8.9-11.15.1.2318-1.8.9/forge-1.8.9-11.15.1.2318-1.8.9-installer.jar"),
            "1.8.9 应优先带后缀：{:?}",
            old
        );
        assert!(
            new[0].contains("1.12.2-14.23.5.2861/forge-1.12.2-14.23.5.2861-installer.jar"),
            "1.12.2 应优先不带后缀：{:?}",
            new
        );

        let f19 = forge_installer_sources("1.9", "12.16.1.1934");
        assert!(
            f19[0].contains("/1.9-12.16.1.1934/forge-1.9-12.16.1.1934-installer.jar"),
            "1.9 应优先不带后缀：{:?}",
            f19
        );
    }

    #[test]
    fn test_choose_processor_java_keeps_config_java_when_it_suffices() {
        let candidates = vec![
            cand("C:/zulu-26/bin/java.exe", 26),
            cand("C:/lib17/bin/java.exe", 17),
        ];

        let picked =
            choose_processor_java(17, ("C:/lib17/bin/java.exe", Some(17)), &candidates).unwrap();

        assert_eq!(picked, "C:/lib17/bin/java.exe");
    }

    #[test]
    fn test_choose_processor_java_picks_lowest_sufficient_candidate() {
        let candidates = vec![
            cand("C:/zulu-26/bin/java.exe", 26),
            cand("C:/lib-21/bin/java.exe", 21),
            cand("C:/lib17/bin/java.exe", 17),
        ];

        // 配置里那份只有 17，跑不动要 25 的 processor
        let picked =
            choose_processor_java(25, ("C:/lib17/bin/java.exe", Some(17)), &candidates).unwrap();

        assert_eq!(picked, "C:/zulu-26/bin/java.exe");
    }

    #[test]
    fn test_choose_processor_java_fails_loudly_when_nothing_is_new_enough() {
        let candidates = vec![cand("C:/lib17/bin/java.exe", 17)];

        let err = choose_processor_java(25, ("C:/lib17/bin/java.exe", Some(17)), &candidates)
            .expect_err("没有够新的 Java 就该拒绝安装，而不是装出半成品");

        assert!(err.contains("25"), "要说清需要几: {}", err);
        assert!(err.contains("17"), "要列出本机有什么: {}", err);
    }

    #[test]
    fn test_processor_vars_resolve_installer_and_vanilla_jar() {
        let root = std::path::Path::new("C:/game");
        let vars = ProcessorVars {
            game_dir: root,
            libraries_dir: &root.join("libraries"),
            installer: &root.join("installer.jar"),
            minecraft_jar: root.join("versions/26.3/26.3.jar"),
            data_dir: root.join("libraries/net/neoforged/neoforge/26.3.0.0-beta"),
            java_fallback: "C:/lib17/bin/java.exe",
            java_candidates: &[],
            mc_version: "26.3",
            loader_version: "26.3.0.0-beta",
        };

        assert_eq!(
            resolve_processor_arg("{INSTALLER}", None, &vars),
            vars.installer.to_string_lossy()
        );
        assert_eq!(
            resolve_processor_arg("{MINECRAFT_JAR}", None, &vars),
            vars.minecraft_jar.to_string_lossy()
        );
        assert_eq!(
            resolve_processor_arg("{ROOT}/libraries/", None, &vars).replace('\\', "/"),
            "C:/game/libraries/"
        );
    }

    /// install_profile 里 `/data/client.lzma` 指的是 installer 内部的文件，
    /// 我们把它抽到 loader 的库目录，所以必须按 data_dir 解析，不能拼到游戏根目录
    #[test]
    fn test_processor_data_path_uses_data_dir() {
        let root = std::path::Path::new("C:/game");
        let vars = ProcessorVars {
            game_dir: root,
            libraries_dir: &root.join("libraries"),
            installer: &root.join("installer.jar"),
            minecraft_jar: root.join("versions/26.3/26.3.jar"),
            data_dir: root.join("libraries/net/neoforged/neoforge/26.3.0.0-beta"),
            java_fallback: "C:/lib17/bin/java.exe",
            java_candidates: &[],
            mc_version: "26.3",
            loader_version: "26.3.0.0-beta",
        };

        let data = serde_json::json!({ "BINPATCH": { "client": "/data/client.lzma" } });
        let map = data.as_object().unwrap();

        let resolved = resolve_processor_arg("{BINPATCH}", Some(map), &vars).replace('\\', "/");

        assert!(
            resolved.ends_with("libraries/net/neoforged/neoforge/26.3.0.0-beta/client.lzma"),
            "binpatch 要按抽出来的 data 目录解析，实际 {}",
            resolved
        );
    }

    /// NeoForge 21.11 的清单第一条是 `sides: ["server"]`，客户端安装要跳过它，
    /// 否则会白跑一次 EXTRACT_FILES（写服务端的启动脚本）
    #[test]
    fn test_server_only_processor_is_skipped_for_client() {
        assert!(!runs_on_client(&serde_json::json!({ "sides": ["server"] })));
        assert!(runs_on_client(&serde_json::json!({ "sides": ["client"] })));
        assert!(runs_on_client(
            &serde_json::json!({ "sides": ["client", "server"] })
        ));
        // 没有 sides 表示两端都要跑
        assert!(runs_on_client(&serde_json::json!({ "jar": "a:b:1" })));
    }

    /// NeoForge 21.11 的 processor 会传 `--side {SIDE}`；不替换就报
    /// "Missing download info for {SIDE} mappings"
    #[test]
    fn test_processor_side_placeholder_resolves_to_client() {
        let root = std::path::Path::new("C:/game");
        let vars = ProcessorVars {
            game_dir: root,
            libraries_dir: &root.join("libraries"),
            installer: &root.join("installer.jar"),
            minecraft_jar: root.join("versions/1.21.11/1.21.11.jar"),
            data_dir: root.join("libraries/net/neoforged/neoforge/21.11.48"),
            java_fallback: "C:/lib17/bin/java.exe",
            java_candidates: &[],
            mc_version: "1.21.11",
            loader_version: "21.11.48",
        };

        assert_eq!(resolve_processor_arg("{SIDE}", None, &vars), "client");
        assert_eq!(
            resolve_processor_arg("{ROOT}/run-{SIDE}.txt", None, &vars).replace('\\', "/"),
            "C:/game/run-client.txt"
        );
    }

    /// `{MOJMAPS}` 的 client 值是 `[...:mappings@txt]`：输出必须落在 .txt 上，
    /// 否则下一个 processor 的 `--input-mappings` 会指向不存在的文件
    #[test]
    fn test_mojmaps_output_keeps_txt_extension() {
        let root = std::path::Path::new("C:/game");
        let vars = ProcessorVars {
            game_dir: root,
            libraries_dir: &root.join("libraries"),
            installer: &root.join("installer.jar"),
            minecraft_jar: root.join("versions/1.21.11/1.21.11.jar"),
            data_dir: root.join("libraries/net/neoforged/neoforge/21.11.48"),
            java_fallback: "C:/lib17/bin/java.exe",
            java_candidates: &[],
            mc_version: "1.21.11",
            loader_version: "21.11.48",
        };

        let data = serde_json::json!({
            "MOJMAPS": {
                "client": "[net.minecraft:client:1.21.11:mappings@txt]",
                "server": "[net.minecraft:server:1.21.11:mappings@txt]"
            }
        });
        let map = data.as_object().unwrap();

        let resolved = resolve_processor_arg("{MOJMAPS}", Some(map), &vars).replace('\\', "/");

        assert!(
            resolved
                .ends_with("libraries/net/minecraft/client/1.21.11/client-1.21.11-mappings.txt"),
            "映射输出路径不对: {}",
            resolved
        );
    }

    /// BMCL 与 MC 光速都镜像了 Mojang 库、Forge 与 NeoForge 的 maven；这三类都要换源，
    /// 否则官方地址在国内会先卡满超时
    #[test]
    fn test_mirror_urls_covers_neoforge_forge_and_mojang() {
        let source = DownloadSource::HkmcMain;
        assert_eq!(
            mirror_urls(
                "https://maven.neoforged.net/releases/net/neoforged/fancymodloader/loader/12.0.0/loader-12.0.0.jar",
                source,
            )
            .first()
            .map(String::as_str),
            Some(
                "https://bsmirror.hkmc.online/maven/net/neoforged/fancymodloader/loader/12.0.0/loader-12.0.0.jar"
            )
        );
        assert_eq!(
            mirror_urls("https://maven.minecraftforge.net/net/minecraftforge/forge/1.12.2-14.23.5.2860/forge-1.12.2-14.23.5.2860-universal.jar", source)
                .first()
                .map(String::as_str),
            Some("https://bsmirror.hkmc.online/maven/net/minecraftforge/forge/1.12.2-14.23.5.2860/forge-1.12.2-14.23.5.2860-universal.jar")
        );
        assert_eq!(
            mirror_urls(
                "https://libraries.minecraft.net/com/mojang/brigadier/1.3.11/brigadier-1.3.11.jar",
                source,
            )
            .first()
            .map(String::as_str),
            Some(
                "https://bsmirror.hkmc.online/maven/com/mojang/brigadier/1.3.11/brigadier-1.3.11.jar"
            )
        );

        // BMCL 源下第一条是 BMCL，后面才是光速两条线路
        let bmcl = mirror_urls(
            "https://libraries.minecraft.net/com/mojang/brigadier/1.3.11/brigadier-1.3.11.jar",
            DownloadSource::Bmcl,
        );
        assert_eq!(
            bmcl[0],
            format!("{BMCL_BASE}/maven/com/mojang/brigadier/1.3.11/brigadier-1.3.11.jar")
        );
        assert!(bmcl.contains(&format!(
            "{HKMC_MAIN_BASE}/maven/com/mojang/brigadier/1.3.11/brigadier-1.3.11.jar"
        )));
    }

    /// repo1.maven.org 的地址同样能通过镜像的 /maven 路径换源
    #[test]
    fn test_mirror_urls_handles_maven_central() {
        let urls = mirror_urls(
            "https://repo1.maven.org/maven2/com/google/guava/guava/33.6.0-jre/guava-33.6.0-jre.jar",
            DownloadSource::Bmcl,
        );
        assert_eq!(
            urls.first().map(String::as_str),
            Some(
                "https://bmclapi2.bangbang93.com/maven/com/google/guava/guava/33.6.0-jre/guava-33.6.0-jre.jar"
            )
        );
    }

    #[tokio::test]
    async fn test_limited_command_times_out() {
        let mut cmd = slow_command();
        let err = run_limited_command(&mut cmd, "测试命令", Duration::from_millis(300), None)
            .await
            .unwrap_err();

        assert!(err.to_string().contains("超时"), "{err}");
    }

    #[tokio::test]
    async fn test_limited_command_cancels_running_process() {
        let cancel = AtomicBool::new(true);
        let mut cmd = slow_command();
        let err = run_limited_command(&mut cmd, "测试命令", Duration::from_secs(30), Some(&cancel))
            .await
            .unwrap_err();

        assert!(err.to_string().contains("取消"), "{err}");
    }
}
