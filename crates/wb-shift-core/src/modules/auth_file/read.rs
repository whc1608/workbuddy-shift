//! 读取与构建：认证文件读取/备份、账号与 auth 对象构建、从文件导入。

use super::sync::*;
use super::*;

/// 读取认证文件 JSON；不存在或解析失败返回 None。
pub fn read_auth_file(variant: WbVariant) -> Option<Value> {
    read_auth_file_at(&variant.auth_file_path())
}

/// 读取指定路径的认证文件（单测注入临时登录态文件用）。
pub fn read_auth_file_at(path: &Path) -> Option<Value> {
    if !path.exists() {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// 切换前备份当前认证文件，返回备份路径。对照 server.py `backup_auth_file`。
pub fn backup_auth_file(variant: WbVariant) -> Option<PathBuf> {
    let path = variant.auth_file_path();
    if !path.exists() {
        return None;
    }
    let dir = backup_dir();
    std::fs::create_dir_all(&dir).ok()?;
    let ts = utc_iso();
    let dest = dir.join(variant.backup_file_name(&ts));
    std::fs::copy(&path, &dest).ok()?;
    Some(dest)
}

/// 从账号库记录构造官方 account 字段。对照 server.py `build_account_obj`。
pub fn build_account_obj(acc: &Value) -> Value {
    let mut obj: Map<String, Value> = match acc.get("profile_raw") {
        Some(Value::Object(m)) => m.clone(),
        _ => Map::new(),
    };
    obj.insert(
        "uid".to_string(),
        acc.get("uid").cloned().unwrap_or_else(|| json!("")),
    );
    obj.insert(
        "nickname".to_string(),
        acc.get("nickname").cloned().unwrap_or_else(|| json!("")),
    );
    setdefault(&mut obj, "type", json!("personal"));
    setdefault(&mut obj, "accountType", json!(""));
    setdefault(&mut obj, "idp", json!(""));
    setdefault(&mut obj, "oneidAccountId", json!(""));
    setdefault(&mut obj, "areaInfoComplete", json!(false));
    setdefault(&mut obj, "isCurrentOneIdEnterprise", json!(false));
    setdefault(&mut obj, "isCurrentOneIdPersonal", json!(false));
    setdefault(&mut obj, "isFirstLogin", json!(false));
    setdefault(&mut obj, "isCreator", json!(false));
    setdefault(&mut obj, "isAdmin", json!(false));
    setdefault(&mut obj, "uin", json!(""));
    setdefault(&mut obj, "phoneNumber", json!(""));
    setdefault(&mut obj, "lastLogin", json!(true));
    setdefault(&mut obj, "pluginEnabled", json!(true));
    setdefault(
        &mut obj,
        "deployStatus",
        json!({"statusCode": 0, "statusMsg": "", "detailMsg": ""}),
    );
    setdefault(
        &mut obj,
        "sso",
        json!({"domain": "", "domainModifiedTimes": 0}),
    );
    Value::Object(obj)
}

/// 秒级时间戳的判定上界：13 位毫秒必然大于它，10 位秒必然小于它。
///
/// 官方认证文件历史上出现过秒级与毫秒级两种写法，归一前必须先分数量级，
/// 否则 10 位秒会被当成 1970 年附近的时刻（客户端会立即认为凭据需要刷新）。
const SECONDS_UPPER_BOUND: i64 = 100_000_000_000;

/// 把「整数 / 整数字符串 / 秒级 / 毫秒级」的时间戳归一成毫秒，不可用值返回 `None`。
///
/// 不可用的定义：非数字、非正数、以及晚于 `now` 的未来时刻。未来时刻会被官方客户端
/// 当成「刚刚刷新过而且永远不到期」，从而无限期跳过刷新，所以必须按未知丢弃。
pub(super) fn normalize_refresh_ts(value: &Value, now: i64) -> Option<i64> {
    let raw = match value {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64))?,
        Value::String(s) => s.trim().parse::<i64>().ok()?,
        _ => return None,
    };
    if raw <= 0 {
        return None;
    }
    let ms = if raw < SECONDS_UPPER_BOUND {
        raw.checked_mul(1000)?
    } else {
        raw
    };
    (ms > 0 && ms <= now).then_some(ms)
}

