//! 内存测算与 JVM 参数模块
//!
//! 按系统可用内存、版本、加载器与模组规模计算启动堆大小，产出 -Xmx/-Xms、GC 策略与 Metaspace 参数

use crate::errors::LauncherError;
use crate::models::{GcStrategy, JvmAdvancedOptions};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use sysinfo::{MemoryRefreshKind, System};

static MEMORY_SYSTEM: std::sync::LazyLock<Mutex<System>> =
    std::sync::LazyLock::new(|| Mutex::new(System::new()));

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryStats {
    pub total_memory_mb: u64,
    pub used_memory_mb: u64,
    pub available_memory_mb: u64,
    pub memory_usage_percent: f64,
}

pub const AUTO_MEMORY_MIN_MB: u32 = 2048;
pub const AUTO_MEMORY_MAX_MB: u32 = 16384;

const AUTO_MEMORY_DEFAULT_TARGET_MB: u32 = 4096;
const AUTO_MEMORY_AVAILABLE_PERCENT: u32 = 75;
const AUTO_MEMORY_32BIT_LIMIT_MB: u32 = 1536;
const AUTO_MEMORY_STEP_MB: u32 = 256;

#[derive(Debug, Clone)]
pub struct AutoMemoryPlan {
    pub memory_mb: u32,
    /// 按实例规模算出的需求，不含默认目标
    pub need_mb: u32,
    /// 本次实际想要分配的值：max(需求, 4GB)
    pub target_mb: u32,
    pub cap_mb: u32,
    pub base_mb: u32,
    pub loader_mb: u32,
    pub mods_mb: u32,
    pub mod_count: usize,
    pub available_memory_mb: u32,
    pub is_32bit: bool,
    pub clamped: bool,
}

impl AutoMemoryPlan {
    pub fn describe(&self, version: &str, loader: &str) -> String {
        let mut text = format!(
            "自动内存 {} MB（{} / {}：版本基准 {} + 加载器 {} + {} 个模组 {} = 需求 {}，目标 {}；可用 {} MB，按 {}% 上限 {} MB",
            self.memory_mb,
            version,
            loader,
            self.base_mb,
            self.loader_mb,
            self.mod_count,
            self.mods_mb,
            self.need_mb,
            self.target_mb,
            self.available_memory_mb,
            AUTO_MEMORY_AVAILABLE_PERCENT,
            self.cap_mb
        );
        if self.clamped {
            text.push_str("，已按上限压缩");
        }
        if self.is_32bit {
            text.push_str("，32 位 Java");
        }
        text.push('）');
        text
    }
}

pub fn get_system_memory() -> MemoryStats {
    let mut system = MEMORY_SYSTEM.lock().unwrap();
    system.refresh_memory_specifics(MemoryRefreshKind::nothing().with_ram());

    let total_memory_mb = system.total_memory() / 1024 / 1024;
    let used_memory_mb = system.used_memory() / 1024 / 1024;
    let available_memory_mb = system.available_memory() / 1024 / 1024;
    let memory_usage_percent = (used_memory_mb as f64 / total_memory_mb as f64) * 100.0;

    MemoryStats {
        total_memory_mb,
        used_memory_mb,
        available_memory_mb,
        memory_usage_percent,
    }
}

/// 每次启动时按系统当前可用内存和实例规模计算堆大小
///
/// 默认目标 4GB；大包按需求抬高。上限取可用内存的 75%，并限制在 2GB~16GB
pub fn plan_auto_memory(
    version: &str,
    modded: bool,
    mod_count: usize,
    is_32bit: bool,
) -> AutoMemoryPlan {
    let stats = get_system_memory();
    compute_auto_memory(
        version,
        modded,
        mod_count,
        stats.available_memory_mb as u32,
        is_32bit,
    )
}

