//! 老版本 Forge：手工拼库并改写版本 JSON

use super::*;
use crate::errors::LauncherError;
use crate::services::http_client;
use log::info;
use serde_json::Value;
use std::fs;
use std::io::Read;
use std::path::Path;
use zip::ZipArchive;

pub(crate) async fn manual_install_old_forge(
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

    // 解压 maven/ 与 universal.jar 是纯同步的批量写盘，不能占着 async worker
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
                            mc = mc,
                            v = ver
                        );
                        let target = libs.join(&forge_lib);
                        if let Some(p) = target.parent() {
                            fs::create_dir_all(p).ok();
                        }
                        fs::write(&target, &buf).ok();
                    }
                }
            }

            Ok(())
        })
        .await
        .map_err(|e| LauncherError::Custom(format!("解压任务异常终止: {}", e)))??;
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
pub(crate) fn loader_jar_present(
    libraries_dir: &Path,
    mc_version: &str,
    loader_version: &str,
) -> bool {
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
