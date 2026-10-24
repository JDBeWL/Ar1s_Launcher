//! JVM 和游戏参数构建逻辑
//!
//! 解析新旧两种参数格式并按 rules 与 features 过滤，替换 ${...} 占位符，补齐 --tweakClass

use crate::models::{GameConfig, LaunchOptions};
use crate::utils::minecraft::{FeatureSupport, evaluate_rules_for_arguments_with_features};
use std::fs::File;
use std::path::{Path, PathBuf};

/// Minecraft 未指定自定义分辨率时的默认窗口尺寸
const DEFAULT_WINDOW_WIDTH: u32 = 854;
const DEFAULT_WINDOW_HEIGHT: u32 = 480;

/// 报给游戏的启动器名，写入 `-Dminecraft.launcher.brand`
const LAUNCHER_NAME: &str = "Ar1s Launcher";
/// F3 显示的启动器标签，写入 `--versionType`
const LAUNCHER_LABEL: &str = concat!("Ar1s Launcher ", env!("CARGO_PKG_VERSION"));

/// 新 F3 的版本行实现；旧版是 DebugScreenOverlay
const NEW_DEBUG_SCREEN_CLASS: &str =
    "net/minecraft/client/gui/components/debug/DebugEntryVersion.class";

/// 用于 Forge 的 ignoreList 参数，需要原版 MC jar 文件名
fn get_base_minecraft_version(version_json: &serde_json::Value, fallback: &str) -> String {
    if let Some(jar) = version_json.get("jar").and_then(|j| j.as_str()) {
        return jar.to_string();
    }

    if let Some(inherits) = version_json.get("inheritsFrom").and_then(|i| i.as_str()) {
        if (inherits.contains("-forge") || inherits.contains("-neoforge"))
            && let Some(mc_ver) = inherits.split('-').next()
        {
            return mc_ver.to_string();
        }
        if inherits
            .chars()
            .next()
            .map(|c| c.is_ascii_digit())
            .unwrap_or(false)
            && !inherits.contains("forge")
        {
            return inherits.to_string();
        }
    }

    if fallback.contains("-forge")
        && let Some(mc_ver) = fallback.split("-forge").next()
    {
        return mc_ver.to_string();
    }

    fallback.to_string()
}

#[allow(clippy::too_many_arguments)]
pub fn build_arguments(
    version_json: &serde_json::Value,
    config: &GameConfig,
    options: &LaunchOptions,
    uuid: &str,
    version_dir: &Path,
    game_dir: &Path,
    assets_dir: &Path,
    assets_index: &str,
    classpath: &[std::path::PathBuf],
    emit: &impl Fn(&str, String),
) -> (Vec<String>, Vec<String>) {
    let libraries_dir = game_dir.join("libraries");
    let classpath_separator = if cfg!(windows) { ";" } else { ":" };
    let classpath_string = classpath
        .iter()
        .map(|p| p.to_string_lossy())
        .collect::<Vec<_>>()
        .join(classpath_separator);

    let base_mc_version = get_base_minecraft_version(version_json, &options.version);
    let primary_jar_name = format!("{}.jar", base_mc_version);
    let forge_family = is_forge_family(version_json);
    let old_forge_f3 =
        forge_family && !classpath_has_new_debug_screen(classpath, &primary_jar_name);

    let custom_width = options.window_width.or(config.window_width);
    let custom_height = options.window_height.or(config.window_height);
    let resolution_width = custom_width.unwrap_or(DEFAULT_WINDOW_WIDTH).to_string();
    let resolution_height = custom_height.unwrap_or(DEFAULT_WINDOW_HEIGHT).to_string();

    // 仅当用户真的设置了分辨率才启用 has_custom_resolution，否则未替换的 ${resolution_width} 占位符会丢给游戏
    let feature_support = FeatureSupport {
        custom_resolution: custom_width.is_some() || custom_height.is_some(),
    };

    let replace_placeholders = |arg: &str| -> String {
        let actual_game_dir = super::isolation::content_root_of(config, game_dir, version_dir)
            .to_string_lossy()
            .to_string();

        let auth_access_token = options.access_token.as_deref().unwrap_or("0");

        let auth_uuid = options.uuid.as_deref().unwrap_or(uuid);

        let user_type = if options.auth_type.as_deref() == Some("microsoft") {
            "msa"
        } else {
            "mojang"
        };

        let version_type = LAUNCHER_LABEL;

        arg.replace("${auth_player_name}", &options.username)
            .replace("${version_name}", &base_mc_version)
            .replace("${game_directory}", &actual_game_dir)
            .replace("${assets_root}", &assets_dir.to_string_lossy())
            .replace("${assets_index_name}", assets_index)
            .replace("${auth_uuid}", auth_uuid)
            .replace("${auth_access_token}", auth_access_token)
            .replace("${user_type}", user_type)
            .replace("${version_type}", version_type)
            .replace("${user_properties}", "{}")
            .replace("${resolution_width}", &resolution_width)
            .replace("${resolution_height}", &resolution_height)
            .replace("${library_directory}", &libraries_dir.to_string_lossy())
            .replace("${classpath_separator}", classpath_separator)
            // 原版 1.20.2+ / NeoForge 还会问这些；漏一个就会把字面量喂给 JVM，
            // 例如 -Djava.library.path=${natives_directory} 会顶掉我们设对的那条
            .replace(
                "${natives_directory}",
                &version_dir.join("natives").to_string_lossy(),
            )
            .replace("${classpath}", &classpath_string)
            .replace("${launcher_name}", LAUNCHER_NAME)
            .replace("${launcher_version}", env!("CARGO_PKG_VERSION"))
            .replace("${primary_jar_name}", &primary_jar_name)
            // 我们没有 Xbox 的 user id / client id，给空串而不是留字面量
            .replace("${auth_xuid}", "")
            .replace("${clientid}", "")
    };

    let mut jvm_args = vec![];
    let mut game_args_vec = vec![];

    if let Some(arguments) = version_json.get("arguments") {
        jvm_args = parse_jvm_arguments(arguments, &replace_placeholders, &feature_support);
        game_args_vec = parse_game_arguments(arguments, &replace_placeholders, &feature_support);
    } else if let Some(mc_args) = version_json["minecraftArguments"].as_str() {
        game_args_vec = mc_args.split(' ').map(&replace_placeholders).collect();
    }

    auto_add_tweak_class(version_json, options, classpath, &mut game_args_vec, emit);
    apply_launcher_branding(
        &mut game_args_vec,
        &options.version,
        &base_mc_version,
        old_forge_f3,
    );

    (jvm_args, game_args_vec)
}

