//! 旅行请求与配置：档位能力过滤、请求封装（配置/状态/出发/领取）与取数。

use super::status::*;
use super::*;

/// 档位不支持成长中心时的统一短路结果（design D7：国际版无派猫猫旅行）。
///
/// 直接返回而不发请求、不写缓存：否则后台循环会持续对国际版账号打无效请求，
/// 并把脏数据写进 `travel_cache.json`。
pub fn unsupported_variant_skip() -> Value {
    json!({"status": "skipped", "reason": "unsupported_variant"})
}

/// 只保留支持成长中心（派猫猫旅行）的账号。
pub(super) fn travel_capable_accounts(accounts: Vec<Value>) -> Vec<Value> {
    accounts
        .into_iter()
        .filter(|account| variant_of(account).supports_travel())
        .collect()
}

pub(super) fn build_travel_headers(account: &Value) -> HashMap<String, String> {
    let mut headers = build_auth_headers(account);
    headers.insert("x-client-platform".to_string(), "web".to_string());
    headers.insert("origin".to_string(), WORKBUDDY_API_ENDPOINT.to_string());
    headers.insert(
        "referer".to_string(),
        format!("{WORKBUDDY_API_ENDPOINT}/profile/growth-center"),
    );
    headers
}

pub(super) use crate::modules::config::is_unauthorized;

/// 发旅行接口请求；遇到未授权且存在 refresh token 时刷新一次并重试。
pub(super) async fn travel_request(
    path: &str,
    method: &str,
    body: Option<Value>,
    account: &Value,
) -> Value {
    // R6-06：先尝试用本机 live 凭据解出临时副本（写开关打开时才生效）；
    // 解不出来才回退信封提示（issue #94 的文案与形状逐字不变）。
    let resolved = match crate::modules::at_rest::resolve_or_envelope_error(
        account,
        crate::modules::at_rest::AccessMode::Write,
    ) {
        Ok(temp) => temp,
        Err(err) => return json!({"code": -2, "message": err}),
    };
    let account = resolved.as_ref().unwrap_or(account);
    let url = format!("{WORKBUDDY_API_ENDPOINT}{path}");
    let headers = build_travel_headers(account);
    let mut resp = http_request(&url, method, body.clone(), Some(&headers)).await;
    if crate::modules::at_rest::is_temp(account) {
        // 临时副本的令牌来自客户端：401 只重读 live 一次，**绝不**调刷新接口。
        if is_unauthorized(&resp) {
            if let Some(fresh) = crate::modules::at_rest::reread_after_unauthorized(account) {
                let headers = build_travel_headers(&fresh);
                resp = http_request(&url, method, body, Some(&headers)).await;
            }
        }
        return resp;
    }
    if is_unauthorized(&resp)
        && !account
            .get("refresh_token")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .is_empty()
    {
        let refreshed = refresh_account_token(account.clone()).await;
        let headers = build_travel_headers(&refreshed);
        resp = http_request(&url, method, body, Some(&headers)).await;
    }
    resp
}

pub(super) fn resp_error(resp: &Value, fallback_code: i64) -> String {
    resp.get("message")
        .or_else(|| resp.get("msg"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("code={fallback_code}"))
}

/// 读取旅行配置（地点列表等）。返回 `{ ok, enabled, locations: [{ id, name }] }`。
pub(super) async fn fetch_travel_config(account: &Value) -> Value {
    let resp = travel_request(&format!("{TRAVEL_API_PREFIX}/config"), "GET", None, account).await;
    let code = resp.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
    if code == 0 || code == 200 {
        let data = resp.get("data").cloned().unwrap_or_else(|| json!({}));
        let enabled_flag = data.get("enabled").and_then(Value::as_bool).unwrap_or(true);
        let locations: Vec<Value> = data
            .get("locations")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|l| {
                json!({
                    "id": l.get("id").cloned().unwrap_or(Value::Null),
                    "name": l.get("name").and_then(Value::as_str).unwrap_or(""),
                })
            })
            .collect();
        return json!({
            "ok": true,
            "enabled": enabled_flag && !locations.is_empty(),
            "locations": locations,
        });
    }
    json!({
        "ok": false,
        "error": resp_error(&resp, code),
    })
}

