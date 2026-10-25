//! 游戏进程启动和监控逻辑
//!
//! 派生隐藏控制台的 Java 子进程，日志经有界队列批量转发，退出时回显日志尾巴与 hs_err_pid

use super::crash_diagnosis::{self, LaunchContext};
use crate::errors::LauncherError;
use crate::services::instance_logs::{self, LaunchFinish};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};
use tauri::Emitter;

const MAX_GAME_RUNTIME: Duration = Duration::from_secs(24 * 60 * 60);

/// 队列满时直接丢弃，绝不阻塞读取线程——堵住会让管道写满，游戏日志线程卡住，表现为周期性掉帧
const LOG_QUEUE_CAPACITY: usize = 4096;

const LOG_FLUSH_MAX_LINES: usize = 200;

const LOG_FLUSH_INTERVAL: Duration = Duration::from_millis(200);

const LOG_FLUSH_WAIT: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, PartialEq, Eq)]
enum LogStream {
    Stdout,
    Stderr,
}

type LogItem = (LogStream, String);

/// 诊断要读完整输出，4 MiB 足够装下一次启动的全部输出
const MAX_CAPTURED_BYTES: usize = 4 * 1024 * 1024;

/// 崩溃时供诊断使用的输出缓冲：到上限后停止收集，只报丢了多少行
pub struct OutputCapture {
    lines: Vec<String>,
    bytes: usize,
    cap: usize,
    dropped: usize,
}

impl Default for OutputCapture {
    fn default() -> Self {
        Self::with_cap(MAX_CAPTURED_BYTES)
    }
}

impl OutputCapture {
    pub fn with_cap(cap: usize) -> Self {
        Self {
            lines: Vec::new(),
            bytes: 0,
            cap,
            dropped: 0,
        }
    }

    pub fn push(&mut self, line: String) {
        if self.bytes + line.len() > self.cap {
            self.dropped += 1;
            return;
        }
        self.bytes += line.len();
        self.lines.push(line);
    }

    /// 溢出时把真实丢弃行数补成最后一行，诊断和原文都看得到
    pub fn finish(&self) -> Vec<String> {
        let mut out = self.lines.clone();
        if self.dropped > 0 {
            out.push(format!(
                "… 输出达到 {} 字节上限，另有 {} 行未收集",
                self.cap, self.dropped
            ));
        }
        out
    }
}

/// argv 里 --accessToken 后面跟的是真实的 Minecraft bearer token，进 IPC 之前必须掩掉
fn redacted_args(args: &[String]) -> Vec<&str> {
    let mut out = Vec::with_capacity(args.len());
    let mut mask_next = false;

    for arg in args {
        if mask_next {
            out.push("***");
            mask_next = false;
        } else if arg == "--accessToken" {
            out.push(arg);
            mask_next = true;
        } else if arg.starts_with("--accessToken=") {
            out.push("--accessToken=***");
        } else {
            out.push(arg);
        }
    }

    out
}

struct LaunchHistory {
    instance_dir: PathBuf,
    record_id: Option<i64>,
    started_at_ms: i64,
}

fn finish_launch_history(
    history: &LaunchHistory,
    content_root: &Path,
    exit_code: Option<i32>,
    duration_secs: u64,
    crash_report: Option<String>,
    archive_log: bool,
) {
    let Some(record_id) = history.record_id else {
        return;
    };
    instance_logs::record_launch_finished(LaunchFinish {
        instance_dir: history.instance_dir.clone(),
        content_root: content_root.to_path_buf(),
        history_id: record_id,
        started_at_ms: history.started_at_ms,
        exit_code,
        duration_secs,
        crash_report,
        archive_log,
    });
}

pub fn spawn_and_monitor_process(
    java_path: &str,
    final_args: Vec<String>,
    working_dir: &Path,
    instance_dir: PathBuf,
    launch_ctx: LaunchContext,
    window: tauri::Window,
    run_lock: crate::utils::file_lock::FileLock,
) -> Result<(), LauncherError> {
    let mut command = Command::new(java_path);
    command.args(&final_args);
    command.current_dir(working_dir);

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }

    let display_command = format!("{} {}", java_path, redacted_args(&final_args).join(" "));
    let _ = window.emit("log-debug", format!("最终启动命令: {}", display_command));
    window.emit("launch-command", display_command)?;

    let child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let pid = child.id();
    let _ = window.emit("log-debug", format!("游戏进程已启动，PID: {}", pid));

    // 进程已经起来了，事件发不出去也必须继续监控并持有运行锁
    let _ = window.emit(
        "minecraft-launched",
        format!("游戏进程已启动，PID: {}", pid),
    );

    let started_at_ms = super::now_millis();
    let history = LaunchHistory {
        record_id: instance_logs::record_launch_started(&instance_dir, started_at_ms),
        instance_dir,
        started_at_ms,
    };

    spawn_monitor_thread(
        child,
        window,
        pid,
        working_dir.to_path_buf(),
        history,
        launch_ctx,
        run_lock,
    );

    Ok(())
}

