//! 版本 JSON 的库解析
//!
//! 下载、classpath、修复校验共用这一份规则：rules、natives、classifiers-only 与旧 Forge 的 maven 坐标回退

use crate::utils::minecraft::{evaluate_rules, get_mc_os_name, maven_name_to_path};
use serde_json::Value;
use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryFile {
    pub relative_path: String,
    /// 官方下载地址；maven 回退和本地校验不依赖它
    pub url: Option<String>,
    pub size: u64,
    pub sha1: Option<String>,
    pub is_native: bool,
    /// 路径由 maven 坐标推导，没有 downloads 元数据；下载侧可走镜像的 /maven 路径
    pub from_maven: bool,
}

/// 解析当前平台实际需要的库文件；natives 只取当前 OS 的 classifier，
/// 只有 classifiers 没有 artifact 的条目不会伪造主 jar
pub fn resolve_library_files(version_json: &Value) -> Vec<LibraryFile> {
    let mut files = Vec::new();
    let mut seen = HashSet::new();
    let Some(libraries) = version_json.get("libraries").and_then(Value::as_array) else {
        return files;
    };
    let os = get_mc_os_name();
    let arch = if cfg!(target_pointer_width = "64") {
        "64"
    } else {
        "32"
    };

    for lib in libraries {
        if !evaluate_rules(lib.get("rules")) {
            continue;
        }
        let name = lib.get("name").and_then(Value::as_str);

        if let Some(artifact) = lib.get("downloads").and_then(|d| d.get("artifact")) {
            if let Some(path) = artifact.get("path").and_then(Value::as_str) {
                push_unique(
                    &mut files,
                    &mut seen,
                    LibraryFile {
                        relative_path: path.to_string(),
                        url: artifact
                            .get("url")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        size: artifact.get("size").and_then(Value::as_u64).unwrap_or(0),
                        sha1: artifact
                            .get("sha1")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        is_native: false,
                        from_maven: false,
                    },
                );
            }
        } else if lib.get("downloads").is_none()
            && let Some(name) = name
            && let Some(path) = maven_name_to_path(name, None, "jar")
        {
            push_unique(
                &mut files,
                &mut seen,
                LibraryFile {
                    url: Some(maven_url(lib, &path)),
                    relative_path: path,
                    size: 0,
                    sha1: None,
                    is_native: false,
                    from_maven: true,
                },
            );
        }

        let Some(natives) = lib.get("natives").and_then(Value::as_object) else {
            continue;
        };
        let Some(classifier) = natives.get(os).and_then(Value::as_str) else {
            continue;
        };
        let classifier = classifier.replace("${arch}", arch);

        if let Some(artifact) = lib
            .get("downloads")
            .and_then(|d| d.get("classifiers"))
            .and_then(|c| c.get(&classifier))
            .or_else(|| lib.get("classifiers").and_then(|c| c.get(&classifier)))
        {
            let declared_path = artifact.get("path").and_then(Value::as_str);
            let declared_url = artifact.get("url").and_then(Value::as_str);
            let path = declared_path
                .map(str::to_string)
                .or_else(|| name.and_then(|n| native_path_from_name(n, &classifier)));
            if let Some(path) = path {
                let from_maven = declared_path.is_none() || declared_url.is_none();
                push_unique(
                    &mut files,
                    &mut seen,
                    LibraryFile {
                        url: if from_maven {
                            Some(native_url(&path))
                        } else {
                            declared_url.map(str::to_string)
                        },
                        relative_path: path,
                        size: artifact.get("size").and_then(Value::as_u64).unwrap_or(0),
                        sha1: artifact
                            .get("sha1")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        is_native: true,
                        from_maven,
                    },
                );
            }
        } else if let Some(name) = name
            && let Some(path) = native_path_from_name(name, &classifier)
        {
            push_unique(
                &mut files,
                &mut seen,
                LibraryFile {
                    url: Some(native_url(&path)),
                    relative_path: path,
                    size: 0,
                    sha1: None,
                    is_native: true,
                    from_maven: true,
                },
            );
        }
    }

    files
}

fn push_unique(files: &mut Vec<LibraryFile>, seen: &mut HashSet<String>, file: LibraryFile) {
    if seen.insert(file.relative_path.clone()) {
        files.push(file);
    }
}

fn maven_url(lib: &Value, path: &str) -> String {
    match lib.get("url").and_then(Value::as_str) {
        Some(base) => join_url(base, path),
        None => format!("https://repo1.maven.org/maven2/{path}"),
    }
}

fn native_url(path: &str) -> String {
    format!("https://libraries.minecraft.net/{path}")
}

fn join_url(base: &str, path: &str) -> String {
    if base.ends_with('/') {
        format!("{}{}", base, path)
    } else {
        format!("{}/{}", base, path)
    }
}

