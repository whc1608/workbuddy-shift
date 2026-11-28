//! 签到执行与调度：单账号流程、自动轮次、批量入口与并发守卫。

use super::logs::*;
use super::status::*;
use super::window::*;
use super::*;
use crate::modules::locks;

pub(super) static CHECKIN_RUNNING: AtomicBool = AtomicBool::new(false);
pub(super) static CHECKIN_ACCOUNTS_RUNNING: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

/// 上一轮是否有账号提交/查询失败（进程内，不落盘）。
pub(super) static LAST_CYCLE_HAD_ERRORS: AtomicBool = AtomicBool::new(false);
/// 窗口模式下的快速重试预算；干净轮次重置为满额。
pub(super) static FAST_RETRY_BUDGET: AtomicU32 = AtomicU32::new(FAST_RETRY_MAX);

/// Automatic recovery cadence shared by every host.
pub const CHECKIN_RECOVERY_INTERVAL: Duration = Duration::from_secs(30 * 60);

/// 窗口模式下到点失败后的快速重试间隔。
pub(super) const FAST_RETRY_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// 快速重试次数上限（12 × 5 分钟 ≈ 1 小时）；耗尽后回落 30 分钟一轮。
pub(super) const FAST_RETRY_MAX: u32 = 12;

/// 窗口模式下两轮之间的最小间隔（到点即办）。
pub(super) const MIN_CYCLE_DELAY: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CheckinCycleMode {
    /// Always verify every account against the server after a host starts.
    StartupVerify,
    /// Re-verify every account against the server during background recovery.
    PeriodicRecovery,
}

#[derive(Debug, Eq, PartialEq)]
pub(super) enum StatusDecision {
    Already,
    Submit,
    Error(String),
}

pub(super) struct AccountRunGuard {
    pub(super) key: String,
}

impl AccountRunGuard {
    /// 取得账号级运行标记：`Ok(None)` = 该账号正在签到；`Err` = 运行标记损坏。
    pub(super) fn try_acquire(account: &Value) -> Result<Option<Self>, String> {
        let key = account
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .map(String::from)
            .unwrap_or_else(|| account_display_name(account));
        let mut running = locks::guard(
            CHECKIN_ACCOUNTS_RUNNING.get_or_init(|| Mutex::new(HashSet::new())),
            "签到",
        )?;
        if !running.insert(key.clone()) {
            return Ok(None);
        }
        Ok(Some(Self { key }))
    }
}

impl Drop for AccountRunGuard {
    fn drop(&mut self) {
        // 移除自己插入的 key 是幂等操作：中毒时也必须继续，否则该账号会永久
        // 停在「正在签到」而无法再次签到。
        if let Some(running) = CHECKIN_ACCOUNTS_RUNNING.get() {
            locks::recover(running).remove(&self.key);
        }
    }
}

