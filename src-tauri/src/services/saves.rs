//! 存档扫描、备份、恢复与导入
//!
//! 备份固定放实例目录 `.ar1s/save-backups`；自动备份保留份数由全局配置控制。

use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use chrono::TimeZone;
use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::errors::LauncherError;
use crate::models::{SaveBackupInfo, SaveBackupKind, SaveImportSummary, SaveInfo};
use crate::services::launcher::isolation;
use crate::services::{config, instance};
use crate::utils::file_utils;

pub(crate) const BACKUP_MANIFEST: &str = "ar1s-backup.json";
const MAX_ICON_BYTES: u64 = 512 * 1024;
pub(crate) const MAX_EXTRACT_BYTES: u64 = 16 * 1024 * 1024 * 1024;
pub(crate) const AUTO_BACKUP_COOLDOWN_MS: i64 = 10 * 60 * 1000;

struct SaveContext {
    game_dir: PathBuf,
    saves_dir: PathBuf,
    backups_dir: PathBuf,
}

fn save_context(instance_name: &str) -> Result<SaveContext, LauncherError> {
    let config = config::load_config()?;
    let game_dir = PathBuf::from(&config.game_dir);
    let instance_dir = file_utils::instance_dir(&game_dir.join("versions"), instance_name)?;
    let saves_dir = isolation::content_root(&config, &game_dir, instance_name).join("saves");
    let backups_dir = instance_dir.join(".ar1s").join("save-backups");
    Ok(SaveContext {
        game_dir,
        saves_dir,
        backups_dir,
    })
}

pub(crate) fn is_safe_world_name(name: &str) -> bool {
    if name.is_empty() || name.contains('\0') {
        return false;
    }
    let mut components = Path::new(name).components();
    matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none()
}

pub fn list_saves(instance_name: &str) -> Result<Vec<SaveInfo>, LauncherError> {
    let context = save_context(instance_name)?;
    list_saves_in(&context.saves_dir)
}

pub(crate) fn list_saves_in(saves_dir: &Path) -> Result<Vec<SaveInfo>, LauncherError> {
    if !saves_dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut saves = Vec::new();
    for entry in fs::read_dir(saves_dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let folder = entry.file_name().to_string_lossy().to_string();
        if !is_safe_world_name(&folder) {
            continue;
        }
        let world_dir = entry.path();
        let level_dat = world_dir.join("level.dat");
        if !level_dat.is_file() {
            continue;
        }

        let meta = read_level_meta(&level_dat);
        let last_played = meta.last_played.or_else(|| file_mtime_ms(&level_dat));
        let display_name = meta.name.unwrap_or_else(|| folder.clone());

        saves.push(SaveInfo {
            folder,
            display_name,
            last_played,
            size: dir_size(&world_dir).unwrap_or(0),
            icon_data_url: read_icon(&world_dir.join("icon.png")),
        });
    }

    saves.sort_by(|a, b| {
        b.last_played
            .unwrap_or(0)
            .cmp(&a.last_played.unwrap_or(0))
            .then_with(|| {
                a.display_name
                    .to_lowercase()
                    .cmp(&b.display_name.to_lowercase())
            })
    });
    Ok(saves)
}

struct LevelMeta {
    name: Option<String>,
    last_played: Option<i64>,
}

fn read_level_meta(level_dat: &Path) -> LevelMeta {
    let result = (|| -> Option<LevelMeta> {
        let bytes = fs::read(level_dat).ok()?;
        let parsed = if bytes.starts_with(&[0x1f, 0x8b]) {
            let mut decoder = GzDecoder::new(bytes.as_slice());
            let mut decompressed = Vec::new();
            decoder.read_to_end(&mut decompressed).ok()?;
            fastnbt::from_bytes::<fastnbt::Value>(&decompressed).ok()?
        } else {
            fastnbt::from_bytes::<fastnbt::Value>(&bytes).ok()?
        };

        let fastnbt::Value::Compound(root) = parsed else {
            return None;
        };
        let data = match root.get("Data") {
            Some(fastnbt::Value::Compound(data)) => data,
            _ => &root,
        };

        let name = match data.get("LevelName") {
            Some(fastnbt::Value::String(value)) => Some(value.clone()),
            _ => None,
        };
        let last_played = match data.get("LastPlayed") {
            Some(fastnbt::Value::Long(value)) => Some(*value),
            _ => None,
        };
        Some(LevelMeta { name, last_played })
    })();

    result.unwrap_or(LevelMeta {
        name: None,
        last_played: None,
    })
}

fn file_mtime_ms(path: &Path) -> Option<i64> {
    fs::metadata(path)
        .ok()?
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_millis() as i64)
}

fn dir_size(dir: &Path) -> io::Result<u64> {
    let mut total = 0u64;
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            total = total.saturating_add(dir_size(&entry.path())?);
        } else if file_type.is_file() {
            total = total.saturating_add(entry.metadata()?.len());
        }
    }
    Ok(total)
}

