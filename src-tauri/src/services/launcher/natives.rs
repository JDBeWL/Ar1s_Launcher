//! Natives 库解压逻辑
//!
//! 按 OS 与 ${arch} 选 natives jar 解压到版本目录供 java.library.path，非空目录直接复用

use crate::errors::LauncherError;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// 解压 natives 并返回目录
///
/// 锁必须异步获取：`file_lock::acquire` 是 `thread::sleep` 轮询，放在 async fn 里
/// 会把一个 tokio worker 原地睡满整个超时（另一个实例正在解压时最坏 30 秒），
/// 期间所有其他 command 一起卡住
pub async fn extract_natives(
    version_json: &serde_json::Value,
    game_dir: &Path,
    version_dir: &Path,
    libraries_base_dir: &Path,
    current_os: &str,
    window: &tauri::Window,
) -> Result<PathBuf, LauncherError> {
    // 同一实例多开时只能有一个进程在解压，否则第二个进程会读到写了一半的文件。
    // 锁不能放在实例目录里：Windows 上目录内有打开的文件就重命名不了，
    // 会挡住删除/重命名实例，所以统一放到游戏目录的锁目录
    let lock_path = crate::services::instance::lock_dir(game_dir).join("natives.lock");
    let _natives_lock =
        crate::utils::file_lock::acquire_async(&lock_path, std::time::Duration::from_secs(30))
            .await?;

    let version_json = version_json.clone();
    let version_dir = version_dir.to_path_buf();
    let libraries_base_dir = libraries_base_dir.to_path_buf();
    let current_os = current_os.to_string();
    let window = window.clone();

    tokio::task::spawn_blocking(move || {
        let emit = |event: &str, msg: String| {
            let _ = tauri::Emitter::emit(&window, event, msg);
        };
        extract_natives_sync(
            &version_json,
            &version_dir,
            &libraries_base_dir,
            &current_os,
            &emit,
        )
    })
    .await
    .map_err(|e| LauncherError::Custom(format!("Natives 解压任务异常终止: {}", e)))?
}

/// 同步解压实现，调用方负责加锁并放进阻塞线程池
fn extract_natives_sync(
    version_json: &serde_json::Value,
    version_dir: &Path,
    libraries_base_dir: &Path,
    current_os: &str,
    emit: &impl Fn(&str, String),
) -> Result<PathBuf, LauncherError> {
    let natives_dir = version_dir.join("natives");
    emit(
        "log-debug",
        format!("Natives目录: {}", natives_dir.display()),
    );

    if natives_dir.exists() {
        if fs::read_dir(&natives_dir)
            .map(|mut d| d.next().is_some())
            .unwrap_or(false)
        {
            emit(
                "log-debug",
                "Natives目录已存在且非空，跳过解压（使用缓存）".to_string(),
            );
            return Ok(natives_dir);
        }
        fs::remove_dir_all(&natives_dir)?;
    }
    fs::create_dir_all(&natives_dir)?;

    let Some(libraries) = version_json["libraries"].as_array() else {
        return Ok(natives_dir);
    };

    let mut extracted = 0usize;

    for lib in libraries {
        let Some(natives) = lib.get("natives") else {
            continue;
        };

        let Some(os_classifier) = natives.get(current_os).and_then(|v| v.as_str()) else {
            continue;
        };

        // 分类符里的 ${arch} 需替换成运行架构
        let arch = if std::env::consts::ARCH.contains("64") {
            "64"
        } else {
            "32"
        };
        let classifier = os_classifier.replace("${arch}", arch);

        let Some(artifact) = lib
            .get("downloads")
            .and_then(|d| d.get("classifiers"))
            .and_then(|c| c.get(&classifier))
        else {
            continue;
        };

        let lib_path = libraries_base_dir.join(artifact["path"].as_str().unwrap_or(""));

        if !lib_path.exists() {
            emit(
                "log-error",
                format!("Natives库文件不存在: {}", lib_path.display()),
            );
            return Err(LauncherError::Custom(format!(
                "Natives库文件不存在: {}",
                lib_path.display()
            )));
        }

        extract_native_jar(&lib_path, &natives_dir, lib)?;
        extracted += 1;
    }

    // 逐库、逐条 emit 会在启动瞬间刷出上百个 IPC 事件；汇总成一条，
    // 排障需要的「解压了几个库 / 目录里有什么」都还在
    emit(
        "log-debug",
        format!(
            "Natives 解压完成：{} 个库 -> {}",
            extracted,
            natives_dir.display()
        ),
    );
    log_natives_dir_contents(&natives_dir, emit);

    Ok(natives_dir)
}

fn extract_native_jar(
    lib_path: &Path,
    natives_dir: &Path,
    lib: &serde_json::Value,
) -> Result<(), LauncherError> {
    let file = fs::File::open(lib_path)?;
    let mut archive = zip::ZipArchive::new(file)?;

    for i in 0..archive.len() {
        let mut file = archive.by_index(i)?;
        let entry_name = file.name()?.to_string();

        if should_exclude_entry(&entry_name, lib) {
            continue;
        }

        if entry_name.ends_with('/') {
            continue;
        }

        // 取出最后一段文件名，避免嵌套目录
        let file_stem = Path::new(&entry_name)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(&entry_name);

        let outpath = natives_dir.join(file_stem);

        if let Some(p) = outpath.parent()
            && !p.exists()
        {
            fs::create_dir_all(p)?;
        }

        let mut outfile = fs::File::create(&outpath)?;
        io::copy(&mut file, &mut outfile)?;
    }

    Ok(())
}

fn should_exclude_entry(entry_name: &str, lib: &serde_json::Value) -> bool {
    let Some(extract_rules) = lib.get("extract") else {
        return false;
    };

    let Some(exclude) = extract_rules.get("exclude").and_then(|e| e.as_array()) else {
        return false;
    };

    exclude
        .iter()
        .any(|v| entry_name.starts_with(v.as_str().unwrap_or("")))
}

fn log_natives_dir_contents(natives_dir: &Path, emit: &impl Fn(&str, String)) {
    if !natives_dir.exists() {
        return;
    }

    if let Ok(entries) = fs::read_dir(natives_dir) {
        let names: Vec<String> = entries
            .flatten()
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();

        emit(
            "log-debug",
            format!("natives 目录内容: [{}]", names.join(", ")),
        );
    }
}
