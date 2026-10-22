//! 敏感文件权限收紧
//!
//! 配置文件里有正版 token，落盘后尽量只让当前用户读到；这是加固而不是前置条件，
//! 失败只记日志，不能让保存流程失败

use std::path::Path;

/// 已经收紧过权限的路径。
///
/// `restrict_impl` 会拉起 `icacls` 子进程，一次几十毫秒，而配置写入是高频操作。
/// DACL 用 `/inheritance:r` 砍掉继承后，覆写同一路径不会把继承加回来——实测 `MoveFileEx`
/// 覆盖已存在文件时保留的是被改名文件的显式 ACE，所以「同一路径只收紧一次」不留可读窗口
#[cfg(windows)]
static RESTRICTED: std::sync::Mutex<Vec<std::path::PathBuf>> = std::sync::Mutex::new(Vec::new());

/// 把文件权限收紧到当前用户（Windows 额外保留 SYSTEM 与管理员）
pub fn restrict_to_current_user(path: &Path) {
    restrict_impl(path);
}

/// Unix 下把权限位设成 0600；失败返回错误，由调用方决定是否忽略
#[cfg(unix)]
pub fn set_owner_only(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
pub fn set_owner_only(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

/// Windows 没有 unix 的 0600：收紧 DACL，只保留当前用户、SYSTEM 与管理员
#[cfg(windows)]
fn restrict_impl(path: &Path) {
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    // 只做一次，重复拉起子进程纯属浪费。锁中毒时直接重算，不值得为它 panic
    if RESTRICTED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains(&path.to_path_buf())
    {
        return;
    }

    let user = match (std::env::var("USERDOMAIN"), std::env::var("USERNAME")) {
        (Ok(domain), Ok(name)) if !domain.is_empty() && !name.is_empty() => {
            format!("{}\\{}", domain, name)
        }
        // 域/用户名取不到时不做任何改动，保持原有继承权限
        _ => return,
    };

    let mut command = Command::new("icacls");
    command
        .arg(path)
        .arg("/inheritance:r")
        .creation_flags(CREATE_NO_WINDOW);
    // 用户按名字给，SYSTEM 与 Administrators 用 SID，避免系统语言不同导致名称解析失败
    for grant in [
        format!("{}:F", user),
        "*S-1-5-18:F".to_string(),
        "*S-1-5-32-544:F".to_string(),
    ] {
        command.arg("/grant:r").arg(grant);
    }

    match command.output() {
        Ok(output) if output.status.success() => {
            // 只有真收紧成功才记下来；失败下回还要再试
            RESTRICTED
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(path.to_path_buf());
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            log::warn!("收紧配置文件权限失败: {}", stderr.trim());
        }
        Err(error) => log::warn!("无法调用 icacls 收紧配置文件权限: {}", error),
    }
}

#[cfg(not(windows))]
fn restrict_impl(_path: &Path) {}

#[cfg(test)]
mod tests {
    use super::set_owner_only;
    use std::fs;

    #[cfg(unix)]
    #[test]
    fn test_set_owner_only_strips_group_and_other_bits() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("ar1s.json");
        fs::write(&path, b"{}").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();

        set_owner_only(&path).unwrap();

        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[cfg(not(unix))]
    #[test]
    fn test_set_owner_only_is_a_noop_outside_unix() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("ar1s.json");
        fs::write(&path, b"{}").unwrap();

        assert!(set_owner_only(&path).is_ok());
    }
}
