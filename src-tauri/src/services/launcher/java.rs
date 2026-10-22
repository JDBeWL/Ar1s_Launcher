//! Java 解析与玩家 UUID 模块
//!
//! 相对路径解析、候选装配（并行探测 + 缓存）、按判据自动挑选

use super::java_requirement::{self, JavaRequirement};
use crate::errors::LauncherError;
use crate::models::GameConfig;
use crate::services::config::load_config;
pub use crate::services::java::JavaArch;
use crate::services::java::{JavaProbe, probe_java, resolve_java_exe};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use uuid::Uuid;

pub fn generate_offline_uuid(username: &str) -> String {
    if let Ok(config) = load_config()
        && let (Some(saved_username), Some(saved_uuid)) = (&config.username, &config.uuid)
        && saved_username == username
    {
        return saved_uuid.clone();
    }

    Uuid::new_v3(
        &Uuid::NAMESPACE_DNS,
        format!("OfflinePlayer:{}", username).as_bytes(),
    )
    .to_string()
}

/// 将相对路径解析为绝对路径（相对于可执行文件所在目录）
pub fn resolve_path(path: &str) -> PathBuf {
    let p = PathBuf::from(path);
    if p.is_absolute() {
        return p;
    }

    if let Ok(exe_dir) = std::env::current_exe()
        .map_err(|e| format!("无法获取可执行文件路径: {}", e))
        .and_then(|exe| {
            exe.parent()
                .map(|p| p.to_path_buf())
                .ok_or("无法获取目录".to_string())
        })
    {
        return exe_dir.join(&p);
    }

    p
}

/// 解析 Java 可执行文件路径（支持相对路径与安装目录）
pub fn resolve_java_path(config: &GameConfig) -> Result<String, LauncherError> {
    if let Some(config_path) = &config.java_path
        && !config_path.is_empty()
    {
        // 用户很可能填的是 JAVA_HOME 而不是 exe，这里统一落成可执行文件
        let resolved = resolve_java_exe(config_path);
        if resolved.exists() {
            return Ok(resolved.to_string_lossy().into_owned());
        }
        if PathBuf::from(config_path).is_relative() {
            return Err(LauncherError::Custom(format!(
                "Java 相对路径 '{}' 解析后不存在: {}",
                config_path,
                resolved.display()
            )));
        }
    }

    if probe_java("java").is_some() {
        Ok("java".to_string())
    } else {
        Err(LauncherError::Custom(
            "未在配置中找到有效的Java路径，且系统PATH中也未找到Java。".to_string(),
        ))
    }
}

pub fn resolve_game_dir(game_dir: &str) -> PathBuf {
    resolve_path(game_dir)
}

/// 返回 Java 主版本号；Java 8 的 "1.8.0_x" 版本串按 8 计（解析规则见探测层）
pub fn detect_java_version(java_path: &str) -> Option<u32> {
    probe_java(java_path)?.version
}

/// 没有已知上限时的哨兵值
pub const NO_UPPER_BOUND: u32 = 99;

/// 实例的 Java 判据：读版本 JSON 声明 + mods 实测，不再靠版本表猜
pub fn profile_for(version_id: &str, game_dir: &Path) -> java_requirement::InstanceProfile {
    java_requirement::analyze(version_id, game_dir)
}

/// 一个真正可用的 Java：路径、大版本、位数
#[derive(Debug, Clone)]
pub struct JavaCandidate {
    pub path: String,
    pub version: u32,
    pub arch: JavaArch,
    /// 预览版：同版本时排在正式版之后
    pub is_ea: bool,
    /// 用户在配置里显式指定过：同分时优先，避免每次启动换一份
    pub from_config: bool,
}

#[derive(Debug, Clone)]
pub struct JavaMatchResult {
    pub path: String,
    pub version: u32,
    /// 是否选到了能满足判据的那个版本（被模组抬高下限时也算满足）
    pub is_optimal: bool,
    pub warning: Option<String>,
}

