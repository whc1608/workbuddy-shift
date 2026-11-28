//! CodeBuddy 国际版 IDE（桌面客户端）账号切换。
//!
//! 复用 WorkBuddy AI 账号库中的 token，写入
//! `~/Library/Application Support/CodeBuddy/.../state.vscdb` 的 Safe Storage
//! secret，并可选重启 CodeBuddy。与 CodeBuddy CN、CodeBuddy CLI 完全独立。

use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::modules::account::{self, get_str};
#[cfg(target_os = "macos")]
use crate::modules::config::home_dir;
use crate::modules::config::{
    atomic_write, clear_codebuddy_ide_app_cache, load_codebuddy_ide_app_cache, now_ms,
    save_codebuddy_ide_app_cache, store_dir,
};
use crate::modules::variant::{codebuddy_domain_for, WbVariant};
// 复用 process 模块带并发管道读取的正确实现；本地轮询版会在子进程输出
// 超过 64KB（如 `ps -axo pid=,args=`）时因管道写满而死锁到超时。
use crate::modules::process;
// Windows 的关闭路径改走 `process::TaskKillGuard`（含「别把自己杀掉」的保护），不再直接用别名。
#[cfg(not(target_os = "windows"))]
use crate::modules::process::run_cmd_timeout as run_cmd;
use crate::modules::vscode_cn_inject::{
    codebuddy_ide_data_dir, has_secret_row_for, inject_codebuddy_ide_secret,
    read_codebuddy_ide_secret, CodeBuddyIdeFlavor, CODEBUDDY_INTL_TARGET,
};
use crate::modules::vscode_session::is_safe_uid;

fn intl_data_dir() -> Option<PathBuf> {
    codebuddy_ide_data_dir(CodeBuddyIdeFlavor::Intl)
}

fn intl_state_db_path() -> Option<PathBuf> {
    intl_data_dir().map(|d| d.join("User").join("globalStorage").join("state.vscdb"))
}

fn inject_intl_secret(plaintext: &str, user_data_dir: Option<&Path>) -> Result<PathBuf, String> {
    inject_codebuddy_ide_secret(CodeBuddyIdeFlavor::Intl, plaintext, user_data_dir)
}

fn read_intl_secret(user_data_dir: Option<&Path>) -> Result<Option<String>, String> {
    read_codebuddy_ide_secret(CodeBuddyIdeFlavor::Intl, user_data_dir)
}

const STATE_FILE: &str = "codebuddy_ide.json";
#[cfg(target_os = "macos")]
const MACOS_BUNDLE_ID: &str = "com.tencent.codebuddy";
#[cfg(target_os = "macos")]
const MACOS_APP_NAME: &str = "CodeBuddy.app";

fn state_path() -> PathBuf {
    store_dir().join(STATE_FILE)
}

fn load_state() -> Value {
    std::fs::read_to_string(state_path())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| json!({}))
}

fn save_state(state: &Value) -> Result<(), String> {
    let path = state_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let content = serde_json::to_string_pretty(state).map_err(|e| e.to_string())?;
    atomic_write(&path, &content).map_err(|e| e.to_string())
}

fn set_active_account_id(account_id: &str) -> Result<(), String> {
    let mut state = load_state();
    if let Some(obj) = state.as_object_mut() {
        obj.insert("activeAccountId".to_string(), json!(account_id));
        obj.insert("updatedAt".to_string(), json!(now_ms()));
    }
    save_state(&state)
}