fn spawn_monitor_thread(
    mut child: Child,
    window: tauri::Window,
    pid: u32,
    working_dir: std::path::PathBuf,
    history: LaunchHistory,
    launch_ctx: crash_diagnosis::LaunchContext,
    run_lock: crate::utils::file_lock::FileLock,
) {
    let stdout = child.stdout.take().expect("Failed to open stdout");
    let stderr = child.stderr.take().expect("Failed to open stderr");

    let is_running = Arc::new(AtomicBool::new(true));
    let captured: Arc<Mutex<OutputCapture>> = Arc::new(Mutex::new(OutputCapture::default()));

    let (log_tx, log_rx) = mpsc::sync_channel::<LogItem>(LOG_QUEUE_CAPACITY);

    let tx_stdout: SyncSender<LogItem> = log_tx.clone();
    let capture_stdout = captured.clone();
    std::thread::spawn(move || {
        let reader = BufReader::new(stdout);
        // 读到 EOF 为止：进程秒退时管道里还压着最后几行报错，提前 break 会让崩溃弹窗没有输出
        for line in reader.lines() {
            match line {
                Ok(l) => {
                    push_line(&capture_stdout, &l);
                    let _ = tx_stdout.try_send((LogStream::Stdout, l));
                }
                Err(_) => break,
            }
        }
    });

    let tx_stderr: SyncSender<LogItem> = log_tx;
    let capture_stderr = captured.clone();
    std::thread::spawn(move || {
        let reader = BufReader::new(stderr);
        for line in reader.lines() {
            match line {
                Ok(l) => {
                    push_line(&capture_stderr, &l);
                    let _ = tx_stderr.try_send((LogStream::Stderr, l));
                }
                Err(_) => break,
            }
        }
    });

    let forwarder_done = Arc::new(AtomicBool::new(false));
    let forwarder_done_flag = forwarder_done.clone();
    let window_forwarder = window.clone();
    let forwarder: JoinHandle<()> = std::thread::spawn(move || {
        forward_game_logs(log_rx, &window_forwarder);
        forwarder_done_flag.store(true, Ordering::SeqCst);
    });

    std::thread::spawn(move || {
        let start_time = Instant::now();
        let is_running_main = is_running.clone();

        let is_running_timeout = is_running.clone();
        let window_timeout = window.clone();
        let timeout_thread = std::thread::spawn(move || {
            while is_running_timeout.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_secs(60));

                if !is_running_timeout.load(Ordering::SeqCst) {
                    break;
                }

                let elapsed = start_time.elapsed();
                if elapsed > MAX_GAME_RUNTIME {
                    let _ = window_timeout.emit(
                        "log-warning",
                        format!(
                            "游戏运行时间超过 {} 小时，监控线程将停止",
                            MAX_GAME_RUNTIME.as_secs() / 3600
                        ),
                    );
                    break;
                }
            }
        });

        match wait_for_process_holding_lock(&mut child, MAX_GAME_RUNTIME, run_lock) {
            Ok(Some(status)) => {
                is_running_main.store(false, Ordering::SeqCst);
                wait_log_flushed(&forwarder_done);
                if forwarder_done.load(Ordering::SeqCst) {
                    let _ = forwarder.join();
                }
                let lines = captured.lock().map(|buf| buf.finish()).unwrap_or_default();
                let elapsed = start_time.elapsed();
                handle_process_exit(
                    status,
                    &window,
                    &lines,
                    &working_dir,
                    &launch_ctx,
                    &history,
                    elapsed.as_secs(),
                );
            }
            Ok(None) => {
                is_running_main.store(false, Ordering::SeqCst);
                finish_launch_history(
                    &history,
                    &working_dir,
                    None,
                    start_time.elapsed().as_secs(),
                    None,
                    false,
                );
                let _ = window.emit(
                    "log-warning",
                    format!("游戏进程 (PID: {}) 运行超时，停止监控", pid),
                );
                let _ = window.emit(
                    "minecraft-timeout",
                    format!(
                        "游戏运行超过 {} 小时，监控已停止",
                        MAX_GAME_RUNTIME.as_secs() / 3600
                    ),
                );
            }
            Err(e) => {
                is_running_main.store(false, Ordering::SeqCst);
                finish_launch_history(
                    &history,
                    &working_dir,
                    None,
                    start_time.elapsed().as_secs(),
                    None,
                    false,
                );
                let _ = window.emit("log-error", format!("监控游戏进程时出错: {}", e));
                let _ = window.emit("minecraft-error", format!("监控游戏进程时出错: {}", e));
            }
        }

        let _ = timeout_thread.join();
    });
}