fn read_icon(path: &Path) -> Option<String> {
    let meta = fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_ICON_BYTES {
        return None;
    }
    let bytes = fs::read(path).ok()?;
    Some(format!("data:image/png;base64,{}", STANDARD.encode(bytes)))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BackupManifest {
    format: u32,
    folder: String,
    name: String,
    created_at: i64,
    kind: SaveBackupKind,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

pub(crate) fn clamp_retention(value: u32) -> u32 {
    value.clamp(
        crate::models::MIN_SAVE_BACKUP_RETENTION,
        crate::models::MAX_SAVE_BACKUP_RETENTION,
    )
}

fn validate_backup_file_name(file_name: &str) -> Result<(), LauncherError> {
    if file_name.is_empty()
        || !file_name.ends_with(".zip")
        || file_name.contains('/')
        || file_name.contains('\\')
        || file_name.contains("..")
    {
        return Err(LauncherError::Custom("备份文件名无效".to_string()));
    }
    Ok(())
}

pub fn list_backups(instance_name: &str) -> Result<Vec<SaveBackupInfo>, LauncherError> {
    let context = save_context(instance_name)?;
    list_backups_in(&context.backups_dir)
}

pub(crate) fn list_backups_in(backups_dir: &Path) -> Result<Vec<SaveBackupInfo>, LauncherError> {
    if !backups_dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut backups = Vec::new();
    for world_entry in fs::read_dir(backups_dir)? {
        let world_entry = world_entry?;
        if !world_entry.file_type()?.is_dir() {
            continue;
        }
        let world = world_entry.file_name().to_string_lossy().to_string();
        if !is_safe_world_name(&world) {
            continue;
        }

        for file_entry in fs::read_dir(world_entry.path())? {
            let file_entry = file_entry?;
            if !file_entry.file_type()?.is_file() {
                continue;
            }
            let file_name = file_entry.file_name().to_string_lossy().to_string();
            if validate_backup_file_name(&file_name).is_err() {
                continue;
            }
            let Some(manifest) = read_backup_manifest(&file_entry.path()) else {
                log::warn!(
                    "跳过无法读取 manifest 的备份: {}",
                    file_entry.path().display()
                );
                continue;
            };
            if manifest.format != 1 {
                log::warn!(
                    "跳过不受支持的备份格式 {}: {}",
                    manifest.format,
                    file_entry.path().display()
                );
                continue;
            }
            backups.push(SaveBackupInfo {
                world: world.clone(),
                file_name,
                display_name: manifest.name,
                created_at: manifest.created_at,
                size: file_entry.metadata().map(|meta| meta.len()).unwrap_or(0),
                kind: manifest.kind,
            });
        }
    }

    backups.sort_by_key(|backup| std::cmp::Reverse(backup.created_at));
    Ok(backups)
}

pub async fn backup_save(
    instance_name: &str,
    world: &str,
) -> Result<SaveBackupInfo, LauncherError> {
    if !is_safe_world_name(world) {
        return Err(LauncherError::Custom("存档文件夹名无效".to_string()));
    }
    let context = save_context(instance_name)?;
    let retention = clamp_retention(config::load_config()?.save_backup_retention);

    let _run_lock = instance::acquire_run_lock(&context.game_dir, instance_name)
        .await
        .map_err(|error| match error {
            LauncherError::Busy(_) => {
                LauncherError::Custom("实例正在运行，请先退出游戏再备份存档".to_string())
            }
            other => other,
        })?;

    let world = world.to_string();
    tokio::task::spawn_blocking(move || {
        create_backup_at(
            &context.saves_dir,
            &context.backups_dir,
            &world,
            SaveBackupKind::Manual,
            now_ms(),
            retention,
        )
    })
    .await
    .map_err(|error| LauncherError::Custom(format!("备份任务异常终止: {}", error)))?
}

pub(crate) fn create_backup_at(
    saves_dir: &Path,
    backups_dir: &Path,
    world: &str,
    kind: SaveBackupKind,
    now_ms: i64,
    retention: u32,
) -> Result<SaveBackupInfo, LauncherError> {
    if !is_safe_world_name(world) {
        return Err(LauncherError::Custom("存档文件夹名无效".to_string()));
    }
    let world_dir = saves_dir.join(world);
    let level_dat = world_dir.join("level.dat");
    if !level_dat.is_file() {
        return Err(LauncherError::Custom(format!(
            "存档 '{}' 不存在或缺少 level.dat",
            world
        )));
    }

    let display_name = read_level_meta(&level_dat)
        .name
        .unwrap_or_else(|| world.to_string());
    let world_backups = backups_dir.join(world);
    fs::create_dir_all(&world_backups)?;

    let created = chrono::Local
        .timestamp_millis_opt(now_ms)
        .single()
        .unwrap_or_else(chrono::Local::now);
    let base = format!(
        "{}-{:03}-{}",
        created.format("%Y%m%d-%H%M%S"),
        now_ms.rem_euclid(1000),
        kind.as_str()
    );
    let mut file_name = format!("{}.zip", base);
    let mut counter = 1;
    while world_backups.join(&file_name).exists() {
        file_name = format!("{}-{}.zip", base, counter);
        counter += 1;
    }
    let final_path = world_backups.join(&file_name);
    let temp_path = world_backups.join(format!(".tmp-{}.zip", file_utils::unique_suffix()));

    let manifest = BackupManifest {
        format: 1,
        folder: world.to_string(),
        name: display_name.clone(),
        created_at: now_ms,
        kind,
    };

    let result = write_backup_zip(&world_dir, &temp_path, &manifest)
        .and_then(|_| fs::rename(&temp_path, &final_path).map_err(LauncherError::Io));
    if let Err(error) = result {
        let _ = fs::remove_file(&temp_path);
        return Err(error);
    }

    prune_auto_backups(&world_backups, retention)?;

    let size = fs::metadata(&final_path)
        .map(|meta| meta.len())
        .unwrap_or(0);
    Ok(SaveBackupInfo {
        world: world.to_string(),
        file_name,
        display_name,
        created_at: now_ms,
        size,
        kind,
    })
}

fn write_backup_zip(
    world_dir: &Path,
    zip_path: &Path,
    manifest: &BackupManifest,
) -> Result<(), LauncherError> {
    let file = File::create(zip_path)?;
    let mut zip = ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    zip.start_file(BACKUP_MANIFEST, options)?;
    zip.write_all(serde_json::to_string_pretty(manifest)?.as_bytes())?;
    append_dir_to_zip(&mut zip, world_dir, world_dir, options)?;
    zip.finish()?.sync_all()?;
    Ok(())
}

fn append_dir_to_zip(
    zip: &mut ZipWriter<File>,
    root: &Path,
    current: &Path,
    options: SimpleFileOptions,
) -> Result<(), LauncherError> {
    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            continue;
        }
        let path = entry.path();
        let relative = path
            .strip_prefix(root)
            .map_err(|error| LauncherError::Custom(format!("计算备份相对路径失败: {}", error)))?
            .to_string_lossy()
            .replace('\\', "/");
        if relative.is_empty() || !file_utils::is_safe_relative_path(&relative) {
            continue;
        }

        if file_type.is_dir() {
            append_dir_to_zip(zip, root, &path, options)?;
        } else if file_type.is_file() {
            zip.start_file(relative, options)?;
            let mut input = io::BufReader::with_capacity(1024 * 1024, File::open(&path)?);
            io::copy(&mut input, zip)?;
        }
    }
    Ok(())
}

