use super::fetch::existing_library_is_valid;
use super::java::{choose_installer_java, choose_processor_java};
use super::legacy::loader_jar_present;
use super::mirror::{forge_installer_sources, mirror_urls};
use super::modern::{resolve_processor_arg, runs_on_client};
use super::official::run_limited_command;
use super::processors::ProcessorVars;
use super::versions::{find_installed_loader_dir, snapshot_version_dirs};
use crate::services::download::source::{BMCL_BASE, DownloadSource, HKMC_MAIN_BASE};
use crate::services::launcher::java::{JavaArch, JavaCandidate};
use std::sync::atomic::AtomicBool;
use std::time::Duration;
use tokio::process::Command;

#[cfg(windows)]
use super::CREATE_NO_WINDOW;

fn slow_command() -> Command {
    #[cfg(windows)]
    {
        let mut cmd = Command::new("ping");
        cmd.args(["-n", "10", "127.0.0.1"]);
        cmd.creation_flags(CREATE_NO_WINDOW);
        cmd
    }
    #[cfg(not(windows))]
    {
        let mut cmd = Command::new("sleep");
        cmd.arg("5");
        cmd
    }
}

fn cand(path: &str, version: u32) -> JavaCandidate {
    JavaCandidate {
        path: path.to_string(),
        version,
        arch: JavaArch::Bits64,
        is_ea: false,
        from_config: false,
    }
}

#[test]
fn test_existing_library_accepts_matching_sha1() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("lib.jar");
    let mut bytes = vec![0u8; 200];
    bytes[..4].copy_from_slice(b"PK\x03\x04");
    std::fs::write(&path, &bytes).unwrap();

    let sha1 = crate::utils::file_utils::sha1_hex(&bytes);
    assert!(existing_library_is_valid(&path, Some(&sha1)).unwrap());
    assert!(!existing_library_is_valid(&path, Some(&"0".repeat(40))).unwrap());
}

#[test]
fn test_existing_library_without_sha1_requires_zip_header() {
    let tmp = tempfile::tempdir().unwrap();
    let jar = tmp.path().join("lib.jar");
    let mut bytes = vec![0u8; 200];
    bytes[..4].copy_from_slice(b"PK\x03\x04");
    std::fs::write(&jar, &bytes).unwrap();
    assert!(existing_library_is_valid(&jar, None).unwrap());

    // 半截文件 / 镜像错误页没有 ZIP 头，必须重新下载
    let broken = tmp.path().join("broken.jar");
    std::fs::write(&broken, "<html>error</html>").unwrap();
    assert!(!existing_library_is_valid(&broken, None).unwrap());

    // 空文件同样不能算已安装
    let empty = tmp.path().join("empty.jar");
    std::fs::write(&empty, b"").unwrap();
    assert!(!existing_library_is_valid(&empty, None).unwrap());
}

#[test]
fn test_find_installed_loader_dir_accepts_the_expected_id() {
    let tmp = tempfile::tempdir().unwrap();
    let versions = tmp.path().join("versions");
    let dir = versions.join("1.20.1-forge-47.2.0");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("1.20.1-forge-47.2.0.json"), "{}").unwrap();

    let before = snapshot_version_dirs(&versions);
    assert_eq!(
        find_installed_loader_dir(&versions, &before, "1.20.1-forge-47.2.0", "1.20.1").as_deref(),
        Some("1.20.1-forge-47.2.0")
    );
}

/// 1.7.10 的安装器把 id 写成大小写不同的形式，认不出来实例就退化成原版
#[test]
fn test_find_installed_loader_dir_falls_back_to_new_dirs() {
    let tmp = tempfile::tempdir().unwrap();
    let versions = tmp.path().join("versions");
    std::fs::create_dir_all(versions.join("1.7.10")).unwrap();

    let before = snapshot_version_dirs(&versions);

    let installed = versions.join("1.7.10-Forge10.13.4.1614");
    std::fs::create_dir_all(&installed).unwrap();
    std::fs::write(
        installed.join("1.7.10-Forge10.13.4.1614.json"),
        serde_json::json!({
            "id": "1.7.10-Forge10.13.4.1614",
            "inheritsFrom": "1.7.10",
            "libraries": [{ "name": "net.minecraftforge:forge:1.7.10-10.13.4.1614" }]
        })
        .to_string(),
    )
    .unwrap();

    assert_eq!(
        find_installed_loader_dir(&versions, &before, "1.7.10-forge-10.13.4.1614", "1.7.10")
            .as_deref(),
        Some("1.7.10-Forge10.13.4.1614")
    );
}

