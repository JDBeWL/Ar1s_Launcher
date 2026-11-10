//! 诊断结果的数据模型
//!
//! 这些类型会经 Tauri 序列化给前端；规则内部的中间结构留在 `rules` 里

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Confidence {
    /// 日志里有直接点名的原文
    High,
    /// 特征命中但因果需要推断
    Medium,
}

impl Confidence {
    pub fn label(self) -> &'static str {
        match self {
            Confidence::High => "直接证据",
            Confidence::Medium => "特征推断",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum FindingKind {
    Mixin,
    /// 加载器读类元数据的工具链跟不上本次 Java 的 class 版本
    ClassReaderTooOld,
    JavaVersion,
    ClassVersion,
    /// JVM 连主类都没找到（参数被拆开、classpath 缺件）
    MainClass,
    MissingClass,
    Memory,
    Natives,
    JvmCrash,
    SecurityManager,
    /// 游戏自己写下的 crash-reports 摘要
    GameCrashReport,
    Unknown,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Evidence {
    pub line_no: usize,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum FindingRole {
    /// 直接导致这次起不来的那条
    Root,
    /// 被根因带出来的下游报错
    Symptom,
}

impl FindingRole {
    pub fn label(self) -> &'static str {
        match self {
            FindingRole::Root => "根因",
            FindingRole::Symptom => "表象",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    pub kind: FindingKind,
    pub role: FindingRole,
    /// 一句话摘要：弹窗顶部和报告的「结论」行只用它，卡片里才放完整结论
    pub summary: String,
    pub conclusion: String,
    pub confidence: Confidence,
    /// 「问题模组」/「问题类」这类人话标签，由每条规则自己给
    pub culprit_label: &'static str,
    /// 能定位到 mods 目录里的哪个 jar / 哪个类
    pub culprits: Vec<String>,
    pub evidence: Vec<Evidence>,
    pub suggestion: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextLine {
    pub label: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SuggestedJava {
    pub path: String,
    pub version: u32,
    /// true 表示低于判据下限；只在 Java 太新导致的崩溃里给，仅作尝试
    pub below_requirement: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CrashDiagnosis {
    pub exit_code: Option<i32>,
    pub headline: String,
    pub recognized: bool,
    pub findings: Vec<Finding>,
    pub context: Vec<ContextLine>,
    /// 命中行附近的原文窗口，带行号
    pub excerpt: Vec<Evidence>,
    pub raw_output: String,
    /// 复制按钮用的纯文本，与弹窗同源
    pub report_text: String,
    /// 实例 id：弹窗里的“换 Java 重试”要拿它直接重启
    pub instance: String,
    /// 判据允许、且不是本次用过的那份 Java；有它才给得出重试按钮
    pub suggested_java: Option<SuggestedJava>,
}

/// 本次运行由游戏写下的 crash-reports 原文
pub struct CrashReportEvidence<'a> {
    pub path: &'a str,
    pub text: &'a str,
}

#[derive(Debug, Clone, Default)]
pub struct LaunchContext {
    pub version_id: String,
    pub loader: String,
    pub declared_java: Option<u32>,
    pub used_java_path: String,
    pub used_java_version: Option<u32>,
    pub memory_mb: u32,
    pub mod_files: Vec<String>,
    /// 进程起来到退出之间跑了多久，用来分清「秒崩」和「玩几分钟才崩」
    pub runtime_secs: Option<u64>,
}
