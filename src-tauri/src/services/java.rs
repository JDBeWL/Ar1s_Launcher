//! Java 检测模块
//!
//! 扫描各发行版安装目录、PATH 与 JAVA_HOME；探测结果（大版本 + 进程位数）按可执行文件缓存，
//! 路径列表缓存 1 小时，避免每次启动都把本机每个 JVM 重启一遍问版本

use crate::{LauncherError, update_config_async};
use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{LazyLock, RwLock};
use std::time::{Duration, Instant};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

const JAVA_BIN: &str = if cfg!(windows) { "java.exe" } else { "java" };

struct JavaCache {
    paths: Vec<String>,
    cached_at: Instant,
}

static JAVA_CACHE: LazyLock<RwLock<Option<JavaCache>>> = LazyLock::new(|| RwLock::new(None));

const JAVA_CACHE_DURATION: Duration = Duration::from_secs(3600);

/// JVM 进程是 32 位还是 64 位：决定能开多大堆，同版本时优先 64 位
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JavaArch {
    Bits64,
    Bits32,
    Unknown,
}

impl JavaArch {
    /// 排序用，越小越优先
    pub fn rank(self) -> u8 {
        match self {
            JavaArch::Bits64 => 0,
            JavaArch::Unknown => 1,
            JavaArch::Bits32 => 2,
        }
    }
}

#[derive(Debug, Clone)]
pub struct JavaProbe {
    /// 大版本；解析不出来时仍是 None，但 arch/raw 可能有效
    pub version: Option<u32>,
    /// 发行版自己报的版本串，界面直接显示
    pub raw: Option<String>,
    pub arch: JavaArch,
    /// `java.home`：转发器（Oracle javapath）会把它指向真正的安装目录
    pub home: Option<String>,
}

impl JavaProbe {
    /// 预览版（`23-ea`、`24-ea+1`）：同版本时排在正式版之后
    pub fn is_ea(&self) -> bool {
        self.raw.as_deref().is_some_and(|raw| {
            let raw = raw.to_lowercase();
            raw.contains("-ea") || raw.contains("ea+") || raw.contains("-early")
        })
    }
}

struct CachedProbe {
    probe: JavaProbe,
    cached_at: Instant,
}

/// 一个 JVM 的启动成本在 Windows 上是百毫秒级，启动一次游戏要问好几轮，
/// 所以按可执行文件路径缓存，只有刷新扫描时才清空
static JAVA_PROBE_CACHE: LazyLock<RwLock<HashMap<String, CachedProbe>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

const JAVA_PROBE_DURATION: Duration = Duration::from_secs(6 * 3600);

/// 单次 `java -version` 探测的上限：正常 JVM 是百毫秒级，坏掉的 exe 不能把扫描拖死
const JAVA_PROBE_TIMEOUT: Duration = Duration::from_secs(10);
const JAVA_PROBE_POLL_INTERVAL: Duration = Duration::from_millis(20);

pub fn invalidate_java_cache() {
    if let Ok(mut cache) = JAVA_CACHE.write() {
        *cache = None;
    }
    if let Ok(mut probes) = JAVA_PROBE_CACHE.write() {
        probes.clear();
    }
    log::info!("Java 检测缓存已清除");
}

/// `1.8.0_432` → 8，`17.0.8` → 17，`24-ea` → 24，`21.0.5+11-LTS` → 21
pub fn parse_major(version: &str) -> Option<u32> {
    let head = version
        .split(|c: char| !(c.is_ascii_digit() || c == '.'))
        .find(|part| !part.is_empty())?;

    let mut parts = head.split('.');
    let first: u32 = parts.next()?.parse().ok()?;

    match first {
        // 1.8.0_x / 1.7.0_80 这种老写法，第二位才是大版本
        1 => parts.next()?.parse().ok(),
        // 9 起直接是大版本；0 与 1 一样是历史写法，不该出现在 JDK 9+
        2.. => Some(first),
        _ => None,
    }
}

fn parse_arch(value: &str) -> JavaArch {
    match value.trim().to_lowercase().as_str() {
        "amd64" | "x86_64" | "x64" | "aarch64" | "arm64" | "ppc64" | "ppc64le" | "s390x"
        | "riscv64" => JavaArch::Bits64,
        "x86" | "i386" | "i486" | "i586" | "i686" | "arm" | "ppc" => JavaArch::Bits32,
        _ => JavaArch::Unknown,
    }
}

/// `openjdk version "17.0.8" 2023-07-18` 里引号内那段
fn quoted_version(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        if !line.to_lowercase().contains("version") {
            return None;
        }
        // 这一行里没有引号就继续看下一行，别整段放弃
        let start = line.find('"')?;
        let end = line[start + 1..].find('"')?;
        Some(line[start + 1..start + 1 + end].to_string())
    })
}

fn run_capture(exe: &Path, args: &[&str]) -> Option<(String, String)> {
    run_capture_with_timeout(exe, args, JAVA_PROBE_TIMEOUT).ok()
}

enum CaptureFailure {
    Timeout,
    Spawn,
    Io,
}