#[test]
fn test_find_installed_loader_dir_ignores_unrelated_new_dirs() {
    let tmp = tempfile::tempdir().unwrap();
    let versions = tmp.path().join("versions");
    std::fs::create_dir_all(&versions).unwrap();
    let before = snapshot_version_dirs(&versions);

    // 装 Forge 期间可能顺带装出来的原版目录，不该被认成加载器
    let vanilla = versions.join("1.20.1");
    std::fs::create_dir_all(&vanilla).unwrap();
    std::fs::write(
        vanilla.join("1.20.1.json"),
        serde_json::json!({ "id": "1.20.1", "libraries": [] }).to_string(),
    )
    .unwrap();

    assert_eq!(
        find_installed_loader_dir(&versions, &before, "1.20.1-forge-47.2.0", "1.20.1"),
        None
    );
}

#[test]
fn test_installer_java_prefers_the_exact_version() {
    let installer = std::path::Path::new("C:/nonexistent/installer.jar");
    let candidates = vec![
        cand("C:/jdk8/bin/java.exe", 8),
        cand("C:/jdk17/bin/java.exe", 17),
    ];

    // 1.12.2 的安装器只能在 8 上跑，17 会 UnsupportedClassVersionError
    assert_eq!(
        choose_installer_java(installer, "1.12.2", &candidates).as_deref(),
        Some("C:/jdk8/bin/java.exe")
    );

    // 1.20.1 要 17；没有 17 时只能往上取最近的一份
    assert_eq!(
        choose_installer_java(installer, "1.20.1", &candidates).as_deref(),
        Some("C:/jdk17/bin/java.exe")
    );
    let only_newer = vec![
        cand("C:/jdk8/bin/java.exe", 8),
        cand("C:/jdk21/bin/java.exe", 21),
    ];
    assert_eq!(
        choose_installer_java(installer, "1.20.1", &only_newer).as_deref(),
        Some("C:/jdk21/bin/java.exe")
    );
}

#[test]
fn test_installer_java_is_none_when_nothing_is_new_enough() {
    let installer = std::path::Path::new("C:/nonexistent/installer.jar");
    // 1.21.4 的安装器要 21，本机只有 8 和 17，跑不了就直接走手动安装
    let candidates = vec![
        cand("C:/jdk8/bin/java.exe", 8),
        cand("C:/jdk17/bin/java.exe", 17),
    ];

    assert_eq!(
        choose_installer_java(installer, "1.21.4", &candidates),
        None
    );
}

#[test]
fn test_loader_jar_present_accepts_both_coordinate_forms() {
    let tmp = tempfile::tempdir().unwrap();
    let libs = tmp.path().join("libraries");

    // 1.7.10 的坐标多一段 -{mc} 后缀，按自己拼的路径去查会漏
    let legacy = libs.join("net/minecraftforge/forge/1.7.10-10.13.4.1614-1.7.10");
    std::fs::create_dir_all(&legacy).unwrap();
    std::fs::write(
        legacy.join("forge-1.7.10-10.13.4.1614-1.7.10-universal.jar"),
        b"x",
    )
    .unwrap();

    assert!(loader_jar_present(&libs, "1.7.10", "10.13.4.1614"));
    assert!(!loader_jar_present(&libs, "1.7.10", "10.13.4.1615"));
    assert!(!loader_jar_present(&libs, "1.12.2", "14.23.5.2860"));
}

