//! 崩溃日志特征诊断
//!
//! 判据只来自三处：日志原文特征、实例版本 JSON 的声明、本次实际用于启动的 Java；一条都没命中就如实说未识别

use regex::Regex;
use serde::Serialize;
use std::sync::LazyLock;

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

/// `Mixin apply for mod <id> failed <config>:<mixinClass> from mod <id> -> <targetClass>:`
static RE_MIXIN_HEAD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"Mixin apply for mod (\S+) failed (\S+?):(\S+) from mod \S+ -> ([\w.$]+):")
        .expect("mixin 特征表达式合法")
});

/// class 版本号只从日志原文里取，不查表
static RE_CLASS_VERSION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"UnsupportedClassVersionError: (\S+) has been compiled by a more recent version of the Java Runtime \(class file version (\d+)",
    )
    .expect("class 版本表达式合法")
});

/// JVM 自己的报错会跟着系统语言走，中英文都要认
static RE_MAIN_CLASS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:Could not find or load main class|找不到或无法加载主类)\s*:?\s*([\w.$=/-]+)")
        .expect("主类缺失表达式合法")
});

static RE_MISSING_CLASS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:NoClassDefFoundError|ClassNotFoundException): ([\w./$]+)")
        .expect("缺类表达式合法")
});

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

/// 最深层的 `Caused by:`，也就是日志真正想说的原因
fn root_cause(lines: &[String], from: usize) -> Option<(usize, String)> {
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

fn jars_for_mod(mod_id: &str, ctx: &LaunchContext) -> Vec<String> {
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

fn evidence_at(lines: &[String], idx: usize) -> Evidence {
    Evidence {
        line_no: idx + 1,
        text: lines[idx].clone(),
    }
}

fn rule_mixin(lines: &[String], ctx: &LaunchContext) -> Option<Finding> {
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

/// 展示窗口：命中行之前留几行、整段最多留几行
const EXCERPT_BEFORE: usize = 8;
const EXCERPT_MAX_LINES: usize = 60;

/// mixin 内置 ASM 读不了新 JDK 的 class 时，日志会直接报出它不支持的 major 版本
static RE_CLASS_READ_TOO_OLD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"Unsupported class file major version (\d+)").expect("class 读取表达式合法")
});

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

fn rule_class_reader_too_old(lines: &[String], ctx: &LaunchContext) -> Option<Finding> {
    let hits: Vec<(usize, u32)> = lines
        .iter()
        .enumerate()
        .filter_map(|(i, line)| {
            RE_CLASS_READ_TOO_OLD
                .captures(line)
                .and_then(|c| Some((i, c[1].parse().ok()?)))
        })
        .collect();

    let (idx, major) = *hits.first()?;
    let java = super::java_requirement::class_file_to_java(major)?;
    let matches_runtime = ctx.used_java_version == Some(java);

    // 本次 Java 正好就是编译该类的版本，矛盾不在“Java 太旧”，而在读类的那套工具（加载器自带的 ASM）
    let upgrade_loader = format!(
        "本次已经是 Java {}，不是 Java 选错，而是 {} 自带的 ASM 太旧、读不了新版本游戏的类。把 {} 升级到最新版再启动。",
        java, ctx.loader, ctx.loader
    );

    let suggestion = if !matches_runtime {
        // JVM 自己都认不了这个 class：确实是 Java 太旧
        match ctx.declared_java {
            Some(declared) => format!(
                "把启动该实例的 Java 换成版本 JSON 声明的 Java {}。",
                declared
            ),
            None => "换成该加载器支持的更老 Java 版本启动。".to_string(),
        }
    } else {
        match ctx.declared_java {
            Some(declared) if declared == java => upgrade_loader,
            // 实例本来就声明了更老的 Java，退回声明版本同样能解决，优先给代价小的那条
            Some(declared) => format!(
                "把启动该实例的 Java 退回版本 JSON 声明的 Java {}；确实要用 Java {} 的话，得先把 {} 升级到支持它的版本。",
                declared, java, ctx.loader
            ),
            None => upgrade_loader,
        }
    };

    Some(Finding {
        kind: FindingKind::ClassReaderTooOld,
        role: FindingRole::Root,
        summary: format!("加载器无法读取 Java {} 的类文件", java),
        conclusion: format!(
            "加载器读取类元数据时遇到 major {}（Java {}）编译的类文件{}；同类报错 {} 次，最早出现在第 {} 行",
            major,
            java,
            if matches_runtime {
                format!("，本次用的正是 Java {}", java)
            } else {
                String::new()
            },
            hits.len(),
            idx + 1
        ),
        confidence: if matches_runtime {
            Confidence::High
        } else {
            Confidence::Medium
        },
        culprit_label: "本次用的 Java",
        culprits: vec![ctx.used_java_path.clone()],
        evidence: vec![evidence_at(lines, idx)],
        suggestion,
    })
}

/// 从第一行命中特征的日志抽出 (行号, 抽取结果)
fn find_signal<T>(
    lines: &[String],
    mut extract: impl FnMut(&str) -> Option<T>,
) -> Option<(usize, T)> {
    lines
        .iter()
        .enumerate()
        .find_map(|(i, line)| extract(line).map(|value| (i, value)))
}

