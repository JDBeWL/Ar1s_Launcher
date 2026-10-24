//! 实例级覆盖解析
//!
//! 本次启动临时选的 Java 优先于实例设置，实例设置优先于全局。

use crate::models::{GameConfig, JvmAdvancedOptions, LaunchOptions};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveLaunchOverrides {
    pub java_path: Option<String>,
    pub memory_mb: Option<u32>,
    pub jvm_advanced: JvmAdvancedOptions,
    pub window_width: Option<u32>,
    pub window_height: Option<u32>,
}

pub fn resolve_overrides(config: &GameConfig, options: &LaunchOptions) -> EffectiveLaunchOverrides {
    let settings = options.instance_settings.as_ref();

    let mut jvm_advanced = config.jvm_advanced.clone();
    if let Some(extra_args) = settings.and_then(|settings| settings.jvm_extra_args.as_ref()) {
        jvm_advanced.extra_args = extra_args.clone();
    }

    EffectiveLaunchOverrides {
        java_path: options
            .override_java_path
            .clone()
            .or_else(|| settings.and_then(|settings| settings.java_path.clone())),
        memory_mb: settings.and_then(|settings| settings.memory_mb),
        jvm_advanced,
        window_width: settings
            .and_then(|settings| settings.window_width)
            .or(options.window_width)
            .or(config.window_width),
        window_height: settings
            .and_then(|settings| settings.window_height)
            .or(options.window_height)
            .or(config.window_height),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{GameConfig, InstanceSettings, LaunchOptions};

    fn base_options() -> LaunchOptions {
        LaunchOptions {
            version: "1.20.1".to_string(),
            username: "Steve".to_string(),
            memory: Some(2048),
            window_width: Some(1280),
            window_height: Some(720),
            fullscreen: None,
            auth_type: None,
            access_token: None,
            uuid: None,
            override_java_path: None,
            instance_settings: None,
            force_java_mismatch: false,
        }
    }

    #[test]
    fn follows_global_without_instance_settings() {
        let config = GameConfig {
            max_memory: 4096,
            window_width: Some(1920),
            window_height: Some(1080),
            ..GameConfig::default()
        };
        let options = base_options();
        let effective = resolve_overrides(&config, &options);

        assert_eq!(effective.java_path, None);
        assert_eq!(effective.memory_mb, None);
        assert_eq!(effective.window_width, Some(1280));
        assert_eq!(effective.window_height, Some(720));
        assert_eq!(effective.jvm_advanced, config.jvm_advanced);
    }

    #[test]
    fn instance_values_win_over_global() {
        let config = GameConfig {
            max_memory: 4096,
            window_width: Some(1920),
            window_height: Some(1080),
            ..GameConfig::default()
        };
        let mut options = base_options();
        options.memory = Some(2048);
        options.instance_settings = Some(InstanceSettings {
            java_path: Some("C:/instance-java/bin/java.exe".to_string()),
            memory_mb: Some(8192),
            jvm_extra_args: Some("-Dinstance=1".to_string()),
            window_width: Some(1600),
            window_height: None,
        });

        let effective = resolve_overrides(&config, &options);
        assert_eq!(
            effective.java_path.as_deref(),
            Some("C:/instance-java/bin/java.exe")
        );
        assert_eq!(effective.memory_mb, Some(8192));
        assert_eq!(effective.jvm_advanced.extra_args, "-Dinstance=1");
        assert_eq!(effective.window_width, Some(1600));
        assert_eq!(effective.window_height, Some(720));
    }

    #[test]
    fn temporary_java_wins_over_instance_java() {
        let config = GameConfig::default();
        let mut options = base_options();
        options.override_java_path = Some("C:/temp-java/bin/java.exe".to_string());
        options.instance_settings = Some(InstanceSettings {
            java_path: Some("C:/instance-java/bin/java.exe".to_string()),
            ..InstanceSettings::default()
        });

        let effective = resolve_overrides(&config, &options);
        assert_eq!(
            effective.java_path.as_deref(),
            Some("C:/temp-java/bin/java.exe")
        );
    }

    #[test]
    fn empty_instance_extra_args_clear_global_extra_args() {
        let config = GameConfig {
            jvm_advanced: JvmAdvancedOptions {
                extra_args: "-Dglobal=1".to_string(),
                ..JvmAdvancedOptions::default()
            },
            ..GameConfig::default()
        };
        let mut options = base_options();
        options.instance_settings = Some(InstanceSettings {
            jvm_extra_args: Some(String::new()),
            ..InstanceSettings::default()
        });

        assert_eq!(
            resolve_overrides(&config, &options).jvm_advanced.extra_args,
            ""
        );
    }
}
