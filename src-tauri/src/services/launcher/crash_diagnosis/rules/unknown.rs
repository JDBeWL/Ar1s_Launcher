//! 兜底：没有任何特征命中时，只如实给出最深的异常原因

use super::*;

/// 没有任何特征命中时唯一能诚实给出的东西：最深的 Caused by
///
/// 表里它排在最后且标了兜底，跑到这里就一定是前面全空，不必再判一次
pub(super) fn rule_unknown(lines: &[String]) -> Option<Finding> {
    let (idx, text) = root_cause(lines, 0)
        .or_else(|| last_exception_line(lines))
        .or_else(|| {
            let last = lines.iter().rposition(|l| !l.trim().is_empty())?;
            Some((last, lines[last].trim().to_string()))
        })?;

    // 只剩栈帧时别把它包装成“原因”，那会让人以为问题出在 modlauncher 这一行
    let conclusion = if text.starts_with("at ") || text.starts_with("...") {
        format!("日志里没有异常类型，最后一行是 {}", text)
    } else {
        format!("最深的异常原因：{}", text)
    };

    Some(Finding {
        kind: FindingKind::Unknown,
        role: FindingRole::Root,
        summary: "未识别出已知的崩溃特征".to_string(),
        conclusion,
        confidence: Confidence::Medium,
        culprit_label: "",
        culprits: Vec::new(),
        evidence: vec![evidence_at(lines, idx)],
        suggestion:
            "这条不在启动器已识别的特征内，请把原文和环境行连同整合包名称反馈给模组或整合包作者。"
                .to_string(),
    })
}
