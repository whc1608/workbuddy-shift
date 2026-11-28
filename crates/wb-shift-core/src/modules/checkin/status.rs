//! 签到请求与状态判定：请求候选、状态查询、提交与结果判定。

use super::cycle::*;
use super::*;

pub(super) use crate::modules::config::is_unauthorized;

/// 该响应是否应在刷新/重试判定**之前**原样返回（国际版「未开启 / 未开放 / 已过期」类业务码）。
///
/// 这类提示是业务结果，不是鉴权失败；而 `is_unauthorized` 的弱关键字含「过期」，
/// 若不先短路就会白刷一次 token 并重发一次（刷新失败还会写 `needs_relogin`）。
/// 国内版不做任何短路，语义逐字不变。
pub(super) fn skips_refresh_before_retry(variant: WbVariant, resp: &Value) -> bool {
    if variant != WbVariant::Ai {
        return false;
    }
    let msg = resp
        .get("message")
        .or_else(|| resp.get("msg"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    is_inactive_message(msg)
}

/// 发单次签到请求；遇到未授权且存在 refresh token 时刷新一次并重试。
pub(super) async fn checkin_request_once(path: &str, account: &Value, variant: WbVariant) -> Value {
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
    let url = format!("{}{path}", variant.api_endpoint());
    let headers = build_auth_headers(account);
    let mut resp = http_request(&url, "POST", Some(json!({})), Some(&headers)).await;
    // 国际版「未开放 / 已过期」类业务码：直接返回，绝不刷新、绝不重试。
    if skips_refresh_before_retry(variant, &resp) {
        return resp;
    }
    if crate::modules::at_rest::is_temp(account) {
        // 临时副本的令牌来自客户端：401 只重读 live 一次，**绝不**调刷新接口。
        if is_unauthorized(&resp) {
            if let Some(fresh) = crate::modules::at_rest::reread_after_unauthorized(account) {
                let headers = build_auth_headers(&fresh);
                resp = http_request(&url, "POST", Some(json!({})), Some(&headers)).await;
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
        let headers = build_auth_headers(&refreshed);
        resp = http_request(&url, "POST", Some(json!({})), Some(&headers)).await;
    }
    resp
}

/// 发签到相关请求：路径候选按档位生成，**只有 404 才回落**到下一个候选。
///
/// 401/403（鉴权）、10085（网关指纹拦截）、`code=-1`（传输错误）都必须原样返回：
/// 它们不是路径问题，回落只会掩盖真因并多打一次无意义的请求。
async fn checkin_request(suffix: &str, account: &Value) -> Value {
    let variant = variant_of(account);
    let paths = variant.billing_paths(&format!("{CHECKIN_API_PREFIX}{suffix}"));
    let mut last = json!({"code": -1, "message": "无可用签到路径"});
    for (index, path) in paths.iter().enumerate() {
        let resp = checkin_request_once(path, account, variant).await;
        if index + 1 == paths.len() || !is_route_missing(&resp) {
            return resp;
        }
        last = resp;
    }
    last
}

/// 成功的状态响应 → 结果对象；非成功返回 None。
pub(super) fn status_from_response(resp: &Value) -> Option<Value> {
    let code = resp.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
    if code != 0 && code != 200 {
        return None;
    }
    let data = resp.get("data").cloned().unwrap_or_else(|| json!({}));
    Some(json!({
        "ok": true,
        "todayCheckedIn": data.get("today_checked_in")
            .or_else(|| data.get("todayCheckedIn"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        "raw": data,
    }))
}

/// 查询签到状态：新接口 checkin-activity-status，失败回退 checkin-status。
///
/// `checkin-activity-status` / `checkin-status` 是**国内版专有**接口；国际版没有
/// 对应实现（也没有签到本身）。因此国际版不发起任何请求，直接返回
/// `statusUnsupported: true`；签到链路的其它入口在 `checkin_account` 处统一跳过。
pub async fn get_checkin_status(account: &Value) -> Value {
    if variant_of(account) == WbVariant::Cn {
        let resp = checkin_request("/checkin-activity-status", account).await;
        if let Some(status) = status_from_response(&resp) {
            return status;
        }
        let resp2 = checkin_request("/checkin-status", account).await;
        if let Some(status) = status_from_response(&resp2) {
            return status;
        }
        let code2 = resp2.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
        return json!({
            "ok": false,
            "error": resp2.get("message")
                .or_else(|| resp2.get("msg"))
                .and_then(|v| v.as_str())
                .unwrap_or(&format!("code={code2}"))
                .to_string(),
        });
    }
    json!({
        "ok": false,
        "statusUnsupported": true,
        "error": "该档位暂无签到状态接口",
    })
}

/// 页面展示触发的状态查询遵守单账号开关；手动签到内部仍直接查询服务端。
pub async fn get_checkin_status_for_display(account: &Value) -> Value {
    with_auto_checkin_preference(account, &load_checkin_config(), get_checkin_status(account)).await
}

/// 国际版「功能不可用」类业务提示：签到活动未开启 / 未开放 / 已过期。
pub(super) fn is_inactive_message(message: &str) -> bool {
    let raw = message.to_lowercase();
    [
        "未开启",
        "未开放",
        "已过期",
        "inactive",
        "not enabled",
        "not available",
    ]
    .iter()
    .any(|keyword| raw.contains(keyword))
}

/// 执行签到（POST daily-checkin）；服务端返回已签到提示按成功处理。
pub async fn perform_checkin(account: &Value) -> Value {
    let variant = variant_of(account);
    let resp = checkin_request("/daily-checkin", account).await;
    let code = resp.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
    if code == 0 || code == 200 {
        return json!({"ok": true, "raw": resp.get("data").cloned().unwrap_or_else(|| json!({}))});
    }
    let msg = resp
        .get("message")
        .or_else(|| resp.get("msg"))
        .and_then(|v| v.as_str())
        .unwrap_or(&format!("code={code}"))
        .to_string();
    // 幂等业务码（已签到）保持既有 `already` 语义，两档位一致。
    if msg.contains("已签到") || msg.to_lowercase().contains("repeat") {
        return json!({"ok": true, "already": true, "message": msg});
    }
    // 国际版「功能未开启 / 未开放 / 已过期 / inactive」= 该档位未开放签到，
    // 归类为新增结果 `inactive`：绝不伪造成 success。仅对国际版生效，
    // 国内版保持既有 error 归类（零回归）。
    if variant == WbVariant::Ai && is_inactive_message(&msg) {
        return json!({"ok": false, "inactive": true, "message": msg});
    }
    json!({"ok": false, "error": msg})
}

pub(super) fn decide_from_status(status: &Value) -> StatusDecision {
    // 没有状态查询接口的档位允许直接提交一次 daily-checkin，而不是把账号判成失败
    // 并让调度反复重试。安全性来自 daily-checkin 自身的幂等性——重复提交会返回
    // 「已签到」，因此这里不可能产生重复签到；结果也不会被伪造成 success。
    // 国际版当前在 `checkin_account` 入口即被跳过，此分支保留为防御。
    if status.get("statusUnsupported").and_then(Value::as_bool) == Some(true) {
        return StatusDecision::Submit;
    }
    if status.get("ok").and_then(Value::as_bool) != Some(true) {
        return StatusDecision::Error(
            status
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("查询签到状态失败")
                .to_string(),
        );
    }
    if status.get("todayCheckedIn").and_then(Value::as_bool) == Some(true) {
        StatusDecision::Already
    } else {
        StatusDecision::Submit
    }
}
