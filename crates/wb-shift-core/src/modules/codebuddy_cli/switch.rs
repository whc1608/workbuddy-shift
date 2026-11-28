//! 切换与安装：CLI 进程识别/关闭、安装 helper、切换账号与写 state 顺序。

use super::helper::*;
use super::settings::*;
use super::state::*;
use super::*;

/// 只认 CodeBuddy CLI / prewarm 的包路径，排除 IDE（`.app` / `Programs\CodeBuddy`）
/// 和本工具。禁止用 `codebuddy` 单字去匹配。
///
/// 只在 macOS 的 `list_codebuddy_cli_pids` 与 Linux 的 `/proc` 扫描路径里使用 ——
/// Windows 走 PowerShell 查询，不需要它。Windows 的非测试构建会因此报 dead_code，
/// 而 CI 的 setup-rust-toolchain 默认带 `-D warnings`，故显式放行（保留测试可用）。
#[cfg_attr(all(target_os = "windows", not(test)), allow(dead_code))]
pub(super) fn is_codebuddy_cli_process_args(args: &str) -> bool {
    let lower = args.to_ascii_lowercase();
    // 自排除：命令行含本工具产物名（`workbuddy-shift` 子串已涵盖
    // `workbuddy-shift-desktop`）即不是 CodeBuddy CLI 进程。
    if lower.contains("workbuddy-shift") {
        return false;
    }
    if lower.contains(".app/contents/") {
        return false;
    }
    if lower.contains("codebuddy cn") || lower.contains("codebuddycn") {
        return false;
    }
    if lower.contains("\\programs\\codebuddy\\") || lower.contains("/programs/codebuddy/") {
        return false;
    }
    lower.contains("@tencent-ai/codebuddy-code")
        || lower.contains("codebuddy-code/dist-server")
        || lower.contains("codebuddy-code\\dist-server")
        || lower.contains("codebuddy-code/bin/")
        || lower.contains("codebuddy-code\\bin\\")
        || lower.contains("/bin/codebuddy")
        || lower.contains("\\bin\\codebuddy")
        || lower.contains("codebuddy.cmd")
        || lower.contains("cbc-prewarm")
}

pub(super) fn current_cli_variant(accounts: &[Value], state: &Value) -> Option<WbVariant> {
    let active = if cfg!(windows) {
        read_json_file(&settings_path())
            .as_ref()
            .and_then(settings_env_token)
            .and_then(|token| account_index_by_token(accounts, token))
    } else {
        state_account_index(state, accounts)
    }?;
    accounts.get(active.0).map(WbVariant::from_account)
}

pub(super) fn list_codebuddy_cli_pids() -> Vec<u32> {
    let self_pid = std::process::id();
    #[cfg(target_os = "macos")]
    {
        let patterns = [
            "@tencent-ai/codebuddy-code".to_string(),
            "cbc-prewarm".to_string(),
            "/bin/codebuddy".to_string(),
        ];
        process::macos_rows_by_patterns(&patterns)
            .into_iter()
            .filter(|(pid, args)| *pid != self_pid && is_codebuddy_cli_process_args(args))
            .map(|(pid, _)| pid)
            .collect()
    }
    #[cfg(target_os = "windows")]
    {
        let script = "Get-CimInstance Win32_Process -ErrorAction SilentlyContinue | \
             Where-Object { \
               $_.CommandLine -and ( \
                 $_.CommandLine -like '*@tencent-ai/codebuddy-code*' -or \
                 $_.CommandLine -like '*codebuddy-code*dist-server*' -or \
                 $_.CommandLine -like '*\\bin\\codebuddy*' -or \
                 $_.CommandLine -like '*cbc-prewarm*' -or \
                 $_.CommandLine -like '*codebuddy.cmd*' \
               ) \
             } | ForEach-Object { $_.ProcessId }";
        let Some(stdout) = process::ps_output(script, 5) else {
            return Vec::new();
        };
        stdout
            .lines()
            .filter_map(|line| line.trim().parse::<u32>().ok())
            .filter(|pid| *pid != self_pid)
            .collect()
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let mut pids = Vec::new();
        let Ok(entries) = std::fs::read_dir("/proc") else {
            return pids;
        };
        for entry in entries.flatten() {
            let pid: u32 = match entry.file_name().to_string_lossy().parse() {
                Ok(pid) => pid,
                Err(_) => continue,
            };
            if pid == self_pid {
                continue;
            }
            let cmdline = match std::fs::read(format!("/proc/{pid}/cmdline")) {
                Ok(bytes) if !bytes.is_empty() => {
                    String::from_utf8_lossy(&bytes).replace('\0', " ")
                }
                _ => continue,
            };
            if is_codebuddy_cli_process_args(&cmdline) {
                pids.push(pid);
            }
        }
        pids
    }
}

