//! 账号切换：关闭/启动应用、校验目标、注入会话并收尾。

#[allow(unused_imports)]
use super::session_link::*;
#[allow(unused_imports)]
use super::status::*;
use super::*;

#[cfg(target_os = "macos")]
pub(super) fn close_codebuddy_cn_macos(timeout_secs: i64) -> Result<(), String> {
    let started = Instant::now();
    let timeout = Duration::from_secs(timeout_secs.max(1) as u64);
    let resolved = macos_cn_app_path_resolved();
    let main_patterns = macos_cn_main_patterns(resolved.as_deref());
    let bundle_patterns = macos_cn_bundle_patterns(resolved.as_deref());
    let remaining = || {
        timeout
            .saturating_sub(started.elapsed())
            .max(Duration::from_millis(100))
    };

    let quit_script = format!("quit app id \"{MACOS_BUNDLE_ID}\"");
    let quit = run_cmd("osascript", &["-e", quit_script.as_str()], 10);
    match quit {
        Some(out) if !out.status.success() => {
            eprintln!(
                "[codebuddy-cn-ide] osascript quit failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        None => eprintln!("[codebuddy-cn-ide] osascript quit timed out"),
        _ => {}
    }

    let graceful = Duration::from_secs(8).min(remaining());
    let _ = process::wait_macos_main_gone(&main_patterns, graceful);

    let bundle_pids = process::macos_pids_by_patterns(&bundle_patterns);
    if !bundle_pids.is_empty() {
        eprintln!(
            "[codebuddy-cn-ide] killing {} bundle process(es)…",
            bundle_pids.len()
        );
        process::kill_macos_pids(&bundle_pids);
    }

    let leftover = process::wait_macos_patterns_empty(&bundle_patterns, remaining());
    if leftover.is_empty() {
        return Ok(());
    }
    let pids: Vec<String> = leftover.iter().map(|pid| pid.to_string()).collect();
    Err(format!(
        "CodeBuddy CN 进程无法完全关闭（残留进程: {}）。请手动执行: kill -9 {}",
        pids.join(", "),
        pids.join(" ")
    ))
}

/// 优雅关闭 CodeBuddy CN IDE → 超时只强杀无窗口残留 → 报错点名残留 PID（+ 窗口标题）。
///
/// 关闭手段分两类（理由见 `process::win_close` 的模块注释）：
/// - 有可见窗口的 PID → `SC_CLOSE`（等价用户点 ✕，保存提示由 IDE 自己弹），**不发** `taskkill`；
/// - 没有可见窗口的 PID → 保持旧行为，`taskkill /PID /T`（不带 `/F`）。
///
/// 超时后的 `/F` 只打给「此刻仍没有可见窗口」的残留 PID：对走保存流程的 IDE 强杀会丢
/// 用户未保存的编辑，宁可超时报错。
#[cfg(target_os = "windows")]
pub(super) fn close_codebuddy_cn_windows(timeout_secs: i64) -> Result<(), String> {
    let rows = windows_cn_process_rows();
    if rows.is_empty() {
        return Ok(());
    }
    let pids: Vec<u32> = rows.iter().map(|r| r.pid).collect();

    let window_pids = process::visible_window_pids(&pids);
    for pid in &window_pids {
        // 返回值只表示「有没有命中窗口」：集合已经算过，这里忽略即可。
        let _ = process::post_close_to_visible_windows(*pid);
    }
    // 无窗口的 PID 走旧行为（不带 `/F` 的 `/T`），集合与「允许强杀」同源。
    let guard = process::TaskKillGuard::new();
    for pid in process::force_kill_targets(&pids, &window_pids) {
        guard.kill("CodeBuddy CN", pid, false);
    }

    let started = Instant::now();
    let timeout = Duration::from_secs(timeout_secs.max(1) as u64);
    let graceful_budget = Duration::from_secs(8).min(timeout);
    let wait_targets = process::close_wait_targets(&pids, &window_pids);
    let remaining = process::wait_windows_pids_gone(&wait_targets, graceful_budget);
    if remaining.is_empty() {
        return Ok(());
    }

    // 重新探测：窗口可能是在上一轮探测之后才建出来的。
    let still_windowed = process::visible_window_pids(&remaining);
    for pid in process::force_kill_targets(&remaining, &still_windowed) {
        guard.kill("CodeBuddy CN", pid, true);
    }
    let rest = timeout
        .saturating_sub(started.elapsed())
        .max(Duration::from_secs(1));
    let leftover = process::wait_windows_pids_gone(&remaining, rest);
    if leftover.is_empty() {
        return Ok(());
    }
    let titles = process::window_titles(&leftover);
    Err(process::close_timeout_error_message(
        "CodeBuddy CN",
        &leftover,
        &titles,
    ))
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub(super) fn close_codebuddy_cn_linux(timeout_secs: i64) -> Result<(), String> {
    let pids = linux_codebuddy_cn_pids();
    if pids.is_empty() {
        return Ok(());
    }
    kill_linux_pids(&pids, "-15");
    let timeout = Duration::from_secs(timeout_secs.max(1) as u64);
    let graceful = Duration::from_secs(8).min(timeout);
    let remaining = wait_linux_pids_gone(&pids, graceful);
    if remaining.is_empty() {
        return Ok(());
    }
    kill_linux_pids(&remaining, "-9");
    let rest = timeout.saturating_sub(graceful).max(Duration::from_secs(1));
    let leftover = wait_linux_pids_gone(&remaining, rest);
    if leftover.is_empty() {
        return Ok(());
    }
    Err(format!(
        "CodeBuddy CN 进程无法关闭（残留进程: {}）。请手动执行: kill -9 {}",
        leftover
            .iter()
            .map(|p| p.to_string())
            .collect::<Vec<_>>()
            .join(", "),
        leftover
            .iter()
            .map(|p| p.to_string())
            .collect::<Vec<_>>()
            .join(" ")
    ))
}

pub fn close_codebuddy_cn(timeout_secs: i64) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        close_codebuddy_cn_macos(timeout_secs)
    }
    #[cfg(target_os = "windows")]
    {
        close_codebuddy_cn_windows(timeout_secs)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        close_codebuddy_cn_linux(timeout_secs)
    }
}

#[cfg(target_os = "macos")]
pub(super) fn validate_macos_cn_startup(app: &Path) -> Result<(), String> {
    let main_patterns = macos_cn_main_patterns(Some(app));
    let deadline = Instant::now() + Duration::from_secs(30);
    let sustain = Duration::from_secs(10);
    let mut seen_at: Option<Instant> = None;
    while Instant::now() < deadline {
        let now = Instant::now();
        let alive = !process::macos_pids_by_patterns(&main_patterns).is_empty();
        if alive {
            match seen_at {
                None => seen_at = Some(now),
                Some(start) => {
                    if now.duration_since(start) >= sustain {
                        return Ok(());
                    }
                }
            }
        } else if seen_at.is_some() {
            return Err(format!(
                "CodeBuddy CN 启动后立即退出（疑似残留单例锁）。请先手动打开一次 CodeBuddy CN（路径: {}）",
                app.display()
            ));
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    Err(format!(
        "启动 CodeBuddy CN 超时，未能确认运行（路径: {}）。请先手动打开一次 CodeBuddy CN。",
        app.display()
    ))
}

#[cfg(target_os = "macos")]
pub(super) fn launch_codebuddy_cn_macos() -> Result<(), String> {
    let app = macos_cn_app_path_resolved().ok_or_else(|| {
        "未找到 CodeBuddy CN 应用（尝试路径: /Applications/CodeBuddy CN.app）。请先手动打开一次 CodeBuddy CN 后重试。".to_string()
    })?;
    if !process::is_app_bundle(&app) {
        return Err(format!(
            "未找到 CodeBuddy CN 应用（尝试路径: {}）。请先手动打开一次 CodeBuddy CN 后重试。",
            app.display()
        ));
    }

    let bundle_patterns = macos_cn_bundle_patterns(Some(&app));
    let bundle_pids = process::macos_pids_by_patterns(&bundle_patterns);
    if !bundle_pids.is_empty() {
        process::kill_macos_pids(&bundle_pids);
        let _ = process::wait_macos_patterns_empty(&bundle_patterns, Duration::from_secs(5));
    }

    let app_lossy = app.to_string_lossy();
    let open = run_cmd(
        "open",
        &["-n", "-a", app_lossy.as_ref(), "--args", "--new-window"],
        10,
    );
    match open {
        Some(out) if out.status.success() => {}
        Some(out) => {
            let reason = String::from_utf8_lossy(&out.stderr).trim().to_string();
            let reason = if reason.is_empty() {
                format!("open 退出码 {}", out.status.code().unwrap_or(-1))
            } else {
                reason
            };
            return Err(format!(
                "启动 CodeBuddy CN 失败: {reason}（路径: {}）",
                app.display()
            ));
        }
        None => {
            return Err(format!(
                "启动 CodeBuddy CN 失败: open 超时（路径: {}）",
                app.display()
            ));
        }
    }

    validate_macos_cn_startup(&app)
}

pub fn launch_codebuddy_cn() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        launch_codebuddy_cn_macos()
    }
    #[cfg(target_os = "windows")]
    {
        let exe = windows_cn_exe_path_resolved().ok_or_else(|| {
            "未找到 CodeBuddy CN 程序（尝试路径: %LOCALAPPDATA%\\Programs\\CodeBuddy CN\\CodeBuddy CN.exe）。请在 Windows 上打开 CodeBuddy CN 后重试。".to_string()
        })?;
        if !is_existing_cn_exe(&exe) {
            return Err(format!(
                "未找到 CodeBuddy CN 程序（尝试路径: {}）。请在 Windows 上打开 CodeBuddy CN 后重试。",
                exe.display()
            ));
        }
        persist_cn_app_cache(&exe);
        process::cmd_builder(&exe)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| format!("启动 CodeBuddy CN 失败: {e}（路径: {}）", exe.display()))?;
        Ok(())
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let exe = codebuddy_cn_app_path().ok_or_else(|| {
            "未找到 CodeBuddy CN 可执行文件。已查过桌面条目（.desktop 的 Exec）与常见路径：/usr/bin/buddycn、/usr/share/buddycn/bin/buddycn、/usr/bin/codebuddy-cn。请先手动打开一次 CodeBuddy CN。".to_string()
        })?;
        process::cmd_builder(&exe)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| format!("启动 CodeBuddy CN 失败: {e}（路径: {}）", exe.display()))?;
        Ok(())
    }
}

