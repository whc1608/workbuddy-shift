//! 账号存储：读取/写入 `~/.workbuddy-shift/accounts.json`，与 Python 版共享数据目录。
//!
//! 对照 server.py `load_accounts` / `save_accounts` / `find_account` /
//! `account_display_name` / `account_meta`。

use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;

use crate::modules::config::{accounts_file, atomic_write, now_ms};
use crate::modules::variant::WbVariant;

/// 判断字段是否为 WorkBuddy 5.6 加密信封对象（`{$wbEncrypted, envelope}`）。
///
/// 导出导入预览与采集共用同一判定，不要在调用方复制实现。
pub fn is_envelope(v: &Value, key: &str) -> bool {
    v.get(key).is_some_and(is_envelope_value)
}

/// 判断一个**已取出的值**是否为加密信封对象。
///
/// 与 [`is_envelope`] 同源（后者转发到这里），供「凭据已经被 `secret_value` 取出来、
/// 只剩值本身」的场景使用（刷新侧判定 live 快照即为此，R6-01）。不要在调用方另写一份
/// `map.contains_key("$wbEncrypted")`：信封形态一旦演化，两处会分裂。
pub fn is_envelope_value(value: &Value) -> bool {
    matches!(value, Value::Object(map) if map.contains_key("$wbEncrypted"))
}

/// 是否持有未过期的明文 access_token（OAuth 扫码所得形态）。
/// 无 expiresAt 时视为有效（保守：不因缺字段丢弃明文凭据）。
///
/// 刷新侧（`refresh::decide_live_action`）判 live 信封闸门时共用本函数：口径必须与
/// 「本机重导入的信封不得覆盖未过期明文」完全一致，否则会出现「采集认为明文有效、
/// 刷新认为已过期」的分裂（R6-01）。
pub fn has_unexpired_plain_token(acc: &Value) -> bool {
    let Some(Value::String(s)) = acc.get("access_token") else {
        return false;
    };
    if s.trim().is_empty() {
        return false;
    }
    match acc.get("expiresAt").and_then(|v| v.as_i64()) {
        Some(exp) => exp > now_ms(),
        None => true,
    }
}

/// 仅本机维护的账号字段：不来自官方采集，**也不允许被官方数据覆盖**。
///
/// 三处写回路径共用这一份白名单：
/// 1. [`store::upsert_account_in`]——刷新 Token / 切换回写（`updated` 缺键时继承旧值）；
/// 2. [`identity::upsert_collected_account`]——扫码重登 / 自动导入本机登录态；
/// 3. `export_import::import::merge_import_record`——导入备份（旧备份缺键时保留本地值）。
///
/// **新增任何「本地元数据」字段（未来的标签、置顶、备注等级…）必须加进这里**，
/// 否则「刷新 Token」会拿官方数据把整条记录覆盖掉、用户刚填的东西直接消失。
/// R5-04 的 `mark` 就是踩了这个坑：漏掉一处，标记在刷新后被冲掉（见两处 `preserve_mark` 单测）。
///
/// `lastUsedAt` / `lastUsedClient`（issue #184「最近切换」）同理：切换时写入，官方资料
/// 重采（刷新 Token / 扫码重登 / 导入备份）都必须保留，否则每刷新一次官方资料，卡片上的
/// 「最近切换」徽标就会集体消失。写入入口只有 [`display::touch_last_used`]。
///
/// 白名单的一致性由 `tests/store.rs` 的 `local_account_fields_*` 不变量测试守住：
/// 加字段而忘了给样例值时，那条测试会直接失败并指出字段名。
///
/// **反向约束：认证状态字段不能进这里。** `needs_relogin` / `needs_relogin_reason` /
/// `needs_relogin_since` 这类「标记」靠**删键**清除，而 `inherit_local_fields` 的语义是
/// 「`incoming` 缺键就用旧值补回」——把它们当本地字段加进来，续期成功删掉的标记会被旧值
/// 复活，表现为「换了 Token 还是显示需重新登录」，且无编译错误、无运行时报错、无日志。
/// 该反向约束由 `refresh::tests::marker_fields_are_not_local_account_fields` 求交守住。
pub const LOCAL_ACCOUNT_FIELDS: [&str; 6] = [
    "note",
    "displayField",
    "mark",
    "lastUsedAt",
    "lastUsedClient",
    // R6-01：凭据来源（`oauth` / `local` / `import`）。本机维护字段——刷新 Token 的
    // `updated` 对象天然不带它，不继承就会被清空，账号随即退回「判不出」，
    // 而判不出按 `local` 处理会让 OAuth 账号莫名停止刷新。
    "credentialSource",
];

/// 把 `existing` 里的本地字段补进 `incoming`：`incoming` 已有的键保持原样（以新数据为准）。
///
/// 三处写回路径的行为差异只体现在「哪些键算本地字段」，所以循环体收敛在这里，
/// 避免三份手写循环各自漂移。
pub fn inherit_local_fields(existing: &Value, incoming: &mut Value) {
    let Some(obj) = incoming.as_object_mut() else {
        return;
    };
    for key in LOCAL_ACCOUNT_FIELDS {
        if !obj.contains_key(key) {
            if let Some(value) = existing.get(key) {
                obj.insert(key.to_string(), value.clone());
            }
        }
    }
}

// 手动添加账号（token 方式）已随 UI 入口「手动添加」一并下线；
// `identity_email` 中的 "手动添加" 占位过滤保留，用于兼容历史手动添加的旧账号。

mod display;
mod identity;
mod source;
mod store;

pub use display::{
    account_display_name, account_meta, build_auth_headers, display_value, envelope_token_error,
    get_str, mark_meta, touch_last_used, touch_last_used_in, update_account_display,
    update_account_display_in, LAST_USED_CLIENTS, MARK_COLORS, MARK_TEXT_MAX_CHARS,
};
pub use identity::{
    enterprise_id_of, is_enterprise_account, save_collected_account, secret_value,
    upsert_collected_account, variant_of,
};
pub use source::{
    credential_source, explicit_credential_source, set_credential_source, CredentialSource,
    CREDENTIAL_SOURCE_FIELD,
};
pub use store::{
    accounts_file_in, delete_account, find_account, import_local, load_accounts, load_accounts_at,
    save_accounts, upsert_account,
};

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    use super::display::*;
    use super::store::*;
    include!("tests/store.rs");
    include!("tests/display.rs");
}