/// 同步等子进程带超时：`Command::output()` 没有超时，异常 Java 能让探测永久卡住。
/// stdout/stderr 各用一个读线程，避免输出超过管道缓冲时子进程阻塞、被误判超时。
fn run_capture_with_timeout(
    exe: &Path,
    args: &[&str],
    timeout: Duration,
) -> Result<(String, String), CaptureFailure> {
    let mut command = Command::new(exe);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);

    let mut child = command.spawn().map_err(|_| CaptureFailure::Spawn)?;
    let stdout = child.stdout.take().ok_or(CaptureFailure::Spawn)?;
    let stderr = child.stderr.take().ok_or(CaptureFailure::Spawn)?;
    let stdout_reader = std::thread::spawn(move || read_pipe(stdout));
    let stderr_reader = std::thread::spawn(move || read_pipe(stderr));

    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                log::warn!(
                    "Java 探测超时（{} 秒），已终止: {}",
                    timeout.as_secs(),
                    exe.display()
                );
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(CaptureFailure::Timeout);
            }
            Ok(None) => std::thread::sleep(JAVA_PROBE_POLL_INTERVAL),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                log::warn!("等待 Java 探测进程失败: {}", error);
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(CaptureFailure::Io);
            }
        }
    }

    let stdout = stdout_reader.join().map_err(|_| CaptureFailure::Io)?;
    let stderr = stderr_reader.join().map_err(|_| CaptureFailure::Io)?;
    Ok((
        String::from_utf8_lossy(&stderr).into_owned(),
        String::from_utf8_lossy(&stdout).into_owned(),
    ))
}

fn read_pipe(mut pipe: impl Read) -> Vec<u8> {
    let mut bytes = Vec::new();
    let _ = pipe.read_to_end(&mut bytes);
    bytes
}

/// 从 `-XshowSettings:properties` 的输出里取一个键。
/// 必须是真的 `key = value`：`java.version.date` 也以 `java.version` 开头，不能认错
fn property_value(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let rest = line.trim().strip_prefix(key)?.trim_start();
        let value = rest.strip_prefix('=')?.trim();
        (!value.is_empty()).then(|| value.to_string())
    })
}

/// 一次调用同时拿 java.version、os.arch 与 java.home；老 JRE 不认 `-XshowSettings` 时退回 `-version`
fn probe_uncached(exe: &Path) -> Option<JavaProbe> {
    match run_capture_with_timeout(
        exe,
        &["-XshowSettings:properties", "-version"],
        JAVA_PROBE_TIMEOUT,
    ) {
        Ok((stderr, stdout)) => {
            let text = format!("{}\n{}", stderr, stdout);

            if let Some(raw) = property_value(&text, "java.version") {
                return Some(JavaProbe {
                    version: parse_major(&raw),
                    raw: Some(raw),
                    arch: property_value(&text, "os.arch")
                        .map(|value| parse_arch(&value))
                        .unwrap_or(JavaArch::Unknown),
                    home: property_value(&text, "java.home"),
                });
            }
        }
        // 超时的进程已经杀掉，换参数再来一轮只会再等一个超时
        Err(CaptureFailure::Timeout) => return None,
        Err(_) => {}
    }

    let (stderr, stdout) = match run_capture_with_timeout(exe, &["-version"], JAVA_PROBE_TIMEOUT) {
        Ok(output) => output,
        Err(CaptureFailure::Timeout) => return None,
        Err(_) => run_capture_with_timeout(
            exe,
            &["-XshowSettings:properties", "-version"],
            JAVA_PROBE_TIMEOUT,
        )
        .ok()?,
    };
    let text = format!("{}\n{}", stderr, stdout);
    let raw = quoted_version(&text)?;

    Some(JavaProbe {
        version: parse_major(&raw),
        raw: Some(raw),
        arch: JavaArch::Unknown,
        home: None,
    })
}

fn comparable(path: &Path) -> String {
    let mut text = path.to_string_lossy().replace('\\', "/").to_lowercase();
    if !text.ends_with('/') {
        text.push('/');
    }
    text
}

/// exe 是不是就装在这个 home 里（Java 8 的 JDK 会报 `<jdk>/jre`，父目录也算）
fn lives_in_home(exe: &Path, home: &Path) -> bool {
    let exe = comparable(exe);
    if exe.starts_with(&comparable(home)) {
        return true;
    }

    home.file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case("jre"))
        && home
            .parent()
            .is_some_and(|parent| exe.starts_with(&comparable(parent)))
}

/// Oracle 的 `javapath`/`java8path` 里那个 java.exe 只是个转发器（1.4MB，和真实那份不是同一个文件），
/// canonicalize 到不了同一条路径，于是同一个 JVM 会被列两遍。按它自己报的 java.home 换回真身
pub fn real_java_exe(exe: &Path, home: Option<&str>) -> PathBuf {
    let Some(home) = home else {
        return exe.to_path_buf();
    };
    let home = PathBuf::from(home);
    if lives_in_home(exe, &home) {
        return exe.to_path_buf();
    }

    let candidate = home.join("bin").join(JAVA_BIN);
    if candidate.is_file() {
        candidate
    } else {
        exe.to_path_buf()
    }
}

/// 探测并换回真身：转发器报的 java.home 才指向真正的安装
pub fn probe_java_resolved(path: &str) -> Option<(PathBuf, JavaProbe)> {
    let exe = resolve_java_exe(path);
    let probe = probe_java_exe(&exe)?;

    let real = real_java_exe(&exe, probe.home.as_deref());
    if real == exe {
        return Some((exe, probe));
    }

    // 换过去的那份必须自己也能跑，否则保留原路径
    match probe_java_exe(&real) {
        Some(probe) => Some((real, probe)),
        None => Some((exe, probe)),
    }
}

/// 路径 → 真正的 java 可执行文件：目录按 `<dir>/bin/java` 解释，相对路径落在启动器目录
pub fn resolve_java_exe(path: &str) -> PathBuf {
    let resolved = crate::services::launcher::java::resolve_path(path);
    let base = if resolved.exists() {
        resolved
    } else {
        PathBuf::from(path)
    };

    if base.is_dir() {
        base.join("bin").join(JAVA_BIN)
    } else if cfg!(windows) && base.extension().is_none() && !base.exists() {
        // 有人会把 `.../bin/java` 直接粘进来，Windows 上实际是 java.exe
        base.with_extension("exe")
    } else {
        base
    }
}