fn active_account_id_from_state() -> Option<String> {
    load_state()
        .get("activeAccountId")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn windows_image_stem(name: &str) -> &str {
    let file = name.rsplit(['\\', '/']).next().unwrap_or(name).trim();
    if file.len() >= 4 && file[file.len() - 4..].eq_ignore_ascii_case(".exe") {
        file[..file.len() - 4].trim()
    } else {
        file
    }
}

/// 精确映像名：`CodeBuddy`（忽略 .exe / 路径 / 大小写）。
#[cfg_attr(not(any(test, target_os = "windows")), allow(dead_code))]
fn is_codebuddy_ide_image_name(name: &str) -> bool {
    windows_image_stem(name).eq_ignore_ascii_case("CodeBuddy")
}

fn is_plain_codebuddy_image_name(name: &str) -> bool {
    windows_image_stem(name).eq_ignore_ascii_case("CodeBuddy")
}

/// 路径是否含独立目录分量 `CodeBuddy CN`（国内 IDE，不是国际版 `CodeBuddy`）。
fn path_contains_codebuddy_cn_dir(path: &str) -> bool {
    path.split(['\\', '/'])
        .any(|part| part.eq_ignore_ascii_case("CodeBuddy CN"))
}

/// Windows 国际 IDE：`CodeBuddy.exe`，且路径不含 `CodeBuddy CN`。
#[cfg_attr(not(any(test, target_os = "windows")), allow(dead_code))]
fn is_codebuddy_ide_windows_exe(path: &str) -> bool {
    if path_contains_codebuddy_cn_dir(path) {
        return false;
    }
    is_codebuddy_ide_image_name(path) || is_plain_codebuddy_image_name(path)
}

fn persist_ide_app_cache(path: &Path) {
    if load_codebuddy_ide_app_cache().as_deref() == Some(path) {
        return;
    }
    let _ = save_codebuddy_ide_app_cache(path);
}

#[cfg(target_os = "macos")]
fn macos_ide_app_candidates(home: &Path) -> Vec<PathBuf> {
    vec![
        PathBuf::from("/Applications").join(MACOS_APP_NAME),
        home.join("Applications").join(MACOS_APP_NAME),
    ]
}

#[cfg(target_os = "macos")]
fn macos_ide_main_patterns(resolved_app: Option<&Path>) -> Vec<String> {
    match resolved_app {
        Some(app) => vec![format!("{}/Contents/MacOS", app.display())],
        None => vec!["CodeBuddy.app/Contents/MacOS".to_string()],
    }
}

#[cfg(target_os = "macos")]
fn macos_ide_bundle_patterns(resolved_app: Option<&Path>) -> Vec<String> {
    match resolved_app {
        Some(app) => vec![app.display().to_string()],
        None => vec!["CodeBuddy.app".to_string()],
    }
}

#[cfg(target_os = "macos")]
fn macos_ide_running_app_path() -> Option<PathBuf> {
    let patterns = macos_ide_main_patterns(None);
    for (_pid, args) in process::macos_rows_by_patterns(&patterns) {
        if let Some(p) = process::extract_app_bundle_from_args(&args) {
            if process::is_app_bundle(&p) {
                return Some(p);
            }
        }
    }
    None
}

#[cfg(target_os = "macos")]
fn macos_ide_mdfind_app_path() -> Option<PathBuf> {
    let query = format!("kMDItemCFBundleIdentifier == '{MACOS_BUNDLE_ID}'c");
    let out = run_cmd("mdfind", &[query.as_str()], 5)?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(PathBuf::from)
}

#[cfg(target_os = "macos")]
fn macos_ide_app_path_resolved() -> Option<PathBuf> {
    if let Some(p) = macos_ide_running_app_path() {
        persist_ide_app_cache(&p);
        return Some(p);
    }
    if let Some(cached) = load_codebuddy_ide_app_cache() {
        if process::is_app_bundle(&cached) {
            return Some(cached);
        }
        clear_codebuddy_ide_app_cache();
    }
    for p in macos_ide_app_candidates(&home_dir()) {
        if process::is_app_bundle(&p) {
            persist_ide_app_cache(&p);
            return Some(p);
        }
    }
    if let Some(p) = macos_ide_mdfind_app_path() {
        if process::is_app_bundle(&p) {
            persist_ide_app_cache(&p);
            return Some(p);
        }
    }
    None
}

#[cfg_attr(not(any(test, target_os = "windows")), allow(dead_code))]
fn windows_ide_fallback_exe_candidates(
    local_appdata: Option<&str>,
    program_files: Option<&str>,
    program_files_x86: Option<&str>,
    username: Option<&str>,
    drives: &[char],
) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut push = |p: PathBuf| {
        if !out.iter().any(|e| e == &p) {
            out.push(p);
        }
    };
    let folders = ["CodeBuddy"];
    let exe_names = ["CodeBuddy.exe", "CodeBuddy.exe"];
    let mut push_install = |base: PathBuf| {
        for folder in folders {
            for exe in exe_names {
                push(base.join(folder).join(exe));
            }
        }
    };
    if let Some(local) = local_appdata.map(str::trim).filter(|s| !s.is_empty()) {
        push_install(PathBuf::from(local).join("Programs"));
    }
    if let Some(pf) = program_files.map(str::trim).filter(|s| !s.is_empty()) {
        push_install(PathBuf::from(pf));
    }
    if let Some(pf86) = program_files_x86.map(str::trim).filter(|s| !s.is_empty()) {
        push_install(PathBuf::from(pf86));
    }
    let user = username.map(str::trim).filter(|s| !s.is_empty());
    for drive in drives {
        let letter = drive.to_ascii_uppercase();
        if !letter.is_ascii_alphabetic() {
            continue;
        }
        let root = format!("{letter}:");
        if let Some(user) = user {
            push_install(
                PathBuf::from(&root)
                    .join("Users")
                    .join(user)
                    .join("AppData")
                    .join("Local")
                    .join("Programs"),
            );
        }
        push_install(PathBuf::from(&root).join("Program Files"));
    }
    out
}

