//! 与 Java 版本有关的判据
//!
//! 三条规则分别对应：JVM 认不了 class、加载器的 ASM 认不了 class、本次 JDK 高于实例声明

use regex::Regex;
use std::sync::LazyLock;

use super::*;
use crate::services::launcher::java_requirement::class_file_to_java;

/// class 版本号只从日志原文里取，不查表
static RE_CLASS_VERSION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"UnsupportedClassVersionError: (\S+) has been compiled by a more recent version of the Java Runtime \(class file version (\d+)",
    )
    .expect("class 版本表达式合法")
});

/// mixin 内置 ASM 读不了新 JDK 的 class 时，日志会直接报出它不支持的 major 版本
static RE_CLASS_READ_TOO_OLD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"Unsupported class file major version (\d+)").expect("class 读取表达式合法")
});

pub(super) fn rule_class_reader_too_old(lines: &[String], ctx: &LaunchContext) -> Option<Finding> {
    let hits: Vec<(usize, u32)> = lines
        .iter()
        .enumerate()
        .filter_map(|(i, line)| {
            RE_CLASS_READ_TOO_OLD
                .captures(line)
                .and_then(|c| Some((i, c[1].parse().ok()?)))
        })
        .collect();

    let (idx, major) = *hits.first()?;
    let java = class_file_to_java(major)?;
    let matches_runtime = ctx.used_java_version == Some(java);

    // 本次 Java 正好就是编译该类的版本，矛盾不在“Java 太旧”，而在读类的那套工具（加载器自带的 ASM）
    let upgrade_loader = format!(
        "本次已经是 Java {}，不是 Java 选错，而是 {} 自带的 ASM 太旧、读不了新版本游戏的类。把 {} 升级到最新版再启动。",
        java, ctx.loader, ctx.loader
    );

    let suggestion = if !matches_runtime {
        // JVM 自己都认不了这个 class：确实是 Java 太旧
        match ctx.declared_java {
            Some(declared) => format!(
                "把启动该实例的 Java 换成版本 JSON 声明的 Java {}。",
                declared
            ),
            None => "换成该加载器支持的更老 Java 版本启动。".to_string(),
        }
    } else {
        match ctx.declared_java {
            Some(declared) if declared == java => upgrade_loader,
            // 实例本来就声明了更老的 Java，退回声明版本同样能解决，优先给代价小的那条
            Some(declared) => format!(
                "把启动该实例的 Java 退回版本 JSON 声明的 Java {}；确实要用 Java {} 的话，得先把 {} 升级到支持它的版本。",
                declared, java, ctx.loader
            ),
            None => upgrade_loader,
        }
    };

    Some(Finding {
        kind: FindingKind::ClassReaderTooOld,
        role: FindingRole::Root,
        summary: format!("加载器无法读取 Java {} 的类文件", java),
        conclusion: format!(
            "加载器读取类元数据时遇到 major {}（Java {}）编译的类文件{}；同类报错 {} 次，最早出现在第 {} 行",
            major,
            java,
            if matches_runtime {
                format!("，本次用的正是 Java {}", java)
            } else {
                String::new()
            },
            hits.len(),
            idx + 1
        ),
        confidence: if matches_runtime {
            Confidence::High
        } else {
            Confidence::Medium
        },
        culprit_label: "本次用的 Java",
        culprits: vec![ctx.used_java_path.clone()],
        evidence: vec![evidence_at(lines, idx)],
        suggestion,
    })
}

pub(super) fn rule_class_version(lines: &[String], ctx: &LaunchContext) -> Option<Finding> {
    let (idx, (class, major)) = find_signal(lines, |line| {
        let cap = RE_CLASS_VERSION.captures(line)?;
        Some((cap[1].to_string(), cap[2].parse().ok()?))
    })?;

    let required = class_file_to_java(major);
    let suggestion = match required {
        Some(java) => format!(
            "把启动该实例的 Java 换成 {} 或更高（版本 JSON 声明 {}）",
            java,
            ctx.declared_java.unwrap_or(java)
        ),
        None => "把启动该实例的 Java 换成能读该 class 版本的更高版本".to_string(),
    };

    Some(Finding {
        kind: FindingKind::ClassVersion,
        role: FindingRole::Root,
        summary: format!("有类按 class 版本 {} 编译，本次 Java 太旧", major),
        conclusion: format!(
            "{} 按 class 版本 {} 编译，本次 Java {} 无法加载",
            class,
            major,
            ctx.used_java_version
                .map_or("版本未知".to_string(), |v| v.to_string())
        ),
        confidence: Confidence::High,
        culprit_label: "问题类",
        culprits: vec![class],
        evidence: vec![evidence_at(lines, idx)],
        suggestion,
    })
}

/// 本次运行时高于实例声明的运行时，是「起不来」里最常被误认成模组坏了的一条
pub(crate) fn rule_java_above_declared(ctx: &LaunchContext) -> Option<Finding> {
    let used = ctx.used_java_version?;
    let declared = ctx.declared_java?;
    if used <= declared {
        return None;
    }

    Some(Finding {
        kind: FindingKind::JavaVersion,
        role: FindingRole::Root,
        summary: format!("本次 Java {} 高于实例声明的 {}", used, declared),
        conclusion: format!(
            "本次使用 Java {} 启动，实例声明为 Java {}；较新的 JDK 可能超出该加载器的兼容范围",
            used, declared
        ),
        confidence: Confidence::Medium,
        culprit_label: "",
        culprits: Vec::new(),
        evidence: Vec::new(),
        suggestion: format!(
            "先用 Java {} 启动一次做对照：能启动说明问题在本次 JDK，仍然失败则排除 Java 因素。",
            declared
        ),
    })
}