/// 去重与缓存共用的键：真实路径 + 小写。
/// 同一份 JVM 被目录、`bin/java.exe`、符号链接三种写法命中时必须落到同一个键
pub fn exe_key(exe: &Path) -> String {
    match exe.canonicalize() {
        Ok(real) => real.to_string_lossy().replace("\\", "/").to_lowercase(),
        Err(_) => exe.to_string_lossy().replace("\\", "/").to_lowercase(),
    }
}

/// 探测指定 Java（带缓存）；路径可以指向可执行文件、安装目录，或 PATH 上的 `java`
pub fn probe_java(path: &str) -> Option<JavaProbe> {
    probe_java_exe(&resolve_java_exe(path))
}

/// 探测缓存键：按“调用方给的那条路径”记，不做 canonicalize。
///
/// `java.home` 是跟着启动路径走的（同一个 JVM 经符号链接启动会报链接那边的 home），
/// 若按规范化路径共用缓存项，两条写法会互相污染，进而把真身又改回链接
fn probe_cache_key(exe: &Path) -> String {
    exe.to_string_lossy()
        .replace('\\', "/")
        .trim_start_matches("//?/")
        .to_lowercase()
}

/// 已知可执行文件路径时的入口，扫描时用它省掉一次路径解析
pub fn probe_java_exe(exe: &Path) -> Option<JavaProbe> {
    if !exe.is_file() {
        // `java` 这种纯命令名交给 PATH 去找，不做文件存在性判断
        if exe.components().count() != 1 {
            return None;
        }
    }

    let key = probe_cache_key(exe);

    if let Ok(cache) = JAVA_PROBE_CACHE.read()
        && let Some(hit) = cache.get(&key)
        && hit.cached_at.elapsed() < JAVA_PROBE_DURATION
    {
        return Some(hit.probe.clone());
    }

    let probe = probe_uncached(exe)?;

    if let Ok(mut cache) = JAVA_PROBE_CACHE.write() {
        cache.insert(
            key,
            CachedProbe {
                probe: probe.clone(),
                cached_at: Instant::now(),
            },
        );
    }

    Some(probe)
}

fn get_cached_java_paths() -> Option<Vec<String>> {
    if let Ok(cache) = JAVA_CACHE.read()
        && let Some(ref cached) = *cache
        && cached.cached_at.elapsed() < JAVA_CACHE_DURATION
    {
        log::debug!("使用缓存的 Java 路径列表 ({} 个)", cached.paths.len());
        return Some(cached.paths.clone());
    }
    None
}

fn update_java_cache(paths: Vec<String>) {
    if let Ok(mut cache) = JAVA_CACHE.write() {
        *cache = Some(JavaCache {
            paths: paths.clone(),
            cached_at: Instant::now(),
        });
        log::info!("Java 检测缓存已更新 ({} 个路径)", paths.len());
    }
}

fn normalize_path(path: &str) -> String {
    if cfg!(windows) {
        path.replace("/", "\\")
    } else {
        path.replace("\\", "/")
    }
}

/// 扫描阶段的“这个 exe 是不是 Java”：直接复用探测结果，
/// 一次启动就把版本和位数一起留下，后面选 Java 时不必再启一遍
fn is_valid_java_executable(java_path: &Path) -> bool {
    probe_java_exe(java_path).is_some_and(|probe| probe.version.is_some())
}

fn find_java_in_path(java_cmd: &str) -> bool {
    is_valid_java_executable(Path::new(java_cmd))
}