fn read_backup_manifest(path: &Path) -> Option<BackupManifest> {
    let file = File::open(path).ok()?;
    let mut archive = ZipArchive::new(file).ok()?;
    let mut entry = archive.by_name(BACKUP_MANIFEST).ok()?;
    let mut text = String::new();
    entry.read_to_string(&mut text).ok()?;
    serde_json::from_str(&text).ok()
}

fn prune_auto_backups(world_backups: &Path, retention: u32) -> Result<(), LauncherError> {
    let retention = clamp_retention(retention) as usize;
    let mut automatic = Vec::new();

    for entry in fs::read_dir(world_backups)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let file_name = entry.file_name().to_string_lossy().to_string();
        if validate_backup_file_name(&file_name).is_err() {
            continue;
        }
        let Some(manifest) = read_backup_manifest(&entry.path()) else {
            continue;
        };
        if manifest.kind != SaveBackupKind::Manual {
            automatic.push((manifest.created_at, entry.path()));
        }
    }

    automatic.sort_by_key(|entry| std::cmp::Reverse(entry.0));
    for (_, path) in automatic.into_iter().skip(retention) {
        let _ = fs::remove_file(path);
    }
    Ok(())
}

pub fn delete_backup(
    instance_name: &str,
    world: &str,
    file_name: &str,
) -> Result<(), LauncherError> {
    if !is_safe_world_name(world) {
        return Err(LauncherError::Custom("存档文件夹名无效".to_string()));
    }
    validate_backup_file_name(file_name)?;
    let context = save_context(instance_name)?;
    let path = context.backups_dir.join(world).join(file_name);
    if !path.is_file() {
        return Err(LauncherError::Custom(format!(
            "备份文件不存在: {}",
            file_name
        )));
    }
    fs::remove_file(path)?;
    Ok(())
}