/// 读取当前旅行状态。缺 state / 未知 state 视为失败，避免被当成 idle 后误标已领取。
///
/// 官方以 `data.state` + `data.daily_limit_reached` 判断能否派出：
/// arrived → 领奖；traveling → 等待；idle 且未达每日上限 → 可 depart。
pub(super) async fn fetch_travel_status(account: &Value) -> Value {
    let resp = travel_request(&format!("{TRAVEL_API_PREFIX}/status"), "GET", None, account).await;
    let code = resp.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
    if code == 0 || code == 200 {
        let data = resp.get("data").cloned().unwrap_or_else(|| json!({}));
        let raw_state = data.get("state").and_then(Value::as_str);
        let Some(state) = parse_travel_state(raw_state) else {
            return json!({
                "ok": false,
                "error": match raw_state {
                    Some(value) if !value.trim().is_empty() => format!("unknown travel state: {value}"),
                    _ => "missing travel state".to_string(),
                },
            });
        };
        return json!({
            "ok": true,
            "state": state,
            "locationId": data
                .get("location")
                .and_then(|l| l.get("id"))
                .cloned()
                .unwrap_or(Value::Null),
            "departAt": data.get("depart_at").and_then(Value::as_i64).unwrap_or(0),
            "arriveAt": data.get("arrive_at").and_then(Value::as_i64).unwrap_or(0),
            "dailyLimitReached": data.get("daily_limit_reached").and_then(Value::as_bool).unwrap_or(false),
            "buddyId": data.get("buddy_id").and_then(Value::as_i64).unwrap_or(0),
            "recordId": data.get("record_id").and_then(Value::as_i64).unwrap_or(0),
            "serverNow": data.get("server_now").and_then(Value::as_i64).unwrap_or(0),
            "locationName": data
                .get("location")
                .and_then(|location| location.get("name"))
                .and_then(Value::as_str)
                .unwrap_or(""),
            "rewardCredit": data.get("reward_credit").cloned().unwrap_or(Value::Null),
        });
    }
    json!({
        "ok": false,
        "error": resp_error(&resp, code),
    })
}

/// 派猫猫旅行（depart）。返回 `{ ok, state }` 或 `{ ok:false, message }`。
pub(super) async fn depart_travel(account: &Value, location_id: &Value) -> Value {
    let resp = travel_request(
        &format!("{TRAVEL_API_PREFIX}/depart"),
        "POST",
        Some(json!({ "location_id": location_id })),
        account,
    )
    .await;
    let code = resp.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
    if code == 0 || code == 200 {
        let data = resp.get("data").cloned().unwrap_or_else(|| json!({}));
        return json!({
            "ok": true,
            "state": data.get("state").and_then(Value::as_str).unwrap_or("traveling"),
        });
    }
    json!({
        "ok": false,
        "code": code,
        "message": resp_error(&resp, code),
    })
}

/// 领取旅行奖励（state==='arrived' 时调用）。返回 `{ ok, rewardCredit }` 或 `{ ok:false, message }`。
pub(super) async fn claim_travel(account: &Value, record_id: i64) -> Value {
    let payload = if record_id > 0 {
        json!({ "record_id": record_id })
    } else {
        json!({})
    };
    let resp = travel_request(
        &format!("{TRAVEL_API_PREFIX}/claim"),
        "POST",
        Some(payload),
        account,
    )
    .await;
    let code = resp.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
    if code == 0 || code == 200 {
        let data = resp.get("data").cloned().unwrap_or_else(|| json!({}));
        let reward_credit = data
            .get("reward_credit")
            .and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)));
        return json!({ "ok": true, "rewardCredit": reward_credit });
    }
    json!({
        "ok": false,
        "code": code,
        "message": resp_error(&resp, code),
    })
}