fn get_java_installation_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    #[cfg(target_os = "windows")]
    {
        let drives = get_windows_drives();

        for drive in &drives {
            let program_files = format!("{}:\\Program Files", drive);
            let program_files_x86 = format!("{}:\\Program Files (x86)", drive);

            dirs.push(PathBuf::from(&program_files).join("Java"));
            dirs.push(PathBuf::from(&program_files_x86).join("Java"));

            dirs.push(PathBuf::from(&program_files).join("Microsoft"));
            dirs.push(PathBuf::from(&program_files_x86).join("Microsoft"));

            dirs.push(PathBuf::from(&program_files).join("Eclipse Adoptium"));
            dirs.push(PathBuf::from(&program_files_x86).join("Eclipse Adoptium"));
            dirs.push(PathBuf::from(&program_files).join("Eclipse Foundation"));

            dirs.push(PathBuf::from(&program_files).join("AdoptOpenJDK"));
            dirs.push(PathBuf::from(&program_files_x86).join("AdoptOpenJDK"));

            dirs.push(PathBuf::from(&program_files).join("Amazon Corretto"));
            dirs.push(PathBuf::from(&program_files_x86).join("Amazon Corretto"));

            dirs.push(PathBuf::from(&program_files).join("Zulu"));
            dirs.push(PathBuf::from(&program_files_x86).join("Zulu"));
            dirs.push(PathBuf::from(&program_files).join("Azul Zulu"));

            dirs.push(PathBuf::from(&program_files).join("BellSoft"));
            dirs.push(PathBuf::from(&program_files_x86).join("BellSoft"));

            dirs.push(PathBuf::from(&program_files).join("RedHat"));
            dirs.push(PathBuf::from(&program_files_x86).join("RedHat"));

            dirs.push(PathBuf::from(&program_files).join("SapMachine"));

            dirs.push(PathBuf::from(&program_files).join("GraalVM"));

            dirs.push(PathBuf::from(&program_files).join("OpenJDK"));
            dirs.push(PathBuf::from(&program_files_x86).join("OpenJDK"));
        }

        if let Ok(user_profile) = std::env::var("USERPROFILE") {
            dirs.push(
                PathBuf::from(&user_profile)
                    .join("scoop")
                    .join("apps")
                    .join("openjdk"),
            );
            dirs.push(
                PathBuf::from(&user_profile)
                    .join("scoop")
                    .join("apps")
                    .join("temurin-lts-jdk"),
            );
            dirs.push(
                PathBuf::from(&user_profile)
                    .join("scoop")
                    .join("apps")
                    .join("temurin8-jdk"),
            );
            dirs.push(
                PathBuf::from(&user_profile)
                    .join("scoop")
                    .join("apps")
                    .join("zulu-jdk"),
            );
            dirs.push(
                PathBuf::from(&user_profile)
                    .join("scoop")
                    .join("apps")
                    .join("corretto-jdk"),
            );

            // .jdks：IntelliJ IDEA 下载的 JDK 存放处
            dirs.push(PathBuf::from(&user_profile).join(".jdks"));

            dirs.push(
                PathBuf::from(&user_profile)
                    .join(".sdkman")
                    .join("candidates")
                    .join("java"),
            );
        }

        if let Ok(pf) = std::env::var("ProgramFiles")
            && !dirs.iter().any(|d| d.starts_with(&pf))
        {
            dirs.push(PathBuf::from(&pf).join("Java"));
        }
        if let Ok(pf86) = std::env::var("ProgramFiles(x86)")
            && !dirs.iter().any(|d| d.starts_with(&pf86))
        {
            dirs.push(PathBuf::from(&pf86).join("Java"));
        }
    }

    #[cfg(target_os = "macos")]
    {
        dirs.push(PathBuf::from("/Library/Java/JavaVirtualMachines"));
        dirs.push(PathBuf::from("/System/Library/Java/JavaVirtualMachines"));
        dirs.push(PathBuf::from("/usr/local/Cellar/openjdk"));
        dirs.push(PathBuf::from("/opt/homebrew/Cellar/openjdk"));

        if let Ok(home) = std::env::var("HOME") {
            dirs.push(
                PathBuf::from(&home)
                    .join(".sdkman")
                    .join("candidates")
                    .join("java"),
            );
            dirs.push(PathBuf::from(&home).join(".jdks"));
        }
    }

    #[cfg(target_os = "linux")]
    {
        dirs.push(PathBuf::from("/usr/lib/jvm"));
        dirs.push(PathBuf::from("/usr/local/lib/jvm"));
        dirs.push(PathBuf::from("/opt/java"));
        dirs.push(PathBuf::from("/opt/jdk"));

        if let Ok(home) = std::env::var("HOME") {
            dirs.push(
                PathBuf::from(&home)
                    .join(".sdkman")
                    .join("candidates")
                    .join("java"),
            );
            dirs.push(PathBuf::from(&home).join(".jdks"));
        }
    }

    dirs.push(
        crate::services::config::init_storage()
            .join("runtime")
            .join("java"),
    );

    dirs.into_iter().filter(|dir| dir.exists()).collect()
}

/// 枚举有盘的盘符
///
/// 不能对 A..Z 逐个 `exists()`：空光驱或已断开的网络映射盘会让单次探测卡十几秒。
/// `GetLogicalDrives` 一次调用拿到全部盘符，再用 `GetDriveTypeW` 跳过不存在的和网络盘
#[cfg(target_os = "windows")]
fn get_windows_drives() -> Vec<char> {
    use windows_sys::Win32::Storage::FileSystem::{GetDriveTypeW, GetLogicalDrives};

    const DRIVE_REMOTE: u32 = 4;
    const DRIVE_NO_ROOT_DIR: u32 = 1;

    let mask = unsafe { GetLogicalDrives() };
    if mask == 0 {
        // 调用失败时退回逐个探测，至少还能找到 Java，只是慢
        return probe_drives_one_by_one();
    }

    let mut drives = Vec::new();
    for letter in b'A'..=b'Z' {
        if mask & (1 << (letter - b'A')) == 0 {
            continue;
        }
        let root: Vec<u16> = format!("{}:\\", letter as char)
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let drive_type = unsafe { GetDriveTypeW(root.as_ptr()) };
        if drive_type == DRIVE_NO_ROOT_DIR || drive_type == DRIVE_REMOTE {
            continue;
        }
        drives.push(letter as char);
    }
    drives
}

/// `GetLogicalDrives` 失败时的兜底；正常不会走到
#[cfg(target_os = "windows")]
fn probe_drives_one_by_one() -> Vec<char> {
    (b'A'..=b'Z')
        .map(|letter| letter as char)
        .filter(|letter| PathBuf::from(format!("{}:\\", letter)).exists())
        .collect()
}

/// 一个目录若直接就是 JDK/JRE 家目录，返回它的 java 可执行文件路径（只判存在，不验证）
fn java_home_exe(dir: &Path) -> Option<PathBuf> {
    let exe = dir
        .join("bin")
        .join(if cfg!(windows) { "java.exe" } else { "java" });

    exe.is_file().then_some(exe)
}

