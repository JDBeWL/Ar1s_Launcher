//! Classpath 构建和库预检逻辑
//!
//! 按 rules 挑库、优先 downloads.artifact.path 定位 jar，LaunchWrapper 主类下递归补依赖

use crate::errors::LauncherError;
use crate::utils::library::resolve_library_files;
use crate::utils::minecraft::find_jar_version;
use std::fs;
use std::path::{Path, PathBuf};

pub fn find_library_jar(dir: &Path, patterns: &[&str]) -> Option<PathBuf> {
    let read_dir = fs::read_dir(dir).ok()?;

    for entry in read_dir.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_library_jar(&path, patterns) {
                return Some(found);
            }
        } else {
            let name = entry.file_name().to_string_lossy().to_lowercase();
            let full_path = path.to_string_lossy().to_lowercase();

            for pattern in patterns {
                if (name.contains(pattern) || full_path.contains(pattern)) && name.ends_with(".jar")
                {
                    return Some(path);
                }
            }
        }
    }
    None
}

pub fn precheck_and_heal_library(
    classpath: &mut Vec<PathBuf>,
    libraries_base_dir: &Path,
    library_name: &str,
    classpath_patterns: &[&str],
    search_patterns: &[&str],
    emit: &impl Fn(&str, String),
) -> bool {
    let is_missing = !classpath.iter().any(|p| {
        let s = p.to_string_lossy().to_lowercase();
        classpath_patterns.iter().any(|pat| s.contains(pat))
    });

    if !is_missing {
        return true;
    }

    emit(
        "log-debug",
        format!(
            "预检：Classpath 未包含 {}，尝试在 libraries 目录自动查找",
            library_name
        ),
    );

    if let Some(jar) = find_library_jar(libraries_base_dir, search_patterns) {
        emit(
            "log-debug",
            format!(
                "自动自愈：发现 {} 库，加入 Classpath: {}",
                library_name,
                jar.display()
            ),
        );
        classpath.push(jar);
        true
    } else {
        emit(
            "log-error",
            format!("预检失败：在 libraries 中未找到 {} 库。", library_name),
        );
        false
    }
}

pub fn build_classpath(
    version_json: &serde_json::Value,
    libraries_base_dir: &Path,
    version_dir: &Path,
    version: &str,
    emit: &impl Fn(&str, String),
) -> Result<Vec<PathBuf>, LauncherError> {
    let mut classpath = vec![];
    let mut missing: Vec<String> = vec![];

    for file in resolve_library_files(version_json) {
        // natives 的 classifier 由 natives.rs 解压，不进 classpath
        if file.is_native {
            continue;
        }

        let path = libraries_base_dir.join(&file.relative_path);
        if path.exists() {
            classpath.push(path);
        } else {
            missing.push(file.relative_path);
        }
    }

    // 逐条 emit 会让启动瞬间刷出几十上百个 IPC 事件，前端要跟着做同样多次 DOM 追加。
    // 这里汇总成一条：真正的诊断价值在「总数 + 缺失项」，不在逐行清单
    emit(
        "log-debug",
        format!("Classpath 就绪：{} 个库文件", classpath.len()),
    );

    // 缺库必须当场说：静默丢进 classpath 只会在很久之后变成一个 NoClassDefFoundError
    if !missing.is_empty() {
        let list = missing
            .iter()
            .map(|m| format!("  · {}", m))
            .collect::<Vec<_>>()
            .join("\n");
        let msg = format!(
            "libraries 里缺 {} 个文件，这个实例装得不完整：\n{}\n用「修复实例」重新补齐后再启动。",
            missing.len(),
            list
        );
        emit("log-error", msg.clone());
        return Err(LauncherError::Custom(msg));
    }

    let versions_dir = version_dir
        .parent()
        .ok_or_else(|| LauncherError::Custom("无法获取 versions 目录".to_string()))?;
    let game_dir = versions_dir
        .parent()
        .ok_or_else(|| LauncherError::Custom("无法获取游戏根目录".to_string()))?;
    let jar_version = find_jar_version(version_json, game_dir)?;

    let main_game_jar_path = if jar_version != version {
        versions_dir
            .join(&jar_version)
            .join(format!("{}.jar", jar_version))
    } else {
        version_dir.join(format!("{}.jar", version))
    };

    emit(
        "log-debug",
        format!(
            "主游戏JAR路径: {} (jar_version: {})",
            main_game_jar_path.display(),
            jar_version
        ),
    );

    if !main_game_jar_path.exists() {
        emit(
            "log-error",
            format!("主游戏JAR文件不存在: {}", main_game_jar_path.display()),
        );
        return Err(LauncherError::localized(
            "main_jar_missing",
            format!("主游戏JAR文件不存在: {}", main_game_jar_path.display()),
            vec![main_game_jar_path.display().to_string()],
        ));
    }

    classpath.push(main_game_jar_path);
    Ok(classpath)
}

