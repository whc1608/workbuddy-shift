//! 进程控制：检测、关闭、启动 WorkBuddy。
//!
//! 对照 server.py `is_workbuddy_running` / `_wait_process_gone` /
//! `close_workbuddy` / `launch_workbuddy`。

// Windows 映像名/路径辅助在非 Windows 上只供单测使用。
#![cfg_attr(not(target_os = "windows"), allow(dead_code, unused_imports))]

use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};
// macOS 的 app 路径解析收敛在本模块内（`macos_workbuddy_app_path_*`），不再回引 auth_file。
#[cfg(target_os = "macos")]
use self::macos::close_workbuddy_macos;
#[cfg(target_os = "macos")]
use self::macos::launch_workbuddy_macos;
#[cfg(target_os = "macos")]
use self::macos::macos_main_patterns;
#[cfg(target_os = "macos")]
use self::macos::macos_workbuddy_app_path_resolved;
#[cfg(target_os = "windows")]
use self::windows::close_workbuddy_windows;
#[cfg(target_os = "windows")]
use self::windows::persist_workbuddy_exe;
#[cfg(target_os = "windows")]
use self::windows::windows_exe_names;
#[cfg(target_os = "windows")]
use self::windows::windows_workbuddy_process_rows;
#[cfg(not(target_os = "macos"))]
use crate::modules::auth_file;
use crate::modules::variant::WbVariant;
use std::io::Read;

#[cfg(target_os = "macos")]
mod macos;
// Win32 窗口消息关闭助手（`SC_CLOSE`）：只在 Windows 编译，非 Windows 上没有 FFI 面。
#[cfg(target_os = "windows")]
mod win_close;
mod windows;
// 客户端版本读取的唯一实现（诊断包与 R6-06 平台闸门共用）。
mod version;
pub use version::client_version;

#[cfg(target_os = "macos")]
pub(crate) use self::macos::extract_app_bundle_from_args;
#[cfg(target_os = "macos")]
pub(crate) use self::macos::filter_ps_rows;
#[cfg(target_os = "macos")]
pub(crate) use self::macos::is_app_bundle;
#[cfg(target_os = "macos")]
pub(crate) use self::macos::kill_macos_pids;
#[cfg(target_os = "macos")]
pub(crate) use self::macos::macos_pids_by_patterns;
#[cfg(target_os = "macos")]
pub(crate) use self::macos::macos_rows_by_patterns;
#[cfg(target_os = "macos")]
pub use self::macos::macos_workbuddy_app_path;
#[cfg(target_os = "macos")]
pub(crate) use self::macos::wait_macos_main_gone;
#[cfg(target_os = "macos")]
pub(crate) use self::macos::wait_macos_patterns_empty;
#[cfg(target_os = "windows")]
pub(crate) use self::win_close::{
    post_close_to_visible_windows, visible_window_pids, window_titles,
};
#[cfg(target_os = "windows")]
pub(crate) use self::windows::existing_windows_drives;
pub(crate) use self::windows::parse_windows_display_icon;
pub(crate) use self::windows::parse_windows_process_rows;
#[cfg(target_os = "windows")]
pub(crate) use self::windows::ps_output;
#[cfg(target_os = "windows")]
pub(crate) use self::windows::wait_windows_pids_gone;
#[cfg(target_os = "windows")]
pub use self::windows::windows_default_app_path;
#[cfg(target_os = "windows")]
pub(crate) use self::windows::windows_tasklist_image_rows;
#[cfg(target_os = "windows")]
pub use self::windows::windows_workbuddy_exe_path;
pub(crate) use self::windows::WindowsProcessRow;

/// 创建子进程命令。Windows 上加 CREATE_NO_WINDOW，避免每次执行 tasklist/powershell
/// 等控制台命令时闪出 cmd 黑窗口（GUI 应用卡顿/跳动的主因）。
pub(crate) fn cmd_builder(program: impl AsRef<std::ffi::OsStr>) -> Command {
    // 只有 Windows cfg 分支会调用 creation_flags，非 Windows 下这个 mut 没有使用者。
    #[allow(unused_mut)]
    let mut c = Command::new(program);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    c
}

