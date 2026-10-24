use serde::{Deserialize, Serialize};

use crate::errors::LauncherError;

pub const MIN_INSTANCE_MEMORY_MB: u32 = 512;
pub const MAX_INSTANCE_MEMORY_MB: u32 = 262_144;
pub const MIN_INSTANCE_WINDOW_WIDTH: u32 = 640;
pub const MAX_INSTANCE_WINDOW_WIDTH: u32 = 7680;
pub const MIN_INSTANCE_WINDOW_HEIGHT: u32 = 480;
pub const MAX_INSTANCE_WINDOW_HEIGHT: u32 = 4320;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct InstanceSettings {
    pub java_path: Option<String>,
    pub memory_mb: Option<u32>,
    pub jvm_extra_args: Option<String>,
    pub window_width: Option<u32>,
    pub window_height: Option<u32>,
}

impl InstanceSettings {
    pub fn normalized(&self) -> Self {
        let trim_path = |value: &Option<String>| {
            value
                .as_deref()
                .map(str::trim)
                .filter(|path| !path.is_empty())
                .map(str::to_string)
        };

        Self {
            java_path: trim_path(&self.java_path),
            memory_mb: self.memory_mb,
            // 空串是“明确不追加”，不能归一化成 None
            jvm_extra_args: self
                .jvm_extra_args
                .as_ref()
                .map(|args| args.trim().to_string()),
            window_width: self.window_width,
            window_height: self.window_height,
        }
    }

    pub fn validate(&self) -> Result<(), LauncherError> {
        if let Some(memory) = self.memory_mb
            && !(MIN_INSTANCE_MEMORY_MB..=MAX_INSTANCE_MEMORY_MB).contains(&memory)
        {
            return Err(LauncherError::Custom(format!(
                "实例内存必须在 {} 到 {} MB 之间",
                MIN_INSTANCE_MEMORY_MB, MAX_INSTANCE_MEMORY_MB
            )));
        }
        if let Some(width) = self.window_width
            && !(MIN_INSTANCE_WINDOW_WIDTH..=MAX_INSTANCE_WINDOW_WIDTH).contains(&width)
        {
            return Err(LauncherError::Custom(format!(
                "实例窗口宽度必须在 {} 到 {} 之间",
                MIN_INSTANCE_WINDOW_WIDTH, MAX_INSTANCE_WINDOW_WIDTH
            )));
        }
        if let Some(height) = self.window_height
            && !(MIN_INSTANCE_WINDOW_HEIGHT..=MAX_INSTANCE_WINDOW_HEIGHT).contains(&height)
        {
            return Err(LauncherError::Custom(format!(
                "实例窗口高度必须在 {} 到 {} 之间",
                MIN_INSTANCE_WINDOW_HEIGHT, MAX_INSTANCE_WINDOW_HEIGHT
            )));
        }
        Ok(())
    }

    pub fn jvm_extra_args_or<'a>(&'a self, fallback: &'a str) -> &'a str {
        self.jvm_extra_args.as_deref().unwrap_or(fallback)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_follow_global() {
        let settings = InstanceSettings::default();
        assert_eq!(
            settings,
            InstanceSettings {
                java_path: None,
                memory_mb: None,
                jvm_extra_args: None,
                window_width: None,
                window_height: None,
            }
        );
    }

    #[test]
    fn normalizes_empty_values() {
        let settings = InstanceSettings {
            java_path: Some("  ".to_string()),
            memory_mb: Some(4096),
            jvm_extra_args: Some("  -Dfoo=bar  ".to_string()),
            window_width: Some(1280),
            window_height: Some(720),
        };

        let normalized = settings.normalized();
        assert_eq!(normalized.java_path, None);
        assert_eq!(normalized.jvm_extra_args.as_deref(), Some("-Dfoo=bar"));
    }

    #[test]
    fn keeps_explicit_empty_jvm_args() {
        let settings = InstanceSettings {
            jvm_extra_args: Some(String::new()),
            ..InstanceSettings::default()
        };
        assert_eq!(settings.normalized().jvm_extra_args.as_deref(), Some(""));
    }

    #[test]
    fn rejects_out_of_range_values() {
        let too_small_memory = InstanceSettings {
            memory_mb: Some(511),
            ..InstanceSettings::default()
        };
        assert!(too_small_memory.validate().is_err());

        let too_narrow = InstanceSettings {
            window_width: Some(639),
            ..InstanceSettings::default()
        };
        assert!(too_narrow.validate().is_err());

        let valid = InstanceSettings {
            memory_mb: Some(4096),
            window_width: Some(1280),
            window_height: Some(720),
            ..InstanceSettings::default()
        };
        assert!(valid.validate().is_ok());
    }

    #[test]
    fn jvm_extra_args_falls_back_to_global() {
        let following = InstanceSettings::default();
        assert_eq!(following.jvm_extra_args_or("global"), "global");

        let overridden = InstanceSettings {
            jvm_extra_args: Some("-Dfoo=bar".to_string()),
            ..InstanceSettings::default()
        };
        assert_eq!(overridden.jvm_extra_args_or("global"), "-Dfoo=bar");
    }
}
