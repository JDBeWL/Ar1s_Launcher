use super::local::sanitize_instance_name;
use super::plan::build_targets;
use super::progress::{
    FILE_COMPLETED, FILE_DOWNLOADING, ModpackFileState, dispatch_order, snapshot_files,
};
use crate::models::GameConfig;
use crate::services::modpack_format::{ModrinthIndexFile, ModrinthIndexHashes};
use crate::utils::file_utils;
use std::sync::Arc;
use std::sync::atomic::Ordering;

fn make_index_file(path: &str, size: Option<u64>) -> ModrinthIndexFile {
    ModrinthIndexFile {
        path: path.to_string(),
        hashes: ModrinthIndexHashes {
            sha1: String::new(),
            sha512: None,
        },
        downloads: Vec::new(),
        file_size: size,
        env: None,
    }
}

fn make_file_state(path: &str, size: u64) -> Arc<ModpackFileState> {
    ModpackFileState::new(path.to_string(), size)
}

#[test]
fn test_dispatch_order_starts_with_largest_files() {
    let files = vec![
        make_index_file("small.jar", Some(10)),
        make_index_file("large.jar", Some(500)),
        make_index_file("unknown.jar", None),
        make_index_file("medium.jar", Some(100)),
    ];

    assert_eq!(dispatch_order(&files), vec![1, 3, 0, 2]);
}

#[test]
fn test_file_progress_snapshot_exposes_active_only() {
    let active = make_file_state("mods/active.jar", 100);
    active.downloaded.store(40, Ordering::Relaxed);
    active.status.store(FILE_DOWNLOADING, Ordering::Release);

    let completed = make_file_state("mods/done.jar", 200);
    completed.downloaded.store(200, Ordering::Relaxed);
    completed.status.store(FILE_COMPLETED, Ordering::Release);

    let pending = make_file_state("config/pending.toml", 50);
    let states = vec![active, completed, pending];

    let snapshot = snapshot_files(&states, 1024.0);

    assert_eq!(snapshot.total_files, 3);
    assert_eq!(snapshot.completed_files, 1);
    assert_eq!(snapshot.files.len(), 1);
    assert_eq!(snapshot.files[0].path, "mods/active.jar");
    assert_eq!(snapshot.files[0].downloaded, 40);
    assert_eq!(snapshot.files[0].total, 100);
    assert_eq!(snapshot.downloaded_bytes, 240);
    assert_eq!(snapshot.total_bytes, 350);
    assert_eq!(snapshot.speed, 1024.0);
}

#[test]
fn test_sanitize_instance_name_keeps_readable_names() {
    assert_eq!(
        sanitize_instance_name("Fabulously Optimized"),
        "Fabulously Optimized"
    );
    assert_eq!(sanitize_instance_name("我的整合包"), "我的整合包");
    assert_eq!(sanitize_instance_name("Pack: 1/2?"), "Pack- 1-2-");
}

#[test]
fn test_sanitize_instance_name_falls_back_for_unusable_names() {
    assert_eq!(sanitize_instance_name(""), "modpack");
    assert_eq!(sanitize_instance_name(".."), "modpack");
    assert_eq!(sanitize_instance_name("CON"), "modpack");
    assert_eq!(sanitize_instance_name("   "), "modpack");
}

#[test]
fn test_sanitize_instance_name_truncates_to_the_byte_limit() {
    // 校验按字节算上限，64 个汉字有 192 字节，必须先截短
    let long = sanitize_instance_name(&"名".repeat(100));
    assert!(long.len() <= 64);
    assert!(file_utils::validate_instance_name(&long).is_valid);

    let ascii = sanitize_instance_name(&"a".repeat(100));
    assert_eq!(ascii.chars().count(), 64);
}

#[test]
fn test_build_targets_rejects_shared_content_root() {
    let config = GameConfig {
        game_dir: "C:/game".to_string(),
        version_isolation: false,
        ..GameConfig::default()
    };

    let error = build_targets(&config, "pack")
        .err()
        .expect("非隔离模式必须拒绝安装")
        .to_string();
    assert!(error.contains("版本隔离"), "unexpected error: {error}");
}

#[test]
fn test_build_targets_keeps_modpack_content_inside_instance() {
    let config = GameConfig {
        game_dir: "C:/game".to_string(),
        version_isolation: true,
        ..GameConfig::default()
    };

    let targets = build_targets(&config, "pack").unwrap();
    assert_eq!(targets.content_root, targets.instance_dir);
}
