//! 实例级设置文件
//!
//! 字段为 None 表示跟随全局；损坏或越界的文件按默认值处理，不阻塞启动。

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::errors::LauncherError;
use crate::models::InstanceSettings;
use crate::services::config;
use crate::utils::file_utils;

pub const SETTINGS_FILE_NAME: &str = "instance-settings.json";

pub(crate) fn instance_dir(instance_name: &str) -> Result<PathBuf, LauncherError> {
    let config = config::load_config()?;
    let game_dir = PathBuf::from(&config.game_dir);
    file_utils::instance_dir(&game_dir.join("versions"), instance_name)
}

pub fn load(instance_name: &str) -> Result<InstanceSettings, LauncherError> {
    let dir = instance_dir(instance_name)?;
    read_from_instance_dir(&dir)
}

pub fn save(instance_name: &str, settings: InstanceSettings) -> Result<(), LauncherError> {
    let settings = settings.normalized();
    settings.validate()?;
    let dir = instance_dir(instance_name)?;
    write_to_instance_dir(&dir, &settings)
}

pub(crate) fn read_from_instance_dir(dir: &Path) -> Result<InstanceSettings, LauncherError> {
    let path = dir.join(SETTINGS_FILE_NAME);
    if !path.exists() {
        return Ok(InstanceSettings::default());
    }

    let content = fs::read_to_string(&path)?;
    match serde_json::from_str::<InstanceSettings>(&content) {
        Ok(settings) => {
            let settings = settings.normalized();
            if let Err(error) = settings.validate() {
                log::warn!(
                    "实例设置值无效，按跟随全局处理: {} ({})",
                    path.display(),
                    error
                );
                return Ok(InstanceSettings::default());
            }
            Ok(settings)
        }
        Err(error) => {
            let backup = path.with_extension("json.bak");
            let _ = fs::copy(&path, &backup);
            log::warn!(
                "实例设置文件无法解析，已备份到 {} 并按跟随全局处理: {}",
                backup.display(),
                error
            );
            Ok(InstanceSettings::default())
        }
    }
}

pub(crate) fn write_to_instance_dir(
    dir: &Path,
    settings: &InstanceSettings,
) -> Result<(), LauncherError> {
    fs::create_dir_all(dir)?;
    let path = dir.join(SETTINGS_FILE_NAME);
    let json = serde_json::to_string_pretty(settings)?;
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    tmp.write_all(json.as_bytes())?;
    tmp.flush()?;
    tmp.persist(&path).map_err(|e| e.error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::InstanceSettings;

    #[test]
    fn missing_file_means_follow_global() {
        let dir = tempfile::TempDir::new().unwrap();
        let settings = read_from_instance_dir(dir.path()).unwrap();
        assert_eq!(settings, InstanceSettings::default());
    }

    #[test]
    fn writes_and_reads_round_trip() {
        let dir = tempfile::TempDir::new().unwrap();
        let settings = InstanceSettings {
            java_path: Some("C:/Java/bin/java.exe".to_string()),
            memory_mb: Some(6144),
            jvm_extra_args: Some("-Dfoo=bar".to_string()),
            window_width: Some(1600),
            window_height: Some(900),
        };

        write_to_instance_dir(dir.path(), &settings).unwrap();
        let restored = read_from_instance_dir(dir.path()).unwrap();
        assert_eq!(restored, settings);
    }

    #[test]
    fn corrupt_file_is_backed_up_and_defaults_to_follow_global() {
        let dir = tempfile::TempDir::new().unwrap();
        fs::write(dir.path().join(SETTINGS_FILE_NAME), b"not json").unwrap();

        let settings = read_from_instance_dir(dir.path()).unwrap();
        assert_eq!(settings, InstanceSettings::default());
        assert!(dir.path().join("instance-settings.json.bak").exists());
    }

    #[test]
    fn normalizes_on_write() {
        let dir = tempfile::TempDir::new().unwrap();
        let settings = InstanceSettings {
            java_path: Some("  ".to_string()),
            ..InstanceSettings::default()
        };
        write_to_instance_dir(dir.path(), &settings.normalized()).unwrap();
        let restored = read_from_instance_dir(dir.path()).unwrap();
        assert_eq!(restored.java_path, None);
    }

    #[test]
    fn out_of_range_file_falls_back_to_follow_global() {
        let dir = tempfile::TempDir::new().unwrap();
        fs::write(dir.path().join(SETTINGS_FILE_NAME), r#"{"memoryMb":1}"#).unwrap();
        let restored = read_from_instance_dir(dir.path()).unwrap();
        assert_eq!(restored, InstanceSettings::default());
    }
}