fn rule_class_version(lines: &[String], ctx: &LaunchContext) -> Option<Finding> {
    let (idx, (class, major)) = find_signal(lines, |line| {
        let cap = RE_CLASS_VERSION.captures(line)?;
        Some((cap[1].to_string(), cap[2].parse().ok()?))
    })?;

    let required = super::java_requirement::class_file_to_java(major);
    let suggestion = match required {
        Some(java) => format!(
            "把启动该实例的 Java 换成 {} 或更高（版本 JSON 声明 {}）",
            java,
            ctx.declared_java.unwrap_or(java)
        ),
        None => "把启动该实例的 Java 换成能读该 class 版本的更高版本".to_string(),
    };

    Some(Finding {
        kind: FindingKind::ClassVersion,
        role: FindingRole::Root,
        summary: format!("有类按 class 版本 {} 编译，本次 Java 太旧", major),
        conclusion: format!(
            "{} 按 class 版本 {} 编译，本次 Java {} 无法加载",
            class,
            major,
            ctx.used_java_version
                .map_or("版本未知".to_string(), |v| v.to_string())
        ),
        confidence: Confidence::High,
        culprit_label: "问题类",
        culprits: vec![class],
        evidence: vec![evidence_at(lines, idx)],
        suggestion,
    })
}

