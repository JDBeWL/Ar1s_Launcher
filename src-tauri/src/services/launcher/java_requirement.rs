//! Java 需求判定模块
//!
//! 下限取 Mojang 版本 JSON 声明的 javaVersion（读不到时按 MC 版本号推断），mods 目录实测抬高下限；
//! 只有 LaunchWrapper 时代的老 Forge 另设硬上限（等于声明值），其余加载器不设上限

use super::java::JavaCandidate;
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::SystemTime;

/// 声明缺失时的兜底；Mojang 对每个版本都写了 javaVersion，正常不会走到
pub const FALLBACK_JAVA: u32 = 8;

/// 允许向下搜索 inheritsFrom 的层数，实例 → 加载器 → 原版
const INHERITS_DEPTH: u32 = 4;

/// 一个 jar 里取样多少个 class 就够判断它的编译目标；整包全扫在大 mod 上是白费
const CLASS_SAMPLE_LIMIT: usize = 32;

/// 一个 Java 版本至少出现在这么多个类里才算数；单个高版本类多是可选分支或误打包
const MIN_CLASSES_FOR_FLOOR: usize = 2;

/// jar class 版本探测缓存上限；一个实例的模组远少于这个数
const JAR_JAVA_CACHE_MAX: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoaderKind {
    Vanilla,
    Forge,
    /// LaunchWrapper 时代的 Forge（1.12 及更早）：类加载与反射依赖被 Java 9+ 的模块系统切断
    ForgeLegacy,
    NeoForge,
    Fabric,
    Quilt,
}

impl LoaderKind {
    pub fn label(self) -> &'static str {
        match self {
            LoaderKind::Vanilla => "原版",
            LoaderKind::Forge => "Forge",
            LoaderKind::ForgeLegacy => "Forge（LaunchWrapper 时代）",
            LoaderKind::NeoForge => "NeoForge",
            LoaderKind::Fabric => "Fabric",
            LoaderKind::Quilt => "Quilt",
        }
    }

    /// 上限只给旧版 Forge：它的声明值就是实际能用的最高 JDK，不是最低要求。
    /// modlauncher 之后的 Forge（1.13+）能往上走，NeoForge 没有历史包袱；具体上限见 `loader_ceiling`
    fn caps_at_declared(self) -> bool {
        matches!(self, LoaderKind::ForgeLegacy)
    }
}

