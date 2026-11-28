#![cfg_attr(not(target_os = "windows"), allow(dead_code, unused_imports))]

use super::image_name_from_path_str;
use super::image_stem;
use super::is_crashpad_helper_name;
use super::is_self_image_name;
use super::run_cmd_timeout;
#[cfg(any(target_os = "windows", target_os = "macos"))]
use crate::modules::config;
use crate::modules::variant::WbVariant;
use std::path::{Path, PathBuf};
#[cfg(target_os = "windows")]
use std::time::{Duration, Instant};

mod parsing;
mod processes;
mod registry;

pub(crate) use parsing::{parse_windows_display_icon, parse_windows_process_rows};
// 跨平台部分（映像名、行结构）在非 Windows 上也要参与单测；
// 真正调用系统能力的函数只在 Windows 编译，避免 Linux/macOS 出现 unresolved import。
#[cfg(target_os = "windows")]
pub(crate) use processes::{
    close_workbuddy_windows, existing_windows_drives, ps_output, wait_windows_pids_gone,
    windows_tasklist_image_rows, windows_workbuddy_process_rows, TaskKillGuard,
};
pub(crate) use processes::{windows_exe_names, WindowsProcessRow};
#[cfg(target_os = "windows")]
pub(crate) use registry::persist_workbuddy_exe;
#[cfg(target_os = "windows")]
pub use registry::{windows_default_app_path, windows_workbuddy_exe_path};

#[cfg(test)]
mod tests {
    use super::super::*;
    use super::*;

    use super::parsing::*;
    use super::processes::*;
    use super::registry::*;
    include!("tests/parsing.rs");
    include!("tests/processes.rs");
    include!("tests/registry.rs");
}
