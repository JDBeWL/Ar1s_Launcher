//! mixin 应用失败：加载器给某个模组的注入点找不到目标类/方法

use regex::Regex;
use std::sync::LazyLock;

use super::*;

/// `Mixin apply for mod <id> failed <config>:<mixinClass> from mod <id> -> <targetClass>:`
static RE_MIXIN_HEAD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"Mixin apply for mod (\S+) failed (\S+?):(\S+) from mod \S+ -> ([\w.$]+):")
        .expect("mixin 特征表达式合法")
});

pub(super) fn rule_mixin(lines: &[String], ctx: &LaunchContext) -> Option<Finding> {
    let (idx, (mod_id, config, target)) = find_signal(lines, |line| {
        let cap = RE_MIXIN_HEAD.captures(line)?;
        Some((cap[1].to_string(), cap[2].to_string(), cap[4].to_string()))
    })?;

    let mut evidence = vec![evidence_at(lines, idx)];
    let root = match root_cause(lines, idx) {
        Some((root_idx, text)) => {
            if root_idx != idx {
                evidence.push(evidence_at(lines, root_idx));
            }
            text
        }
        None => "未追到 Caused by 链".to_string(),
    };

    Some(Finding {
        kind: FindingKind::Mixin,
        role: FindingRole::Root,
        summary: format!("{} 的 mixin 应用失败", mod_id),
        conclusion: format!(
            "{} 的 mixin（{}）在应用到 {} 时失败：{}",
            mod_id, config, target, root
        ),
        confidence: Confidence::High,
        culprit_label: "问题模组",
        culprits: jars_for_mod(&mod_id, ctx),
        evidence,
        suggestion: "将该模组单独移出 mods 目录启动一次，确认它是否与当前环境不兼容。".to_string(),
    })
}
