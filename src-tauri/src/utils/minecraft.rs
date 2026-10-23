//! Minecraft 版本与规则解析工具
//!
//! 版本号解析与新旧分档、Maven 坐标转路径、版本 JSON 的 rules 与 features 求值

/// 1.17+ 或 26.x 新格式视为新版本；用数值解析而非字符串前缀匹配，`1.21.1` 才不会误判
pub fn is_new_version(version: &str) -> bool {
    parse_mc_version(version).is_some_and(|(major, minor)| major > 1 || (major == 1 && minor >= 17))
}

/// 1.12 ~ 1.16.x 视为中等版本
pub fn is_mid_version(version: &str) -> bool {
    parse_mc_version(version).is_some_and(|(major, minor)| major == 1 && (12..=16).contains(&minor))
}

/// 只取前两段数字，忽略 patch 和 `-forge-` 之类后缀；不足两段返回 None
pub fn parse_mc_version(version: &str) -> Option<(u32, u32)> {
    let base = version.split('-').next()?;
    let parts: Vec<&str> = base.split('.').collect();
    if parts.len() < 2 {
        return None;
    }
    let major: u32 = parts[0].parse().ok()?;
    let minor: u32 = parts[1].parse().ok()?;
    Some((major, minor))
}

/// 坐标不足三段返回 None；classifier 缺省时取坐标的第四段
pub fn maven_name_to_path(name: &str, classifier: Option<&str>, extension: &str) -> Option<String> {
    // Maven 的 `@ext` 语法：`net.minecraft:client:1.21.11:mappings@txt`。
    // 不认它就会生成 `client-1.21.11-mappings@txt.jar`，processor 拿去当输出路径必然出错
    let mut extension = extension.to_string();
    let parts: Vec<String> = name
        .split(':')
        .map(|part| match part.split_once('@') {
            Some((head, ext)) if !ext.is_empty() => {
                extension = ext.to_string();
                head.to_string()
            }
            _ => part.to_string(),
        })
        .collect();

    if parts.len() < 3 {
        return None;
    }

    let group = parts[0].replace('.', "/");
    let artifact = &parts[1];
    let version = &parts[2];
    let resolved_classifier = classifier
        .map(str::to_string)
        .or_else(|| parts.get(3).cloned());

    let filename = if let Some(c) = resolved_classifier {
        format!("{}-{}-{}.{}", artifact, version, c, extension)
    } else {
        format!("{}-{}.{}", artifact, version, extension)
    };

    Some(format!("{}/{}/{}/{}", group, artifact, version, filename))
}

/// Minecraft 版本 JSON 用 `osx` 而非 Rust 的 `macos`
pub fn get_mc_os_name() -> &'static str {
    if std::env::consts::OS == "macos" {
        "osx"
    } else {
        std::env::consts::OS
    }
}

/// 库规则：无 rules 默认允许，有 rules 默认禁止，按顺序评估取最后命中项
pub fn evaluate_rules(rules: Option<&serde_json::Value>) -> bool {
    // 库规则不看 features，传默认上下文即可
    evaluate_rules_impl(rules, RuleDefault::Deny, &FeatureSupport::default())
}

/// 参数规则是相反的黑名单：有 rules 也默认允许，只有被 disallow 才排除
/// 本重载不带 features 上下文，任何 features 条件都判为不支持
pub fn evaluate_rules_for_arguments(rules: Option<&serde_json::Value>) -> bool {
    evaluate_rules_for_arguments_with_features(rules, &FeatureSupport::default())
}

/// features 条件不满足时不许放行，否则未替换的 `${resolution_width}` 会直接进游戏
pub fn evaluate_rules_for_arguments_with_features(
    rules: Option<&serde_json::Value>,
    support: &FeatureSupport,
) -> bool {
    evaluate_rules_impl(rules, RuleDefault::Allow, support)
}

/// Minecraft 1.13+ 用 features 控制可选参数，这些字段决定带占位符的参数能否放行
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FeatureSupport {
    /// 用户在设置中指定了自定义窗口分辨率
    pub custom_resolution: bool,
}

enum RuleDefault {
    Allow,
    Deny,
}