/// 候选 = 配置指定 + 手动添加的路径 + 全盘扫描结果
///
/// 探测要起进程，一次启动会问好几轮，所以这里并行探测，且命中探测缓存时几乎零成本。
/// javapath 这类转发器会被换回真实安装，同一个 JVM 不会既以转发器、又以真身出现两遍
pub fn collect_candidates(config: &GameConfig) -> Vec<JavaCandidate> {
    let mut inputs: Vec<(String, bool)> = Vec::new();
    let mut seen_exe: HashSet<String> = HashSet::new();

    let mut push = |path: &str, from_config: bool| {
        if path.trim().is_empty() {
            return;
        }
        let exe = resolve_java_exe(path);
        if seen_exe.insert(crate::services::java::exe_key(&exe)) {
            inputs.push((path.to_string(), from_config));
        }
    };

    if let Some(ref java_path) = config.java_path {
        push(java_path, true);
    }

    for path in &config.custom_java_paths {
        push(path, true);
    }

    if let Ok(installations) = crate::services::java::find_java_installations_command_sync() {
        for path in &installations {
            push(path, false);
        }
    }

    let resolved: Vec<Option<(PathBuf, JavaProbe)>> = std::thread::scope(|scope| {
        let handles: Vec<_> = inputs
            .iter()
            .map(|(path, _)| {
                let path = path.clone();
                scope.spawn(move || crate::services::java::probe_java_resolved(&path))
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().ok().flatten())
            .collect()
    });

    let mut seen_resolved: HashSet<String> = HashSet::new();

    inputs
        .into_iter()
        .zip(resolved)
        .filter_map(|((_, from_config), hit)| {
            let (exe, probe) = hit?;
            let version = probe.version?;

            // 换回真身后可能与另一条候选重合
            if !seen_resolved.insert(crate::services::java::exe_key(&exe)) {
                return None;
            }

            Some(JavaCandidate {
                path: exe.to_string_lossy().replace("\\", "/"),
                version,
                arch: probe.arch,
                is_ea: probe.is_ea(),
                from_config,
            })
        })
        .collect()
}

/// 选取策略：在允许区间里取离下限最近的一个，同版本再按位数/正式版/来源排
///
/// 下限本身已经是 max(声明版本, 模组实测)，所以“等于 target”不再是唯一正解：
/// 模组把下限抬到 21 时，声明值 8 根本进不了允许区间
pub fn choose_java(req: &JavaRequirement, candidates: &[JavaCandidate]) -> Option<JavaMatchResult> {
    let allowed: Vec<&JavaCandidate> = candidates
        .iter()
        .filter(|c| java_requirement::java_allowed(req, c.version))
        .collect();

    if allowed.is_empty() {
        return None;
    }

    let floor = req.min;
    let chosen = allowed.iter().copied().min_by(|a, b| {
        (a.version - floor)
            .cmp(&(b.version - floor))
            // 同版本优先 64 位：32 位 JVM 连 1.5G 堆都开不出来
            .then_with(|| a.arch.rank().cmp(&b.arch.rank()))
            .then_with(|| a.is_ea.cmp(&b.is_ea))
            // 用户显式指定的那份排前面，行为才稳定
            .then_with(|| b.from_config.cmp(&a.from_config))
            .then_with(|| a.path.cmp(&b.path))
    })?;

    let matched_floor = chosen.version == req.min;
    let raised_by_mods = req.min > req.target;
    let band = match req.hard_max {
        Some(max) => format!("{}~{}", req.min, max),
        None => format!("{}+", req.min),
    };

    let warning = if matched_floor && !raised_by_mods {
        None
    } else if matched_floor {
        let text = format!(
            "模组把 Java 下限抬到 {}（Mojang 声明 {}），已选用 Java {}",
            req.min, req.target, chosen.version
        );
        log::info!("{}", text);
        Some(text)
    } else if raised_by_mods {
        let text = format!(
            "模组要求 Java ≥ {}（Mojang 声明 {}），本机没有 Java {}，改用 Java {}",
            req.min, req.target, req.min, chosen.version
        );
        log::warn!("{}", text);
        Some(text)
    } else {
        let text = format!(
            "本机没有该实例声明的 Java {}，改用 Java {}（允许区间 {}）",
            req.target, chosen.version, band
        );
        log::warn!("{}", text);
        Some(text)
    };

    Some(JavaMatchResult {
        path: chosen.path.clone(),
        version: chosen.version,
        is_optimal: matched_floor,
        warning,
    })
}

/// 判据不过时该怎么做
#[derive(Debug)]
pub enum JavaGate {
    /// 用户确认过要照用当前这份，报告只为留日志
    KeepCurrent(String),
    SwitchTo(JavaMatchResult),
    /// 一个能用的都没有，附带诊断报告
    Refuse(String),
}

/// 当前 Java 不满足判据时的处置：用户的选择优先于自动匹配
pub fn resolve_gate(
    profile: &java_requirement::InstanceProfile,
    current: (&str, u32),
    force: bool,
    candidates: &[JavaCandidate],
) -> JavaGate {
    let report = || java_requirement::build_report(profile, Some(current), candidates);

    if force {
        return JavaGate::KeepCurrent(report());
    }

    match choose_java(&profile.requirement, candidates) {
        Some(better) => JavaGate::SwitchTo(better),
        None => JavaGate::Refuse(report()),
    }
}

/// 装配候选并按实例判据挑选
pub fn auto_match_java(profile: &java_requirement::InstanceProfile) -> Option<JavaMatchResult> {
    let config = load_config().ok()?;
    let candidates = collect_candidates(&config);

    if candidates.is_empty() {
        log::warn!("未找到任何可用的 Java 安装");
        return None;
    }

    choose_java(&profile.requirement, &candidates)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::launcher::java_requirement::JavaRequirement;

    fn candidate(path: &str, version: u32, arch: JavaArch, from_config: bool) -> JavaCandidate {
        JavaCandidate {
            path: path.to_string(),
            version,
            arch,
            is_ea: false,
            from_config,
        }
    }

    fn req(min: u32, target: u32, hard_max: Option<u32>) -> JavaRequirement {
        JavaRequirement {
            target,
            min,
            hard_max,
            reasons: Vec::new(),
        }
    }

    fn profile(req: JavaRequirement) -> java_requirement::InstanceProfile {
        let declared = req.target;
        java_requirement::InstanceProfile {
            requirement: req,
            loader: java_requirement::LoaderKind::Forge,
            mods: Vec::new(),
            declared_java: Some(declared),
            resolved: true,
            version_id: "1.13.2-Forge".to_string(),
            game_dir: std::path::PathBuf::from("C:/game"),
        }
    }

    /// 用户选了「仍用当前 Java」时，后端不许再替他换一份
    #[test]
    fn test_user_forced_java_wins_over_auto_match() {
        // 1.13.2-Forge：上限 8，本次用的是 25，本机还确实有一份 8
        let inst = profile(req(8, 8, Some(8)));
        let candidates = vec![candidate(
            "C:/jdk8/bin/java.exe",
            8,
            JavaArch::Bits64,
            false,
        )];

        match resolve_gate(&inst, ("D:/jdk25/bin/java.exe", 25), true, &candidates) {
            JavaGate::KeepCurrent(report) => {
                assert!(report.contains("Java 25"), "{}", report);
            }
            other => panic!("用户确认过就该照用当前 Java，实际 {:?}", other),
        }

        // 没确认时才自动换
        match resolve_gate(&inst, ("D:/jdk25/bin/java.exe", 25), false, &candidates) {
            JavaGate::SwitchTo(matched) => assert_eq!(matched.version, 8),
            other => panic!("{:?}", other),
        }

        // 一份能用的都没有：拒绝，并把报告带回去
        let only_newer = vec![candidate(
            "D:/jdk25/bin/java.exe",
            25,
            JavaArch::Bits64,
            false,
        )];
        match resolve_gate(&inst, ("D:/jdk25/bin/java.exe", 25), false, &only_newer) {
            JavaGate::Refuse(report) => {
                assert!(report.contains("Java 8"), "{}", report);
                assert!(report.contains("先在设置里补一份"), "{}", report);
            }
            other => panic!("{:?}", other),
        }
    }

    #[test]
    fn test_same_version_prefers_64_bit() {
        // 路径字典序把 32 位排在前面，只有按位数排才会选对
        let candidates = vec![
            candidate("C:/jdk8-32/bin/java.exe", 8, JavaArch::Bits32, false),
            candidate("C:/jdk8-64/bin/java.exe", 8, JavaArch::Bits64, false),
        ];

        let chosen = choose_java(&req(8, 8, None), &candidates).unwrap();

        assert!(chosen.path.contains("64"), "{}", chosen.path);
        assert!(chosen.is_optimal);
    }

    #[test]
    fn test_closest_to_floor_beats_newer_versions() {
        // 1.20.1 声明 17，装了 17 和 21 时不该随手拿 21
        let candidates = vec![
            candidate("C:/jdk21/bin/java.exe", 21, JavaArch::Bits64, false),
            candidate("C:/jdk17/bin/java.exe", 17, JavaArch::Bits64, false),
        ];

        let chosen = choose_java(&req(17, 17, None), &candidates).unwrap();

        assert_eq!(chosen.version, 17);
        assert!(chosen.warning.is_none());
    }

    #[test]
    fn test_legacy_forge_never_gets_a_newer_jdk() {
        // 1.12.2 的 Forge 上限就是 8：装了 17 也不能拿来凑
        let candidates = vec![
            candidate("C:/jdk17/bin/java.exe", 17, JavaArch::Bits64, true),
            candidate("C:/jdk8/bin/java.exe", 8, JavaArch::Bits64, true),
        ];

        assert_eq!(
            choose_java(&req(8, 8, Some(8)), &candidates).map(|c| c.version),
            Some(8)
        );
        assert!(
            choose_java(
                &req(8, 8, Some(8)),
                &[candidate(
                    "C:/jdk17/bin/java.exe",
                    17,
                    JavaArch::Bits64,
                    true
                )]
            )
            .is_none(),
            "没有 8 就该返回 None，让调用方给出报告"
        );
    }

    #[test]
    fn test_mods_raising_the_floor_explains_itself() {
        // 声明 8，但模组把下限抬到 21：这不是“缺 Java 8”，而是模组要求
        let candidates = vec![
            candidate("C:/jdk8/bin/java.exe", 8, JavaArch::Bits64, false),
            candidate("C:/jdk21/bin/java.exe", 21, JavaArch::Bits64, false),
        ];

        let chosen = choose_java(&req(21, 8, None), &candidates).unwrap();

        assert_eq!(chosen.version, 21);
        assert!(chosen.is_optimal, "模组抬高的下限被满足就算最优");
        let warning = chosen.warning.unwrap();
        assert!(warning.contains("模组"), "{}", warning);
        assert!(warning.contains("21"), "{}", warning);
    }

    #[test]
    fn test_mods_floor_without_exact_version_says_so() {
        // 模组要求 21，本机只有 26：别把它说成“缺声明版本”，而是缺模组要的那个
        let candidates = vec![candidate(
            "C:/jdk26/bin/java.exe",
            26,
            JavaArch::Bits64,
            false,
        )];

        let chosen = choose_java(&req(21, 8, None), &candidates).unwrap();

        assert_eq!(chosen.version, 26);
        assert!(!chosen.is_optimal);
        let warning = chosen.warning.unwrap();
        assert!(warning.contains("模组要求"), "{}", warning);
        assert!(warning.contains("Java 21"), "{}", warning);
    }

    #[test]
    fn test_missing_declared_version_is_reported_as_a_switch() {
        // 声明 17，本机只有 21：要说清是“没有 17”而不是“模组要求”
        let candidates = vec![candidate(
            "C:/jdk21/bin/java.exe",
            21,
            JavaArch::Bits64,
            false,
        )];

        let chosen = choose_java(&req(17, 17, None), &candidates).unwrap();

        assert_eq!(chosen.version, 21);
        assert!(!chosen.is_optimal);
        let warning = chosen.warning.unwrap();
        assert!(warning.contains("没有该实例声明的 Java 17"), "{}", warning);
    }

    #[test]
    fn test_config_path_breaks_the_tie_stably() {
        // 同版本同位数：优先用户挑的那份，否则每次启动可能换一个
        let candidates = vec![
            candidate("C:/aaa/jdk17/bin/java.exe", 17, JavaArch::Bits64, false),
            candidate("C:/zzz/jdk17/bin/java.exe", 17, JavaArch::Bits64, true),
        ];

        let chosen = choose_java(&req(17, 17, None), &candidates).unwrap();

        assert_eq!(chosen.path, "C:/zzz/jdk17/bin/java.exe");
    }

    #[test]
    fn test_preview_build_loses_to_release_of_same_version() {
        let mut ea = candidate("C:/jdk24-ea/bin/java.exe", 24, JavaArch::Bits64, false);
        ea.is_ea = true;
        let release = candidate("C:/jdk24/bin/java.exe", 24, JavaArch::Bits64, false);

        let chosen = choose_java(&req(24, 24, None), &[ea, release]).unwrap();

        assert_eq!(chosen.path, "C:/jdk24/bin/java.exe");
    }
}