/// 运行命令并等待退出，超时则 kill 并返回 None（对应 Python `subprocess.run(timeout=...)`）。
///
/// 注意：stdout/stderr 必须与等待并发读取——先等退出再读会在输出超过
/// 64KB 管道缓冲时死锁（`ps -axo` 全量输出在进程多的机器上很容易超过），
/// 子进程写满阻塞永不退出，最终被超时 kill 并返回 None。
pub(crate) fn run_cmd_timeout(program: &str, args: &[&str], timeout_secs: u64) -> Option<Output> {
    let mut child = cmd_builder(program)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let mut stderr = child.stderr.take()?;

    let out_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        buf
    });
    let err_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf);
        buf
    });

    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    let mut status = None;
    while status.is_none() {
        match child.try_wait() {
            Ok(Some(s)) => status = Some(s),
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    // 管道随进程退出关闭，读线程随即结束。
                    return None;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(_) => return None,
        }
    }
    let out = out_reader.join().unwrap_or_default();
    let err = err_reader.join().unwrap_or_default();
    Some(Output {
        status: status.unwrap(),
        stdout: out,
        stderr: err,
    })
}

/// 给 PowerShell 脚本前置 UTF-8 输出编码设置。
///
/// **为什么需要**：Windows PowerShell 5.1 按控制台代码页写 stdout，简中系统是 GBK（936）；
/// 我们按 UTF-8 解码，`C:\Users\丁昊\…` 这类非 ASCII 路径会变成替换字符 `U+FFFD`，
/// 之后再拿这个路径 spawn 只会得到 `os error 3`（系统找不到指定的路径）。
/// 把 `[Console]::OutputEncoding` 改成 UTF-8 后，PowerShell 自己的输出流与它转发的原生
/// 命令输出都按 UTF-8 编码，与 Rust 侧的 `String::from_utf8_lossy` 对齐。
///
/// 用 `try/catch` 包住：极少数环境（无控制台句柄）设不上编码，此时宁可维持旧行为，
/// 也不能让整条探测脚本因为一行前置语句失败而不返回任何结果。
pub(crate) fn utf8_script(script: &str) -> String {
    format!("try {{ [Console]::OutputEncoding = [Text.Encoding]::UTF8 }} catch {{}}; {script}")
}

/// 关闭后的等待集合：只等「仍持有可见顶层窗口」的 PID。
///
/// **为什么可以收窄**：主进程负责把状态写回 `state.vscdb`（Electron 主进程持有全部
/// 内存态），语言服务 / GPU / 扩展宿主这类子进程不参与落盘。用户关窗口后主进程退出，
/// 库写入就已经安全；此时若还坚持等那些后台进程退出，只会把「已经关掉了」误判成
/// 「等待退出超时」，正是本次要修的两个故障之一。
///
/// 退化规则（宁可多等，不能误判成功）：
/// - `window_pids` 为空（进程都在后台 / 窗口还没创建出来）→ 等全部 `pids`（旧行为）；
/// - 交集为空（调用方给的集合对不上）→ 同样等全部 `pids`。
pub(crate) fn close_wait_targets(pids: &[u32], window_pids: &[u32]) -> Vec<u32> {
    if pids.is_empty() {
        return Vec::new();
    }
    if window_pids.is_empty() {
        return pids.to_vec();
    }
    let mut targets: Vec<u32> = Vec::new();
    for pid in pids {
        if window_pids.contains(pid) && !targets.contains(pid) {
            targets.push(*pid);
        }
    }
    if targets.is_empty() {
        return pids.to_vec();
    }
    targets
}

