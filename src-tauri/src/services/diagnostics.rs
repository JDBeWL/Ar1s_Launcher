//! 诊断包导出
//!
//! 只收集排障需要的文件：启动器/实例日志、crash-reports、版本 JSON、模组文件名、Java 与系统信息。
//! token 与设置同存 ar1s.json，所以进包的配置快照必须走 `config_for_diagnostics` 逐字段脱敏。

use crate::errors::LauncherError;
use crate::models::GameConfig;
use crate::services::config::{self, config_for_diagnostics};
use crate::services::launcher::isolation;
use crate::services::launcher::java::{JavaArch, JavaCandidate};
use crate::services::memory;
use crate::utils::file_utils;
use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

pub struct BundleContext<'a> {
    pub instance_name: &'a str,
    pub storage_dir: &'a Path,
    pub game_dir: &'a Path,
    pub config: &'a GameConfig,
    pub java_candidates: &'a [JavaCandidate],
}

pub async fn export_bundle(
    instance_name: String,
    target_path: String,
) -> Result<(), LauncherError> {
    tokio::task::spawn_blocking(move || -> Result<(), LauncherError> {
        let config = config::load_config()?;
        let game_dir = PathBuf::from(&config.game_dir);
        let storage_dir = config::init_storage().to_path_buf();
        let candidates = crate::services::launcher::java::collect_candidates(&config);
        let ctx = BundleContext {
            instance_name: &instance_name,
            storage_dir: &storage_dir,
            game_dir: &game_dir,
            config: config.as_ref(),
            java_candidates: &candidates,
        };

        write_bundle(&ctx, Path::new(&target_path))
    })
    .await?
}

pub fn write_bundle(ctx: &BundleContext<'_>, target: &Path) -> Result<(), LauncherError> {
    validate_target(target)?;

    let instance_dir = file_utils::instance_dir(&ctx.game_dir.join("versions"), ctx.instance_name)?;
    if !instance_dir.is_dir() {
        return Err(LauncherError::Custom(format!(
            "实例 '{}' 不存在",
            ctx.instance_name
        )));
    }
    let content_root = isolation::content_root(ctx.config, ctx.game_dir, ctx.instance_name);

    let temp = target.with_extension("zip.part");
    let result = (|| -> Result<(), LauncherError> {
        let file = File::create(&temp)?;
        let mut zip = ZipWriter::new(file);
        write_entries(&mut zip, ctx, &instance_dir, &content_root)?;
        let file = zip.finish()?;
        file.sync_all()?;
        Ok(())
    })();

    if let Err(error) = result {
        let _ = fs::remove_file(&temp);
        return Err(error);
    }

    // Windows 的 rename 不覆盖已存在文件；保存对话框已经确认过覆盖，这里先删旧包
    if target.exists()
        && let Err(error) = fs::remove_file(target)
    {
        let _ = fs::remove_file(&temp);
        return Err(error.into());
    }
    if let Err(error) = fs::rename(&temp, target) {
        let _ = fs::remove_file(&temp);
        return Err(error.into());
    }
    Ok(())
}

fn validate_target(target: &Path) -> Result<(), LauncherError> {
    if !target
        .extension()
        .is_some_and(|ext| ext.to_string_lossy().eq_ignore_ascii_case("zip"))
    {
        return Err(LauncherError::Custom(
            "诊断包必须保存为 .zip 文件".to_string(),
        ));
    }
    if target.is_dir() {
        return Err(LauncherError::Custom("诊断包路径是一个目录".to_string()));
    }
    match target.parent() {
        Some(parent) if parent.is_dir() => Ok(()),
        _ => Err(LauncherError::Custom("诊断包保存目录不存在".to_string())),
    }
}