/// 没有 classifier 元数据时的路径回退；lwjgl 2.x 的 natives 在 lwjgl-platform 坐标下
fn native_path_from_name(name: &str, classifier: &str) -> Option<String> {
    let parts: Vec<&str> = name.split(':').collect();
    if parts.len() < 3 {
        return None;
    }

    let group = parts[0].replace('.', "/");
    let artifact = parts[1];
    let version = parts[2];

    if artifact == "lwjgl" {
        Some(format!(
            "{}/{}-platform/{}/{}-platform-{}-{}.jar",
            group, artifact, version, artifact, version, classifier
        ))
    } else {
        Some(format!(
            "{}/{}/{}/{}-{}-{}.jar",
            group, artifact, version, artifact, version, classifier
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::resolve_library_files;
    use crate::utils::minecraft::get_mc_os_name;
    use serde_json::{Value, json};

    fn os() -> &'static str {
        get_mc_os_name()
    }

    fn native_classifier() -> String {
        format!("natives-{}", os())
    }

    fn other_os() -> &'static str {
        if os() == "windows" {
            "linux"
        } else {
            "windows"
        }
    }

    #[test]
    fn test_classifiers_only_library_has_no_fabricated_main_jar() {
        let classifier = native_classifier();
        let path =
            format!("org/lwjgl/lwjgl/lwjgl-platform/2.9.2/lwjgl-platform-2.9.2-{classifier}.jar");
        let version = json!({
            "libraries": [{
                "name": "org.lwjgl.lwjgl:lwjgl-platform:2.9.2",
                "natives": { os(): classifier },
                "downloads": {
                    "classifiers": {
                        classifier.clone(): { "path": path, "url": format!("https://libraries.minecraft.net/{path}") }
                    }
                }
            }]
        });

        let files = resolve_library_files(&version);

        assert_eq!(files.len(), 1);
        assert!(files[0].is_native);
        assert_eq!(files[0].relative_path, path);
    }

    #[test]
    fn test_native_library_keeps_common_artifact() {
        let classifier = native_classifier();
        let main = "com/example/native/1.0/native-1.0.jar";
        let native = format!("com/example/native/1.0/native-1.0-{classifier}.jar");
        let version = json!({
            "libraries": [{
                "name": "com.example:native:1.0",
                "natives": { os(): classifier },
                "downloads": {
                    "artifact": { "path": main, "url": format!("https://libraries.minecraft.net/{main}") },
                    "classifiers": {
                        classifier.clone(): { "path": native, "url": format!("https://libraries.minecraft.net/{native}") }
                    }
                }
            }]
        });

        let files = resolve_library_files(&version);

        assert_eq!(files.len(), 2);
        assert!(
            files
                .iter()
                .any(|f| !f.is_native && f.relative_path == main)
        );
        assert!(
            files
                .iter()
                .any(|f| f.is_native && f.relative_path == native)
        );
    }

    #[test]
    fn test_natives_only_use_current_os() {
        let version = json!({
            "libraries": [{
                "name": "com.example:native:1.0",
                "natives": { os(): "current", other_os(): "other" },
                "downloads": {
                    "classifiers": {
                        "current": { "path": "current.jar", "url": "https://libraries.minecraft.net/current.jar" },
                        "other": { "path": "other.jar", "url": "https://libraries.minecraft.net/other.jar" }
                    }
                }
            }]
        });

        let files = resolve_library_files(&version);

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].relative_path, "current.jar");
    }

    #[test]
    fn test_rules_filter_libraries() {
        let version = json!({
            "libraries": [
                {
                    "name": "com.example:allowed:1.0",
                    "rules": [{ "action": "allow", "os": { "name": os() } }]
                },
                {
                    "name": "com.example:denied:1.0",
                    "rules": [{ "action": "allow", "os": { "name": other_os() } }]
                }
            ]
        });

        let files = resolve_library_files(&version);

        assert_eq!(files.len(), 1);
        assert!(files[0].relative_path.ends_with("allowed-1.0.jar"));
    }

    #[test]
    fn test_maven_fallback_uses_library_url_base() {
        let version = json!({
            "libraries": [{
                "name": "net.minecraftforge:forge:1.20.1-47.2.0",
                "url": "https://maven.minecraftforge.net/"
            }]
        });

        let files = resolve_library_files(&version);

        assert_eq!(files.len(), 1);
        assert!(files[0].from_maven);
        assert_eq!(
            files[0].relative_path,
            "net/minecraftforge/forge/1.20.1-47.2.0/forge-1.20.1-47.2.0.jar"
        );
        assert_eq!(
            files[0].url.as_deref(),
            Some(
                "https://maven.minecraftforge.net/net/minecraftforge/forge/1.20.1-47.2.0/forge-1.20.1-47.2.0.jar"
            )
        );
    }

    #[test]
    fn test_lwjgl_platform_native_path_from_name() {
        let version = json!({
            "libraries": [{
                "name": "org.lwjgl.lwjgl:lwjgl-platform:2.9.1",
                "natives": { os(): "natives-classifier" }
            }]
        });

        let files = resolve_library_files(&version);

        assert!(files.iter().any(|f| {
            f.is_native
                && f.relative_path
                    == "org/lwjgl/lwjgl/lwjgl-platform/2.9.1/lwjgl-platform-2.9.1-natives-classifier.jar"
        }));
    }

    #[test]
    fn test_arch_placeholder_is_replaced() {
        let arch = if cfg!(target_pointer_width = "64") {
            "64"
        } else {
            "32"
        };
        let classifier = format!("natives-{arch}");
        let path = format!("com/example/native/1.0/native-1.0-{classifier}.jar");
        let version = json!({
            "libraries": [{
                "name": "com.example:native:1.0",
                "natives": { os(): "natives-${arch}" },
                "downloads": {
                    "classifiers": {
                        classifier: { "path": path, "url": format!("https://libraries.minecraft.net/{path}") }
                    }
                }
            }]
        });

        let files: Vec<_> = resolve_library_files(&version);

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].relative_path, path);
    }

    #[test]
    fn test_duplicate_artifact_paths_are_deduped() {
        let artifact: Value = json!({
            "path": "com/example/tool/1.0/tool-1.0.jar",
            "url": "https://libraries.minecraft.net/com/example/tool/1.0/tool-1.0.jar"
        });
        let version = json!({
            "libraries": [
                { "name": "com.example:tool:1.0", "downloads": { "artifact": artifact.clone() } },
                { "name": "com.example:tool:1.0", "downloads": { "artifact": artifact } }
            ]
        });

        assert_eq!(resolve_library_files(&version).len(), 1);
    }
}
