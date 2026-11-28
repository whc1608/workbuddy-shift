//! WorkBuddy 积分资源查询。
//!
//! WorkBuddy 套餐页使用 summary/paid/free 三个资源接口；旧的
//! `POST /v2/billing/meter/get-user-resource` 仍作为兼容回退。
//! 这里仅返回脱敏后的资源摘要，不把 token 或完整响应交给前端。

use chrono::{Local, NaiveDate, NaiveDateTime, TimeZone};
use serde_json::{json, Value};
use std::collections::HashSet;

use crate::modules::account::{
    account_display_name, build_auth_headers, enterprise_id_of, is_enterprise_account, variant_of,
};
use crate::modules::at_rest;
use crate::modules::config::{
    http_request, is_route_missing, load_checkin_config, now_ms, CHECKIN_API_PREFIX,
    WORKBUDDY_API_ENDPOINT,
};
use crate::modules::credit_usage;
use crate::modules::refresh::{ensure_fresh_token, refresh_account_token};
use crate::modules::variant::WbVariant;

/// 查询单账号的积分资源及到期时间（只读用途，受只读解密开关控制）。
pub async fn get_credit_expiry(account: &Value) -> Value {
    get_credit_expiry_for(account, at_rest::AccessMode::ReadOnly).await
}

/// 查询单账号的积分资源及到期时间，显式指定解密用途（写侧调用点用）。
///
/// **顺序不能反**：先 `usable_account`（信封 + 开关打开 → 解出临时副本），再判信封。
/// 反过来写会让 R6-06 静默失效 —— 功能「看起来上线了」，解密分支却永远走不到。
pub async fn get_credit_expiry_for(account: &Value, mode: at_rest::AccessMode) -> Value {
    // R6-06：**先解密、后判信封**。顺序由 `resolve_or_envelope_error` 一处保证，
    // 调用点不许自己先判信封（写反会让解密分支永远走不到）。
    // 临时副本必须活到函数结束：绑定声明在解析之后，让借用成立。
    let resolved = match at_rest::resolve_or_envelope_error(account, mode) {
        Ok(temp) => temp,
        Err(error) => {
            return json!({
                "ok": false,
                "accountId": account.get("id").cloned().unwrap_or(Value::Null),
                "accountName": account_display_name(account),
                "error": error,
            })
        }
    };
    let account = resolved.as_ref().unwrap_or(account);

    let now = now_ms();

    // 企业版分流（issue #167）：企业配额不在个人版资源接口里。必须放在 cn/ai
    // 分派之前——官方国内/国际版企业账号都走企业接口（两侧 asar 已验证同构）。
    if is_enterprise_account(account) {
        let enterprise_id = match enterprise_id_of(account) {
            Some(enterprise_id) => Some(enterprise_id),
            // 兜底判定命中但缺 ID（历史备份）：补拉一次。
            None => resolve_enterprise_id(account).await,
        };
        return match enterprise_id {
            Some(enterprise_id) => fetch_enterprise_credit(account, &enterprise_id, now).await,
            // 补不到 ID 时给出可读错误，不得回落个人版接口把企业账号显示成「暂无可用积分」。
            None => {
                credit_error_result(account, "企业账号缺少企业 ID，无法查询企业配额".to_string())
            }
        };
    }

    if !uses_three_endpoint_query(variant_of(account)) {
        return fetch_legacy_credit(account, now).await;
    }

    let responses = fetch_new_resource_responses(account).await;
    if let Some(resources) =
        normalized_new_resources(&responses.summary, &responses.paid, &responses.free, now)
    {
        return credit_result(account, resources, now);
    }

    // 旧接口回退必须复用三路请求已刷新过的账号，避免再次拿原始 refresh token
    // 发起第二次刷新并把刚落盘的新 token 覆盖成失效状态。
    let mut fallback_account = responses.account;
    let mut response = fetch_legacy_user_resource(&fallback_account).await;
    if is_unauthorized(&response)
        && !responses.refresh_attempted
        && !fallback_account
            .get("refresh_token")
            .and_then(Value::as_str)
            .unwrap_or("")
            .is_empty()
    {
        // 新接口没有触发过 401 刷新时，仍保留旧接口原有的一次重试能力；
        // 若新接口已刷新过，则禁止这里再次刷新，保证一次查询最多一次 401 refresh。
        fallback_account = refresh_account_token(fallback_account).await;
        response = fetch_legacy_user_resource(&fallback_account).await;
    }
    legacy_credit_result(account, &response, now)
}

use http::*;
use resources::*;

mod http;
mod resources;

pub use http::{api_base_for, authenticated_post};

#[cfg(test)]
mod tests {
    use super::*;

    // 用 `include!` 而不是 `mod`：`tests/` 下这些文件共用同一个 `use super::*` 作用域，
    // 每个文件不是独立模块（改成 `mod` 就要给每个文件补一套导入、helper 也要逐文件重复）。
    // 另一个作用是控制文件规模：新增一组用例时可以单独开一个文件，不必因为几行测试
    // 把某个测试文件推过 500 行硬线、再去 `docs/ARCHITECTURE.md` 登记豁免。
    include!("tests/fixtures.rs");
    include!("tests/resources.rs");
    include!("tests/http.rs");
    include!("tests/failure.rs");
}