pub fn compute_auto_memory(
    version: &str,
    modded: bool,
    mod_count: usize,
    available_memory_mb: u32,
    is_32bit: bool,
) -> AutoMemoryPlan {
    let base_mb = version_base_mb(version);
    let loader_mb = if modded { 512 } else { 0 };
    let mods_mb = mods_extra_mb(mod_count);
    let need_mb = base_mb + loader_mb + mods_mb;
    let target_mb = need_mb.max(AUTO_MEMORY_DEFAULT_TARGET_MB);

    let available_cap = available_memory_mb.saturating_mul(AUTO_MEMORY_AVAILABLE_PERCENT) / 100;
    let mut cap_mb = available_cap.clamp(AUTO_MEMORY_MIN_MB, AUTO_MEMORY_MAX_MB);
    // 32 位进程的地址空间是硬限制，无法满足 2GB 下限
    if is_32bit {
        cap_mb = cap_mb.min(AUTO_MEMORY_32BIT_LIMIT_MB);
    }

    let clamped = target_mb > cap_mb;
    let floor_mb = if is_32bit {
        AUTO_MEMORY_32BIT_LIMIT_MB
    } else {
        AUTO_MEMORY_MIN_MB
    };
    let memory_mb = target_mb.min(cap_mb).max(floor_mb) / AUTO_MEMORY_STEP_MB * AUTO_MEMORY_STEP_MB;

    AutoMemoryPlan {
        memory_mb,
        need_mb,
        target_mb,
        cap_mb,
        base_mb,
        loader_mb,
        mods_mb,
        mod_count,
        available_memory_mb,
        is_32bit,
        clamped,
    }
}

fn version_base_mb(version: &str) -> u32 {
    if crate::utils::minecraft::is_new_version(version) {
        2048
    } else if crate::utils::minecraft::is_mid_version(version) {
        1536
    } else if crate::utils::minecraft::parse_mc_version(version).is_some() {
        1024
    } else {
        // 版本号认不出时宁可多给，不要一启动就 OOM
        2048
    }
}

fn mods_extra_mb(count: usize) -> u32 {
    match count {
        0 => 0,
        1..=30 => 512,
        31..=80 => 1024,
        81..=150 => 2048,
        151..=250 => 3072,
        _ => 4096,
    }
}

/// 默认仅追加 G1GC 调优，其余维持官方启动器的最小参数集
///
/// 更多调优项由 `JvmAdvancedOptions` 开关控制
pub fn optimize_jvm_memory_args(
    memory_mb: u32,
    java_version: Option<u32>,
    advanced: &JvmAdvancedOptions,
) -> Vec<String> {
    let mut args = Vec::new();

    let cpu_cores = std::thread::available_parallelism()
        .map(|count| count.get() as u32)
        .unwrap_or(4);
    // GC 线程数：留 1-2 核给游戏主线程和系统
    let gc_threads = if cpu_cores <= 2 {
        1
    } else {
        (cpu_cores - 1).min(4)
    };

    // Xms 与 Xmx 相等，避免堆动态扩缩的开销
    args.push(format!("-Xmx{}M", memory_mb));
    args.push(format!("-Xms{}M", memory_mb));

    let java_major = java_version.unwrap_or(17);

    // 新版 Java 下 LWJGL/JNA/JOML 正常工作的前提，不属于调优项
    if java_major >= 17 {
        args.push("--enable-native-access=ALL-UNNAMED".to_string());
    }
    if java_major >= 23 {
        args.push("--sun-misc-unsafe-memory-access=allow".to_string());
    }

    match advanced.gc_strategy {
        GcStrategy::G1 => {
            args.push("-XX:+UseG1GC".to_string());
            args.push("-XX:G1HeapRegionSize=32M".to_string());
            args.push("-XX:G1NewSizePercent=20".to_string());
            args.push("-XX:G1ReservePercent=20".to_string());
            args.push("-XX:MaxGCPauseMillis=50".to_string());
        }
        GcStrategy::Parallel => {
            args.push("-XX:+UseParallelGC".to_string());
        }
        GcStrategy::Serial => {
            args.push("-XX:+UseSerialGC".to_string());
        }
        GcStrategy::Z => {
            // ZGC 需要 Java 15+，旧版本只能回退到 G1，否则 JVM 根本起不来
            if java_major < 15 {
                args.push("-XX:+UseG1GC".to_string());
            } else {
                args.push("-XX:+UseZGC".to_string());
                // 分代 ZGC：Java 21~23 需显式开启，Java 24+ 默认分代且该标志已移除
                if (21..24).contains(&java_major) {
                    args.push("-XX:+ZGenerational".to_string());
                }
            }
        }
        // 不加参数，行为等同官方启动器
        GcStrategy::Default => {}
    }

    if advanced.always_pre_touch {
        args.push("-XX:+AlwaysPreTouch".to_string());
    }

    if advanced.disable_explicit_gc {
        args.push("-XX:+DisableExplicitGC".to_string());
    }

    if advanced.string_deduplication {
        args.push("-XX:+UseStringDeduplication".to_string());
    }

    if advanced.perf_disable_shared_mem {
        args.push("-XX:+PerfDisableSharedMem".to_string());
    }

    // Metaspace 上限默认留空：不设上限会持续索取本地内存拖垮系统，限太紧又触发 Metaspace OOM
    let metaspace_initial = advanced.metaspace_size_mb.filter(|mb| *mb > 0);
    let metaspace_max = advanced.metaspace_max_size_mb.filter(|mb| *mb > 0);

    match (metaspace_initial, metaspace_max) {
        (Some(initial), Some(max)) => {
            // 初始水位不能高于上限，否则参数自相矛盾
            args.push(format!("-XX:MetaspaceSize={}m", initial.min(max)));
            args.push(format!("-XX:MaxMetaspaceSize={}m", max));
        }
        (Some(initial), None) => {
            args.push(format!("-XX:MetaspaceSize={}m", initial));
        }
        (None, Some(max)) => {
            // 只填上限时初始值对齐上限，避免加载期反复触发带 Full GC 的类卸载
            args.push(format!("-XX:MetaspaceSize={}m", max));
            args.push(format!("-XX:MaxMetaspaceSize={}m", max));
        }
        (None, None) => {}
    }

    if advanced.heap_dump_on_oom {
        args.push("-XX:+HeapDumpOnOutOfMemoryError".to_string());
        args.push("-XX:HeapDumpPath=./logs/heapdump.hprof".to_string());
    }

    // JIT 编译器线程数，加速启动时的代码编译
    args.push(format!("-XX:CICompilerCount={}", gc_threads.min(4)));

    args.extend(parse_extra_args(&advanced.extra_args));

    // 兜底：自定义参数可能带实验性选项，统一补解锁开关防止 JVM 拒绝启动
    ensure_experimental_unlock(&mut args);

    args
}