/// 切换 CodeBuddy CN IDE 账号：关进程 → 注入 secret → 启动。
pub fn switch_account(account_id: &str, restart: bool) -> Result<Value, String> {
    let (acc, data_dir) = validate_switch_target(account_id)?;

    if restart {
        eprintln!("[codebuddy-cn-ide] closing CodeBuddy CN…");
        close_codebuddy_cn(20)?;
    }

    inject_session_and_finish(account_id, &acc, &data_dir, restart)
}

/// 切换前置校验：账号存在 + `access_token` 非空 + 用户数据目录存在。返回账号条目与数据目录。
///
/// 必须在关闭 IDE **之前**执行：账号不存在、token 为空、数据目录缺失这类「无论怎么关都
/// 注定失败」的目标，不该让用户的 IDE 被关掉。幂等且廉价，注入前会再次使用返回的数据目录。
pub(crate) fn validate_switch_target(account_id: &str) -> Result<(Value, PathBuf), String> {
    let acc =
        account::find_account(account_id).ok_or_else(|| format!("账号不存在: {account_id}"))?;
    let token = get_str(&acc, "access_token")
        .ok_or_else(|| "账号缺少 access_token，无法注入 CodeBuddy CN".to_string())?;
    if token.is_empty() {
        return Err("账号 access_token 为空".to_string());
    }

    let data_dir =
        codebuddy_cn_data_dir().ok_or_else(|| "无法定位 CodeBuddy CN 数据目录".to_string())?;
    if !data_dir.exists() {
        return Err(format!(
            "未找到 CodeBuddy CN 用户数据目录（{}）。请先手动打开 CodeBuddy CN 并登录一次。",
            data_dir.display()
        ));
    }
    Ok((acc, data_dir))
}

