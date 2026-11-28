//! 会话/凭据桥接与当前账号检测：会话 JSON、secret token 解析、账号匹配与 detect。

#[allow(unused_imports)]
use super::status::*;
#[allow(unused_imports)]
use super::switch::*;
use super::*;

/// 构造注入到 CN IDE 的会话 JSON（与 CN 客户端登录态写入结构一致）。
pub fn build_session_json(acc: &Value) -> String {
    let uid = get_str(acc, "uid").unwrap_or_default();
    let nickname = get_str(acc, "nickname").unwrap_or_default();
    let enterprise_id = get_str(acc, "enterpriseId")
        .or_else(|| get_str(acc, "enterprise_id"))
        .unwrap_or_default();
    let enterprise_name = get_str(acc, "enterpriseName")
        .or_else(|| get_str(acc, "enterprise_name"))
        .unwrap_or_default();
    let domain = codebuddy_domain_for(
        get_str(acc, "domain").unwrap_or_default().as_str(),
        account::variant_of(acc),
    );
    let refresh_token = get_str(acc, "refresh_token").unwrap_or_default();
    let access_token = get_str(acc, "access_token").unwrap_or_default();
    let token_type = get_str(acc, "token_type").unwrap_or_else(|| "Bearer".to_string());
    let expires_at = acc.get("expiresAt").and_then(|v| v.as_i64()).unwrap_or(0);

    json!({
        "id": "Tencent-Cloud.genie-ide",
        "token": access_token,
        "refreshToken": refresh_token,
        "expiresAt": expires_at,
        "domain": domain,
        "accessToken": format!("{uid}+{access_token}"),
        "converted": true,
        "account": {
            "id": uid,
            "uid": uid,
            "label": nickname,
            "nickname": nickname,
            "enterpriseId": enterprise_id,
            "enterpriseName": enterprise_name,
            "pluginEnabled": true,
            "lastLogin": true,
        },
        "auth": {
            "accessToken": access_token,
            "refreshToken": refresh_token,
            "tokenType": token_type,
            "domain": domain,
            "expiresAt": expires_at,
            "expiresIn": expires_at,
            "refreshExpiresIn": 0,
            "refreshExpiresAt": 0,
            "lastRefreshTime": now_ms(),
        }
    })
    .to_string()
}

pub(super) fn parse_token_from_secret(secret: &str) -> Option<(Option<String>, String)> {
    let trimmed = secret.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        let token = value
            .get("token")
            .or_else(|| value.get("access_token"))
            .or_else(|| value.get("accessToken"))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .or_else(|| {
                value
                    .get("auth")
                    .and_then(|a| a.get("accessToken").or_else(|| a.get("access_token")))
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
            })?;
        let uid = value
            .get("uid")
            .or_else(|| value.pointer("/account/uid"))
            .or_else(|| value.pointer("/account/id"))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        // accessToken 可能是 `uid+token`
        if let Some((prefix, suffix)) = token.split_once('+') {
            let suffix = suffix.trim();
            if !suffix.is_empty() {
                let uid = uid.or_else(|| {
                    let p = prefix.trim();
                    if p.is_empty() {
                        None
                    } else {
                        Some(p.to_string())
                    }
                });
                return Some((uid, suffix.to_string()));
            }
        }
        return Some((uid, token));
    }
    if let Some((prefix, suffix)) = trimmed.split_once('+') {
        let suffix = suffix.trim();
        if !suffix.is_empty() {
            let uid = {
                let p = prefix.trim();
                if p.is_empty() {
                    None
                } else {
                    Some(p.to_string())
                }
            };
            return Some((uid, suffix.to_string()));
        }
    }
    Some((None, trimmed.to_string()))
}

/// 当前国际版 IDE 登录账号的 uid（用于定位可复制的会话目录）。
///
/// 与国内版 [`crate::modules::codebuddy_cn_ide::active_cn_ide_uid`] 同序：本机 IDE 登录
/// secret 解析 uid → 回退本地状态文件记录的账号 id → 账号库 uid。非法 uid（`default` /
/// `Public` / 路径穿越）视为未登录（返回 `None`），**不回退顶替**——否则会复制错账号树。
pub fn active_intl_ide_uid() -> Option<String> {
    resolve_active_uid(
        read_intl_secret(None).ok().flatten(),
        active_account_id_from_state(),
        |id| account::find_account(id).and_then(|acc| get_str(&acc, "uid")),
    )
}

/// [`active_intl_ide_uid`] 的可测内核：secret → 状态文件账号 id → 账号库。
///
/// 回退链收敛在一处，调用方不再各写一份；`uid_of_account` 由调用方注入（生产走账号库，
/// 单测传桩），使单测无需读取钥匙串、也不触碰真实 `~/.workbuddy-shift`。
pub(super) fn resolve_active_uid(
    secret: Option<String>,
    state_account_id: Option<String>,
    uid_of_account: impl Fn(&str) -> Option<String>,
) -> Option<String> {
    secret
        .as_deref()
        .and_then(parse_token_from_secret)
        .and_then(|(uid, _token)| uid)
        .map(|uid| uid.trim().to_string())
        .filter(|uid| !uid.is_empty())
        .or_else(|| state_account_id.and_then(|id| uid_of_account(&id)))
        .filter(|uid| is_safe_uid(uid))
}

pub(super) fn match_account_for_token(uid: Option<&str>, token: &str) -> Option<Value> {
    let accounts = account::load_accounts();
    if let Some(uid) = uid.filter(|s| !s.is_empty()) {
        if let Some(acc) = accounts
            .iter()
            .find(|a| get_str(a, "uid").as_deref() == Some(uid))
        {
            return Some(acc.clone());
        }
    }
    accounts
        .into_iter()
        .find(|a| get_str(a, "access_token").as_deref() == Some(token))
}

/// 从本机 CN IDE 读取当前 token；若能匹配账号库则返回匹配信息（不新建账号）。
pub fn detect_current_account() -> Result<Value, String> {
    let secret = read_intl_secret(None)?;
    let Some(secret) = secret else {
        return Ok(json!({
            "ok": true,
            "found": false,
            "message": "本机 CodeBuddy 未找到登录 secret",
        }));
    };
    let Some((uid, token)) = parse_token_from_secret(&secret) else {
        return Err("本地 CodeBuddy 登录信息解析失败".to_string());
    };
    if let Some(acc) = match_account_for_token(uid.as_deref(), &token) {
        let id = get_str(&acc, "id").unwrap_or_default();
        let _ = set_active_account_id(&id);
        return Ok(json!({
            "ok": true,
            "found": true,
            "matched": true,
            "accountId": id,
            "account": account::account_meta(&acc),
            "uid": uid,
        }));
    }
    Ok(json!({
        "ok": true,
        "found": true,
        "matched": false,
        "uid": uid,
        "message": "本机已登录 CodeBuddy，但账号库中无匹配账号；请用该账号重新 OAuth 扫码登录后再切换。",
    }))
}
