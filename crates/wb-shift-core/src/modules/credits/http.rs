//! HTTP 取数层：鉴权 POST、域名/路径选择、响应状态判定与三类资源接口编排。

use super::*;

use crate::modules::at_rest;

/// 旧资源接口路径后缀（按档位生成完整候选，见 `WbVariant::billing_paths`）。
pub(super) const USER_RESOURCE_SUFFIX: &str = "/get-user-resource";
pub(super) const WORKBUDDY_WEB_ENDPOINT: &str = "https://www.workbuddy.cn";
pub(super) const RESOURCE_SUMMARY_PATH: &str = "/billing/meter/get-user-resource-summary";
pub(super) const RESOURCE_PAID_PACKAGES_PATH: &str =
    "/billing/meter/get-user-resource-paid-packages";
pub(super) const RESOURCE_FREE_PACKAGES_PATH: &str =
    "/billing/meter/get-user-resource-free-packages";
/// 企业配额接口（issue #167）：企业账号不走个人版资源接口，官方按 `enterpriseId`
/// 分流到该接口；国内版与国际版路径同构（两侧 asar 均已验证）。
pub(super) const ENTERPRISE_USAGE_PATH: &str = "/v2/billing/meter/get-enterprise-user-usage";
/// 企业 ID 补拉接口：`type` 兜底判定为企业版但账号数据缺 ID 时使用（单次尝试）。
pub(super) const ENTERPRISE_ACCOUNTS_PATH: &str = "/console/accounts";
pub(super) const PRODUCT_CODE: &str = "p_tcaca";
pub(super) const EXPIRING_SOON_DAYS: i64 = 7;
/// DeductionEndTime 比 CycleEndTime 晚超过该天数时，视前者为长期占位
/// （官方数据形态：如 035 的 DeductionEndTime=2049 与 CycleEndTime=当月月底并存），改用 CycleEndTime。
pub(super) const EXPIRY_CYCLE_OVERRIDE_DAYS: i64 = 365;
/// 最终解析出的到期时间距 now 超过该天数时视为长期有效（expireAt=null），
/// 避免 2049 这类占位值流入前端。
pub(super) const FAR_FUTURE_EXPIRY_DAYS: i64 = 730;

// 付费/免费包查询码表：国内版公开套餐配置 ∪ 官网 usercenter 国际版码集
// ∪ 官方客户端 PAID/FREE_PACKAGE_CODES。请求体多带码对不存在的包无副作用；
// 解析器不依赖这份清单，summary 仍可带回未列出的包（但无时间字段）。
pub(super) const PAID_PACKAGE_CODES: &[&str] = &[
    "TCACA_code_002_AkiJS3ZHF5",
    "TCACA_code_023_4xbGhMrE6q",
    "TCACA_code_026_BaESVICNoi",
    "TCACA_code_027_0FCGVA6vSa",
    "TCACA_code_009_0XmEQc2xOf",
    "TCACA_code_038_OhvqZtiPKr",
    "TCACA_code_003_FAnt7lcmRT",
    "TCACA_code_036_lupO5WgNdG",
];
pub(super) const FREE_PACKAGE_CODES: &[&str] = &[
    "TCACA_code_008_cfWoLwvjU4",
    "TCACA_code_007_nzdH5h4Nl0",
    "TCACA_code_028_NtpWi0jzXs",
    "TCACA_code_029_6wCGEWquYy",
    "TCACA_code_030_BjSt89qTvr",
    "TCACA_code_001_PqouKr6QWV",
    "TCACA_code_006_DbXS0lrypC",
    "TCACA_code_035_ArVxJcGDsm",
    "TCACA_code_037_WxOD3MpI2o",
    "TCACA_code_039_KRcQj7wUat",
    "TCACA_code_040_mi9rCYg46x",
];