pub async fn restore_backup(
    instance_name: &str,
    world: &str,
    file_name: &str,
) -> Result<(), LauncherError> {
    if !is_safe_world_name(world) {
        return Err(LauncherError::Custom("存档文件夹名无效".to_string()));
    }
    validate_backup_file_name(file_name)?;

    let context = save_context(instance_name)?;
    let backup_path = context.backups_dir.join(world).join(file_name);
    let manifest = read_backup_manifest(&backup_path)
        .ok_or_else(|| LauncherError::Custom("备份文件损坏或缺少 manifest".to_string()))?;
    if manifest.format != 1 {
        return Err(LauncherError::Custom(format!(
            "不支持的备份格式: {}",
            manifest.format
        )));
    }
    if manifest.folder != world {
        return Err(LauncherError::Custom("备份与目标存档不匹配".to_string()));
    }

    let retention = clamp_retention(config::load_config()?.save_backup_retention);
    let _run_lock = instance::acquire_run_lock(&context.game_dir, instance_name)
        .await
        .map_err(|error| match error {
            LauncherError::Busy(_) => {
                LauncherError::Custom("实例正在运行，请先退出游戏再恢复存档".to_string())
            }
            other => other,
        })?;

    tokio::task::spawn_blocking(move || {
        let target = context.saves_dir.join(&manifest.folder);
        if target.exists() {
            create_backup_at(
                &context.saves_dir,
                &context.backups_dir,
                &manifest.folder,
                SaveBackupKind::PreRestore,
                now_ms(),
                retention,
            )?;
        }

        let temp = context
            .saves_dir
            .join(format!(".restore-{}", file_utils::unique_suffix()));
        fs::create_dir_all(&context.saves_dir)?;
        extract_world_zip(&backup_path, &temp)?;
        if !temp.join("level.dat").is_file() {
            let _ = fs::remove_dir_all(&temp);
            return Err(LauncherError::Custom(
                "备份内容缺少 level.dat，已中止恢复".to_string(),
            ));
        }

        let old = context
            .saves_dir
            .join(format!(".old-{}", file_utils::unique_suffix()));
        let had_old = target.exists();
        if had_old {
            fs::rename(&target, &old)?;
        }
        if let Err(error) = fs::rename(&temp, &target) {
            if had_old {
                let _ = fs::rename(&old, &target);
            }
            let _ = fs::remove_dir_all(&temp);
            return Err(LauncherError::Io(error));
        }
        if had_old {
            let _ = fs::remove_dir_all(&old);
        }
        Ok(())
    })
    .await
    .map_err(|error| LauncherError::Custom(format!("恢复任务异常终止: {}", error)))?
}

pub(crate) fn extract_world_zip(zip_path: &Path, target_dir: &Path) -> Result<(), LauncherError> {
    let file = File::open(zip_path)?;
    let mut archive = ZipArchive::new(file)?;
    let mut declared_total = 0u64;

    for index in 0..archive.len() {
        let entry = archive.by_index(index)?;
        if entry.name()?.as_ref() == BACKUP_MANIFEST {
            continue;
        }
        let Some(relative) = entry.enclosed_name() else {
            return Err(LauncherError::Custom("备份包含不安全的路径".to_string()));
        };
        let relative = relative.to_string_lossy().replace('\\', "/");
        if relative.is_empty() {
            continue;
        }
        if !file_utils::is_safe_relative_path(&relative) {
            return Err(LauncherError::Custom(format!(
                "备份包含不安全的路径: {}",
                relative
            )));
        }
        declared_total = declared_total.saturating_add(entry.size());
        if declared_total > MAX_EXTRACT_BYTES {
            return Err(LauncherError::Custom(
                "备份解压后体积超过上限，已中止".to_string(),
            ));
        }
    }

    drop(archive);
    let file = File::open(zip_path)?;
    let mut archive = ZipArchive::new(file)?;
    fs::create_dir_all(target_dir)?;

    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        if entry.name()?.as_ref() == BACKUP_MANIFEST {
            continue;
        }
        let Some(relative) = entry.enclosed_name() else {
            continue;
        };
        let relative = relative.to_string_lossy().replace('\\', "/");
        if relative.is_empty() || !file_utils::is_safe_relative_path(&relative) {
            continue;
        }
        let out_path = target_dir.join(relative);
        if entry.is_dir() {
            fs::create_dir_all(&out_path)?;
            continue;
        }
        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut output = File::create(&out_path)?;
        io::copy(&mut entry, &mut output)?;
    }
    Ok(())
}

