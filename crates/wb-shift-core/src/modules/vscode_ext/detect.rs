//! 扩展/IDE 运行检测：Windows/macOS/Linux 进程探测、当前账号与扩展 uid 识别。

#[allow(unused_imports)]
use super::status::*;
#[allow(unused_imports)]
use super::switch::*;
use super::*;

pub(super) fn windows_image_stem(name: &str) -> &str {
    let file = name.rsplit(['\\', '/']).next().unwrap_or(name).trim();
    if file.len() >= 4 && file[file.len() - 4..].eq_ignore_ascii_case(".exe") {
        file[..file.len() - 4].trim()
    } else {
        file
    }
}

/// 精确映像名：`Code`（忽略 .exe / 路径 / 大小写）。
///
/// 严格排除 `Code - Insiders`、`CodeBuddy CN`、`CodeBuddy` 等其它变体。
#[cfg_attr(not(any(test, target_os = "windows")), allow(dead_code))]
pub(super) fn is_code_image_name(name: &str) -> bool {
    windows_image_stem(name).eq_ignore_ascii_case("Code")
}

#[cfg_attr(not(any(test, target_os = "windows")), allow(dead_code))]
pub(super) fn keep_windows_code_row(row: &process::WindowsProcessRow) -> bool {
    let path_s = row
        .exe_path
        .as_ref()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let file_name = path_s.rsplit(['\\', '/']).next().unwrap_or("").trim();
    if process::is_self_image_name(&row.name) || process::is_self_image_name(file_name) {
        return false;
    }
    if process::is_crashpad_helper_name(&row.name) || process::is_crashpad_helper_name(file_name) {
        return false;
    }
    is_code_image_name(&row.name) || is_code_image_name(file_name)
}

#[cfg(target_os = "windows")]
pub(super) fn windows_code_cim_process_script() -> &'static str {
    "Get-CimInstance Win32_Process -ErrorAction SilentlyContinue | \
         Where-Object { $_.Name -eq 'Code.exe' } | \
         ForEach-Object { '{0}|{1}|{2}' -f $_.ProcessId, $_.Name, $_.ExecutablePath }"
}

#[cfg(target_os = "windows")]
pub(super) fn windows_code_process_rows() -> Vec<process::WindowsProcessRow> {
    let self_pid = std::process::id();
    if let Some(stdout) = process::ps_output(windows_code_cim_process_script(), 5) {
        let rows: Vec<_> = process::parse_windows_process_rows(&stdout)
            .into_iter()
            .filter(|row| row.pid != self_pid && keep_windows_code_row(row))
            .collect();
        if !rows.is_empty() {
            return rows;
        }
    }
    process::windows_tasklist_image_rows("Code.exe")
        .into_iter()
        .filter(|row| row.pid != self_pid && keep_windows_code_row(row))
        .collect()
}

#[cfg(target_os = "macos")]
pub(super) fn macos_code_main_patterns() -> Vec<String> {
    vec!["Visual Studio Code.app/Contents/MacOS".to_string()]
}

#[cfg_attr(
    not(any(test, not(any(target_os = "macos", target_os = "windows")))),
    allow(dead_code)
)]
pub(super) fn linux_cmdline_is_code(cmdline: &str) -> bool {
    let lower = cmdline.to_ascii_lowercase();
    // 自排除：命令行含本工具产物名（`workbuddy-shift` 子串已涵盖
    // `workbuddy-shift-desktop`）即不是 VS Code 进程。
    if lower.contains("workbuddy-shift") {
        return false;
    }
    if lower.contains("crashpad") || lower.contains("--type=") {
        return false;
    }
    // 严格排除 Insiders / VSCodium / Cursor / Windsurf 等变体。
    if lower.contains("code-insiders")
        || lower.contains("vscodium")
        || lower.contains("codium")
        || lower.contains("cursor")
        || lower.contains("windsurf")
        || lower.contains("codebuddy")
    {
        return false;
    }
    // 主进程命令行形如 `/usr/share/code/code …`；辅助进程命令行不含主程序路径。
    lower.contains("/code/code")
        || lower.contains("/bin/code")
        || lower.contains("visual studio code")
}

