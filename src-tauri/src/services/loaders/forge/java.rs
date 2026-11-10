//! 安装器与 processor 需要哪个 Java

use crate::errors::LauncherError;
use crate::services::launcher::java::JavaCandidate;
use std::fs;
use std::io::Read;
use std::path::Path;
use zip::ZipArchive;

/// 安装器自己也是 jar：它编译到的 class 版本、以及该 MC 版本的常规 Java，
/// 两者取高——1.20.1 的安装器字节码可能是 8，但运行时按 17 才稳
pub(crate) fn installer_requirement(installer_path: &Path, mc_version: &str) -> u32 {
    let by_class =
        crate::services::launcher::java_requirement::jar_java_version(installer_path).unwrap_or(0);
    let by_mc = crate::services::launcher::java_requirement::infer_java_for_version(mc_version)
        .unwrap_or(0);

    by_class.max(by_mc).max(8)
}

/// 挑能跑安装器的那份：优先精确命中，其次够用的最低版。
/// 旧 Forge 安装器在 Java 9+ 上直接 UnsupportedClassVersionError，不能随手往上挑
pub(crate) fn choose_installer_java(
    installer_path: &Path,
    mc_version: &str,
    candidates: &[JavaCandidate],
) -> Option<String> {
    let required = installer_requirement(installer_path, mc_version);

    // Java 8 时代的安装器在 9+ 上必然 UnsupportedClassVersionError / 找不到 Nashorn，
    // 与其白跑一轮（还可能弹个 GUI），不如直接交给手动安装
    if required <= 8 {
        return candidates
            .iter()
            .find(|candidate| candidate.version == 8)
            .map(|candidate| candidate.path.clone());
    }

    if let Some(exact) = candidates.iter().find(|c| c.version == required) {
        return Some(exact.path.clone());
    }

    candidates
        .iter()
        .filter(|c| c.version > required)
        .min_by(|a, b| {
            a.version
                .cmp(&b.version)
                .then_with(|| a.arch.rank().cmp(&b.arch.rank()))
                .then_with(|| a.path.cmp(&b.path))
        })
        .map(|c| c.path.clone())
}

pub(crate) fn get_jar_main_class(jar_path: &Path) -> Result<String, LauncherError> {
    let file = fs::File::open(jar_path)?;
    let mut archive = ZipArchive::new(file)?;

    let mut manifest = archive
        .by_name("META-INF/MANIFEST.MF")
        .map_err(|_| LauncherError::Custom("JAR 中没有 MANIFEST.MF".to_string()))?;

    let mut content = String::new();
    manifest.read_to_string(&mut content)?;

    for line in content.lines() {
        if line.starts_with("Main-Class:") {
            return Ok(line.trim_start_matches("Main-Class:").trim().to_string());
        }
    }

    Err(LauncherError::Custom(
        "MANIFEST 中没有 Main-Class".to_string(),
    ))
}

/// processor 是外部 jar，它自己编译到的 class 版本决定了能跑它的 Java：
/// 配置里那份够用就不动，否则从候选里挑够用的最低版
pub(crate) fn choose_processor_java(
    required: u32,
    fallback: (&str, Option<u32>),
    candidates: &[JavaCandidate],
) -> Result<String, String> {
    if fallback.1.is_some_and(|ver| ver >= required) {
        return Ok(fallback.0.to_string());
    }

    let mut usable: Vec<&JavaCandidate> = candidates
        .iter()
        .filter(|candidate| candidate.version >= required)
        .collect();

    if usable.is_empty() {
        let found = if candidates.is_empty() {
            "没发现任何可用的 Java".to_string()
        } else {
            let mut versions: Vec<u32> = candidates.iter().map(|c| c.version).collect();
            versions.sort_unstable();
            versions.dedup();
            format!(
                "本机只有 Java {}",
                versions
                    .iter()
                    .map(|v| v.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        return Err(format!(
            "安装器需要 Java {} 来跑 processor，{}；请在设置里补一份再装",
            required, found
        ));
    }

    usable.sort_by(|a, b| {
        a.version
            .cmp(&b.version)
            .then_with(|| a.arch.rank().cmp(&b.arch.rank()))
            .then_with(|| a.path.cmp(&b.path))
    });

    Ok(usable[0].path.clone())
}
