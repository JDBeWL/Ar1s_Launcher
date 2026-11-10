//! classpath 与主类问题：参数拼装错误、加载器 jar 缺失、前置模组没装

use regex::Regex;
use std::sync::LazyLock;

use super::*;

/// JVM 自己的报错会跟着系统语言走，中英文都要认
static RE_MAIN_CLASS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:Could not find or load main class|找不到或无法加载主类)\s*:?\s*([\w.$=/-]+)")
        .expect("主类缺失表达式合法")
});

static RE_MISSING_CLASS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:NoClassDefFoundError|ClassNotFoundException): ([\w./$]+)")
        .expect("缺类表达式合法")
});

pub(super) fn rule_main_class_missing(lines: &[String]) -> Option<Finding> {
    let (idx, class) = find_signal(lines, |line| {
        RE_MAIN_CLASS.captures(line).map(|cap| cap[1].to_string())
    })?;

    // 被当成主类的那串东西如果长得像 JVM 参数，说明开关和值在合并时被拆开了
    let looks_like_value = class.contains('=') || class.contains('/');

    Some(Finding {
        kind: FindingKind::MainClass,
        role: FindingRole::Root,
        summary: "JVM 找不到要启动的主类".to_string(),
        conclusion: if looks_like_value {
            format!(
                "JVM 把 `{}` 当成了主类 —— 这是某个 JVM 参数的值，说明它前面的开关丢了",
                class
            )
        } else {
            format!("JVM 找不到主类 `{}`，classpath 里可能缺了加载器 jar", class)
        },
        confidence: Confidence::High,
        culprit_label: "主类",
        culprits: vec![class],
        evidence: vec![evidence_at(lines, idx)],
        suggestion: if looks_like_value {
            "启动参数的合并出了问题，重建这个实例通常就能恢复。".to_string()
        } else {
            "检查实例的版本 JSON 是否完整；重建实例通常能恢复。".to_string()
        },
    })
}

pub(super) fn rule_missing_class(lines: &[String]) -> Option<Finding> {
    let (idx, class) = find_signal(lines, |line| {
        // mixin 会探测可选目标并打同名 WARN，健康启动里也常见；真正缺类会带异常栈
        if line.contains("Error loading class:") {
            return None;
        }
        RE_MISSING_CLASS
            .captures(line)
            .map(|cap| cap[1].to_string())
    })?;

    Some(Finding {
        kind: FindingKind::MissingClass,
        role: FindingRole::Root,
        summary: "运行时找不到某个类".to_string(),
        conclusion: format!("运行时找不到类 {}", class),
        confidence: Confidence::Medium,
        culprit_label: "问题类",
        culprits: vec![class],
        evidence: vec![evidence_at(lines, idx)],
        suggestion: "核对该模组的依赖列表并补齐前置模组；若已安装，换成与当前 MC 版本匹配的构建。"
            .to_string(),
    })
}