pub(super) fn terminate_codebuddy_cli_pids(pids: &[u32]) {
    if pids.is_empty() {
        return;
    }
    #[cfg(target_os = "macos")]
    {
        let owned: Vec<String> = std::iter::once("-15".to_string())
            .chain(pids.iter().map(u32::to_string))
            .collect();
        let args: Vec<&str> = owned.iter().map(String::as_str).collect();
        let _ = process::run_cmd_timeout("kill", &args, 10);
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(200));
        }
        process::kill_macos_pids(pids);
    }
    #[cfg(target_os = "windows")]
    {
        // `taskkill /T` 会连子树一起收：本工具若从 CodeBuddy 的终端启动，就在这棵树里。
        let guard = process::TaskKillGuard::new();
        for pid in pids {
            guard.kill("CodeBuddy CLI", *pid, false);
        }
        let remaining = process::wait_windows_pids_gone(pids, Duration::from_secs(3));
        // CLI 是终端进程，没有可以投 `SC_CLOSE` 的窗口：`window_pids` 传空集 = 残留全部可
        // 强杀（与旧行为等价）。判定仍走 [`process::force_kill_targets`]，与三条 GUI 关闭
        // 路径共用同一份「只强杀无窗口进程」的约束，不在这里另写一份。
        for pid in process::force_kill_targets(&remaining, &[]) {
            guard.kill("CodeBuddy CLI", pid, true);
        }
        let leftover = process::wait_windows_pids_gone(&remaining, Duration::from_secs(2));
        if !leftover.is_empty() {
            // 关闭不阻断切换（见 `close_running_codebuddy_cli` 的契约），所以文案进错误日志
            // 而不是返回值：点名残留 PID，用户可在日志页看到并手动处理。
            let message = process::close_timeout_error_message("CodeBuddy CLI", &leftover, &[]);
            crate::modules::error_log::record("backend", "关闭 CodeBuddy CLI 超时", &message);
            eprintln!("[codebuddy-cli] close timeout: {message}");
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let term: Vec<String> = std::iter::once("-15".to_string())
            .chain(pids.iter().map(u32::to_string))
            .collect();
        let args: Vec<&str> = term.iter().map(String::as_str).collect();
        let _ = process::run_cmd_timeout("kill", &args, 10);
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let alive: Vec<u32> = pids
                .iter()
                .copied()
                .filter(|pid| Path::new(&format!("/proc/{pid}")).exists())
                .collect();
            if alive.is_empty() || Instant::now() >= deadline {
                if !alive.is_empty() {
                    let kill: Vec<String> = std::iter::once("-9".to_string())
                        .chain(alive.iter().map(u32::to_string))
                        .collect();
                    let args: Vec<&str> = kill.iter().map(String::as_str).collect();
                    let _ = process::run_cmd_timeout("kill", &args, 10);
                }
                break;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }
}

pub(super) fn close_running_codebuddy_cli() -> (bool, usize) {
    let pids = list_codebuddy_cli_pids();
    if pids.is_empty() {
        return (false, 0);
    }
    terminate_codebuddy_cli_pids(&pids);
    (true, pids.len())
}

/// 切换结果文案（唯一构造点）：**始终**说明是否关闭了正在运行的 CLI。
///
/// 有进程 → 报出关闭数量（用户需要知道当前会话被打断）；
/// 无进程 → 明说"未发现"，避免用户以为提示语是模板敷衍。
pub(super) fn region_switch_message(
    variant: WbVariant,
    region_changed: bool,
    closed_count: usize,
) -> String {
    let region = if variant == WbVariant::Ai {
        "国际版"
    } else {
        "国内版"
    };
    match (region_changed, closed_count) {
        (true, 0) => format!("已切换到{region}。未发现正在运行的 CodeBuddy CLI，新开会话即可"),
        (true, closed_count) => format!(
            "已切换到{region}并关闭正在运行的 CodeBuddy CLI（{closed_count} 个进程）。请重新打开 CLI 后再发会话"
        ),
        (false, 0) => {
            "CodeBuddy CLI 默认账号已更新。未发现正在运行的 CodeBuddy CLI，新开会话即可".to_string()
        }
        (false, closed_count) => format!(
            "CodeBuddy CLI 默认账号已更新，并关闭正在运行的 CodeBuddy CLI（{closed_count} 个进程）。请重新打开 CLI 后再发会话"
        ),
    }
}

/// 关闭进程之后发生的失败：CLI 已经退出，但账号没切成功。
///
/// 关闭不可回滚，所以错误必须显式说明"我已经把你的 CLI 关了"——否则用户只会看到
/// "写文件失败"，然后奇怪为什么终端里的会话掉了。无进程可关时不追加这句。
pub(super) fn after_close_error(error: String, closed_count: usize) -> String {
    if closed_count == 0 {
        return error;
    }
    format!("{error}；已关闭 {closed_count} 个正在运行的 CodeBuddy CLI，但账号未切换成功，请重试")
}

/// 切换的写入阶段：**先关闭正在运行的 CLI，再写 `state.json`**。
///
/// 顺序不可互换——先写 state 会留下「新 state + 旧进程仍活」的窗口：旧进程仍持旧 key，
/// 还可能把旧站点缓存写回去。关闭成功与否都不阻断切换（关不掉就只告警计数）。
///
/// 抽成函数只为可测：单测注入 `close` 回调与临时路径，断言关闭发生在写入之前。
pub(super) fn close_then_write_state<F>(
    state_file: &Path,
    content: &str,
    close: F,
) -> Result<(bool, usize), String>
where
    F: FnOnce() -> (bool, usize),
{
    let (closed, closed_count) = close();
    atomic_write(state_file, content)
        .map(|_| (closed, closed_count))
        .map_err(|_| {
            after_close_error(
                if cfg!(windows) {
                    auth_config_error("状态阶段", "无法写入所选 CLI 账号状态，请检查文件权限")
                } else {
                    helper_validation_error("状态阶段", "无法写入所选账号状态，请检查文件权限")
                },
                closed_count,
            )
        })
}

/// Windows 写入 settings env 认证；其他平台安装/升级项目提供的 helper。
/// 只有用户显式调用这个命令时才会修改用户级配置。
///
/// 兼容旧版：早期 helper 是 `helper.sh`（bash + python3），若当前配置的是
/// workbuddy-shift 的旧 helper，允许直接原地升级，并清理旧文件。
pub fn install_helper() -> Result<Value, String> {
    if cfg!(windows) {
        return install_env_auth();
    }
    let target = rotate_dir().join(HELPER_FILE);
    let logic_target = rotate_dir().join(LOGIC_FILE);
    let legacy_target = rotate_dir().join(LEGACY_HELPER_FILE);
    let legacy_windows_target = rotate_dir().join(LEGACY_WINDOWS_HELPER_FILE);
    if let Some(current_command) = helper_command() {
        let Some(current) = command_path(&current_command) else {
            return Err("已有其他 CodeBuddy CLI apiKeyHelper 命令；请先确认后再替换".to_string());
        };
        if !same_path(&current, &target, cfg!(windows))
            && !same_path(&current, &legacy_target, cfg!(windows))
            && !same_path(&current, &legacy_windows_target, cfg!(windows))
        {
            return Err("已有其他 CodeBuddy CLI helper；请先确认后再替换".to_string());
        }
    }

    let settings = settings_path();
    let previous_settings = std::fs::read_to_string(&settings).ok();
    let previous_logic = std::fs::read_to_string(&logic_target).ok();
    let mut settings_value = if let Some(content) = previous_settings.as_ref() {
        serde_json::from_str(content).map_err(|_| "CodeBuddy settings.json 不是有效 JSON")?
    } else {
        json!({})
    };
    if !settings_value.is_object() {
        return Err("CodeBuddy settings.json 顶层不是对象".to_string());
    }

    std::fs::create_dir_all(rotate_dir()).map_err(|_| {
        helper_validation_error("安装阶段", "无法创建 helper 目录，请检查用户目录权限")
    })?;
    // 各平台都直接配置这份 helper.cjs。
    atomic_write(&logic_target, STANDARD_HELPER)
        .map_err(|_| helper_validation_error("安装阶段", "无法写入 helper.cjs，请检查文件权限"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if std::fs::set_permissions(&logic_target, std::fs::Permissions::from_mode(0o700)).is_err()
        {
            restore_file(&logic_target, previous_logic.as_deref());
            return Err(helper_validation_error(
                "安装阶段",
                "无法设置 helper 执行权限",
            ));
        }
    }
    // CodeBuddy 2.138.0 会先将 apiKeyHelper 当作文件路径解析，然后在
    // Windows Git Bash eval 前将 `C:\\...` 归一化为 `C:/...`。这里必须
    // 保持为未引号的绝对路径，否则 CLI 会错将它解析到当前工作目录。
    let configured_target = match configured_helper_path(&target) {
        Ok(path) => path,
        Err(error) => {
            restore_file(&logic_target, previous_logic.as_deref());
            return Err(error);
        }
    };
    let configured_command = configured_target.to_string_lossy().to_string();
    settings_value["apiKeyHelper"] = json!(configured_command);
    let content = match serde_json::to_string_pretty(&settings_value) {
        Ok(content) => content,
        Err(_) => {
            restore_file(&logic_target, previous_logic.as_deref());
            return Err(helper_validation_error(
                "配置阶段",
                "无法生成 CodeBuddy settings.json",
            ));
        }
    };
    if let Some(parent) = settings.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            restore_file(&logic_target, previous_logic.as_deref());
            return Err(helper_validation_error(
                "配置阶段",
                "无法创建 CodeBuddy 配置目录，请检查用户目录权限",
            ));
        }
    }
    if atomic_write(&settings, &content).is_err() {
        restore_file(&logic_target, previous_logic.as_deref());
        return Err(helper_validation_error(
            "配置阶段",
            "无法写入 CodeBuddy settings.json，请检查文件权限",
        ));
    }

    let accounts = account::load_accounts();
    let state = load_state();
    let active_account =
        state_account_index(&state, &accounts).and_then(|(index, _)| accounts.get(index));
    let variant = active_account.map(WbVariant::from_account);
    let validation = active_account
        .ok_or_else(|| {
            helper_validation_error("账号阶段", "当前没有可供 helper 验证的账号，请先添加账号")
        })
        .and_then(|account| validate_helper_for_account(&configured_command, account));
    if let Err(error) = validation {
        restore_file(&settings, previous_settings.as_deref());
        restore_file(&logic_target, previous_logic.as_deref());
        return Err(error);
    }
    if let Some(variant) = variant {
        if let Err(error) = persist_cli_region_env(variant) {
            restore_file(&settings, previous_settings.as_deref());
            restore_file(&logic_target, previous_logic.as_deref());
            return Err(error);
        }
    }

    // 验证通过后再清理旧版 helper.sh，避免失败时破坏旧配置。
    if legacy_target.exists() && legacy_target != target {
        let _ = std::fs::remove_file(&legacy_target);
    }
    if legacy_windows_target.exists() && legacy_windows_target != target {
        let _ = std::fs::remove_file(&legacy_windows_target);
    }

    Ok(json!({
        "ok": true,
        "configured": true,
        "authMode": "api-key-helper",
        "helperPresent": true,
        "helperSupportsAccountIds": true,
        "verified": true,
        "message": "CodeBuddy CLI 认证配置已更新；当前运行会话不会切换，请由 ACP 重新加载会话或重启 CLI 后生效",
    }))
}