pub(super) fn response_error(response: &Value) -> String {
    let nested = response.get("data").filter(|value| value.is_object());
    let code = response_code(response).unwrap_or(-1);
    response
        .get("message")
        .or_else(|| response.get("msg"))
        .or_else(|| nested.and_then(|value| value.get("message")))
        .or_else(|| nested.and_then(|value| value.get("msg")))
        .and_then(|value| value.as_str())
        .filter(|message| !message.trim().is_empty())
        .map(|message| message.chars().take(160).collect::<String>())
        .unwrap_or_else(|| format!("积分查询失败（code={code}）"))
}

pub(super) fn response_code(response: &Value) -> Option<i64> {
    fn parse_code(value: &Value) -> Option<i64> {
        value.as_i64().or_else(|| {
            value
                .as_str()
                .and_then(|text| text.trim().parse::<i64>().ok())
        })
    }
    response
        .get("code")
        .and_then(parse_code)
        .or_else(|| response.get("data")?.get("code").and_then(parse_code))
}

pub(super) fn is_success(response: &Value) -> bool {
    if !response.is_object() {
        return false;
    }
    match response_code(response) {
        Some(0) | Some(200) => true,
        Some(_) => false,
        None => {
            response.get("data").is_some()
                && response.get("ok").and_then(Value::as_bool) != Some(false)
                && response.get("success").and_then(Value::as_bool) != Some(false)
        }
    }
}

pub(super) fn is_unauthorized(response: &Value) -> bool {
    let code = response_code(response).unwrap_or(-1);
    // 网关 WAF 10085 是客户端指纹拦截，不是 token 过期；刷新无效。
    if code == 10085 {
        return false;
    }
    if code == 401 || code == 403 {
        return true;
    }
    let message = response
        .get("message")
        .or_else(|| response.get("msg"))
        .or_else(|| response.get("data").and_then(|value| value.get("message")))
        .or_else(|| response.get("data").and_then(|value| value.get("msg")))
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .to_lowercase();
    ["unauthorized", "401", "登录", "失效", "过期", "token"]
        .iter()
        .any(|keyword| message.contains(keyword))
}

pub(super) fn is_transport_error(response: &Value) -> bool {
    response_code(response) == Some(-1)
        && response
            .get("message")
            .and_then(Value::as_str)
            .is_some_and(|message| !message.trim().is_empty())
}

/// 发起需要账号身份的 JSON POST 请求。
///
/// 资源查询和官方用量查询必须共用这条链路：先按现有惰性策略保证 token
/// 新鲜，遇到未授权时使用 refresh token 重试一次。调用方只拿到上游 JSON，
/// 不会把认证字段拼进返回值。
pub async fn authenticated_post(account: &Value, url: &str, body: Value) -> Value {
    // R6-06：先尝试用本机 live 凭据解出临时副本（信封账号 + 只读开关打开时才生效）；
    // 解不出来才回退信封提示（issue #94 的文案与形状逐字不变）。
    let resolved = match at_rest::resolve_or_envelope_error(account, at_rest::AccessMode::ReadOnly)
    {
        Ok(temp) => temp,
        Err(err) => return json!({"code": -2, "message": err}),
    };
    let account = resolved.as_ref().unwrap_or(account);
    if at_rest::is_temp(account) {
        // 临时副本没有 refresh token：401 时只重读 live 一次，**绝不**调刷新接口。
        let mut response = post_with_account(account, url, body.clone()).await;
        if is_unauthorized(&response) {
            if let Some(fresh) = at_rest::reread_after_unauthorized(account) {
                response = post_with_account(&fresh, url, body).await;
            }
        }
        return response;
    }
    let config = load_checkin_config();
    let mut working_account = ensure_fresh_token(account.clone(), &config).await;
    let mut response = post_with_account(&working_account, url, body.clone()).await;

    if is_unauthorized(&response)
        && !working_account
            .get("refresh_token")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .is_empty()
    {
        working_account = refresh_account_token(working_account).await;
        response = post_with_account(&working_account, url, body).await;
    }

    response
}