fn evaluate_rules_impl(
    rules: Option<&serde_json::Value>,
    default: RuleDefault,
    support: &FeatureSupport,
) -> bool {
    let Some(rules_array) = rules.and_then(|r| r.as_array()) else {
        return true;
    };

    if rules_array.is_empty() {
        return matches!(default, RuleDefault::Allow);
    }

    let mc_os = get_mc_os_name();
    let mut allowed = matches!(default, RuleDefault::Allow);

    for rule in rules_array {
        let action_is_allow = rule["action"].as_str() == Some("allow");

        if let Some(features) = rule.get("features") {
            if check_features(features, support) {
                allowed = action_is_allow;
            } else {
                // 条件不满足时只撤销 allow；disallow 命中不了就保持默认
                if action_is_allow {
                    allowed = false;
                }
            }
            continue;
        }

        if let Some(os) = rule.get("os") {
            let os_name = os["name"].as_str().unwrap_or("");
            if os_name == mc_os {
                allowed = action_is_allow;
            } else {
                allowed = !action_is_allow;
            }
        } else {
            allowed = action_is_allow;
        }
    }

    allowed
}

/// 所有请求的 features 都成立才 true；未知 feature 一律视为不支持
pub fn check_features(features: &serde_json::Value, support: &FeatureSupport) -> bool {
    if let Some(obj) = features.as_object() {
        for (key, value) in obj {
            if value.as_bool() == Some(true) && !is_feature_supported(key.as_str(), support) {
                return false;
            }
        }
    }
    true
}

fn is_feature_supported(name: &str, support: &FeatureSupport) -> bool {
    match name {
        // 用户没配自定义分辨率时不能放行，否则版本 JSON 的占位符替换不了
        "has_custom_resolution" => support.custom_resolution,
        _ => false,
    }
}

/// 优先级 `jar` > `inheritsFrom`（递归）> `id`；父版本 JSON 缺失时直接返回 `inheritsFrom`
pub fn find_jar_version(
    version_json: &serde_json::Value,
    game_dir: &std::path::Path,
) -> Result<String, crate::errors::LauncherError> {
    find_jar_version_at_depth(version_json, game_dir, 0)
}

/// 正常继承链只有一两层；环形 JSON 会无限递归直到栈溢出，深度上限兜底
const MAX_INHERITS_DEPTH: usize = 16;

fn find_jar_version_at_depth(
    version_json: &serde_json::Value,
    game_dir: &std::path::Path,
    depth: usize,
) -> Result<String, crate::errors::LauncherError> {
    if depth > MAX_INHERITS_DEPTH {
        return Err(crate::errors::LauncherError::Custom(
            "版本继承链过深或存在循环".to_string(),
        ));
    }

    if let Some(jar) = version_json["jar"].as_str() {
        return Ok(jar.to_string());
    }

    if let Some(inherits_from) = version_json["inheritsFrom"].as_str() {
        let parent_json_path = game_dir
            .join("versions")
            .join(inherits_from)
            .join(format!("{}.json", inherits_from));

        if parent_json_path.exists() {
            let parent_str = std::fs::read_to_string(&parent_json_path)?;
            let parent_json: serde_json::Value = serde_json::from_str(&parent_str)?;
            return find_jar_version_at_depth(&parent_json, game_dir, depth + 1);
        }

        return Ok(inherits_from.to_string());
    }

    if let Some(id) = version_json["id"].as_str() {
        return Ok(id.to_string());
    }

    Err(crate::errors::LauncherError::Custom(
        "无法确定 JAR 版本".to_string(),
    ))
}