/// 加载器自身的兼容上限：高于这条线确定起不来，不是「不推荐」。
/// 1.13~1.15 的 modlauncher 仍依赖被 Java 9+ 模块系统切断的反射，实测只能在 8 上跑；
/// 1.16 的官方范围是 8 与 11；17 以后没有「确定不兼容」的组合，不设上限
fn loader_ceiling(loader: LoaderKind, declared: u32, mc: Option<(u32, u32)>) -> Option<u32> {
    if loader.caps_at_declared() {
        return Some(declared);
    }

    match (loader, mc) {
        (LoaderKind::Forge, Some((1, 13..=15))) => Some(8),
        (LoaderKind::Forge, Some((1, 16))) => Some(11),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JavaRequirement {
    /// Mojang 声明的大版本；自动匹配优先命中它
    pub target: u32,
    /// 可能被 mods 的 class 版本抬高
    pub min: u32,
    /// None 表示没有已知的上限
    pub hard_max: Option<u32>,
    pub reasons: Vec<String>,
}

impl JavaRequirement {
    /// min > hard_max 时没有任何 Java 能满足，只能靠报告说明
    pub fn unsatisfiable(&self) -> bool {
        self.hard_max.is_some_and(|max| self.min > max)
    }
}

#[derive(Debug, Clone)]
pub struct ModProbe {
    pub file: String,
    /// 参与下限判定的版本：出现次数达到阈值的最高 Java
    pub java: u32,
    /// 取样到的原始最高版本，可能比 `java` 高
    pub highest: u32,
    pub highest_count: usize,
    pub sampled: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JarJavaProfile {
    /// 用于下限判定的版本：忽略只出现一两次的孤立高版本类
    pub floor: u32,
    pub highest: u32,
    pub highest_count: usize,
    pub sampled: usize,
}

struct CachedJarJava {
    len: u64,
    modified: Option<SystemTime>,
    profile: JarJavaProfile,
}

/// jar 路径 -> 长度/修改时间与探测结果；启动前 `check_java_compatibility`
/// 和真正启动会各扫一次，没有这层缓存时同一个 jar 要被打开两遍
static JAR_JAVA_CACHE: LazyLock<Mutex<HashMap<PathBuf, CachedJarJava>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[derive(Debug, Clone)]
pub struct InstanceProfile {
    pub requirement: JavaRequirement,
    pub loader: LoaderKind,
    pub mods: Vec<ModProbe>,
    /// 版本 JSON 里真实写了 javaVersion 才有值；兜底成的 8 不算声明
    pub declared_java: Option<u32>,
    /// 版本 JSON 读不到时为 false，此时不做硬拦
    pub resolved: bool,
    pub version_id: String,
    pub game_dir: PathBuf,
}

pub fn class_file_to_java(major: u32) -> Option<u32> {
    // class 48 = Java 4，往后每加 1 对应一个新大版本
    if (48..=90).contains(&major) {
        Some(major - 44)
    } else {
        None
    }
}

/// 版本 JSON 读不到 javaVersion 时（整合包自带 JSON、父版本没装、手写实例）按 MC 版本号推断，
/// 兜底成 8 会让 1.21 实例选到 Java 8 然后在启动瞬间崩
pub fn infer_java_for_version(version_id: &str) -> Option<u32> {
    let base = version_id.split('-').next()?.trim();
    let mut parts = base.split('.');

    if parts.next()?.parse::<u32>().ok()? != 1 {
        return None;
    }

    let minor: u32 = parts.next()?.parse().ok()?;
    let patch: u32 = parts
        .next()
        .and_then(|p| p.parse::<u32>().ok())
        .unwrap_or(0);

    Some(match minor {
        0..=16 => 8,
        17..=19 => 17,
        // 1.20.5 起 Mojang 把声明改成了 21
        20 if patch >= 5 => 21,
        20 => 17,
        _ => 21,
    })
}

pub fn declared_major(json: &Value) -> Option<u32> {
    json.get("javaVersion")?
        .get("majorVersion")?
        .as_u64()
        .map(|v| v as u32)
}

fn has_library(json: &Value, group: &str) -> bool {
    json["libraries"].as_array().is_some_and(|libs| {
        libs.iter().any(|lib| {
            lib["name"]
                .as_str()
                .or_else(|| lib["group"].as_str())
                .is_some_and(|name| name.starts_with(group))
        })
    })
}

/// Forge 还要再分一代：老 Forge 靠 LaunchWrapper 启动，实例 JSON 里会留 `net.minecraft.launchwrapper`
/// —— 要么在 libraries 里，要么直接就是 mainClass。两个信号都认，避免被裁过 libraries 的整合包漏判
fn forge_kind(json: &Value) -> LoaderKind {
    let legacy_by_lib = has_library(json, "net.minecraft.launchwrapper");
    let legacy_by_main = json["mainClass"]
        .as_str()
        .is_some_and(|main| main.starts_with("net.minecraft.launchwrapper"));

    if legacy_by_lib || legacy_by_main {
        LoaderKind::ForgeLegacy
    } else {
        LoaderKind::Forge
    }
}

/// 优先看 libraries（最可靠），退回版本 id 里的加载器字样
pub fn detect_loader(json: &Value, version_id: &str) -> LoaderKind {
    if has_library(json, "net.neoforged") {
        return LoaderKind::NeoForge;
    }
    if has_library(json, "net.minecraftforge") {
        return forge_kind(json);
    }
    // Quilt 自己依赖一堆 net.fabricmc 的库，必须先认 quilt-loader，否则会被当成 Fabric
    if has_library(json, "org.quiltmc") {
        return LoaderKind::Quilt;
    }
    if has_library(json, "net.fabricmc") {
        return LoaderKind::Fabric;
    }

    let id = version_id.to_lowercase();
    if id.contains("neoforge") {
        LoaderKind::NeoForge
    } else if id.contains("forge") {
        LoaderKind::Forge
    } else if id.contains("fabric") {
        LoaderKind::Fabric
    } else if id.contains("quilt") {
        LoaderKind::Quilt
    } else {
        LoaderKind::Vanilla
    }
}

fn sample_class_versions(jar: &Path) -> Option<Vec<u32>> {
    let file = fs::File::open(jar).ok()?;
    let mut archive = zip::ZipArchive::new(file).ok()?;

    let mut versions = Vec::with_capacity(CLASS_SAMPLE_LIMIT);
    let mut examined = 0usize;

    for i in 0..archive.len() {
        if examined >= CLASS_SAMPLE_LIMIT {
            break;
        }

        let Ok(mut entry) = archive.by_index(i) else {
            continue;
        };
        let Ok(name) = entry.name() else { continue };
        if !name.ends_with(".class")
            || name.starts_with("META-INF/")
            || name.ends_with("module-info.class")
        {
            continue;
        }
        examined += 1;

        let mut head = [0u8; 8];
        if entry.read_exact(&mut head).is_err() {
            continue;
        }
        if head[..4] != [0xCA, 0xFE, 0xBA, 0xBE] {
            continue;
        }

        if let Some(java) = class_file_to_java(u16::from_be_bytes([head[6], head[7]]) as u32) {
            versions.push(java);
        }
    }

    (!versions.is_empty()).then_some(versions)
}

/// 读 jar 里 class 的最高主版本号（取样上限内）
///
/// 只看第一个 entry 会被 `module-info.class`、签名文件或字母序靠前的低版本类带偏；
/// `META-INF/versions/` 是多版本 jar 的高版本实现，运行时不一定加载，跳过才不会误判
pub fn jar_java_version(jar: &Path) -> Option<u32> {
    sample_class_versions(jar)?.into_iter().max()
}

fn profile_from_versions(versions: &[u32]) -> JarJavaProfile {
    let mut counts: HashMap<u32, usize> = HashMap::new();
    for version in versions {
        *counts.entry(*version).or_default() += 1;
    }

    let (&highest, &highest_count) = counts
        .iter()
        .max_by_key(|(version, count)| (**version, **count))
        .expect("versions 非空");
    let sampled = versions.len();

    // 单个高版本类可能来自可选分支或误打包（例如 watervision 只有 1/23 个类是 21），
    // 不能因此把整个实例的下限抬上去；样本太少时宁可保守取最高
    let floor = if sampled <= 3 {
        highest
    } else {
        counts
            .iter()
            .filter(|(_, count)| **count >= MIN_CLASSES_FOR_FLOOR)
            .map(|(version, _)| *version)
            .max()
            .unwrap_or(highest)
    };

    JarJavaProfile {
        floor,
        highest,
        highest_count,
        sampled,
    }
}

pub fn jar_java_profile(jar: &Path) -> Option<JarJavaProfile> {
    Some(profile_from_versions(&sample_class_versions(jar)?))
}

fn cached_jar_java(path: &Path, len: u64, modified: Option<SystemTime>) -> Option<JarJavaProfile> {
    if let Ok(cache) = JAR_JAVA_CACHE.lock()
        && let Some(hit) = cache.get(path)
        && hit.len == len
        && hit.modified == modified
    {
        return Some(hit.profile.clone());
    }

    let profile = jar_java_profile(path)?;
    if let Ok(mut cache) = JAR_JAVA_CACHE.lock() {
        if cache.len() >= JAR_JAVA_CACHE_MAX && !cache.contains_key(path) {
            cache.clear();
        }
        cache.insert(
            path.to_path_buf(),
            CachedJarJava {
                len,
                modified,
                profile: profile.clone(),
            },
        );
    }
    Some(profile)
}

struct ModFile {
    path: PathBuf,
    name: String,
    len: u64,
    modified: Option<SystemTime>,
}

fn scan_mod_file(file: &ModFile) -> Option<ModProbe> {
    let profile = cached_jar_java(&file.path, file.len, file.modified)?;
    Some(ModProbe {
        file: file.name.clone(),
        java: profile.floor,
        highest: profile.highest,
        highest_count: profile.highest_count,
        sampled: profile.sampled,
    })
}

fn scan_mod_files(files: Vec<ModFile>) -> Vec<ModProbe> {
    if files.is_empty() {
        return Vec::new();
    }

    let workers = std::thread::available_parallelism()
        .map(|count| count.get())
        .unwrap_or(4)
        .min(8)
        .min(files.len());

    if workers <= 1 {
        return files.iter().filter_map(scan_mod_file).collect();
    }

    let chunk_size = files.len().div_ceil(workers);
    std::thread::scope(|scope| {
        let handles: Vec<_> = files
            .chunks(chunk_size)
            .map(|chunk| {
                scope.spawn(move || chunk.iter().filter_map(scan_mod_file).collect::<Vec<_>>())
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|handle| handle.join().unwrap_or_default())
            .collect()
    })
}

/// 禁用中的 *.jar.disabled 不算
pub fn scan_mods(mods_dir: &Path) -> Vec<ModProbe> {
    let Ok(entries) = fs::read_dir(mods_dir) else {
        return Vec::new();
    };

    let mut files = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.to_lowercase().ends_with(".jar") {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }

        files.push(ModFile {
            path: entry.path(),
            name,
            len: metadata.len(),
            modified: metadata.modified().ok(),
        });
    }

    let mut probes = scan_mod_files(files);
    probes.sort_by(|a, b| b.java.cmp(&a.java).then_with(|| a.file.cmp(&b.file)));
    probes
}

pub fn build_requirement(
    declared: Option<u32>,
    loader: LoaderKind,
    mods: &[ModProbe],
) -> JavaRequirement {
    build_requirement_with_measurements(declared, None, None, loader, None, mods)
}

/// `inferred` 是按 MC 版本号推出来的目标版本，只在 JSON 没有声明时生效；
/// `mc` 用来判断这一代加载器自己还能不能吃到新 JDK
pub fn build_requirement_with_fallback(
    declared: Option<u32>,
    inferred: Option<u32>,
    loader: LoaderKind,
    mc: Option<(u32, u32)>,
    mods: &[ModProbe],
) -> JavaRequirement {
    build_requirement_with_measurements(declared, inferred, None, loader, mc, mods)
}

/// `client_java` 是实际客户端 jar 的 class 版本实测值；有它时下限跟实测走，
/// 声明值只作为优先目标，避免 JSON 写 21、实际只需要 17 时被迫上 25
pub fn build_requirement_with_measurements(
    declared: Option<u32>,
    inferred: Option<u32>,
    client_java: Option<u32>,
    loader: LoaderKind,
    mc: Option<(u32, u32)>,
    mods: &[ModProbe],
) -> JavaRequirement {
    let target = declared.or(inferred).unwrap_or(FALLBACK_JAVA);
    let mut reasons = Vec::new();

    match (declared, inferred) {
        (Some(v), _) => reasons.push(format!("Mojang 声明 javaVersion.majorVersion = {}", v)),
        (None, Some(v)) => reasons.push(format!(
            "版本 JSON 没有 javaVersion 声明，按 MC 版本号推定为 Java {}",
            v
        )),
        (None, None) => reasons.push(format!(
            "版本 JSON 没有 javaVersion 声明，按 Java {} 兜底",
            FALLBACK_JAVA
        )),
    }

    if let Some(java) = client_java {
        reasons.push(format!(
            "客户端 jar 实测要求 Java {}（class {}）",
            java,
            java + 44
        ));
        if java != target {
            reasons.push(format!(
                "声明值是 Java {}、实测是 Java {}，Java 下限按实测取",
                target, java
            ));
        }
    }

    let hard_max = match loader_ceiling(loader, target, mc) {
        Some(max) if loader.caps_at_declared() => {
            reasons.push(format!(
                "{} 实例不高于声明的 Java {} 运行：旧类加载与反射依赖被 Java 9+ 的模块系统切断",
                loader.label(),
                target
            ));
            Some(max)
        }
        Some(max) => {
            reasons.push(format!(
                "{} 在 MC {} 上只能用到 Java {}：再新的 JDK 会切断它依赖的反射",
                loader.label(),
                mc.map(|(a, b)| format!("{}.{}", a, b))
                    .unwrap_or_else(|| "该版本".to_string()),
                max
            ));
            Some(max)
        }
        None => {
            reasons.push(format!("{} 实例不设上限，较新的 JDK 可用", loader.label()));
            None
        }
    };

    let mut min = client_java.unwrap_or(target);
    if let Some(highest) = mods.first()
        && highest.java > min
    {
        reasons.push(format!(
            "模组 {} 要求 Java {}（class 主版本），下限抬到 {}",
            highest.file, highest.java, highest.java
        ));
        min = highest.java;
    }

    // 孤立高版本类不计入下限，但要在报告里留痕，否则用户会疑惑“明明有 21 的类为什么用 17”
    if let Some(outlier) = mods
        .iter()
        .filter(|probe| probe.highest > probe.java)
        .max_by_key(|probe| probe.highest)
    {
        reasons.push(format!(
            "模组 {} 的最高 class 为 Java {}，但取样 {} 个类中仅 {} 个，按孤立类忽略",
            outlier.file, outlier.highest, outlier.sampled, outlier.highest_count
        ));
    }

    JavaRequirement {
        target,
        min,
        hard_max,
        reasons,
    }
}

pub fn java_allowed(req: &JavaRequirement, java: u32) -> bool {
    java >= req.min && req.hard_max.is_none_or(|max| java <= max)
}

fn read_version_json(game_dir: &Path, version_id: &str) -> Option<Value> {
    let path = game_dir
        .join("versions")
        .join(version_id)
        .join(format!("{}.json", version_id));
    let content = fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

/// 沿 inheritsFrom 走到底：带回第一个 javaVersion 声明，以及链尾那个版本 id（推断目标版本用）
fn resolve_declaration(game_dir: &Path, version_id: &str) -> (Option<u32>, String) {
    let mut current = version_id.to_string();
    let mut declared = None;

    for _ in 0..=INHERITS_DEPTH {
        let Some(json) = read_version_json(game_dir, &current) else {
            break;
        };

        if declared.is_none() {
            declared = declared_major(&json);
        }

        match json["inheritsFrom"].as_str() {
            Some(parent) => current = parent.to_string(),
            None => break,
        }
    }

    (declared, current)
}

/// 沿继承链收集实际会作为客户端 jar 的文件；实例目录里可能有整包复制过来的 jar，
/// 加载器实例则通常只有父版本那一份
fn collect_client_jars(game_dir: &Path, version_id: &str) -> Vec<PathBuf> {
    let versions_dir = game_dir.join("versions");
    let mut jars = Vec::new();
    let mut current = version_id.to_string();
    let mut seen = std::collections::HashSet::new();

    for _ in 0..=INHERITS_DEPTH {
        if !seen.insert(current.clone()) {
            break;
        }

        let Ok(dir) = crate::utils::file_utils::version_dir(&versions_dir, &current) else {
            break;
        };
        let own = dir.join(format!("{}.jar", current));
        if own.is_file() {
            jars.push(own);
        }

        let Some(json) = read_version_json(game_dir, &current) else {
            break;
        };

        if let Some(jar_id) = json["jar"].as_str()
            && let Ok(jar_dir) = crate::utils::file_utils::version_dir(&versions_dir, jar_id)
        {
            let jar = jar_dir.join(format!("{}.jar", jar_id));
            if jar.is_file() {
                jars.push(jar);
            }
        }

        match json["inheritsFrom"].as_str() {
            Some(parent) => current = parent.to_string(),
            None => break,
        }
    }

    jars.sort();
    jars.dedup();
    jars
}

fn measure_client_java(game_dir: &Path, version_id: &str) -> Option<u32> {
    collect_client_jars(game_dir, version_id)
        .into_iter()
        .filter_map(|jar| {
            let metadata = fs::metadata(&jar).ok()?;
            cached_jar_java(&jar, metadata.len(), metadata.modified().ok())
                .map(|profile| profile.floor)
        })
        .max()
}

/// 模组目录跟着版本隔离走：开启时在实例目录，关闭时是共享的根目录。
/// 判错会把共享目录的模组算进这个实例，Java 下限与模组清单因此指向没被加载的文件
pub fn mods_dir(game_dir: &Path, version_id: &str, isolated: bool) -> PathBuf {
    if isolated {
        game_dir.join("versions").join(version_id).join("mods")
    } else {
        game_dir.join("mods")
    }
}

/// 诊断报告要按 mod id 反指到具体 jar，这里只取名字不做 class 探测
pub fn list_mod_files(mods_dir: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(mods_dir) else {
        return Vec::new();
    };

    entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            name.to_lowercase().ends_with(".jar").then_some(name)
        })
        .collect()
}

pub fn analyze(version_id: &str, game_dir: &Path) -> InstanceProfile {
    let instance_json = read_version_json(game_dir, version_id);
    let loader = instance_json
        .as_ref()
        .map(|json| detect_loader(json, version_id))
        .unwrap_or(LoaderKind::Vanilla);

    let (declared, base_version_id) = resolve_declaration(game_dir, version_id);
    let inferred = infer_java_for_version(&base_version_id);
    // 兼容上限按这一代加载器算：1.13 的 Forge 换到 Java 25 是必崩，不是“不推荐”
    let mc = crate::utils::minecraft::parse_mc_version(&base_version_id);

    let mods = scan_mods(&mods_dir(
        game_dir,
        version_id,
        crate::services::config::is_version_isolated(),
    ));
    let client_java = measure_client_java(game_dir, version_id);

    InstanceProfile {
        requirement: build_requirement_with_measurements(
            declared,
            inferred,
            client_java,
            loader,
            mc,
            &mods,
        ),
        loader,
        mods,
        declared_java: declared,
        resolved: instance_json.is_some(),
        version_id: version_id.to_string(),
        game_dir: game_dir.to_path_buf(),
    }
}

/// 给详情对话框看的纯文本报告；同时会被写进日志
pub fn build_report(
    profile: &InstanceProfile,
    current: Option<(&str, u32)>,
    candidates: &[JavaCandidate],
) -> String {
    let req = &profile.requirement;
    let mut lines: Vec<String> = Vec::new();

    lines.push(format!("实例：{}", profile.version_id));
    lines.push(format!("加载器：{}", profile.loader.label()));
    lines.push(format!(
        "Java 要求：{}（优先 {}）{}",
        req.min,
        req.target,
        match req.hard_max {
            Some(max) => format!("，不得高于 {}", max),
            None => "，无上限".to_string(),
        }
    ));

    if let Some((path, ver)) = current {
        lines.push(format!("当前选择：Java {} · {}", ver, path));
    } else {
        lines.push("当前选择：未确定".to_string());
    }

    lines.push(String::new());
    lines.push("判定依据：".to_string());
    for reason in &req.reasons {
        lines.push(format!("  · {}", reason));
    }

    if !profile.mods.is_empty() {
        lines.push(String::new());
        lines.push(format!(
            "模组 class 版本（前 {} 项）：",
            profile.mods.len().min(8)
        ));
        for probe in profile.mods.iter().take(8) {
            let outlier = if probe.highest > probe.java {
                format!(
                    "（最高 {}，取样 {} 个类中仅 {} 个）",
                    probe.highest, probe.sampled, probe.highest_count
                )
            } else {
                String::new()
            };
            lines.push(format!(
                "  · {} → Java {}{}",
                probe.file, probe.java, outlier
            ));
        }
    }

    if req.unsatisfiable() {
        lines.push(String::new());
        lines.push(format!(
            "无法同时满足：{} 上限是 Java {}，但实例实测要求 Java {}。请把模组换成匹配版本，或用对应版本重建实例。",
            profile.loader.label(),
            req.hard_max.unwrap_or(0),
            req.min
        ));
    }

    lines.push(String::new());
    if candidates.is_empty() {
        lines.push("本机可用 Java：一个都没扫到".to_string());
    } else {
        lines.push("本机可用 Java：".to_string());
        for candidate in candidates {
            let mark = if java_allowed(req, candidate.version) {
                "可用"
            } else {
                "不行"
            };
            let arch = if candidate.arch == crate::services::java::JavaArch::Bits32 {
                " · 32 位"
            } else {
                ""
            };
            lines.push(format!(
                "  · Java {} [{}]{}{} {}",
                candidate.version,
                mark,
                arch,
                if candidate.is_ea { " · 预览版" } else { "" },
                candidate.path
            ));
        }

        // 有候选、但一个都进不了允许区间：这时候真正该做的是去装一份，而不是重试
        let any_allowed = candidates
            .iter()
            .any(|candidate| java_allowed(req, candidate.version));
        if !req.unsatisfiable() && !any_allowed {
            let band = match req.hard_max {
                Some(max) if max == req.min => format!("Java {}", req.min),
                Some(max) => format!("Java {}~{}", req.min, max),
                None => format!("Java {}+", req.min),
            };
            lines.push(format!(
                "这里没有能用的：该实例需要 {}，先在设置里补一份再来启动。",
                band
            ));
        }
    }

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::launcher::java::{JavaArch, JavaCandidate};

    fn cand(path: &str, version: u32) -> JavaCandidate {
        JavaCandidate {
            path: path.to_string(),
            version,
            arch: JavaArch::Bits64,
            is_ea: false,
            from_config: false,
        }
    }

    #[test]
    fn test_infer_java_for_version_covers_the_boundaries() {
        // 整合包父版本没装、JSON 里没有 javaVersion 时靠这条兜住
        assert_eq!(infer_java_for_version("1.12.2"), Some(8));
        assert_eq!(infer_java_for_version("1.16.5"), Some(8));
        assert_eq!(infer_java_for_version("1.17"), Some(17));
        assert_eq!(infer_java_for_version("1.20.1"), Some(17));
        // 1.20.5 起 Mojang 改声明成 21
        assert_eq!(infer_java_for_version("1.20.4"), Some(17));
        assert_eq!(infer_java_for_version("1.20.5"), Some(21));
        assert_eq!(infer_java_for_version("1.21.4"), Some(21));
        // 实例 id 上带加载器后缀也要认
        assert_eq!(infer_java_for_version("1.21.4-Fabric"), Some(21));
        assert_eq!(infer_java_for_version("1.20.1-forge-47.2.0"), Some(17));
        // 快照认不出来就交给兜底，不能乱猜
        assert_eq!(infer_java_for_version("24w14a"), None);
    }

    #[test]
    fn test_inferred_target_replaces_the_blind_java8_fallback() {
        let req = build_requirement_with_fallback(None, Some(21), LoaderKind::Vanilla, None, &[]);
        assert_eq!(req.target, 21);
        assert_eq!(req.min, 21);
        assert!(!java_allowed(&req, 8));
        assert!(java_allowed(&req, 21));
        assert!(
            req.reasons.iter().any(|r| r.contains("推定")),
            "{:?}",
            req.reasons
        );
    }

    #[test]
    fn test_jar_scan_ignores_module_info_and_multirelease() {
        let tmp = tempfile::tempdir().unwrap();

        // module-info 是 9+ 的产物，不代表这个 jar 需要 Java 9 才能跑
        let with_module_info = tmp.path().join("a.jar");
        write_jar_entries(
            &with_module_info,
            &[("module-info.class", 53), ("com/example/A.class", 52)],
        );
        assert_eq!(jar_java_version(&with_module_info), Some(8));

        // 多版本 jar 的 META-INF/versions 只在对应 JVM 上加载，不该抬高下限
        let multi_release = tmp.path().join("b.jar");
        write_jar_entries(
            &multi_release,
            &[
                ("com/example/A.class", 52),
                ("META-INF/versions/21/com/example/A.class", 65),
            ],
        );
        assert_eq!(jar_java_version(&multi_release), Some(8));
    }

    #[test]
    fn test_jar_scan_takes_the_highest_sampled_class() {
        // 只看第一个 entry 时，字母序靠前的低版本类会把要求压回旧版本
        let tmp = tempfile::tempdir().unwrap();
        let jar = tmp.path().join("mod.jar");
        write_jar_entries(
            &jar,
            &[
                ("aaa/Old.class", 52),
                ("zzz/New.class", 65),
                ("mmm/Mid.class", 61),
            ],
        );
        assert_eq!(jar_java_version(&jar), Some(21));
    }

    #[test]
    fn test_jar_profile_ignores_a_single_high_outlier() {
        // 实测自 Society Sunlit Valley：watervision 23 个类里只有 1 个是 Java 21
        let tmp = tempfile::tempdir().unwrap();
        let jar = tmp.path().join("watervision.jar");
        let mut entries: Vec<(String, u16)> = (0..22)
            .map(|i| (format!("com/old/C{}.class", i), 61))
            .collect();
        entries.push(("com/new/ExternalGlTexture.class".to_string(), 65));
        write_jar_entries(&jar, &entries);

        let profile = jar_java_profile(&jar).unwrap();
        assert_eq!(profile.floor, 17);
        assert_eq!(profile.highest, 21);
        assert_eq!(profile.highest_count, 1);
        assert_eq!(profile.sampled, 23);
        // raw max 仍然保留，给安装器之类的老调用方
        assert_eq!(jar_java_version(&jar), Some(21));
    }

    #[test]
    fn test_jar_profile_counts_repeated_high_version() {
        let tmp = tempfile::tempdir().unwrap();
        let jar = tmp.path().join("newmod.jar");
        let mut entries: Vec<(String, u16)> = (0..20)
            .map(|i| (format!("com/example/C{}.class", i), 61))
            .collect();
        entries.push(("com/example/A.class".to_string(), 65));
        entries.push(("com/example/B.class".to_string(), 65));
        write_jar_entries(&jar, &entries);

        let profile = jar_java_profile(&jar).unwrap();
        assert_eq!(profile.floor, 21);
        assert_eq!(profile.highest_count, 2);
    }

    #[test]
    fn test_jar_profile_trusts_a_tiny_jar() {
        let tmp = tempfile::tempdir().unwrap();
        let jar = tmp.path().join("tiny.jar");
        write_jar(&jar, 65);
        assert_eq!(jar_java_profile(&jar).unwrap().floor, 21);
    }

    #[test]
    fn test_scan_mods_does_not_raise_floor_for_a_lone_class() {
        let tmp = tempfile::tempdir().unwrap();
        let jar = tmp.path().join("watervision.jar");
        let mut entries: Vec<(String, u16)> = (0..22)
            .map(|i| (format!("com/old/C{}.class", i), 61))
            .collect();
        entries.push(("com/new/ExternalGlTexture.class".to_string(), 65));
        write_jar_entries(&jar, &entries);

        let probes = scan_mods(tmp.path());
        assert_eq!(probes.len(), 1);
        assert_eq!(probes[0].java, 17);
        assert_eq!(probes[0].highest, 21);
        assert_eq!(probes[0].highest_count, 1);
    }

    #[test]
    fn test_client_jar_measurement_can_lower_the_declared_floor() {
        let mods = vec![ModProbe {
            file: "watervision.jar".to_string(),
            java: 17,
            highest: 21,
            highest_count: 1,
            sampled: 23,
        }];
        let req = build_requirement_with_measurements(
            Some(21),
            None,
            Some(17),
            LoaderKind::Vanilla,
            None,
            &mods,
        );

        assert_eq!(req.target, 21);
        assert_eq!(req.min, 17);
        assert!(java_allowed(&req, 17));
        assert!(req.reasons.iter().any(|r| r.contains("客户端 jar 实测")));
        assert!(req.reasons.iter().any(|r| r.contains("按孤立类忽略")));
    }

    #[test]
    fn test_missing_client_jar_keeps_the_declared_floor() {
        let req = build_requirement_with_measurements(
            Some(21),
            None,
            None,
            LoaderKind::Vanilla,
            None,
            &[],
        );
        assert_eq!(req.min, 21);
    }

    #[test]
    fn test_mod_floor_still_wins_over_a_lower_client_measurement() {
        let req = build_requirement_with_measurements(
            Some(21),
            None,
            Some(17),
            LoaderKind::Vanilla,
            None,
            &[probe("requires21.jar", 21)],
        );
        assert_eq!(req.min, 21);
    }

    /// 实例 JSON 自己带 jar 时直接读；加载器实例没 jar 时沿 inheritsFrom 找父版本
    #[test]
    fn test_measure_client_java_follows_inherits() {
        let tmp = tempfile::tempdir().unwrap();
        let game = tmp.path().to_path_buf();
        let versions = game.join("versions");

        let parent_dir = versions.join("1.20.1");
        fs::create_dir_all(&parent_dir).unwrap();
        fs::write(
            parent_dir.join("1.20.1.json"),
            serde_json::to_string(&vanilla_json("1.20.1", 17)).unwrap(),
        )
        .unwrap();
        write_jar(&parent_dir.join("1.20.1.jar"), 61);

        let pack_id = "society";
        let pack_dir = versions.join(pack_id);
        fs::create_dir_all(&pack_dir).unwrap();
        fs::write(
            pack_dir.join(format!("{}.json", pack_id)),
            serde_json::to_string(&serde_json::json!({
                "id": pack_id,
                "inheritsFrom": "1.20.1",
                "javaVersion": { "majorVersion": 21 }
            }))
            .unwrap(),
        )
        .unwrap();

        assert_eq!(measure_client_java(&game, pack_id), Some(17));

        // 实例目录自己带 jar 时与父版本取更高者，防止漏掉加载器自带的客户端
        write_jar(&pack_dir.join(format!("{}.jar", pack_id)), 65);
        assert_eq!(measure_client_java(&game, pack_id), Some(21));
    }

    #[test]
    fn test_analyze_uses_the_measured_jar_floor() {
        let tmp = tempfile::tempdir().unwrap();
        let game = tmp.path().to_path_buf();
        let pack_id = "society";
        let pack_dir = game.join("versions").join(pack_id);
        fs::create_dir_all(&pack_dir).unwrap();
        fs::write(
            pack_dir.join(format!("{}.json", pack_id)),
            serde_json::to_string(&serde_json::json!({
                "id": pack_id,
                "javaVersion": { "majorVersion": 21 }
            }))
            .unwrap(),
        )
        .unwrap();
        write_jar(&pack_dir.join(format!("{}.jar", pack_id)), 61);

        let profile = analyze(pack_id, &game);
        assert_eq!(profile.requirement.target, 21);
        assert_eq!(profile.requirement.min, 17);
    }

    #[test]
    fn test_list_mod_files_keeps_only_enabled_jars() {
        let dir = tempfile::TempDir::new().unwrap();
        for name in ["a.jar", "b.jar.disabled", "notes.txt", "C.JAR"] {
            fs::write(dir.path().join(name), b"x").unwrap();
        }

        let mut files = list_mod_files(dir.path());
        files.sort();
        assert_eq!(files, ["C.JAR", "a.jar"]);
    }

    /// 目录位置只看设置：关闭隔离后游戏读根目录，实例目录里残留的模组不该被算进来
    #[test]
    fn test_mods_dir_follows_isolation_setting() {
        let dir = tempfile::TempDir::new().unwrap();
        let shared = dir.path().join("mods");
        let isolated = dir.path().join("versions").join("pack").join("mods");

        assert_eq!(mods_dir(dir.path(), "pack", true), isolated);
        assert_eq!(mods_dir(dir.path(), "pack", false), shared);

        // 两个目录都建出来也不改变判定
        fs::create_dir_all(&shared).unwrap();
        fs::create_dir_all(&isolated).unwrap();
        assert_eq!(mods_dir(dir.path(), "pack", true), isolated);
        assert_eq!(mods_dir(dir.path(), "pack", false), shared);
    }

    fn vanilla_json(mc: &str, java: u64) -> Value {
        serde_json::json!({ "id": mc, "javaVersion": { "majorVersion": java } })
    }

    /// 1.12 及更早那代 Forge：libraries 里留着 LaunchWrapper（实测 1.8.9 installer 的 install_profile.json）
    fn legacy_forge_json(mc: &str) -> Value {
        serde_json::json!({
            "id": format!("{}-forge-14.23.5.2860", mc),
            "inheritsFrom": mc,
            "libraries": [
                { "name": "net.minecraftforge:forge:1.12.2-14.23.5.2860" },
                { "name": "net.minecraft.launchwrapper:launchwrapper:1.12" }
            ]
        })
    }

    /// 1.13 之后 Forge 改用 modlauncher（实测 1.16.5 installer 的 version.json）
    fn modern_forge_json(mc: &str) -> Value {
        serde_json::json!({
            "id": format!("{}-forge-36.2.39", mc),
            "inheritsFrom": mc,
            "libraries": [
                { "name": "net.minecraftforge:forge:1.16.5-36.2.39" },
                { "name": "cpw.mods:modlauncher:8.1.3" }
            ]
        })
    }

    #[test]
    fn test_class_file_major_maps_to_java() {
        assert_eq!(class_file_to_java(52), Some(8));
        assert_eq!(class_file_to_java(55), Some(11));
        assert_eq!(class_file_to_java(61), Some(17));
        assert_eq!(class_file_to_java(65), Some(21));
        assert_eq!(class_file_to_java(45), None);
    }

    #[test]
    fn test_vanilla_declares_no_upper_bound() {
        let req = build_requirement(Some(8), LoaderKind::Vanilla, &[]);
        assert_eq!((req.min, req.target), (8, 8));
        assert_eq!(req.hard_max, None);
        assert!(java_allowed(&req, 8));
        // 原版 1.12.2 没用到旧 Forge 的那些依赖，新 JDK 能起
        assert!(java_allowed(&req, 17));
        assert!(java_allowed(&req, 26));
    }

    /// 那一代 modlauncher 在 Java 9+ 上必崩：1.13.2-Forge 用 Java 25 会在 1 秒内挂
    #[test]
    fn test_forge_era_caps_the_jdk() {
        let r =
            build_requirement_with_fallback(Some(8), None, LoaderKind::Forge, Some((1, 13)), &[]);
        assert_eq!(r.hard_max, Some(8));
        assert!(java_allowed(&r, 8));
        assert!(!java_allowed(&r, 25));

        // 1.16 的官方范围是 8 与 11
        let r =
            build_requirement_with_fallback(Some(8), None, LoaderKind::Forge, Some((1, 16)), &[]);
        assert_eq!(r.hard_max, Some(11));
        assert!(java_allowed(&r, 11));
        assert!(!java_allowed(&r, 17));

        // 1.17 起没有「确定不兼容」的组合，不设上限
        let r =
            build_requirement_with_fallback(Some(17), None, LoaderKind::Forge, Some((1, 20)), &[]);
        assert_eq!(r.hard_max, None);
        assert!(java_allowed(&r, 21));

        // 原版不受加载器的世代限制影响
        let r =
            build_requirement_with_fallback(Some(8), None, LoaderKind::Vanilla, Some((1, 13)), &[]);
        assert_eq!(r.hard_max, None);
        assert!(java_allowed(&r, 17));
    }

    #[test]
    fn test_legacy_forge_caps_and_modern_forge_does_not() {
        let legacy = legacy_forge_json("1.12.2");
        assert_eq!(
            detect_loader(&legacy, "1.12.2-forge-14.23.5.2860"),
            LoaderKind::ForgeLegacy
        );
        let legacy_req = build_requirement(Some(8), LoaderKind::ForgeLegacy, &[]);
        assert_eq!(legacy_req.hard_max, Some(8));
        assert!(java_allowed(&legacy_req, 8));
        // 搜索结果里 1.12.2 Forge 的兼容范围就是 Java 7-8，11 一样起不来
        assert!(!java_allowed(&legacy_req, 11));
        assert!(!java_allowed(&legacy_req, 17));

        // modlauncher 时代的 Forge 能往上走：1.16.5 官方支持 Java 8 与 11
        let modern = modern_forge_json("1.16.5");
        assert_eq!(
            detect_loader(&modern, "1.16.5-forge-36.2.39"),
            LoaderKind::Forge
        );
        let modern_req = build_requirement(Some(8), LoaderKind::Forge, &[]);
        assert_eq!(modern_req.hard_max, None);
        assert!(java_allowed(&modern_req, 8));
        assert!(java_allowed(&modern_req, 11));

        // NeoForge 的声明是下限，较新的 JDK 不该被拦，但低于声明仍然不行
        let neoforge = build_requirement(Some(21), LoaderKind::NeoForge, &[]);
        assert_eq!(neoforge.hard_max, None);
        assert!(java_allowed(&neoforge, 25));
        assert!(!java_allowed(&neoforge, 17));
    }

    #[test]
    fn test_legacy_forge_is_also_detected_from_main_class() {
        // libraries 被裁过的整合包：mainClass 本身就是 LaunchWrapper 的入口
        let json = serde_json::json!({
            "id": "1.8-forge-11.14.4.1577",
            "mainClass": "net.minecraft.launchwrapper.Launch",
            "libraries": [{ "name": "net.minecraftforge:forge:1.8-11.14.4.1577" }]
        });
        assert_eq!(
            detect_loader(&json, "1.8-forge-11.14.4.1577"),
            LoaderKind::ForgeLegacy
        );
    }

    #[test]
    fn test_mod_class_version_raises_the_floor() {
        let mods = vec![probe("newmod-1.0.jar", 21), probe("oldmod-1.0.jar", 8)];
        let req = build_requirement(Some(8), LoaderKind::Vanilla, &mods);

        assert_eq!(req.min, 21);
        assert_eq!(req.target, 8);
        assert!(!java_allowed(&req, 17));
        assert!(java_allowed(&req, 21));
        assert!(req.reasons.iter().any(|r| r.contains("newmod-1.0.jar")));
    }

    #[test]
    fn test_forge_with_newer_mod_is_unsatisfiable() {
        let mods = vec![probe("oops.jar", 21)];
        let req = build_requirement(Some(8), LoaderKind::ForgeLegacy, &mods);

        assert!(req.unsatisfiable());
        assert!(!java_allowed(&req, 8));
        assert!(!java_allowed(&req, 21));

        let profile = InstanceProfile {
            requirement: req,
            loader: LoaderKind::ForgeLegacy,
            mods,
            declared_java: Some(8),
            resolved: true,
            version_id: "1.8.9-forge-14.23.5.2860".to_string(),
            game_dir: PathBuf::from("/tmp"),
        };
        let report = build_report(
            &profile,
            Some(("C:/jdk-26/bin/java.exe", 26)),
            &[cand("C:/jdk-26/bin/java.exe", 26)],
        );
        assert!(report.contains("无法同时满足"), "{}", report);
        assert!(report.contains("Java 26 [不行]"), "{}", report);
    }

    #[test]
    fn test_missing_declaration_falls_back_without_crashing() {
        let req = build_requirement(None, LoaderKind::Vanilla, &[]);
        assert_eq!(req.min, FALLBACK_JAVA);
        assert!(req.reasons.iter().any(|r| r.contains("兜底")));
    }

    #[test]
    fn test_fabric_and_quilt_are_detected_and_uncapped() {
        let fabric =
            serde_json::json!({ "libraries": [{ "name": "net.fabricmc:fabric-loader:0.15.0" }] });
        // Quilt 的真实 profile 一定同时带 net.fabricmc 的库（mixin/tiny-remapper），
        // 所以只认 org.quiltmc 而不先判 fabric 是不行的
        let quilt = serde_json::json!({
            "libraries": [
                { "name": "net.fabricmc:sponge-mixin:0.12.5+mixin.0.8.5" },
                { "name": "net.fabricmc:tiny-remapper:0.8.6" },
                { "name": "org.quiltmc:quilt-loader:0.30.1" }
            ]
        });
        assert_eq!(detect_loader(&fabric, ""), LoaderKind::Fabric);
        assert_eq!(detect_loader(&quilt, ""), LoaderKind::Quilt);
        assert_eq!(
            build_requirement(Some(17), LoaderKind::Fabric, &[]).hard_max,
            None
        );
    }

    /// 1.13.2-Forge 撞上 Java 25 时，报告要直接说“去装一份 Java 8”，而不是只列一堆 [不行]
    #[test]
    fn test_report_tells_you_to_install_java_when_none_is_allowed() {
        let req =
            build_requirement_with_fallback(Some(8), None, LoaderKind::Forge, Some((1, 13)), &[]);
        let profile = InstanceProfile {
            requirement: req,
            loader: LoaderKind::Forge,
            mods: Vec::new(),
            declared_java: Some(8),
            resolved: true,
            version_id: "1.13.2-Forge".to_string(),
            game_dir: PathBuf::from("/tmp"),
        };

        let report = build_report(
            &profile,
            Some(("D:/jdk25/bin/java.exe", 25)),
            &[cand("D:/jdk25/bin/java.exe", 25)],
        );

        assert!(report.contains("先在设置里补一份"), "{}", report);
        assert!(report.contains("Java 8"), "{}", report);
    }

    #[test]
    fn test_report_lists_mods_and_candidates() {
        let json = vanilla_json("1.21.4", 21);
        assert_eq!(declared_major(&json), Some(21));

        let req = build_requirement(Some(21), LoaderKind::Vanilla, &[]);
        let profile = InstanceProfile {
            requirement: req,
            loader: LoaderKind::Vanilla,
            mods: vec![probe("a.jar", 21)],
            declared_java: Some(21),
            resolved: true,
            version_id: "1.21.4".to_string(),
            game_dir: PathBuf::from("/tmp"),
        };
        let candidates = vec![
            cand("C:/java-17/bin/java.exe", 17),
            cand("C:/java-21/bin/java.exe", 21),
        ];
        let report = build_report(&profile, Some(("C:/java-17/bin/java.exe", 17)), &candidates);

        assert!(report.contains("a.jar → Java 21"));
        assert!(report.contains("Java 17 [不行]"));
        assert!(report.contains("Java 21 [可用]"));
    }

    #[test]
    fn test_disabled_mods_are_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        let mods = tmp.path().to_path_buf();
        write_jar(&mods.join("live.jar"), 65);
        write_jar(&mods.join("dead.jar.disabled"), 65);

        let probes = scan_mods(&mods);
        assert_eq!(probes.len(), 1, "{:?}", probes);
        assert_eq!(probes[0].file, "live.jar");
    }

    fn probe(file: &str, java: u32) -> ModProbe {
        ModProbe {
            file: file.to_string(),
            java,
            highest: java,
            highest_count: 4,
            sampled: 4,
        }
    }

    fn write_jar(path: &Path, class_major: u16) {
        use zip::write::SimpleFileOptions;
        let file = fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        zip.start_file("com/example/A.class", SimpleFileOptions::default())
            .unwrap();
        std::io::Write::write_all(
            &mut zip,
            &[
                0xCA,
                0xFE,
                0xBA,
                0xBE,
                0,
                0,
                (class_major >> 8) as u8,
                class_major as u8,
            ],
        )
        .unwrap();
        zip.finish().unwrap();
    }

    fn write_jar_entries<S: AsRef<str>>(path: &Path, entries: &[(S, u16)]) {
        use zip::write::SimpleFileOptions;
        let file = fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);

        for (name, class_major) in entries {
            let major = *class_major;
            zip.start_file((*name).as_ref(), SimpleFileOptions::default())
                .unwrap();
            std::io::Write::write_all(
                &mut zip,
                &[
                    0xCA,
                    0xFE,
                    0xBA,
                    0xBE,
                    0,
                    0,
                    (major >> 8) as u8,
                    major as u8,
                ],
            )
            .unwrap();
        }

        zip.finish().unwrap();
    }

    #[test]
    fn test_jar_java_version_reads_first_class() {
        let tmp = tempfile::tempdir().unwrap();
        let jar = tmp.path().join("x.jar");
        write_jar(&jar, 61);
        assert_eq!(jar_java_version(&jar), Some(17));
    }

    #[test]
    fn test_analyze_walks_inherits_from_for_declaration() {
        let tmp = tempfile::tempdir().unwrap();
        let game = tmp.path().to_path_buf();
        let versions = game.join("versions");

        // 原版 JSON 带声明，Forge 实例 JSON 不带
        let parent_dir = versions.join("1.8.9");
        fs::create_dir_all(&parent_dir).unwrap();
        fs::write(
            parent_dir.join("1.8.9.json"),
            serde_json::to_string(&vanilla_json("1.8.9", 8)).unwrap(),
        )
        .unwrap();

        let child_id = "1.8.9-forge-11.15.1.2318";
        let child_dir = versions.join(child_id);
        fs::create_dir_all(&child_dir).unwrap();
        fs::write(
            child_dir.join(format!("{}.json", child_id)),
            serde_json::to_string(&legacy_forge_json("1.8.9")).unwrap(),
        )
        .unwrap();

        let profile = analyze(child_id, &game);
        assert_eq!(profile.loader, LoaderKind::ForgeLegacy);
        assert!(profile.resolved);
        assert_eq!(profile.requirement.target, 8);
        assert_eq!(profile.requirement.hard_max, Some(8));
    }
}