/// 导出认证文件时应写入的 `lastRefreshTime`（毫秒；`0` = 未知）。
///
/// 导出既没有登录也没有刷新 token，把「导出时刻」写进这个字段会让几小时前刷新过的
/// 凭据看起来像刚刷新（官方客户端用该字段做约 30 秒的刷新冷却判断），从而推迟真正
/// 需要的刷新。因此只回填**确实发生过**的刷新，取以下两个来源的最大有效值：
///
/// - 账号库的 `refreshedAt`：`refresh.rs` 续期成功时用 `now_ms()` 写入的毫秒；
/// - 认证文件已记录的 `lastRefreshTime`：平铺 `auth_raw.lastRefreshTime` 与嵌套
///   `auth_raw.auth.lastRefreshTime` 两种结构都要认（导入时 `auth_raw` 存的是
///   完整认证文件根，历史版本两种都出现过）。
///
/// 两者都不可用时写 `0`：宁可让客户端认为「不知道上次刷新时刻」，也不要用导出时刻或
/// `createdAt` 兜底制造一个假的刷新记录。
pub(super) fn export_last_refresh_time(acc: &Value, now: i64) -> i64 {
    let raw = acc.get("auth_raw");
    let recorded_flat = raw
        .and_then(|r| r.get("lastRefreshTime"))
        .and_then(|v| normalize_refresh_ts(v, now));
    let recorded_nested = raw
        .and_then(|r| r.get("auth"))
        .and_then(|a| a.get("lastRefreshTime"))
        .and_then(|v| normalize_refresh_ts(v, now));
    let refreshed = acc
        .get("refreshedAt")
        .and_then(|v| normalize_refresh_ts(v, now));
    [recorded_flat, recorded_nested, refreshed]
        .into_iter()
        .flatten()
        .max()
        .unwrap_or(0)
}

/// 从账号库记录构造官方 auth 字段。对照 server.py `build_auth_obj`。
pub fn build_auth_obj(acc: &Value) -> Value {
    let mut obj: Map<String, Value> = Map::new();
    let raw = acc.get("auth_raw");
    if let Some(Value::Object(m)) = raw {
        let inner = match m.get("auth") {
            Some(Value::Object(im)) => im.clone(),
            _ => m.clone(),
        };
        obj.extend(inner);
    }
    let token_type = acc
        .get("token_type")
        .and_then(|v| v.as_str())
        .unwrap_or("Bearer")
        .to_string();
    let expires_at = acc.get("expiresAt").and_then(|v| v.as_i64());
    let now = now_ms();

    // token 可能是明文字符串或 WorkBuddy 5.6 加密信封：信封必须原样写回，
    // 由 WorkBuddy 读取时自行解密（同一 keyblob）。降级为空串会静默毁掉登录态。
    obj.insert(
        "accessToken".to_string(),
        secret_value(acc, "access_token").unwrap_or_else(|| json!("")),
    );
    obj.insert(
        "refreshToken".to_string(),
        secret_value(acc, "refresh_token").unwrap_or_else(|| json!("")),
    );
    obj.insert("tokenType".to_string(), token_type.into());
    obj.insert(
        "domain".to_string(),
        get_str(acc, "domain").unwrap_or_default().into(),
    );
    // 写真实刷新时刻（未知写 0），不能写导出时刻：见 export_last_refresh_time。
    obj.insert(
        "lastRefreshTime".to_string(),
        json!(export_last_refresh_time(acc, now)),
    );
    setdefault(
        &mut obj,
        "scope",
        json!("openid profile offline_access email"),
    );

    if let Some(expires_at) = expires_at {
        obj.insert("expiresAt".to_string(), json!(expires_at));
        obj.insert(
            "expiresIn".to_string(),
            json!(((expires_at - now) / 1000).max(0)),
        );
        let refresh_exp = raw
            .and_then(|r| r.get("refreshExpiresAt"))
            .and_then(|v| v.as_i64())
            .unwrap_or(expires_at);
        if !obj.contains_key("refreshExpiresAt") {
            obj.insert("refreshExpiresAt".to_string(), json!(refresh_exp));
        }
        obj.insert(
            "refreshExpiresIn".to_string(),
            json!(((refresh_exp - now) / 1000).max(0)),
        );
    } else {
        setdefault(&mut obj, "expiresIn", json!(0));
        setdefault(&mut obj, "refreshExpiresIn", json!(0));
    }
    setdefault(&mut obj, "notBeforePolicy", json!(0));
    setdefault(&mut obj, "sessionState", json!(""));
    Value::Object(obj)
}

