//! 读当前档位的 live 认证文件 → `same_account` 校验 → **只解密 `auth.accessToken`**。
//!
//! 为什么只解 access：refresh token 留在信封里就不存在「被误用去刷新」的可能；
//! 本模块因此从设计上就满足「绝不用解密出的 refresh token 调刷新接口」。
//!
//! 缓存：解密结果按「账号 id + 档位 + 认证文件路径 + 文件 mtime」缓存，TTL 5 分钟，
//! 且要求 `now < expiresAt`。客户端续期后 mtime 必然变化 → 下一次请求自动重新解密，
//! 这正是「客户端续期后不需要重登」的实现方式。

use std::path::Path;

use base64::Engine as _;
use serde_json::Value;
use zeroize::Zeroizing;

use super::envelope;
use super::key::AtRestKey;
use super::AtRestError;
use crate::modules::account;
use crate::modules::auth_file::same_account;
use crate::modules::locks;
use crate::modules::variant::WbVariant;

/// 解密出的 access token 最多复用 5 分钟（再短会让每次请求都重新解密，收益不大；
/// 再长会扩大内存明文的存在时间）。
pub(super) const ACCESS_CACHE_TTL_MS: i64 = 5 * 60 * 1000;

/// live 文件里的 access 凭据（明文只在内存）。
pub(super) struct LiveAccess {
    pub(super) token: Zeroizing<String>,
    pub(super) expires_at: i64,
}

struct CachedAccess {
    cache_key: String,
    expires_at: i64,
    stored_at: i64,
    token: Zeroizing<String>,
}

/// 解密结果缓存：可重建（下一次请求整体覆盖），中毒取回后继续。
static ACCESS_CACHE: std::sync::Mutex<Option<CachedAccess>> = std::sync::Mutex::new(None);

/// 读 live 认证文件并解出 access（**唯一入口**：路径与时钟由 `AtRestContext` 传入）。
///
/// 只有这一个入口：「读哪个文件、用哪个时钟」在调用链上只有一处决定，单测用临时文件
/// 与固定时钟走的是同一段代码，不存在两条实现漂移的可能。
pub(super) fn read_live_access_at(
    path: &Path,
    account: &Value,
    variant: WbVariant,
    key: &AtRestKey,
    now: i64,
    allow_cache: bool,
) -> Result<LiveAccess, AtRestError> {
    // 档位必须与调用方一致：认证文件是按档位取的，跨档位读会读到另一套账号。
    if account::variant_of(account) != variant {
        return Err(AtRestError::BindingMissing);
    }
    let mtime_ms = file_mtime_ms(path).ok_or(AtRestError::BindingMissing)?;
    let root =
        crate::modules::auth_file::read_auth_file_at(path).ok_or(AtRestError::BindingMissing)?;
    // 解密前必须确认是同一个账号（uid）：绝不允许把 B 的 live 凭据用到 A 的记录上。
    if !same_account(account, &root) {
        return Err(AtRestError::BindingMissing);
    }
    let account_id = account::get_str(account, "id").unwrap_or_default();
    let cache_key = format!(
        "{account_id}|{}|{}|{mtime_ms}",
        variant.as_str(),
        path.display()
    );
    if allow_cache {
        if let Some(cached) = cache_get(&cache_key, now) {
            return Ok(cached);
        }
    }

    let secret = live_access_secret(&root).ok_or(AtRestError::BindingMissing)?;
    let token = match secret {
        // live 已经是明文令牌（客户端未启用 at-rest 加密）：直接用，但同样只做临时副本。
        Value::String(text) => Zeroizing::new(text),
        envelope_value @ Value::Object(_) => {
            let parsed = envelope::parse(&envelope_value)?;
            envelope::open(&parsed, key.key_bytes(), key.key_id())?
        }
        _ => return Err(AtRestError::BindingMissing),
    };
    let expires_at = jwt_expires_at_ms(&token).ok_or(AtRestError::Malformed)?;
    // 已过期：**不刷新**（解密出的 RT 不存在，账号自己的 RT 也不该为临时副本动用），
    // 等客户端下一次续期后 mtime 变化再重新解密。
    if now >= expires_at {
        return Err(AtRestError::Expired);
    }
    cache_put(
        cache_key,
        LiveAccess {
            token: Zeroizing::new(token.to_string()),
            expires_at,
        },
        now,
    );
    Ok(LiveAccess { token, expires_at })
}

/// 取 live 的 access 凭据：优先 `auth.accessToken`（实测结构），兼容平铺写法。
fn live_access_secret(root: &Value) -> Option<Value> {
    let auth = root.get("auth").filter(|value| value.is_object());
    auth.and_then(|auth| account::secret_value(auth, "accessToken"))
        .or_else(|| account::secret_value(root, "accessToken"))
        .or_else(|| auth.and_then(|auth| account::secret_value(auth, "access_token")))
        .or_else(|| account::secret_value(root, "access_token"))
}

/// 认证文件的 mtime（毫秒）。取不到（文件不存在 / 平台不支持）视为不可用。
fn file_mtime_ms(path: &Path) -> Option<i64> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    let elapsed = modified.duration_since(std::time::UNIX_EPOCH).ok()?;
    i64::try_from(elapsed.as_millis()).ok()
}

/// JWT 的 `exp`（秒）换算成毫秒。取不到 `exp` 就无法判断「是否已过期」，按不可用处理。
fn jwt_expires_at_ms(token: &str) -> Option<i64> {
    let payload = token.split('.').nth(1)?;
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .ok()?;
    let value: Value = serde_json::from_slice(&decoded).ok()?;
    let exp = value.get("exp")?.as_i64()?;
    // 官方令牌是秒级；个别历史形态出现毫秒级，按数量级归一（避免把它当成 1970 年）。
    if exp > 100_000_000_000 {
        Some(exp)
    } else {
        Some(exp.saturating_mul(1000))
    }
}

fn cache_get(cache_key: &str, now: i64) -> Option<LiveAccess> {
    let guard = locks::recover(&ACCESS_CACHE);
    let cached = guard.as_ref()?;
    if cached.cache_key != cache_key
        || now.saturating_sub(cached.stored_at) >= ACCESS_CACHE_TTL_MS
        || now >= cached.expires_at
    {
        return None;
    }
    Some(LiveAccess {
        token: Zeroizing::new(cached.token.to_string()),
        expires_at: cached.expires_at,
    })
}

fn cache_put(cache_key: String, access: LiveAccess, now: i64) {
    let mut guard = locks::recover(&ACCESS_CACHE);
    *guard = Some(CachedAccess {
        cache_key,
        expires_at: access.expires_at,
        stored_at: now,
        token: Zeroizing::new(access.token.to_string()),
    });
}

/// 任何解密失败都让缓存立即失效（对外叫 invalidate，避免调用方以为可以「继续用旧的」）。
pub(super) fn invalidate_cache() {
    let mut guard = locks::recover(&ACCESS_CACHE);
    *guard = None;
}