fn write_entries(
    zip: &mut ZipWriter<File>,
    ctx: &BundleContext<'_>,
    instance_dir: &Path,
    content_root: &Path,
) -> Result<(), LauncherError> {
    let snapshot = config_for_diagnostics(ctx.config);
    add_text(
        zip,
        "config/ar1s.json",
        &serde_json::to_string_pretty(&snapshot)?,
    )?;
    add_text(
        zip,
        "system.txt",
        &system_text(ctx, instance_dir, content_root),
    )?;
    add_text(zip, "README.txt", &readme_text(ctx))?;
    add_text(zip, "instance/mods.txt", &mods_text(content_root))?;

    add_file_if_exists(
        zip,
        "instance/instance.json",
        &instance_dir.join("instance.json"),
    )?;
    add_file_if_exists(
        zip,
        "instance/debug-profile.json",
        &instance_dir.join("debug-profile.json"),
    )?;
    add_version_chain(zip, ctx.game_dir, ctx.instance_name)?;

    add_file_if_exists(
        zip,
        "instance/logs/latest.log",
        &content_root.join("logs/latest.log"),
    )?;
    for path in newest_files(
        &content_root.join("logs"),
        |path| path.to_string_lossy().ends_with(".log.gz"),
        3,
    ) {
        add_file(
            zip,
            &format!("instance/logs/{}", archive_name(&path)),
            &path,
        )?;
    }
    for path in newest_files(
        &content_root.join("crash-reports"),
        |path| {
            path.extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("txt"))
        },
        5,
    ) {
        add_file(
            zip,
            &format!("instance/crash-reports/{}", archive_name(&path)),
            &path,
        )?;
    }
    for path in newest_files(
        content_root,
        |path| {
            path.file_name().is_some_and(|name| {
                let name = name.to_string_lossy();
                name.starts_with("hs_err_pid") && name.ends_with(".log")
            })
        },
        3,
    ) {
        add_file(
            zip,
            &format!("instance/hs_err/{}", archive_name(&path)),
            &path,
        )?;
    }
    for path in newest_files(
        &ctx.storage_dir.join("logs"),
        |path| {
            path.extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("log"))
        },
        5,
    ) {
        add_file(
            zip,
            &format!("launcher-logs/{}", archive_name(&path)),
            &path,
        )?;
    }

    Ok(())
}

fn add_version_chain(
    zip: &mut ZipWriter<File>,
    game_dir: &Path,
    instance_name: &str,
) -> Result<(), LauncherError> {
    let mut current = instance_name.to_string();
    let mut seen = std::collections::HashSet::new();

    while seen.insert(current.clone()) {
        let Ok(dir) = file_utils::version_dir(&game_dir.join("versions"), &current) else {
            break;
        };
        let path = dir.join(format!("{}.json", current));
        if !path.is_file() {
            break;
        }
        add_file(
            zip,
            &format!("instance/versions/{0}/{0}.json", current),
            &path,
        )?;

        let Ok(content) = fs::read_to_string(&path) else {
            break;
        };
        let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) else {
            break;
        };
        match json.get("inheritsFrom").and_then(|value| value.as_str()) {
            Some(parent) if !parent.is_empty() => current = parent.to_string(),
            _ => break,
        }
    }
    Ok(())
}

fn add_text(zip: &mut ZipWriter<File>, name: &str, text: &str) -> Result<(), LauncherError> {
    add_bytes(zip, name, text.as_bytes())
}

fn add_bytes(zip: &mut ZipWriter<File>, name: &str, bytes: &[u8]) -> Result<(), LauncherError> {
    zip.start_file(
        name,
        SimpleFileOptions::default().compression_method(CompressionMethod::Deflated),
    )?;
    zip.write_all(bytes)?;
    Ok(())
}

fn add_file(zip: &mut ZipWriter<File>, name: &str, path: &Path) -> Result<(), LauncherError> {
    add_bytes(zip, name, &fs::read(path)?)
}

fn add_file_if_exists(
    zip: &mut ZipWriter<File>,
    name: &str,
    path: &Path,
) -> Result<(), LauncherError> {
    if path.is_file() {
        add_file(zip, name, path)?;
    }
    Ok(())
}

fn newest_files(dir: &Path, predicate: impl Fn(&Path) -> bool, limit: usize) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<(SystemTime, PathBuf)> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && predicate(path))
        .filter_map(|path| Some((path.metadata().ok()?.modified().ok()?, path)))
        .collect();
    files.sort_by_key(|entry| std::cmp::Reverse(entry.0));
    files
        .into_iter()
        .take(limit)
        .map(|(_, path)| path)
        .collect()
}

fn archive_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

fn mods_text(content_root: &Path) -> String {
    let mods_dir = content_root.join("mods");
    let mut entries: Vec<(String, u64, bool)> = Vec::new();

    if let Ok(read_dir) = fs::read_dir(&mods_dir) {
        for entry in read_dir.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            let enabled = name.ends_with(".jar");
            if !enabled && !name.ends_with(".jar.disabled") {
                continue;
            }
            entries.push((
                name,
                entry.metadata().map(|meta| meta.len()).unwrap_or(0),
                enabled,
            ));
        }
    }

    entries.sort_by_key(|entry| entry.0.to_lowercase());
    let enabled = entries.iter().filter(|(_, _, enabled)| *enabled).count();
    let disabled = entries.len() - enabled;

    let mut out = format!(
        "目录: {}\n共 {} 个，启用 {} 个，禁用 {} 个\n\n",
        mods_dir.display(),
        entries.len(),
        enabled,
        disabled
    );
    for (name, size, enabled) in entries {
        out.push_str(&format!(
            "[{}] {} ({:.1} MB)\n",
            if enabled { "启用" } else { "禁用" },
            name,
            size as f64 / 1024.0 / 1024.0
        ));
    }
    out
}