#[test]
fn test_installer_sources_cover_both_coordinate_forms() {
    // 实测边界：1.7.10 / 1.8.9 在 maven 上只有带 -{mc} 后缀的坐标
    let old = forge_installer_sources("1.8.9", "11.15.1.2318");
    assert!(
        old.iter()
            .any(|u| u
                .contains("1.8.9-11.15.1.2318-1.8.9/forge-1.8.9-11.15.1.2318-1.8.9-installer.jar")),
        "1.8.9 必须有带后缀的候选：{:?}",
        old
    );

    // 1.9 起改成不带后缀
    let new = forge_installer_sources("1.12.2", "14.23.5.2861");
    assert!(
        new.iter()
            .any(|u| u.ends_with("1.12.2-14.23.5.2861/forge-1.12.2-14.23.5.2861-installer.jar")),
        "1.12.2 必须有不带后缀的候选：{:?}",
        new
    );

    // 先试实测有效的那种，省掉整轮无谓重试
    assert!(
        old[0].contains("1.8.9-11.15.1.2318-1.8.9/forge-1.8.9-11.15.1.2318-1.8.9-installer.jar"),
        "1.8.9 应优先带后缀：{:?}",
        old
    );
    assert!(
        new[0].contains("1.12.2-14.23.5.2861/forge-1.12.2-14.23.5.2861-installer.jar"),
        "1.12.2 应优先不带后缀：{:?}",
        new
    );

    let f19 = forge_installer_sources("1.9", "12.16.1.1934");
    assert!(
        f19[0].contains("/1.9-12.16.1.1934/forge-1.9-12.16.1.1934-installer.jar"),
        "1.9 应优先不带后缀：{:?}",
        f19
    );
}

#[test]
fn test_choose_processor_java_keeps_config_java_when_it_suffices() {
    let candidates = vec![
        cand("C:/zulu-26/bin/java.exe", 26),
        cand("C:/lib17/bin/java.exe", 17),
    ];

    let picked =
        choose_processor_java(17, ("C:/lib17/bin/java.exe", Some(17)), &candidates).unwrap();

    assert_eq!(picked, "C:/lib17/bin/java.exe");
}

#[test]
fn test_choose_processor_java_picks_lowest_sufficient_candidate() {
    let candidates = vec![
        cand("C:/zulu-26/bin/java.exe", 26),
        cand("C:/lib-21/bin/java.exe", 21),
        cand("C:/lib17/bin/java.exe", 17),
    ];

    // 配置里那份只有 17，跑不动要 25 的 processor
    let picked =
        choose_processor_java(25, ("C:/lib17/bin/java.exe", Some(17)), &candidates).unwrap();

    assert_eq!(picked, "C:/zulu-26/bin/java.exe");
}

#[test]
fn test_choose_processor_java_fails_loudly_when_nothing_is_new_enough() {
    let candidates = vec![cand("C:/lib17/bin/java.exe", 17)];

    let err = choose_processor_java(25, ("C:/lib17/bin/java.exe", Some(17)), &candidates)
        .expect_err("没有够新的 Java 就该拒绝安装，而不是装出半成品");

    assert!(err.contains("25"), "要说清需要几: {}", err);
    assert!(err.contains("17"), "要列出本机有什么: {}", err);
}

#[test]
fn test_processor_vars_resolve_installer_and_vanilla_jar() {
    let root = std::path::Path::new("C:/game");
    let vars = ProcessorVars {
        game_dir: root,
        libraries_dir: &root.join("libraries"),
        installer: &root.join("installer.jar"),
        minecraft_jar: root.join("versions/26.3/26.3.jar"),
        data_dir: root.join("libraries/net/neoforged/neoforge/26.3.0.0-beta"),
        java_fallback: "C:/lib17/bin/java.exe",
        java_candidates: &[],
        mc_version: "26.3",
        loader_version: "26.3.0.0-beta",
    };

    assert_eq!(
        resolve_processor_arg("{INSTALLER}", None, &vars),
        vars.installer.to_string_lossy()
    );
    assert_eq!(
        resolve_processor_arg("{MINECRAFT_JAR}", None, &vars),
        vars.minecraft_jar.to_string_lossy()
    );
    assert_eq!(
        resolve_processor_arg("{ROOT}/libraries/", None, &vars).replace('\\', "/"),
        "C:/game/libraries/"
    );
}