fn find_java_in_directory(dir: &Path) -> Vec<String> {
    let mut paths = Vec::new();

    // 根目录自己可能就是 JDK 家目录（有人把 JDK 直接装进 Program Files\Java）
    if let Some(exe) = java_home_exe(dir).filter(|exe| is_valid_java_executable(exe)) {
        paths.push(exe.to_string_lossy().replace("\\", "/"));
    }

    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            if entry.path().is_dir() {
                let dir_name = entry.file_name().to_string_lossy().to_lowercase();

                if is_java_directory_name(&dir_name) {
                    let java_exe = entry.path().join("bin").join(if cfg!(windows) {
                        "java.exe"
                    } else {
                        "java"
                    });

                    if java_exe.exists() && is_valid_java_executable(&java_exe) {
                        let path_str = java_exe.to_string_lossy().replace("\\", "/");
                        paths.push(path_str.to_owned());
                    } else {
                        // 某些发行版（如 Eclipse Adoptium）还多一层子目录
                        paths.extend(find_java_in_subdirectory(&entry.path()));
                    }
                } else if is_java_vendor_name(&dir_name) {
                    // 只有已知发行商的目录才值得再下钻一层：无条件递归会让 Program Files 下每个目录都 read_dir 一次
                    paths.extend(find_java_in_subdirectory(&entry.path()));
                }
            }
        }
    }

    paths
}

/// 目录名像发行商（而不是像 Java 版本），其下才可能藏着 JDK
fn is_java_vendor_name(name: &str) -> bool {
    [
        "microsoft",
        "eclipse",
        "adoptium",
        "adoptopenjdk",
        "amazon",
        "corretto",
        "zulu",
        "azul",
        "bellsoft",
        "liberica",
        "redhat",
        "sapmachine",
        "graalvm",
        "openjdk",
        "temurin",
        "semeru",
        "dragonwell",
        "bisheng",
        "ibm",
        "oracle",
        "java",
        "jdk",
        "jre",
    ]
    .iter()
    .any(|vendor| name.contains(vendor))
}

fn is_java_directory_name(name: &str) -> bool {
    name.contains("jdk")
        || name.contains("jre")
        || name.contains("java")
        || name.contains("openjdk")
        || name.contains("temurin")
        || name.contains("corretto")
        || name.contains("zulu")
        || name.contains("liberica")
        || name.contains("sapmachine")
        || name.contains("graalvm")
        || name.contains("semeru")
        || name.contains("dragonwell")
        || name.contains("bisheng")
}

fn find_java_in_subdirectory(dir: &Path) -> Vec<String> {
    let mut paths = Vec::new();

    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            if entry.path().is_dir() {
                let sub_dir_name = entry.file_name().to_string_lossy().to_lowercase();

                if is_java_directory_name(&sub_dir_name) {
                    let java_exe = entry.path().join("bin").join(if cfg!(windows) {
                        "java.exe"
                    } else {
                        "java"
                    });

                    if java_exe.exists() && is_valid_java_executable(&java_exe) {
                        let path_str = java_exe.to_string_lossy().replace("\\", "/");
                        paths.push(path_str.to_owned());
                    }

                    #[cfg(target_os = "macos")]
                    {
                        let macos_java = entry
                            .path()
                            .join("Contents")
                            .join("Home")
                            .join("bin")
                            .join("java");
                        if macos_java.exists() && is_valid_java_executable(&macos_java) {
                            let path_str = macos_java.to_string_lossy().replace("\\", "/");
                            paths.push(path_str.to_owned());
                        }
                    }
                }

                // scoop 用 current 目录指向实际安装的符号链接
                if sub_dir_name == "current" {
                    let java_exe = entry.path().join("bin").join(if cfg!(windows) {
                        "java.exe"
                    } else {
                        "java"
                    });

                    if java_exe.exists() && is_valid_java_executable(&java_exe) {
                        let path_str = java_exe.to_string_lossy().replace("\\", "/");
                        paths.push(path_str.to_owned());
                    }
                }
            }
        }
    }

    paths
}

/// 查找 Java 安装路径（带缓存，并行扫描）
pub async fn find_java_installations_command() -> Result<Vec<String>, LauncherError> {
    if let Some(cached_paths) = get_cached_java_paths() {
        return Ok(cached_paths);
    }

    log::info!("开始扫描 Java 安装路径...");
    let start_time = Instant::now();

    let paths = tokio::task::spawn_blocking(scan_java_installations_parallel)
        .await
        .map_err(|e| LauncherError::Custom(format!("Java 扫描任务失败: {}", e)))?;

    let elapsed = start_time.elapsed();
    log::info!(
        "Java 扫描完成，耗时 {:?}，找到 {} 个安装",
        elapsed,
        paths.len()
    );

    update_java_cache(paths.clone());

    Ok(paths)
}

pub fn find_java_installations_command_sync() -> Result<Vec<String>, LauncherError> {
    if let Some(cached_paths) = get_cached_java_paths() {
        return Ok(cached_paths);
    }

    let paths = scan_java_installations_parallel();
    // 扫一次几百毫秒，同步路径也回填缓存，选 Java 时不必重扫
    update_java_cache(paths.clone());

    Ok(paths)
}