/// 「已可切换」后执行：注入 secret → 记录当前账号 → 按需重启（关闭动作由调用方负责）。
///
/// 会话复制等前置写入完成后调用的就是这一步，因此它不碰进程关闭逻辑。
pub(crate) fn inject_session_and_finish(
    account_id: &str,
    acc: &Value,
    data_dir: &Path,
    restart: bool,
) -> Result<Value, String> {
    let session = build_session_json(acc);
    eprintln!("[codebuddy-cn-ide] injecting secret…");
    let db_path = inject_codebuddy_cn_secret(&session, Some(data_dir)).map_err(|err| {
        if err.contains("Safe Storage") || err.contains("Keychain") {
            format!(
                "注入登录状态失败：{err}\n\n请先手动打开 CodeBuddy CN 并登录一次，确保 Keychain 中存在「CodeBuddy CN Safe Storage」条目后再试。"
            )
        } else {
            err
        }
    })?;

    set_active_account_id(account_id)?;

    if restart {
        eprintln!("[codebuddy-cn-ide] launching CodeBuddy CN…");
        launch_codebuddy_cn()?;
    }

    // 「最近切换」时间戳（issue #184）：secret 已注入、当前账号已记录、（按需）已重启，
    // 切换确实生效后才写。位置与 `switch::execute` 一致——启动失败会让本函数返回 Err，
    // 那种情况下不记录，避免「提示失败但卡片显示刚切过」。
    // 失败只影响卡片徽标与排序，不得回滚已写入的 secret、不得把成功改成报错。
    let _ = account::touch_last_used(account_id, "codebuddy-cn-ide");

    Ok(json!({
        "ok": true,
        "account": account::account_display_name(acc),
        "accountId": account_id,
        "dbPath": db_path.to_string_lossy(),
        "restarted": restart,
        "message": if restart {
            format!("已切换 CodeBuddy IDE 到 {} 并重启", account::account_display_name(acc))
        } else {
            format!("已写入 CodeBuddy IDE 凭证（{}）；请手动重启 CodeBuddy CN 生效", account::account_display_name(acc))
        },
    }))
}