async fn post_with_account(account: &Value, url: &str, body: Value) -> Value {
    let headers = resource_auth_headers(account, request_origin(url));
    let response = http_request(url, "POST", Some(body.clone()), Some(&headers)).await;
    if is_transport_error(&response) {
        http_request(url, "POST", Some(body), Some(&headers)).await
    } else {
        response
    }
}

pub(super) fn request_origin(url: &str) -> &'static str {
    // Origin 跟随本次请求 host（契约不变）；新增国际版域后必须同步登记，
    // 否则国际版请求会带上国内 Origin 并被网关的一致性校验拒绝。
    // 只认「host 完全相同或后接 `/`」的前缀，避免相似域名被误当成已知 origin。
    [
        WbVariant::Ai.api_endpoint(),
        WORKBUDDY_WEB_ENDPOINT,
        WORKBUDDY_API_ENDPOINT,
    ]
    .into_iter()
    .find(|origin| match url.strip_prefix(origin) {
        Some(rest) => rest.is_empty() || rest.starts_with('/'),
        None => false,
    })
    .unwrap_or(WORKBUDDY_API_ENDPOINT)
}

pub(super) fn resource_auth_headers(
    account: &Value,
    origin: &str,
) -> std::collections::HashMap<String, String> {
    let mut headers = build_auth_headers(account);
    // WorkBuddy 用户中心的 Axios 拦截器始终携带该头。桌面端使用同一组
    // billing 接口时也保持一致，避免网关把请求当成未知客户端。
    headers.insert("X-Client-Platform".to_string(), "web".to_string());
    headers.insert(
        "Accept".to_string(),
        "application/json, text/plain, */*".to_string(),
    );
    headers.insert("Origin".to_string(), origin.to_string());
    headers.insert(
        "Referer".to_string(),
        format!("{origin}/profile/plans-usage"),
    );
    headers
}

pub(super) fn paid_packages_body() -> Value {
    json!({
        "PageNumber": 1,
        "PageSize": 200,
        "Status": [0, 3],
        "PackageCodes": PAID_PACKAGE_CODES,
        "NeedRenewInfo": true,
    })
}

pub(super) fn free_packages_body() -> Value {
    let now = Local::now();
    let start = now
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .unwrap_or(now.naive_local());
    let end = now
        .date_naive()
        .and_hms_opt(23, 59, 59)
        .unwrap_or(now.naive_local());
    json!({
        "PageNumber": 1,
        "PageSize": 200,
        "Status": [0, 3],
        "SlicePeriodStartTime": start.format("%Y-%m-%d %H:%M:%S").to_string(),
        "SlicePeriodEndTime": end.format("%Y-%m-%d %H:%M:%S").to_string(),
        "PackageCodes": FREE_PACKAGE_CODES,
    })
}

/// 按账号档位/域名选本次请求的基址。
///
/// - 国际版：固定国际版 API 域（不做域名兜底，避免把 AI token 打到国内域）；
/// - 国内版：保持既有 domain 逻辑（workbuddy.cn / codebuddy.cn 二选一）。
pub fn api_base_for(account: &Value) -> &'static str {
    if variant_of(account) == WbVariant::Ai {
        return WbVariant::Ai.api_endpoint();
    }
    // 官网脚本使用相对路径，实际请求的是当前登录 origin。账号库中的 CN
    // OAuth token 默认签发给 www.codebuddy.cn；若把它固定发往
    // www.workbuddy.cn，令牌域和 X-Domain 会不一致并被网关拒绝。
    // 这里只在已知官方 origin 间选择，不允许账号数据拼出任意主机。
    match account
        .get("domain")
        .and_then(Value::as_str)
        .map(str::trim)
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("workbuddy.cn") | Some("www.workbuddy.cn") => WORKBUDDY_WEB_ENDPOINT,
        _ => WORKBUDDY_API_ENDPOINT,
    }
}