pub async fn import_saves(
    instance_name: &str,
    source_paths: &[String],
) -> Result<SaveImportSummary, LauncherError> {
    let context = save_context(instance_name)?;
    let _run_lock = instance::acquire_run_lock(&context.game_dir, instance_name)
        .await
        .map_err(|error| match error {
            LauncherError::Busy(_) => {
                LauncherError::Custom("实例正在运行，请先退出游戏再导入存档".to_string())
            }
            other => other,
        })?;

    let sources = source_paths.to_vec();
    tokio::task::spawn_blocking(move || {
        fs::create_dir_all(&context.saves_dir)?;
        let mut summary = SaveImportSummary {
            imported: Vec::new(),
            skipped: Vec::new(),
        };

        for source in sources {
            let source_path = PathBuf::from(&source);
            if source_path == context.saves_dir {
                summary.skipped.push(source.clone());
                continue;
            }
            if source_path.is_dir() {
                let worlds = discover_world_dirs(&source_path);
                if worlds.is_empty() {
                    summary.skipped.push(source.clone());
                    continue;
                }
                for world in worlds {
                    match copy_world_dir(&world, &context.saves_dir) {
                        Ok(name) => summary.imported.push(name),
                        Err(error) => {
                            log::warn!("导入存档 {} 失败: {}", world.display(), error);
                            summary.skipped.push(world.to_string_lossy().to_string());
                        }
                    }
                }
            } else if source_path.is_file()
                && source_path
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("zip"))
            {
                match import_world_zip(&source_path, &context.saves_dir) {
                    Ok(name) => summary.imported.push(name),
                    Err(error) => {
                        log::warn!("导入存档 zip {} 失败: {}", source, error);
                        summary.skipped.push(source.clone());
                    }
                }
            } else {
                summary.skipped.push(source.clone());
            }
        }

        Ok(summary)
    })
    .await
    .map_err(|error| LauncherError::Custom(format!("导入任务异常终止: {}", error)))?
}

fn discover_world_dirs(root: &Path) -> Vec<PathBuf> {
    if root.join("level.dat").is_file() {
        return vec![root.to_path_buf()];
    }
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|file_type| file_type.is_dir()))
        .map(|entry| entry.path())
        .filter(|path| path.join("level.dat").is_file())
        .collect()
}

fn copy_world_dir(source: &Path, saves_dir: &Path) -> Result<String, LauncherError> {
    let base = source
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "world".to_string());
    let target = unique_world_target(saves_dir, &base);

    if let Err(error) = file_utils::copy_dir_all(source, &target) {
        let _ = fs::remove_dir_all(&target);
        return Err(LauncherError::Io(error));
    }
    Ok(target
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "world".to_string()))
}

fn import_world_zip(source: &Path, saves_dir: &Path) -> Result<String, LauncherError> {
    let file = File::open(source)?;
    let mut archive = ZipArchive::new(file)?;

    let mut declared_total = 0u64;
    let mut roots = BTreeSet::new();
    for index in 0..archive.len() {
        let entry = archive.by_index(index)?;
        let name = entry.name()?.replace('\\', "/");
        declared_total = declared_total.saturating_add(entry.size());
        if declared_total > MAX_EXTRACT_BYTES {
            return Err(LauncherError::Custom(
                "压缩包解压后体积超过上限，已中止".to_string(),
            ));
        }
        if name == "level.dat" {
            roots.insert(String::new());
            continue;
        }
        if let Some(prefix) = name.strip_suffix("/level.dat")
            && !prefix.is_empty()
            && !prefix.contains('/')
        {
            roots.insert(prefix.to_string());
        }
    }

    if roots.is_empty() {
        return Err(LauncherError::Custom(
            "压缩包里没有找到 level.dat".to_string(),
        ));
    }
    if roots.len() > 1 {
        return Err(LauncherError::Custom(
            "压缩包里包含多个存档，请分开导入".to_string(),
        ));
    }

    let prefix = roots.into_iter().next().unwrap_or_default();
    let base = if prefix.is_empty() {
        source
            .file_stem()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "world".to_string())
    } else {
        prefix.clone()
    };
    let target = unique_world_target(saves_dir, &base);
    let temp = saves_dir.join(format!(".import-{}", file_utils::unique_suffix()));
    fs::create_dir_all(&temp)?;

    let result = (|| -> Result<(), LauncherError> {
        let file = File::open(source)?;
        let mut archive = ZipArchive::new(file)?;
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index)?;
            let name = entry.name()?.replace('\\', "/");
            if name == BACKUP_MANIFEST {
                continue;
            }
            let relative = if prefix.is_empty() {
                name.as_str()
            } else {
                match name.strip_prefix(&format!("{}/", prefix)) {
                    Some(rest) => rest,
                    None => continue,
                }
            };
            if relative.is_empty() {
                continue;
            }
            if !file_utils::is_safe_relative_path(relative) {
                return Err(LauncherError::Custom(format!(
                    "压缩包包含不安全的路径: {}",
                    relative
                )));
            }
            let out_path = temp.join(relative);
            if entry.is_dir() {
                fs::create_dir_all(&out_path)?;
                continue;
            }
            if let Some(parent) = out_path.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut output = File::create(&out_path)?;
            io::copy(&mut entry, &mut output)?;
        }
        if !temp.join("level.dat").is_file() {
            return Err(LauncherError::Custom(
                "压缩包内容缺少 level.dat".to_string(),
            ));
        }
        fs::rename(&temp, &target)?;
        Ok(())
    })();

    if let Err(error) = result {
        let _ = fs::remove_dir_all(&temp);
        return Err(error);
    }

    Ok(target
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "world".to_string()))
}