#[cfg_attr(not(any(test, target_os = "windows")), allow(dead_code))]
fn keep_windows_ide_row(row: &process::WindowsProcessRow) -> bool {
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
    if path_contains_codebuddy_cn_dir(&path_s) {
        return false;
    }
    is_codebuddy_ide_windows_exe(&row.name)
        || is_codebuddy_ide_windows_exe(file_name)
        || (!path_s.is_empty() && is_codebuddy_ide_windows_exe(&path_s))
}

#[cfg(target_os = "windows")]
fn is_existing_ide_exe(path: &Path) -> bool {
    path.is_file() && is_codebuddy_ide_windows_exe(&path.to_string_lossy())
}

#[cfg(target_os = "windows")]
fn windows_ide_cim_process_script() -> &'static str {
    "Get-CimInstance Win32_Process -ErrorAction SilentlyContinue | \
         Where-Object { $_.Name -eq 'CodeBuddy.exe' -or $_.Name -eq 'CodeBuddy.exe' } | \
         ForEach-Object { '{0}|{1}|{2}' -f $_.ProcessId, $_.Name, $_.ExecutablePath }"
}

#[cfg(target_os = "windows")]
fn windows_ide_process_rows() -> Vec<process::WindowsProcessRow> {
    let self_pid = std::process::id();
    if let Some(stdout) = process::ps_output(windows_ide_cim_process_script(), 5) {
        let rows: Vec<_> = process::parse_windows_process_rows(&stdout)
            .into_iter()
            .filter(|row| row.pid != self_pid && keep_windows_ide_row(row))
            .collect();
        if !rows.is_empty() {
            return rows;
        }
    }
    let mut rows = Vec::new();
    rows.extend(process::windows_tasklist_image_rows("CodeBuddy.exe"));
    rows.extend(process::windows_tasklist_image_rows("CodeBuddy.exe"));
    rows.into_iter()
        .filter(|row| row.pid != self_pid && keep_windows_ide_row(row))
        .collect()
}

#[cfg(target_os = "windows")]
fn windows_ide_running_exe() -> Option<PathBuf> {
    let stdout = process::ps_output(windows_ide_cim_process_script(), 5)?;
    for row in process::parse_windows_process_rows(&stdout) {
        if !keep_windows_ide_row(&row) {
            continue;
        }
        if let Some(p) = row.exe_path {
            if is_existing_ide_exe(&p) {
                return Some(p);
            }
        }
    }
    None
}