/// 从当前 WorkBuddy 登录态导入账号。对照 server.py `import_from_auth_file`。
pub fn import_from_auth_file(variant: WbVariant) -> Option<Value> {
    imported_account_from_root(read_auth_file(variant)?, variant)
}

pub(super) fn imported_account_from_root(root: Value, variant: WbVariant) -> Option<Value> {
    let account_obj = root
        .get("account")
        .filter(|v| v.is_object())
        .cloned()
        .unwrap_or_else(|| json!({}));
    let auth_obj = root
        .get("auth")
        .filter(|v| v.is_object())
        .cloned()
        .unwrap_or_else(|| json!({}));

    let uid = get_str(&root, "uid").or_else(|| get_str(&account_obj, "uid"));
    let uid = uid.or_else(|| get_str(&account_obj, "id"));
    // WorkBuddy 5.6 起 nickname/accessToken/refreshToken 可能是 `{$wbEncrypted, envelope}`
    // 加密信封：这里必须原样保留（secret_value），不能 get_str 强转字符串——
    // 否则 accessToken 取不到导致导入恒 400，切换写回时也会把信封覆盖成空串。
    let nickname = secret_value(&root, "nickname")
        .or_else(|| secret_value(&root, "name"))
        .or_else(|| secret_value(&account_obj, "nickname"))
        .or_else(|| secret_value(&account_obj, "label"));
    let email = get_str(&root, "email")
        .or_else(|| get_str(&account_obj, "email"))
        .or_else(|| get_str(&auth_obj, "email"));
    let access_token = secret_value(&auth_obj, "accessToken")
        .or_else(|| secret_value(&auth_obj, "access_token"))
        .or_else(|| secret_value(&root, "accessToken"))
        .or_else(|| secret_value(&root, "access_token"));
    let refresh_token = secret_value(&auth_obj, "refreshToken")
        .or_else(|| secret_value(&auth_obj, "refresh_token"))
        .or_else(|| secret_value(&root, "refreshToken"))
        .or_else(|| secret_value(&root, "refresh_token"));
    let token_type = get_str(&auth_obj, "tokenType")
        .or_else(|| get_str(&auth_obj, "token_type"))
        .unwrap_or_else(|| "Bearer".to_string());
    let domain = get_str(&root, "domain").or_else(|| get_str(&auth_obj, "domain"));
    let expires_at = parse_ts(root.get("expiresAt").or_else(|| auth_obj.get("expiresAt")));
    let refresh_expires_at = parse_ts(
        root.get("refreshExpiresAt")
            .or_else(|| auth_obj.get("refreshExpiresAt")),
    );

    access_token.as_ref()?;

    Some(json!({
        "id": uuid::Uuid::new_v4().to_string(),
        "uid": uid,
        "nickname": nickname,
        "email": email,
        "variant": variant.as_str(),
        // 凭据来源（R6-01）：从本机登录态采集所得，与客户端共用同一个 refresh 会话，
        // 因此 live 出现加密信封时**不许**用库里的旧 RT 去刷新（会把客户端顶掉）。
        // 键名必须与 `account::CREDENTIAL_SOURCE_FIELD` 逐字一致（`json!` 只接受字面量键）。
        "credentialSource": crate::modules::account::CredentialSource::Local.as_str(),
        "enterpriseName": get_str(&root, "enterpriseName")
            .or_else(|| get_str(&root, "enterprise_name"))
            .or_else(|| get_str(&account_obj, "enterpriseName"))
            .or_else(|| get_str(&account_obj, "enterprise_name")),
        "enterpriseId": get_str(&root, "enterpriseId")
            .or_else(|| get_str(&root, "enterprise_id"))
            .or_else(|| get_str(&account_obj, "enterpriseId"))
            .or_else(|| get_str(&account_obj, "enterprise_id")),
        "access_token": access_token,
        "refresh_token": refresh_token,
        "token_type": token_type,
        "domain": domain,
        "expiresAt": expires_at,
        "refreshExpiresAt": refresh_expires_at,
        "auth_raw": root,
        "profile_raw": account_obj,
        "createdAt": now_ms(),
    }))
}

/// 字符串时间戳转 i64（数字原样保留，不做秒/毫秒换算）。
/// 对照 server.py `import_from_auth_file` 的 str→int 逻辑。
pub(super) fn parse_ts(v: Option<&Value>) -> Option<i64> {
    match v {
        Some(Value::Number(n)) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        Some(Value::String(s)) => s.trim().parse::<f64>().ok().map(|f| f as i64),
        _ => None,
    }
}