fn unique_world_target(saves_dir: &Path, base: &str) -> PathBuf {
    let base = sanitize_world_folder(base);
    let mut candidate = base.clone();
    let mut counter = 1;
    while saves_dir.join(&candidate).exists() {
        candidate = format!("{}-{}", base, counter);
        counter += 1;
    }
    saves_dir.join(candidate)
}

fn sanitize_world_folder(name: &str) -> String {
    let mut sanitized: String = name
        .chars()
        .map(|character| {
            if character.is_control()
                || matches!(
                    character,
                    '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
                )
            {
                '_'
            } else {
                character
            }
        })
        .collect();
    sanitized = sanitized.trim().trim_matches('.').to_string();
    if sanitized.is_empty() {
        "world".to_string()
    } else {
        sanitized
    }
}

pub async fn auto_backup_before_update(instance_name: &str) -> Result<(), LauncherError> {
    let config = config::load_config()?;
    if !config.save_auto_backup {
        return Ok(());
    }
    let retention = clamp_retention(config.save_backup_retention);
    let context = save_context(instance_name)?;

    let _run_lock = instance::acquire_run_lock(&context.game_dir, instance_name)
        .await
        .map_err(|error| match error {
            LauncherError::Busy(_) => LauncherError::Custom(
                "实例正在运行，无法在更新前自动备份存档，请先退出游戏".to_string(),
            ),
            other => other,
        })?;

    tokio::task::spawn_blocking(move || {
        let worlds = list_world_folders(&context.saves_dir)?;
        if worlds.is_empty() {
            return Ok(());
        }
        let backups = list_backups_in(&context.backups_dir)?;
        let now = now_ms();

        for world in worlds {
            let latest_auto = backups
                .iter()
                .filter(|backup| backup.world == world && backup.kind == SaveBackupKind::Auto)
                .map(|backup| backup.created_at)
                .max();
            if should_skip_auto_backup(latest_auto, now) {
                continue;
            }
            create_backup_at(
                &context.saves_dir,
                &context.backups_dir,
                &world,
                SaveBackupKind::Auto,
                now,
                retention,
            )?;
        }
        Ok(())
    })
    .await
    .map_err(|error| LauncherError::Custom(format!("自动备份任务异常终止: {}", error)))?
}

fn list_world_folders(saves_dir: &Path) -> Result<Vec<String>, LauncherError> {
    if !saves_dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut worlds = Vec::new();
    for entry in fs::read_dir(saves_dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let folder = entry.file_name().to_string_lossy().to_string();
        if is_safe_world_name(&folder) && entry.path().join("level.dat").is_file() {
            worlds.push(folder);
        }
    }
    Ok(worlds)
}

