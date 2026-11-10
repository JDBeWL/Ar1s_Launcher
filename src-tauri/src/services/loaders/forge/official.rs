//! 跑官方 installer jar

use super::{CREATE_NO_WINDOW, INSTALLER_TIMEOUT};
use crate::errors::LauncherError;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::process::Command;

pub(crate) fn check_cancelled(cancel: Option<&AtomicBool>) -> Result<(), LauncherError> {
    if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
        return Err(LauncherError::Cancelled);
    }
    Ok(())
}

pub(crate) async fn wait_for_cancel(flag: &AtomicBool) {
    while !flag.load(Ordering::Relaxed) {
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// 安装器和 processor 都不能无限挂起；取消或超时会 drop 子进程句柄，
/// 配合 kill_on_drop 把 JVM 一起收掉
pub(crate) async fn run_limited_command(
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

pub(crate) async fn run_official_installer(
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
