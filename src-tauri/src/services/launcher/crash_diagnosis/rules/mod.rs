//! 崩溃特征规则表
//!
//! 每条规则是表里的一行：需要什么输入、是不是兜底、命中时产出什么。新增一类崩溃特征只要
//! 加一条表项和一个 `fn(&RuleInput) -> Option<Finding>`，不必再改 `diagnose` 的主流程。
//!
//! **表的顺序就是优先级**：`findings` 的第一条会出现在弹窗标题上，因此根因类判据排在前面，
//! 表象类（mixin 失败、缺类）排在其后，由调用方统一降级成「表象」。

pub(crate) use super::model::{
    Confidence, Evidence, Finding, FindingKind, FindingRole, LaunchContext,
};
pub(crate) use super::text::{
    evidence_at, find_signal, jars_for_mod, last_exception_line, root_cause,
};

pub(crate) use game_report::{GAME_CRASH_REPORT_MAX_LINES, ParsedCrashReport, parse_crash_report};
pub(crate) use java::rule_java_above_declared;

mod classpath;
mod game_report;
mod java;
mod jvm;
mod mixin;
mod runtime;
mod unknown;

/// 规则能看到的全部输入
pub(crate) struct RuleInput<'a> {
    pub lines: &'a [String],
    pub ctx: &'a LaunchContext,
    /// JVM 自己写下的 hs_err_pid 原文
    pub hs_err: Option<&'a str>,
    /// 游戏自己写下的 crash-reports，已解析
    pub crash_report: Option<&'a ParsedCrashReport>,
}

/// 规则依赖哪份输入；缺输入直接跳过，省得每条规则自己判一遍
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Needs {
    Log,
    HsErr,
    GameReport,
}

impl Needs {
    fn available(self, input: &RuleInput<'_>) -> bool {
        match self {
            Needs::Log => true,
            Needs::HsErr => input.hs_err.is_some(),
            Needs::GameReport => input.crash_report.is_some(),
        }
    }
}

pub(crate) struct Rule {
    /// 稳定标识，排查「这条为什么没命中」时用得上
    pub name: &'static str,
    pub needs: Needs,
    /// 兜底规则：前面的规则一条都没命中时才跑
    pub fallback: bool,
    /// 命中后算不算「识别出了已知特征」。兜底规则里，游戏自己的崩溃报告算，unknown 只是如实转述
    pub confirms: bool,
    pub run: fn(&RuleInput<'_>) -> Option<Finding>,
}

static RULES: &[Rule] = &[
    // JVM 自身崩溃的信息量最大，先跑
    Rule {
        name: "jvm_crash",
        needs: Needs::HsErr,
        fallback: false,
        confirms: true,
        run: |input| input.hs_err.and_then(jvm::rule_jvm_crash),
    },
    // 加载器自带的 ASM 读不了本次 JDK 时，下面的 mixin / 缺类都只是下游表象
    Rule {
        name: "class_reader_too_old",
        needs: Needs::Log,
        fallback: false,
        confirms: true,
        run: |input| java::rule_class_reader_too_old(input.lines, input.ctx),
    },
    Rule {
        name: "mixin",
        needs: Needs::Log,
        fallback: false,
        confirms: true,
        run: |input| mixin::rule_mixin(input.lines, input.ctx),
    },
    Rule {
        name: "class_version",
        needs: Needs::Log,
        fallback: false,
        confirms: true,
        run: |input| java::rule_class_version(input.lines, input.ctx),
    },
    Rule {
        name: "main_class",
        needs: Needs::Log,
        fallback: false,
        confirms: true,
        run: |input| classpath::rule_main_class_missing(input.lines),
    },
    Rule {
        name: "missing_class",
        needs: Needs::Log,
        fallback: false,
        confirms: true,
        run: |input| classpath::rule_missing_class(input.lines),
    },
    Rule {
        name: "memory",
        needs: Needs::Log,
        fallback: false,
        confirms: true,
        run: |input| runtime::rule_memory(input.lines, input.ctx),
    },
    Rule {
        name: "natives",
        needs: Needs::Log,
        fallback: false,
        confirms: true,
        run: |input| runtime::rule_natives(input.lines),
    },
    Rule {
        name: "security_manager",
        needs: Needs::Log,
        fallback: false,
        confirms: true,
        run: |input| runtime::rule_security_manager(input.lines, input.ctx),
    },
    Rule {
        name: "module_access",
        needs: Needs::Log,
        fallback: false,
        confirms: true,
        run: |input| runtime::rule_module_access(input.lines, input.ctx),
    },
    // stdout 没特征时，游戏自己写下的报告比「最后一行异常」可靠
    Rule {
        name: "game_crash_report",
        needs: Needs::GameReport,
        fallback: true,
        confirms: true,
        run: |input| {
            input
                .crash_report
                .and_then(game_report::rule_game_crash_report)
        },
    },
    Rule {
        name: "unknown",
        needs: Needs::Log,
        fallback: true,
        confirms: false,
        run: |input| unknown::rule_unknown(input.lines),
    },
];

/// 跑一遍规则表的结果
pub(crate) struct RuleOutcome {
    pub findings: Vec<Finding>,
    /// 是否有实打实的判据命中；只有兜底规则命中时仍算「未识别」
    pub recognized: bool,
}

/// 按表顺序跑，兜底规则只在前面全空时补位
pub(crate) fn collect_findings(input: &RuleInput<'_>) -> RuleOutcome {
    let mut outcome = RuleOutcome {
        findings: Vec::new(),
        recognized: false,
    };

    for rule in RULES {
        if rule.fallback && !outcome.findings.is_empty() {
            continue;
        }
        if !rule.needs.available(input) {
            continue;
        }
        if let Some(finding) = (rule.run)(input) {
            // 崩溃诊断只在出事时跑，命中日志留着，排查「为什么给出这个结论」时直接看
            log::debug!("崩溃诊断命中规则 {}", rule.name);
            if rule.confirms {
                outcome.recognized = true;
            }
            outcome.findings.push(finding);
        }
    }

    outcome
}

/// 表里每条规则的标识，测试用它保证表项和实现没走散
#[cfg(test)]
pub(crate) fn rule_names() -> Vec<&'static str> {
    RULES.iter().map(|rule| rule.name).collect()
}