/// 对单个账号执行完整签到流程：惰性刷新 → 查状态 → 未签到时提交 → 写提交日志。
pub async fn checkin_account(account: &Value) -> Value {
    let _account_guard = match AccountRunGuard::try_acquire(account) {
        Ok(Some(guard)) => guard,
        Ok(None) => return json!({"result": "error", "error": "该账号正在签到，请稍后再试"}),
        Err(error) => return json!({"result": "error", "error": error}),
    };
    // 国际版没有签到接口：自动周期、一键签到与单账号签到的公开入口都在这里汇聚，
    // 统一短路以确保不向任何签到接口发起请求。
    if !variant_of(account).supports_checkin() {
        return json!({"result": "skipped", "reason": "unsupported_variant"});
    }
    let cfg = load_checkin_config();
    // R6-06：**必须在 ensure_fresh_token 之前**解析，否则信封账号会先被 R6-01 的
    // SkipEncrypted 挡一次（日志里出现无意义的 skipped_encrypted）才走到解密分支。
    let resolved = crate::modules::at_rest::usable_account(
        account,
        crate::modules::at_rest::AccessMode::Write,
    );
    let account = resolved.as_ref().unwrap_or(account);
    let acc = ensure_fresh_token(account.clone(), &cfg).await;
    let variant = variant_of(&acc).as_str();
    let status = get_checkin_status(&acc).await;
    match decide_from_status(&status) {
        StatusDecision::Already => return json!({"result": "already"}),
        StatusDecision::Error(error) => {
            return json!({"result": "error", "error": error});
        }
        StatusDecision::Submit => {}
    }

    // Only this branch submits daily-checkin, so only its outcome is eligible
    // for the sign-in log.
    let entry = json!({
        "ts": now_ms(),
        "accountId": acc.get("id").cloned().unwrap_or(Value::Null),
        "email": account_display_name(&acc),
        "variant": variant,
    });
    let res = perform_checkin(&acc).await;
    if res.get("inactive").and_then(|v| v.as_bool()) == Some(true) {
        // inactive 语义（新增）：既不能记为成功，也不应让统计页计为失败，
        // 因此**不写签到日志**（写了必然被算作 success 或 failed 之一），
        // 也不做任何重试；结果对象显式带 inactive: true 供上层区分。
        return json!({
            "result": "inactive",
            "inactive": true,
            "message": res.get("message").cloned().unwrap_or(Value::Null),
        });
    }
    let result = if res.get("ok").and_then(|v| v.as_bool()) == Some(true) {
        if res.get("already").and_then(|v| v.as_bool()) == Some(true) {
            "already"
        } else {
            "success"
        }
    } else {
        "error"
    };
    let error = if result == "error" {
        res.get("error")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    } else {
        None
    };
    let mut entry_map = json!({
        "result": result,
        "ts": entry["ts"],
        "accountId": entry["accountId"],
        "email": entry["email"],
        "variant": entry["variant"],
    });
    if let Some(e) = error.clone() {
        entry_map["error"] = json!(e);
    }
    add_checkin_log(&entry_map);
    json!({"result": result, "error": error})
}

/// 只匹配稳定 id，不以昵称或邮箱排除其它账号。
pub(super) fn auto_checkin_excluded(account: &Value, cfg: &Value) -> bool {
    account.get("id").and_then(Value::as_str).is_some_and(|id| {
        cfg.get("excluded_account_ids")
            .and_then(Value::as_array)
            .is_some_and(|ids| ids.iter().any(|value| value.as_str() == Some(id)))
    })
}

pub(super) fn allows_auto_checkin(account: &Value, cfg: &Value) -> bool {
    variant_of(account).supports_checkin() && !auto_checkin_excluded(account, cfg)
}

/// 在执行异步操作前检查偏好：被排除时不 poll future，因此不会查询、刷新 token 或提交。
pub(super) async fn with_auto_checkin_preference(
    account: &Value,
    cfg: &Value,
    operation: impl Future<Output = Value>,
) -> Value {
    if auto_checkin_excluded(account, cfg) {
        return json!({"ok": false, "result": "skipped", "reason": "auto_checkin_disabled"});
    }
    operation.await
}

/// 自动调度、时间段计划与批量签到共用的账号集合；单账号手动签到不经过这里。
pub(super) fn auto_checkin_accounts(accounts: Vec<Value>, cfg: &Value) -> Vec<Value> {
    accounts
        .into_iter()
        .filter(|acc| allows_auto_checkin(acc, cfg))
        .collect()
}

/// 账号标识：稳定 id，缺失时回落展示名（与 `AccountRunGuard` 同口径）。
pub(super) fn account_key(account: &Value) -> String {
    account
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(String::from)
        .unwrap_or_else(|| account_display_name(account))
}