/// 并行扫描 Java 安装，须在阻塞线程池中调用
fn scan_java_installations_parallel() -> Vec<String> {
    let java_dirs = get_java_installation_dirs();

    let dir_results: Vec<Vec<String>> = std::thread::scope(|s| {
        let handles: Vec<_> = java_dirs
            .iter()
            .map(|dir| s.spawn(|| find_java_in_directory(dir)))
            .collect();
        handles.into_iter().filter_map(|h| h.join().ok()).collect()
    });

    let mut paths: Vec<String> = dir_results.into_iter().flatten().collect();

    if let Ok(path_env) = std::env::var("PATH") {
        let separator = if cfg!(windows) { ';' } else { ':' };
        let path_entries: Vec<&str> = path_env.split(separator).collect();

        let path_results: Vec<String> = std::thread::scope(|s| {
            let handles: Vec<_> = path_entries
                .iter()
                .map(|entry| {
                    s.spawn(|| {
                        let path_buf = PathBuf::from(*entry);
                        let java_exe =
                            path_buf.join(if cfg!(windows) { "java.exe" } else { "java" });

                        if java_exe.exists() && is_valid_java_executable(&java_exe) {
                            Some(java_exe.to_string_lossy().replace("\\", "/"))
                        } else {
                            None
                        }
                    })
                })
                .collect();
            handles
                .into_iter()
                .filter_map(|h| h.join().ok().flatten())
                .collect()
        });

        paths.extend(path_results);
    }

    if let Ok(java_home) = std::env::var("JAVA_HOME") {
        let java_home_path = PathBuf::from(&java_home);
        let java_exe =
            java_home_path
                .join("bin")
                .join(if cfg!(windows) { "java.exe" } else { "java" });

        if java_exe.exists() && is_valid_java_executable(&java_exe) {
            let java_exe_path = java_exe.to_string_lossy().replace("\\", "/");
            if !paths.contains(&java_exe_path) {
                paths.push(java_exe_path);
            }
        }
    }

    for env_var in &[
        "JDK_HOME",
        "JRE_HOME",
        "JAVA_8_HOME",
        "JAVA_11_HOME",
        "JAVA_17_HOME",
        "JAVA_21_HOME",
    ] {
        if let Ok(java_path) = std::env::var(env_var) {
            let java_path_buf = PathBuf::from(&java_path);
            let java_exe =
                java_path_buf
                    .join("bin")
                    .join(if cfg!(windows) { "java.exe" } else { "java" });

            if java_exe.exists() && is_valid_java_executable(&java_exe) {
                let java_exe_path = java_exe.to_string_lossy().replace("\\", "/");
                if !paths.contains(&java_exe_path) {
                    paths.push(java_exe_path);
                }
            }
        }
    }

    let paths: Vec<String> = paths
        .into_iter()
        .map(|path| real_java_path(&path))
        .collect();

    dedup_by_canonical(paths)
}

/// 路径对应的真实安装：转发器（Oracle javapath / java8path）换到 java.home 下那份
fn real_java_path(path: &str) -> String {
    let exe = PathBuf::from(path);
    let Some(probe) = probe_java_exe(&exe) else {
        return path.to_string();
    };

    let real = real_java_exe(&exe, probe.home.as_deref());
    if real == exe {
        return path.to_string();
    }

    real.to_string_lossy().replace('\\', "/")
}

