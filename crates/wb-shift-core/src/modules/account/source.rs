//! 凭据来源（`credentialSource`）：这条凭据是「OAuth 扫码」「本机登录态采集」还是
//! 「导入备份」，决定官方认证文件（live）里出现加密信封时该照常刷新还是该跳过（R6-01）。
//!
//! 取值只有三个：`oauth` / `local` / `import`。
//!
//! # 为什么需要它
//!
//! WorkBuddy 5.6 起客户端会把自己的 token 换成 `$wbEncrypted` 加密信封。`oauth` 账号是
//! 独立会话（扫码所得），live 里的信封与本工具的凭据是**两套**，照常刷新不会顶掉客户端；
//! 而 `local` / `import` 账号的明文与客户端共用同一个 refresh 会话，用库里的旧 refresh token
//! 去刷会把客户端手上的新令牌轮换掉（客户端掉登录）。因此信封出现时必须先看来源再决定动作。
//!
//! # 边界
//!
//! - 该字段属于**本机维护字段**（[`super::LOCAL_ACCOUNT_FIELDS`]）：刷新 / 采集 / 导入都不能
//!   把它清掉，否则一次「刷新 Token」就会让账号退回「判不出」；
//! - **不暴露到 HTTP DTO**（`account_meta` 不带该字段），避免新增契约面（见 R6-01「明确不做」）；
//! - 导出/导入保留该字段；备份里没有该字段时由导入侧按 `import` 补上。

use serde_json::Value;

/// 账号库里承载来源的字段名（与 JSON 键逐字一致：改名等于改持久化契约）。
pub const CREDENTIAL_SOURCE_FIELD: &str = "credentialSource";

/// 凭据来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialSource {
    /// OAuth 扫码所得：独立会话，明文过期后照常刷新。
    Oauth,
    /// 本机登录态采集所得：与客户端共用会话，遇到 live 信封一律不刷新。
    Local,
    /// 导入备份所得：备份无法区分来源，按保守档处理（同 `local`）。
    Import,
}

impl CredentialSource {
    /// 落库用的字符串取值（与持久化契约逐字一致）。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Oauth => "oauth",
            Self::Local => "local",
            Self::Import => "import",
        }
    }

    /// 解析落库取值；未知/空白返回 `None`（调用方据此回退到推断）。
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim() {
            "oauth" => Some(Self::Oauth),
            "local" => Some(Self::Local),
            "import" => Some(Self::Import),
            _ => None,
        }
    }

    /// 该来源在 live 出现信封、且库内明文已过期时是否**允许**照常刷新。
    ///
    /// 只有 `oauth` 允许：它是独立会话。`local` / `import` 与客户端共用会话，刷新会把客户端
    /// 手里的 refresh token 轮换掉，宁可让用户重新扫码也不让客户端掉登录。
    pub fn may_refresh_on_envelope(self) -> bool {
        matches!(self, Self::Oauth)
    }
}

/// 账号库里**显式写下的**来源；字段缺失或取值未知时返回 `None`（不做任何推断）。
///
/// 导入侧用它判断「备份是否自带了来源」：带了就原样保留（导出/导入往返不改口径），
/// 没带才补 `import`。
pub fn explicit_credential_source(account: &Value) -> Option<CredentialSource> {
    account
        .get(CREDENTIAL_SOURCE_FIELD)
        .and_then(Value::as_str)
        .and_then(CredentialSource::parse)
}

/// 读取凭据来源；字段缺失时按 `auth_raw` 的形状推断（历史账号零迁移）。
///
/// 推断口径（R6-01）：
/// - `auth_raw` 同时含 `auth` 与 `account` 两个根键 → 本机认证文件根 → [`CredentialSource::Local`]；
/// - 其余非空对象 → 扫码时的 token 响应形状 → [`CredentialSource::Oauth`]；
/// - 缺失 / 非对象 / 空对象 → 判不出 → [`CredentialSource::Local`]（保守：误判为 `local`
///   只会少刷新，不会顶掉客户端；误判为 `oauth` 才会破坏客户端的登录态）。
pub fn credential_source(account: &Value) -> CredentialSource {
    if let Some(explicit) = explicit_credential_source(account) {
        return explicit;
    }
    match account.get("auth_raw") {
        Some(Value::Object(map)) if map.contains_key("auth") && map.contains_key("account") => {
            CredentialSource::Local
        }
        Some(Value::Object(map)) if !map.is_empty() => CredentialSource::Oauth,
        _ => CredentialSource::Local,
    }
}

/// 就地写入来源（三个写入点共用：oauth 扫码 / 本机采集 / 导入备份）。
pub fn set_credential_source(account: &mut Value, source: CredentialSource) {
    if let Some(map) = account.as_object_mut() {
        map.insert(
            CREDENTIAL_SOURCE_FIELD.to_string(),
            Value::String(source.as_str().to_string()),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 显式来源优先，且三个取值都能原样往返。
    #[test]
    fn explicit_source_round_trips() {
        for source in [
            CredentialSource::Oauth,
            CredentialSource::Local,
            CredentialSource::Import,
        ] {
            let mut account = json!({"id": "a-1"});
            set_credential_source(&mut account, source);
            assert_eq!(explicit_credential_source(&account), Some(source));
            assert_eq!(credential_source(&account), source);
            assert_eq!(account[CREDENTIAL_SOURCE_FIELD], source.as_str());
        }
    }

    /// 未知取值不得被当成合法来源：回退到推断，而不是写进新值。
    #[test]
    fn unknown_value_falls_back_to_inference() {
        let account = json!({
            "id": "a-1",
            "credentialSource": "something-else",
            "auth_raw": {"auth": {}, "account": {}},
        });
        assert_eq!(explicit_credential_source(&account), None);
        assert_eq!(credential_source(&account), CredentialSource::Local);
    }

    /// 历史账号推断：认证文件根 → local；token 响应 → oauth；缺失 → local（保守）。
    #[test]
    fn legacy_accounts_infer_source_from_auth_raw_shape() {
        let local = json!({"auth_raw": {"account": {"uid": "u"}, "auth": {"accessToken": "at"}}});
        assert_eq!(credential_source(&local), CredentialSource::Local);

        let oauth = json!({"auth_raw": {"accessToken": "at", "expiresIn": 7200}});
        assert_eq!(credential_source(&oauth), CredentialSource::Oauth);

        assert_eq!(
            credential_source(&json!({"id": "a-1"})),
            CredentialSource::Local,
            "没有 auth_raw 时判不出，按保守档 local"
        );
        assert_eq!(
            credential_source(&json!({"auth_raw": {}})),
            CredentialSource::Local,
            "空对象同样判不出"
        );
        assert_eq!(
            credential_source(&json!({"auth_raw": "not-an-object"})),
            CredentialSource::Local
        );
    }

    /// 只有 oauth 允许在 live 出现信封时照常刷新。
    #[test]
    fn only_oauth_may_refresh_on_envelope() {
        assert!(CredentialSource::Oauth.may_refresh_on_envelope());
        assert!(!CredentialSource::Local.may_refresh_on_envelope());
        assert!(!CredentialSource::Import.may_refresh_on_envelope());
    }
}
