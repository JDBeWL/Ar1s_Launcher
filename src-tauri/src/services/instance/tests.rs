use super::lock_dir;
use super::maintenance::prune_stale_run_locks;
use super::mods::{parse_mods_toml_name, read_mod_metadata};
use super::scan::get_instances_sync;
use std::fs;
use std::io::Write;

/// 锁必须放在实例目录之外，否则 Windows 上目录内有打开的文件就无法重命名/删除实例
#[test]
fn test_run_locks_live_outside_instance_dirs() {
    let game = tempfile::TempDir::new().unwrap();
    let versions = game.path().join("versions");
    let instance = versions.join("1.20.1");
    fs::create_dir_all(&instance).unwrap();

    let lock_root = lock_dir(game.path());
    assert_eq!(lock_root, game.path().join(".ar1s"));
    assert!(!lock_root.starts_with(&versions));
    assert!(!instance.join(".natives.lock").exists());
}

#[test]
fn test_prune_removes_run_locks_of_deleted_instances_only() {
    let game = tempfile::TempDir::new().unwrap();
    let versions = game.path().join("versions");
    fs::create_dir_all(versions.join("still-here")).unwrap();

    let lock_root = lock_dir(game.path());
    fs::create_dir_all(&lock_root).unwrap();
    fs::write(lock_root.join("run-still-here.lock"), b"").unwrap();
    fs::write(lock_root.join("run-gone.lock"), b"").unwrap();
    fs::write(lock_root.join("instances.lock"), b"").unwrap();

    prune_stale_run_locks(game.path(), &versions);

    assert!(lock_root.join("run-still-here.lock").exists());
    assert!(!lock_root.join("run-gone.lock").exists());
    assert!(lock_root.join("instances.lock").exists());
}

/// 有游戏进程持有共享锁时，清理逻辑绝不能把它的锁文件删掉
#[test]
fn test_prune_never_removes_a_held_run_lock() {
    let game = tempfile::TempDir::new().unwrap();
    let versions = game.path().join("versions");
    fs::create_dir_all(&versions).unwrap();
    let lock_root = lock_dir(game.path());
    fs::create_dir_all(&lock_root).unwrap();

    let path = lock_root.join("run-orphan.lock");
    let held = crate::utils::file_lock::acquire_shared(&path, std::time::Duration::from_millis(50))
        .expect("持有共享锁");

    // 实例目录不存在，但锁仍被持有：不能删
    prune_stale_run_locks(game.path(), &versions);
    assert!(path.exists(), "正在运行的实例的锁不能被清掉");

    drop(held);
    assert!(!path.exists(), "持有者释放后文件自动消失");
}

#[test]
fn test_prune_is_a_noop_without_lock_dir() {
    let game = tempfile::TempDir::new().unwrap();
    let versions = game.path().join("versions");
    fs::create_dir_all(&versions).unwrap();

    prune_stale_run_locks(game.path(), &versions);
}

