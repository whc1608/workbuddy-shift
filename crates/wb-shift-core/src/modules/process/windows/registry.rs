//! 安装路径与注册表探测：候选 exe、注册表 probe 脚本、exe 路径解析与持久化。

use super::parsing::*;
use super::processes::*;
use super::*;

pub(super) fn windows_path(parts: &[&str]) -> PathBuf {
    PathBuf::from(parts.join("\\"))
}

/// 环境变量默认目录 + 盘符扫描候选（不访问文件系统，便于非 Windows 单测）。
pub(super) fn windows_fallback_exe_candidates(
    local_appdata: Option<&str>,
    program_files: Option<&str>,
    program_files_x86: Option<&str>,
    username: Option<&str>,
    drives: &[char],
    variant: WbVariant,
) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut push = |p: PathBuf| {
        if !out.iter().any(|e| e == &p) {
            out.push(p);
        }
    };
    let mut push_win_dir = |parts: &[&str]| {
        for exe in windows_exe_names(variant) {
            let mut wb = parts.to_vec();
            wb.push(&exe);
            push(windows_path(&wb));
        }
    };

    if let Some(local) = local_appdata.map(str::trim).filter(|s| !s.is_empty()) {
        for dir in variant.windows_image_names() {
            push_win_dir(&[local, "Programs", dir]);
        }
    }
    if let Some(pf) = program_files.map(str::trim).filter(|s| !s.is_empty()) {
        for dir in variant.windows_image_names() {
            push_win_dir(&[pf, dir]);
        }
    }
    if let Some(pf86) = program_files_x86.map(str::trim).filter(|s| !s.is_empty()) {
        for dir in variant.windows_image_names() {
            push_win_dir(&[pf86, dir]);
        }
    }

    let user = username.map(str::trim).filter(|s| !s.is_empty());
    for drive in drives {
        let letter = drive.to_ascii_uppercase();
        if !letter.is_ascii_alphabetic() {
            continue;
        }
        let root = format!("{letter}:");
        if let Some(user) = user {
            for dir in variant.windows_image_names() {
                push_win_dir(&[&root, "Users", user, "AppData", "Local", "Programs", dir]);
            }
        }
        for dir in variant.windows_image_names() {
            push_win_dir(&[&root, "Program Files", dir]);
        }
        // 便携版 / 自定义安装目录：常见做法是直接放在盘根（如 `D:\WorkBuddy\WorkBuddy.exe`），
        // 既不在 Program Files 也不在 LOCALAPPDATA。不覆盖这类布局时，按进程路径取不到
        // （例如客户端以管理员身份运行、当前进程读不到它的 Path）就彻底找不到 exe。
        for dir in variant.windows_image_names() {
            push_win_dir(&[&root, dir]);
        }
    }
    out
}

/// 路径最后一段必须是本档位的 WorkBuddy 映像名（忽略 .exe）。
pub(super) fn is_workbuddy_exe_file_name(path: &Path, variant: WbVariant) -> bool {
    let owned = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| image_name_from_path_str(&path.to_string_lossy()).to_string());
    is_workbuddy_image_name(&owned, variant)
}

#[cfg(target_os = "windows")]
pub(super) fn is_existing_workbuddy_exe(path: &Path, variant: WbVariant) -> bool {
    path.is_file() && is_workbuddy_exe_file_name(path, variant)
}

/// 记住已存在的 exe；缓存已是同一路径则不重复写。
#[cfg(target_os = "windows")]
pub(crate) fn persist_workbuddy_exe(path: &Path, variant: WbVariant) {
    if !is_existing_workbuddy_exe(path, variant) {
        return;
    }
    if config::load_workbuddy_exe_cache(variant).as_deref() == Some(path) {
        return;
    }
    let _ = config::save_workbuddy_exe_cache(variant, path);
}