const UNLOCK_EXPERIMENTAL: &str = "-XX:+UnlockExperimentalVMOptions";

/// 解锁开关必须先于实验性选项，否则 JVM 直接拒绝创建虚拟机
fn ensure_experimental_unlock(args: &mut Vec<String>) {
    if !args.iter().any(|a| a.starts_with("-XX:")) {
        return;
    }

    // 旧位置的开关（多来自自定义参数）同样不合法，先删再插到最前
    args.retain(|a| a != UNLOCK_EXPERIMENTAL);
    args.insert(0, UNLOCK_EXPERIMENTAL.to_string());
}

/// 解析用户自定义 JVM 参数：支持换行或空格分隔，`#` 开头视为注释
fn parse_extra_args(raw: &str) -> Vec<String> {
    let mut result = Vec::new();

    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        for token in line.split_whitespace() {
            result.push(token.to_string());
        }
    }

    result
}

/// 仅校验最低 512MB，不检查上限
pub fn is_memory_setting_safe(requested_memory_mb: u32) -> Result<bool, LauncherError> {
    if requested_memory_mb < 512 {
        return Err(LauncherError::Custom(
            "内存设置过低！Minecraft 至少需要 512MB 内存。".to_string(),
        ));
    }

    Ok(true)
}

pub fn get_memory_warning_message(requested_memory_mb: u32) -> Option<String> {
    let memory_stats = get_system_memory();
    let warning_limit = (memory_stats.total_memory_mb as f32 * 0.9) as u32;

    if requested_memory_mb > warning_limit {
        Some(format!(
            "警告：内存设置 {}MB 超过系统总内存 {}MB 的90%。这可能导致系统不稳定。",
            requested_memory_mb, memory_stats.total_memory_mb
        ))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_auto_memory_need_and_default_target() {
        let vanilla_new = compute_auto_memory("1.20.1", false, 0, 32768, false);
        assert_eq!(vanilla_new.need_mb, 2048);
        assert_eq!(vanilla_new.target_mb, 4096);
        assert_eq!(vanilla_new.memory_mb, 4096);

        let modded_mid = compute_auto_memory("1.16.5", true, 0, 32768, false);
        assert_eq!(modded_mid.need_mb, 2048);

        let old = compute_auto_memory("1.7.10", false, 0, 32768, false);
        assert_eq!(old.need_mb, 1024);
        assert_eq!(old.memory_mb, 4096);

        let small_pack = compute_auto_memory("1.20.1-Forge", true, 30, 32768, false);
        assert_eq!(small_pack.need_mb, 3072);
        assert_eq!(small_pack.memory_mb, 4096);

        let large_pack = compute_auto_memory("1.20.1-Forge", true, 300, 32768, false);
        assert_eq!(large_pack.need_mb, 6656);
        assert_eq!(large_pack.memory_mb, 6656);
    }

    #[test]
    fn test_auto_memory_cap_is_75_percent_of_available() {
        // 可用 8GB → 上限 6GB；大包需要 6.5GB，只能给 6GB
        let plan = compute_auto_memory("1.20.1-Forge", true, 300, 8192, false);
        assert_eq!(plan.cap_mb, 6144);
        assert_eq!(plan.memory_mb, 6144);
        assert!(plan.clamped);

        // 可用 4GB → 上限 3GB，低于默认目标 4GB
        let plan = compute_auto_memory("1.20.1", false, 0, 4096, false);
        assert_eq!(plan.cap_mb, 3072);
        assert_eq!(plan.memory_mb, 3072);
        assert!(plan.clamped);

        // 普通实例在内存充足时不会吃满 75%，仍按 4GB 目标
        let plan = compute_auto_memory("1.20.1", false, 0, 16384, false);
        assert_eq!(plan.cap_mb, 12288);
        assert_eq!(plan.memory_mb, 4096);
        assert!(!plan.clamped);
    }

    #[test]
    fn test_auto_memory_floor_and_ceiling() {
        // 可用内存很低也要满足 2GB 下限
        let plan = compute_auto_memory("1.20.1", false, 0, 1000, false);
        assert_eq!(plan.cap_mb, AUTO_MEMORY_MIN_MB);
        assert_eq!(plan.memory_mb, AUTO_MEMORY_MIN_MB);
        assert!(plan.clamped);

        // 再大的可用内存也只到 16GB
        let plan = compute_auto_memory("1.20.1-Forge", true, 300, 65536, false);
        assert_eq!(plan.cap_mb, AUTO_MEMORY_MAX_MB);
        assert_eq!(plan.memory_mb, 6656);
    }

    #[test]
    fn test_auto_memory_caps_32bit_java() {
        let plan = compute_auto_memory("1.20.1-Forge", true, 300, 32768, true);
        assert_eq!(plan.cap_mb, 1536);
        assert_eq!(plan.memory_mb, 1536);
        assert!(plan.clamped);
    }

    #[test]
    fn test_auto_memory_rounds_down_to_step() {
        // 可用 8800 → 75% = 6600，向下取整到 6400
        let plan = compute_auto_memory("1.20.1-Forge", true, 300, 8800, false);
        assert_eq!(plan.cap_mb, 6600);
        assert_eq!(plan.memory_mb, 6400);
        assert!(plan.clamped);
    }

    #[test]
    fn test_jvm_args_generation() {
        let args = optimize_jvm_memory_args(2048, Some(17), &JvmAdvancedOptions::default());
        assert!(args.iter().any(|arg| arg.contains("-Xmx2048M")));
        assert!(args.iter().any(|arg| arg.contains("-Xms2048M")));
    }

    #[test]
    fn test_jvm_args_default_has_only_g1gc() {
        let args = optimize_jvm_memory_args(2048, Some(17), &JvmAdvancedOptions::default());
        assert!(args.iter().any(|arg| arg == "-XX:+UseG1GC"));
        assert!(args.iter().any(|arg| arg == "-XX:G1HeapRegionSize=32M"));
        assert!(args.iter().any(|arg| arg == "-XX:G1NewSizePercent=20"));
        assert!(args.iter().any(|arg| arg == "-XX:G1ReservePercent=20"));
        assert!(args.iter().any(|arg| arg == "-XX:MaxGCPauseMillis=50"));
        // 实验性选项必须先解锁，否则 JVM 拒绝启动
        let unlock_pos = args
            .iter()
            .position(|arg| arg == "-XX:+UnlockExperimentalVMOptions")
            .expect("必须包含 UnlockExperimentalVMOptions");
        let g1_new_pos = args
            .iter()
            .position(|arg| arg == "-XX:G1NewSizePercent=20")
            .unwrap();
        assert!(unlock_pos < g1_new_pos, "解锁开关必须排在实验性选项之前");
        for unexpected in [
            "-XX:+AlwaysPreTouch",
            "-XX:+DisableExplicitGC",
            "-XX:+PerfDisableSharedMem",
            "-XX:+HeapDumpOnOutOfMemoryError",
            "-XX:+UseStringDeduplication",
            "-XX:+UseParallelGC",
            "-XX:+UseZGC",
        ] {
            assert!(!args.iter().any(|arg| arg == unexpected), "{}", unexpected);
        }
        assert!(!args.iter().any(|arg| arg.starts_with("-XX:MetaspaceSize")));
    }

    #[test]
    fn test_jvm_args_advanced_options_are_off_by_default() {
        let opts = JvmAdvancedOptions::default();
        assert_eq!(
            opts.gc_strategy,
            GcStrategy::G1,
            "默认应为 G1，与官方启动器一致"
        );
        assert!(!opts.always_pre_touch);
        assert!(!opts.disable_explicit_gc);
        assert!(!opts.string_deduplication);
        assert!(!opts.perf_disable_shared_mem);
        assert!(!opts.heap_dump_on_oom);
        assert!(opts.metaspace_size_mb.is_none());
        assert!(opts.metaspace_max_size_mb.is_none());
        assert!(opts.extra_args.is_empty());
    }

    #[test]
    fn test_jvm_args_advanced_flags_applied_when_enabled() {
        let opts = JvmAdvancedOptions {
            always_pre_touch: true,
            disable_explicit_gc: true,
            perf_disable_shared_mem: true,
            string_deduplication: true,
            heap_dump_on_oom: true,
            metaspace_size_mb: Some(256),
            metaspace_max_size_mb: Some(512),
            ..JvmAdvancedOptions::default()
        };
        let args = optimize_jvm_memory_args(4096, Some(17), &opts);
        assert!(args.iter().any(|arg| arg == "-XX:+AlwaysPreTouch"));
        assert!(args.iter().any(|arg| arg == "-XX:+DisableExplicitGC"));
        assert!(args.iter().any(|arg| arg == "-XX:+PerfDisableSharedMem"));
        assert!(args.iter().any(|arg| arg == "-XX:+UseStringDeduplication"));
        assert!(args.iter().any(|arg| arg == "-XX:MetaspaceSize=256m"));
        assert!(
            args.iter()
                .any(|arg| arg == "-XX:+HeapDumpOnOutOfMemoryError")
        );
    }

    #[test]
    fn test_jvm_args_gc_strategy_default_matches_official() {
        // Default 策略不加任何 GC 参数，与官方启动器一致
        let opts = JvmAdvancedOptions {
            gc_strategy: GcStrategy::Default,
            ..JvmAdvancedOptions::default()
        };
        let args = optimize_jvm_memory_args(4096, Some(17), &opts);
        for gc in [
            "-XX:+UseG1GC",
            "-XX:+UseParallelGC",
            "-XX:+UseSerialGC",
            "-XX:+UseZGC",
        ] {
            assert!(!args.iter().any(|arg| arg == gc), "{}", gc);
        }
        assert!(!args.iter().any(|arg| arg.starts_with("-XX:G1")));
    }

    #[test]
    fn test_jvm_args_gc_strategy_parallel_and_serial() {
        let parallel = JvmAdvancedOptions {
            gc_strategy: GcStrategy::Parallel,
            ..JvmAdvancedOptions::default()
        };
        let args = optimize_jvm_memory_args(2048, Some(17), &parallel);
        assert!(args.iter().any(|arg| arg == "-XX:+UseParallelGC"));
        assert!(!args.iter().any(|arg| arg == "-XX:+UseG1GC"));

        let serial = JvmAdvancedOptions {
            gc_strategy: GcStrategy::Serial,
            ..JvmAdvancedOptions::default()
        };
        let args = optimize_jvm_memory_args(2048, Some(17), &serial);
        assert!(args.iter().any(|arg| arg == "-XX:+UseSerialGC"));
        assert!(!args.iter().any(|arg| arg == "-XX:+UseG1GC"));
    }

    #[test]
    fn test_jvm_args_gc_strategy_z_selected_explicitly() {
        let opts = JvmAdvancedOptions {
            gc_strategy: GcStrategy::Z,
            ..JvmAdvancedOptions::default()
        };
        let args = optimize_jvm_memory_args(8192, Some(21), &opts);
        assert!(args.iter().any(|arg| arg == "-XX:+UseZGC"));
        assert!(args.iter().any(|arg| arg == "-XX:+ZGenerational"));
        assert!(!args.iter().any(|arg| arg == "-XX:+UseG1GC"));
    }

    #[test]
    fn test_jvm_args_gc_strategy_z_falls_back_on_old_java() {
        // Java 8 无 ZGC，必须回退 G1 而不是启动失败
        let opts = JvmAdvancedOptions {
            gc_strategy: GcStrategy::Z,
            ..JvmAdvancedOptions::default()
        };
        let args = optimize_jvm_memory_args(4096, Some(8), &opts);
        assert!(!args.iter().any(|arg| arg == "-XX:+UseZGC"));
        assert!(args.iter().any(|arg| arg == "-XX:+UseG1GC"));
    }

    #[test]
    fn test_jvm_args_metaspace_initial_and_max() {
        let opts = JvmAdvancedOptions {
            metaspace_size_mb: Some(256),
            metaspace_max_size_mb: Some(512),
            ..JvmAdvancedOptions::default()
        };
        let args = optimize_jvm_memory_args(4096, Some(17), &opts);
        assert!(args.iter().any(|arg| arg == "-XX:MetaspaceSize=256m"));
        assert!(args.iter().any(|arg| arg == "-XX:MaxMetaspaceSize=512m"));
    }

    #[test]
    fn test_jvm_args_metaspace_max_only_sets_initial_too() {
        let opts = JvmAdvancedOptions {
            metaspace_max_size_mb: Some(768),
            ..JvmAdvancedOptions::default()
        };
        let args = optimize_jvm_memory_args(4096, Some(17), &opts);
        assert!(args.iter().any(|arg| arg == "-XX:MetaspaceSize=768m"));
        assert!(args.iter().any(|arg| arg == "-XX:MaxMetaspaceSize=768m"));
    }

    #[test]
    fn test_jvm_args_metaspace_initial_clamped_to_max() {
        let opts = JvmAdvancedOptions {
            metaspace_size_mb: Some(1024),
            metaspace_max_size_mb: Some(512),
            ..JvmAdvancedOptions::default()
        };
        let args = optimize_jvm_memory_args(4096, Some(17), &opts);
        assert!(args.iter().any(|arg| arg == "-XX:MetaspaceSize=512m"));
        assert!(!args.iter().any(|arg| arg == "-XX:MetaspaceSize=1024m"));
        assert!(args.iter().any(|arg| arg == "-XX:MaxMetaspaceSize=512m"));
    }

    #[test]
    fn test_jvm_args_metaspace_zero_values_ignored() {
        let opts = JvmAdvancedOptions {
            metaspace_size_mb: Some(0),
            metaspace_max_size_mb: Some(0),
            ..JvmAdvancedOptions::default()
        };
        let args = optimize_jvm_memory_args(4096, Some(17), &opts);
        assert!(!args.iter().any(|arg| arg.starts_with("-XX:MetaspaceSize")));
        assert!(
            !args
                .iter()
                .any(|arg| arg.starts_with("-XX:MaxMetaspaceSize"))
        );
    }

    #[test]
    fn test_jvm_args_custom_experimental_arg_gets_unlock_prepended() {
        let opts = JvmAdvancedOptions {
            gc_strategy: GcStrategy::Default,
            extra_args: "-XX:+UseZGC\n-XX:+ZGenerational".to_string(),
            ..JvmAdvancedOptions::default()
        };
        let args = optimize_jvm_memory_args(4096, Some(21), &opts);
        assert_eq!(args.first().unwrap(), "-XX:+UnlockExperimentalVMOptions");
        assert!(args.contains(&"-XX:+UseZGC".to_string()));
    }

    #[test]
    fn test_jvm_args_no_duplicate_unlock() {
        let opts = JvmAdvancedOptions {
            extra_args: "-XX:+UnlockExperimentalVMOptions\n-XX:+UseZGC".to_string(),
            ..JvmAdvancedOptions::default()
        };
        let args = optimize_jvm_memory_args(4096, Some(21), &opts);
        let count = args
            .iter()
            .filter(|a| *a == "-XX:+UnlockExperimentalVMOptions")
            .count();
        assert_eq!(count, 1, "解锁开关不应重复出现");
    }

    #[test]
    fn test_jvm_args_extra_args_parsed_and_appended() {
        let opts = JvmAdvancedOptions {
            extra_args:
                "# 我想试试 ZGC\n-XX:+UseZGC\n-XX:+ZGenerational\n\n  -Dfoo=bar -Dbar=baz \n"
                    .to_string(),
            ..JvmAdvancedOptions::default()
        };
        let args = optimize_jvm_memory_args(4096, Some(21), &opts);
        assert!(args.iter().any(|arg| arg == "-XX:+UseZGC"));
        assert!(args.iter().any(|arg| arg == "-XX:+ZGenerational"));
        assert!(args.iter().any(|arg| arg == "-Dfoo=bar"));
        assert!(args.iter().any(|arg| arg == "-Dbar=baz"));
        assert!(!args.iter().any(|arg| arg.starts_with('#')));
        // 自定义参数排在最后，便于覆盖前面的设置
        let last_index = args
            .iter()
            .rposition(|arg| arg == "-XX:+UseG1GC")
            .expect("G1GC 仍在启用");
        assert!(args[last_index..].contains(&"-XX:+UseZGC".to_string()));
    }

    #[test]
    fn test_jvm_args_enable_native_access_java17() {
        let args = optimize_jvm_memory_args(2048, Some(17), &JvmAdvancedOptions::default());
        assert!(
            args.iter()
                .any(|arg| arg == "--enable-native-access=ALL-UNNAMED")
        );
    }

    #[test]
    fn test_jvm_args_no_native_access_java8() {
        let args = optimize_jvm_memory_args(2048, Some(8), &JvmAdvancedOptions::default());
        assert!(
            !args
                .iter()
                .any(|arg| arg == "--enable-native-access=ALL-UNNAMED")
        );
    }

    #[test]
    fn test_jvm_args_unsafe_memory_access_java25() {
        // Java 23+ 需显式允许 Unsafe 内存访问
        let args = optimize_jvm_memory_args(2048, Some(25), &JvmAdvancedOptions::default());
        assert!(
            args.iter()
                .any(|arg| arg == "--sun-misc-unsafe-memory-access=allow")
        );
    }

    #[test]
    fn test_jvm_args_no_unsafe_memory_access_java21() {
        let args = optimize_jvm_memory_args(2048, Some(21), &JvmAdvancedOptions::default());
        assert!(
            !args
                .iter()
                .any(|arg| arg == "--sun-misc-unsafe-memory-access=allow")
        );
    }

    #[test]
    fn test_jvm_args_no_zgc_on_java8() {
        let args = optimize_jvm_memory_args(4096, Some(8), &JvmAdvancedOptions::default());
        assert!(!args.iter().any(|arg| arg == "-XX:+UseZGC"));
        assert!(args.iter().any(|arg| arg == "-XX:+UseG1GC"));
        assert!(
            !args
                .iter()
                .any(|arg| arg == "--enable-native-access=ALL-UNNAMED")
        );
    }

    #[test]
    fn test_memory_setting_safe() {
        assert!(is_memory_setting_safe(256).is_err());
        assert!(is_memory_setting_safe(511).is_err());
        assert!(is_memory_setting_safe(512).is_ok());
        assert!(is_memory_setting_safe(4096).is_ok());
    }
}