/// 把 CodeBuddy CLI 的当前账号设为账号库中的目标账号（手动切换与自动轮换的唯一入口）。
///
/// 顺序固定：**前置校验 → 关闭正在运行的 CLI → 写 `state.json` → 校验 helper/区域环境**。
/// 先关后写是为了维持不变式「活着的 CLI 进程持有的 key == `state.json` 的 activeAccountId」：
/// key 是进程级快照，先写 state 就会留下「新 state + 旧进程仍活」的窗口。
///
/// 两个细节：
/// - 关闭失败 / 没有进程都不阻断切换（只影响返回文案里的数量）；
/// - 关闭**不可回滚**：后续任一阶段失败时，错误里都会说明 CLI 已被关闭。
pub fn switch_active_account(account_id: &str) -> Result<Value, String> {
    if cfg!(windows) {
        ensure_no_process_env_override()?;
    }
    if !cfg!(windows) && helper_migration_required() {
        return Err("检测到旧版 CodeBuddy CLI helper；请先在账号页升级 CLI helper".to_string());
    }
    if !cfg!(windows) && !helper_is_configured() {
        return Err(
            "未检测到 CodeBuddy CLI apiKeyHelper；请先在 ~/.codebuddy/settings.json 配置轮换 helper"
                .to_string(),
        );
    }

    let accounts = account::load_accounts();
    let Some((index, canonical_id)) = account_index(&accounts, account_id) else {
        return Err("账号不存在".to_string());
    };
    let variant = WbVariant::from_account(&accounts[index]);
    ensure_region_env_compatible(variant)?;

    // Windows 先验证并生成完整 settings 值（纯内存，不落盘），避免无效 JSON、
    // 缺失 token 等前置错误造成"CLI 已关、账号没切"。
    let windows_settings = if cfg!(windows) {
        let settings = settings_path();
        let token = settings_account_token(&accounts[index])?;
        let (previous, value) = prepare_settings_env_update(&settings, token, variant)?;
        Some((settings, previous, value, token.to_string()))
    } else {
        None
    };

    // 目录先建好：写 state 是"关进程之后"的事，那一段里不再留失败点。
    std::fs::create_dir_all(rotate_dir()).map_err(|_| {
        if cfg!(windows) {
            auth_config_error("状态阶段", "无法创建 CLI 账号状态目录，请检查用户目录权限")
        } else {
            helper_validation_error("状态阶段", "无法创建 helper 状态目录，请检查用户目录权限")
        }
    })?;

    let previous_state = std::fs::read_to_string(state_path()).ok();
    let switched_at = now_ms();
    let mut state = load_state();
    let previous_variant = current_cli_variant(&accounts, &state);
    state["active"] = json!(index);
    state["activeAccountId"] = json!(canonical_id);
    state["updatedAt"] = json!(switched_at);
    let content = serde_json::to_string_pretty(&state).map_err(|error| error.to_string())?;
    // 顺序关键点：关闭在前、写 state 在后（用例固定这个顺序）。
    let (cli_closed, closed_count) =
        close_then_write_state(&state_path(), &content, close_running_codebuddy_cli)?;

    if let Some((settings, previous_settings, settings_value, token)) = windows_settings {
        if let Err(error) = commit_settings_env_update(
            &settings,
            previous_settings.as_deref(),
            &settings_value,
            &token,
            variant,
        ) {
            restore_file(&state_path(), previous_state.as_deref());
            return Err(after_close_error(error, closed_count));
        }
    } else {
        // helper 校验必须在 state 写完之后：helper 是照 `state.json` 选 token 的，
        // 先校验等于拿旧账号去比对新账号。
        let validation = helper_command()
            .ok_or_else(|| helper_validation_error("配置阶段", "apiKeyHelper 配置为空"))
            .and_then(|command| validate_helper_for_account(&command, &accounts[index]))
            .and_then(|_| persist_cli_region_env(variant));
        if let Err(error) = validation {
            restore_file(&state_path(), previous_state.as_deref());
            return Err(after_close_error(error, closed_count));
        }
    }

    let region_changed = previous_variant != Some(variant);

    // 「最近切换」时间戳（issue #184）：state 与 helper/区域环境都已落盘，切换确实生效后才写。
    // 位置与 `switch::execute` / `codebuddy_ide::switch` 一致——上面任一失败都会提前返回 Err，
    // 那种情况下不记录，避免「提示失败但卡片显示刚切过」。
    // 失败只影响卡片徽标与排序，不得回滚已写入的 state、不得把成功改成报错。
    //
    // 用 `canonical_id`（账号的 `id` 字段）而不是入参 `account_id`：入参允许是 `uid`
    // （见 `account_index`），而 `touch_last_used` 只按 `id` 定位账号。
    let _ = account::touch_last_used(&canonical_id, "cli");

    Ok(json!({
        "ok": true,
        "configured": true,
        "synced": true,
        "verified": true,
        "authMode": if cfg!(windows) { "settings-env" } else { "api-key-helper" },
        "activeIndex": index,
        "activeAccountId": canonical_id,
        "regionChanged": region_changed,
        "cliClosed": cli_closed,
        "closedProcessCount": closed_count,
        "message": region_switch_message(variant, region_changed, closed_count),
    }))
}