fn system_text(ctx: &BundleContext<'_>, instance_dir: &Path, content_root: &Path) -> String {
    let memory = memory::get_system_memory();
    let mut out = String::new();

    let _ = writeln!(out, "Ar1s Launcher {}", env!("CARGO_PKG_VERSION"));
    let _ = writeln!(
        out,
        "导出时间: {}",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
    );
    let _ = writeln!(out, "实例: {}", ctx.instance_name);
    let _ = writeln!(out, "游戏目录: {}", ctx.game_dir.display());
    let _ = writeln!(
        out,
        "版本隔离: {}",
        if ctx.config.version_isolation {
            "开"
        } else {
            "关"
        }
    );
    let _ = writeln!(out, "实例目录: {}", instance_dir.display());
    let _ = writeln!(out, "内容目录: {}", content_root.display());
    let _ = writeln!(
        out,
        "系统: {} / {}，{} 个 CPU 线程",
        std::env::consts::OS,
        std::env::consts::ARCH,
        std::thread::available_parallelism()
            .map(|count| count.get())
            .unwrap_or(0)
    );
    let _ = writeln!(
        out,
        "内存: 共 {} MB，已用 {} MB，可用 {} MB",
        memory.total_memory_mb, memory.used_memory_mb, memory.available_memory_mb
    );
    let _ = writeln!(
        out,
        "最大堆: {} MB，自动内存: {}",
        ctx.config.max_memory,
        if ctx.config.auto_memory_enabled {
            "开"
        } else {
            "关"
        }
    );
    let _ = writeln!(
        out,
        "Java 配置: {}",
        ctx.config.java_path.as_deref().unwrap_or("未设置")
    );
    let _ = writeln!(
        out,
        "手动 Java 路径: {}",
        if ctx.config.custom_java_paths.is_empty() {
            "无".to_string()
        } else {
            ctx.config.custom_java_paths.join("; ")
        }
    );
    let _ = writeln!(
        out,
        "自动匹配 Java: {}",
        if ctx.config.auto_match_java {
            "开"
        } else {
            "关"
        }
    );
    let _ = writeln!(out, "检测到的 Java:");
    if ctx.java_candidates.is_empty() {
        let _ = writeln!(out, "  （无）");
    }
    for candidate in ctx.java_candidates {
        let _ = writeln!(
            out,
            "  - Java {} · {} · {}{}",
            candidate.version,
            arch_label(candidate.arch),
            candidate.path,
            if candidate.from_config {
                "（配置指定）"
            } else {
                ""
            }
        );
    }

    out
}

fn arch_label(arch: JavaArch) -> &'static str {
    match arch {
        JavaArch::Bits64 => "64-bit",
        JavaArch::Bits32 => "32-bit",
        JavaArch::Unknown => "位数未知",
    }
}

fn readme_text(ctx: &BundleContext<'_>) -> String {
    format!(
        "Ar1s Launcher 诊断包\n\
         导出时间: {}\n\
         实例: {}\n\n\
         包含: 启动器日志、实例 latest.log、历史日志、crash-reports、hs_err、版本 JSON、模组文件名、Java 与系统信息。\n\
         已排除: 存档、模组本体、资源包、光影包、options.txt 与模组配置。\n\
         配置中的 token、用户名、UUID、皮肤 URL 和代理 URL 已移除或打码。\n\
         游戏日志、crash-reports 与系统信息保留原文，可能包含玩家名、服务器地址和本机路径。\n",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
        ctx.instance_name
    )
}

#[cfg(test)]
mod tests {
    use super::{BundleContext, write_bundle};
    use crate::models::GameConfig;
    use crate::services::config;
    use crate::services::launcher::java::{JavaArch, JavaCandidate};
    use std::collections::HashMap;
    use std::fs;
    use std::io::Read;
    use std::path::Path;