/// 依次尝试路径候选，**只有 404 才回落**到下一个候选。
///
/// 401/403/10085/传输错误都必须原样返回：把它们当成「路径不对」会掩盖真因，
/// 也会让上层误判为可重试（见 design D4 与 credits 既有契约）。
async fn post_with_fallback(account: &Value, paths: &[String], body: Value) -> Value {
    let base = api_base_for(account);
    let mut last = json!({"code": -1, "message": "无可用接口路径"});
    for (index, path) in paths.iter().enumerate() {
        let url = format!("{base}{path}");
        let response = post_with_account(account, &url, body.clone()).await;
        if index + 1 == paths.len() || !is_route_missing(&response) {
            return response;
        }
        last = response;
    }
    last
}

pub(super) struct NewResourceResponses {
    pub(super) account: Value,
    pub(super) summary: Value,
    pub(super) paid: Value,
    pub(super) free: Value,
    pub(super) refresh_attempted: bool,
}

async fn retry_new_response_if_unauthorized(
    account: &Value,
    response: Value,
    paths: &[String],
    body: Value,
) -> Value {
    if is_unauthorized(&response) {
        post_with_fallback(account, paths, body).await
    } else {
        response
    }
}

/// 统一惰性刷新后并行请求三类新资源接口；若任一路返回未授权，只刷新一次，
/// 然后仅重试该分支，避免三个 future 同时刷新并覆盖账号库中的 token。
pub(super) async fn fetch_new_resource_responses(account: &Value) -> NewResourceResponses {
    let config = load_checkin_config();
    let working_account = ensure_fresh_token(account.clone(), &config).await;
    // 路径候选按档位生成（国际版先 /billing/meter/... 再回落 /v2/billing/meter/...）。
    let variant = variant_of(&working_account);
    let summary_paths = variant.billing_paths(RESOURCE_SUMMARY_PATH);
    let paid_paths = variant.billing_paths(RESOURCE_PAID_PACKAGES_PATH);
    let free_paths = variant.billing_paths(RESOURCE_FREE_PACKAGES_PATH);
    let summary_body = json!({});
    let paid_body = paid_packages_body();
    let free_body = free_packages_body();
    let (summary, paid, free) = tokio::join!(
        post_with_fallback(&working_account, &summary_paths, summary_body.clone()),
        post_with_fallback(&working_account, &paid_paths, paid_body.clone()),
        post_with_fallback(&working_account, &free_paths, free_body.clone()),
    );

    if !(is_unauthorized(&summary) || is_unauthorized(&paid) || is_unauthorized(&free)) {
        return NewResourceResponses {
            account: working_account,
            summary,
            paid,
            free,
            refresh_attempted: false,
        };
    }

    let can_refresh = working_account
        .get("refresh_token")
        .and_then(Value::as_str)
        .is_some_and(|token| !token.trim().is_empty());
    if !can_refresh {
        return NewResourceResponses {
            account: working_account,
            summary,
            paid,
            free,
            refresh_attempted: false,
        };
    }
    let refreshed = refresh_account_token(working_account).await;
    let (summary, paid, free) = tokio::join!(
        retry_new_response_if_unauthorized(&refreshed, summary, &summary_paths, summary_body),
        retry_new_response_if_unauthorized(&refreshed, paid, &paid_paths, paid_body),
        retry_new_response_if_unauthorized(&refreshed, free, &free_paths, free_body),
    );
    NewResourceResponses {
        account: refreshed,
        summary,
        paid,
        free,
        refresh_attempted: true,
    }
}

pub(super) async fn fetch_legacy_user_resource(account: &Value) -> Value {
    let now = Local::now();
    let begin = now.format("%Y-%m-%d %H:%M:%S").to_string();
    let end = (now + chrono::Duration::days(365 * 101))
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();
    let body = json!({
        "PageNumber": 1,
        "PageSize": 100,
        "ProductCode": PRODUCT_CODE,
        "Status": [0, 3],
        "PackageEndTimeRangeBegin": begin,
        "PackageEndTimeRangeEnd": end,
    });
    // 旧接口同样按档位走候选回落（国内版只有一个候选，与改造前一致）。
    let paths =
        variant_of(account).billing_paths(&format!("{CHECKIN_API_PREFIX}{USER_RESOURCE_SUFFIX}"));
    // 新接口编排已经统一执行过惰性刷新，并在任一路未授权时只刷新一次。
    // 旧接口回退必须直接复用该账号，不能重新进入 authenticated_post，
    // 否则可能重复刷新并用旧 refresh token 覆盖刚落盘的新 token。
    post_with_fallback(account, &paths, body).await
}