#[cfg(target_os = "windows")]
fn windows_ide_registry_exe_candidates() -> Vec<PathBuf> {
    let script = r#"
$ErrorActionPreference = 'SilentlyContinue'
$out = @()
$names = @('CodeBuddy.exe', 'CodeBuddy.exe')
$appHives = @(
  'HKCU:\SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths',
  'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths',
  'HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\App Paths'
)
foreach ($hive in $appHives) {
  foreach ($n in $names) {
    $key = Join-Path $hive $n
    $props = Get-ItemProperty -LiteralPath $key
    if ($props) {
      $def = $props.'(default)'
      if ($def) { $out += [string]$def }
      if ($props.Path) { $out += [string](Join-Path $props.Path $n) }
    }
  }
}
$unHives = @(
  'HKCU:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall',
  'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall',
  'HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall'
)
foreach ($hive in $unHives) {
  Get-ChildItem -LiteralPath $hive | ForEach-Object {
    $dn = $_.GetValue('DisplayName')
    if (-not $dn) { return }
    $dnl = [string]$dn
    if ($dnl -match 'workbuddy-shift') { return }
    if ($dnl -match 'CodeBuddy CN') { return }
    if ($dnl -notmatch 'CodeBuddy') { return }
    $icon = $_.GetValue('DisplayIcon')
    if ($icon) { $out += [string]$icon }
    $loc = $_.GetValue('InstallLocation')
    if ($loc) {
      $out += [string](Join-Path $loc 'CodeBuddy.exe')
      $out += [string](Join-Path $loc 'CodeBuddy.exe')
    }
  }
}
$out | ForEach-Object { $_ }
"#;
    let Some(stdout) = process::ps_output(script, 8) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for line in stdout.lines() {
        let Some(parsed) = process::parse_windows_display_icon(line) else {
            continue;
        };
        if process::is_self_image_name(&parsed) {
            continue;
        }
        if !is_codebuddy_ide_windows_exe(&parsed) {
            continue;
        }
        let pb = PathBuf::from(parsed);
        if !out.iter().any(|e| e == &pb) {
            out.push(pb);
        }
    }
    out
}

#[cfg(target_os = "windows")]
fn windows_ide_exe_path_resolved() -> Option<PathBuf> {
    if let Some(p) = windows_ide_running_exe() {
        persist_ide_app_cache(&p);
        return Some(p);
    }
    if let Some(cached) = load_codebuddy_ide_app_cache() {
        if is_existing_ide_exe(&cached) {
            return Some(cached);
        }
        clear_codebuddy_ide_app_cache();
    }
    for p in windows_ide_registry_exe_candidates() {
        if is_existing_ide_exe(&p) {
            persist_ide_app_cache(&p);
            return Some(p);
        }
    }
    let local = std::env::var("LOCALAPPDATA").ok();
    let pf = std::env::var("PROGRAMFILES").ok();
    let pf86 = std::env::var("PROGRAMFILES(X86)").ok();
    let user = std::env::var("USERNAME").ok();
    let drives = process::existing_windows_drives();
    for p in windows_ide_fallback_exe_candidates(
        local.as_deref(),
        pf.as_deref(),
        pf86.as_deref(),
        user.as_deref(),
        &drives,
    ) {
        if is_existing_ide_exe(&p) {
            persist_ide_app_cache(&p);
            return Some(p);
        }
    }
    None
}

mod detect;
mod status;
mod switch;

pub use detect::{active_intl_ide_uid, build_session_json, detect_current_account};
pub use status::{codebuddy_ide_app_path, is_codebuddy_ide_running, status};
pub use switch::{close_codebuddy_ide, launch_codebuddy_ide, switch_account};
#[allow(unused_imports)]
pub(crate) use switch::{inject_session_and_finish, validate_switch_target};

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(unused_imports)]
    use super::detect::*;
    #[allow(unused_imports)]
    use super::status::*;
    #[allow(unused_imports)]
    use super::switch::*;
    include!("tests/switch.rs");
}