/// 注册表探测脚本模板：`__APP_EXE_NAMES__` 替换为 `'X.exe','Y.exe'`，
/// `__DISPLAY_MATCH__` 替换为本档位映像名正则（Uninstall 项过滤）。
pub(super) const WINDOWS_REGISTRY_PROBE_TEMPLATE: &str = r#"
$ErrorActionPreference = 'SilentlyContinue'
$out = @()
$appNames = @(__APP_EXE_NAMES__)
$appHives = @(
  'HKCU:\SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths',
  'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths',
  'HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\App Paths'
)
foreach ($hive in $appHives) {
  foreach ($n in $appNames) {
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
    if ($dnl -match 'workbuddy-shift|workbuddy-shift-desktop') { return }
    if ($dnl -notmatch '__DISPLAY_MATCH__') { return }
    $icon = $_.GetValue('DisplayIcon')
    if ($icon) { $out += [string]$icon }
    $loc = $_.GetValue('InstallLocation')
    if ($loc) {
      foreach ($e in @(__APP_EXE_NAMES__)) { $out += [string](Join-Path $loc $e) }
    }
  }
}
$out | ForEach-Object { $_ }
"#;

/// 注册表探测脚本（App Paths + Uninstall，含 WOW6432Node）。
/// 按档位替换映像名与 DisplayName 匹配模式，避免跨档位互相发现。
pub(super) fn windows_registry_probe_script(variant: WbVariant) -> String {
    let quoted_exes: Vec<String> = windows_exe_names(variant)
        .iter()
        .map(|name| format!("'{name}'"))
        .collect();
    let app_names = quoted_exes.join(",");
    let display_match = variant.windows_image_names().join("|");
    WINDOWS_REGISTRY_PROBE_TEMPLATE
        .replace("__APP_EXE_NAMES__", &app_names)
        .replace("__DISPLAY_MATCH__", &display_match)
}

#[cfg(target_os = "windows")]
pub(super) fn windows_registry_exe_candidates(variant: WbVariant) -> Vec<PathBuf> {
    let Some(stdout) = ps_output(&windows_registry_probe_script(variant), 8) else {
        return Vec::new();
    };
    parse_windows_registry_path_lines(&stdout, variant)
}

/// Windows：动态查找 WorkBuddy 可执行文件路径。
///
/// 顺序：运行中进程 Path → 上次成功路径缓存 → 注册表 App Paths / Uninstall
/// （含 WOW6432Node、DisplayIcon）→ LOCALAPPDATA/Program Files 与各盘符常见目录。
/// 命中且文件存在则写入缓存；缓存指向丢失文件则丢弃。
#[cfg(target_os = "windows")]
pub fn windows_workbuddy_exe_path(variant: WbVariant) -> Option<PathBuf> {
    if let Some(p) = windows_running_workbuddy_exe(variant) {
        persist_workbuddy_exe(&p, variant);
        return Some(p);
    }

    if let Some(cached) = config::load_workbuddy_exe_cache(variant) {
        if is_existing_workbuddy_exe(&cached, variant) {
            return Some(cached);
        }
        config::clear_workbuddy_exe_cache(variant);
    }

    for p in windows_registry_exe_candidates(variant) {
        if is_existing_workbuddy_exe(&p, variant) {
            persist_workbuddy_exe(&p, variant);
            return Some(p);
        }
    }

    let local = std::env::var("LOCALAPPDATA").ok();
    let pf = std::env::var("PROGRAMFILES").ok();
    let pf86 = std::env::var("PROGRAMFILES(X86)").ok();
    let user = std::env::var("USERNAME").ok();
    let drives = existing_windows_drives();
    let candidates = windows_fallback_exe_candidates(
        local.as_deref(),
        pf.as_deref(),
        pf86.as_deref(),
        user.as_deref(),
        &drives,
        variant,
    );
    for p in candidates {
        if is_existing_workbuddy_exe(&p, variant) {
            persist_workbuddy_exe(&p, variant);
            return Some(p);
        }
    }
    None
}

/// Windows 默认安装路径（探测全失败时的回落，供启动失败文案写出尝试路径）。
#[cfg(target_os = "windows")]
pub fn windows_default_app_path(variant: WbVariant) -> PathBuf {
    let local = std::env::var("LOCALAPPDATA").unwrap_or_default();
    let dir = variant.windows_image_names()[0];
    windows_path(&[&local, "Programs", dir, &format!("{dir}.exe")])
}