/// 允许 `/F` 强杀的 PID：`pids` 里**当前仍没有可见窗口**的那些。
///
/// **硬约束**：有可见窗口的进程正在走自己的退出流程（可能正弹着保存提示），对它强杀会
/// 丢掉用户未保存的编辑。这类进程一律只投 `SC_CLOSE`，并且宁可超时报错（文案点名残留
/// PID + 窗口标题，由用户自己决定怎么处理），绝不 `/F`。
///
/// 语义与边界：
/// - 保持入参顺序、去重；
/// - `window_pids` 中不属于 `pids` 的 PID 一律忽略（不越权关闭别的进程）；
/// - `window_pids` 为空 = 这批进程都没有可见窗口（例如 CodeBuddy CLI）→ 全部可强杀；
/// - 崩溃后 PID 被复用时，新进程若持有可见窗口就出现在 `window_pids` 里 → 不会被强杀。
///
/// 四条 Windows 关闭路径（WorkBuddy / CodeBuddy IDE / CodeBuddy CN IDE / CodeBuddy CLI）
/// 共用这一份判定，不要各写一遍「pids - window_pids」。
pub(crate) fn force_kill_targets(pids: &[u32], window_pids: &[u32]) -> Vec<u32> {
    let mut targets: Vec<u32> = Vec::new();
    for pid in pids {
        if window_pids.contains(pid) || targets.contains(pid) {
            continue;
        }
        targets.push(*pid);
    }
    targets
}

/// 超时文案里最多点名的窗口数：再多用户也读不完，且说明不是单一卡点。
pub(crate) const MAX_NAMED_WINDOW_TITLES: usize = 3;
/// 单个窗口标题的字符上限：标题常带完整文件路径，原样拼进界面会把提示撑爆。
pub(crate) const MAX_WINDOW_TITLE_CHARS: usize = 80;

/// 归一化窗口标题：去首尾空白 → 丢掉空串 → 按**字符**截断到 [`MAX_WINDOW_TITLE_CHARS`]
/// → 去重 → 最多 [`MAX_NAMED_WINDOW_TITLES`] 个。
///
/// 纯逻辑，便于单测覆盖「重复 / 空白 / 超长 / 超量」四种脏数据；Win32 取标题本身无法单测。
pub(crate) fn named_window_titles(titles: &[String]) -> Vec<String> {
    let mut named: Vec<String> = Vec::new();
    for title in titles {
        let title = title.trim();
        if title.is_empty() {
            continue;
        }
        let short: String = title.chars().take(MAX_WINDOW_TITLE_CHARS).collect();
        if named.contains(&short) {
            continue;
        }
        named.push(short);
        if named.len() >= MAX_NAMED_WINDOW_TITLES {
            break;
        }
    }
    named
}

/// 「窗口: 「a」、「b」」；没有可用标题时返回 `None`（调用方必须能退化成只报 PID）。
pub(crate) fn window_titles_clause(titles: &[String]) -> Option<String> {
    let named = named_window_titles(titles);
    if named.is_empty() {
        return None;
    }
    let windows = named
        .iter()
        .map(|title| format!("「{title}」"))
        .collect::<Vec<String>>()
        .join("、");
    Some(format!("窗口: {windows}"))
}

/// Windows 关闭路径共用的超时文案：点名残留 PID，拿得到窗口标题时一并列出。
///
/// 例：`WorkBuddy 进程无法关闭（残留 PID: 1234, 5678；窗口: 「main.ts - 项目」）。请手动结束这些进程，处理完可重试。`
///
/// **为什么点名 PID**：`请手动结束 WorkBuddy 进程` 这类泛化文案把用户丢进任务管理器
/// 自己找；残留 PID（+ 可读的窗口标题）直接告诉他该结束哪一个。文案只用于 Windows
/// （「任务管理器」是 Windows 说法），macOS / Linux 的超时文案各自列 PID，不要混用。
pub(crate) fn close_timeout_error_message(
    product: &str,
    alive: &[u32],
    titles: &[String],
) -> String {
    let pids = alive
        .iter()
        .map(u32::to_string)
        .collect::<Vec<String>>()
        .join(", ");
    let windows = window_titles_clause(titles)
        .map(|clause| format!("；{clause}"))
        .unwrap_or_default();
    format!(
        "{product} 进程无法关闭（残留 PID: {pids}{windows}）。请手动结束这些进程，处理完可重试。"
    )
}