fn should_skip_auto_backup(latest_auto_at: Option<i64>, now_ms: i64) -> bool {
    latest_auto_at.is_some_and(|created| now_ms.saturating_sub(created) < AUTO_BACKUP_COOLDOWN_MS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_level_dat(path: &Path, name: &str, last_played: i64) {
        let mut nbt = Vec::new();
        nbt.push(10);
        nbt.extend_from_slice(&0u16.to_be_bytes());
        nbt.push(10);
        nbt.extend_from_slice(&4u16.to_be_bytes());
        nbt.extend_from_slice(b"Data");
        nbt.push(8);
        nbt.extend_from_slice(&9u16.to_be_bytes());
        nbt.extend_from_slice(b"LevelName");
        nbt.extend_from_slice(&(name.len() as u16).to_be_bytes());
        nbt.extend_from_slice(name.as_bytes());
        nbt.push(4);
        nbt.extend_from_slice(&10u16.to_be_bytes());
        nbt.extend_from_slice(b"LastPlayed");
        nbt.extend_from_slice(&last_played.to_be_bytes());
        nbt.push(0);
        nbt.push(0);

        let mut encoder = flate2::write::GzEncoder::new(
            File::create(path).unwrap(),
            flate2::Compression::default(),
        );
        encoder.write_all(&nbt).unwrap();
        encoder.finish().unwrap();
    }

    fn make_world(saves_dir: &Path, folder: &str, name: &str, last_played: i64, icon: bool) {
        let world = saves_dir.join(folder);
        fs::create_dir_all(world.join("region")).unwrap();
        write_level_dat(&world.join("level.dat"), name, last_played);
        fs::write(world.join("region/r.0.0.mca"), vec![0u8; 2048]).unwrap();
        if icon {
            fs::write(world.join("icon.png"), b"PNG").unwrap();
        }
    }

    #[test]
    fn lists_only_valid_worlds_and_sorts_by_last_played() {
        let dir = tempfile::TempDir::new().unwrap();
        let saves_dir = dir.path();
        make_world(saves_dir, "world-a", "Alpha", 1_000, true);
        make_world(saves_dir, "world-b", "Beta", 2_000, false);
        fs::create_dir_all(saves_dir.join("not-a-world")).unwrap();

        let saves = list_saves_in(saves_dir).unwrap();
        assert_eq!(saves.len(), 2);
        assert_eq!(saves[0].folder, "world-b");
        assert_eq!(saves[0].display_name, "Beta");
        assert_eq!(saves[0].last_played, Some(2_000));
        assert!(saves[0].icon_data_url.is_none());
        assert_eq!(saves[1].folder, "world-a");
        assert!(
            saves[1]
                .icon_data_url
                .as_deref()
                .unwrap()
                .starts_with("data:image/png;base64,")
        );
        assert!(saves[1].size >= 2048);
    }

    #[test]
    fn corrupt_level_dat_falls_back_to_folder_and_mtime() {
        let dir = tempfile::TempDir::new().unwrap();
        let world = dir.path().join("broken");
        fs::create_dir_all(&world).unwrap();
        fs::write(world.join("level.dat"), b"not nbt").unwrap();

        let saves = list_saves_in(dir.path()).unwrap();
        assert_eq!(saves.len(), 1);
        assert_eq!(saves[0].display_name, "broken");
        assert!(saves[0].last_played.is_some());
    }

    #[test]
    fn rejects_unsafe_world_names() {
        assert!(is_safe_world_name("world"));
        assert!(is_safe_world_name("我的世界"));
        assert!(!is_safe_world_name(".."));
        assert!(!is_safe_world_name("a/b"));
        assert!(!is_safe_world_name("a\\b"));
        assert!(!is_safe_world_name(""));
    }

    #[test]
    fn backup_round_trip_lists_and_deletes() {
        let dir = tempfile::TempDir::new().unwrap();
        let saves_dir = dir.path().join("saves");
        let backups_dir = dir.path().join("instance/.ar1s/save-backups");
        fs::create_dir_all(&saves_dir).unwrap();
        make_world(&saves_dir, "world-a", "Alpha", 1_000, true);

        let info = create_backup_at(
            &saves_dir,
            &backups_dir,
            "world-a",
            SaveBackupKind::Manual,
            1_760_000_000_000,
            3,
        )
        .unwrap();
        assert_eq!(info.world, "world-a");
        assert_eq!(info.display_name, "Alpha");
        assert!(info.size > 0);

        let listed = list_backups_in(&backups_dir).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].file_name, info.file_name);
        assert_eq!(listed[0].kind, SaveBackupKind::Manual);

        fs::remove_file(backups_dir.join("world-a").join(&info.file_name)).unwrap();
        assert!(list_backups_in(&backups_dir).unwrap().is_empty());
    }

    #[test]
    fn retention_keeps_newest_auto_backups_and_all_manual() {
        let dir = tempfile::TempDir::new().unwrap();
        let saves_dir = dir.path().join("saves");
        let backups_dir = dir.path().join("instance/.ar1s/save-backups");
        fs::create_dir_all(&saves_dir).unwrap();
        make_world(&saves_dir, "world-a", "Alpha", 1_000, false);

        for index in 0..5 {
            create_backup_at(
                &saves_dir,
                &backups_dir,
                "world-a",
                SaveBackupKind::Auto,
                1_760_000_000_000 + index,
                2,
            )
            .unwrap();
        }
        create_backup_at(
            &saves_dir,
            &backups_dir,
            "world-a",
            SaveBackupKind::Manual,
            1_760_000_000_100,
            2,
        )
        .unwrap();

        let listed = list_backups_in(&backups_dir).unwrap();
        let auto = listed
            .iter()
            .filter(|backup| backup.kind == SaveBackupKind::Auto)
            .count();
        let manual = listed
            .iter()
            .filter(|backup| backup.kind == SaveBackupKind::Manual)
            .count();
        assert_eq!(auto, 2);
        assert_eq!(manual, 1);
    }

    #[test]
    fn extract_round_trip_restores_files() {
        let dir = tempfile::TempDir::new().unwrap();
        let saves_dir = dir.path().join("saves");
        let backups_dir = dir.path().join("instance/.ar1s/save-backups");
        fs::create_dir_all(&saves_dir).unwrap();
        make_world(&saves_dir, "world-a", "Alpha", 1_000, false);

        create_backup_at(
            &saves_dir,
            &backups_dir,
            "world-a",
            SaveBackupKind::Manual,
            1_760_000_000_000,
            3,
        )
        .unwrap();

        fs::write(saves_dir.join("world-a/region/r.0.0.mca"), vec![1u8; 4]).unwrap();
        let before = fs::read(saves_dir.join("world-a/region/r.0.0.mca")).unwrap();

        let backup_name = list_backups_in(&backups_dir).unwrap()[0].file_name.clone();
        let backup_path = backups_dir.join("world-a").join(&backup_name);
        let temp = saves_dir.join(".test-restore");
        extract_world_zip(&backup_path, &temp).unwrap();
        assert!(temp.join("level.dat").is_file());
        let restored = fs::read(temp.join("region/r.0.0.mca")).unwrap();
        assert_ne!(before, restored);

        let target = saves_dir.join("world-a");
        fs::remove_dir_all(&target).unwrap();
        fs::rename(&temp, &target).unwrap();
        assert_eq!(fs::read(target.join("region/r.0.0.mca")).unwrap(), restored);
    }

    #[test]
    fn extract_rejects_path_traversal() {
        let dir = tempfile::TempDir::new().unwrap();
        let zip_path = dir.path().join("evil.zip");
        {
            let file = File::create(&zip_path).unwrap();
            let mut zip = ZipWriter::new(file);
            let options = SimpleFileOptions::default();
            zip.start_file("../evil.txt", options).unwrap();
            zip.write_all(b"evil").unwrap();
            zip.finish().unwrap();
        }

        let result = extract_world_zip(&zip_path, &dir.path().join("out"));
        assert!(result.is_err());
        assert!(!dir.path().join("evil.txt").exists());
    }

    #[test]
    fn import_folder_renames_collision() {
        let dir = tempfile::TempDir::new().unwrap();
        let saves_dir = dir.path().join("saves");
        fs::create_dir_all(&saves_dir).unwrap();
        make_world(&saves_dir, "world-a", "Alpha", 1_000, false);

        let source = dir.path().join("incoming");
        make_world(dir.path(), "incoming", "Incoming", 2_000, false);

        let imported = copy_world_dir(&source, &saves_dir).unwrap();
        assert_eq!(imported, "incoming");
        let imported_again = copy_world_dir(&source, &saves_dir).unwrap();
        assert_eq!(imported_again, "incoming-1");
        assert!(saves_dir.join("incoming").join("level.dat").is_file());
        assert!(saves_dir.join("incoming-1").join("level.dat").is_file());
    }

    #[test]
    fn sanitize_world_folder_removes_unsafe_chars() {
        assert_eq!(sanitize_world_folder("my/world"), "my_world");
        assert_eq!(sanitize_world_folder("..."), "world");
        assert_eq!(sanitize_world_folder("  "), "world");
        assert_eq!(sanitize_world_folder("正常世界"), "正常世界");
    }

    #[test]
    fn import_zip_world_extracts_prefixed_entries() {
        let dir = tempfile::TempDir::new().unwrap();
        let saves_dir = dir.path().join("saves");
        fs::create_dir_all(&saves_dir).unwrap();

        let level_source = dir.path().join("level.dat");
        write_level_dat(&level_source, "Zip World", 3_000);
        let level_bytes = fs::read(&level_source).unwrap();

        let zip_path = dir.path().join("world.zip");
        {
            let file = File::create(&zip_path).unwrap();
            let mut zip = ZipWriter::new(file);
            let options = SimpleFileOptions::default();
            zip.start_file("my-world/level.dat", options).unwrap();
            zip.write_all(&level_bytes).unwrap();
            zip.start_file("my-world/region/r.0.0.mca", options)
                .unwrap();
            zip.write_all(&[0u8; 16]).unwrap();
            zip.finish().unwrap();
        }

        let imported = import_world_zip(&zip_path, &saves_dir).unwrap();
        assert_eq!(imported, "my-world");
        assert!(saves_dir.join("my-world/level.dat").is_file());
        assert!(saves_dir.join("my-world/region/r.0.0.mca").is_file());
    }

    #[test]
    fn auto_backup_is_skipped_within_cooldown() {
        let now = 1_760_000_000_000;
        assert!(should_skip_auto_backup(Some(now - 5 * 60 * 1000), now));
        assert!(!should_skip_auto_backup(Some(now - 11 * 60 * 1000), now));
        assert!(!should_skip_auto_backup(None, now));
    }

    #[test]
    fn automatic_backups_are_created_for_each_world() {
        let dir = tempfile::TempDir::new().unwrap();
        let saves_dir = dir.path().join("saves");
        let backups_dir = dir.path().join("instance/.ar1s/save-backups");
        fs::create_dir_all(&saves_dir).unwrap();
        make_world(&saves_dir, "world-a", "Alpha", 1_000, false);
        make_world(&saves_dir, "world-b", "Beta", 2_000, false);

        for world in list_world_folders(&saves_dir).unwrap() {
            create_backup_at(
                &saves_dir,
                &backups_dir,
                &world,
                SaveBackupKind::Auto,
                1_760_000_000_000,
                3,
            )
            .unwrap();
        }

        let backups = list_backups_in(&backups_dir).unwrap();
        assert_eq!(backups.len(), 2);
        assert!(
            backups
                .iter()
                .all(|backup| backup.kind == SaveBackupKind::Auto)
        );
    }
}