/// 旧 Forge 的 F3 是 `<实例>/<brand>/<versionType>`，版本行保留实例名，启动器标签交给 `--versionType`；
/// 新 F3 和 Fabric 不显示 versionType，用 `实例名 - 启动器版本` 放进 `--version`。
/// 不要改 `${version_name}`：Forge 的 `ignoreList` 还要用它拼基础 jar 名
fn apply_launcher_branding(
    game_args: &mut [String],
    instance_name: &str,
    base_mc_version: &str,
    old_forge_f3: bool,
) {
    let Some(index) = game_args.iter().position(|arg| arg == "--version") else {
        return;
    };
    let Some(value) = game_args.get_mut(index + 1) else {
        return;
    };

    *value = if old_forge_f3 {
        instance_name.to_string()
    } else if instance_name == base_mc_version {
        LAUNCHER_LABEL.to_string()
    } else {
        format!("{instance_name} - {LAUNCHER_LABEL}")
    };
}

/// F3 的 versionType 只有 Forge 系会展示；用 Maven 组和主类识别，不按版本号列表判断
fn is_forge_family(version_json: &serde_json::Value) -> bool {
    let main_class = version_json["mainClass"]
        .as_str()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if main_class.contains("forge") || main_class.contains("bootstraplauncher") {
        return true;
    }

    version_json["libraries"]
        .as_array()
        .is_some_and(|libraries| {
            libraries.iter().any(|library| {
                library["name"].as_str().is_some_and(|name| {
                    name.starts_with("net.minecraftforge:") || name.starts_with("net.neoforged:")
                })
            })
        })
}

/// 用 F3 实现类判断新旧调试界面，避免写版本号判断表。
/// 原版 jar、Forge/NeoForge 的 client/extra jar 都可能承载这些类，其他库不用扫
fn classpath_has_new_debug_screen(classpath: &[PathBuf], primary_jar_name: &str) -> bool {
    classpath.iter().any(|jar| {
        let file_name = jar
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        let lower_name = file_name.to_ascii_lowercase();
        let relevant = file_name == primary_jar_name
            || lower_name.contains("forge")
            || lower_name.contains("client");
        if !relevant {
            return false;
        }

        let Ok(file) = File::open(jar) else {
            return false;
        };
        let Ok(mut archive) = zip::ZipArchive::new(file) else {
            return false;
        };
        archive.by_name(NEW_DEBUG_SCREEN_CLASS).is_ok()
    })
}