fn image_stem(name: &str) -> &str {
    let name = name.trim();
    if name.len() >= 4 && name[name.len() - 4..].eq_ignore_ascii_case(".exe") {
        name[..name.len() - 4].trim()
    } else {
        name
    }
}

/// Windows 路径可能含 `\`；在非 Windows 上 `Path::file_name` 不会按 `\` 切分。
fn image_name_from_path_str(s: &str) -> &str {
    s.rsplit(['\\', '/']).next().unwrap_or(s).trim()
}

/// 本工具自身的映像名（忽略 .exe、大小写）。
///
/// 取**实际产出的两个二进制名**：CLI/服务端 `crates/wb-shift-server` 的 bin 名
/// `workbuddy-shift`，以及桌面端 `src-tauri` 的包名 `workbuddy-shift-desktop`。
pub(crate) fn is_self_image_name(name: &str) -> bool {
    let stem = image_stem(image_name_from_path_str(name));
    stem.eq_ignore_ascii_case("workbuddy-shift")
        || stem.eq_ignore_ascii_case("workbuddy-shift-desktop")
}

pub(crate) fn is_crashpad_helper_name(name: &str) -> bool {
    image_name_from_path_str(name)
        .to_ascii_lowercase()
        .contains("crashpad_handler")
}

/// WorkBuddy 是否在运行（按档位判定，两档位互不命中）。
pub fn is_workbuddy_running(variant: WbVariant) -> bool {
    #[cfg(target_os = "macos")]
    {
        // footer 语义 = GUI 主进程在运行：仅包内守护子进程存活不算运行中。
        // 使用解析出的主进程模式，解析失败回退字面量。
        let resolved = macos_workbuddy_app_path_resolved(variant);
        let patterns = macos_main_patterns(resolved.as_deref(), variant);
        !macos_pids_by_patterns(&patterns).is_empty()
    }
    #[cfg(target_os = "windows")]
    {
        !windows_workbuddy_process_rows(variant).is_empty()
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let pattern = variant.linux_process_pattern();
        match cmd_builder("pgrep").args(["-f", pattern]).output() {
            Ok(out) => out.status.success() && !out.stdout.is_empty(),
            Err(_) => false,
        }
    }
}

/// 轮询等待 WorkBuddy 进程全部退出，返回是否已退出。对照 `_wait_process_gone`。
pub fn wait_process_gone(variant: WbVariant, timeout_secs: f64) -> bool {
    let deadline = Instant::now() + Duration::from_secs_f64(timeout_secs);
    while Instant::now() < deadline {
        if !is_workbuddy_running(variant) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    !is_workbuddy_running(variant)
}

/// 关闭 WorkBuddy：优雅退出 → 超时后强杀 → 确认进程消失。对照 `close_workbuddy`。
pub fn close_workbuddy(variant: WbVariant, timeout_secs: i64) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        close_workbuddy_windows(variant, timeout_secs)
    }
    #[cfg(target_os = "macos")]
    {
        close_workbuddy_macos(variant, timeout_secs)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        if !is_workbuddy_running(variant) {
            return Ok(());
        }
        let pattern = variant.linux_process_pattern();
        let _ = run_cmd_timeout("pkill", &["-15", "-f", pattern], 10);
        if wait_process_gone(variant, (timeout_secs.min(5)) as f64) {
            return Ok(());
        }
        let _ = run_cmd_timeout("pkill", &["-9", "-f", pattern], 10);
        if wait_process_gone(variant, timeout_secs as f64) {
            return Ok(());
        }
        Err("WorkBuddy 进程无法关闭，请手动结束 workbuddy 进程".to_string())
    }
}

