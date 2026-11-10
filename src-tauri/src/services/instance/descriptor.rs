//! 从版本 JSON 的声明里认出加载器与版本号

use serde_json::Value;

/// Forge/NeoForge 把加载器信息写在 arguments.game 的 `--fml.*` 参数里，
/// 旧版 JSON 则在 minecraftArguments 字符串中
pub(super) fn parse_fml_arguments(
    json_value: &Value,
) -> (Option<String>, Option<String>, Option<String>) {
    let args = match json_value
        .get("arguments")
        .and_then(|a| a.get("game"))
        .and_then(|g| g.as_array())
    {
        Some(arr) => arr,
        None => {
            let raw_args = match json_value["minecraftArguments"].as_str() {
                Some(s) => s,
                None => return (None, None, None),
            };
            let tokens: Vec<&str> = raw_args.split_whitespace().collect();
            let mut forge_ver: Option<String> = None;
            let mut mc_ver: Option<String> = None;
            let mut is_neoforge = false;

            for (i, token) in tokens.iter().enumerate() {
                match *token {
                    "--fml.forgeVersion" => {
                        if i + 1 < tokens.len() {
                            forge_ver = Some(tokens[i + 1].to_string());
                        }
                    }
                    "--fml.neoForgeVersion" => {
                        if i + 1 < tokens.len() {
                            forge_ver = Some(tokens[i + 1].to_string());
                            is_neoforge = true;
                        }
                    }
                    "--fml.mcVersion" if i + 1 < tokens.len() => {
                        mc_ver = Some(tokens[i + 1].to_string());
                    }
                    _ => {}
                }
            }

            if forge_ver.is_some() || mc_ver.is_some() {
                let lt = if is_neoforge { "NeoForge" } else { "Forge" };
                return (Some(lt.to_string()), forge_ver, mc_ver);
            }
            return (None, None, None);
        }
    };

    let mut forge_ver: Option<String> = None;
    let mut mc_ver: Option<String> = None;
    let mut is_neoforge = false;

    for (i, arg) in args.iter().enumerate() {
        let token = match arg.as_str() {
            Some(s) => s,
            None => continue,
        };

        match token {
            "--fml.forgeVersion" => {
                if let Some(next) = args.get(i + 1).and_then(|v| v.as_str()) {
                    forge_ver = Some(next.to_string());
                }
            }
            "--fml.neoForgeVersion" => {
                if let Some(next) = args.get(i + 1).and_then(|v| v.as_str()) {
                    forge_ver = Some(next.to_string());
                    is_neoforge = true;
                }
            }
            "--fml.mcVersion" => {
                if let Some(next) = args.get(i + 1).and_then(|v| v.as_str()) {
                    mc_ver = Some(next.to_string());
                }
            }
            _ => {}
        }
    }

    if forge_ver.is_some() || mc_ver.is_some() {
        let lt = if is_neoforge { "NeoForge" } else { "Forge" };
        return (Some(lt.to_string()), forge_ver, mc_ver);
    }

    (None, None, None)
}

/// Fabric/Quilt 的版本 JSON 用 patches 分别记录 game 与加载器条目
pub(super) fn parse_patches(
    json_value: &Value,
) -> (Option<String>, Option<String>, Option<String>) {
    let patches = match json_value["patches"].as_array() {
        Some(arr) => arr,
        None => return (None, None, None),
    };

    let mut game_version: Option<String> = None;
    let mut loader_type: Option<String> = None;
    let mut loader_version: Option<String> = None;

    for patch in patches {
        let id = match patch["id"].as_str() {
            Some(s) => s,
            None => continue,
        };
        let version = patch["version"].as_str().map(String::from);

        match id {
            "game" => {
                game_version = version;
            }
            "fabric" => {
                loader_type = Some("Fabric".to_string());
                loader_version = version;
            }
            "quilt" => {
                loader_type = Some("Quilt".to_string());
                loader_version = version;
            }
            "forge" => {
                loader_type = Some("Forge".to_string());
                loader_version = version;
            }
            "neoforge" => {
                loader_type = Some("NeoForge".to_string());
                loader_version = version;
            }
            _ => {}
        }
    }

    if loader_type.is_some() || game_version.is_some() {
        return (loader_type, loader_version, game_version);
    }

    (None, None, None)
}

/// 按 Maven 坐标的 group/artifact 识别各加载器
pub(super) fn parse_loader_from_libraries(json_value: &Value) -> (Option<String>, Option<String>) {
    let libs = match json_value["libraries"].as_array() {
        Some(arr) => arr,
        None => return (None, None),
    };

    for lib in libs {
        let name = match lib["name"].as_str() {
            Some(n) => n,
            None => continue,
        };

        let parts: Vec<&str> = name.split(':').collect();
        if parts.len() < 3 {
            continue;
        }

        let group = parts[0];
        let artifact = parts[1];
        let version = parts[2];

        if group == "net.minecraftforge" && artifact == "forge" {
            let loader_ver = version
                .split('-')
                .next_back()
                .unwrap_or(version)
                .to_string();
            return (Some("Forge".to_string()), Some(loader_ver));
        }

        if group == "net.minecraftforge" && artifact == "fmlloader" {
            let loader_ver = version
                .split('-')
                .next_back()
                .unwrap_or(version)
                .to_string();
            return (Some("Forge".to_string()), Some(loader_ver));
        }

        if group == "net.fabricmc" && artifact == "fabric-loader" {
            return (Some("Fabric".to_string()), Some(version.to_string()));
        }

        if group == "org.quiltmc" && artifact == "quilt-loader" {
            return (Some("Quilt".to_string()), Some(version.to_string()));
        }

        if group == "net.neoforged" && (artifact == "neoforge" || artifact == "fml") {
            let loader_ver = version
                .split('-')
                .next_back()
                .unwrap_or(version)
                .to_string();
            return (Some("NeoForge".to_string()), Some(loader_ver));
        }
    }

    (None, None)
}