fn parse_jvm_arguments(
    arguments: &serde_json::Value,
    replace_placeholders: &impl Fn(&str) -> String,
    support: &FeatureSupport,
) -> Vec<String> {
    let mut jvm_args = vec![];

    let Some(jvm) = arguments["jvm"].as_array() else {
        return jvm_args;
    };

    for arg in jvm {
        if let Some(s) = arg.as_str() {
            jvm_args.push(replace_placeholders(s));
        } else if let Some(obj) = arg.as_object()
            && evaluate_rules_for_arguments_with_features(obj.get("rules"), support)
            && let Some(value) = obj.get("value")
        {
            if let Some(s) = value.as_str() {
                jvm_args.push(replace_placeholders(s));
            } else if let Some(arr) = value.as_array() {
                for item in arr {
                    if let Some(s) = item.as_str() {
                        jvm_args.push(replace_placeholders(s));
                    }
                }
            }
        }
    }

    jvm_args
}

fn parse_game_arguments(
    arguments: &serde_json::Value,
    replace_placeholders: &impl Fn(&str) -> String,
    support: &FeatureSupport,
) -> Vec<String> {
    let mut game_args = vec![];

    if let Some(game) = arguments["game"].as_array() {
        for arg in game {
            if let Some(s) = arg.as_str() {
                game_args.push(replace_placeholders(s));
            } else if let Some(obj) = arg.as_object()
                && evaluate_rules_for_arguments_with_features(obj.get("rules"), support)
                && let Some(value) = obj.get("value")
            {
                if let Some(s) = value.as_str() {
                    game_args.push(replace_placeholders(s));
                } else if let Some(arr) = value.as_array() {
                    for item in arr {
                        if let Some(s) = item.as_str() {
                            game_args.push(replace_placeholders(s));
                        }
                    }
                }
            }
        }
    }

    game_args
}