pub fn set_game_language(instance_dir: &std::path::Path, lang: &str) -> std::io::Result<()> {
    let options_path = instance_dir.join("options.txt");
    let content = if options_path.exists() {
        std::fs::read_to_string(&options_path)?
    } else {
        String::new()
    };

    let lang_line = format!("lang:{}", lang);
    let mut lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();
    let mut found = false;

    for line in lines.iter_mut() {
        if line.starts_with("lang:") {
            *line = lang_line.clone();
            found = true;
            break;
        }
    }

    if !found {
        lines.push(lang_line);
    }

    std::fs::write(&options_path, lines.join("\n"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// NeoForge 的 processor 数据里有 `net.minecraft:client:1.21.11:mappings@txt` 这种坐标，
    /// `@` 后面是扩展名而不是分类符
    #[test]
    fn test_maven_name_honours_ext_suffix() {
        assert_eq!(
            maven_name_to_path("net.minecraft:client:1.21.11:mappings@txt", None, "jar").as_deref(),
            Some("net/minecraft/client/1.21.11/client-1.21.11-mappings.txt")
        );
        assert_eq!(
            maven_name_to_path(
                "net.neoforged:neoform:1.21.11:mappings@tsrg.lzma",
                None,
                "jar"
            )
            .as_deref(),
            Some("net/neoforged/neoform/1.21.11/neoform-1.21.11-mappings.tsrg.lzma")
        );
        assert_eq!(
            maven_name_to_path("com.mojang:brigadier:1.3.11", None, "jar").as_deref(),
            Some("com/mojang/brigadier/1.3.11/brigadier-1.3.11.jar")
        );
    }

    #[test]
    fn test_is_new_version() {
        assert!(is_new_version("1.17"));
        assert!(is_new_version("1.17.1"));
        assert!(is_new_version("1.20.4"));
        assert!(is_new_version("1.21"));
        assert!(is_new_version("1.21.1"));
        assert!(is_new_version("1.22"));
        assert!(is_new_version("1.20.2-forge-48.0.48"));
        assert!(is_new_version("26.1.0"));
        assert!(is_new_version("26.1"));
        assert!(is_new_version("26.0.1-fabric-0.16.0"));
        assert!(!is_new_version("1.16.5"));
        assert!(!is_new_version("1.12.2"));
        assert!(!is_new_version("1.7.10"));
    }

    #[test]
    fn test_is_mid_version() {
        assert!(is_mid_version("1.12"));
        assert!(is_mid_version("1.16.5"));
        assert!(is_mid_version("1.14.4"));
        assert!(!is_mid_version("1.17"));
        assert!(!is_mid_version("1.11"));
        assert!(!is_mid_version("1.7.10"));
        assert!(!is_mid_version("26.1.0"));
    }

    #[test]
    fn test_maven_name_to_path_basic() {
        let result = maven_name_to_path("net.minecraftforge:forge:1.20.2-48.0.48", None, "jar");
        assert_eq!(
            result,
            Some("net/minecraftforge/forge/1.20.2-48.0.48/forge-1.20.2-48.0.48.jar".to_string())
        );
    }

    #[test]
    fn test_maven_name_to_path_with_classifier() {
        let result = maven_name_to_path("org.lwjgl:lwjgl:3.3.1", Some("natives-windows"), "jar");
        assert_eq!(
            result,
            Some("org/lwjgl/lwjgl/3.3.1/lwjgl-3.3.1-natives-windows.jar".to_string())
        );
    }

    #[test]
    fn test_maven_name_to_path_invalid() {
        assert_eq!(maven_name_to_path("invalid", None, "jar"), None);
        assert_eq!(maven_name_to_path("only:two", None, "jar"), None);
    }

    #[test]
    fn test_maven_name_to_path_empty() {
        assert_eq!(maven_name_to_path("", None, "jar"), None);
    }

    #[test]
    fn test_maven_name_to_path_extra_parts() {
        let result = maven_name_to_path("com.example:artifact:1.0:class1:class2", None, "jar");
        assert_eq!(
            result,
            Some("com/example/artifact/1.0/artifact-1.0-class1.jar".to_string())
        );
    }

    #[test]
    fn test_maven_name_to_path_no_dots_in_group() {
        let result = maven_name_to_path("mymod:core:2.0.0", None, "jar");
        assert_eq!(result, Some("mymod/core/2.0.0/core-2.0.0.jar".to_string()));
    }

    #[test]
    fn test_maven_name_to_path_custom_extension() {
        let result = maven_name_to_path(
            "net.minecraftforge:forge:1.20.2-48.0.48",
            Some("sources"),
            "zip",
        );
        assert_eq!(
            result,
            Some(
                "net/minecraftforge/forge/1.20.2-48.0.48/forge-1.20.2-48.0.48-sources.zip"
                    .to_string()
            )
        );
    }

    #[test]
    fn test_evaluate_rules_no_rules() {
        assert!(evaluate_rules(None));
    }

    #[test]
    fn test_evaluate_rules_unconditional_allow() {
        let rules = serde_json::json!([{"action": "allow"}]);
        assert!(evaluate_rules(Some(&rules)));
    }

    #[test]
    fn test_evaluate_rules_unconditional_disallow() {
        let rules = serde_json::json!([{"action": "disallow"}]);
        assert!(!evaluate_rules(Some(&rules)));
    }

    #[test]
    fn test_evaluate_rules_os_conditional() {
        let current_mc_os = get_mc_os_name();

        let rules = serde_json::json!([
            {"action": "allow", "os": {"name": current_mc_os}}
        ]);
        assert!(evaluate_rules(Some(&rules)));

        let rules = serde_json::json!([
            {"action": "disallow", "os": {"name": current_mc_os}}
        ]);
        assert!(!evaluate_rules(Some(&rules)));

        // 其他 OS 的 allow 命中不了，保持默认禁止
        let other_os = if current_mc_os == "windows" {
            "osx"
        } else {
            "windows"
        };
        let rules = serde_json::json!([
            {"action": "allow", "os": {"name": other_os}}
        ]);
        assert!(!evaluate_rules(Some(&rules)));
    }

    #[test]
    fn test_evaluate_rules_mixed() {
        let current_mc_os = get_mc_os_name();

        // 先无条件 allow，再条件性 disallow 当前 OS → 最终 disallow
        let rules = serde_json::json!([
            {"action": "allow"},
            {"action": "disallow", "os": {"name": current_mc_os}}
        ]);
        assert!(!evaluate_rules(Some(&rules)));

        // 先无条件 disallow，再条件性 allow 当前 OS → 最终 allow
        let rules = serde_json::json!([
            {"action": "disallow"},
            {"action": "allow", "os": {"name": current_mc_os}}
        ]);
        assert!(evaluate_rules(Some(&rules)));
    }

    #[test]
    fn test_evaluate_rules_empty_array() {
        let rules = serde_json::json!([]);
        assert!(!evaluate_rules(Some(&rules)));
    }

    #[test]
    fn test_evaluate_rules_os_no_match() {
        let other_os = if get_mc_os_name() == "windows" {
            "osx"
        } else {
            "windows"
        };
        let rules = serde_json::json!([
            {"action": "disallow", "os": {"name": other_os}}
        ]);
        assert!(evaluate_rules(Some(&rules)));
    }

    #[test]
    fn test_evaluate_rules_multiple_os_rules() {
        let current_mc_os = get_mc_os_name();

        let rules = serde_json::json!([
            {"action": "allow", "os": {"name": "windows"}},
            {"action": "disallow", "os": {"name": current_mc_os}}
        ]);
        assert!(!evaluate_rules(Some(&rules)));

        let rules = serde_json::json!([
            {"action": "disallow", "os": {"name": "windows"}},
            {"action": "allow", "os": {"name": current_mc_os}}
        ]);
        assert!(evaluate_rules(Some(&rules)));
    }

    #[test]
    fn test_evaluate_rules_invalid_action() {
        let rules = serde_json::json!([
            {"action": "maybe"}
        ]);
        assert!(!evaluate_rules(Some(&rules)));
    }

    #[test]
    fn test_evaluate_rules_missing_action() {
        let rules = serde_json::json!([
            {"os": {"name": "windows"}}
        ]);
        assert!(!evaluate_rules(Some(&rules)));
    }

    #[test]
    fn test_evaluate_rules_os_without_name() {
        let rules = serde_json::json!([
            {"action": "allow", "os": {"arch": "x86"}}
        ]);
        assert!(!evaluate_rules(Some(&rules)));
    }

    #[test]
    fn test_evaluate_rules_non_array() {
        let rules = serde_json::json!({"action": "allow"});
        assert!(evaluate_rules(Some(&rules)));
    }

    #[test]
    fn test_evaluate_rules_for_arguments_no_rules() {
        assert!(evaluate_rules_for_arguments(None));
    }

    #[test]
    fn test_evaluate_rules_for_arguments_default_allow() {
        let rules = serde_json::json!([
            {"action": "disallow", "os": {"name": "osx"}}
        ]);
        let current_mc_os = get_mc_os_name();
        if current_mc_os != "osx" {
            assert!(evaluate_rules_for_arguments(Some(&rules)));
        }
    }

    #[test]
    fn test_evaluate_rules_for_arguments_disallow_current_os() {
        let current_mc_os = get_mc_os_name();
        let rules = serde_json::json!([
            {"action": "disallow", "os": {"name": current_mc_os}}
        ]);
        assert!(!evaluate_rules_for_arguments(Some(&rules)));
    }

    #[test]
    fn test_evaluate_rules_for_arguments_features_custom_resolution_disabled() {
        // 未设置自定义分辨率时不应放行，否则会传入未替换的 ${resolution_width}
        let rules = serde_json::json!([
            {"action": "allow", "features": {"has_custom_resolution": true}}
        ]);
        assert!(!evaluate_rules_for_arguments(Some(&rules)));
    }

    #[test]
    fn test_evaluate_rules_for_arguments_features_custom_resolution_enabled() {
        let rules = serde_json::json!([
            {"action": "allow", "features": {"has_custom_resolution": true}}
        ]);
        let support = FeatureSupport {
            custom_resolution: true,
        };
        assert!(evaluate_rules_for_arguments_with_features(
            Some(&rules),
            &support
        ));
    }

    #[test]
    fn test_evaluate_rules_for_arguments_features_unsupported() {
        let rules = serde_json::json!([
            {"action": "allow", "features": {"is_demo_user": true}}
        ]);
        let support = FeatureSupport {
            custom_resolution: true,
        };
        assert!(!evaluate_rules_for_arguments(Some(&rules)));
        assert!(!evaluate_rules_for_arguments_with_features(
            Some(&rules),
            &support
        ));
    }

    #[test]
    fn test_evaluate_rules_for_arguments_empty_array() {
        let rules = serde_json::json!([]);
        assert!(evaluate_rules_for_arguments(Some(&rules)));
    }

    #[test]
    fn test_evaluate_rules_for_arguments_allow_on_osx_only() {
        let rules = serde_json::json!([
            {"action": "allow", "os": {"name": "osx"}}
        ]);
        let current_mc_os = get_mc_os_name();
        if current_mc_os == "osx" {
            assert!(evaluate_rules_for_arguments(Some(&rules)));
        } else {
            assert!(!evaluate_rules_for_arguments(Some(&rules)));
        }
    }

    #[test]
    fn test_evaluate_rules_allow_on_osx_only_library() {
        let rules = serde_json::json!([
            {"action": "allow", "os": {"name": "osx"}}
        ]);
        let current_mc_os = get_mc_os_name();
        if current_mc_os == "osx" {
            assert!(evaluate_rules(Some(&rules)));
        } else {
            assert!(!evaluate_rules(Some(&rules)));
        }
    }

    #[test]
    fn test_check_features_supported() {
        let features = serde_json::json!({"has_custom_resolution": true});
        let support = FeatureSupport {
            custom_resolution: true,
        };
        assert!(check_features(&features, &support));
        assert!(!check_features(&features, &FeatureSupport::default()));
    }

    #[test]
    fn test_check_features_unsupported() {
        let features = serde_json::json!({"is_demo_user": true});
        let support = FeatureSupport {
            custom_resolution: true,
        };
        assert!(!check_features(&features, &support));
    }

    #[test]
    fn test_check_features_mixed() {
        let features = serde_json::json!({"has_custom_resolution": true, "is_demo_user": true});
        let support = FeatureSupport {
            custom_resolution: true,
        };
        assert!(!check_features(&features, &support));
    }

    #[test]
    fn test_get_mc_os_name() {
        let os = get_mc_os_name();
        assert!(
            os == "windows" || os == "linux" || os == "osx",
            "未知的 OS 名称: {}",
            os
        );
    }

    #[test]
    fn test_set_game_language_create_new_file() {
        let dir = tempfile::tempdir().unwrap();
        set_game_language(dir.path(), "zh_cn").unwrap();

        let content = std::fs::read_to_string(dir.path().join("options.txt")).unwrap();
        assert!(content.contains("lang:zh_cn"));
    }

    #[test]
    fn test_set_game_language_update_existing() {
        let dir = tempfile::tempdir().unwrap();
        let options_path = dir.path().join("options.txt");
        std::fs::write(&options_path, "lang:en_us\nrenderDistance:12\n").unwrap();

        set_game_language(dir.path(), "zh_cn").unwrap();

        let content = std::fs::read_to_string(&options_path).unwrap();
        assert!(content.contains("lang:zh_cn"));
        assert!(!content.contains("lang:en_us"));
        assert!(content.contains("renderDistance:12"));
    }

    #[test]
    fn test_set_game_language_append_to_existing() {
        let dir = tempfile::tempdir().unwrap();
        let options_path = dir.path().join("options.txt");
        std::fs::write(&options_path, "renderDistance:12\nfov:70\n").unwrap();

        set_game_language(dir.path(), "ja_jp").unwrap();

        let content = std::fs::read_to_string(&options_path).unwrap();
        assert!(content.contains("lang:ja_jp"));
        assert!(content.contains("renderDistance:12"));
        assert!(content.contains("fov:70"));
    }

    #[test]
    fn test_set_game_language_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let options_path = dir.path().join("options.txt");
        std::fs::write(&options_path, "").unwrap();

        set_game_language(dir.path(), "en_us").unwrap();

        let content = std::fs::read_to_string(&options_path).unwrap();
        assert!(content.contains("lang:en_us"));
    }

    #[test]
    fn test_set_game_language_update_preserves_other_lang_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let options_path = dir.path().join("options.txt");
        std::fs::write(&options_path, "lang:en_us\nlangServer:en_us\n").unwrap();

        set_game_language(dir.path(), "zh_cn").unwrap();

        let content = std::fs::read_to_string(&options_path).unwrap();
        assert!(content.contains("lang:zh_cn"));
        assert!(content.contains("langServer:en_us"));
    }
}