/// install_profile 里 `/data/client.lzma` 指的是 installer 内部的文件，
/// 我们把它抽到 loader 的库目录，所以必须按 data_dir 解析，不能拼到游戏根目录
#[test]
fn test_processor_data_path_uses_data_dir() {
    let root = std::path::Path::new("C:/game");
    let vars = ProcessorVars {
        game_dir: root,
        libraries_dir: &root.join("libraries"),
        installer: &root.join("installer.jar"),
        minecraft_jar: root.join("versions/26.3/26.3.jar"),
        data_dir: root.join("libraries/net/neoforged/neoforge/26.3.0.0-beta"),
        java_fallback: "C:/lib17/bin/java.exe",
        java_candidates: &[],
        mc_version: "26.3",
        loader_version: "26.3.0.0-beta",
    };

    let data = serde_json::json!({ "BINPATCH": { "client": "/data/client.lzma" } });
    let map = data.as_object().unwrap();

    let resolved = resolve_processor_arg("{BINPATCH}", Some(map), &vars).replace('\\', "/");

    assert!(
        resolved.ends_with("libraries/net/neoforged/neoforge/26.3.0.0-beta/client.lzma"),
        "binpatch 要按抽出来的 data 目录解析，实际 {}",
        resolved
    );
}

/// NeoForge 21.11 的清单第一条是 `sides: ["server"]`，客户端安装要跳过它，
/// 否则会白跑一次 EXTRACT_FILES（写服务端的启动脚本）
#[test]
fn test_server_only_processor_is_skipped_for_client() {
    assert!(!runs_on_client(&serde_json::json!({ "sides": ["server"] })));
    assert!(runs_on_client(&serde_json::json!({ "sides": ["client"] })));
    assert!(runs_on_client(
        &serde_json::json!({ "sides": ["client", "server"] })
    ));
    // 没有 sides 表示两端都要跑
    assert!(runs_on_client(&serde_json::json!({ "jar": "a:b:1" })));
}

/// NeoForge 21.11 的 processor 会传 `--side {SIDE}`；不替换就报
/// "Missing download info for {SIDE} mappings"
#[test]
fn test_processor_side_placeholder_resolves_to_client() {
    let root = std::path::Path::new("C:/game");
    let vars = ProcessorVars {
        game_dir: root,
        libraries_dir: &root.join("libraries"),
        installer: &root.join("installer.jar"),
        minecraft_jar: root.join("versions/1.21.11/1.21.11.jar"),
        data_dir: root.join("libraries/net/neoforged/neoforge/21.11.48"),
        java_fallback: "C:/lib17/bin/java.exe",
        java_candidates: &[],
        mc_version: "1.21.11",
        loader_version: "21.11.48",
    };

    assert_eq!(resolve_processor_arg("{SIDE}", None, &vars), "client");
    assert_eq!(
        resolve_processor_arg("{ROOT}/run-{SIDE}.txt", None, &vars).replace('\\', "/"),
        "C:/game/run-client.txt"
    );
}

/// `{MOJMAPS}` 的 client 值是 `[...:mappings@txt]`：输出必须落在 .txt 上，
/// 否则下一个 processor 的 `--input-mappings` 会指向不存在的文件
#[test]
fn test_mojmaps_output_keeps_txt_extension() {
    let root = std::path::Path::new("C:/game");
    let vars = ProcessorVars {
        game_dir: root,
        libraries_dir: &root.join("libraries"),
        installer: &root.join("installer.jar"),
        minecraft_jar: root.join("versions/1.21.11/1.21.11.jar"),
        data_dir: root.join("libraries/net/neoforged/neoforge/21.11.48"),
        java_fallback: "C:/lib17/bin/java.exe",
        java_candidates: &[],
        mc_version: "1.21.11",
        loader_version: "21.11.48",
    };

    let data = serde_json::json!({
        "MOJMAPS": {
            "client": "[net.minecraft:client:1.21.11:mappings@txt]",
            "server": "[net.minecraft:server:1.21.11:mappings@txt]"
        }
    });
    let map = data.as_object().unwrap();

    let resolved = resolve_processor_arg("{MOJMAPS}", Some(map), &vars).replace('\\', "/");

    assert!(
        resolved.ends_with("libraries/net/minecraft/client/1.21.11/client-1.21.11-mappings.txt"),
        "映射输出路径不对: {}",
        resolved
    );
}