#[test]
fn test_migrate_moves_resume_state_and_drops_old_locks() {
    let game = tempfile::TempDir::new().unwrap();
    let versions = game.path().join("versions");
    let instance = versions.join("1.20.1");
    fs::create_dir_all(&instance).unwrap();
    fs::write(instance.join(".natives.lock"), b"").unwrap();

    let legacy = game.path().join(".download_state");
    fs::create_dir_all(&legacy).unwrap();
    fs::write(legacy.join("1.20.1.json"), r#"{"completed_files":{}}"#).unwrap();
    fs::write(legacy.join("download.lock"), b"").unwrap();
    fs::write(legacy.join("run-1.20.1.lock"), b"").unwrap();

    super::migrate_legacy_state(game.path(), &versions);

    // 续传状态要搬过去，否则已经下好的文件会重下
    let migrated = lock_dir(game.path()).join("download_state/1.20.1.json");
    assert!(migrated.exists());
    assert!(!legacy.exists());
    assert!(!instance.join(".natives.lock").exists());
}

/// 关闭隔离后，实例卡片上的模组数必须来自共享根目录，
/// 而不是实例目录里那些根本不会被加载的旧文件
#[test]
fn test_shared_mode_counts_mods_from_game_root() {
    let game = tempfile::TempDir::new().unwrap();
    let versions = game.path().join("versions");
    let instance = versions.join("1.20.1-Forge");
    fs::create_dir_all(&instance).unwrap();
    fs::write(
        instance.join("1.20.1-Forge.json"),
        r#"{"id":"1.20.1-Forge","inheritsFrom":"1.20.1","mainClass":"cpw.mods.bootstraplauncher.BootstrapLauncher"}"#,
    )
    .unwrap();

    // 实例目录里残留一个模组，共享目录里有两个
    fs::create_dir_all(instance.join("mods")).unwrap();
    fs::write(instance.join("mods/stale.jar"), b"x").unwrap();
    fs::create_dir_all(game.path().join("mods")).unwrap();
    for name in ["a.jar", "b.jar"] {
        fs::write(game.path().join("mods").join(name), b"x").unwrap();
    }

    let isolated = get_instances_sync(&versions, true).unwrap();
    assert_eq!(isolated[0].mod_count, Some(1), "隔离时应看实例目录");

    let shared = get_instances_sync(&versions, false).unwrap();
    assert_eq!(shared[0].mod_count, Some(2), "关闭隔离时应看根目录");
}

#[test]
fn test_base_and_loader_versions_are_both_listed() {
    let game = tempfile::TempDir::new().unwrap();
    let versions = game.path().join("versions");

    fs::create_dir_all(versions.join("test-base")).unwrap();
    fs::write(
        versions.join("test-base/test-base.json"),
        r#"{"id":"test-base","mainClass":"net.minecraft.client.main.Main"}"#,
    )
    .unwrap();

    fs::create_dir_all(versions.join("test-forge")).unwrap();
    fs::write(
        versions.join("test-forge/test-forge.json"),
        r#"{"id":"test-forge","inheritsFrom":"test-base","mainClass":"cpw.mods.bootstraplauncher.BootstrapLauncher"}"#,
    )
    .unwrap();

    let list = get_instances_sync(&versions, true).unwrap();

    assert_eq!(list.len(), 2);
    assert!(list.iter().any(|instance| instance.id == "test-base"));
    assert!(list.iter().any(|instance| instance.id == "test-forge"));
}

#[test]
fn test_mods_toml_name_survives_table_headers() {
    let text = "modLoader=\"javafml\"\nloaderVersion=\"[47,)\"\nlicense=\"MIT\"\n\n[[mods]]\nmodId=\"examplemod\"\nversion=\"1.0\"\ndisplayName=\"示例模组\"\n";
    assert_eq!(parse_mods_toml_name(text).as_deref(), Some("示例模组"));
}

#[test]
fn test_mods_toml_name_falls_back_to_mod_id() {
    let text = "[[mods]]\nmodId=\"examplemod\"\n";
    assert_eq!(parse_mods_toml_name(text).as_deref(), Some("examplemod"));
}

#[test]
fn test_mods_toml_name_skips_empty_display_name() {
    let text = "[[mods]]\nmodId=\"examplemod\"\ndisplayName=\"\"\n";
    assert_eq!(parse_mods_toml_name(text).as_deref(), Some("examplemod"));
}

/// Forge MDK 模板到处是 `#mandatory`，行内注释不能被当成名字的一部分
#[test]
fn test_mods_toml_name_strips_inline_comment() {
    let text = "[[mods]] #mandatory\nmodId = \"opack2reload\" #mandatory\ndisplayName = \"0Pack2Reload\" #mandatory\n";
    assert_eq!(parse_mods_toml_name(text).as_deref(), Some("0Pack2Reload"));
}

#[test]
fn test_mods_toml_name_keeps_hash_inside_quotes() {
    let text = "[[mods]]\nmodId=\"examplemod\"\ndisplayName=\"Mod #1\" #mandatory\n";
    assert_eq!(parse_mods_toml_name(text).as_deref(), Some("Mod #1"));
}

#[test]
fn test_mods_toml_name_reads_literal_strings_and_escapes() {
    // 单引号是字面串，不处理转义
    let text = "[[mods]]\nmodId='examplemod'\ndisplayName='Literal \\\" Name'\n";
    assert_eq!(
        parse_mods_toml_name(text).as_deref(),
        Some("Literal \\\" Name")
    );

    // 双引号串里的转义要还原
    let text = "[[mods]]\nmodId=\"examplemod\"\ndisplayName=\"A \\\"quoted\\\" mod\"\n";
    assert_eq!(
        parse_mods_toml_name(text).as_deref(),
        Some("A \"quoted\" mod")
    );
}

#[test]
fn test_mods_toml_name_reads_single_line_multiline_string() {
    let text = "[[mods]]\nmodId=\"examplemod\"\ndisplayName=\"\"\"Example\"\"\"\n";
    assert_eq!(parse_mods_toml_name(text).as_deref(), Some("Example"));
}

/// description 的多行正文里可能写着看起来像键值的行，不能拿来当名字
#[test]
fn test_mods_toml_name_ignores_multiline_description_body() {
    let text = "[[mods]]\nmodId=\"examplemod\"\ndisplayName=\"Example\"\ndescription='''\nmodId=\"bogus\"\ndisplayName=\"Bogus\"\n'''\n";
    assert_eq!(parse_mods_toml_name(text).as_deref(), Some("Example"));
}

#[test]
fn test_mods_toml_name_skips_commented_out_display_name() {
    let text = "[[mods]]\nmodId=\"examplemod\"\n#displayName=\"Bogus\" #optional\n";
    assert_eq!(parse_mods_toml_name(text).as_deref(), Some("examplemod"));
}

/// 未跑资源处理的构建会把 `${mod_name}` 原样打进 jar，不能拿来当展示名
#[test]
fn test_mods_toml_name_skips_unresolved_placeholders() {
    let text = "modId=\"compactmekanismmachines\"\nversion=\"${mod_version}\"\ndisplayName=\"${mod_name}\"\n";
    assert_eq!(
        parse_mods_toml_name(text).as_deref(),
        Some("compactmekanismmachines")
    );

    let text = "modId=\"${mod_id}\"\ndisplayName=\"${mod_name}\"\n";
    assert_eq!(parse_mods_toml_name(text), None);
}

fn write_metadata_jar(path: &std::path::Path, forge: &str, neoforge: Option<&str>) {
    let mut zip = zip::ZipWriter::new(fs::File::create(path).unwrap());
    let options = zip::write::SimpleFileOptions::default();
    zip.start_file("META-INF/mods.toml", options).unwrap();
    zip.write_all(forge.as_bytes()).unwrap();
    if let Some(neoforge) = neoforge {
        zip.start_file("META-INF/neoforge.mods.toml", options)
            .unwrap();
        zip.write_all(neoforge.as_bytes()).unwrap();
    }
    zip.finish().unwrap();
}

/// NeoForge 只读 neoforge.mods.toml；同包残留的 Forge 模板不应抢先
#[test]
fn test_mod_metadata_prefers_neoforge_manifest() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("mod.jar");
    write_metadata_jar(
        &path,
        "modId=\"compactmekanismmachines\"\ndisplayName=\"${mod_name}\"\n",
        Some("modId=\"compactmekanismmachines\"\ndisplayName=\"Compact Mekanism Machines\"\n"),
    );

    assert_eq!(
        read_mod_metadata(&path).0.as_deref(),
        Some("Compact Mekanism Machines")
    );
}

#[test]
fn test_mod_metadata_ignores_placeholder_only_metadata() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("mod.jar");
    write_metadata_jar(
        &path,
        "modId=\"${mod_id}\"\ndisplayName=\"${mod_name}\"\n",
        None,
    );

    assert_eq!(read_mod_metadata(&path), (None, None));
}