fn rule_main_class_missing(lines: &[String]) -> Option<Finding> {
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

fn rule_missing_class(lines: &[String]) -> Option<Finding> {
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

fn rule_memory(lines: &[String], ctx: &LaunchContext) -> Option<Finding> {
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

fn rule_natives(lines: &[String]) -> Option<Finding> {
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

fn rule_security_manager(lines: &[String], ctx: &LaunchContext) -> Option<Finding> {
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
fn rule_module_access(lines: &[String], ctx: &LaunchContext) -> Option<Finding> {
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

/// JVM 自己崩了时的帧，V=JVM 本体，J=JIT，C=原生库
static RE_HS_FRAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"#\s+([A-Za-z])\s+\[([\w.-]+)").expect("帧表达式合法"));

static RE_HS_INTERNAL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"#\s*Internal Error \(([^)]*)\)").expect("内部错误表达式合法"));

fn rule_jvm_crash(hs_err: &str) -> Option<Finding> {
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

/// 日志尾巴常常是一串栈帧，把最后一行当成“根因”只会得到 `at cpw.mods.modlauncher...`
fn last_exception_line(lines: &[String]) -> Option<(usize, String)> {
    lines.iter().enumerate().rev().find_map(|(idx, line)| {
        let text = line.trim();
        let looks_like_exception =
            (text.contains("Exception") || text.contains("Error")) && !text.starts_with("at ");
        looks_like_exception.then(|| (idx, text.to_string()))
    })
}

/// 没有任何特征命中时唯一能诚实给出的东西：最深的 Caused by
fn rule_unknown(lines: &[String], hs_err_present: bool) -> Option<Finding> {
    if hs_err_present {
        return None;
    }

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

/// 崩溃报告全文在磁盘上，弹窗里只带开头这些行
const GAME_CRASH_REPORT_MAX_LINES: usize = 160;

/// 报告里的异常行必须是全限定类名，别把模组列表或说明文字当异常
static RE_REPORT_EXCEPTION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:[A-Za-z_$][\w$]*\.)+[A-Za-z_$][\w$]*(?:Exception|Error)(?::|$)")
        .expect("崩溃报告异常表达式合法")
});

struct ParsedCrashReport {
    description: Option<(usize, String)>,
    exception: Option<(usize, String)>,
    cause: Option<(usize, String)>,
}

impl ParsedCrashReport {
    fn description_text(&self) -> Option<&str> {
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
fn parse_crash_report(text: &str) -> ParsedCrashReport {
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

fn rule_game_crash_report(report: &ParsedCrashReport) -> Option<Finding> {
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

/// 报告里的原文只留尾部这么多行，再早的内容由 excerpt 窗口负责给出
const RAW_OUTPUT_MAX_LINES: usize = 2000;

fn raw_window(lines: &[String]) -> String {
    if lines.len() <= RAW_OUTPUT_MAX_LINES {
        return lines.join("\n");
    }
    let dropped = lines.len() - RAW_OUTPUT_MAX_LINES;
    let mut out = format!("… 前面 {} 行未收录\n", dropped);
    out.push_str(&lines[dropped..].join("\n"));
    out
}

fn context_lines(ctx: &LaunchContext) -> Vec<ContextLine> {
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
fn excerpt_window(lines: &[String], findings: &[Finding]) -> Vec<Evidence> {
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

fn plain_text(
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

/// 本次运行时高于实例声明的运行时，是「起不来」里最常被误认成模组坏了的一条
fn rule_java_above_declared(ctx: &LaunchContext) -> Option<Finding> {
    let used = ctx.used_java_version?;
    let declared = ctx.declared_java?;
    if used <= declared {
        return None;
    }

    Some(Finding {
        kind: FindingKind::JavaVersion,
        role: FindingRole::Root,
        summary: format!("本次 Java {} 高于实例声明的 {}", used, declared),
        conclusion: format!(
            "本次使用 Java {} 启动，实例声明为 Java {}；较新的 JDK 可能超出该加载器的兼容范围",
            used, declared
        ),
        confidence: Confidence::Medium,
        culprit_label: "",
        culprits: Vec::new(),
        evidence: Vec::new(),
        suggestion: format!(
            "先用 Java {} 启动一次做对照：能启动说明问题在本次 JDK，仍然失败则排除 Java 因素。",
            declared
        ),
    })
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
        .map(|report| parse_crash_report(report.text));
    let mut findings: Vec<Finding> = Vec::new();
    let mut causes = 0usize;

    if let Some(f) = hs_err.and_then(rule_jvm_crash) {
        causes += 1;
        findings.push(f);
    }
    if let Some(f) = rule_class_reader_too_old(lines, ctx) {
        causes += 1;
        findings.push(f);
    }
    findings.extend(
        [
            rule_mixin(lines, ctx),
            rule_class_version(lines, ctx),
            rule_main_class_missing(lines),
            rule_missing_class(lines),
            rule_memory(lines, ctx),
            rule_natives(lines),
            rule_security_manager(lines, ctx),
            rule_module_access(lines, ctx),
        ]
        .into_iter()
        .flatten()
        .inspect(|_| causes += 1),
    );

    let mut recognized = causes > 0;
    if !recognized {
        // stdout 没特征时，游戏自己写下的报告比“最后一行异常”可靠
        if let Some(finding) = parsed_report.as_ref().and_then(rule_game_crash_report) {
            findings.push(finding);
            recognized = true;
        } else {
            findings.extend(rule_unknown(lines, !findings.is_empty()));
        }
    }

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
            .take(GAME_CRASH_REPORT_MAX_LINES)
            .collect::<Vec<_>>()
            .join("\n");
        if total > GAME_CRASH_REPORT_MAX_LINES {
            body.push_str(&format!(
                "\n… 其余 {} 行见原文件",
                total - GAME_CRASH_REPORT_MAX_LINES
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
mod tests {
    use super::*;

    /// 取自本机 2026-10-17 18:40 用 Java 26 启动 Closing Song1.6.6.1 的真实 stderr
    const MIXIN_CRASH: &str = r#"[18:40:23] [main/FATAL] [mixin/]: Mixin apply for mod fancymenu failed fancymenu.forge.mixins.json:client.MixinForgeLoadingOverlay from mod fancymenu -> net.minecraftforge.client.loading.ForgeLoadingOverlay: org.spongepowered.asm.mixin.transformer.throwables.InvalidMixinException Unexpecteded ClassMetadataNotFoundException whilst transforming the mixin class: [MAIN Applicator Phase -> fancymenu.forge.mixins.json:client.MixinForgeLoadingOverlay from mod fancymenu -> Apply Methods -> (Lnet/minecraft/client/gui/screens/Screen;Lnet/minecraft/client/Minecraft;IILcom/llamalad7/mixinextras/injector/wrapoperation/Operation;)V:wrapOperation$ddg001$fancymenu$wrapInitScreenFancyMenu -> Transform Descriptor -> desc=Lnet/minecraft/client/Minecraft;]
org.spongepowered.asm.mixin.transformer.throwables.InvalidMixinException: Unexpecteded ClassMetadataNotFoundException whilst transforming the mixin class: [MAIN Applicator Phase -> fancymenu.forge.mixins.json:client.MixinForgeLoadingOverlay from mod fancymenu -> Transform Descriptor -> desc=Lnet/minecraft/client/Minecraft;]
	at org.spongepowered.asm.mixin.transformer.MixinTargetContext.transformMethod(MixinTargetContext.java:510) ~[mixin-0.8.5.jar:0.8.5+Jenkins-b310]
	at cpw.mods.modlauncher.Launcher.run(Launcher.java:108) ~[modlauncher-10.0.9.jar:?]
Caused by: org.spongepowered.asm.mixin.throwables.ClassMetadataNotFoundException: net.minecraft.client.Minecraft
	at org.spongepowered.asm.mixin.transformer.MixinTargetContext.transformSingleDescriptor(MixinTargetContext.java:1001) ~[mixin-0.8.5.jar:0.8.5+Jenkins-b310]
	... 35 more
Exception in thread "main" java.lang.RuntimeException: org.spongepowered.asm.mixin.transformer.throwables.MixinTransformerError: An unexpected critical error was encountered
	at cpw.mods.modlauncher.LaunchServiceHandlerDecorator.launch(LaunchServiceHandlerDecorator.java:32)
Caused by: org.spongepowered.asm.mixin.transformer.throwables.MixinTransformerError: An unexpected critical error was encountered
	at org.spongepowered.asm.mixin.transformer.MixinProcessor.applyMixins(MixinProcessor.java:392)
Caused by: org.spongepowered.asm.mixin.throwables.MixinApplyError: Mixin [fancymenu.forge.mixins.json:client.MixinForgeLoadingOverlay from mod fancymenu] from phase [DEFAULT] in config [fancymenu.forge.mixins.json] FAILED during APPLY
	at org.spongepowered.asm.mixin.transformer.MixinProcessor.handleMixinError(MixinProcessor.java:638)
Caused by: org.spongepowered.asm.mixin.throwables.ClassMetadataNotFoundException: net.minecraft.client.Minecraft
	at org.spongepowered.asm.mixin.transformer.MixinTargetContext.transformSingleDescriptor(MixinTargetContext.java:1001)"#;

    fn lines(text: &str) -> Vec<String> {
        text.lines().map(|l| l.to_string()).collect()
    }

    fn ctx_java_26() -> LaunchContext {
        LaunchContext {
            version_id: "Closing Song1.6.6.1".to_string(),
            loader: "Forge".to_string(),
            declared_java: Some(17),
            used_java_path: "C:/Program Files/Zulu/zulu-26/bin/java.exe".to_string(),
            used_java_version: Some(26),
            memory_mb: 9600,
            mod_files: vec![
                "Connector-1.0.0-beta.46+1.20.1.jar".to_string(),
                "fancymenu_forge_3.3.2_MC_1.20.1.jar".to_string(),
                "Jade-1.20.1-Forge-11.12.2.jar".to_string(),
            ],
            runtime_secs: None,
        }
    }

    #[test]
    fn test_mixin_failure_names_the_mod_behind_the_crash() {
        let d = diagnose(Some(1), &lines(MIXIN_CRASH), &ctx_java_26(), None);

        assert!(d.recognized);
        let mixin = d
            .findings
            .iter()
            .find(|f| f.kind == FindingKind::Mixin)
            .expect("应当识别出 mixin 失败");
        assert_eq!(mixin.confidence, Confidence::High);
        assert_eq!(mixin.culprits, vec!["fancymenu_forge_3.3.2_MC_1.20.1.jar"]);
        assert_eq!(mixin.culprit_label, "问题模组");
        assert_eq!(mixin.evidence[0].line_no, 1);
        assert!(d.headline.contains("fancymenu"));
        // 配置名是证据的一部分，但不能混进「去更新哪个模组」的建议里
        assert!(mixin.conclusion.contains("fancymenu.forge.mixins.json"));
        assert!(!mixin.suggestion.contains("mixins.json"));
    }

    #[test]
    fn test_mixin_failure_with_newer_java_than_declared_flags_java() {
        let d = diagnose(Some(1), &lines(MIXIN_CRASH), &ctx_java_26(), None);

        let java = d
            .findings
            .iter()
            .find(|f| f.kind == FindingKind::JavaVersion)
            .expect("本次 Java 高于声明时应当把 Java 列为嫌疑");
        assert_eq!(java.confidence, Confidence::Medium);
        assert!(java.conclusion.contains("26"));
        assert!(java.conclusion.contains("17"));
    }

    fn ctx_plain() -> LaunchContext {
        LaunchContext {
            version_id: "test".to_string(),
            loader: "Forge".to_string(),
            declared_java: Some(17),
            used_java_path: "C:/java-17/bin/java.exe".to_string(),
            used_java_version: Some(17),
            memory_mb: 4096,
            mod_files: Vec::new(),
            runtime_secs: None,
        }
    }

    fn only(d: &CrashDiagnosis, kind: FindingKind) -> Finding {
        d.findings
            .iter()
            .find(|f| f.kind == kind)
            .unwrap_or_else(|| panic!("期望有 {:?}，实际 {:?}", kind, d.findings))
            .clone()
    }

    #[test]
    fn test_unsupported_class_version_points_at_the_required_java() {
        let log = "Exception in thread \"main\" java.lang.UnsupportedClassVersionError: dev/kaetus/mymod/Main has been compiled by a more recent version of the Java Runtime (class file version 65.0), this version of the Java Runtime only recognizes class file versions up to 61.0\n\tat java.base/java.lang.ClassLoader.defineClass1(Native Method)";

        let d = diagnose(Some(1), &lines(log), &ctx_plain(), None);

        let f = only(&d, FindingKind::ClassVersion);
        assert_eq!(f.confidence, Confidence::High);
        assert_eq!(f.culprits, vec!["dev/kaetus/mymod/Main"]);
        assert_eq!(f.culprit_label, "问题类");
        assert!(f.conclusion.contains("65"));
        assert!(f.suggestion.contains("21"), "应当把 65 折成所需 Java");
    }

    #[test]
    fn test_missing_class_is_reported_as_absent_dependency() {
        let log = "java.lang.NoClassDefFoundError: me/shedaniel/autoconfig/ui/ConfigListScreen\n\tat net.minecraftforge.client.ForgeHooksClient.openScreen(ForgeHooksClient.java:120)\nCaused by: java.lang.ClassNotFoundException: me.shedaniel.autoconfig.ui.ConfigListScreen\n\tat java.base/java.net.URLClassLoader.findClass(URLClassLoader.java:445)";

        let d = diagnose(Some(1), &lines(log), &ctx_plain(), None);

        let f = only(&d, FindingKind::MissingClass);
        assert_eq!(f.confidence, Confidence::Medium);
        assert_eq!(
            f.culprits,
            vec!["me/shedaniel/autoconfig/ui/ConfigListScreen"]
        );
        assert!(f.suggestion.contains("前置"));
        assert_eq!(f.culprit_label, "问题类");
        // 结论只说事实；被降级成表象时不能还留着「前置模组没装」这种指控
        assert!(!f.conclusion.contains("前置"));
    }

    #[test]
    fn test_out_of_memory_quotes_the_memory_we_configured() {
        let log = "[18:52:01] [Render thread/ERROR]: Unhandled exception\njava.lang.OutOfMemoryError: Java heap space\n\tat net.minecraft.world.chunk.ChunkArray.<init>(ChunkArray.java:22)";

        let d = diagnose(Some(1), &lines(log), &ctx_plain(), None);

        let f = only(&d, FindingKind::Memory);
        assert_eq!(f.confidence, Confidence::High);
        assert!(f.conclusion.contains("4096"), "要说清本次给了多少堆");
    }

    #[test]
    fn test_unsatisfied_link_error_points_at_natives() {
        let log = "Caused by: java.lang.UnsatisfiedLinkError: Could not load native library: lwjgl.dll, OS: Windows 11\n\tat org.lwjgl.system.libc.Libc.<clinit>(Libc.java:40)";

        let d = diagnose(Some(1), &lines(log), &ctx_plain(), None);

        let f = only(&d, FindingKind::Natives);
        assert!(f.culprits.iter().any(|c| c.contains("lwjgl.dll")));
        assert_eq!(f.culprit_label, "问题文件");
        assert!(f.suggestion.contains("natives"));
    }

    #[test]
    fn test_security_manager_failure_means_java_is_too_new() {
        let log = "java.lang.UnsupportedOperationException: The Security Manager is deprecated and will be removed\n\tat java.base/java.lang.System.setSecurityManager(System.java:403)\n\tat net.minecraftforge.fml.loading.FMLLoader.setupSecurity(FMLLoader.java:96)";

        let d = diagnose(Some(1), &lines(log), &ctx_plain(), None);

        let f = only(&d, FindingKind::SecurityManager);
        assert_eq!(f.confidence, Confidence::High);
        assert!(f.evidence[0].text.contains("Security Manager"));
        assert_eq!(f.culprit_label, "本次用的 Java");
    }

    /// 取自 19:42 那次用 Java 26 启动同一整合包的 stderr 原文
    const ASM_CRASH: &str = r#"[19:42:16] [main/WARN] [mixin/]: Error loading class: net/minecraft/server/level/ServerLevel (java.lang.IllegalArgumentException: Unsupported class file major version 70)
[19:42:17] [main/WARN] [mixin/]: Error loading class: com/simibubi/create/content/fluids/spout/FillingBySpout (java.lang.ClassNotFoundException: com.simibubi.create.content.fluids.spout.FillingBySpout)
[19:42:17] [main/WARN] [mixin/]: Error loading class: net/minecraft/client/Minecraft (java.lang.IllegalArgumentException: Unsupported class file major version 70)
[19:42:19] [main/FATAL] [mixin/]: Mixin apply for mod fancymenu failed fancymenu.forge.mixins.json:client.MixinForgeLoadingOverlay from mod fancymenu -> net.minecraftforge.client.loading.ForgeLoadingOverlay: org.spongepowered.asm.mixin.transformer.throwables.InvalidMixinException Unexpecteded ClassMetadataNotFoundException whilst transforming the mixin class
Caused by: org.spongepowered.asm.mixin.throwables.ClassMetadataNotFoundException: net.minecraft.client.Minecraft"#;

    #[test]
    fn test_asm_cannot_read_this_jdk_becomes_the_root_cause() {
        let d = diagnose(Some(1), &lines(ASM_CRASH), &ctx_java_26(), None);

        let f = only(&d, FindingKind::ClassReaderTooOld);
        assert_eq!(f.role, FindingRole::Root);
        assert_eq!(f.confidence, Confidence::High);
        assert!(f.conclusion.contains("major 70"));
        assert!(f.conclusion.contains("Java 26"));
        assert!(f.conclusion.contains("2 次"), "该说清这类报错有多少");
        assert_eq!(f.culprit_label, "本次用的 Java");
        assert!(f.suggestion.contains("17"));
    }

    #[test]
    fn test_downstream_mixin_becomes_symptom() {
        let d = diagnose(Some(1), &lines(ASM_CRASH), &ctx_java_26(), None);

        assert_eq!(
            d.findings.first().unwrap().kind,
            FindingKind::ClassReaderTooOld
        );
        let mixin = only(&d, FindingKind::Mixin);
        assert_eq!(mixin.role, FindingRole::Symptom);
        assert!(mixin.suggestion.contains("根因"));
        assert!(!mixin.suggestion.contains("移出 mods"));
        assert!(
            !d.findings
                .iter()
                .any(|f| f.kind == FindingKind::MissingClass),
            "mixin 探测可选目标的 WARN 不能当成缺类证据"
        );
        assert!(
            !d.findings
                .iter()
                .any(|f| f.kind == FindingKind::JavaVersion),
            "根因已经点明 Java，不该再补一条同样的推断"
        );
    }

    /// 取自本机 2026-10-18 22:34 正常启动的 Vanilla Perfected 26.3 日志：
    /// mixin 探测 Sodium 的几个可选目标失败，游戏照常跑完
    const MIXIN_PROBE_WARNS: &str = r#"[22:34:41] [main/WARN]: Error loading class: me/jellysquid/mods/sodium/client/render/chunk/DefaultChunkRenderer (java.lang.ClassNotFoundException: me/jellysquid/mods/sodium/client/render/chunk/DefaultChunkRenderer)
[22:34:42] [main/WARN]: Error loading class: me/jellysquid/mods/sodium/client/render/chunk/RenderSectionManager (java.lang.ClassNotFoundException: me/jellysquid/mods/sodium/client/render/chunk/RenderSectionManager)
[22:34:42] [main/WARN]: Error loading class: me/jellysquid/mods/sodium/client/render/SodiumWorldRenderer (java.lang.ClassNotFoundException: me/jellysquid/mods/sodium/client/render/SodiumWorldRenderer)"#;

    #[test]
    fn test_mixin_probe_warns_are_not_a_missing_class_root_cause() {
        let d = diagnose(Some(-8), &lines(MIXIN_PROBE_WARNS), &ctx_plain(), None);

        assert!(
            !d.findings
                .iter()
                .any(|f| f.kind == FindingKind::MissingClass),
            "这些 WARN 在正常跑完的游戏里也会出现，不能当缺类根因：{:?}",
            d.findings
        );
    }

    #[test]
    fn test_headline_is_the_short_summary_not_the_full_conclusion() {
        let d = diagnose(Some(1), &lines(ASM_CRASH), &ctx_java_26(), None);

        assert_eq!(d.headline, d.findings[0].summary);
        assert!(!d.headline.contains("net.minecraftforge.client.loading"));
        let mixin = only(&d, FindingKind::Mixin);
        assert!(mixin.summary.contains("fancymenu"));
        assert!(mixin.summary.chars().count() < 40);
    }

    #[test]
    fn test_mixin_alone_is_the_root_cause() {
        let d = diagnose(Some(1), &lines(MIXIN_CRASH), &ctx_java_26(), None);

        let mixin = only(&d, FindingKind::Mixin);
        assert_eq!(mixin.role, FindingRole::Root);
        assert!(mixin.suggestion.contains("移出 mods"));
        assert!(
            !d.headline.contains("ClassMetadataNotFoundException"),
            "顶部不能再抄一遍完整结论"
        );
    }

    #[test]
    fn test_unrecognized_crash_admits_it_and_keeps_the_cause_chain() {
        let log = "[18:52:01] [main/ERROR]: Something went wrong\njava.lang.IllegalStateException: registry blowup\n\tat com.example.Thing.marshal(Thing.java:9)\nCaused by: com.example.NestedException: 缺少必需的配置项 registry_seed\n\tat com.example.Thing.<init>(Thing.java:4)";

        let d = diagnose(Some(1), &lines(log), &ctx_plain(), None);

        assert!(!d.recognized, "没有特征命中时不能装作认出来了");
        assert!(d.headline.contains("未识别"));
        let unknown = only(&d, FindingKind::Unknown);
        assert!(unknown.culprits.is_empty());
        assert!(
            unknown
                .conclusion
                .contains("缺少必需的配置项 registry_seed")
        );
    }

    /// 游戏自己写下的报告：stdout 没有可用特征时，报告里的阶段/异常/直接原因才是证据
    const GAME_CRASH_REPORT: &str = r#"---- Minecraft Crash Report ----
// I bet Cylons wouldn't have this problem.
Time: 2026-10-18 22:35:26
Description: Rendering overlay

java.lang.NullPointerException: Cannot invoke "net.minecraft.client.Minecraft" because "this.mc" is null
	at com.example.Mod.overlay(Mod.java:10)
Caused by: java.lang.IllegalStateException: renderer already closed
	at com.example.Renderer.close(Renderer.java:42)

A detailed walkthrough of the error, its code path and all known details is as follows:
"#;

    #[test]
    fn test_game_crash_report_supplies_a_finding_when_stdout_has_none() {
        let d = diagnose_with_crash_report(
            Some(1),
            &[],
            &ctx_plain(),
            None,
            Some(CrashReportEvidence {
                path: "crash-reports/crash-test-client.txt",
                text: GAME_CRASH_REPORT,
            }),
        );

        assert!(d.recognized);
        let f = only(&d, FindingKind::GameCrashReport);
        assert_eq!(f.role, FindingRole::Root);
        assert!(f.summary.contains("Rendering overlay"));
        assert!(f.conclusion.contains("NullPointerException"));
        assert!(f.conclusion.contains("renderer already closed"));
        assert_eq!(f.evidence.len(), 3);
        assert_eq!(f.evidence[0].line_no, 4);
        assert!(
            d.context
                .iter()
                .any(|c| c.label == "崩溃阶段" && c.value == "Rendering overlay")
        );
        assert!(
            d.context
                .iter()
                .any(|c| c.label == "崩溃报告" && c.value.ends_with("crash-test-client.txt"))
        );
        assert!(d.raw_output.contains("--- 游戏崩溃报告"));
        assert!(d.report_text.contains("renderer already closed"));
    }

    #[test]
    fn test_existing_stdout_root_keeps_precedence_over_the_report() {
        let log = "java.lang.OutOfMemoryError: Java heap space";

        let d = diagnose_with_crash_report(
            Some(1),
            &lines(log),
            &ctx_plain(),
            None,
            Some(CrashReportEvidence {
                path: "crash-reports/crash-test-client.txt",
                text: GAME_CRASH_REPORT,
            }),
        );

        only(&d, FindingKind::Memory);
        assert!(
            !d.findings
                .iter()
                .any(|f| f.kind == FindingKind::GameCrashReport),
            "stdout 已经定位到根因时，报告摘要不该抢 headline"
        );
        assert!(
            d.context.iter().any(|c| c.label == "崩溃报告"),
            "报告路径仍要附在环境里"
        );
    }

    #[test]
    fn test_report_evidence_does_not_shift_stdout_excerpt() {
        let stdout = lines("stdout first line\nstdout second line");

        let d = diagnose_with_crash_report(
            Some(1),
            &stdout,
            &ctx_plain(),
            None,
            Some(CrashReportEvidence {
                path: "crash-reports/crash-test-client.txt",
                text: GAME_CRASH_REPORT,
            }),
        );

        assert!(
            d.excerpt.iter().all(|e| e.text.starts_with("stdout")),
            "报告行号不能拿来截 stdout: {:?}",
            d.excerpt
        );
    }

    #[test]
    fn test_hs_err_report_becomes_a_jvm_crash_finding() {
        let hs = "# A fatal error has been detected by the Java Runtime Environment:\n#\t# Internal Error (safepoint.cpp:512), pid=1234, tid=5678\n#\t# Problematic frame:\n#\t# V  [libjvm.dll+0x8a1b2c]";

        let d = diagnose(Some(1), &[], &ctx_plain(), Some(hs));

        assert!(d.recognized);
        let f = only(&d, FindingKind::JvmCrash);
        assert!(f.culprits.iter().any(|c| c.contains("libjvm.dll")));
        assert_eq!(f.culprit_label, "问题文件");
        assert_eq!(d.findings.first().unwrap().kind, FindingKind::JvmCrash);
    }

    #[test]
    fn test_context_lines_explain_the_environment() {
        let d = diagnose(Some(1), &lines(MIXIN_CRASH), &ctx_java_26(), None);

        let value = |label: &str| {
            d.context
                .iter()
                .find(|c| c.label == label)
                .map(|c| c.value.clone())
        };
        assert_eq!(value("实例").as_deref(), Some("Closing Song1.6.6.1"));
        assert!(value("本次 Java").unwrap().contains("26"));
        assert!(value("本次 Java").unwrap().contains("zulu-26"));
        assert_eq!(value("模组数").as_deref(), Some("3"));
    }

    #[test]
    fn test_excerpt_keeps_the_window_around_the_hit_line() {
        let mut big: Vec<String> = (1..=150).map(|i| format!("前置噪音第 {} 行", i)).collect();
        big.extend(lines(MIXIN_CRASH));

        let d = diagnose(Some(1), &big, &ctx_java_26(), None);

        assert!(
            d.excerpt
                .iter()
                .any(|e| e.text.contains("Mixin apply for mod fancymenu"))
        );
        assert!(d.excerpt[0].line_no > 130, "窗口应当跟着命中行走");
        assert_eq!(d.raw_output.lines().count(), big.len(), "原文不能只留尾巴");
    }

    #[test]
    fn test_raw_output_trims_the_head_and_says_how_much() {
        let big: Vec<String> = (1..=2500).map(|i| format!("line {}", i)).collect();

        let d = diagnose(Some(1), &big, &ctx_plain(), None);

        assert_eq!(d.raw_output.lines().count(), RAW_OUTPUT_MAX_LINES + 1);
        assert!(d.raw_output.starts_with("… 前面 500 行未收录"));
        assert!(d.raw_output.ends_with("line 2500"));
    }

    #[test]
    fn test_report_text_carries_conclusion_evidence_and_environment() {
        let d = diagnose(Some(1), &lines(MIXIN_CRASH), &ctx_java_26(), None);

        assert!(d.report_text.contains(&d.headline));
        assert!(d.report_text.contains("根因 · 直接证据"));
        assert!(d.report_text.contains("net.minecraft.client.Minecraft"));
        assert!(d.report_text.contains("本次 Java"));
        assert!(
            d.report_text
                .contains("问题模组：fancymenu_forge_3.3.2_MC_1.20.1.jar")
        );
    }

    /// Java 匹配由启动闸门负责，崩溃报告里再列一遍「声明 Java」会把人引偏
    #[test]
    fn test_context_reports_runtime_and_loader_not_the_java_gate_again() {
        let mut ctx = ctx_java_26();
        ctx.runtime_secs = Some(95);
        let d = diagnose(Some(1), &lines(MIXIN_CRASH), &ctx, None);

        let labels: Vec<&str> = d.context.iter().map(|c| c.label.as_str()).collect();
        assert!(!labels.contains(&"声明 Java"));
        assert!(labels.contains(&"加载器"));
        assert!(labels.contains(&"启动到崩溃"));
        assert!(
            d.context
                .iter()
                .any(|c| c.label == "启动到崩溃" && c.value.contains("95"))
        );
    }

    /// Java 与声明一致时不该把它当成嫌疑，否则每条崩溃都带上同一句无关提示
    #[test]
    fn test_java_rule_stays_quiet_when_used_matches_declared() {
        let d = diagnose(Some(1), &lines(MIXIN_CRASH), &ctx_plain(), None);

        assert!(
            !d.findings
                .iter()
                .any(|f| f.kind == FindingKind::JavaVersion)
        );
    }

    /// 1.13~1.16 的 Forge 撞上 Java 9+ 的模块系统时，日志里长这样
    const MODULE_ACCESS_CRASH: &str = r#"Exception in thread "main" java.lang.reflect.InvocationTargetException
	at java.base/jdk.internal.reflect.NativeMethodAccessorImpl.invoke0(Native Method)
	at cpw.mods.modlauncher.Launcher.main(Launcher.java:62)
Caused by: java.lang.reflect.InaccessibleObjectException: Unable to make field private final java.lang.String java.lang.Class.name accessible: module java.base does not "opens java.lang" to unnamed module @3d4eac69
	at java.base/java.lang.reflect.AccessibleObject.checkCanSetAccessible(AccessibleObject.java:354)"#;

    fn ctx_java_25_declared_8() -> LaunchContext {
        LaunchContext {
            version_id: "1.13.2-Forge".to_string(),
            loader: "Forge".to_string(),
            declared_java: Some(8),
            used_java_path: "D:/Program Files/BellSoft/LibericaJDK-25/bin/java.exe".to_string(),
            used_java_version: Some(25),
            memory_mb: 9600,
            mod_files: Vec::new(),
            runtime_secs: Some(1),
        }
    }

    #[test]
    fn test_module_access_with_newer_java_is_a_high_confidence_root_cause() {
        let d = diagnose(
            Some(1),
            &lines(MODULE_ACCESS_CRASH),
            &ctx_java_25_declared_8(),
            None,
        );

        assert!(d.recognized, "模块访问被拒应当算已识别");
        let java = d
            .findings
            .iter()
            .find(|f| f.kind == FindingKind::JavaVersion)
            .expect("应当把 Java 版本列为根因");
        assert_eq!(java.confidence, Confidence::High);
        assert_eq!(java.role, FindingRole::Root);
        assert!(java.conclusion.contains("25"));
        assert!(java.conclusion.contains("8"));
        assert!(java.suggestion.contains("Java 8"), "{}", java.suggestion);

        // 已经点名 Java 之后不该再补一条更弱的同义推断
        assert_eq!(
            d.findings
                .iter()
                .filter(|f| f.kind == FindingKind::JavaVersion)
                .count(),
            1
        );
    }

    #[test]
    fn test_module_access_is_not_blamed_on_java_when_versions_match() {
        let mut ctx = ctx_java_25_declared_8();
        ctx.declared_java = Some(25);
        let d = diagnose(Some(1), &lines(MODULE_ACCESS_CRASH), &ctx, None);

        // 声明与本次一致，那是模组自己要 add-opens，不是 JDK 太新
        assert!(
            !d.findings
                .iter()
                .any(|f| f.kind == FindingKind::JavaVersion)
        );
    }

    #[test]
    fn test_unknown_rule_prefers_the_exception_over_a_stack_frame() {
        // 尾巴只剩栈帧时，把 at cpw.mods.modlauncher... 当根因没有意义
        let text = "Exception in thread \"main\" java.lang.reflect.InvocationTargetException\n\tat cpw.mods.modlauncher.Launcher.main(Launcher.java:62)";
        let d = diagnose(Some(1), &lines(text), &ctx_java_25_declared_8(), None);

        let unknown = d
            .findings
            .iter()
            .find(|f| f.kind == FindingKind::Unknown)
            .expect("没有特征命中时应当给出未识别");
        assert!(
            unknown.conclusion.contains("InvocationTargetException"),
            "{}",
            unknown.conclusion
        );
        assert!(!unknown.conclusion.contains("最深的异常原因：at "));
    }

    #[test]
    fn test_unknown_rule_does_not_call_a_frame_a_root_cause() {
        let text = "\tat cpw.mods.modlauncher.Launcher.main(Launcher.java:62)";
        let d = diagnose(Some(1), &lines(text), &ctx_java_25_declared_8(), None);

        let unknown = d
            .findings
            .iter()
            .find(|f| f.kind == FindingKind::Unknown)
            .expect("应当给出未识别");
        assert!(
            unknown.conclusion.contains("没有异常类型"),
            "{}",
            unknown.conclusion
        );
    }

    /// 参数被拆开后，JVM 会把某个 `-D` 的值当成主类；
    /// 这条规则要能说清“不是缺文件，是参数拼坏了”
    #[test]
    fn test_orphan_jvm_value_is_reported_as_broken_arguments() {
        let log = "Error: Could not find or load main class java.base/jdk.internal.misc=ALL-UNNAMED
Caused by: java.lang.ClassNotFoundException: java.base/jdk.internal.misc=ALL-UNNAMED";
        let d = diagnose(Some(1), &lines(log), &ctx_plain(), None);

        let f = only(&d, FindingKind::MainClass);
        assert_eq!(f.role, FindingRole::Root);
        assert_eq!(f.confidence, Confidence::High);
        assert!(f.conclusion.contains("参数"), "{}", f.conclusion);
        assert!(f.suggestion.contains("重建"), "{}", f.suggestion);
    }

    #[test]
    fn test_localized_main_class_error_is_recognized() {
        let log = "错误: 找不到或无法加载主类 net.neoforged.fml.startup.Client
原因: java.lang.ClassNotFoundException: net.neoforged.fml.startup.Client";
        let d = diagnose(Some(1), &lines(log), &ctx_plain(), None);

        let f = only(&d, FindingKind::MainClass);
        assert!(f.conclusion.contains("classpath"), "{}", f.conclusion);
        assert_eq!(
            f.culprits,
            vec!["net.neoforged.fml.startup.Client".to_string()]
        );
    }

    fn ctx_quilt_26_3() -> LaunchContext {
        LaunchContext {
            version_id: "26.3-Quilt".to_string(),
            loader: "Quilt".to_string(),
            declared_java: Some(25),
            used_java_path: "D:/Program Files/BellSoft/LibericaJDK-25/bin/java.exe".to_string(),
            used_java_version: Some(25),
            memory_mb: 9600,
            mod_files: Vec::new(),
            runtime_secs: Some(1),
        }
    }

    /// Quilt 0.20.0-beta.9 自带 ASM 9.5，读不了 Java 25 的类。
    /// 此时“换 Java”是句废话——本次就是 25，该换的是加载器。
    #[test]
    fn test_loader_too_old_advice_is_to_upgrade_loader_not_java() {
        let log = "\
[01:50:42] [main/INFO]: Loading Minecraft 26.3 with Quilt Loader 0.20.0-beta.9
[01:50:42] [main/INFO]: Mappings not present!
[01:50:42] [main/ERROR]: Uncaught exception in thread \"main\"
java.lang.IllegalArgumentException: Unsupported class file major version 69
\tat org.objectweb.asm.ClassReader.<init>(ClassReader.java:199)
\tat org.quiltmc.loader.impl.entrypoint.GameTransformer.lambda$locateEntrypoints$0(GameTransformer.java:89)
\tat org.quiltmc.loader.impl.game.minecraft.MinecraftGameProvider.initialize(MinecraftGameProvider.java:407)";

        let d = diagnose(Some(1), &lines(log), &ctx_quilt_26_3(), None);
        let finding = only(&d, FindingKind::ClassReaderTooOld);

        assert!(
            finding.suggestion.contains("升级"),
            "应指向升级加载器: {}",
            finding.suggestion
        );
        assert!(
            finding.suggestion.contains("Quilt"),
            "应点名是哪个加载器: {}",
            finding.suggestion
        );
        assert!(
            !finding.suggestion.contains("换成版本 JSON 声明的 Java 25"),
            "本次已经是 Java 25，再说换 Java 是误导: {}",
            finding.suggestion
        );
        assert_eq!(finding.role, FindingRole::Root);
    }
}