/// 追加一行到诊断缓冲；锁被占用时丢这一行，绝不阻塞读线程
fn push_line(captured: &Mutex<OutputCapture>, line: &str) {
    if let Ok(mut buf) = captured.lock() {
        buf.push(line.to_string());
    }
}

fn wait_log_flushed(forwarder_done: &AtomicBool) {
    let deadline = Instant::now() + LOG_FLUSH_WAIT;
    while !forwarder_done.load(Ordering::SeqCst) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// 按固定节奏批量转发日志到前端，避免逐行事件淹没 WebView
fn forward_game_logs(rx: Receiver<LogItem>, window: &tauri::Window) {
    let mut batch: Vec<LogItem> = Vec::with_capacity(LOG_FLUSH_MAX_LINES);

    loop {
        match rx.recv_timeout(LOG_FLUSH_INTERVAL) {
            Ok(item) => {
                batch.push(item);
                while batch.len() < LOG_FLUSH_MAX_LINES {
                    match rx.try_recv() {
                        Ok(item) => batch.push(item),
                        Err(_) => break,
                    }
                }
                flush_log_batch(&batch, window);
                batch.clear();
            }
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }

    flush_log_batch(&batch, window);
}

fn flush_log_batch(batch: &[LogItem], window: &tauri::Window) {
    if batch.is_empty() {
        return;
    }

    let mut out = String::new();
    let mut err = String::new();
    for (stream, line) in batch {
        let target = match stream {
            LogStream::Stdout => &mut out,
            LogStream::Stderr => &mut err,
        };
        target.push_str(line);
        target.push('\n');
    }

    if !out.is_empty() {
        let _ = window.emit("log-debug", out);
    }
    if !err.is_empty() {
        let _ = window.emit("log-error", err);
    }
}

fn wait_for_process_with_timeout(
    child: &mut Child,
    timeout: Duration,
) -> Result<Option<ExitStatus>, std::io::Error> {
    let start = Instant::now();

    loop {
        match child.try_wait()? {
            Some(status) => return Ok(Some(status)),
            None => {
                if start.elapsed() > timeout {
                    return Ok(None);
                }
                std::thread::sleep(Duration::from_millis(500));
            }
        }
    }
}

/// 运行锁只覆盖游戏进程本身。日志转发、崩溃诊断和 timeout 线程清理可能还要
/// 几十秒，不能把锁带进这些收尾流程，否则玩家关掉游戏后短时间内无法重开
fn wait_for_process_holding_lock(
    child: &mut Child,
    timeout: Duration,
    run_lock: crate::utils::file_lock::FileLock,
) -> Result<Option<ExitStatus>, std::io::Error> {
    let _run_lock = run_lock;
    wait_for_process_with_timeout(child, timeout)
}

/// 只认本次运行之后写下的 hs_err，上次崩溃留下的那份不能当本次证据
fn find_hs_err_log(working_dir: &Path, launched_after: SystemTime) -> Option<String> {
    if let Ok(entries) = fs::read_dir(working_dir) {
        let mut latest: Option<(std::time::SystemTime, String)> = None;
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with("hs_err_pid")
                && name_str.ends_with(".log")
                && let Ok(metadata) = entry.metadata()
                && let Ok(modified) = metadata.modified()
            {
                if modified < launched_after {
                    continue;
                }
                if latest.as_ref().is_none_or(|(t, _)| modified > *t)
                    && let Ok(content) = fs::read_to_string(entry.path())
                {
                    latest = Some((modified, content));
                }
            }
        }
        return latest.map(|(_, content)| content);
    }
    None
}

/// 本次运行写下的崩溃报告。关闭看门狗会先把报告落盘，再 `System.exit(-8)`
fn find_new_crash_report(
    working_dir: &Path,
    launched_after: SystemTime,
) -> Option<(std::path::PathBuf, String)> {
    let mut latest: Option<(SystemTime, std::path::PathBuf, String)> = None;

    for entry in fs::read_dir(working_dir.join("crash-reports"))
        .ok()?
        .flatten()
    {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if !name_str.starts_with("crash-") || !name_str.ends_with(".txt") {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let Ok(modified) = metadata.modified() else {
            continue;
        };
        if modified < launched_after {
            continue;
        }
        if latest
            .as_ref()
            .is_some_and(|(time, _, _)| modified <= *time)
        {
            continue;
        }
        let path = entry.path();
        if let Ok(content) = fs::read_to_string(&path) {
            latest = Some((modified, path, content));
        }
    }

    latest.map(|(_, path, content)| (path, content))
}

/// 26.3 起客户端退出阶段卡住 15 秒，游戏会写这份报告并主动结束进程
fn is_client_shutdown_watchdog(report: &str) -> bool {
    report.contains("Watchdog (Client shutdown")
}

/// 退出码留在诊断和日志里，通知文案只给人看
fn exit_message(exit_code: Option<i32>) -> String {
    match exit_code {
        Some(0) => "游戏已退出".to_string(),
        Some(code) => format!("游戏异常退出（代码 {}）", code),
        None => "游戏已退出（状态未知）".to_string(),
    }
}

/// hs_err 全文几十 KB，报告里只留头部这些行
const HS_ERR_MAX_LINES: usize = 200;

fn handle_process_exit(
    status: ExitStatus,
    window: &tauri::Window,
    output_lines: &[String],
    working_dir: &Path,
    launch_ctx: &LaunchContext,
    history: &LaunchHistory,
    runtime_secs: u64,
) {
    let launched_at =
        SystemTime::UNIX_EPOCH + Duration::from_millis(history.started_at_ms.max(0) as u64);
    let exit_code = status.code();
    let _ = window.emit(
        "log-debug",
        format!("游戏进程退出，状态码: {:?}", exit_code),
    );

    let mut message = exit_message(exit_code);
    let mut crash_report_name = None;

    if exit_code.unwrap_or(-1) != 0 {
        // 关闭看门狗是玩家点了退出之后收尾超时，不是启动崩溃：弹“启动失败”会冤到模组
        let new_crash_report = find_new_crash_report(working_dir, launched_at);
        crash_report_name = new_crash_report
            .as_ref()
            .and_then(|(path, _)| path.file_name().and_then(|name| name.to_str()))
            .map(str::to_string);
        let shutdown_watchdog = new_crash_report
            .as_ref()
            .is_some_and(|(_, report)| is_client_shutdown_watchdog(report));

        if shutdown_watchdog {
            if let Some((path, _)) = &new_crash_report {
                let _ = window.emit(
                    "log-warning",
                    format!(
                        "游戏关闭超时，已被客户端看门狗结束（常见于模组残留的非 daemon 线程）；崩溃报告: {}",
                        path.display()
                    ),
                );
            }
            message = "游戏已退出（关闭超时，已被游戏自身的看门狗结束；通常是模组残留线程导致）"
                .to_string();
        } else {
            let hs_err = find_hs_err_log(working_dir, launched_at).map(|text| {
                let total = text.lines().count();
                let mut head = text
                    .lines()
                    .take(HS_ERR_MAX_LINES)
                    .collect::<Vec<_>>()
                    .join("\n");
                if total > HS_ERR_MAX_LINES {
                    head.push_str("\n… hs_err 其余部分见工作目录里的原文件");
                }
                head
            });

            let mut ctx = launch_ctx.clone();
            ctx.runtime_secs = Some(runtime_secs);

            let report_path = new_crash_report
                .as_ref()
                .map(|(path, _)| path.to_string_lossy().to_string());
            let report_evidence = new_crash_report.as_ref().and_then(|(_, text)| {
                report_path
                    .as_deref()
                    .map(|path| crash_diagnosis::CrashReportEvidence { path, text })
            });

            let mut diagnosis = crash_diagnosis::diagnose_with_crash_report(
                exit_code,
                output_lines,
                &ctx,
                hs_err.as_deref(),
                report_evidence,
            );
            attach_suggested_java(&mut diagnosis, &ctx);
            let _ = window.emit("minecraft-crash", &diagnosis);
        }
    }

    finish_launch_history(
        history,
        working_dir,
        exit_code,
        runtime_secs,
        crash_report_name,
        true,
    );

    let _ = window.emit("minecraft-exited", message);
}

/// 诊断点到 Java 时，顺手挑一份判据允许、且不是本次用的 Java。
/// 弹窗靠它给出「换 Java 重试」，而不是只让用户自己去设置页翻
fn attach_suggested_java(diagnosis: &mut crash_diagnosis::CrashDiagnosis, ctx: &LaunchContext) {
    let java_related = diagnosis.findings.iter().any(|finding| {
        matches!(
            finding.kind,
            crash_diagnosis::FindingKind::JavaVersion
                | crash_diagnosis::FindingKind::ClassReaderTooOld
                | crash_diagnosis::FindingKind::ClassVersion
                | crash_diagnosis::FindingKind::SecurityManager
        )
    });
    if !java_related {
        return;
    }

    let Ok(config) = crate::services::config::load_config() else {
        return;
    };
    let game_dir = super::java::resolve_game_dir(&config.game_dir);
    let profile = super::java_requirement::analyze(&ctx.version_id, &game_dir);

    let mut candidates = super::java::collect_candidates(&config);
    candidates.retain(|candidate| candidate.path != ctx.used_java_path);

    if let Some(matched) = super::java::choose_java(&profile.requirement, &candidates) {
        diagnosis.suggested_java = Some(crash_diagnosis::SuggestedJava {
            path: matched.path,
            version: matched.version,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{
        OutputCapture, exit_message, find_hs_err_log, find_new_crash_report,
        is_client_shutdown_watchdog, redacted_args, wait_for_process_holding_lock,
    };
    use std::fs;
    use std::path::Path;
    use std::process::{Command, Stdio};
    use std::time::{Duration, SystemTime};

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn test_exit_message_never_leaks_debug_formatting() {
        assert_eq!(exit_message(Some(0)), "游戏已退出");
        assert_eq!(exit_message(Some(-8)), "游戏异常退出（代码 -8）");
        assert_eq!(exit_message(None), "游戏已退出（状态未知）");
        assert!(
            !exit_message(Some(0)).contains("Some"),
            "用户不该看到 Rust 的 Option 调试格式"
        );
    }

    fn write_hs_err(dir: &Path, name: &str, body: &str, age: Duration) -> std::io::Result<()> {
        let path = dir.join(name);
        fs::write(&path, body)?;
        let file = fs::OpenOptions::new().append(true).open(&path)?;
        file.set_modified(SystemTime::now() - age)
    }

    fn write_crash_report(
        dir: &Path,
        name: &str,
        body: &str,
        age: Duration,
    ) -> std::io::Result<()> {
        let reports = dir.join("crash-reports");
        fs::create_dir_all(&reports)?;
        let path = reports.join(name);
        fs::write(&path, body)?;
        let file = fs::OpenOptions::new().append(true).open(&path)?;
        file.set_modified(SystemTime::now() - age)
    }

    const SHUTDOWN_WATCHDOG_REPORT: &str = "\
---- Minecraft Crash Report ----
Description: Client shutdown from post-main

java.lang.Error: Watchdog (Client shutdown from post-main)
\tat net.minecraft.server.dedicated.ServerWatchdog.createWatchdogCrashReport(ServerWatchdog.java:83)";

    /// 26.3 的 ClientShutdownWatchdog 在关闭卡住 15 秒后写这份报告再 exit(-8)。
    /// 这是玩家点了退出之后的收尾超时，不能拿去当“启动失败”弹窗。
    #[test]
    fn test_new_shutdown_watchdog_report_is_recognized() {
        let dir = tempfile::TempDir::new().unwrap();
        write_crash_report(
            dir.path(),
            "crash-2026-10-18_22.35.26-client.txt",
            SHUTDOWN_WATCHDOG_REPORT,
            Duration::from_secs(1),
        )
        .unwrap();

        let launched = SystemTime::now() - Duration::from_secs(60);
        let (path, report) = find_new_crash_report(dir.path(), launched).expect("应当找到本次报告");
        assert!(path.ends_with("crash-2026-10-18_22.35.26-client.txt"));
        assert!(is_client_shutdown_watchdog(&report));
    }

    #[test]
    fn test_previous_crash_report_is_ignored() {
        let dir = tempfile::TempDir::new().unwrap();
        write_crash_report(
            dir.path(),
            "crash-old-client.txt",
            SHUTDOWN_WATCHDOG_REPORT,
            Duration::from_secs(3600),
        )
        .unwrap();

        let launched = SystemTime::now() - Duration::from_secs(60);
        assert_eq!(find_new_crash_report(dir.path(), launched), None);
    }

    #[test]
    fn test_launch_crash_report_is_not_mistaken_for_shutdown() {
        let dir = tempfile::TempDir::new().unwrap();
        write_crash_report(
            dir.path(),
            "crash-2026-10-18_22.35.26-client.txt",
            "---- Minecraft Crash Report ----\nDescription: Rendering overlay\n\njava.lang.NullPointerException: test",
            Duration::from_secs(1),
        )
        .unwrap();

        let launched = SystemTime::now() - Duration::from_secs(60);
        let (_, report) = find_new_crash_report(dir.path(), launched).expect("应当找到本次报告");
        assert!(!is_client_shutdown_watchdog(&report));
    }

    #[test]
    fn test_newest_crash_report_wins() {
        let dir = tempfile::TempDir::new().unwrap();
        write_crash_report(
            dir.path(),
            "crash-old-client.txt",
            "---- Minecraft Crash Report ----\nDescription: old",
            Duration::from_secs(120),
        )
        .unwrap();
        write_crash_report(
            dir.path(),
            "crash-new-client.txt",
            SHUTDOWN_WATCHDOG_REPORT,
            Duration::from_secs(1),
        )
        .unwrap();

        let launched = SystemTime::now() - Duration::from_secs(180);
        let (path, _) = find_new_crash_report(dir.path(), launched).expect("应当找到本次报告");
        assert!(path.ends_with("crash-new-client.txt"));
    }

    #[test]
    fn test_run_lock_released_when_game_process_exits() {
        let dir = tempfile::TempDir::new().unwrap();
        let lock_path = dir.path().join("run.lock");
        let lock =
            crate::utils::file_lock::acquire_shared(&lock_path, Duration::from_millis(50)).unwrap();

        #[cfg(windows)]
        let mut child = Command::new("ping")
            .args(["-n", "2", "127.0.0.1"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        #[cfg(not(windows))]
        let mut child = Command::new("sleep")
            .arg("0.2")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();

        let status =
            wait_for_process_holding_lock(&mut child, Duration::from_secs(10), lock).unwrap();
        assert!(status.is_some());

        // 运行锁要在 flush 日志、join 超时线程之前释放，否则关掉游戏后短时间内无法重开
        assert!(
            crate::utils::file_lock::acquire(&lock_path, Duration::from_millis(100)).is_ok(),
            "游戏进程已退出，运行锁仍被监控线程持有"
        );
    }

    #[test]
    fn test_find_hs_err_log_ignores_previous_runs() {
        let dir = tempfile::TempDir::new().unwrap();
        write_hs_err(
            dir.path(),
            "hs_err_pid111.log",
            "OLD",
            Duration::from_secs(3600),
        )
        .unwrap();
        write_hs_err(
            dir.path(),
            "hs_err_pid222.log",
            "FRESH",
            Duration::from_secs(1),
        )
        .unwrap();

        let launched = SystemTime::now() - Duration::from_secs(60);
        assert_eq!(
            find_hs_err_log(dir.path(), launched).as_deref(),
            Some("FRESH")
        );

        let future = SystemTime::now() + Duration::from_secs(3600);
        assert_eq!(
            find_hs_err_log(dir.path(), future),
            None,
            "上一次运行留下的 hs_err 不能当本次的证据"
        );
    }

    #[test]
    fn test_redacted_args_masks_token_after_flag() {
        let input = args(&[
            "--username",
            "Steve",
            "--accessToken",
            "SECRET",
            "--version",
            "1.20.4",
        ]);
        assert_eq!(
            redacted_args(&input),
            [
                "--username",
                "Steve",
                "--accessToken",
                "***",
                "--version",
                "1.20.4"
            ]
        );
    }

    #[test]
    fn test_redacted_args_masks_inline_token() {
        let input = args(&["--accessToken=SECRET", "--gameDir", "dir"]);
        assert_eq!(
            redacted_args(&input),
            ["--accessToken=***", "--gameDir", "dir"]
        );
        assert!(redacted_args(&Vec::new()).is_empty());
    }

    #[test]
    fn test_capture_keeps_head_and_reports_the_overflow() {
        let mut cap = OutputCapture::with_cap(8);
        for _ in 0..3 {
            cap.push("abcd".to_string());
        }

        let lines = cap.finish();
        assert_eq!(lines.len(), 3, "最后一行是溢出说明");
        assert_eq!(lines[0], "abcd");
        assert!(lines[2].contains("另有 1 行未收集"));
    }
}