/// 是否走 summary/paid/free 三路新接口。
///
/// 三路只对国内版存在；国际版只用 `get-user-resource`（用户 2026-09-16 HAR
/// 抓包证实），对它发三路等于 3×2 个 404，故按档位分派取数入口。
pub(super) fn uses_three_endpoint_query(variant: WbVariant) -> bool {
    matches!(variant, WbVariant::Cn)
}

/// 单接口档位（国际版）的取数：旧接口自带完整鉴权链路。
///
/// 不能复用「三路已刷新账号」的前提——国际版不经过 `fetch_new_resource_responses`，
/// 必须自己完成惰性刷新与 401 后最多一次的 token 刷新。
pub(super) async fn fetch_legacy_credit(account: &Value, now: i64) -> Value {
    let config = load_checkin_config();
    let mut working_account = ensure_fresh_token(account.clone(), &config).await;
    let mut response = fetch_legacy_user_resource(&working_account).await;
    if is_unauthorized(&response)
        && !working_account
            .get("refresh_token")
            .and_then(Value::as_str)
            .unwrap_or("")
            .is_empty()
    {
        working_account = refresh_account_token(working_account).await;
        response = fetch_legacy_user_resource(&working_account).await;
    }
    legacy_credit_result(account, &response, now)
}

/// 旧接口响应 → 积分结果：成功走正常投影，失败带可读错误。
pub(super) fn legacy_credit_result(account: &Value, response: &Value, now: i64) -> Value {
    if is_success(response) && has_resource_accounts(response) {
        let resources = resource_accounts(response)
            .into_iter()
            .map(|resource| resource_summary(resource, now))
            .collect();
        return credit_result(account, resources, now);
    }
    // 个人版失败分支（手里有上游响应）：档位透传给前端做下一步动作。
    credit_failure_result(account, response)
}

/// 企业配额取数：注入企业 ID 后复用 `authenticated_post`（惰性刷新 + 401 重试一次
/// + 信封短路全部继承），域名沿用 `api_base_for` 的档位分派。
pub(super) async fn fetch_enterprise_credit(
    account: &Value,
    enterprise_id: &str,
    now: i64,
) -> Value {
    let mut working_account = account.clone();
    if let Some(object) = working_account.as_object_mut() {
        // 补拉场景账号数据里没有 enterpriseId：先注入再发请求，
        // 让既有请求头链路带上 X-Enterprise-Id / X-Tenant-Id。
        object.insert(
            "enterpriseId".to_string(),
            Value::String(enterprise_id.to_string()),
        );
    }
    let url = format!("{}{ENTERPRISE_USAGE_PATH}", api_base_for(&working_account));
    let response = authenticated_post(&working_account, &url, json!({})).await;
    enterprise_credit_result(account, &response, now)
}

/// `type` 兜底判定为企业版但账号数据缺 `enterpriseId`（如历史备份）时，
/// 从 `/console/accounts` 补拉一次（对齐 WorkDaddy `resolveEnterpriseId`）。
/// 单次尝试、不触发 token 刷新；失败返回 `None`，由调用方给出可读错误。
pub(super) async fn resolve_enterprise_id(account: &Value) -> Option<String> {
    let url = format!("{}{ENTERPRISE_ACCOUNTS_PATH}", api_base_for(account));
    let headers = build_auth_headers(account);
    let response = http_request(&url, "GET", None, Some(&headers)).await;
    enterprise_id_from_accounts(&response)
}