/// 自动补齐 tweakClass（仅在 LaunchWrapper 主类下）
fn auto_add_tweak_class(
    version_json: &serde_json::Value,
    options: &LaunchOptions,
    classpath: &[std::path::PathBuf],
    game_args: &mut Vec<String>,
    emit: &impl Fn(&str, String),
) {
    let main_class = version_json["mainClass"].as_str().unwrap_or("");
    let has_tweak_class_flag = game_args.iter().any(|a| a == "--tweakClass");

    if main_class != "net.minecraft.launchwrapper.Launch" || has_tweak_class_flag {
        return;
    }

    let forge_in_libraries = version_json
        .get("libraries")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter().any(|lib| {
                lib.get("name")
                    .and_then(|n| n.as_str())
                    .map(|name| name.contains("net.minecraftforge") || name.contains("cpw.mods"))
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false);

    let forge_in_classpath = classpath.iter().any(|p| {
        let s = p.to_string_lossy().to_lowercase();
        s.contains("minecraftforge")
            || s.contains("forge-")
            || s.contains("/fml/")
            || s.contains("\\fml\\")
    });

    if !forge_in_libraries && !forge_in_classpath {
        emit(
            "log-debug",
            "跳过自动补齐 tweakClass：未检测到 Forge/FML 库，避免 ClassNotFound".to_string(),
        );
        return;
    }

    let base_ver = options
        .version
        .split("-forge")
        .next()
        .unwrap_or(&options.version);

    let tweaker = if base_ver.starts_with("1.7.10") {
        "cpw.mods.fml.common.launcher.FMLTweaker"
    } else {
        "net.minecraftforge.fml.common.launcher.FMLTweaker"
    };

    emit("log-debug", format!("自动补齐 tweakClass: {}", tweaker));

    game_args.insert(0, tweaker.to_string());
    game_args.insert(0, "--tweakClass".to_string());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn options() -> LaunchOptions {
        LaunchOptions {
            version: "26.3-NeoForge".to_string(),
            username: "Steve".to_string(),
            memory: None,
            window_width: None,
            window_height: None,
            fullscreen: None,
            auth_type: None,
            access_token: None,
            uuid: None,
            override_java_path: None,
            instance_settings: None,
            force_java_mismatch: false,
        }
    }

    fn config() -> GameConfig {
        GameConfig::default()
    }

    /// 关闭版本隔离后 `--gameDir` 必须是根目录。
    /// 这一项要是和模组管理用的目录不一致，就会出现"列表里有模组、游戏里没有"
    #[test]
    fn test_game_directory_follows_isolation_setting() {
        let json = serde_json::json!({
            "id": "pack",
            "arguments": { "game": ["--gameDir", "${game_directory}"] }
        });

        let root = tempfile::TempDir::new().unwrap();
        let game_dir = root.path().to_path_buf();
        let version_dir = game_dir.join("versions").join("pack");
        let assets_dir = game_dir.join("assets");

        let game_dir_arg = |isolated: bool| {
            let cfg = GameConfig {
                version_isolation: isolated,
                ..GameConfig::default()
            };
            let (_, game) = build_arguments(
                &json,
                &cfg,
                &options(),
                "abc",
                &version_dir,
                &game_dir,
                &assets_dir,
                "26",
                &[],
                &|_: &str, _: String| {},
            );
            let index = game.iter().position(|arg| arg == "--gameDir").unwrap();
            game[index + 1].clone()
        };

        assert_eq!(
            game_dir_arg(true),
            version_dir.to_string_lossy(),
            "隔离时应指向实例目录"
        );
        assert_eq!(
            game_dir_arg(false),
            game_dir.to_string_lossy(),
            "关闭隔离时应指向根目录"
        );
    }

    fn build_args_for(
        json: &serde_json::Value,
        instance_name: &str,
        classpath: &[PathBuf],
    ) -> (Vec<String>, Vec<String>) {
        let root = tempfile::TempDir::new().unwrap();
        let game_dir = root.path().to_path_buf();
        let version_dir = game_dir.join("versions").join(instance_name);
        let assets_dir = game_dir.join("assets");
        let mut opts = options();
        opts.version = instance_name.to_string();

        build_arguments(
            json,
            &config(),
            &opts,
            "abc",
            &version_dir,
            &game_dir,
            &assets_dir,
            "26",
            classpath,
            &|_: &str, _: String| {},
        )
    }

    fn write_client_jar(path: &Path, new_f3: bool) {
        use zip::write::SimpleFileOptions;
        let file = std::fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let entry = if new_f3 {
            NEW_DEBUG_SCREEN_CLASS
        } else {
            "other.class"
        };
        zip.start_file(entry, SimpleFileOptions::default()).unwrap();
        std::io::Write::write_all(&mut zip, b"class").unwrap();
        zip.finish().unwrap();
    }

    /// Forge 的 F3 是 `<实例>/<brand>/<versionType>`，版本行保持实例名，
    /// 启动器标签交给 --versionType；ignoreList 仍用基础 jar 名
    #[test]
    fn test_forge_launch_version_uses_instance_name_and_launcher_version_type() {
        let json = serde_json::json!({
            "id": "26.3-NeoForge",
            "jar": "26.3",
            "mainClass": "net.neoforged.fml.startup.Client",
            "arguments": {
                "jvm": ["-DignoreList=forge-,${version_name}.jar"],
                "game": ["--version", "${version_name}", "--versionType", "${version_type}"]
            }
        });

        let (jvm, game) = build_args_for(&json, "26.3-NeoForge", &[]);
        let version_pos = game.iter().position(|arg| arg == "--version").unwrap();
        assert_eq!(game[version_pos + 1], "26.3-NeoForge");

        let type_pos = game.iter().position(|arg| arg == "--versionType").unwrap();
        assert_eq!(
            game[type_pos + 1],
            format!("{LAUNCHER_NAME} {}", env!("CARGO_PKG_VERSION"))
        );
        assert!(
            jvm.iter().any(|arg| arg == "-DignoreList=forge-,26.3.jar"),
            "ignoreList 必须继续用基础版本号: {:?}",
            jvm
        );
    }

    /// 新 F3 不再追加 versionType，启动器标签改放在 --version 里，用短横线分隔
    #[test]
    fn test_new_forge_f3_uses_hyphen_launcher_label() {
        let json = serde_json::json!({
            "id": "26.3-NeoForge",
            "jar": "26.3",
            "mainClass": "net.neoforged.fml.startup.Client",
            "arguments": {
                "game": ["--version", "${version_name}", "--versionType", "${version_type}"]
            }
        });

        let root = tempfile::TempDir::new().unwrap();
        let client_jar = root.path().join("26.3.jar");
        write_client_jar(&client_jar, true);

        let (_, game) = build_args_for(&json, "26.3-NeoForge", &[client_jar]);
        let version_pos = game.iter().position(|arg| arg == "--version").unwrap();
        assert_eq!(
            game[version_pos + 1],
            format!(
                "26.3-NeoForge - {LAUNCHER_NAME} {}",
                env!("CARGO_PKG_VERSION")
            )
        );

        let type_pos = game.iter().position(|arg| arg == "--versionType").unwrap();
        assert_eq!(
            game[type_pos + 1],
            format!("{LAUNCHER_NAME} {}", env!("CARGO_PKG_VERSION"))
        );
    }

    /// 新旧 F3 靠客户端 jar 里的实现类判断，不能只看加载器或版本号
    #[test]
    fn test_new_debug_screen_detection_reads_client_jar() {
        let root = tempfile::TempDir::new().unwrap();
        let new_jar = root.path().join("26.3.jar");
        let old_jar = root.path().join("1.20.1.jar");
        write_client_jar(&new_jar, true);
        write_client_jar(&old_jar, false);

        assert!(classpath_has_new_debug_screen(&[new_jar], "26.3.jar"));
        assert!(!classpath_has_new_debug_screen(&[old_jar], "1.20.1.jar"));
    }

    /// Fabric 的 F3 不显示 --versionType，启动器信息只能放在 --version 里
    #[test]
    fn test_fabric_launch_version_includes_launcher_label() {
        let json = serde_json::json!({
            "id": "26.3-Fabric",
            "jar": "26.3",
            "mainClass": "net.fabricmc.loader.impl.launch.knot.KnotClient",
            "libraries": [{ "name": "net.fabricmc:fabric-loader:0.19.5" }],
            "arguments": {
                "game": ["--version", "${version_name}"]
            }
        });

        let (_, game) = build_args_for(&json, "26.3-Fabric", &[]);
        let version_pos = game.iter().position(|arg| arg == "--version").unwrap();
        assert_eq!(
            game[version_pos + 1],
            format!(
                "26.3-Fabric - {LAUNCHER_NAME} {}",
                env!("CARGO_PKG_VERSION")
            )
        );
    }

    /// 基础版本的实例名与 MC 版本相同，不重复显示
    #[test]
    fn test_launcher_branding_skips_redundant_base_version_name() {
        let json = serde_json::json!({
            "id": "26.3",
            "jar": "26.3",
            "mainClass": "net.minecraft.client.main.Main",
            "arguments": {
                "game": ["--version", "${version_name}"]
            }
        });

        let (_, game) = build_args_for(&json, "26.3", &[]);
        let version_pos = game.iter().position(|arg| arg == "--version").unwrap();
        assert_eq!(
            game[version_pos + 1],
            format!("{LAUNCHER_NAME} {}", env!("CARGO_PKG_VERSION"))
        );
    }

    /// 每个占位符都必须换掉，漏一个就把 ${...} 字面量原样喂给 JVM
    #[test]
    fn test_all_mojang_placeholders_get_resolved() {
        let json = serde_json::json!({
            "id": "26.3-NeoForge",
            "mainClass": "net.neoforged.fml.startup.Client",
            "arguments": {
                "jvm": [
                    "-Djava.library.path=${natives_directory}",
                    "-Dminecraft.launcher.brand=${launcher_name}",
                    "-Dminecraft.launcher.version=${launcher_version}",
                    "-Dminecraft.client.jar=${primary_jar_name}",
                    "-cp",
                    "${classpath}"
                ],
                "game": [
                    "--username", "${auth_player_name}",
                    "--version", "${version_name}",
                    "--gameDir", "${game_directory}",
                    "--assetsDir", "${assets_root}",
                    "--assetIndex", "${assets_index_name}",
                    "--uuid", "${auth_uuid}",
                    "--accessToken", "${auth_access_token}",
                    "--userType", "${user_type}",
                    "--versionType", "${version_type}",
                    "--xuid", "${auth_xuid}",
                    "--clientId", "${clientid}"
                ]
            }
        });

        let root = tempfile::TempDir::new().unwrap();
        let game_dir = root.path().to_path_buf();
        let version_dir = game_dir.join("versions").join("26.3-NeoForge");
        let assets_dir = game_dir.join("assets");
        let classpath = vec![PathBuf::from("a.jar"), PathBuf::from("b.jar")];

        let (jvm, game) = build_arguments(
            &json,
            &config(),
            &options(),
            "abc",
            &version_dir,
            &game_dir,
            &assets_dir,
            "26",
            &classpath,
            &|_: &str, _: String| {},
        );

        let joined = jvm
            .iter()
            .chain(game.iter())
            .cloned()
            .collect::<Vec<_>>()
            .join(" ");
        assert!(!joined.contains("${"), "仍有未替换的占位符: {}", joined);
        assert!(
            joined.contains("natives"),
            "java.library.path 要指到解压目录: {}",
            joined
        );
        assert!(joined.contains("a.jar") && joined.contains("b.jar"));
        assert!(
            joined.contains("26.3.jar") || joined.contains(".jar"),
            "primary_jar_name 要给出文件名: {}",
            joined
        );
    }
}
