//! 进程枚举与关闭：镜像名判定、tasklist/PowerShell 行查询、PID 等待与 close。

#[cfg(target_os = "windows")]
use super::super::{
    close_timeout_error_message, close_wait_targets, force_kill_targets,
    post_close_to_visible_windows, utf8_script, visible_window_pids, window_titles,
};
use super::parsing::*;
use super::registry::*;
use super::*;

/// 精确匹配本档位的 WorkBuddy 映像，禁止子串命中其它档位或 workbuddy-shift。
pub(super) fn is_workbuddy_image_name(name: &str, variant: WbVariant) -> bool {
    let stem = image_stem(image_name_from_path_str(name));
    variant
        .windows_image_names()
        .iter()
        .any(|candidate| stem.eq_ignore_ascii_case(candidate))
}

/// 本档位的 Windows 可执行文件名。
pub(crate) fn windows_exe_names(variant: WbVariant) -> Vec<String> {
    variant
        .windows_image_names()
        .iter()
        .map(|name| format!("{name}.exe"))
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WindowsProcessRow {
    pub(crate) pid: u32,
    pub(crate) name: String,
    pub(crate) exe_path: Option<PathBuf>,
}

/// Windows：执行 PowerShell 并取 stdout。
///
/// 脚本统一经 [`utf8_script`] 前置 UTF-8 输出编码：否则 PowerShell 按控制台代码页
/// （简中 = GBK）写 stdout，含中文的路径会解成 `U+FFFD`，后续 spawn 直接 `os error 3`。
#[cfg(target_os = "windows")]
pub(crate) fn ps_output(script: &str, timeout_secs: u64) -> Option<String> {
    let encoded = utf8_script(script);
    let out = run_cmd_timeout(
        "powershell",
        &["-NoProfile", "-NonInteractive", "-Command", &encoded],
        timeout_secs,
    )?;
    Some(String::from_utf8_lossy(&out.stdout).to_string())
}

#[cfg(target_os = "windows")]
pub(crate) fn windows_tasklist_image_rows(image: &str) -> Vec<WindowsProcessRow> {
    let filter = format!("IMAGENAME eq {image}");
    let Some(out) = run_cmd_timeout("tasklist", &["/FI", &filter, "/FO", "CSV", "/NH"], 5) else {
        return Vec::new();
    };
    parse_tasklist_csv(&String::from_utf8_lossy(&out.stdout))
}

/// 精确映像名收集本档位 WorkBuddy PID，排除本工具、自身 PID 与 crashpad。
#[cfg(target_os = "windows")]
pub(crate) fn windows_workbuddy_process_rows(variant: WbVariant) -> Vec<WindowsProcessRow> {
    let self_pid = std::process::id();
    let mut rows = Vec::new();
    for exe in windows_exe_names(variant) {
        rows.extend(windows_tasklist_image_rows(&exe));
    }
    filter_windows_workbuddy_rows(&rows, variant)
        .into_iter()
        .filter(|r| r.pid != self_pid)
        .collect()
}

#[cfg(target_os = "windows")]
pub(super) fn is_windows_pid_running(pid: u32) -> bool {
    let filter = format!("PID eq {pid}");
    match run_cmd_timeout("tasklist", &["/FI", &filter, "/FO", "CSV", "/NH"], 5) {
        Some(out) => parse_tasklist_csv(&String::from_utf8_lossy(&out.stdout))
            .iter()
            .any(|r| r.pid == pid),
        None => true,
    }
}

#[cfg(target_os = "windows")]
pub(crate) fn wait_windows_pids_gone(pids: &[u32], timeout: Duration) -> Vec<u32> {
    if pids.is_empty() {
        return Vec::new();
    }
    let deadline = Instant::now() + timeout;
    loop {
        let alive: Vec<u32> = pids
            .iter()
            .copied()
            .filter(|pid| is_windows_pid_running(*pid))
            .collect();
        if alive.is_empty() || Instant::now() >= deadline {
            return alive;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

#[cfg(target_os = "windows")]
pub(crate) fn existing_windows_drives() -> Vec<char> {
    ('A'..='Z')
        .filter(|c| Path::new(&format!(r"{c}:\")).exists())
        .collect()
}

#[cfg(target_os = "windows")]
pub(super) fn windows_running_workbuddy_exe(variant: WbVariant) -> Option<PathBuf> {
    let names = variant.windows_image_names().join(",");
    let script = format!(
        "Get-Process -Name {names} -ErrorAction SilentlyContinue | \
         ForEach-Object {{ $p = ''; try {{ $p = $_.Path }} catch {{}}; '{{0}}|{{1}}|{{2}}' -f $_.Id, $_.ProcessName, $p }}"
    );
    let stdout = ps_output(&script, 5)?;
    for row in filter_windows_workbuddy_rows(&parse_windows_process_rows(&stdout), variant) {
        if let Some(p) = row.exe_path {
            if is_existing_workbuddy_exe(&p, variant) {
                return Some(p);
            }
        }
    }
    None
}

/// 优雅关闭 WorkBuddy → 超时只强杀无窗口残留 → 报错点名残留 PID（+ 窗口标题）。
///
/// **为什么不是对每个 PID 直接 `taskkill /PID /T`**：不带 `/F` 时它只对「有窗口」的进程
/// 有效——对没有窗口的子进程只回一行「只能强制终止」，而这些子进程并不是真正要关的主
/// 窗口；主窗口本身则要靠目标进程自己走保存/退出流程。所以：
/// - 有可见窗口的 PID → 投 `SC_CLOSE`（等价用户点 ✕，保存提示由目标进程自己弹），
///   **不再**对它发 `taskkill /T`；
/// - 没有可见窗口的 PID → 保持旧行为，`taskkill /PID /T`（不带 `/F`）。
///
/// 超时后的 `/F` **只允许**打给「此刻仍没有可见窗口」的残留 PID（[`force_kill_targets`]）：
/// 对有窗口的进程强杀会丢用户未保存的内容，宁可超时报错让用户自己处理。
/// 本进程的祖先 PID 链（父 → 祖父 → …，不含自身）。
///
/// 用途只有一个：`taskkill /T` 会连子树一起收，如果某个待杀目标是我们的祖先，
/// 整棵树就把本工具也杀了。见 [`tree_kill_hits_self`]。
///
/// 用 `Win32_Process` 逐级上溯而不是一次性拉全表：只读几条记录，代价可忽略；
/// 取不到（PowerShell 不可用、超时）时返回空集，等价于「不做保护、维持旧行为」。
#[cfg(target_os = "windows")]
fn own_ancestor_pids() -> Vec<u32> {
    let self_pid = std::process::id();
    let script = format!(
        "$p = Get-CimInstance Win32_Process -Filter \"ProcessId = {self_pid}\" -ErrorAction SilentlyContinue; \
         for ($i = 0; $i -lt 32 -and $p; $i++) {{ \
           $parent = $p.ParentProcessId; \
           if (-not $parent) {{ break }}; \
           $parent; \
           $p = Get-CimInstance Win32_Process -Filter \"ProcessId = $parent\" -ErrorAction SilentlyContinue \
         }}"
    );
    let Some(stdout) = ps_output(&script, 5) else {
        return Vec::new();
    };
    parse_pid_lines(&stdout)
}

/// 从「一行一个 PID」的输出里取 PID（去重、丢 0、忽略非数字噪声）。
fn parse_pid_lines(stdout: &str) -> Vec<u32> {
    let mut out = Vec::new();
    for token in stdout.split_whitespace() {
        if let Ok(pid) = token.trim().parse::<u32>() {
            if pid != 0 && !out.contains(&pid) {
                out.push(pid);
            }
        }
    }
    out
}

/// 对 `target_pid` 用 `/T` 会不会把本进程一起杀掉。
///
/// 判据：`target_pid` 出现在本进程的祖先链上。命中时必须退化成「只杀该 PID」，
/// 否则切换会在关 WorkBuddy 那一步把自己也结束，留下「两个应用一起消失」的现象
/// （2026-11-23 从 WorkBuddy 内置终端启动本工具时实际发生）。
pub(super) fn tree_kill_hits_self(target_pid: u32, ancestors: &[u32]) -> bool {
    ancestors.contains(&target_pid)
}

/// 记录一次「为避免自杀而降级」的处置，便于事后从 error.log 解释行为差异。
#[cfg(target_os = "windows")]
fn record_self_tree_guard(product: &str, target_pid: u32) {
    crate::modules::error_log::record(
        "backend",
        "关闭客户端时命中自我保护：改为只杀目标进程，不带 /T",
        &format!("{product} target_pid={target_pid}（本进程在它的子树里）"),
    );
}

/// 带「别把自己杀掉」保护的 `taskkill` 封装。
///
/// `taskkill /T` 会连整棵子树一起收：本工具若由目标客户端的终端（或它拉起的进程）启动，
/// 就在那棵树里，关客户端会把本工具一起结束 —— 用户看到的是「两个应用一起消失」，
/// 切换停在半路。三种真实场景：从 WorkBuddy 内置终端启动（2026-11-23）、从 CodeBuddy
/// 的终端启动、以及工具被 IDE 的子进程拉起（2026-11-28 实证：点「复制并切换」后闪退，
/// 会话已迁移但界面消失）。
///
/// 命中祖先时降级为「只杀该 PID、不带 `/T`」：目标仍是同一个进程，只是不连带它的子树，
/// 本工具至少能活到把结果写回去。祖先链只查一次（PowerShell 调用不便宜），查询失败时
/// 退化为旧行为——不能因为查询失败就放弃关闭进程。
#[cfg(target_os = "windows")]
pub(crate) struct TaskKillGuard {
    ancestors: Vec<u32>,
}

#[cfg(target_os = "windows")]
impl TaskKillGuard {
    /// 解析一次本进程的祖先链。
    pub(crate) fn new() -> Self {
        Self {
            ancestors: own_ancestor_pids(),
        }
    }

    /// 关闭 `pid`：`force` 对应是否带 `/F`。
    ///
    /// **为什么不是直接 `taskkill /PID /T [ /F ]`**：见类型注释。命中祖先时去掉 `/T`，
    /// 并记一条日志说明这次为什么少了树杀。
    pub(crate) fn kill(&self, product: &str, pid: u32, force: bool) {
        let hits_self = tree_kill_hits_self(pid, &self.ancestors);
        if hits_self {
            record_self_tree_guard(product, pid);
        }
        let args = taskkill_args(pid, force, hits_self);
        let owned: Vec<&str> = args.iter().map(String::as_str).collect();
        let _ = run_cmd_timeout("taskkill", &owned, 10);
    }
}

/// `taskkill` 的参数（纯函数，便于单测）：
/// - 不命中祖先 → `/PID <p> /T [ /F ]`（保持旧行为，连子树一起收）；
/// - 命中祖先 → `/PID <p> [ /F ]`（去掉 `/T`，否则连本进程一起被杀）。
///
/// 注意 `/T` 与 `/F` 的顺序：Windows 不要求顺序，但保持「先 `/T` 后 `/F`」与既有代码一致，
/// 便于对照历史日志。
///
/// 不按平台 cfg 收窄：调用方只在 Windows 编译，但单测要在 Ubuntu / macOS 的 CI 也跑一遍，
/// 否则这条「命中祖先必须去掉 /T」的回归只能靠 Windows 机器发现。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn taskkill_args(pid: u32, force: bool, hits_self: bool) -> Vec<String> {
    let pid = pid.to_string();
    let mut args = vec!["/PID".to_string(), pid];
    if !hits_self {
        args.push("/T".to_string());
    }
    if force {
        args.push("/F".to_string());
    }
    args
}

#[cfg(target_os = "windows")]
pub(crate) fn close_workbuddy_windows(variant: WbVariant, timeout_secs: i64) -> Result<(), String> {
    let rows = windows_workbuddy_process_rows(variant);
    if rows.is_empty() {
        return Ok(());
    }
    let pids: Vec<u32> = rows.iter().map(|r| r.pid).collect();

    let window_pids = visible_window_pids(&pids);
    for pid in &window_pids {
        // 返回值只表示「有没有命中窗口」：集合已经算过，这里忽略即可。
        let _ = post_close_to_visible_windows(*pid);
    }
    // `/T` 会连子进程一起收。若本进程恰好是某个目标的**子孙**（例如从 WorkBuddy
    // 内置终端启动本工具），整棵树就把自己也杀了 —— 切换会停在半路，用户看到的是
    // 「WorkBuddy 和 WorkBuddy Shift 一起消失」。这种情形退化为只杀目标 PID 本身：
    // 目标仍是同一个进程，只是不连带它的子树，我们至少能活到把认证写完。
    let product = product_name(variant);
    let guard = TaskKillGuard::new();
    // 没有可见窗口的 PID 走旧行为（不带 `/F` 的 `/T`）。集合与「允许强杀的 PID」同源，
    // 故复用 [`force_kill_targets`]，不另写一份「pids - window_pids」。
    for pid in force_kill_targets(&pids, &window_pids) {
        guard.kill(product, pid, false);
    }

    let started = Instant::now();
    let timeout = Duration::from_secs(timeout_secs.max(1) as u64);
    let graceful_budget = Duration::from_secs(8).min(timeout);
    // 等待判据收窄到「仍持有可见窗口的 PID」：后台子进程不参与落盘，等它们只会把
    // 「已经关掉了」误判成「等待退出超时」。
    let wait_targets = close_wait_targets(&pids, &window_pids);
    let remaining = wait_windows_pids_gone(&wait_targets, graceful_budget);
    if remaining.is_empty() {
        return Ok(());
    }

    // 重新探测而不是复用上面的 `window_pids`：窗口可能是在上一轮探测之后才建出来的。
    let still_windowed = visible_window_pids(&remaining);
    for pid in force_kill_targets(&remaining, &still_windowed) {
        guard.kill(product, pid, true);
    }
    let rest = timeout
        .saturating_sub(started.elapsed())
        .max(Duration::from_secs(1));
    let leftover = wait_windows_pids_gone(&remaining, rest);
    if leftover.is_empty() {
        return Ok(());
    }
    let titles = window_titles(&leftover);
    Err(close_timeout_error_message(product, &leftover, &titles))
}

/// 产品名（只用于日志文案）：取该档位第一个映像名，与旧超时文案保持一致。
#[cfg(target_os = "windows")]
fn product_name(variant: WbVariant) -> &'static str {
    variant
        .windows_image_names()
        .first()
        .copied()
        .unwrap_or("WorkBuddy")
}

#[cfg(test)]
mod self_tree_guard_tests {
    use super::*;

    /// `/T` 只在目标**不是**我们的祖先时才允许：否则杀树会连本进程一起收。
    #[test]
    fn tree_kill_is_refused_only_when_target_is_our_ancestor() {
        let ancestors = [4242, 1000, 4];
        assert!(tree_kill_hits_self(4242, &ancestors), "父进程必须命中");
        assert!(tree_kill_hits_self(4, &ancestors), "更上层祖先也要命中");
        assert!(
            !tree_kill_hits_self(9999, &ancestors),
            "无关进程应继续用 /T"
        );
        assert!(
            !tree_kill_hits_self(4242, &[]),
            "取不到祖先链时维持旧行为（不做保护），不能因为查询失败就放弃关进程"
        );
    }

    /// 上溯命令的输出可能带空行、重复 PID 与非数字噪声，解析要稳。
    #[test]
    fn parse_pid_lines_dedupes_and_drops_noise() {
        assert_eq!(
            parse_pid_lines("1234\r\n5678\r\n\r\n1234\r\nabc\r\n0\r\n"),
            vec![1234, 5678]
        );
        assert_eq!(parse_pid_lines(""), Vec::<u32>::new());
        assert_eq!(parse_pid_lines("not-a-pid"), Vec::<u32>::new());
    }

    /// 命中祖先时必须去掉 `/T`，否则 `taskkill` 会把本进程所在整棵树一起收掉
    /// （2026-11-28：点「复制并切换」后会话迁移成功但应用闪退）。
    #[test]
    fn taskkill_args_drop_tree_flag_only_for_ancestors() {
        assert_eq!(
            taskkill_args(4242, false, true),
            vec!["/PID".to_string(), "4242".to_string()],
            "命中祖先：不带 /T，也不带 /F"
        );
        assert_eq!(
            taskkill_args(4242, true, true),
            vec!["/PID".to_string(), "4242".to_string(), "/F".to_string()],
            "命中祖先且需要强杀：保留 /F，但仍不带 /T"
        );
        assert_eq!(
            taskkill_args(4242, false, false),
            vec!["/PID".to_string(), "4242".to_string(), "/T".to_string()],
            "无关进程：保持旧行为"
        );
        assert_eq!(
            taskkill_args(4242, true, false),
            vec![
                "/PID".to_string(),
                "4242".to_string(),
                "/T".to_string(),
                "/F".to_string()
            ],
            "无关进程 + 强杀：/T 与 /F 都在"
        );
    }

    /// 守卫用注入的祖先链判断：命中祖先的 PID 走降级，其余照旧。
    #[test]
    fn guard_detects_own_ancestors() {
        let ancestors = vec![1000, 4242];
        let args = taskkill_args(4242, false, tree_kill_hits_self(4242, &ancestors));
        assert!(!args.contains(&"/T".to_string()), "祖先 PID：{args:?}");
        let args = taskkill_args(9999, false, tree_kill_hits_self(9999, &ancestors));
        assert!(args.contains(&"/T".to_string()), "无关 PID：{args:?}");
    }
}
