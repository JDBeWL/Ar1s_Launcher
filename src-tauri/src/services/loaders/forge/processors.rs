//! processor 执行器：解析参数、选择 Java、跑进程

#[cfg(windows)]
use super::CREATE_NO_WINDOW;
use super::PROCESSOR_TIMEOUT;
use super::*;
use crate::errors::LauncherError;
use crate::services::launcher::java::JavaCandidate;
use crate::utils::minecraft::maven_name_to_path;
use log::{debug, info};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use tokio::process::Command;

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
