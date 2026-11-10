//! JVM 自身崩溃（hs_err_pid 里有崩溃帧）

use regex::Regex;
use std::sync::LazyLock;

use super::*;

/// JVM 自己崩了时的帧，V=JVM 本体，J=JIT，C=原生库
static RE_HS_FRAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"#\s+([A-Za-z])\s+\[([\w.-]+)").expect("帧表达式合法"));

static RE_HS_INTERNAL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"#\s*Internal Error \(([^)]*)\)").expect("内部错误表达式合法"));

pub(super) fn rule_jvm_crash(hs_err: &str) -> Option<Finding> {
    let hs_lines: Vec<&str> = hs_err.lines().collect();
    let frame_idx = hs_lines.iter().position(|l| RE_HS_FRAME.is_match(l))?;
    let frame = RE_HS_FRAME.captures(hs_lines[frame_idx])?;

    let internal = hs_lines.iter().enumerate().find_map(|(i, l)| {
        RE_HS_INTERNAL
            .captures(l)
            .map(|c| (i + 1, c[1].to_string()))
    });

    let mut evidence = vec![Evidence {
        line_no: frame_idx + 1,
        text: hs_lines[frame_idx].trim().to_string(),
    }];
    if let Some((idx, text)) = internal {
        evidence.push(Evidence {
            line_no: idx,
            text: format!("# Internal Error ({})", text),
        });
    }

    Some(Finding {
        kind: FindingKind::JvmCrash,
        role: FindingRole::Root,
        summary: "JVM 进程自身崩溃".to_string(),
        conclusion: format!(
            "JVM 进程自身崩溃，出问题的帧在 {}（{} 段）",
            &frame[2], &frame[1]
        ),
        confidence: Confidence::High,
        culprit_label: "问题文件",
        culprits: vec![frame[2].to_string()],
        evidence,
        suggestion: "原生层崩溃与模组无关：更新显卡驱动后重试；hs_err_pid 全文需要留给 JDK 或驱动的上游排查。".to_string(),
    })
}