/// 启动 WorkBuddy。失败返回可读错误。对照 `launch_workbuddy`。
///
/// progress 可选：macOS 启动与存活确认阶段推送心跳（避免界面长时间静默）；
/// Windows/Linux 分支忽略该参数。
pub fn launch_workbuddy(variant: WbVariant, progress: Option<&dyn Fn(&str)>) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        launch_workbuddy_macos(variant, progress)
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = progress;
        let app = auth_file::workbuddy_app_path(variant);
        #[cfg(target_os = "windows")]
        {
            let default_exe = windows_exe_names(variant)[0].clone();
            let exe = if app
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("exe"))
            {
                app.clone()
            } else {
                app.join(default_exe)
            };
            if !exe.exists() {
                return Err(format!(
                    "未找到 WorkBuddy 程序（尝试路径: {}）。请在 Windows 上打开 WorkBuddy 后重试。",
                    exe.display()
                ));
            }
            persist_workbuddy_exe(&exe, variant);
            cmd_builder(&exe)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|e| format!("启动 WorkBuddy 失败: {e}（路径: {}）", exe.display()))?;
            Ok(())
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            let _ = cmd_builder(&app)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn();
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// PowerShell 脚本前置 UTF-8 输出编码：必须是脚本的一部分（同一条 `-Command`），
    /// 且原脚本内容一字不改地跟在后面。
    #[test]
    fn utf8_script_prepends_output_encoding_and_keeps_script() {
        let script = "Get-CimInstance Win32_Process | ForEach-Object { $_.ExecutablePath }";
        let wrapped = utf8_script(script);
        assert!(
            wrapped.contains("[Console]::OutputEncoding = [Text.Encoding]::UTF8"),
            "{wrapped}"
        );
        // 设不上编码时不能让整条脚本失败：必须被 try/catch 包住。
        assert!(wrapped.contains("try {"), "{wrapped}");
        assert!(wrapped.contains("catch {}"), "{wrapped}");
        assert!(wrapped.ends_with(script), "{wrapped}");
        assert_eq!(
            wrapped,
            format!(
                "try {{ [Console]::OutputEncoding = [Text.Encoding]::UTF8 }} catch {{}}; {script}"
            )
        );
    }

    /// 等待集合收窄：有窗口的 pid 全退出即算关闭完成；一个都没有 / 对不上时退回全部 pid
    /// （宁可多等，也不能把「其实还开着」误判成「已关闭」）。
    #[test]
    fn close_wait_targets_narrows_to_windowed_pids_without_false_success() {
        // 主进程 1001 有可见窗口，语言服务 1002 / GPU 1003 没有 → 只等 1001。
        assert_eq!(close_wait_targets(&[1001, 1002, 1003], &[1001]), vec![1001]);
        // 多个窗口进程都要等（多开窗口时每个窗口的 pid 都可能有窗口）。
        assert_eq!(
            close_wait_targets(&[1001, 1002], &[1001, 1002]),
            vec![1001, 1002]
        );
        // 全部在后台（没有任何可见窗口）→ 退回旧行为，等全部。
        assert_eq!(close_wait_targets(&[1001, 1002], &[]), vec![1001, 1002]);
        // 窗口集合与 pid 集合对不上（调用方算错 / 进程刚好换代）→ 等全部，不误判成功。
        assert_eq!(close_wait_targets(&[1001], &[9999]), vec![1001]);
        // 空 pid → 空集合（调用方据此直接返回 Ok）。
        assert!(close_wait_targets(&[], &[1001]).is_empty());
    }

    /// 强杀集合 = 残留中「当前仍无可见窗口」的那些：有窗口的一律排除（硬约束，
    /// 强杀它们会丢用户未保存的编辑），顺序保持、去重、不越权。
    #[test]
    fn force_kill_targets_excludes_windowed_pids() {
        // 主进程 1001 有窗口、子进程 1002 / 1003 没有 → 只能强杀 1002 / 1003。
        assert_eq!(
            force_kill_targets(&[1001, 1002, 1003], &[1001]),
            vec![1002, 1003]
        );
        // 全都没有可见窗口（CLI / 后台启动）→ 全部可强杀。
        assert_eq!(force_kill_targets(&[1001, 1002], &[]), vec![1001, 1002]);
        // 全都有可见窗口 → 一个都不许强杀（宁可超时报错）。
        assert!(force_kill_targets(&[1001, 1002], &[1001, 1002]).is_empty());
        // 空集入参 → 空集。
        assert!(force_kill_targets(&[], &[1001]).is_empty());
        assert!(force_kill_targets(&[], &[]).is_empty());
        // 重复 pid 只强杀一次。
        assert_eq!(force_kill_targets(&[1002, 1002], &[]), vec![1002]);
        // 换代 pid：`window_pids` 里多出来的 PID 不返回（不越权关闭别的进程）；
        // 入参里的 PID 只要不在 `window_pids` 中就算「无窗口」，所以调用方必须传**当前**
        // 的可见窗口集合（残留 pid 要重新探测，不能复用关闭前的快照）。
        assert_eq!(
            force_kill_targets(&[1002], &[9999]),
            vec![1002],
            "窗口集合里不属于入参的 PID 被忽略"
        );
    }

    /// 超时文案点名残留 PID，并在拿得到标题时把窗口一并列出来。
    #[test]
    fn close_timeout_error_message_names_pids_and_windows() {
        let titles = vec![
            "main.ts - 项目 - WorkBuddy".to_string(),
            // 重复 / 空白标题必须折叠，否则文案会变成噪音。
            "main.ts - 项目 - WorkBuddy".to_string(),
            "   ".to_string(),
        ];
        let message = close_timeout_error_message("WorkBuddy", &[4242, 4243], &titles);
        assert!(message.contains("4242, 4243"), "{message}");
        assert!(message.contains("main.ts - 项目 - WorkBuddy"), "{message}");
        assert_eq!(
            message.matches("main.ts - 项目").count(),
            1,
            "重复标题只点名一次: {message}"
        );
        assert!(message.contains("处理完可重试"), "{message}");
        assert!(!message.contains("「   」"), "空白标题不得出现: {message}");

        // 拿不到标题 → 退化到只报 PID，不能出现空的「窗口: 」。
        let bare = close_timeout_error_message("WorkBuddy", &[4242], &[]);
        assert!(bare.contains("残留 PID: 4242"), "{bare}");
        assert!(!bare.contains("窗口"), "{bare}");
        assert_eq!(
            bare,
            close_timeout_error_message("WorkBuddy", &[4242], &[String::new()])
        );
    }

    /// 标题超长（含完整路径）时截断到字符上限，不把界面撑爆；最多点名 3 个窗口。
    #[test]
    fn close_timeout_error_message_truncates_and_caps_titles() {
        let long = "a".repeat(500);
        let message = close_timeout_error_message("CodeBuddy", &[1], &[long]);
        assert!(
            message.contains(&"a".repeat(MAX_WINDOW_TITLE_CHARS)),
            "{message}"
        );
        assert!(
            !message.contains(&"a".repeat(MAX_WINDOW_TITLE_CHARS + 1)),
            "{message}"
        );

        let many: Vec<String> = (0..6).map(|i| format!("窗口{i}")).collect();
        let message = close_timeout_error_message("CodeBuddy", &[1], &many);
        assert!(message.contains("窗口0"), "{message}");
        assert!(message.contains("窗口2"), "{message}");
        assert!(!message.contains("窗口3"), "最多点名 3 个: {message}");
    }

    /// 窗口标题归一化的纯逻辑边界：按字符（不是字节）截断，中文标题不会被切坏。
    #[test]
    fn named_window_titles_truncates_on_char_boundaries() {
        let chinese = "视".repeat(MAX_WINDOW_TITLE_CHARS + 10);
        let named = named_window_titles(&[chinese]);
        assert_eq!(named.len(), 1);
        assert_eq!(named[0].chars().count(), MAX_WINDOW_TITLE_CHARS);
        assert!(named[0].is_char_boundary(named[0].len()));
        // 只有空白的集合 → 没有可点名标题。
        assert!(named_window_titles(&[" ".to_string(), "\t".to_string()]).is_empty());
        assert!(window_titles_clause(&[" ".to_string()]).is_none());
        assert_eq!(
            window_titles_clause(&["a".to_string(), "b".to_string()]),
            Some("窗口: 「a」、「b」".to_string())
        );
    }
}