/// BMCL 与 MC 光速都镜像了 Mojang 库、Forge 与 NeoForge 的 maven；这三类都要换源，
/// 否则官方地址在国内会先卡满超时
#[test]
fn test_mirror_urls_covers_neoforge_forge_and_mojang() {
    let source = DownloadSource::HkmcMain;
    assert_eq!(
        mirror_urls(
            "https://maven.neoforged.net/releases/net/neoforged/fancymodloader/loader/12.0.0/loader-12.0.0.jar",
            source,
        )
        .first()
        .map(String::as_str),
        Some(
            "https://bsmirror.hkmc.online/maven/net/neoforged/fancymodloader/loader/12.0.0/loader-12.0.0.jar"
        )
    );
    assert_eq!(
        mirror_urls("https://maven.minecraftforge.net/net/minecraftforge/forge/1.12.2-14.23.5.2860/forge-1.12.2-14.23.5.2860-universal.jar", source)
            .first()
            .map(String::as_str),
        Some("https://bsmirror.hkmc.online/maven/net/minecraftforge/forge/1.12.2-14.23.5.2860/forge-1.12.2-14.23.5.2860-universal.jar")
    );
    assert_eq!(
        mirror_urls(
            "https://libraries.minecraft.net/com/mojang/brigadier/1.3.11/brigadier-1.3.11.jar",
            source,
        )
        .first()
        .map(String::as_str),
        Some("https://bsmirror.hkmc.online/maven/com/mojang/brigadier/1.3.11/brigadier-1.3.11.jar")
    );

    // BMCL 源下第一条是 BMCL，后面才是光速两条线路
    let bmcl = mirror_urls(
        "https://libraries.minecraft.net/com/mojang/brigadier/1.3.11/brigadier-1.3.11.jar",
        DownloadSource::Bmcl,
    );
    assert_eq!(
        bmcl[0],
        format!("{BMCL_BASE}/maven/com/mojang/brigadier/1.3.11/brigadier-1.3.11.jar")
    );
    assert!(bmcl.contains(&format!(
        "{HKMC_MAIN_BASE}/maven/com/mojang/brigadier/1.3.11/brigadier-1.3.11.jar"
    )));
}

/// repo1.maven.org 的地址同样能通过镜像的 /maven 路径换源
#[test]
fn test_mirror_urls_handles_maven_central() {
    let urls = mirror_urls(
        "https://repo1.maven.org/maven2/com/google/guava/guava/33.6.0-jre/guava-33.6.0-jre.jar",
        DownloadSource::Bmcl,
    );
    assert_eq!(
        urls.first().map(String::as_str),
        Some(
            "https://bmclapi2.bangbang93.com/maven/com/google/guava/guava/33.6.0-jre/guava-33.6.0-jre.jar"
        )
    );
}

#[tokio::test]
async fn test_limited_command_times_out() {
    let mut cmd = slow_command();
    let err = run_limited_command(&mut cmd, "测试命令", Duration::from_millis(300), None)
        .await
        .unwrap_err();

    assert!(err.to_string().contains("超时"), "{err}");
}

#[tokio::test]
async fn test_limited_command_cancels_running_process() {
    let cancel = AtomicBool::new(true);
    let mut cmd = slow_command();
    let err = run_limited_command(&mut cmd, "测试命令", Duration::from_secs(30), Some(&cancel))
        .await
        .unwrap_err();

    assert!(err.to_string().contains("取消"), "{err}");
}
