//! 游戏自己写下的 crash-reports
//!
//! stdout 里没有可用特征时，报告里的 Description / 异常行 / 直接原因更可靠

use regex::Regex;
use std::sync::LazyLock;

use super::*;

/// 崩溃报告全文在磁盘上，弹窗里只带开头这些行
pub(crate) const GAME_CRASH_REPORT_MAX_LINES: usize = 160;

/// 报告里的异常行必须是全限定类名，别把模组列表或说明文字当异常
static RE_REPORT_EXCEPTION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:[A-Za-z_$][\w$]*\.)+[A-Za-z_$][\w$]*(?:Exception|Error)(?::|$)")
        .expect("崩溃报告异常表达式合法")
});

pub(crate) struct ParsedCrashReport {
    description: Option<(usize, String)>,
    exception: Option<(usize, String)>,
    cause: Option<(usize, String)>,
}

impl ParsedCrashReport {
    pub(crate) fn description_text(&self) -> Option<&str> {
        self.description
            .as_ref()
            .and_then(|(_, line)| line.strip_prefix("Description:"))
            .map(str::trim)
    }

    fn exception_text(&self) -> Option<&str> {
        self.exception.as_ref().map(|(_, line)| line.as_str())
    }

    fn cause_text(&self) -> Option<&str> {
        self.cause
            .as_ref()
            .and_then(|(_, line)| line.strip_prefix("Caused by:"))
            .map(str::trim)
    }
}

/// 行号保留报告原文的 1-based 行号，evidence 里能直接对上文件
pub(crate) fn parse_crash_report(text: &str) -> ParsedCrashReport {
    let report_lines: Vec<&str> = text.lines().collect();

    let description = report_lines.iter().enumerate().find_map(|(i, line)| {
        line.trim()
            .starts_with("Description:")
            .then(|| (i + 1, line.trim().to_string()))
    });

    let exception = report_lines.iter().enumerate().find_map(|(i, line)| {
        let trimmed = line.trim();
        RE_REPORT_EXCEPTION
            .is_match(trimmed)
            .then(|| (i + 1, trimmed.to_string()))
    });

    // 只看异常行后面一小段，别把线程转储里无关的 Caused by 当直接原因
    let cause = exception.as_ref().and_then(|(line_no, _)| {
        report_lines
            .iter()
            .enumerate()
            .skip(*line_no)
            .take(100)
            .find_map(|(i, line)| {
                line.trim()
                    .starts_with("Caused by:")
                    .then(|| (i + 1, line.trim().to_string()))
            })
    });

    ParsedCrashReport {
        description,
        exception,
        cause,
    }
}

pub(super) fn rule_game_crash_report(report: &ParsedCrashReport) -> Option<Finding> {
    let description = report.description_text();
    let exception = report.exception_text();
    if description.is_none() && exception.is_none() {
        return None;
    }
    let cause = report.cause_text();

    let summary = description
        .map(|value| format!("游戏崩溃于「{}」", value))
        .unwrap_or_else(|| "游戏崩溃报告".to_string());

    let mut conclusion = String::new();
    if let Some(value) = description {
        conclusion.push_str(&format!("游戏在「{}」阶段崩溃", value));
    }
    if let Some(value) = exception {
        if !conclusion.is_empty() {
            conclusion.push('：');
        }
        conclusion.push_str(value);
    }
    if let Some(value) = cause {
        conclusion.push_str(&format!("；直接原因：{}", value));
    }

    let evidence: Vec<Evidence> = [&report.description, &report.exception, &report.cause]
        .into_iter()
        .flatten()
        .map(|(line_no, text)| Evidence {
            line_no: *line_no,
            text: text.clone(),
        })
        .collect();

    Some(Finding {
        kind: FindingKind::GameCrashReport,
        role: FindingRole::Root,
        summary,
        conclusion,
        confidence: Confidence::High,
        culprit_label: "崩溃报告",
        culprits: Vec::new(),
        evidence,
        suggestion: "先按报告里的直接原因排查对应模组或驱动；完整报告已随诊断附带。".to_string(),
    })
}
