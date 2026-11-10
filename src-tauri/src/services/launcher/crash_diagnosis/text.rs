//! 日志切分与展示文本
//!
//! 行级定位（找特征行、找 Caused by）、弹出窗口、纯文本报告都在这里，
//! 规则只负责「认特征」，不关心怎么展示

use super::model::{ContextLine, Evidence, Finding, FindingKind, LaunchContext};

pub(crate) fn root_cause(lines: &[String], from: usize) -> Option<(usize, String)> {
    let idx = (from..lines.len())
        .rev()
        .find(|i| lines[*i].trim_start().starts_with("Caused by:"))?;
    Some((
        idx,
        lines[idx]
            .trim_start()
            .trim_start_matches("Caused by: ")
            .to_string(),
    ))
}

pub(crate) fn jars_for_mod(mod_id: &str, ctx: &LaunchContext) -> Vec<String> {
    let needle = mod_id.to_lowercase().replace('_', "-");
    let hits: Vec<String> = ctx
        .mod_files
        .iter()
        .filter(|jar| {
            jar.to_lowercase()
                .replace(['_', '.'], "-")
                .contains(&needle)
        })
        .cloned()
        .collect();

    // 整合包裁过 mods 目录时按 id 点名
    if hits.is_empty() {
        vec![mod_id.to_string()]
    } else {
        hits
    }
}

pub(crate) fn evidence_at(lines: &[String], idx: usize) -> Evidence {
    Evidence {
        line_no: idx + 1,
        text: lines[idx].clone(),
    }
}

/// 展示窗口：命中行之前留几行、整段最多留几行
const EXCERPT_BEFORE: usize = 8;
const EXCERPT_MAX_LINES: usize = 60;

/// 从第一行命中特征的日志抽出 (行号, 抽取结果)
pub(crate) fn find_signal<T>(
    lines: &[String],
    mut extract: impl FnMut(&str) -> Option<T>,
) -> Option<(usize, T)> {
    lines
        .iter()
        .enumerate()
        .find_map(|(i, line)| extract(line).map(|value| (i, value)))
}

/// 日志尾巴常常是一串栈帧，把最后一行当成“根因”只会得到 `at cpw.mods.modlauncher...`
pub(crate) fn last_exception_line(lines: &[String]) -> Option<(usize, String)> {
    lines.iter().enumerate().rev().find_map(|(idx, line)| {
        let text = line.trim();
        let looks_like_exception =
            (text.contains("Exception") || text.contains("Error")) && !text.starts_with("at ");
        looks_like_exception.then(|| (idx, text.to_string()))
    })
}

/// 报告里的原文只留尾部这么多行，再早的内容由 excerpt 窗口负责给出
pub(crate) const RAW_OUTPUT_MAX_LINES: usize = 2000;

pub(crate) fn raw_window(lines: &[String]) -> String {
    if lines.len() <= RAW_OUTPUT_MAX_LINES {
        return lines.join("\n");
    }
    let dropped = lines.len() - RAW_OUTPUT_MAX_LINES;
    let mut out = format!("… 前面 {} 行未收录\n", dropped);
    out.push_str(&lines[dropped..].join("\n"));
    out
}

pub(crate) fn context_lines(ctx: &LaunchContext) -> Vec<ContextLine> {
    let used = ctx
        .used_java_version
        .map(|v| v.to_string())
        .unwrap_or_else(|| "未识别".to_string());
    let runtime = ctx
        .runtime_secs
        .map(|secs| format!("{} 秒", secs))
        .unwrap_or_else(|| "未记录".to_string());

    [
        ("实例", ctx.version_id.clone()),
        ("加载器", ctx.loader.clone()),
        ("本次 Java", format!("{} · {}", used, ctx.used_java_path)),
        ("堆内存", format!("{} MB", ctx.memory_mb)),
        ("模组数", ctx.mod_files.len().to_string()),
        ("启动到崩溃", runtime),
    ]
    .into_iter()
    .map(|(label, value)| ContextLine {
        label: label.to_string(),
        value,
    })
    .collect()
}

/// 只取命中行附近，100 行的尾巴会把真正的 Caused by 挤掉
pub(crate) fn excerpt_window(lines: &[String], findings: &[Finding]) -> Vec<Evidence> {
    let hit = findings
        .iter()
        .filter(|f| !matches!(f.kind, FindingKind::JvmCrash | FindingKind::GameCrashReport))
        .flat_map(|f| f.evidence.iter().map(|e| e.line_no))
        .min()
        .map(|line_no| line_no.saturating_sub(1).min(lines.len().saturating_sub(1)));

    let start = hit.unwrap_or(0).saturating_sub(EXCERPT_BEFORE);
    let end = (start + EXCERPT_MAX_LINES).min(lines.len());

    lines[start..end]
        .iter()
        .enumerate()
        .map(|(i, text)| Evidence {
            line_no: start + i + 1,
            text: text.clone(),
        })
        .collect()
}

pub(crate) fn plain_text(
    exit_code: Option<i32>,
    headline: &str,
    findings: &[Finding],
    context: &[ContextLine],
    raw_output: &str,
) -> String {
    let mut out = String::new();
    out.push_str(&format!("游戏退出码 {:?}\n结论：{}\n", exit_code, headline));

    for (i, f) in findings.iter().enumerate() {
        out.push_str(&format!(
            "\n诊断 {}（{} · {}）\n  结论：{}\n",
            i + 1,
            f.role.label(),
            f.confidence.label(),
            f.conclusion
        ));
        if !f.culprits.is_empty() {
            out.push_str(&format!(
                "  {}：{}\n",
                f.culprit_label,
                f.culprits.join("、")
            ));
        }
        for e in &f.evidence {
            out.push_str(&format!("  原文第 {} 行：{}\n", e.line_no, e.text));
        }
        out.push_str(&format!("  建议：{}\n", f.suggestion));
    }

    out.push_str("\n环境\n");
    for c in context {
        out.push_str(&format!("  {}：{}\n", c.label, c.value));
    }

    out.push_str("\n完整输出\n");
    out.push_str(raw_output);
    out.push('\n');

    out
}
