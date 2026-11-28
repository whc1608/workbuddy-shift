//! 旅行周期：出发/领取决策与执行、账号同步与两个周期入口。

use super::config::*;
use super::status::*;
use super::*;

pub(super) fn depart_result(
    account: &Value,
    uid: Option<&str>,
    ok: bool,
    already: bool,
    skip: Option<&str>,
    state: Option<&str>,
    message: &str,
) -> Value {
    json!({
        "accountId": account_key(account),
        "uid": uid,
        "ok": ok,
        "already": already,
        "skip": skip,
        "state": state,
        "message": message,
        "claimed": false,
        "rewardCredit": Value::Null,
        "claimedAt": 0,
        "at": now_ms(),
    })
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum DepartClass {
    AlreadyTraveling,
    DailyLimit,
    NoBuddy,
    LocationUnavailable,
    Other,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum TravelAction {
    Claim,
    WaitTraveling,
    SkipDailyLimit,
    Depart,
    StatusError,
}

/// 与官方网页同一套判断：先看 state，再看 daily_limit_reached。
/// idle + daily_limit_reached 即官网「累了，明天再来吧」。
pub(super) fn decide_travel_action(state: &str, daily_limit_reached: bool) -> TravelAction {
    match state {
        "arrived" => TravelAction::Claim,
        "traveling" => TravelAction::WaitTraveling,
        "idle" if daily_limit_reached => TravelAction::SkipDailyLimit,
        "idle" => TravelAction::Depart,
        _ => TravelAction::StatusError,
    }
}

/// HTTP 429 是限流，不是「今日已派」。只有文案明确 daily limit 才算当日完成。
pub(super) fn classify_depart_error(_code: i64, message: &str) -> DepartClass {
    let raw = message.to_lowercase();
    if raw.contains("already traveling") {
        DepartClass::AlreadyTraveling
    } else if raw.contains("daily limit") || raw.contains("daily_limit") {
        DepartClass::DailyLimit
    } else if raw.contains("no active buddy") {
        DepartClass::NoBuddy
    } else if raw.contains("location not available") {
        DepartClass::LocationUnavailable
    } else {
        DepartClass::Other
    }
}

/// 对单个账号执行派猫猫旅行：依次尝试地点列表，报错分类处理。
pub async fn depart_travel_for_account(account: &Value) -> Value {
    // 国际版无成长中心：不发请求、不写缓存（design D7）。
    if !variant_of(account).supports_travel() {
        return unsupported_variant_skip();
    }
    let cfg = load_checkin_config();
    // R6-06：**必须在 ensure_fresh_token 之前**解析（信封账号先拿到临时副本，
    // 刷新守卫直接静默返回），否则每次都会先走一遍 skipped_encrypted。
    let resolved = crate::modules::at_rest::usable_account(
        account,
        crate::modules::at_rest::AccessMode::Write,
    );
    let account = resolved.as_ref().unwrap_or(account);
    let acc = ensure_fresh_token(account.clone(), &cfg).await;
    let uid = acc.get("uid").and_then(Value::as_str).map(String::from);
    let uid_ref = uid.as_deref();

    let config = fetch_travel_config(&acc).await;
    if config.get("ok").and_then(Value::as_bool) != Some(true) {
        return depart_result(
            &acc,
            uid_ref,
            false,
            false,
            Some("config-error"),
            None,
            config
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("读取旅行配置失败"),
        );
    }
    let locations = config
        .get("locations")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if config.get("enabled").and_then(Value::as_bool) != Some(true) || locations.is_empty() {
        return depart_result(
            &acc,
            uid_ref,
            false,
            false,
            Some("no-location"),
            None,
            "无旅行地点",
        );
    }

    let mut last_unavailable = false;
    for location in &locations {
        let location_id = location.get("id").cloned().unwrap_or(Value::Null);
        let res = depart_travel(&acc, &location_id).await;
        if res.get("ok").and_then(Value::as_bool) == Some(true) {
            let mut result = depart_result(
                &acc,
                uid_ref,
                true,
                false,
                None,
                Some(
                    res.get("state")
                        .and_then(Value::as_str)
                        .unwrap_or("traveling"),
                ),
                "",
            );
            if let Some(name) = nonempty_str(location.get("name").unwrap_or(&Value::Null)) {
                result["locationName"] = json!(name);
            }
            let status = fetch_travel_status(&acc).await;
            if status.get("ok").and_then(Value::as_bool) == Some(true) {
                apply_status_record(&mut result, &status);
            }
            return result;
        }

        let raw = res.get("message").and_then(Value::as_str).unwrap_or("");
        let code = res.get("code").and_then(Value::as_i64).unwrap_or(-1);
        match classify_depart_error(code, raw) {
            DepartClass::AlreadyTraveling => {
                return depart_result(
                    &acc,
                    uid_ref,
                    true,
                    true,
                    None,
                    Some("traveling"),
                    "已在旅行中",
                );
            }
            DepartClass::DailyLimit => {
                let mut result = depart_result(
                    &acc,
                    uid_ref,
                    true,
                    true,
                    Some("daily-limit"),
                    Some("idle"),
                    "今日已派",
                );
                result["claimed"] = json!(true);
                result["claimedAt"] = json!(now_ms());
                return result;
            }
            DepartClass::NoBuddy => {
                return depart_result(
                    &acc,
                    uid_ref,
                    false,
                    false,
                    Some("no-buddy"),
                    None,
                    "无 Buddy",
                );
            }
            DepartClass::LocationUnavailable => {
                last_unavailable = true;
            }
            DepartClass::Other => {
                return depart_result(
                    &acc,
                    uid_ref,
                    false,
                    false,
                    Some("error"),
                    None,
                    &truncate_message(&raw.to_lowercase()),
                );
            }
        }
    }

    if last_unavailable {
        depart_result(
            &acc,
            uid_ref,
            false,
            false,
            Some("location-unavailable"),
            None,
            "地点不可用",
        )
    } else {
        depart_result(&acc, uid_ref, false, false, Some("error"), None, "派发失败")
    }
}

/// 领取单账号旅行奖励。未知/缺 state 保持未领取；idle 不默认当成已领。
async fn claim_travel_for_account(account: &Value, prior: &Value) -> Value {
    let mut result = prior.clone();
    let cfg = load_checkin_config();
    // R6-06：**必须在 ensure_fresh_token 之前**解析（信封账号先拿到临时副本，
    // 刷新守卫直接静默返回），否则每次都会先走一遍 skipped_encrypted。
    let resolved = crate::modules::at_rest::usable_account(
        account,
        crate::modules::at_rest::AccessMode::Write,
    );
    let account = resolved.as_ref().unwrap_or(account);
    let acc = ensure_fresh_token(account.clone(), &cfg).await;

    let status = fetch_travel_status(&acc).await;
    if status.get("ok").and_then(Value::as_bool) != Some(true) {
        result["skip"] = json!("status-error");
        result["message"] = json!(status
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("查询旅行状态失败"));
        return result;
    }

    let state = status.get("state").and_then(Value::as_str).unwrap_or("");
    let record_id = status.get("recordId").and_then(Value::as_i64).unwrap_or(0);
    match state {
        "traveling" => {
            result["claimed"] = json!(false);
            result["ok"] = json!(true);
            result["state"] = json!("traveling");
            apply_status_record(&mut result, &status);
        }
        "arrived" => {
            apply_status_record(&mut result, &status);
            apply_claim_response(&acc, &mut result, record_id).await;
        }
        "idle" => {
            let daily_limit = status
                .get("dailyLimitReached")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if daily_limit {
                apply_daily_limit_reached(&mut result, &status);
            } else if result_claimed(&result) {
                result["state"] = json!("idle");
            } else if result.get("ok").and_then(Value::as_bool) == Some(true) {
                apply_claim_response(&acc, &mut result, record_id).await;
            } else {
                result["claimed"] = json!(false);
                result["state"] = json!("idle");
            }
        }
        _ => {
            result["skip"] = json!("status-error");
            result["claimed"] = json!(false);
            result["message"] = json!(format!("unknown travel state: {state}"));
        }
    }
    result
}

async fn apply_claim_response(account: &Value, result: &mut Value, record_id: i64) {
    let claim = claim_travel(account, record_id).await;
    if claim.get("ok").and_then(Value::as_bool) == Some(true) {
        mark_claimed(
            result,
            Some(claim.get("rewardCredit").cloned().unwrap_or(Value::Null)),
            None,
        );
        return;
    }
    let raw = claim
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_lowercase();
    if raw.contains("no unclaimed travel") || raw.contains("daily_limit") {
        mark_claimed(result, None, Some("已领取（网页端）"));
    } else if raw.contains("not arrived yet") {
        result["claimed"] = json!(false);
        result["ok"] = json!(true);
        result["state"] = json!("arrived");
    } else {
        result["skip"] = json!("claim-error");
        result["message"] = json!(truncate_message(&raw));
    }
}

pub(super) async fn sync_account_for_dispatch(account: &Value, prior: Option<&Value>) -> Value {
    let cfg = load_checkin_config();
    // R6-06：**必须在 ensure_fresh_token 之前**解析（信封账号先拿到临时副本，
    // 刷新守卫直接静默返回），否则每次都会先走一遍 skipped_encrypted。
    let resolved = crate::modules::at_rest::usable_account(
        account,
        crate::modules::at_rest::AccessMode::Write,
    );
    let account = resolved.as_ref().unwrap_or(account);
    let acc = ensure_fresh_token(account.clone(), &cfg).await;
    let uid = acc.get("uid").and_then(Value::as_str).map(String::from);
    let uid_ref = uid.as_deref();

    let status = fetch_travel_status(&acc).await;
    if status.get("ok").and_then(Value::as_bool) != Some(true) {
        return depart_result(
            &acc,
            uid_ref,
            false,
            false,
            Some("status-error"),
            None,
            status
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("查询旅行状态失败"),
        );
    }

    let state = status.get("state").and_then(Value::as_str).unwrap_or("");
    let daily_limit = status
        .get("dailyLimitReached")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    match decide_travel_action(state, daily_limit) {
        TravelAction::WaitTraveling => {
            let mut result = depart_result(
                &acc,
                uid_ref,
                true,
                true,
                None,
                Some("traveling"),
                "已在旅行中",
            );
            apply_status_record(&mut result, &status);
            result
        }
        TravelAction::Claim => {
            let stub = prior.cloned().unwrap_or_else(|| {
                depart_result(&acc, uid_ref, true, true, None, Some("arrived"), "已到达")
            });
            claim_travel_for_account(account, &stub).await
        }
        TravelAction::SkipDailyLimit => {
            let mut result = prior.cloned().unwrap_or_else(|| {
                depart_result(
                    &acc,
                    uid_ref,
                    true,
                    true,
                    Some("daily-limit"),
                    Some("idle"),
                    "今日已派",
                )
            });
            apply_daily_limit_reached(&mut result, &status);
            result
        }
        TravelAction::Depart => depart_travel_for_account(account).await,
        TravelAction::StatusError => depart_result(
            &acc,
            uid_ref,
            false,
            false,
            Some("status-error"),
            None,
            &format!("unknown travel state: {state}"),
        ),
    }
}

/// 对所有账号依次派猫猫旅行（每日缓存幂等；存在可重试项时不标记当日完成）。
pub async fn run_travel_cycle() -> Value {
    let Some(_guard) = RunFlagGuard::try_acquire(&TRAVEL_RUNNING) else {
        return json!({"status": "skipped", "reason": "already_running"});
    };
    let cfg = load_travel_config();
    if cfg.get("enabled").and_then(Value::as_bool) != Some(true) {
        return json!({"status": "disabled"});
    }
    let accounts = load_accounts();
    if accounts.is_empty() {
        return json!({"status": "no_accounts"});
    }
    // 档位不支持成长中心时不发请求、不写缓存（design D7）。
    let accounts = travel_capable_accounts(accounts);
    if accounts.is_empty() {
        return unsupported_variant_skip();
    }

    let today = today_str();
    let mut cache = load_travel_cache();
    roll_cache_to_today(&mut cache, &today);

    let prior_results = cache_results(&cache).cloned().unwrap_or_default();
    let mut results = Map::new();
    let mut summary_accounts = Vec::new();
    for acc in &accounts {
        let id = account_key(acc);
        let prior = prior_results.get(&id);
        let r = sync_account_for_dispatch(acc, prior).await;
        let r = if let Some(prior) = prior {
            merge_claim_state(prior, &r)
        } else {
            r
        };
        let skip = r.get("skip").and_then(Value::as_str);
        let result = if r.get("ok").and_then(Value::as_bool) == Some(true) {
            "success"
        } else {
            "error"
        };
        summary_accounts.push(json!({
            "accountId": id,
            "email": account_display_name(acc),
            "result": result,
            "skip": skip,
            "message": r.get("message").cloned().unwrap_or(Value::Null),
        }));
        results.insert(id, r);
    }

    let overlay = json!({
        "date": today,
        "results": Value::Object(results.clone()),
    });
    persist_travel_cache(&overlay);

    json!({
        "status": "ok",
        "completed": !has_retryable_results(&results),
        "accounts": summary_accounts,
    })
}

/// 检查并领取未领奖励。不绑死「今天」的 cache.date，跨日仍处理 traveling/arrived。
pub async fn run_travel_claim_cycle() -> Value {
    let Some(_guard) = RunFlagGuard::try_acquire(&TRAVEL_CLAIM_RUNNING) else {
        return json!({"status": "skipped", "reason": "already_running"});
    };
    let mut cache = load_travel_cache();
    let Some(results) = cache.get_mut("results").and_then(Value::as_object_mut) else {
        return json!({"status": "skipped", "reason": "nothing-to-claim"});
    };

    let ids: Vec<String> = results
        .iter()
        .filter(|(_, r)| result_in_flight(r))
        .map(|(id, _)| id.clone())
        .collect();
    if ids.is_empty() {
        return json!({"status": "skipped", "reason": "nothing-to-claim"});
    }

    let accounts = travel_capable_accounts(load_accounts());
    let mut claimed = 0;
    let total = ids.len();
    let mut overlay_results = Map::new();
    for id in &ids {
        let Some(account) = accounts
            .iter()
            .find(|a| account_key(a).as_str() == id.as_str())
        else {
            continue;
        };
        let prior = results.get(id).cloned().unwrap_or_else(|| json!({}));
        let updated = claim_travel_for_account(account, &prior).await;
        if result_claimed(&updated) {
            claimed += 1;
        }
        overlay_results.insert(id.clone(), updated);
    }
    let overlay = json!({
        "date": cache.get("date").cloned().unwrap_or(Value::Null),
        "results": Value::Object(overlay_results),
    });
    persist_travel_cache(&overlay);

    json!({ "status": "ok", "total": total, "claimed": claimed })
}