    fn read_entry(entries: &HashMap<String, Vec<u8>>, name: &str) -> String {
        String::from_utf8(
            entries
                .get(name)
                .unwrap_or_else(|| panic!("缺少 zip 条目 {name}，实际有 {:?}", entries.keys()))
                .clone(),
        )
        .unwrap()
    }

    fn read_zip(path: &Path) -> HashMap<String, Vec<u8>> {
        let file = fs::File::open(path).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        let mut entries = HashMap::new();
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i).unwrap();
            let name = entry.name().unwrap().to_string();
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).unwrap();
            entries.insert(name, bytes);
        }
        entries
    }

    struct Fixture {
        dir: tempfile::TempDir,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::TempDir::new().unwrap();
            let storage = dir.path().join("storage");
            let game = dir.path().join("game");
            let instance = game.join("versions").join("Test Instance");

            fs::create_dir_all(storage.join("logs")).unwrap();
            fs::write(storage.join("logs/ar1s_1.log"), "launcher log 1").unwrap();
            fs::write(storage.join("logs/ar1s_2.log"), "launcher log 2").unwrap();

            fs::create_dir_all(instance.join("mods")).unwrap();
            fs::write(instance.join("mods/a.jar"), "jar").unwrap();
            fs::write(instance.join("mods/b.jar.disabled"), "jar").unwrap();
            fs::create_dir_all(instance.join("logs")).unwrap();
            fs::write(instance.join("logs/latest.log"), "game latest log").unwrap();
            fs::write(instance.join("logs/2026-10-18-2.log.gz"), "gz").unwrap();
            fs::create_dir_all(instance.join("crash-reports")).unwrap();
            fs::write(
                instance.join("crash-reports/crash-test-client.txt"),
                "---- Minecraft Crash Report ----\nDescription: Rendering overlay",
            )
            .unwrap();
            fs::write(instance.join("hs_err_pid123.log"), "JVM fatal error").unwrap();
            fs::create_dir_all(instance.join("saves/world")).unwrap();
            fs::write(instance.join("saves/world/level.dat"), "WORLD").unwrap();
            fs::write(
                instance.join("Test Instance.json"),
                r#"{"id":"Test Instance","mainClass":"net.minecraft.client.main.Main"}"#,
            )
            .unwrap();
            fs::write(instance.join("instance.json"), r#"{"minecraft":"26.3"}"#).unwrap();
            fs::write(instance.join("debug-profile.json"), r#"{"profile":"test"}"#).unwrap();

            Self { dir }
        }

        fn storage(&self) -> std::path::PathBuf {
            self.dir.path().join("storage")
        }

        fn game(&self) -> std::path::PathBuf {
            self.dir.path().join("game")
        }

        fn target(&self) -> std::path::PathBuf {
            self.dir.path().join("diagnostics.zip")
        }

        fn config(&self) -> GameConfig {
            let mut config = GameConfig {
                game_dir: self.game().to_string_lossy().to_string(),
                version_isolation: true,
                username: Some("Steve".to_string()),
                uuid: Some("uuid-secret".to_string()),
                skin_url: Some("https://skins.example/secret".to_string()),
                proxy_url: Some("http://user:pass@proxy.example".to_string()),
                ..GameConfig::default()
            };
            config.ms_access_token = Some("SECRET-ACCESS".to_string());
            config.ms_refresh_token = Some("SECRET-REFRESH".to_string());
            config
        }

        fn java_candidates(&self) -> Vec<JavaCandidate> {
            vec![JavaCandidate {
                path: "C:/Java/jdk-25/bin/java.exe".to_string(),
                version: 25,
                arch: JavaArch::Bits64,
                is_ea: false,
                from_config: true,
            }]
        }
    }

    #[test]
    fn test_bundle_contains_diagnostics_but_no_secrets_or_saves() {
        let fixture = Fixture::new();
        let config = fixture.config();
        let candidates = fixture.java_candidates();
        let ctx = BundleContext {
            instance_name: "Test Instance",
            storage_dir: &fixture.storage(),
            game_dir: &fixture.game(),
            config: &config,
            java_candidates: &candidates,
        };

        write_bundle(&ctx, &fixture.target()).unwrap();
        let entries = read_zip(&fixture.target());

        assert!(entries.contains_key("README.txt"));
        assert!(entries.contains_key("system.txt"));
        assert!(entries.contains_key("config/ar1s.json"));
        assert!(entries.contains_key("instance/mods.txt"));
        assert!(entries.contains_key("instance/versions/Test Instance/Test Instance.json"));
        assert!(entries.contains_key("instance/logs/latest.log"));
        assert!(entries.contains_key("instance/logs/2026-10-18-2.log.gz"));
        assert!(entries.contains_key("instance/crash-reports/crash-test-client.txt"));
        assert!(entries.contains_key("instance/hs_err/hs_err_pid123.log"));
        assert!(entries.contains_key("instance/debug-profile.json"));
        assert!(
            entries
                .keys()
                .any(|name| name.starts_with("launcher-logs/ar1s_")),
            "启动器日志应进包: {:?}",
            entries.keys()
        );
        assert!(
            !entries.keys().any(|name| name.contains("saves/")),
            "存档绝不能进包"
        );
        assert!(
            !entries.keys().any(|name| name.ends_with("a.jar")),
            "模组本体不进包"
        );

        let mods = read_entry(&entries, "instance/mods.txt");
        assert!(mods.contains("a.jar"));
        assert!(mods.contains("b.jar.disabled"));
        assert!(mods.contains("启用 1 个，禁用 1 个"));

        let system = read_entry(&entries, "system.txt");
        assert!(system.contains("Test Instance"));
        assert!(system.contains("Java 25"));
    }

    #[test]
    fn test_bundle_config_is_redacted() {
        let fixture = Fixture::new();
        let config = fixture.config();
        let ctx = BundleContext {
            instance_name: "Test Instance",
            storage_dir: &fixture.storage(),
            game_dir: &fixture.game(),
            config: &config,
            java_candidates: &[],
        };

        write_bundle(&ctx, &fixture.target()).unwrap();
        let entries = read_zip(&fixture.target());
        let snapshot: serde_json::Value =
            serde_json::from_str(&read_entry(&entries, "config/ar1s.json")).unwrap();

        assert!(snapshot.get("ms_access_token").is_none());
        assert!(snapshot.get("ms_refresh_token").is_none());
        assert!(snapshot["username"].is_null());
        assert!(snapshot["uuid"].is_null());
        assert!(snapshot["skin_url"].is_null());
        assert_eq!(snapshot["proxy_url"], "***");

        let raw = read_entry(&entries, "config/ar1s.json");
        assert!(!raw.contains("SECRET-ACCESS"));
        assert!(!raw.contains("SECRET-REFRESH"));
        assert!(!raw.contains("user:pass"));
        assert!(!raw.contains("uuid-secret"));
    }

    #[test]
    fn test_bundle_rejects_non_zip_target() {
        let fixture = Fixture::new();
        let config = fixture.config();
        let ctx = BundleContext {
            instance_name: "Test Instance",
            storage_dir: &fixture.storage(),
            game_dir: &fixture.game(),
            config: &config,
            java_candidates: &[],
        };

        let target = fixture.dir.path().join("diagnostics.txt");
        let error = write_bundle(&ctx, &target).unwrap_err();
        assert!(error.to_string().contains(".zip"), "{}", error);
        assert!(!target.exists());
    }

    #[test]
    fn test_bundle_rejects_traversal_instance_name() {
        let fixture = Fixture::new();
        let config = fixture.config();
        let ctx = BundleContext {
            instance_name: "../evil",
            storage_dir: &fixture.storage(),
            game_dir: &fixture.game(),
            config: &config,
            java_candidates: &[],
        };

        assert!(write_bundle(&ctx, &fixture.target()).is_err());
    }

    #[test]
    fn test_bundle_can_overwrite_an_existing_zip() {
        let fixture = Fixture::new();
        let config = fixture.config();
        let ctx = BundleContext {
            instance_name: "Test Instance",
            storage_dir: &fixture.storage(),
            game_dir: &fixture.game(),
            config: &config,
            java_candidates: &[],
        };

        write_bundle(&ctx, &fixture.target()).unwrap();
        fs::write(fixture.target(), "stale").unwrap();
        write_bundle(&ctx, &fixture.target()).unwrap();

        let entries = read_zip(&fixture.target());
        assert!(entries.contains_key("README.txt"));
    }

    #[test]
    fn test_diagnostic_config_removes_account_identifiers() {
        let fixture = Fixture::new();
        let snapshot = config::config_for_diagnostics(&fixture.config());

        assert!(snapshot.ms_access_token.is_none());
        assert!(snapshot.ms_refresh_token.is_none());
        assert!(snapshot.username.is_none());
        assert!(snapshot.uuid.is_none());
        assert!(snapshot.skin_url.is_none());
        assert_eq!(snapshot.proxy_url.as_deref(), Some("***"));
    }
}
