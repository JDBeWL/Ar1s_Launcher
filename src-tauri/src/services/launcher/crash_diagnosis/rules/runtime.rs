//! 运行时环境问题：堆内存、原生库、SecurityManager、模块系统

use regex::Regex;
use std::sync::LazyLock;

use super::*;

static RE_OUT_OF_MEMORY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"OutOfMemoryError: (.+)").expect("内存表达式合法"));

static RE_UNSATISFIED_LINK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"UnsatisfiedLinkError: (.+)").expect("原生库表达式合法"));

static RE_NATIVE_LIB: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[\w.-]+\.(?:dll|dylib|so(?:\.\d+)?)").expect("库文件名表达式合法")
});

/// JDK 24 起 setSecurityManager 直接抛异常，老 Forge 必挂在这
static RE_SECURITY_MANAGER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"UnsupportedOperationException: The Security Manager is \w+")
        .expect("SecurityManager 表达式合法")
});

/// Java 9+ 的模块系统：老加载器/模组靠反射摸 JDK 内部类，会被 opens 限制拦住
static RE_MODULE_ACCESS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"InaccessibleObjectException|does not "opens "#).expect("模块访问表达式合法")
});

pub(super) fn rule_memory(lines: &[String], ctx: &LaunchContext) -> Option<Finding> {
    let (idx, detail) = find_signal(lines, |line| {
        RE_OUT_OF_MEMORY
            .captures(line)
            .map(|cap| cap[1].to_string())
    })?;

    Some(Finding {
        kind: FindingKind::Memory,
        role: FindingRole::Root,
        summary: "JVM 内存耗尽".to_string(),
        conclusion: format!("JVM 内存耗尽：{}（本次分配 {} MB）", detail, ctx.memory_mb),
        confidence: Confidence::High,
        culprit_label: "",
        culprits: Vec::new(),
        evidence: vec![evidence_at(lines, idx)],
        suggestion: "适当调高最大堆内存；若已在 8192 MB 以上仍失败，考虑减少模组数量。".to_string(),
    })
}

pub(super) fn rule_natives(lines: &[String]) -> Option<Finding> {
    let (idx, (detail, lib)) = find_signal(lines, |line| {
        let cap = RE_UNSATISFIED_LINK.captures(line)?;
        let detail = cap[1].to_string();
        let lib = RE_NATIVE_LIB
            .find(&detail)
            .map(|m| m.as_str().to_string())
            .unwrap_or_else(|| detail.clone());
        Some((detail, lib))
    })?;

    Some(Finding {
        kind: FindingKind::Natives,
        role: FindingRole::Root,
        summary: "原生库加载失败".to_string(),
        conclusion: format!("原生库加载失败：{}", detail),
        confidence: Confidence::High,
        culprit_label: "问题文件",
        culprits: vec![lib],
        evidence: vec![evidence_at(lines, idx)],
        suggestion: "删除实例目录下的 natives 目录后重新启动，由启动器重新解压；若失败的是显卡相关 dll，更新显卡驱动。".to_string(),
    })
}

pub(super) fn rule_security_manager(lines: &[String], ctx: &LaunchContext) -> Option<Finding> {
    let (idx, _) = find_signal(lines, |line| {
        RE_SECURITY_MANAGER.is_match(line).then(|| line.to_string())
    })?;

    let fallback = match ctx.declared_java {
        Some(declared) => format!("换回实例声明的 Java {} 启动", declared),
        None => "把启动该实例的 Java 换成该加载器支持的更老版本".to_string(),
    };

    Some(Finding {
        kind: FindingKind::SecurityManager,
        role: FindingRole::Root,
        summary: "本次的 JDK 已移除 SecurityManager".to_string(),
        conclusion: "加载器的启动流程需要安装 SecurityManager，本次的 JDK 已移除该 API".to_string(),
        confidence: Confidence::High,
        culprit_label: "本次用的 Java",
        culprits: vec![ctx.used_java_path.clone()],
        evidence: vec![evidence_at(lines, idx)],
        suggestion: format!("这是 JDK 自身移除的 API，改模组无效；{}", fallback),
    })
}

/// 反射被模块系统拦下、且本次 Java 比声明的新 —— 基本可以断定是 JDK 太新，不是模组坏了
pub(super) fn rule_module_access(lines: &[String], ctx: &LaunchContext) -> Option<Finding> {
    let (idx, _) = find_signal(lines, |line| RE_MODULE_ACCESS.is_match(line).then_some(()))?;
    let used = ctx.used_java_version?;
    let declared = ctx.declared_java?;
    if used <= declared {
        return None;
    }

    Some(Finding {
        kind: FindingKind::JavaVersion,
        role: FindingRole::Root,
        summary: format!("Java {} 的模块系统拒绝了反射访问", used),
        conclusion: format!(
            "本次用 Java {} 启动，实例声明为 Java {}：加载器要反射 JDK 内部类，被模块系统的 opens 限制拦下了",
            used, declared
        ),
        confidence: Confidence::High,
        culprit_label: "本次用的 Java",
        culprits: vec![ctx.used_java_path.clone()],
        evidence: vec![evidence_at(lines, idx)],
        suggestion: format!(
            "改用 Java {} 启动；关模块系统或加 --add-opens 都不是正解",
            declared
        ),
    })
}