/// 执行一轮自动签到。启动与周期轮次均逐账号查询服务端状态。
///
/// 只遍历支持签到且允许自动签到的账号。并发锁防止与
/// 手动签到/上一轮重复运行。
///
/// 时间段（`checkin_start` / `checkin_end`）按 `schedule_window` 取，窗口与「到点」
/// 口径成对给出，避免两边不一致：
///
/// - 用户配置了合法窗口：沿用既有口径（允许跨日补签），行为不变；
/// - 未配置或非法：退化为默认窗口 8:00–22:00，并改用「只看今日」口径——窗口内今天还没
///   到点的账号一律不处理；窗口外每轮限量补签（`OUTSIDE_WINDOW_BATCH`），只在夜间开机的
///   用户不会被整天漏签，也不会一轮打完全部。
///
/// 只有「到点未办」的账号才处理：整轮没有账号到点时返回
/// `{"status":"skipped","reason":"before_checkin_time","accounts":[]}`。
///
/// 「未配置时间段」不再等于「一轮签完所有账号」：那样 34 个账号会在 1 分钟内串行提交，
/// 是最像脚本的行为指纹；默认窗口让每个账号落到自己那一天的目标分钟。跨日补签若也
/// 套在默认窗口上，会把这个分散效果抵消掉（昨天的目标在今天 14:00 就顶成到点），
/// 所以默认兜底必须用 `DueScope::TodayOnly`。
pub async fn run_checkin_cycle(_mode: CheckinCycleMode) -> Value {
    let Some(_guard) = RunFlagGuard::try_acquire(&CHECKIN_RUNNING) else {
        return json!({"status": "skipped", "reason": "already_running"});
    };
    let cfg = load_checkin_config();
    if cfg.get("enabled").and_then(|v| v.as_bool()) != Some(true) {
        return json!({"status": "disabled"});
    }
    let accounts = auto_checkin_accounts(load_accounts(), &cfg);
    if accounts.is_empty() {
        return json!({"status": "no_accounts"});
    }
    // 未配置时间段时用默认窗口，而不是退化成「不限时间、全部账号」。
    // 口径必须跟着来源走：配置窗口保留跨日补签；默认窗口只看今日，否则补签会在
    // 今天第一轮就把「今天晚点才轮到」的账号全部顶成到点，分散能力归零。
    let ScheduleWindow { window, scope } = schedule_window(&cfg);
    let accounts = due_accounts(
        accounts,
        now_ms(),
        Some(window),
        &load_checkin_logs(),
        scope,
    );
    if accounts.is_empty() {
        // 整轮没有任何账号到点（含默认窗口），不发任何请求。
        record_cycle_health(false);
        return json!({"status": "skipped", "reason": "before_checkin_time", "accounts": []});
    }
    let mut summary = json!({"status": "ok", "accounts": []});
    let mut had_errors = false;
    for acc in accounts {
        let result = checkin_account(&acc).await;
        let outcome = result.get("result").and_then(Value::as_str);
        if outcome == Some("error") {
            had_errors = true;
        }
        if outcome == Some("already") {
            // 到点复核发现服务端已签（例如用户白天手动签过）→ 补一条窗口回执。
            write_window_receipt(window, &acc);
        }
        let mut row = json!({
            "email": account_display_name(&acc),
            "result": result.get("result").cloned().unwrap_or(Value::Null),
            "error": result.get("error").cloned().unwrap_or(Value::Null),
        });
        // 只在 inactive 时追加标记：国内版结果行结构与改造前逐字一致。
        if result.get("inactive").and_then(Value::as_bool) == Some(true) {
            row["inactive"] = json!(true);
        }
        summary["accounts"].as_array_mut().unwrap().push(row);
    }
    record_cycle_health(had_errors);
    summary
}

/// True when every stored account has a today's log of `success` or `already`.
///
/// Empty account list is false so the tray keeps offering 一键签到.
pub fn all_accounts_checked_in_today() -> bool {
    accounts_checked_in_today(
        &load_accounts(),
        &load_checkin_logs(),
        &date_str(None),
        &load_checkin_config(),
    )
}

