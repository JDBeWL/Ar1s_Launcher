//! 崩溃日志特征诊断
//!
//! 判据只来自三处：日志原文特征、实例版本 JSON 的声明、本次实际用于启动的 Java；一条都没命中就如实说未识别。
//!
//! 规则本身在 [`rules`]（一张按优先级排好的表），这里只做三件事：跑表、把下游报错降级成表象、
//! 拼装给人看的上下文与纯文本报告。

mod model;
mod rules;
mod text;

pub use model::*;

use rules::{ParsedCrashReport, RuleInput, collect_findings, rule_java_above_declared};
use text::{context_lines, excerpt_window, plain_text, raw_window};

/// 测试用它校验原文截断的边界
#[cfg(test)]
pub(crate) use text::RAW_OUTPUT_MAX_LINES;

/// 表象的统一文案：不再让人去动其实没坏的模组
fn as_symptom(mut finding: Finding) -> Finding {
    if matches!(
        finding.kind,
        FindingKind::Mixin | FindingKind::MissingClass | FindingKind::ClassVersion
    ) {
        finding.role = FindingRole::Symptom;
        finding.suggestion = "该报错由上面的根因引起，处理根因后会一并消失。".to_string();
    }
    finding
}

pub fn diagnose(
    exit_code: Option<i32>,
    lines: &[String],
    ctx: &LaunchContext,
    hs_err: Option<&str>,
) -> CrashDiagnosis {
    diagnose_with_crash_report(exit_code, lines, ctx, hs_err, None)
}

pub fn diagnose_with_crash_report(
    exit_code: Option<i32>,
    lines: &[String],
    ctx: &LaunchContext,
    hs_err: Option<&str>,
    crash_report: Option<CrashReportEvidence<'_>>,
) -> CrashDiagnosis {
    let parsed_report = crash_report
        .as_ref()
        .map(|report| rules::parse_crash_report(report.text));

    let outcome = collect_findings(&RuleInput {
        lines,
        ctx,
        hs_err,
        crash_report: parsed_report.as_ref(),
    });
    let mut findings = outcome.findings;
    let recognized = outcome.recognized;

    // 工具链读不了本次 JDK 的 class 时，mixin 和缺类都只是下游表象；
    // 根因已经点名 Java，就不再补一条「Java 高于声明」的重复推断
    let toolchain_root = findings
        .iter()
        .any(|f| f.kind == FindingKind::ClassReaderTooOld);
    // 已经点名过 Java（模块访问被拒）就不再补一条更弱的同义推断
    let java_already_named = findings.iter().any(|f| f.kind == FindingKind::JavaVersion);
    if toolchain_root {
        findings = findings.into_iter().map(as_symptom).collect();
    } else if !java_already_named && let Some(f) = rule_java_above_declared(ctx) {
        findings.push(f);
    }

    let headline = if recognized {
        findings
            .iter()
            .find(|f| f.role == FindingRole::Root)
            .or_else(|| findings.first())
            .map(|f| f.summary.clone())
            .unwrap_or_default()
    } else {
        "未识别出已知的崩溃特征".to_string()
    };

    let mut context = context_lines(ctx);
    if let Some(report) = &crash_report {
        if let Some(description) = parsed_report
            .as_ref()
            .and_then(ParsedCrashReport::description_text)
        {
            context.push(ContextLine {
                label: "崩溃阶段".to_string(),
                value: description.to_string(),
            });
        }
        context.push(ContextLine {
            label: "崩溃报告".to_string(),
            value: report.path.to_string(),
        });
    }
    let excerpt = excerpt_window(lines, &findings);

    let mut raw_output = raw_window(lines);
    if let Some(hs) = hs_err {
        raw_output.push_str("\n\n--- JVM 崩溃日志 (hs_err_pid) ---\n");
        raw_output.push_str(hs);
    }
    if let Some(report) = &crash_report {
        let total = report.text.lines().count();
        let mut body = report
            .text
            .lines()
            .take(rules::GAME_CRASH_REPORT_MAX_LINES)
            .collect::<Vec<_>>()
            .join("\n");
        if total > rules::GAME_CRASH_REPORT_MAX_LINES {
            body.push_str(&format!(
                "\n… 其余 {} 行见原文件",
                total - rules::GAME_CRASH_REPORT_MAX_LINES
            ));
        }
        raw_output.push_str(&format!("\n\n--- 游戏崩溃报告 ({}) ---\n", report.path));
        raw_output.push_str(&body);
    }

    let report_text = plain_text(exit_code, &headline, &findings, &context, &raw_output);

    CrashDiagnosis {
        exit_code,
        headline,
        recognized,
        findings,
        context,
        excerpt,
        raw_output,
        report_text,
        instance: ctx.version_id.clone(),
        suggested_java: None,
    }
}

#[cfg(test)]
mod tests;