#[cfg_attr(
    not(any(test, not(any(target_os = "macos", target_os = "windows")))),
    allow(dead_code)
)]
pub(super) fn linux_exe_is_code(exe: &std::path::Path) -> bool {
    let name = exe
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .trim();
    name.eq_ignore_ascii_case("code")
}

/// 枚举 Linux 上的 Code 进程：`(pid, 可执行文件路径)`。
///
/// 可执行文件路径用于关闭后按同一路径重新打开（不依赖 `code` CLI / PATH）；
/// 读不到 `/proc/<pid>/exe` 时该项为 `None`。
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub(super) fn linux_code_processes() -> Vec<(u32, Option<PathBuf>)> {
    let self_pid = std::process::id();
    let mut rows = Vec::new();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return rows;
    };
    for entry in entries.flatten() {
        let pid: u32 = match entry.file_name().to_string_lossy().parse() {
            Ok(p) => p,
            Err(_) => continue,
        };
        if pid == self_pid {
            continue;
        }
        let exe = std::fs::read_link(format!("/proc/{pid}/exe")).ok();
        if exe.as_deref().is_some_and(linux_exe_is_code) {
            rows.push((pid, exe));
            continue;
        }
        let cmdline = match std::fs::read(format!("/proc/{pid}/cmdline")) {
            Ok(bytes) if !bytes.is_empty() => String::from_utf8_lossy(&bytes).replace('\0', " "),
            _ => continue,
        };
        if linux_cmdline_is_code(&cmdline) {
            rows.push((pid, exe));
        }
    }
    rows
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub(super) fn linux_code_pids() -> Vec<u32> {
    linux_code_processes()
        .into_iter()
        .map(|(pid, _)| pid)
        .collect()
}

/// 从本机 VS Code 扩展读取当前 token；若能匹配账号库则返回匹配信息（不新建账号）。
pub fn detect_current_account() -> Result<Value, String> {
    let secret = read_secret_for(&VSCODE_TARGET, None).map_err(describe_secret_error)?;
    let Some(secret) = secret else {
        return Ok(json!({
            "ok": true,
            "found": false,
            "message": "本机 VS Code 未找到 CodeBuddy 插件登录 secret",
        }));
    };
    let Some((uid, token)) = parse_token_from_secret(&secret) else {
        return Err("本地 VS Code CodeBuddy 插件登录信息解析失败".to_string());
    };
    if let Some(acc) = match_account_for_token(uid.as_deref(), &token) {
        let id = get_str(&acc, "id").unwrap_or_default();
        let _ = set_active_account_id(&id);
        return Ok(json!({
            "ok": true,
            "found": true,
            "matched": true,
            "accountId": id,
            "account": account::account_meta(&acc),
            "uid": uid,
        }));
    }
    Ok(json!({
        "ok": true,
        "found": true,
        "matched": false,
        "uid": uid,
        "message": "本机 VS Code 已登录 CodeBuddy 插件，但账号库中无匹配账号；请用该账号重新 OAuth 扫码登录后再切换。",
    }))
}

/// 当前登录 VS Code CodeBuddy 扩展的账号 uid（用于定位可复制的会话目录）。
///
/// 优先从扩展登录 secret 解析 uid；不可用时回退到本地状态文件记录的账号 id → 账号库 uid。
/// 任一来源都拿不到时返回 `None`（调用方据此给出「未登录」空态）。
pub fn active_ext_uid() -> Option<String> {
    if let Ok(Some(secret)) = read_secret_for(&VSCODE_TARGET, None) {
        if let Some((Some(uid), _token)) = parse_token_from_secret(&secret) {
            let uid = uid.trim().to_string();
            if !uid.is_empty() {
                return Some(uid);
            }
        }
    }
    active_account_id_from_state()
        .and_then(|id| account::find_account(&id))
        .and_then(|acc| get_str(&acc, "uid"))
}