/// 判定「今天是否所有应签到的账号都已签到」。
///
/// 只有允许自动签到的账号（见 `allows_auto_checkin`）参与判定：国际版没有签到接口，
/// 关闭自动签到的账号也不会产生签到日志，若把它们算进来，托盘会永远显示「可签到」。
/// 有账号但没有任何账号需要签到（例如只装了国际版、或全部关闭自动签到）时视为无需
/// 签到，返回 true；账号库为空仍返回 false，保留「一键签到」入口。
pub fn accounts_checked_in_today(
    accounts: &[Value],
    logs: &[Value],
    today: &str,
    cfg: &Value,
) -> bool {
    if accounts.is_empty() {
        return false;
    }
    let pending: Vec<&Value> = accounts
        .iter()
        .filter(|account| allows_auto_checkin(account, cfg))
        .collect();
    if pending.is_empty() {
        return true;
    }
    pending.iter().all(|account| {
        let Some(id) = account.get("id").and_then(Value::as_str) else {
            return false;
        };
        latest_today_result(logs, id, today)
            .map(|result| result == "success" || result == "already")
            .unwrap_or(false)
    })
}

/// 对全部账号批量签到（前端「刷新并签到」/ 一键签到）。
///
/// `variant = None` 覆盖全部档位（设置页与托盘「立即签到」语义）；
/// 显式传入时只处理该档位（账号页按当前档位触发，避免跨档位误签到）。
/// 无论哪种取值，都只处理支持签到的档位：国际版没有签到接口，绝不发起请求；
/// 关闭自动签到的账号同样跳过，并逐账号返回 skipped 原因。
///
/// `respect_window` 只由账号页「刷新并签到」传 `true`：窗口生效且当前不在
/// `[start, end)` 内时整轮跳过（逐账号 `skipped` / `outside_checkin_window`，
/// 返回 `{"status":"skipped","reason":"outside_checkin_window"}`），窗口外不发
/// 任何签到请求。设置页 / 托盘 / 单账号手动入口传 `false`，保持立即签到语义。
pub async fn run_checkin_all(variant: Option<WbVariant>, respect_window: bool) -> Value {
    let Some(_guard) = RunFlagGuard::try_acquire(&CHECKIN_RUNNING) else {
        return json!({"accounts": [], "status": "skipped", "reason": "already_running"});
    };
    let cfg = load_checkin_config();
    let accounts: Vec<Value> = load_accounts()
        .into_iter()
        .filter(|acc| {
            let acc_variant = variant_of(acc);
            acc_variant.supports_checkin() && variant.is_none_or(|target| acc_variant == target)
        })
        .collect();
    if let Some(rows) = manual_window_skip_rows(&accounts, &cfg, respect_window, now_ms()) {
        return json!({"accounts": rows, "status": "skipped", "reason": "outside_checkin_window"});
    }
    json!({"accounts": checkin_all_rows(accounts, &cfg).await})
}

/// 批量签到的逐账号结果行；被排除账号不 poll 操作，因此不会发起任何请求。
pub(super) async fn checkin_all_rows(accounts: Vec<Value>, cfg: &Value) -> Vec<Value> {
    let mut results: Vec<Value> = Vec::new();
    for acc in accounts {
        let r = with_auto_checkin_preference(&acc, cfg, checkin_account(&acc)).await;
        let mut row = json!({
            "accountId": acc.get("id").cloned().unwrap_or(Value::Null),
            "email": account_display_name(&acc),
            "result": r.get("result").cloned().unwrap_or(Value::Null),
            "error": r.get("error").cloned().unwrap_or(Value::Null),
        });
        if r.get("inactive").and_then(Value::as_bool) == Some(true) {
            row["inactive"] = json!(true);
        }
        if let Some(reason) = r.get("reason") {
            row["reason"] = reason.clone();
        }
        results.push(row);
    }
    results
}
