//! 新版 Forge：手工展开 installer 的 processors 清单

use super::*;
use crate::errors::LauncherError;
use crate::services::http_client;
use crate::services::launcher::java::JavaCandidate;
use crate::utils::minecraft::maven_name_to_path;
use log::{info, warn};
use serde_json::Value;
use std::fs;
use std::io::Read;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use zip::ZipArchive;

pub(crate) async fn manual_install_new_forge(
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

    // 新版 installer 内嵌的 maven/ 可能几百 MB，逐条解压是纯同步写盘，不能占着 async worker
    {
        let installer = installer_path.to_path_buf();
        let libs = libraries_dir.clone();
        let mc = forge_version.mcversion.clone();
        let ver = forge_version.version.clone();
        tokio::task::spawn_blocking(move || -> Result<(), LauncherError> {
            let file = fs::File::open(&installer)?;
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
                        let target = libs.join(rel);
                        if let Some(p) = target.parent() {
                            fs::create_dir_all(p)?;
                        }
                        let mut buf = Vec::new();
                        file.read_to_end(&mut buf)?;
                        fs::write(&target, &buf)?;
                    }
                } else if name.starts_with("data/")
                    && !name.ends_with('/')
                    && let Some(rel) = name.strip_prefix("data/")
                {
                    if rel.contains("..") {
                        log::warn!("跳过可疑的 data 路径: {}", name);
                        continue;
                    }
                    let target = libs
                        .join("net/minecraftforge/forge")
                        .join(format!("{}-{}", mc, ver))
                        .join(rel);
                    if let Some(p) = target.parent() {
                        fs::create_dir_all(p)?;
                    }
                    let mut buf = Vec::new();
                    file.read_to_end(&mut buf)?;
                    fs::write(&target, &buf)?;
                }
            }

            Ok(())
        })
        .await
        .map_err(|e| LauncherError::Custom(format!("解压任务异常终止: {}", e)))??;
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
pub(crate) fn runs_on_client(processor: &Value) -> bool {
    match processor.get("sides").and_then(|s| s.as_array()) {
        Some(sides) => sides.iter().any(|side| side.as_str() == Some("client")),
        None => true,
    }
}

pub(crate) fn resolve_processor_arg(
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

pub(crate) fn resolve_data_value(value: &str, vars: &ProcessorVars) -> String {
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