pub fn precheck_launchwrapper_libraries(
    classpath: &mut Vec<PathBuf>,
    libraries_base_dir: &Path,
    emit: &impl Fn(&str, String),
) -> Result<(), LauncherError> {
    if !precheck_and_heal_library(
        classpath,
        libraries_base_dir,
        "LaunchWrapper",
        &["net/minecraft/launchwrapper", "launchwrapper-"],
        &["launchwrapper", "net/minecraft/launchwrapper"],
        emit,
    ) {
        let error_msg = "预检失败：缺少 LaunchWrapper 库。请重新运行 Forge 安装或手动补齐 libraries/net/minecraft/launchwrapper/* 并在版本 JSON 的 libraries 中声明 net.minecraft:launchwrapper:1.12（且包含 downloads.artifact.path）".to_string();
        emit("log-error", error_msg.clone());
        return Err(LauncherError::Custom(error_msg));
    }

    // 预检其他依赖库（不强制要求）
    precheck_and_heal_library(
        classpath,
        libraries_base_dir,
        "jopt-simple",
        &["jopt-simple", "joptsimple"],
        &["jopt-simple", "joptsimple"],
        emit,
    );

    precheck_and_heal_library(
        classpath,
        libraries_base_dir,
        "Forge/FML",
        &["minecraftforge", "forge-", "/fml/", "\\fml\\"],
        &["forge", "minecraftforge", "net/minecraftforge/forge"],
        emit,
    );

    precheck_and_heal_library(
        classpath,
        libraries_base_dir,
        "ASM",
        &["asm", "org/objectweb/asm", "asm-all"],
        &["asm", "org/objectweb/asm", "asm-all"],
        emit,
    );

    precheck_and_heal_library(
        classpath,
        libraries_base_dir,
        "LZMA",
        &["lzma", "xz", "org/tukaani", "lzma-sdk"],
        &["xz", "lzma", "org/tukaani", "lzma-sdk"],
        emit,
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::build_classpath;
    use std::fs;

    /// 造一个最小可启动的目录树：versions/<v>/<v>.jar 必须存在
    fn fixture(json: serde_json::Value) -> (tempfile::TempDir, serde_json::Value) {
        let root = tempfile::TempDir::new().unwrap();
        let version_dir = root.path().join("versions").join("1.12.2");
        fs::create_dir_all(&version_dir).unwrap();
        fs::write(version_dir.join("1.12.2.jar"), b"jar").unwrap();
        fs::create_dir_all(root.path().join("libraries")).unwrap();
        (root, json)
    }

    fn touch(root: &std::path::Path, rel: &str) {
        let path = root.join("libraries").join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"x").unwrap();
    }

    fn build(
        root: &std::path::Path,
        json: &serde_json::Value,
    ) -> Result<Vec<std::path::PathBuf>, crate::errors::LauncherError> {
        build_classpath(
            json,
            &root.join("libraries"),
            &root.join("versions").join("1.12.2"),
            "1.12.2",
            &|_: &str, _: String| {},
        )
    }

    #[test]
    fn test_natives_library_keeps_its_common_jar_on_classpath() {
        let (dir, json) = fixture(serde_json::json!({
            "id": "1.12.2",
            "libraries": [{
                "name": "com.mojang:text2speech:1.10.3",
                "natives": { "windows": "natives-windows" },
                "downloads": { "artifact": { "path": "com/mojang/text2speech/1.10.3/text2speech-1.10.3.jar" } }
            }]
        }));
        touch(
            dir.path(),
            "com/mojang/text2speech/1.10.3/text2speech-1.10.3.jar",
        );

        let cp = build(dir.path(), &json).unwrap();

        assert!(
            cp.iter()
                .any(|p| p.to_string_lossy().contains("text2speech-1.10.3.jar")),
            "带 natives 的库，它的 common jar 也要进 classpath：{:?}",
            cp
        );
    }

    #[test]
    fn test_maven_fallback_honours_classifier() {
        let (dir, json) = fixture(serde_json::json!({
            "id": "1.12.2",
            "libraries": [{ "name": "net.minecraftforge:mergetool:1.1.5:api" }]
        }));
        touch(
            dir.path(),
            "net/minecraftforge/mergetool/1.1.5/mergetool-1.1.5-api.jar",
        );

        let cp = build(dir.path(), &json).unwrap();

        assert!(
            cp.iter()
                .any(|p| p.to_string_lossy().ends_with("mergetool-1.1.5-api.jar")),
            "maven 回退要认 classifier，和安装侧一致：{:?}",
            cp
        );
    }

    /// 只有 classifier 的条目（jinput / twitch-platform）上游没有 common jar
    #[test]
    fn test_classifier_only_entry_is_skipped_not_demanded() {
        let (dir, json) = fixture(serde_json::json!({
            "id": "1.12.2",
            "libraries": [{
                "name": "net.java.jinput:jinput-platform:2.0.5",
                "natives": { "windows": "natives-windows" },
                "downloads": { "classifiers": {
                    "natives-windows": { "path": "net/java/jinput/jinput-platform/2.0.5/jinput-platform-2.0.5-natives-windows.jar" }
                } }
            }]
        }));
        touch(
            dir.path(),
            "net/java/jinput/jinput-platform/2.0.5/jinput-platform-2.0.5-natives-windows.jar",
        );

        let cp = build(dir.path(), &json).unwrap();

        assert!(
            !cp.iter()
                .any(|p| p.to_string_lossy().ends_with("jinput-platform-2.0.5.jar")),
            "不该凭空要求一个不存在的 common jar：{:?}",
            cp
        );
        assert!(
            !cp.iter()
                .any(|p| p.to_string_lossy().contains("natives-windows")),
            "classifier jar 由 natives.rs 解压，不进 classpath"
        );
    }

    #[test]
    fn test_missing_library_fails_with_the_expected_path() {
        let (dir, json) = fixture(serde_json::json!({
            "id": "1.12.2",
            "libraries": [{
                "name": "org.example:gone:1.0",
                "downloads": { "artifact": { "path": "org/example/gone/1.0/gone-1.0.jar" } }
            }]
        }));

        let err = build(dir.path(), &json).expect_err("缺库不该静默丢进 classpath");
        let msg = err.to_string();

        assert!(
            msg.contains("org/example/gone/1.0/gone-1.0.jar"),
            "要说清缺的是哪个文件: {}",
            msg
        );
    }
}