/// 按真实路径去重；同一份安装被符号链接二次命中时保留实体路径，界面上才不会指着链接
fn dedup_by_canonical(paths: Vec<String>) -> Vec<String> {
    let mut unique_paths = Vec::new();
    let mut seen_paths = std::collections::HashSet::new();

    for path in paths {
        let canonical = PathBuf::from(&path).canonicalize().ok().map(|real| {
            let raw = real.to_string_lossy();
            let stripped = raw.strip_prefix(r"\\?\").unwrap_or(&raw);
            stripped.replace("\\", "/")
        });

        let key = canonical
            .as_ref()
            .map(|c| c.to_lowercase())
            .unwrap_or_else(|| path.to_lowercase());

        if seen_paths.insert(key) {
            unique_paths.push(canonical.unwrap_or(path));
        }
    }

    unique_paths.sort();
    unique_paths
}

pub async fn refresh_java_installations() -> Result<Vec<String>, LauncherError> {
    invalidate_java_cache();
    find_java_installations_command().await
}

pub async fn set_java_path_command(path: String) -> Result<(), LauncherError> {
    let normalized_path = normalize_path(&path);

    if !validate_java_path(path.clone()).await? {
        return Err(LauncherError::Custom(format!(
            "无效的Java路径或Java版本: {}",
            normalized_path
        )));
    }

    update_config_async(|config| {
        config.java_path = Some(normalized_path);
        Ok(())
    })
    .await
}

/// 验证 Java 路径，支持相对路径、安装目录（含 bin/）与 PATH 上的 java
pub async fn validate_java_path(path: String) -> Result<bool, LauncherError> {
    // 验证要起子进程，不能占着 async worker；超时由 run_capture 统一兜底
    let result = tokio::task::spawn_blocking(move || {
        if path == "java" || path == "java.exe" {
            let java_cmd = if cfg!(windows) { "java.exe" } else { "java" };
            return find_java_in_path(java_cmd);
        }

        let exe = resolve_java_exe(&path);
        exe.is_file() && is_valid_java_executable(&exe)
    })
    .await
    .map_err(|e| LauncherError::Custom(format!("Java 验证任务异常终止: {}", e)))?;

    Ok(result)
}

pub async fn get_java_version(path: String) -> Result<String, LauncherError> {
    // 探测可能命中缓存（纯内存），冷缓存则要新起一个 JVM 子进程，整段进阻塞池
    tokio::task::spawn_blocking(move || {
        if let Some(probe) = probe_java(&path)
            && let Some(raw) = probe.raw
        {
            return Ok(raw);
        }

        let resolved = resolve_java_exe(&path);
        // 复用带超时的捕获，别在这里另开一条没有上限的 Command::output()
        let Some((stderr, stdout)) = run_capture(&resolved, &["-version"]) else {
            return Err(LauncherError::Custom("无法获取 Java 版本".to_string()));
        };

        // Java 版本信息通常输出到 stderr
        let version_output = if stderr.contains("version") {
            stderr
        } else {
            stdout
        };

        for line in version_output.lines() {
            if line.contains("version") {
                if let Some(start) = line.find('"')
                    && let Some(end) = line[start + 1..].find('"')
                {
                    return Ok(line[start + 1..start + 1 + end].to_string());
                }
                return Ok(line.trim().to_string());
            }
        }

        Err(LauncherError::Custom("无法解析 Java 版本".to_string()))
    })
    .await
    .map_err(|e| LauncherError::Custom(format!("Java 版本探测任务异常终止: {}", e)))?
}

/// 自动检测 Java 安装（同步，供配置加载使用）
pub fn auto_detect_java() -> Result<Vec<String>, LauncherError> {
    let mut java_paths = Vec::new();

    if let Ok(java_home) = std::env::var("JAVA_HOME") {
        let java_exe = if cfg!(windows) { "java.exe" } else { "java" };
        let java_path = PathBuf::from(&java_home).join("bin").join(java_exe);

        if java_path.exists() && is_valid_java_executable(&java_path) {
            java_paths.push(java_path.to_string_lossy().into_owned());
        }
    }

    #[cfg(target_os = "windows")]
    {
        let program_files =
            std::env::var("ProgramFiles").unwrap_or_else(|_| "C:\\Program Files".to_string());
        let program_files_x86 = std::env::var("ProgramFiles(x86)")
            .unwrap_or_else(|_| "C:\\Program Files (x86)".to_string());

        let common_dirs = vec![program_files, program_files_x86];

        for base_dir in common_dirs {
            if let Ok(entries) = std::fs::read_dir(&base_dir) {
                for entry in entries.flatten() {
                    if let Ok(file_type) = entry.file_type()
                        && file_type.is_dir()
                    {
                        let dir_name = entry.file_name().to_string_lossy().to_lowercase();
                        if dir_name.contains("java")
                            || dir_name.contains("jdk")
                            || dir_name.contains("jre")
                        {
                            let java_exe = entry.path().join("bin").join("java.exe");
                            if java_exe.exists() && is_valid_java_executable(&java_exe) {
                                java_paths.push(java_exe.to_string_lossy().into_owned());
                            }
                        }
                    }
                }
            }
        }
    }

    if let Ok(path_env) = std::env::var("PATH") {
        let separator = if cfg!(windows) { ';' } else { ':' };
        for path_entry in path_env.split(separator) {
            let path_buf = PathBuf::from(path_entry);
            let java_exe = path_buf.join(if cfg!(windows) { "java.exe" } else { "java" });

            if java_exe.exists() && is_valid_java_executable(&java_exe) {
                let path_str = java_exe.to_string_lossy().into_owned();
                if !java_paths.contains(&path_str) {
                    java_paths.push(path_str);
                }
            }
        }
    }

    let mut unique_paths = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for path in java_paths {
        let normalized = match PathBuf::from(&path).canonicalize() {
            Ok(canonical) => canonical.to_string_lossy().to_lowercase(),
            Err(_) => path.to_lowercase(),
        };

        if !seen.contains(&normalized) {
            seen.insert(normalized);
            unique_paths.push(path);
        }
    }

    Ok(unique_paths)
}

#[cfg(test)]
mod tests {
    use super::{
        JavaArch, JavaProbe, dedup_by_canonical, is_java_vendor_name, parse_arch, parse_major,
        property_value, quoted_version, real_java_exe,
    };
    use std::fs;
    use std::path::Path;
    use std::time::{Duration, Instant};

    #[test]
    fn test_run_capture_returns_output() {
        #[cfg(windows)]
        let (program, args) = ("cmd", vec!["/C", "echo", "hello"]);
        #[cfg(not(windows))]
        let (program, args) = ("sh", vec!["-c", "echo hello"]);

        let (stderr, stdout) =
            super::run_capture(Path::new(program), &args).expect("capture should succeed");
        assert!(
            stdout.contains("hello"),
            "stdout: {stdout}, stderr: {stderr}"
        );
    }

    #[test]
    fn test_run_capture_times_out_and_terminates_child() {
        #[cfg(windows)]
        let (program, args) = ("ping", vec!["-n", "10", "127.0.0.1"]);
        #[cfg(not(windows))]
        let (program, args) = ("sleep", vec!["5"]);

        let started = Instant::now();
        let result =
            super::run_capture_with_timeout(Path::new(program), &args, Duration::from_millis(200));

        assert!(result.is_err(), "超时的探测必须返回错误");
        assert!(
            matches!(result, Err(super::CaptureFailure::Timeout)),
            "超时必须报告 Timeout"
        );
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "超时后没有及时终止子进程"
        );
    }

    /// 白名单只决定「要不要再往下钻一层」，漏掉发行商就会少找到一份 Java
    #[test]
    fn test_java_vendor_name_covers_common_install_layouts() {
        for name in [
            "microsoft",
            "eclipse adoptium",
            "eclipse foundation",
            "amazon corretto",
            "zulu",
            "azul zulu",
            "bellsoft",
            "liberica",
            "redhat",
            "sapmachine",
            "graalvm",
            "openjdk",
            "temurin-21",
            "ibm",
            "oracle",
            "java",
        ] {
            assert!(
                is_java_vendor_name(name),
                "`{name}` 应当被当成发行商目录继续下钻"
            );
        }

        // 无关目录不该触发递归，否则 Program Files 下每个目录都要 read_dir 一次
        for name in [
            "windowsapps",
            "common files",
            "git",
            "nodejs",
            "nvidia corporation",
            "google",
        ] {
            assert!(
                !is_java_vendor_name(name),
                "`{name}` 不该被继续下钻，这是纯粹的无用 IO"
            );
        }
    }

    #[test]
    fn test_property_value_matches_the_exact_key() {
        // 真实输出里 java.version 与 java.version.date 挨着，前缀匹配会取错
        let text =
            "    java.version = 25.0.4.1\n    java.version.date = 2026-08-18\n    os.arch = amd64";
        assert_eq!(
            property_value(text, "java.version").as_deref(),
            Some("25.0.4.1")
        );
        assert_eq!(property_value(text, "os.arch").as_deref(), Some("amd64"));
        assert_eq!(property_value(text, "java.home"), None);
    }

    #[test]
    fn test_parse_major_covers_the_shapes_vendors_print() {
        // 老写法：第二位才是大版本
        assert_eq!(parse_major("1.8.0_432"), Some(8));
        assert_eq!(parse_major("1.7.0_80"), Some(7));
        // 9 起直接是大版本
        assert_eq!(parse_major("17.0.8"), Some(17));
        assert_eq!(parse_major("21"), Some(21));
        // 预览版也要解析出来，否则 `24-ea` 这类候选被静默丢掉
        assert_eq!(parse_major("24-ea"), Some(24));
        assert_eq!(parse_major("23-ea+1-32"), Some(23));
        // 带 build 号与 LTS 尾巴
        assert_eq!(parse_major("21.0.5+11-LTS"), Some(21));
        assert_eq!(parse_major("11.0.9.1"), Some(11));
    }

    #[test]
    fn test_parse_major_rejects_garbage() {
        assert_eq!(parse_major(""), None);
        assert_eq!(parse_major("garbage"), None);
        assert_eq!(parse_major("0"), None);
    }

    #[test]
    fn test_parse_arch_distinguishes_32_bit() {
        assert_eq!(parse_arch("amd64"), JavaArch::Bits64);
        assert_eq!(parse_arch("x86_64"), JavaArch::Bits64);
        assert_eq!(parse_arch("aarch64"), JavaArch::Bits64);
        // 32 位 JRE 最容易选错：能起来但开不了大堆
        assert_eq!(parse_arch("x86"), JavaArch::Bits32);
        assert_eq!(parse_arch("i386"), JavaArch::Bits32);
        assert_eq!(parse_arch(""), JavaArch::Unknown);
    }

    #[test]
    fn test_quoted_version_reads_the_vendor_line() {
        let text = "Picked up JAVA_TOOL_OPTIONS: -Dfile.encoding=UTF-8\nopenjdk version \"17.0.8\" 2023-07-18\nOpenJDK Runtime Environment";
        assert_eq!(quoted_version(text).as_deref(), Some("17.0.8"));

        let java8 = "java version \"1.8.0_432\"\nJava(TM) SE Runtime Environment";
        assert_eq!(quoted_version(java8).as_deref(), Some("1.8.0_432"));
    }

    #[test]
    fn test_ea_probe_is_ranked_below_release_builds() {
        let ea = JavaProbe {
            version: Some(24),
            raw: Some("24-ea".to_string()),
            arch: JavaArch::Bits64,
            home: None,
        };
        let release = JavaProbe {
            version: Some(24),
            raw: Some("24.0.1".to_string()),
            arch: JavaArch::Bits64,
            home: None,
        };
        assert!(ea.is_ea());
        assert!(!release.is_ea());
    }

    #[test]
    fn test_real_java_exe_rewrites_the_oracle_stub() {
        // Oracle 的 javapath 里那份只是转发器，java.home 才是真身
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("jdk8");
        fs::create_dir_all(home.join("bin")).unwrap();
        fs::write(home.join("bin").join("java.exe"), b"x").unwrap();

        let stub = tmp
            .path()
            .join("Common Files/Oracle/Java/java8path/java.exe");
        fs::create_dir_all(stub.parent().unwrap()).unwrap();
        fs::write(&stub, b"stub").unwrap();

        let real = real_java_exe(&stub, Some(&home.to_string_lossy()));
        assert_eq!(real, home.join("bin").join("java.exe"));
    }

    #[test]
    fn test_real_java_exe_keeps_paths_already_inside_home() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("jdk17");
        fs::create_dir_all(home.join("bin")).unwrap();
        let exe = home.join("bin").join("java.exe");
        fs::write(&exe, b"x").unwrap();

        assert_eq!(real_java_exe(&exe, Some(&home.to_string_lossy())), exe);

        // Java 8 的 JDK：java.home 报的是 <jdk>/jre，用户填的却是 <jdk>/bin/java.exe
        let jre = home.join("jre");
        fs::create_dir_all(jre.join("bin")).unwrap();
        fs::write(jre.join("bin").join("java.exe"), b"x").unwrap();
        assert_eq!(real_java_exe(&exe, Some(&jre.to_string_lossy())), exe);
    }

    #[test]
    fn test_real_java_exe_without_home_is_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        let exe = tmp.path().join("bin/java.exe");
        fs::create_dir_all(exe.parent().unwrap()).unwrap();
        fs::write(&exe, b"x").unwrap();

        assert_eq!(real_java_exe(&exe, None), exe);
    }

    #[test]
    fn test_dedup_keeps_one_representative_per_real_path() {
        let out = dedup_by_canonical(vec![
            "C:/Program Files/Zulu/zulu-26/bin/java.exe".to_string(),
            "C:/Program Files/Zulu/zulu-26/bin/java.exe".to_string(),
        ]);
        assert_eq!(out.len(), 1, "{:?}", out);
    }

    #[test]
    fn test_dedup_keeps_unresolvable_paths() {
        // canonicalize 失败时不能顺手把候选丢掉，否则新装的 JDK 直接看不见
        let out = dedup_by_canonical(vec![
            "Q:/does/not/exist/bin/java.exe".to_string(),
            "R:/does/not/exist/bin/java.exe".to_string(),
        ]);
        assert_eq!(out.len(), 2, "{:?}", out);
    }
}
