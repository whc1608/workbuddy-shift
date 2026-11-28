//! Token 刷新、刷新失败分级与「需重新登录」标记的复核。
//!
//! 对照 server.py `refresh_account_token` / `ensure_fresh_token` /
//! `run_keepalive_cycle`。R5-08（上游 issue #190）在这条链路上补了两件事：
//!
//! 1. **刷新失败分级**（[`classify_refresh_failure`]）：只有「认证被吊销」才写
//!    `needs_relogin`。断网 / 代理挂掉 / 超时 / 5xx / 429 / HTML 错误页属于
//!    「暂时失败」，只写 `refresh_failed_at` / `refresh_failed_reason` 供诊断，
//!    保留原凭据并稍后重试。旧实现把 `code != 0 && code != 200` 一律当成
//!    「需重新登录」，一次网络抖动就会把可用账号永久标红。
//! 2. **认证恢复后的复核**（[`verify_relogin`]）：带标记的账号在任意已认证请求
//!    路径上触发一次侧路复核——真正打一次刷新接口，成功才清除标记。标记只在
//!    「续期成功」时清除，**不得**因为积分查询成功就清除（issue 明确要求）。
//!
//! 复核的三道闸都在本文件内：
//! - 单飞：进程内 `Mutex<HashSet<account_id>>`，同账号同时只有一次（[`VerifySlot`]）；
//! - 冷却：`needs_relogin_checked_at` + 退避阶梯（60s → 5min → 30min → … → 6h 上限）；
//! - 旁路：[`note_auth_success`] / [`ensure_fresh_token`] 只 `spawn` 复核，不 await，
//!   触发它的那次请求的返回值不受影响。会内联刷新的账号不再 spawn（[`should_spawn_verify`]），
//!   否则同一账号会并发打两次刷新接口，refresh token 轮换会让两次请求互相作废。
//!
//! # 3. 当前账号不再被「抢刷」（T01）
//!
//! 保活默认无条件刷新全部账号（见 [`run_keepalive_cycle`]），而官方客户端与本工具共用
//! 同一份 refresh token：客户端刚续期过，本工具再用**账号库里的旧令牌**去刷新，服务端会
//! 把客户端手里的新令牌轮换掉——用户看到的现象是「客户端突然掉登录」。
//!
//! 因此 [`refresh_account_token`] 在**抢到 `credentials.lock` 之后、构造请求之前**重读
//! 官方认证文件（live）并三选一（判定见 [`decide_live_action`]）：
//!
//! - **adopt**：live 里是同一个 uid 且凭据确实更新（`auth_file::CredentialFreshness`）→ 只把快照
//!   回填账号库，**不发刷新请求、不写 live 文件**。这样库内凭据追上客户端，下一轮保活
//!   自然不会再去抢刷；
//! - **skip**：live 文件存在但读不出来（通常是客户端正在写）→ 本次跳过，不改库、不发请求；
//! - **照旧刷新**：文件不存在（该档位没有登录的官方客户端，也就没有「当前账号」）、
//!   或 live 属于别的账号 / 并不更新 → 与改造前逐字一致。
//!
//! 结果通过**只在内存里**的 `refresh_outcome` 字段回传给保活循环（[`refresh_outcome`]），
//! 它永远不进账号库：所有写入都在 `upsert_account` 之后才插入。
//!
//! # 4. 加密信封永不覆盖明文（R6-01）
//!
//! WorkBuddy 5.6 起客户端把 token 换成 `$wbEncrypted` 加密信封。T01 的 adopt 只判
//! 「同 uid + 值不同 + live 更新」，于是**信封被当成「更新的凭据」回填账号库**，把 OAuth
//! 扫码得到的明文覆盖掉；之后积分/签到/旅行全部短路，重登后客户端再续期又复发。
//!
//! 现在 [`decide_live_action`] 在 `differs_from` / `newer_than` **之前**先判信封，并按
//! 账号的凭据来源（[`crate::modules::account::credential_source`]）分流：
//!
//! | live access token | 账号库现状 | 来源 | 动作 | 内存 outcome |
//! | --- | --- | --- | --- | --- |
//! | 明文 | 任意 | 任意 | 照旧 adopt / 刷新 | `adopted` / `refreshed` |
//! | 信封 | 明文未过期 | 任意 | [`LiveAction::KeepPlaintext`] | `kept_plaintext` |
//! | 信封 | 明文已过期 / 无明文 | `oauth` | 照常刷新（独立会话） | `refreshed` |
//! | 信封 | 明文已过期 / 无明文 | `local` / `import` / 判不出 | [`LiveAction::SkipEncrypted`] | `skipped_encrypted` |
//! | 信封 | 已是信封 | 任意 | 跳过，不重复写 | `skipped_encrypted` |
//!
//! 两条跳过分支**只写内存 outcome**：不 `upsert_account`、不写 `needs_relogin`、不发请求。
//! [`apply_live_credentials`] 另有第二道闸（信封不许覆盖明文字符串），防止将来新增调用点
//! 绕过 [`decide_live_action`]。
//!
//! # 5. 信封 refresh token 不再被当成「缺少 refresh token」（R6-02）
//!
//! 信封账号的 `refresh_token` 是对象，`as_str()` 取不到 → 旧实现走「缺 refresh token」
//! 分支，每次启动/保活都补一条**误导性**的「需重新登录」。现在在判空**之前**先判信封
//! （[`has_envelope_credential`]，`access_token` 或 `refresh_token` 任一为信封即命中）：
//! 不写标记、不改库、0 请求，保活计 `skipped` 而不是 `failed`。
//!
//! 历史上已经被误标的账号在下一次刷新/保活路径上**惰性清理**（[`clear_mislabeled_relogin`]）：
//! 只认工具自己生成的两条文案（逐字匹配）且当前确实是信封，其它原因（服务端吊销）一律
//! 不动；不做启动时批量迁移，避免升级后一次性写全库。
//!
//! # 6. `__at_rest` 临时副本绝不刷新（R6-06 契约，由本文件提供守卫）
//!
//! R6-06 解密通道拿到的是**临时账号副本**，内存里带 `"__at_rest": true`。用它去刷新会
//! 用解密出的 refresh token 轮换掉客户端手里的令牌，导致客户端掉登录。因此
//! [`is_at_rest_temp`] 命中时 [`ensure_fresh_token`] / [`refresh_account_token`]
//! **入口第一步**就返回（outcome `skipped_at_rest`），[`run_keepalive_cycle`] 直接跳过。
//!
//! 该标记**只存在于内存**：禁止落盘、禁止进 DTO / 诊断 / 日志，也禁止进
//! `upsert_account` / `apply_live_credentials`。
//!
//! # 7. 凭据事件（R6-01/R6-03 契约）
//!
//! 每个决策点都往 [`crate::modules::credential_events`] 记一条（action 与 `refresh_outcome`
//! 同名，同账号同 action 60s 节流）。诊断包读那 200 条环形日志；桌面 App 没有控制台，
//! 只靠 `eprintln!` 等于不留痕。

use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::AtomicBool;
use std::sync::{Mutex, MutexGuard, OnceLock};

use crate::modules::account::{
    account_meta, build_auth_headers, credential_source, has_unexpired_plain_token, is_envelope,
    is_envelope_value, secret_value, upsert_account,
};
use crate::modules::auth_file;
use crate::modules::config::{
    http_request, is_html_failure, load_checkin_config, norm_ts, now_ms, RunFlagGuard,
};
use crate::modules::credential_events::{
    self, ACTION_ADOPTED, ACTION_KEPT_PLAINTEXT, ACTION_REFRESHED, ACTION_REFRESH_FAILED,
    ACTION_SKIPPED_ENCRYPTED, REASON_INVALID_GRANT, REASON_MISSING_REFRESH_TOKEN, REASON_OK,
};
use crate::modules::variant::WbVariant;

#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};

static KEEPALIVE_RUNNING: AtomicBool = AtomicBool::new(false);
static RELOGIN_SWEEP_RUNNING: AtomicBool = AtomicBool::new(false);

/// 单测里「本进程一共真正发出过多少次刷新请求」的计数器（生产不编译）。
///
/// 验收要求「adopt 时 **0 次**刷新请求」，而请求本身是不可达的（也不该打真实网关），
/// 只能用**发出前的计数不变**来证明「没有发」。放在 `http_request` 调用点前一步，
/// 因此它统计的是「真的要发」，不是「函数被调用」。
#[cfg(test)]
static REFRESH_HTTP_CALLS: AtomicUsize = AtomicUsize::new(0);

/// 单测用的刷新 URL 覆盖：`(账号 id 或 uid, 替换后的 URL)`。
///
/// 非当前账号必须走原刷新路径，而原路径会真的打网关（`www.codebuddy.cn` 等）。单测
/// 改成打本机不可达端口，只验证「尝试过刷新」；**按账号身份匹配**而不是全局生效，
/// 避免并行用例（`refresh_url_follows_account_variant`）读到别人的覆盖值。
#[cfg(test)]
static REFRESH_URL_OVERRIDE: Mutex<Option<(String, String)>> = Mutex::new(None);

/// 本条记录「本次没有真正刷新」的内存标记键（**从不落盘**）。
///
/// 抢不到凭据写入锁时既不能写 `needs_relogin`（那是「账号已失效」的错误结论），也不能
/// 安静地返回成功（保活/复核会报 OK）。因此在返回值里放一个内存标记，让
/// [`refresh_was_skipped`] 的调用方把这次算作「跳过」，不写冷却、不报成功。
const REFRESH_SKIPPED_FIELD: &str = "refresh_skipped";
/// [`REFRESH_SKIPPED_FIELD`] 的唯一取值：抢不到凭据写入锁。
const REFRESH_SKIPPED_LOCK_BUSY: &str = "credential_lock_busy";

/// 本次刷新的**机器可读结果**字段名（**只存在于内存，从不落盘**）。
///
/// 与 [`REFRESH_SKIPPED_FIELD`] 的分工：那个字段只表示「抢不到锁 ⇒ 这次没刷新」，
/// 是给宿主做响应投影用的稳定契约；本字段是**诊断**用的词表，覆盖「走了哪条路径」。
const REFRESH_OUTCOME_FIELD: &str = "refresh_outcome";
/// 采用 live 认证文件的新凭据（没有发刷新请求、没有写 live 文件）。
const REFRESH_OUTCOME_ADOPTED: &str = "adopted";
/// 走完了改造前的刷新路径（成功 / 暂时失败 / 吊销都在此，结论看标记字段）。
const REFRESH_OUTCOME_REFRESHED: &str = "refreshed";
/// live 认证文件存在但读不出来（可能在写）：本次跳过。
const REFRESH_OUTCOME_SKIPPED_LIVE_UNREADABLE: &str = "skipped_live_unreadable";
/// 抢不到 `credentials.lock`：本次跳过。
const REFRESH_OUTCOME_SKIPPED_CREDENTIAL_LOCK: &str = "skipped_credential_lock";
/// live 是加密信封、账号库明文仍有效：保留明文（不回填、不发请求、不落盘，R6-01）。
const REFRESH_OUTCOME_KEPT_PLAINTEXT: &str = "kept_plaintext";
/// 凭据是加密信封且不该刷新（来源非 oauth / 库内已是信封）：不发请求、不改库（R6-01/R6-02）。
const REFRESH_OUTCOME_SKIPPED_ENCRYPTED: &str = "skipped_encrypted";
/// R6-06 的临时账号副本（`__at_rest`）：绝刷新（会轮换掉客户端手里的令牌）。
const REFRESH_OUTCOME_SKIPPED_AT_REST: &str = "skipped_at_rest";

/// 本次刷新的机器可读结果；不是本文件产出的对象（或没走到刷新）时返回 `None`。
///
/// 取值是上面全部 `REFRESH_OUTCOME_*` 常量，即：
///
/// - `adopted`：采用 live 快照，没发请求；
/// - `refreshed`：走完了刷新路径（成功与否看标记字段）；
/// - `kept_plaintext`：live 是信封、库内明文仍可用，保留明文（R6-01）；
/// - `skipped_encrypted`：凭据是信封且不该刷新（R6-01 / R6-02）；
/// - `skipped_live_unreadable`：live 文件存在但读不出来（可能在写）；
/// - `skipped_credential_lock`：抢不到凭据写入锁；
/// - `skipped_at_rest`：R6-06 的临时账号副本，禁止刷新。
///
/// 标记只在返回值里，`upsert_account` 之后才插入，因此**不会**出现在账号库里（单测
/// `adopt_when_live_credentials_are_newer` 同时断言这一点）。
pub fn refresh_outcome(account: &Value) -> Option<&str> {
    account.get(REFRESH_OUTCOME_FIELD).and_then(Value::as_str)
}

/// 内存字段名：R6-06 解密出的**临时账号副本**标记（只存在于内存）。
///
/// 该标记不得落盘、不得进 DTO / 诊断 / 日志；也不是账号库字段（账号库里出现它说明有人
/// 把临时副本写回了，属于事故）。名字带 `__` 前缀是为了在任何序列化/诊断投影里一眼看出
/// 「这是内部标记、不是账号数据」。
///
/// 对外公开是为了让 A 轨（R6-06 解密通道）打标记与这里判标记用**同一个**字符串：
/// 硬编码 `"__at_rest"` 会在改名时静默失配——守卫读不到标记就等于守卫不存在，而那时
/// 临时副本会被真的拿去刷新、把客户端的 refresh token 轮换掉。
pub const AT_REST_FIELD: &str = "__at_rest";

/// 该账号是否为 R6-06 解密通道产出的**临时副本**（见 [`AT_REST_FIELD`]）。
///
/// 守卫语义：临时副本的 token 是本次请求临时解密出来的，用它去打刷新接口会把客户端
/// 手里的 refresh token 轮换掉（客户端直接掉登录）。因此刷新链路上见到它一律**不刷新**。
/// A 轨只调用本函数，不修改 `refresh.rs`。
pub fn is_at_rest_temp(account: &Value) -> bool {
    account.get(AT_REST_FIELD).and_then(Value::as_bool) == Some(true)
}

/// 本次刷新是否因为抢不到凭据写入锁而被跳过（该标记只在内存里，从不落盘）。
pub fn refresh_was_skipped(account: &Value) -> bool {
    refresh_skip_reason(account).is_some()
}

/// 本次刷新被跳过的**机器可读**取值；没被跳过时返回 `None`。
///
/// 宿主（Tauri command / HTTP handler）用它给响应加 `refreshSkipped` 字段：跳过时既不能
/// 报成功（界面会以为令牌已续期），也不能报失败（会让用户以为账号失效）。当前只有
/// [`REFRESH_SKIPPED_LOCK_BUSY`] 一个取值。
pub fn refresh_skip_reason(account: &Value) -> Option<&str> {
    account
        .get(REFRESH_SKIPPED_FIELD)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

/// 本次刷新被跳过的**用户可读**原因（含「未写入任何凭据」的说明）；没被跳过时返回 `None`。
pub fn refresh_skip_message(account: &Value) -> Option<String> {
    refresh_skip_reason(account)?;
    account
        .get("refresh_failed_reason")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// 「刷新令牌」入口的响应投影：`account_meta` + 跳过标记（issue #202）。
///
/// 两个宿主（Tauri `commands::refresh_account_token` 与 HTTP `api_refresh_token`）**共用**
/// 本函数，字段名与取值只有这一处定义：
///
/// - 正常刷新：与改造前完全一致，只有 `account_meta`，不新增任何字段；
/// - 抢不到凭据写入锁：额外带 `refreshSkipped`（机器可读，当前唯一取值
///   `credential_lock_busy`）与 `refreshSkippedReason`（用户可读原因）。
///
/// 为什么不复用 `needsRelogin` 之类既有字段：跳过既不是成功也不是「账号失效」，
/// 借用任何一种都会让前端给出错误结论；新字段只在真正发生跳过的响应里出现，
/// 老客户端读到未知字段会忽略，不构成兼容问题。
pub fn refresh_result_meta(account: &Value) -> Value {
    let mut meta = account_meta(account);
    if let Some(reason) = refresh_skip_reason(account) {
        meta["refreshSkipped"] = json!(reason);
        meta["refreshSkippedReason"] = json!(refresh_skip_message(account).unwrap_or_default());
    }
    meta
}

// ---------------------------------------------------------------------------
// R5-08 A：刷新失败分级
// ---------------------------------------------------------------------------

/// 上游「认证被吊销」业务码：`Invalid refresh token` / `Session doesn't have
/// required client`。两个档位都用这个码表示 refresh token 已不可用。
const CODE_REFRESH_REVOKED: i64 = 12153;

/// 刷新失败分类。**只有 [`RefreshFailure::Revoked`] 允许写 `needs_relogin`。**
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshFailure {
    /// 认证已被吊销（refresh token 失效 / 被拒 / 已过期）：必须重新登录。
    Revoked,
    /// 暂时失败（网络、代理、超时、5xx、429、HTML 错误页、响应缺字段）：
    /// 不写标记，保留原凭据，稍后自动重试。
    Temporary,
    /// 无法判断的业务码：保守按暂时失败处理，只写诊断字段。
    Unknown,
}

impl RefreshFailure {
    /// 该分类是否允许写「需重新登录」标记。
    pub fn marks_relogin(self) -> bool {
        matches!(self, Self::Revoked)
    }

    /// 写进诊断字段的分类名（日志/通知里可直接读）。
    pub fn label(self) -> &'static str {
        match self {
            Self::Revoked => "revoked",
            Self::Temporary => "temporary",
            Self::Unknown => "unknown",
        }
    }
}

/// 失败原因里「暂时失败」的说明：明确告诉用户会自动重试，避免被读成需要手动登录。
const TEMPORARY_HINT: &str = "暂时失败，稍后自动重试";

/// 从响应里取 `code`：兼容数字与数字字符串（网关偶发把 `code` 写成字符串）。
fn response_code(resp: &Value) -> Option<i64> {
    resp.get("code")
        .and_then(|value| {
            value
                .as_i64()
                .or_else(|| value.as_str().and_then(|text| text.trim().parse().ok()))
        })
        .or_else(|| {
            resp.get("data").and_then(|data| {
                data.get("code").and_then(|value| {
                    value
                        .as_i64()
                        .or_else(|| value.as_str().and_then(|text| text.trim().parse().ok()))
                })
            })
        })
}

/// 把响应里所有可读的错误文案拼成一段（`message` / `msg` / `data.*`）。
///
/// 分级只看响应本身，不推断上游故障原因（与上游 #191 的非目标一致）；因此这里
/// 不做任何「根据 code 反推文案」的事，只做取证。
fn response_message(resp: &Value) -> String {
    let mut text = String::new();
    for holder in [Some(resp), resp.get("data")] {
        let Some(holder) = holder else { continue };
        for key in ["message", "msg"] {
            if let Some(value) = holder.get(key).and_then(Value::as_str) {
                let value = value.trim();
                if !value.is_empty() {
                    text.push_str(value);
                    text.push(' ');
                }
            }
        }
    }
    text.trim().to_string()
}

/// 文案是否来自网关的 HTML 错误页。
///
/// 两个来源都要认：
/// - 老形态：`normalize_error_body` 归一化出的「服务端返回 HTML 错误页：<title>」前缀
///   或原始 `<…`；
/// - 新形态：`config::failure` 分类后的固定文案（`登录已失效，需要重新登录` /
///   `服务端返回了网页而不是数据…`）。文案本身不含任何标记，因此**判定不能看文案**，
///   要看 [`is_html_failure`] 读的 `failureKind` 字段。
fn looks_like_html_error(message: &str) -> bool {
    let text = message.trim_start();
    text.starts_with('<') || text.contains("服务端返回 HTML 错误页")
}

/// 文案里是否出现吊销语义。
///
/// 这里的关键词都来自实测：`12153` 的两个官方文案（`invalid_grant: Invalid
/// refresh token`、`Session doesn't have required client`）以及网关的
/// `refresh token expired / revoked`。误判方向是「该提示时不提示」，因此宁可窄一点。
fn has_revocation_semantics(message: &str) -> bool {
    let text = message.to_lowercase();
    if [
        "invalid_grant",
        "invalid refresh token",
        "invalid_refresh_token",
        "required client",
        "revoked",
        "吊销",
        "已失效",
    ]
    .iter()
    .any(|keyword| text.contains(keyword))
    {
        return true;
    }
    // 「refresh token 过期」只有在明确指向 refresh token 时才算吊销：单纯
    // 「access token 过期」是正常续期场景，不能据此要求重新登录。
    (text.contains("refresh token") || text.contains("refresh_token"))
        && (text.contains("expired") || text.contains("过期") || text.contains("失效"))
}

/// 文案里是否出现「传输层/网关」语义（没有 code 时用来兜底判暂时失败）。
fn has_transport_semantics(message: &str) -> bool {
    let text = message.to_lowercase();
    [
        "timed out",
        "timeout",
        "deadline",
        "connection",
        "connect ",
        "dns",
        "proxy",
        "代理",
        "reset by peer",
        "unexpected eof",
        "tls",
        "certificate",
        "bad gateway",
        "gateway",
        "service unavailable",
        "too many requests",
    ]
    .iter()
    .any(|keyword| text.contains(keyword))
}

/// 刷新失败分级。**纯函数**：判据集中在这里，便于按实测补关键词。
///
/// 传入的应当是一次**失败**的刷新响应（`code` 不是 0/200）。成功码会被归到
/// [`RefreshFailure::Unknown`]，调用方不应依赖这一支。
pub fn classify_refresh_failure(resp: &Value) -> RefreshFailure {
    let code = response_code(resp);
    let message = response_message(resp);

    // 网关的 401/5xx 常返回整页 HTML（`http_request` 统一分类成 `failureKind`，
    // 文案是固定短句、不再含标记，所以文案判定与字段判定两条都要走）。这既不是吊销
    // 也不是业务码 —— 先判掉，否则会被下面的 401/403 分支误判成「需重新登录」
    // （issue #94 的教训）。
    if looks_like_html_error(&message) || is_html_failure(resp) {
        return RefreshFailure::Temporary;
    }

    match code {
        // 刷新接口明确说 refresh token 不可用。
        Some(CODE_REFRESH_REVOKED) => return RefreshFailure::Revoked,
        // `http_request` 的传输错误约定：`{"code": -1, "message": ...}`。
        Some(-1) => return RefreshFailure::Temporary,
        // 限流与网关/服务端故障都是可重试的。
        Some(429) => return RefreshFailure::Temporary,
        Some(status) if (500..=599).contains(&status) => return RefreshFailure::Temporary,
        // 401/403：响应是 JSON 且不是 HTML 错误页 —— 刷新接口的鉴权拒绝。
        // 即使文案没写吊销关键词也按吊销处理：继续拿同一个 refresh token 重试
        // 没有意义，只会刷接口（冷却/退避仍然生效）。
        Some(401 | 403) => return RefreshFailure::Revoked,
        _ => {}
    }

    if has_revocation_semantics(&message) {
        return RefreshFailure::Revoked;
    }
    // 缺 `code`（`Value::Null`、非 JSON 响应体、字段被裁剪）或文案是传输层错误：
    // 无法确认凭据状态，按暂时失败处理，绝不给账号标「需重新登录」。
    if code.is_none() || has_transport_semantics(&message) {
        return RefreshFailure::Temporary;
    }
    // 其它业务码（例如网关指纹拦截 10085）：保守按暂时失败，只记诊断字段。
    RefreshFailure::Unknown
}

/// 失败响应 → 给人看的原因文案（分级 + code + 上游原话）。
fn refresh_failure_reason(failure: RefreshFailure, resp: &Value) -> String {
    let code = response_code(resp)
        .map(|code| code.to_string())
        .unwrap_or_else(|| "未知".to_string());
    let message = response_message(resp);
    let message = if message.is_empty() {
        "上游未给出原因".to_string()
    } else {
        message
    };
    match failure {
        RefreshFailure::Revoked => format!("续期被拒（code={code}）：{message}"),
        RefreshFailure::Temporary => format!("续期{TEMPORARY_HINT}（code={code}）：{message}"),
        RefreshFailure::Unknown => {
            format!("续期结果未知（code={code}）：{message}，{TEMPORARY_HINT}")
        }
    }
}

// ---------------------------------------------------------------------------
// R5-08 A：标记的读写（纯内存，落盘由调用方负责）
// ---------------------------------------------------------------------------

/// 「需重新登录」标记的全部字段。清标记时必须一起清，否则会残留旧 reason /
/// 旧起点，前端 tooltip 会显示上一次失败的时间。
const RELOGIN_FIELDS: [&str; 5] = [
    "needs_relogin",
    "needs_relogin_reason",
    "needs_relogin_since",
    "needs_relogin_fail_count",
    "needs_relogin_last_result",
];

/// 暂时失败的诊断字段（不属于标记，清标记时一并清掉）。
const TEMPORARY_FIELDS: [&str; 2] = ["refresh_failed_at", "refresh_failed_reason"];

/// 账号是否带「需重新登录」标记。
pub fn has_relogin_marker(account: &Value) -> bool {
    account.get("needs_relogin").and_then(Value::as_bool) == Some(true)
}

/// 写入「需重新登录」标记（仅 [`RefreshFailure::Revoked`] / 缺 refresh token 时调用）。
///
/// 同时把 `needs_relogin_checked_at` 更新为本次尝试时间：刚失败的这一次本身就是
/// 一次「复核」，冷却从这里开始算，避免紧接着再打一次刷新接口。
fn mark_relogin_required(account: &mut Value, reason: String) {
    let now = now_ms();
    let already_marked = has_relogin_marker(account);
    let fail_count = account
        .get("needs_relogin_fail_count")
        .and_then(Value::as_i64)
        .unwrap_or(0)
        + 1;
    account["needs_relogin"] = json!(true);
    account["needs_relogin_reason"] = json!(reason);
    account["needs_relogin_fail_count"] = json!(fail_count);
    account["needs_relogin_checked_at"] = json!(now);
    if !already_marked {
        account["needs_relogin_since"] = json!(now);
    }
    if let Some(map) = account.as_object_mut() {
        for key in TEMPORARY_FIELDS {
            map.remove(key);
        }
    }
}

/// 记录一次「暂时失败」：**不动** `needs_relogin`（原标记保持原样）。
fn mark_temporary_failure(account: &mut Value, reason: String) {
    account["refresh_failed_at"] = json!(now_ms());
    account["refresh_failed_reason"] = json!(reason);
}

/// 清除「需重新登录」标记及其它刷新诊断字段（唯一清除点，只在续期成功时调用）。
fn clear_relogin_marker(account: &mut Value) {
    if let Some(map) = account.as_object_mut() {
        for key in RELOGIN_FIELDS {
            map.remove(key);
        }
        for key in TEMPORARY_FIELDS {
            map.remove(key);
        }
    }
}

// ---------------------------------------------------------------------------
// R6-01 / R6-02：客户端加密信封凭据
// ---------------------------------------------------------------------------

/// 账号的凭据里是否有加密信封（`access_token` **或** `refresh_token` 是信封对象）。
///
/// R6-02 的判据就是它：信封的 `refresh_token` 是对象，`as_str()` 取不到 → 旧实现误判成
/// 「缺少 refresh token」并写「需重新登录」。两个字段都要认——只判 `refresh_token` 会漏掉
/// 「access 是信封、refresh 缺失」这种形态，只判 `access_token` 会漏掉反向形态。
fn has_envelope_credential(account: &Value) -> bool {
    is_envelope(account, "refresh_token") || is_envelope(account, "access_token")
}

/// 工具自己生成的「缺少 refresh token」文案（R6-02 历史误标**逐字**匹配表）。
///
/// 两条分别来自 `refresh_account_token`（刷新）与 `run_keepalive_cycle`（保活）。
/// 只有**逐字相等**才允许清理：服务端吊销也有自己的文案（`续期被拒（code=…）`），
/// 放宽成「包含关键词」会误清真实吊销标记。
const MISLABELED_RELOGIN_REASONS: [&str; 2] = [
    "缺少 refresh token，无法刷新，需重新登录",
    "缺少 refresh token，无法保活，需重新登录",
];

/// 惰性清理 R6-02 的历史误标：**只有**「带标记 + 原因逐字匹配 + 当前确实是信封」三条
/// 同时成立才清，返回是否真的清过（调用方据此决定要不要落盘）。
///
/// 时机是「该账号下一次刷新/保活」，不做启动时批量迁移：升级后一次性写全库既有风险
/// （写坏账号库代价远高于晚清一条标记），也会把用户没在用的账号无谓唤醒。
fn clear_mislabeled_relogin(account: &mut Value) -> bool {
    if !has_relogin_marker(account) {
        return false;
    }
    let reason = account
        .get("needs_relogin_reason")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !MISLABELED_RELOGIN_REASONS.contains(&reason) {
        return false;
    }
    if !has_envelope_credential(account) {
        // 明文账号上的「缺少 refresh token」可能是真的（RT 被删/被吊销），保留标记。
        return false;
    }
    clear_relogin_marker(account);
    true
}

/// 信封凭据的统一返回值：**不落库、不发请求、不写标记**，只带内存 outcome。
///
/// 唯一的例外是 R6-02 的惰性清理：那一支确实要写库（清历史误标），因此单独落盘一次。
/// 注意本函数**不**持 `credentials.lock`：它没有网络请求、也不改凭据，抢锁只会把切号
/// 与真正的刷新挡在门外。
fn skip_encrypted_credential(mut account: Value) -> Value {
    let cleaned = clear_mislabeled_relogin(&mut account);
    if cleaned {
        eprintln!("[refresh] 清理历史误标：信封凭据不再算「缺少 refresh token」");
        let _ = upsert_account(&account);
    }
    account[REFRESH_OUTCOME_FIELD] = json!(REFRESH_OUTCOME_SKIPPED_ENCRYPTED);
    credential_events::record_for_account(&account, ACTION_SKIPPED_ENCRYPTED, REASON_OK);
    eprintln!("[refresh] 跳过刷新：凭据为客户端加密信封态（不改库、不发请求）");
    account
}

// ---------------------------------------------------------------------------
// R5-08 B：复核的闸门（单飞 + 冷却 + 退避）
// ---------------------------------------------------------------------------

/// 首次复核的冷却窗口。
pub const RELOGIN_VERIFY_COOLDOWN_MS: i64 = 60_000;

/// 退避阶梯：60s → 5min → 30min → 1h → 2h → 4h → 6h（封顶）。
///
/// 吊销类失败连续发生时逐步拉长间隔：上游账号确实被吊销时不该每 60s 打一次接口。
const RELOGIN_BACKOFF_LADDER_MS: [i64; 7] = [
    60_000, 300_000, 1_800_000, 3_600_000, 7_200_000, 14_400_000, 21_600_000,
];

/// 当前账号的复核冷却时长（按连续失败次数取退避阶梯，上限 6h）。
pub fn verify_cooldown_ms(account: &Value) -> i64 {
    let fail_count = account
        .get("needs_relogin_fail_count")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let index = if fail_count <= 1 {
        0
    } else {
        (fail_count - 1).min(RELOGIN_BACKOFF_LADDER_MS.len() as i64 - 1) as usize
    };
    RELOGIN_BACKOFF_LADDER_MS[index]
}

/// 现在是否该复核这个账号（纯函数，`now` 由调用方注入便于单测）。
///
/// 条件：带标记 + 距上次复核（`needs_relogin_checked_at`）已超过冷却窗口。
/// 没有 `needs_relogin_checked_at`（历史标记）时视为「从未复核」，立即复核。
pub fn should_verify_relogin(account: &Value, now: i64) -> bool {
    if !has_relogin_marker(account) {
        return false;
    }
    let Some(last_checked_at) = account
        .get("needs_relogin_checked_at")
        .and_then(Value::as_i64)
    else {
        return true;
    };
    now.saturating_sub(last_checked_at) >= verify_cooldown_ms(account)
}

/// 复核前的判定结果。区分「为什么不做」而不是只给布尔值：日志、单测与将来
/// 的界面提示都要能区分「本来就不需要」「刚试过」「正在试」。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyPlan {
    /// 该复核：带标记且已过冷却窗口。
    Proceed,
    /// 不带标记：正常账号，零额外请求。
    SkipNoMarker,
    /// 冷却中（含退避）：本轮不再打刷新接口。
    SkipCooldown,
}

/// 复核判定（纯函数，`now` 由调用方注入便于单测）。
///
/// 与 [`should_verify_relogin`] 同源但更细：`should_verify_relogin` 供批量扫描做
/// 过滤（只要布尔），本函数供 [`verify_relogin`] 给出可读的跳过原因。
pub fn verify_plan(account: &Value, now: i64) -> VerifyPlan {
    if !has_relogin_marker(account) {
        return VerifyPlan::SkipNoMarker;
    }
    if !should_verify_relogin(account, now) {
        return VerifyPlan::SkipCooldown;
    }
    VerifyPlan::Proceed
}

static VERIFY_SLOTS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

fn verify_slots() -> &'static Mutex<HashSet<String>> {
    VERIFY_SLOTS.get_or_init(|| Mutex::new(HashSet::new()))
}

/// 取在飞集合的锁。锁中毒时取回内部数据继续用：单飞是一层优化，
/// 不该因为一次 panic 变成全局不可用（`scripts/check-rust-locks.mjs` 同样禁止
/// 加锁后 unwrap/expect）。
fn lock_verify_slots() -> MutexGuard<'static, HashSet<String>> {
    verify_slots()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 单飞令牌：拿到表示「本进程内该账号的复核由我负责」，Drop 时自动释放。
///
/// 并发合并的意义：账号页/积分页/签到/保活可能同时触发复核，没有这道闸就会
/// 对同一账号打多次 `/auth/token/refresh`（官方接口对并发刷新不友好）。
struct VerifySlot {
    account_id: String,
}

impl Drop for VerifySlot {
    fn drop(&mut self) {
        let mut slots = lock_verify_slots();
        slots.remove(&self.account_id);
    }
}

/// 尝试占用该账号的复核位；已被别人占用时返回 `None`（调用方应当安静跳过）。
fn try_acquire_verify_slot(account_id: &str) -> Option<VerifySlot> {
    let mut slots = lock_verify_slots();
    if !slots.insert(account_id.to_string()) {
        return None;
    }
    Some(VerifySlot {
        account_id: account_id.to_string(),
    })
}

/// 账号的稳定身份（`id` 优先，历史数据回退 `uid`）。
fn account_identity(account: &Value) -> Option<String> {
    for key in ["id", "uid"] {
        if let Some(value) = account.get(key).and_then(Value::as_str) {
            let value = value.trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

/// 复核结束后回写复核时间与结果（`needs_relogin_checked_at` 是冷却的唯一依据）。
fn record_verify_result(account_id: &str, recovered: bool) {
    let Some(mut account) = crate::modules::account::find_account(account_id) else {
        return;
    };
    if recovered {
        // 成功复核后标记已由 `refresh_account_token` 清除；这里只补「上次复核」。
        clear_relogin_marker(&mut account);
    }
    account["needs_relogin_checked_at"] = json!(now_ms());
    account["needs_relogin_last_result"] = json!(if recovered { "ok" } else { "failed" });
    let _ = upsert_account(&account);
}

/// 复核一个带「需重新登录」标记的账号：真正打一次刷新接口，成功才清标记。
///
/// 返回值只用于日志/单测，**不是**业务结果：调用方（[`note_auth_success`]）不会
/// 等它，也不会改自己那次请求的返回值。
///
/// 顺序很重要：
/// 1. 先按 id 读档 → 不存在就跳过（不复活已删除账号）；
/// 2. 冷却检查（`needs_relogin_checked_at` + 退避）——被冷却时**不**打接口；
/// 3. 单飞占位——同账号并发只打一次；
/// 4. 占位后**重读**：期间账号可能被删、标记可能被别的路径清掉、展示字段
///    （备注 / 显示选择）可能被用户改过；重读后的对象作为刷新输入，
///    避免用旧快照覆盖这些字段；
/// 5. 复核 = `refresh_account_token`（吊销才写标记、成功清标记）；
/// 6. 回写 `needs_relogin_checked_at` 与结果。
pub async fn verify_relogin(account_id: &str) -> Value {
    let Some(account) = crate::modules::account::find_account(account_id) else {
        return json!({"accountId": account_id, "skipped": "missing"});
    };
    match verify_plan(&account, now_ms()) {
        VerifyPlan::Proceed => {}
        VerifyPlan::SkipNoMarker => {
            return json!({"accountId": account_id, "skipped": "no_marker"});
        }
        VerifyPlan::SkipCooldown => {
            return json!({"accountId": account_id, "skipped": "cooldown"});
        }
    }
    let Some(_slot) = try_acquire_verify_slot(account_id) else {
        return json!({"accountId": account_id, "skipped": "in_flight"});
    };
    let Some(current) = crate::modules::account::find_account(account_id) else {
        return json!({"accountId": account_id, "skipped": "deleted"});
    };
    if !has_relogin_marker(&current) {
        return json!({"accountId": account_id, "skipped": "no_marker"});
    }
    let verified = refresh_account_token(current).await;
    if refresh_was_skipped(&verified) {
        // 抢不到凭据写入锁：本次**没有**真正复核。既不能当成成功（会清掉标记、谎报复核通过），
        // 也不能当成失败写冷却（会把真正的自愈往后推）。什么都不写，如实说「跳过了」。
        return json!({
            "accountId": account_id,
            "verified": false,
            "skipped": "credential_lock_busy",
            "reason": verified
                .get("refresh_failed_reason")
                .and_then(Value::as_str)
                .map(str::to_string),
        });
    }
    if refresh_outcome(&verified) == Some(REFRESH_OUTCOME_SKIPPED_LIVE_UNREADABLE) {
        // 认证文件正在被官方客户端写：这次没有打接口，也就没有「复核结论」。
        // 与抢不到锁同一条语义——**不写** `needs_relogin_checked_at`（那是冷却起点，
        // 写了等于谎报「复核失败」），下一轮再试。
        return json!({
            "accountId": account_id,
            "verified": false,
            "skipped": "live_unreadable",
            "reason": Value::Null,
        });
    }
    let recovered = !has_relogin_marker(&verified);
    record_verify_result(account_id, recovered);
    json!({
        "accountId": account_id,
        "verified": recovered,
        "skipped": Value::Null,
        "reason": verified
            .get("needs_relogin_reason")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

/// 旁路 spawn 一次复核：不阻塞调用方，也没有 tokio 运行时时安静跳过。
///
/// 同步上下文（例如 CLI 的一次性调用）里没有 runtime，此时复核留给下一次
/// 带 runtime 的触发点或宿主后台扫描（[`run_relogin_verify_sweep`]）。
fn spawn_verify_relogin(account_id: String) {
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        handle.spawn(async move {
            let _ = verify_relogin(&account_id).await;
        });
    }
}

/// 一次已认证请求成功后调用：带旧标记的账号顺带复核续期。
///
/// 未带标记的账号直接返回（零额外请求）——这是方案 1 的关键前提：正常账号不会
/// 因为这条钩子多打刷新接口。并发与频率由 [`verify_relogin`] 内部保证。
pub fn note_auth_success(account_id: &str) {
    let Some(account) = crate::modules::account::find_account(account_id) else {
        return;
    };
    if !has_relogin_marker(&account) {
        return;
    }
    spawn_verify_relogin(account_id.to_string());
}

/// 兜底扫描：给带标记的账号逐个复核（方案 2）。
///
/// 覆盖「只有后台任务在跑、没有前端请求」的场景。已过冷却的账号才处理，
/// 因此可以按任意间隔调用（建议 10 分钟）而不会产生额外请求。
pub async fn run_relogin_verify_sweep() -> Value {
    let Some(_guard) = RunFlagGuard::try_acquire(&RELOGIN_SWEEP_RUNNING) else {
        return json!({"skipped": "already_running"});
    };
    let now = now_ms();
    let targets: Vec<String> = crate::modules::account::load_accounts()
        .iter()
        .filter(|account| should_verify_relogin(account, now))
        .filter_map(account_identity)
        .collect();
    let mut results: Vec<Value> = Vec::with_capacity(targets.len());
    for account_id in targets {
        results.push(verify_relogin(&account_id).await);
    }
    json!({"checked": results.len(), "results": results})
}

// ---------------------------------------------------------------------------
// 刷新本体
// ---------------------------------------------------------------------------

/// 刷新接口 URL：域名与路径前缀都按账号自身档位选（缺档位字段 → 国内版，零回归）。
fn refresh_url(account: &Value) -> String {
    // 单测注入：非当前账号的用例必须证明「真的去刷新了」，但不能打真实网关。
    // 只对指定账号生效，其它用例（含 `refresh_url_follows_account_variant`）读到的
    // 仍是按档位拼出来的真实 URL。
    #[cfg(test)]
    if let Some(url) = test_url_override(account) {
        return url;
    }
    let variant = crate::modules::account::variant_of(account);
    format!(
        "{}{}/auth/token/refresh",
        variant.api_endpoint(),
        variant.api_prefix()
    )
}

/// 取单测注入的 URL（仅当账号 id 或 uid 与注入键一致时命中）。
#[cfg(test)]
fn test_url_override(account: &Value) -> Option<String> {
    // `REFRESH_URL_OVERRIDE` 是单测的注入点：中毒只可能来自用例自身 panic，
    // 取回内部值继续用即可（不涉及任何业务状态），因此走 `recover`。
    let guard = crate::modules::locks::recover(&REFRESH_URL_OVERRIDE);
    let (key, url) = guard.as_ref()?;
    let matched = ["id", "uid"].iter().any(|field| {
        account
            .get(*field)
            .and_then(Value::as_str)
            .is_some_and(|value| value == key)
    });
    matched.then(|| url.clone())
}

/// 构造刷新请求头：鉴权头 + 刷新会话头。
///
/// `X-Auth-Refresh-Source: plugin` 必须保留：两个档位的官方客户端刷新时都发送该头，
/// 缺失时网关会把这次刷新判定成另一个 client 来源，国际版实测返回
/// `invalid_grant: Invalid refresh token`（code 12153）。抽成纯函数是为了让单测
/// 能把这个头固定住，避免将来被顺手删掉。
fn refresh_headers(account: &Value, refresh_token: &str) -> HashMap<String, String> {
    let mut headers = build_auth_headers(account);
    headers.insert("X-Refresh-Token".to_string(), refresh_token.to_string());
    headers.insert("X-Auth-Refresh-Source".to_string(), "plugin".to_string());
    headers
}

/// 官方认证文件（live）的读取结果。
///
/// **必须区分「文件不存在」与「文件在但读不出来」**：
/// - 不存在 → 该档位没有登录的官方客户端，也就没有「当前账号」，保活要照旧刷新
///   （否则这台机器上没装客户端时保活会永久停摆，闲置令牌会被服务端清理）；
/// - 存在但读不出来 → 官方客户端很可能正在写它，此刻任何结论都不可靠，只能跳过本次。
enum LiveAuthState {
    /// 文件不存在。
    Absent,
    /// 文件存在但读/解析失败。
    Unreadable,
    /// 解析成功的认证文件根。
    Parsed(Value),
}

/// 读 live 认证文件，并保留「不存在 / 读不出来」的区别（[`auth_file::read_auth_file`]
/// 把两者都压成 `None`，这里不能用它）。
fn read_live_auth_state(variant: WbVariant) -> LiveAuthState {
    let path = variant.auth_file_path();
    match std::fs::read_to_string(&path) {
        Ok(text) => match serde_json::from_str::<Value>(&text) {
            Ok(root) => LiveAuthState::Parsed(root),
            Err(_) => LiveAuthState::Unreadable,
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => LiveAuthState::Absent,
        Err(_) => LiveAuthState::Unreadable,
    }
}

/// live 认证文件里可以回填到账号库的凭据快照。
///
/// token 一律用 [`secret_value`] 取：WorkBuddy 5.6 起 token 可能是 `$wbEncrypted`
/// 信封对象，信封必须**原样**搬运（拆成字符串会静默毁掉登录态）。
struct LiveCredentials {
    /// access token（明文串或加密信封）。
    access_token: Value,
    /// refresh token；live 里没有时保留账号库的原值。
    refresh_token: Option<Value>,
    /// access token 到期时刻（毫秒）。
    expires_at: Option<i64>,
    /// 最近一次续期时刻（毫秒）。
    refreshed_at: Option<i64>,
    /// refresh token 到期时刻（毫秒）。
    refresh_expires_at: Option<i64>,
}

impl LiveCredentials {
    /// 从认证文件根取凭据；连 access token 都取不到时返回 `None`（没什么可 adopt 的）。
    fn from_auth_file(root: &Value) -> Option<Self> {
        let auth = root.get("auth");
        let field = |key: &str| {
            auth.and_then(|inner| secret_value(inner, key))
                .or_else(|| secret_value(root, key))
        };
        let access_token = field("accessToken").or_else(|| field("access_token"))?;
        let refresh_token = field("refreshToken").or_else(|| field("refresh_token"));
        let freshness = auth_file::CredentialFreshness::of_auth_file(root);
        let refresh_expires_at = auth
            .and_then(|inner| norm_ts(inner.get("refreshExpiresAt")))
            .or_else(|| norm_ts(root.get("refreshExpiresAt")));
        Some(Self {
            access_token,
            refresh_token,
            expires_at: freshness.expires_at(),
            refreshed_at: freshness.refreshed_at(),
            refresh_expires_at,
        })
    }

    /// live 的凭据与账号库里的是否**不同**（adopt 的前提之一）。
    ///
    /// 只有一项也不差时不 adopt：没有信息可搬，多写一次账号库只会制造噪声。
    fn differs_from(&self, account: &Value) -> bool {
        let stored_access_token = secret_value(account, "access_token");
        if stored_access_token.as_ref() != Some(&self.access_token) {
            return true;
        }
        match (
            self.refresh_token.as_ref(),
            secret_value(account, "refresh_token"),
        ) {
            (Some(live), Some(stored)) => live != &stored,
            // live 有、库里没有 → 不同；live 没有 → 不算差异（保留库内原值）。
            (Some(_), None) => true,
            (None, _) => false,
        }
    }
}

/// 抢到 `credentials.lock` 之后、构造刷新请求之前的判定结果。
enum LiveAction {
    /// 用 live 快照回填账号库：**不发刷新请求、不写 live 文件**。
    Adopt(LiveCredentials),
    /// live 文件存在但读不出来：本次跳过（不改库、不发请求、不写 `needs_relogin`）。
    SkipUnreadable,
    /// live 是加密信封、账号库里的明文仍有效：**保留明文**，不回填、不发请求、不落盘（R6-01）。
    KeepPlaintext,
    /// live 是加密信封且不该刷新（来源非 `oauth`，或库内已经是信封）：不发请求、不改库（R6-01）。
    SkipEncrypted,
    /// 与改造前一致：照常刷新。
    RefreshAsBefore,
}

/// 判定本次该 adopt / 保留明文 / 跳过 / 照旧刷新（整段判定必须在锁内做，理由见调用点）。
///
/// # 信封闸门的位置（R6-01，不得后移）
///
/// 旧实现只判「同 uid + 值不同 + live 更新」就 adopt，于是 `$wbEncrypted` 信封被当成
/// 「更新的凭据」回填账号库、覆盖掉 OAuth 明文。信封判定必须排在 `differs_from`
/// **之前**：信封与明文字符串必然「不同」，也往往「更新」，走到后面就已经来不及了。
fn decide_live_action(account: &Value, variant: WbVariant) -> LiveAction {
    let live = match read_live_auth_state(variant) {
        // 没有 live 文件 = 该档位没有登录的官方客户端 = 没有「当前账号」。
        LiveAuthState::Absent => return LiveAction::RefreshAsBefore,
        LiveAuthState::Unreadable => return LiveAction::SkipUnreadable,
        LiveAuthState::Parsed(root) => root,
    };
    // 身份口径与切号写回同源（`auth_file::same_account`）：两处若各写一份，
    // 迟早会出现「切号认为同一个账号、刷新认为不是」的分裂。
    if !auth_file::same_account(account, &live) {
        return LiveAction::RefreshAsBefore;
    }
    let Some(credentials) = LiveCredentials::from_auth_file(&live) else {
        return LiveAction::RefreshAsBefore;
    };
    // ── 信封闸门（R6-01）：优先级高于「不同 / 更新」两项判据 ─────────────────
    // `credentials.access_token` 已由 `from_auth_file` 覆盖 `auth.accessToken` 与顶层
    // `accessToken` 两个根，因此这里判一次就够。
    if is_envelope_value(&credentials.access_token) {
        if is_envelope(account, "access_token") {
            // 库里已经是信封：再写一次没有任何信息增量，只会白白重排 JSON。
            return LiveAction::SkipEncrypted;
        }
        if has_unexpired_plain_token(account) {
            // 明文还能用：信封一点价值都没有（还要靠客户端 keyblob 才能解），直接保留明文。
            return LiveAction::KeepPlaintext;
        }
        // 明文已过期 / 没有明文：只有 OAuth 独立会话允许照常刷新；`local` / `import` /
        // 判不出一律跳过——拿库里的旧 refresh token 去刷会把客户端手里的令牌轮换掉。
        return match credential_source(account) {
            source if source.may_refresh_on_envelope() => LiveAction::RefreshAsBefore,
            _ => LiveAction::SkipEncrypted,
        };
    }
    if !credentials.differs_from(account) {
        return LiveAction::RefreshAsBefore;
    }
    // 「live 确实更新」只认两个时间戳（`lastRefreshTime` / `expiresAt`），判据与
    // 切号写回的新鲜度校验完全同源：任一侧缺值不比较，相等不算新。
    if auth_file::CredentialFreshness::of_auth_file(&live)
        .newer_than(&auth_file::CredentialFreshness::of_account(account))
        .is_none()
    {
        return LiveAction::RefreshAsBefore;
    }
    LiveAction::Adopt(credentials)
}

/// 把 live 快照回填到账号记录（纯内存；落盘由调用方 `upsert_account` 负责）。
///
/// # 防御纵深（R6-01）
///
/// 入口再判一次「live 是信封、账号是明文字符串 → 一个字段都不写」。`decide_live_action`
/// 已经挡住了这条路，这里是第二道闸：将来若有新调用点（例如 R6-06 的回填写回）绕过判定
/// 直接调本函数，也不会把明文覆盖成信封。
fn apply_live_credentials(account: &mut Value, credentials: &LiveCredentials) {
    let stored_plain = match account.get("access_token") {
        Some(Value::String(text)) => !text.trim().is_empty(),
        _ => false,
    };
    if is_envelope_value(&credentials.access_token) && stored_plain {
        eprintln!("[refresh] 拒绝用加密信封覆盖账号库明文（防御分支，未写入任何字段）");
        return;
    }
    account["access_token"] = credentials.access_token.clone();
    if let Some(refresh_token) = &credentials.refresh_token {
        account["refresh_token"] = refresh_token.clone();
    }
    if let Some(expires_at) = credentials.expires_at {
        account["expiresAt"] = json!(expires_at);
    }
    if let Some(refreshed_at) = credentials.refreshed_at {
        account["refreshedAt"] = json!(refreshed_at);
    }
    if let Some(refresh_expires_at) = credentials.refresh_expires_at {
        account["refreshExpiresAt"] = json!(refresh_expires_at);
    }
    // 官方客户端用这份凭据续期成功（live 比库内更新就是证据），所以「需重新登录」
    // 与暂时失败诊断都已经过时：与真正续期成功同一语义，一起清掉。
    // 这是 [`clear_relogin_marker`] 的第二个调用点，判据仍然是
    // 「凭据已被证明续期过」，没有放宽「只有续期成功才清标记」的约束。
    clear_relogin_marker(account);
}

/// 刷新单账号 token（POST /v2/plugin/auth/token/refresh），成功则落盘并返回新账号。
///
/// 失败按 [`classify_refresh_failure`] 分级：
/// - `Revoked` / 缺 refresh token → 写 `needs_relogin`（确实无法续期）；
/// - `Temporary` / `Unknown` → **不写标记**，只写 `refresh_failed_at` /
///   `refresh_failed_reason`，保留原凭据等下一次重试。
///
/// 成功是**唯一**清除标记的地方（`needs_relogin` / reason / since / 退避计数一起清）；
/// 唯一的另一种「凭据被证明续期过」是 adopt（见下），它清的是同一批字段。
///
/// # 当前账号不再被抢刷（T01）+ 信封分流（R6-01/R6-02）
///
/// 入口先过两道「不刷新」闸门（都在抢锁之前，因为它们既不发请求也不改凭据）：
/// 1. `__at_rest` 临时副本（R6-06）→ 直接返回，outcome `skipped_at_rest`；
/// 2. 凭据是加密信封（`access_token` 或 `refresh_token`）→ 直接返回，outcome
///    `skipped_encrypted`；顺带惰性清理 R6-02 的历史误标。
///
/// 然后抢 `credentials.lock` 并判定 live 认证文件（[`decide_live_action`]）：
/// - live 是明文且更新 → **adopt**：只回填账号库，不发请求、不写 live；
/// - live 是信封且库内明文仍有效 → **保留明文**（`kept_plaintext`）；
/// - live 是信封且不该刷新 → **跳过**（`skipped_encrypted`）；
/// - live 存在但读不出来 → **跳过本次**（不落盘、不写标记）；
/// - 其它情况 → 与改造前逐字一致，照常刷新。
///
/// # 凭据写入互斥（issue #202）
///
/// 整个「读快照 → 打刷新接口 → 写回账号库」都在 `locks/credentials.lock` 里完成，
/// 与切号写回共用同一把锁。两条硬约束：
///
/// 1. **必须在发出刷新请求之前抢到锁**。刷新会轮换 refresh token：若先打接口再抢锁，
///    抢不到就只能丢弃新令牌，而服务端已经把旧令牌作废 —— 那等于把账号写成
///    「需重新登录」，比原来那个 bug 更糟；
/// 2. **抢不到就立即失败**（不等待、不降级继续）：不调用 `upsert_account`、不写
///    `needs_relogin`，只在返回值里放一个内存标记供保活/复核判定为「跳过」。
pub async fn refresh_account_token(mut account: Value) -> Value {
    // ⓪ R6-06 契约：解密出的临时副本绝刷新（会轮换掉客户端手里的令牌）。
    if is_at_rest_temp(&account) {
        account[REFRESH_OUTCOME_FIELD] = json!(REFRESH_OUTCOME_SKIPPED_AT_REST);
        return account;
    }
    // ⓪' R6-02：信封凭据不是「缺少 refresh token」。必须在下面 `rt.is_empty()` 之前判，
    // 否则信封的 refresh_token（对象）取 `as_str()` 得空串，会走进「缺 RT」分支误标重登。
    if has_envelope_credential(&account) {
        return skip_encrypted_credential(account);
    }
    let previous_access_token = account
        .get("access_token")
        .and_then(|value| value.as_str())
        .map(str::to_string);
    let rt = account
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_default();
    if rt.is_empty() {
        // 没有 refresh token 属于「确实无法续期」，按吊销处理。
        //
        // 这里是凭据写入互斥的**明确豁免**：不持 `credentials.lock` 就 `upsert_account`。
        // 与文件头「两个写入入口必须共用同一把锁」的差别有两点：
        // ① 那把锁要防的是「服务端已经把 refresh token 轮换掉、盘上还是旧的」这种双写竞态，
        //    没有 refresh token 连刷新请求都发不出去，根本不存在「已被轮换的新令牌被丢弃」，
        //    这里写下的只是一条「需重新登录」标记 + 未改动的原凭据；
        // ② `upsert_account` 是「重新 load 全量账号库 → 只替换本账号 → 整文件原子写」的读改写，
        //    不是把整份账号库覆盖成手里的旧快照，因此不会像主路径那样用过期快照盖掉别人的写入。
        // 主路径（下面抢锁那段）之所以必须持锁，是因为它中间要 await 一次会轮换令牌的 HTTP 请求。
        // 走到这里已经排除了信封（上面 ⓪'），所以这条标记是真的「RT 被删/被吊销」。
        mark_relogin_required(&mut account, MISLABELED_RELOGIN_REASONS[0].to_string());
        let _ = upsert_account(&account);
        credential_events::record_for_account(
            &account,
            ACTION_REFRESH_FAILED,
            REASON_MISSING_REFRESH_TOKEN,
        );
        return account;
    }

    // ① 先抢锁：抢不到时一个字都还没写、接口也还没打（见上面的两条硬约束）。
    let lock = match auth_file::try_acquire_credential_write_lock() {
        Ok(lock) => lock,
        Err(reason) => return refresh_skipped_by_credential_lock(account, reason),
    };

    // ② 锁内重读 live 认证文件，先判定「这是不是当前登录的账号」。
    //
    // 必须在锁内、且必须在构造请求之前：`credentials.lock` 是「谁在改本机凭据」的唯一
    // 互斥，锁外读到的新鲜度可能在这之后被一次切号改掉；而一旦发出刷新请求，服务端就会
    // 轮换 refresh token——那条路不能回头。
    match decide_live_action(&account, crate::modules::account::variant_of(&account)) {
        LiveAction::Adopt(credentials) => {
            apply_live_credentials(&mut account, &credentials);
            let _ = upsert_account(&account);
            // 与刷新成功路径对齐：账号库落盘后把新 access 同步给 CLI settings。
            //
            // 改造前当前账号一定会走真刷新，末尾（见函数尾部）会把新 access token 写进
            // CLI settings env；adopt 提前 return 之后那段不再执行，若不同步，CLI 会一直
            // 停在旧 access token 上，直到下一次切号或真刷新。
            //
            // 位置必须在 `drop(lock)` 之后：持 `credentials.lock` 期间不得再取 CLI 侧的
            // 锁（`ensure_no_process_env_override` / settings 读改写会取另一把），
            // 否则与切号写回形成锁序反转。诊断字段仍在同步之后写，`account` 不能提前 move。
            drop(lock);
            if cfg!(windows) {
                let _ = crate::modules::codebuddy_cli::sync_windows_env_for_account(
                    &account,
                    previous_access_token.as_deref(),
                );
            }
            // 诊断字段在落盘之后插入：它只属于本次调用，永远不进账号库。
            account[REFRESH_OUTCOME_FIELD] = json!(REFRESH_OUTCOME_ADOPTED);
            eprintln!("[refresh] 采用官方客户端的新凭据：未发刷新请求、未写认证文件");
            credential_events::record_for_account(&account, ACTION_ADOPTED, REASON_OK);
            return account;
        }
        LiveAction::SkipUnreadable => {
            // 不 `upsert_account`、不写 `needs_relogin`、不发请求：文件正在被写时，
            // 任何持久化结论都可能是错的；本次凭据仍可用，等下一次保活再看。
            drop(lock);
            account[REFRESH_OUTCOME_FIELD] = json!(REFRESH_OUTCOME_SKIPPED_LIVE_UNREADABLE);
            eprintln!("[refresh] 跳过刷新：认证文件存在但无法解析（可能正在被客户端写入）");
            return account;
        }
        LiveAction::KeepPlaintext => {
            // R6-01：live 是信封、库内明文仍有效 → 明文比信封值钱（信封还要客户端 keyblob
            // 才能解）。不 `upsert_account`（否则会把 adopt 的旧行为又带回来）、不写
            // `needs_relogin`、不发请求；只留一个内存诊断值供保活与诊断包区分。
            drop(lock);
            account[REFRESH_OUTCOME_FIELD] = json!(REFRESH_OUTCOME_KEPT_PLAINTEXT);
            eprintln!("[refresh] 保留账号库明文：官方客户端已换用加密信封凭据");
            credential_events::record_for_account(&account, ACTION_KEPT_PLAINTEXT, REASON_OK);
            return account;
        }
        LiveAction::SkipEncrypted => {
            // R6-01：live 是信封且不该刷新（来源非 oauth / 库内已是信封）。
            // 与 KeepPlaintext 同一处置：不发请求、不改库、不写标记。
            drop(lock);
            account[REFRESH_OUTCOME_FIELD] = json!(REFRESH_OUTCOME_SKIPPED_ENCRYPTED);
            eprintln!("[refresh] 跳过刷新：凭据为客户端加密信封态（不改库、不发请求）");
            credential_events::record_for_account(&account, ACTION_SKIPPED_ENCRYPTED, REASON_OK);
            return account;
        }
        LiveAction::RefreshAsBefore => {}
    }

    let headers = refresh_headers(&account, &rt);
    // 刷新必须走账号自身档位的域名：把国际版 token 打到国内网关等于登录失效。
    let url = refresh_url(&account);
    // 单测据这个计数器断言「adopt 时 0 次刷新请求」（生产不编译）。
    #[cfg(test)]
    REFRESH_HTTP_CALLS.fetch_add(1, Ordering::SeqCst);
    let resp = http_request(&url, "POST", Some(json!({})), Some(&headers)).await;
    let recovered = apply_refresh_response(&mut account, &resp);
    let _ = upsert_account(&account);
    // 「是否需要用户处置」由分级函数直接给出，不看标记是否存在：账号可能本来就带着上一次
    // 的标记，用「有没有标记」判断会把一次断网记成 `refresh_failed`。
    let revoked = !recovered && classify_refresh_failure(&resp).marks_relogin();
    // 诊断字段同样在落盘之后插入（生产路径的取值见 `REFRESH_OUTCOME_REFRESHED`）。
    account[REFRESH_OUTCOME_FIELD] = json!(REFRESH_OUTCOME_REFRESHED);
    // ③ 账号库已经落盘，临界区到此结束；后面的 CLI 配置同步与凭据无关，不必持锁。
    drop(lock);
    // 凭据事件（R6-01/R6-03 契约）在锁外记：临界区只该覆盖「写凭据」，
    // 不要把另一个文件的读改写也塞进 `credentials.lock`。
    // - 成功 → `refreshed/ok`；
    // - **只**给「需要用户处置」的失败记 `refresh_failed/invalid_grant`（吊销）；
    //   断网 / 5xx / 429 这类**暂时**失败**刻意不记**：保活每天都会跑，暂时失败会把
    //   200 条环形窗口刷满，反而把真正的吊销挤出去（诊断包另有 `[刷新与保活]` 计数）。
    if recovered {
        credential_events::record_for_account(&account, ACTION_REFRESHED, REASON_OK);
    } else if revoked {
        credential_events::record_for_account(
            &account,
            ACTION_REFRESH_FAILED,
            REASON_INVALID_GRANT,
        );
    }
    // Windows 不执行 apiKeyHelper；当前 CLI 账号刷新后同步 settings env。
    // 同步失败不阻断 WorkBuddy 保活；状态接口会根据 settings 与账号库是否
    // 一致显示“待同步”，避免把认证配置错误混入账号数据。
    // 只在续期成功时同步：暂时失败时 token 没变，同步是纯噪声。
    if recovered && cfg!(windows) {
        let _ = crate::modules::codebuddy_cli::sync_windows_env_for_account(
            &account,
            previous_access_token.as_deref(),
        );
    }
    account
}

/// 抢不到凭据写入锁时的返回：**不落盘、不写 `needs_relogin`**，只放内存诊断标记。
///
/// 刻意不调用 `upsert_account`：抢不到锁说明另有写入正在进行，任何读改写都可能拿旧快照
/// 覆盖它的结果 —— 正是 issue #202 的原始故障。
fn refresh_skipped_by_credential_lock(mut account: Value, reason: String) -> Value {
    eprintln!("[refresh] 跳过刷新：{reason}");
    account[REFRESH_SKIPPED_FIELD] = json!(REFRESH_SKIPPED_LOCK_BUSY);
    // 与「暂时失败」同形：宿主据此提示「稍后重试」，且不写任何持久化字段。
    account["refresh_failed_at"] = json!(now_ms());
    account["refresh_failed_reason"] = json!(reason);
    // 诊断取值：保活据此报 `skipped`（而不是 `temporary_failed`——账号本身没失败）。
    account[REFRESH_OUTCOME_FIELD] = json!(REFRESH_OUTCOME_SKIPPED_CREDENTIAL_LOCK);
    account
}

/// 把一次刷新响应**应用到账号对象**上（纯逻辑，不做 IO，便于单测）。
///
/// 这是分级与标记的唯一落点，三条路径：
/// - 成功（`code` 为 0/200 且带 `accessToken`）→ 更新凭据 + 清除标记，返回 `true`；
/// - 失败 → [`classify_refresh_failure`] 分级：`Revoked` 写标记，其余只写诊断字段，
///   返回 `false`；
/// - `code` 是成功码但缺 `accessToken` → 无法确认凭据状态，按暂时失败处理（不写标记），
///   返回 `false`。
///
/// 返回值是本函数唯一表示「续期成功」的信号，调用方用它决定是否同步 CLI 配置。
fn apply_refresh_response(account: &mut Value, resp: &Value) -> bool {
    let code = response_code(resp).unwrap_or(-1);
    if code != 0 && code != 200 {
        let failure = classify_refresh_failure(resp);
        let reason = refresh_failure_reason(failure, resp);
        if failure.marks_relogin() {
            mark_relogin_required(account, reason);
        } else {
            // 断网 / 代理 / 5xx / 429 等：原标记保持原样，只记诊断字段。
            mark_temporary_failure(account, reason);
        }
        return false;
    }

    let data = resp.get("data").cloned().unwrap_or_else(|| json!({}));
    let new_at = data
        .get("accessToken")
        .and_then(|v| v.as_str())
        .or_else(|| data.get("access_token").and_then(|v| v.as_str()))
        .map(|s| s.to_string());
    let Some(new_at) = new_at else {
        // 响应缺字段：无法确认凭据状态（可能是网关裁剪/错误页），按暂时失败处理，
        // 绝不给账号标「需重新登录」。
        mark_temporary_failure(
            account,
            format!("刷新响应缺少 accessToken（{TEMPORARY_HINT}）"),
        );
        return false;
    };

    account["access_token"] = json!(new_at);
    if let Some(new_rt) = data
        .get("refreshToken")
        .and_then(|v| v.as_str())
        .or_else(|| data.get("refresh_token").and_then(|v| v.as_str()))
    {
        account["refresh_token"] = json!(new_rt);
    }
    // 官方接口只返回相对 expiresIn（秒），需换算为绝对时间戳
    let new_exp = norm_ts(data.get("expiresAt").or_else(|| data.get("expires_at")));
    let new_exp = match new_exp {
        Some(v) => Some(v),
        None => data
            .get("expiresIn")
            .and_then(|v| v.as_i64())
            .map(|e| now_ms() + e * 1000),
    };
    if let Some(v) = new_exp {
        account["expiresAt"] = json!(v);
    }
    let fallback_rt_exp = norm_ts(
        account
            .get("auth_raw")
            .and_then(|a| a.get("refreshExpiresAt")),
    );
    let mut new_rt_exp = norm_ts(
        data.get("refreshExpiresAt")
            .or_else(|| data.get("refresh_expires_at")),
    );
    if new_rt_exp.is_none() {
        new_rt_exp = fallback_rt_exp;
    }
    let new_rt_exp = match new_rt_exp {
        Some(v) => Some(v),
        None => data
            .get("refreshExpiresIn")
            .and_then(|v| v.as_i64())
            .map(|e| now_ms() + e * 1000),
    };
    if let Some(v) = new_rt_exp {
        account["refreshExpiresAt"] = json!(v);
    }
    account["refreshedAt"] = json!(now_ms());
    // 唯一清除点：续期成功才清「需重新登录」及全部刷新诊断字段。
    clear_relogin_marker(account);
    true
}

/// 惰性刷新：expiresAt 缺失或剩余 < lazy_refresh_hours 则刷新。返回最新账号。
///
/// R5-08：带「需重新登录」标记的账号在这里顺带触发一次侧路复核。选这里作为
/// 触发点的原因：积分查询（`credits::http::authenticated_post`）、签到轮次、
/// 旅行轮次都要先过这道闸，覆盖面比逐个在成功分支加钩子更广，而且不需要改动
/// 那些模块。复核是 `spawn` 出去的，本次请求的时延与返回值都不受影响。
///
/// **判定顺序很重要**：先算出「本次会不会内联刷新」。会内联刷新时**不再** spawn
/// 复核——那次内联刷新本身就是一次复核（成功清标记 / 吊销写标记），再 spawn 一次
/// 就等于对同一账号并发打两次 `/auth/token/refresh`；官方刷新带 refresh token
/// 轮换，并发刷新会互相把对方的 refresh token 作废，反而制造「需重新登录」。
pub async fn ensure_fresh_token(mut account: Value, cfg: &Value) -> Value {
    // R6-06 契约：解密出的临时副本（`__at_rest`）**绝不刷新**。
    // 它的 refresh token 是客户端手里那份的副本，用它刷新会把客户端的令牌轮换掉、
    // 让用户掉登录。必须在函数入口第一步返回：后面的 `stale` 判定与旁路复核都不能跑。
    if is_at_rest_temp(&account) {
        account[REFRESH_OUTCOME_FIELD] = json!(REFRESH_OUTCOME_SKIPPED_AT_REST);
        return account;
    }
    let lazy_h = cfg
        .get("lazy_refresh_hours")
        .and_then(|v| v.as_i64())
        .unwrap_or(24);
    let exp = account.get("expiresAt").and_then(|v| v.as_i64());
    let stale = match exp {
        Some(e) => now_ms() >= e || e - now_ms() < lazy_h * 3600 * 1000,
        None => true,
    };
    let has_rt = !account
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .is_empty();
    let will_refresh = stale && has_rt;
    // 只有「本次不会内联刷新」且「确实有 refresh token 可试」时才旁路复核：
    // 没有 refresh token 的账号刷新必然失败，spawn 出去也只是白读一次账号库。
    if should_spawn_verify(stale, has_rt, has_relogin_marker(&account)) {
        if let Some(account_id) = account_identity(&account) {
            spawn_verify_relogin(account_id);
        }
    }
    if will_refresh {
        account = refresh_account_token(account).await;
    }
    account
}

/// `ensure_fresh_token` 是否应当旁路复核（纯函数，便于单测固定「内联刷新时不 spawn」）。
///
/// - `stale`：本次会不会内联刷新（token 过期 / 临期 / 缺 expiresAt）；
/// - `has_rt`：有 refresh token 可试；
/// - `marked`：带「需重新登录」标记。
///
/// 三条同时成立才复核：会内联刷新的账号由那次刷新自己完成复核，重复 spawn 会让
/// 同一账号并发打两次刷新接口，而 refresh token 轮换会因此互相作废。
fn should_spawn_verify(stale: bool, has_rt: bool, marked: bool) -> bool {
    !stale && has_rt && marked
}

/// 保活检查：每天由后台循环调用一次，默认（keepalive_days <= 0）无条件刷新
/// 全部带 refresh token 的账号；keepalive_days > 0 时仅刷新剩余不足该天数的账号。
///
/// 高频保活是为了避免官方服务端清理闲置的 refresh 会话——曾出现闲置数天后
/// 刷新返回 12153 invalid_grant（Session doesn't have required client）导致
/// 账号被迫重新登录。
///
/// 每个账号的结果分三档：`ok` / `failed`（吊销，需重新登录）/ `temporary_failed`
/// （断网、5xx 等，稍后自动重试，**不算**账号失效）。T01 之后另加几档，形状不变：
/// - `adopted`：当前登录账号的 live 凭据更新，已回填账号库（**没有**打刷新接口）；
/// - `kept_plaintext`：live 是加密信封、库内明文仍有效，保留明文（R6-01）；
/// - `skipped`：本次没刷新（认证文件正在被写 / 抢不到 `credentials.lock` /
///   凭据是加密信封）。**不是失败**：信封账号计入这一档，不计 `failed`（R6-02）。
pub async fn run_keepalive_cycle() -> Value {
    let Some(_guard) = RunFlagGuard::try_acquire(&KEEPALIVE_RUNNING) else {
        return json!({"skipped": "already_running"});
    };
    let cfg = load_checkin_config();
    let keep_days = cfg
        .get("keepalive_days")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let accounts = crate::modules::account::load_accounts();
    let total = accounts.len();
    let mut results: Vec<Value> = Vec::new();
    for mut acc in accounts {
        // R6-06 防御：临时副本不该出现在账号库里（正常账号库不会有），万一出现也绝不刷新。
        if is_at_rest_temp(&acc) {
            continue;
        }
        let exp = acc.get("expiresAt").and_then(|v| v.as_i64());
        let stale = keep_days <= 0
            || match exp {
                Some(e) => now_ms() >= e || e - now_ms() < keep_days * 24 * 3600 * 1000,
                None => true,
            };
        if !stale {
            continue;
        }
        // R6-02：信封凭据必须在「缺 refresh token」判空**之前**判掉，否则每次启动/保活
        // 都会给信封账号补一条「缺少 refresh token ⇒ 需重新登录」的误导性标记。
        // 这里计 `skipped`（不是 `failed`）：账号没失效，只是本工具读不出明文。
        if has_envelope_credential(&acc) {
            if clear_mislabeled_relogin(&mut acc) {
                eprintln!("[keepalive] 清理历史误标：信封凭据不再算「缺少 refresh token」");
                let _ = upsert_account(&acc);
            }
            // 同账号同 action 有 60s 节流，保活连跑多轮也不会刷屏。
            credential_events::record_for_account(&acc, ACTION_SKIPPED_ENCRYPTED, REASON_OK);
            results.push(json!({
                "email": crate::modules::account::account_display_name(&acc),
                "status": "skipped",
            }));
            continue;
        }
        if acc
            .get("refresh_token")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .is_empty()
        {
            // 与 `refresh_account_token` 的「缺 refresh token」分支同一处理，同样**不持**
            // 凭据写入锁。豁免理由也相同：没有 refresh token 就没有「服务端已轮换旧令牌」的
            // 竞态，写下的只是「需重新登录」标记，原凭据一个字节不变；而 `upsert_account` 是
            // 重新读全量账号库、只替换本账号的读改写，不是整文件覆盖，不会把别的账号刷回旧快照。
            // 走到这里已经排除了信封（上面那条），所以这条标记是真的「RT 被删/被吊销」。
            mark_relogin_required(&mut acc, MISLABELED_RELOGIN_REASONS[1].to_string());
            let _ = upsert_account(&acc);
            credential_events::record_for_account(
                &acc,
                ACTION_REFRESH_FAILED,
                REASON_MISSING_REFRESH_TOKEN,
            );
            results.push(json!({
                "email": crate::modules::account::account_display_name(&acc),
                "status": "missing_rt",
            }));
            continue;
        }
        let fresh = refresh_account_token(acc).await;
        let failed = has_relogin_marker(&fresh);
        let temporary = !failed
            && fresh
                .get("refresh_failed_at")
                .is_some_and(|value| value.is_i64());
        // 诊断取值优先于 `temporary_failed`：「adopt」与「跳过」都**不是**账号失败，
        // 只是本次没走刷新请求。`refresh_outcome` 只在内存里，读不到就是改造前的路径。
        let outcome = refresh_outcome(&fresh);
        let status = if failed {
            "failed"
        } else if outcome == Some(REFRESH_OUTCOME_ADOPTED) {
            "adopted"
        } else if outcome == Some(REFRESH_OUTCOME_KEPT_PLAINTEXT) {
            "kept_plaintext"
        } else if matches!(
            outcome,
            Some(REFRESH_OUTCOME_SKIPPED_LIVE_UNREADABLE)
                | Some(REFRESH_OUTCOME_SKIPPED_CREDENTIAL_LOCK)
                | Some(REFRESH_OUTCOME_SKIPPED_ENCRYPTED)
                | Some(REFRESH_OUTCOME_SKIPPED_AT_REST)
        ) {
            "skipped"
        } else if temporary {
            "temporary_failed"
        } else {
            "ok"
        };
        let reason = if failed {
            fresh.get("needs_relogin_reason")
        } else {
            fresh.get("refresh_failed_reason")
        };
        results.push(json!({
            "email": crate::modules::account::account_display_name(&fresh),
            "status": status,
            "error": if failed || temporary {
                reason.and_then(|v| v.as_str()).map(|s| s.to_string())
            } else {
                None
            },
        }));
    }
    json!({"checked": total, "refreshed": results})
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::path::PathBuf;
    use std::sync::{Arc, Barrier};

    // -----------------------------------------------------------------------
    // 分级（R5-08 A）
    // -----------------------------------------------------------------------

    /// 12153 / invalid_grant / 401 吊销语义 → Revoked（只有这一类能写标记）。
    #[test]
    fn classify_revoked_failures() {
        let cases = [
            json!({"code": 12153, "message": "invalid_grant: Invalid refresh token"}),
            json!({"code": 12153, "message": "Session doesn't have required client"}),
            json!({"code": 400, "message": "invalid_grant"}),
            json!({"code": 400, "message": "Invalid refresh token"}),
            json!({"code": 400, "message": "refresh token expired"}),
            json!({"code": 401, "message": "refresh token has been revoked"}),
            json!({"code": 403, "message": "refresh_token 已过期"}),
            // 字符串 code 也不能漏（网关偶发把 code 写成字符串）。
            json!({"code": "12153", "message": "invalid_grant"}),
        ];
        for resp in cases {
            assert_eq!(
                classify_refresh_failure(&resp),
                RefreshFailure::Revoked,
                "应当判为吊销：{resp}"
            );
        }
    }

    /// 断网 / 5xx / 429 / 超时 / HTML 错误页 → Temporary（绝不写标记）。
    #[test]
    fn classify_temporary_failures() {
        let cases = [
            // `http_request` 的传输错误约定。
            json!({"code": -1, "message": "error sending request for url (...) <- connection closed before message completed"}),
            json!({"code": -1, "message": "operation timed out"}),
            // 网关/服务端故障。
            json!({"code": 500, "message": "Internal Server Error"}),
            json!({"code": 502, "message": "Bad Gateway"}),
            json!({"code": 503}),
            json!({"code": 429, "message": "Too Many Requests"}),
            // 2xx 但响应体不是 JSON（`http_request` 返回 Null）。
            Value::Null,
            // 网关整页 HTML（401/502 都会这样）——必须优先按 HTML 判掉。
            json!({"code": 401, "message": "服务端返回 HTML 错误页：502 Bad Gateway"}),
            json!({"code": 502, "message": "<html><head><title>502</title></head></html>"}),
            // 缺 code 但文案是传输层错误。
            json!({"message": "dns error: failed to lookup address information"}),
        ];
        for resp in cases {
            assert_eq!(
                classify_refresh_failure(&resp),
                RefreshFailure::Temporary,
                "应当判为暂时失败：{resp}"
            );
        }
    }

    /// 其它业务码（无法判断）→ Unknown：保守按暂时失败处理，不写标记。
    #[test]
    fn classify_unknown_business_codes() {
        let resp = json!({"code": 10085, "message": "客户端指纹校验未通过"});
        let failure = classify_refresh_failure(&resp);
        assert_eq!(failure, RefreshFailure::Unknown);
        assert!(!failure.marks_relogin(), "Unknown 不得写 needs_relogin");
    }

    /// 只有 Revoked 允许写标记。
    #[test]
    fn only_revoked_marks_relogin() {
        assert!(RefreshFailure::Revoked.marks_relogin());
        assert!(!RefreshFailure::Temporary.marks_relogin());
        assert!(!RefreshFailure::Unknown.marks_relogin());
        assert_eq!(RefreshFailure::Temporary.label(), "temporary");
    }

    /// 原因文案必须能看出「续期被拒」与「稍后自动重试」的差别。
    #[test]
    fn reason_text_distinguishes_revoked_and_temporary() {
        let revoked = json!({"code": 12153, "message": "invalid_grant"});
        let text = refresh_failure_reason(classify_refresh_failure(&revoked), &revoked);
        assert!(text.contains("续期被拒"), "{text}");
        assert!(text.contains("12153"), "{text}");

        let offline = json!({"code": -1, "message": "connection refused"});
        let text = refresh_failure_reason(classify_refresh_failure(&offline), &offline);
        assert!(text.contains("暂时失败"), "{text}");
        assert!(text.contains("自动重试"), "{text}");
        assert!(!text.contains("重新登录"), "{text}");
    }

    // -----------------------------------------------------------------------
    // 标记读写：断网不标 / 401 标 / 成功清
    // -----------------------------------------------------------------------

    /// 断网（连接错误）不得写「需重新登录」，只写诊断字段。
    #[test]
    fn offline_failure_does_not_mark_relogin() {
        let mut account = json!({"id": "acc-offline", "access_token": "at", "refresh_token": "rt"});
        let resp = json!({"code": -1, "message": "error sending request <- connection refused"});
        let failure = classify_refresh_failure(&resp);
        assert!(!failure.marks_relogin());
        mark_temporary_failure(&mut account, refresh_failure_reason(failure, &resp));

        assert!(!has_relogin_marker(&account), "断网不得写 needs_relogin");
        assert!(account.get("needs_relogin_reason").is_none());
        assert!(account.get("refresh_failed_at").is_some());
        assert_eq!(
            account.get("refresh_failed_reason").and_then(Value::as_str),
            Some(refresh_failure_reason(failure, &resp).as_str())
        );
    }

    /// 401 吊销语义要写标记，并带上「续期被拒」的原因与起始时间。
    #[test]
    fn revoked_failure_marks_relogin() {
        let mut account = json!({"id": "acc-revoked", "access_token": "at", "refresh_token": "rt"});
        let resp = json!({"code": 12153, "message": "invalid_grant: Invalid refresh token"});
        let failure = classify_refresh_failure(&resp);
        assert!(failure.marks_relogin());
        mark_relogin_required(&mut account, refresh_failure_reason(failure, &resp));

        assert!(has_relogin_marker(&account));
        let reason = account
            .get("needs_relogin_reason")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(reason.contains("续期被拒"), "{reason}");
        assert!(account.get("needs_relogin_since").is_some());
        assert_eq!(
            account
                .get("needs_relogin_fail_count")
                .and_then(Value::as_i64),
            Some(1)
        );
        // 刚失败这一次就是一次「复核」，冷却从这里起算。
        assert!(account.get("needs_relogin_checked_at").is_some());
    }

    /// 已有标记时遇到暂时失败：原标记与原因**保持原样**（不被临时错误覆盖）。
    #[test]
    fn temporary_failure_keeps_existing_marker() {
        let mut account = json!({
            "id": "acc-keep",
            "access_token": "at",
            "refresh_token": "rt",
            "needs_relogin": true,
            "needs_relogin_reason": "续期被拒（code=12153）：invalid_grant",
            "needs_relogin_since": 1_700_000_000_000i64,
        });
        let resp = json!({"code": 502, "message": "Bad Gateway"});
        mark_temporary_failure(
            &mut account,
            refresh_failure_reason(classify_refresh_failure(&resp), &resp),
        );

        assert!(has_relogin_marker(&account));
        assert_eq!(
            account.get("needs_relogin_reason").and_then(Value::as_str),
            Some("续期被拒（code=12153）：invalid_grant"),
            "暂时失败不得覆盖原标记原因"
        );
        assert_eq!(
            account.get("needs_relogin_since").and_then(Value::as_i64),
            Some(1_700_000_000_000i64)
        );
        assert!(account.get("refresh_failed_at").is_some());
    }

    /// 续期成功后标记与全部诊断字段一起清掉，不留旧 reason / 旧起始时间。
    #[test]
    fn success_clears_relogin_marker() {
        let mut account = json!({
            "id": "acc-recovered",
            "access_token": "at",
            "refresh_token": "rt",
            "needs_relogin": true,
            "needs_relogin_reason": "续期被拒（code=12153）：invalid_grant",
            "needs_relogin_since": 1_700_000_000_000i64,
            "needs_relogin_fail_count": 3,
            "needs_relogin_last_result": "failed",
            "refresh_failed_at": 1_700_000_100_000i64,
            "refresh_failed_reason": "续期暂时失败（code=-1）",
        });
        clear_relogin_marker(&mut account);

        assert!(!has_relogin_marker(&account));
        for key in RELOGIN_FIELDS.iter().chain(TEMPORARY_FIELDS.iter()) {
            assert!(account.get(*key).is_none(), "{key} 应当被清除：{account}");
        }
        // 凭据字段不受影响。
        assert_eq!(
            account.get("access_token").and_then(Value::as_str),
            Some("at")
        );
    }

    // -----------------------------------------------------------------------
    // 生产分支：刷新响应落到账号对象上的真实路径（`apply_refresh_response`）
    // -----------------------------------------------------------------------

    /// 断网（连接错误）走真实分支：不写标记、不动凭据、只留诊断字段。
    #[test]
    fn refresh_response_connection_error_does_not_mark_relogin() {
        let mut account = json!({
            "id": "acc-offline-live",
            "access_token": "old-at",
            "refresh_token": "old-rt",
        });
        let resp = json!({
            "code": -1,
            "message": "error sending request for url (...) <- connection refused",
        });

        assert!(
            !apply_refresh_response(&mut account, &resp),
            "断网不算续期成功"
        );

        assert!(!has_relogin_marker(&account), "断网不得写 needs_relogin");
        assert!(
            account.get("needs_relogin_reason").is_none(),
            "断网不得写 needs_relogin_reason"
        );
        // 凭据必须原样保留：临时失败不该让账号不可用。
        assert_eq!(
            account.get("access_token").and_then(Value::as_str),
            Some("old-at")
        );
        assert_eq!(
            account.get("refresh_token").and_then(Value::as_str),
            Some("old-rt")
        );
        // 只留诊断字段。
        assert!(account
            .get("refresh_failed_at")
            .and_then(Value::as_i64)
            .is_some());
        let reason = account
            .get("refresh_failed_reason")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(reason.contains("暂时失败"), "{reason}");
        assert!(
            account.get("refreshedAt").is_none(),
            "失败不得写 refreshedAt"
        );
    }

    /// 401（鉴权拒绝）走真实分支：写标记 + 「续期被拒」原因。
    #[test]
    fn refresh_response_unauthorized_marks_relogin() {
        let mut account = json!({
            "id": "acc-401-live",
            "access_token": "old-at",
            "refresh_token": "old-rt",
        });
        let resp = json!({"code": 401, "message": "Unauthorized"});

        assert!(!apply_refresh_response(&mut account, &resp));

        assert!(has_relogin_marker(&account), "401 必须写 needs_relogin");
        let reason = account
            .get("needs_relogin_reason")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(reason.contains("续期被拒"), "{reason}");
        assert!(account.get("needs_relogin_since").is_some());
        // 冷却起点必须落下，否则复核会立刻重试。
        assert!(account.get("needs_relogin_checked_at").is_some());
        // 吊销时不保留「暂时失败」的诊断字段。
        assert!(account.get("refresh_failed_at").is_none());
    }

    /// 12153 吊销业务码走真实分支：同样写标记。
    #[test]
    fn refresh_response_revoked_code_marks_relogin() {
        let mut account = json!({"id": "acc-12153-live", "refresh_token": "old-rt"});
        let resp = json!({"code": 12153, "message": "invalid_grant: Invalid refresh token"});

        assert!(!apply_refresh_response(&mut account, &resp));
        assert!(has_relogin_marker(&account));
        assert_eq!(
            account
                .get("needs_relogin_fail_count")
                .and_then(Value::as_i64),
            Some(1)
        );
    }

    /// 成功响应走真实分支：换发凭据 + **清除**旧标记，一次全清。
    #[test]
    fn refresh_response_success_clears_stale_marker() {
        let mut account = json!({
            "id": "acc-recover-live",
            "access_token": "old-at",
            "refresh_token": "old-rt",
            "needs_relogin": true,
            "needs_relogin_reason": "续期被拒（code=12153）：invalid_grant",
            "needs_relogin_since": 1_700_000_000_000i64,
            "needs_relogin_fail_count": 3,
            "needs_relogin_checked_at": 1_700_000_600_000i64,
            "needs_relogin_last_result": "failed",
            "refresh_failed_at": 1_700_000_700_000i64,
            "refresh_failed_reason": "续期暂时失败（code=-1）",
        });
        let resp = json!({
            "code": 0,
            "data": {"accessToken": "new-at", "refreshToken": "new-rt", "expiresIn": 7200},
        });
        let before = now_ms();

        assert!(
            apply_refresh_response(&mut account, &resp),
            "成功应返回 true"
        );

        assert!(!has_relogin_marker(&account), "成功必须清除标记");
        for key in RELOGIN_FIELDS.iter().chain(TEMPORARY_FIELDS.iter()) {
            assert!(account.get(*key).is_none(), "{key} 应当被清除：{account}");
        }
        assert_eq!(
            account.get("access_token").and_then(Value::as_str),
            Some("new-at")
        );
        assert_eq!(
            account.get("refresh_token").and_then(Value::as_str),
            Some("new-rt")
        );
        let expires_at = account
            .get("expiresAt")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        assert!(
            expires_at >= before + 7_200_000,
            "expiresIn 应换算成绝对时间戳：{expires_at}"
        );
        assert!(account.get("refreshedAt").and_then(Value::as_i64).is_some());
    }

    /// 成功码但缺 accessToken（网关裁剪 / 错误页）：按暂时失败，绝不写标记。
    #[test]
    fn refresh_response_success_without_access_token_is_temporary() {
        let mut account = json!({
            "id": "acc-hollow",
            "access_token": "old-at",
            "refresh_token": "old-rt",
        });
        let resp = json!({"code": 0, "data": {}});

        assert!(
            !apply_refresh_response(&mut account, &resp),
            "缺 accessToken 不算成功"
        );

        assert!(!has_relogin_marker(&account), "缺字段不得写标记");
        assert_eq!(
            account.get("access_token").and_then(Value::as_str),
            Some("old-at")
        );
        let reason = account
            .get("refresh_failed_reason")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(reason.contains("accessToken"), "{reason}");
    }

    /// 网关 HTML 错误页（502）：暂时失败，不写标记。
    #[test]
    fn refresh_response_html_error_page_is_temporary() {
        let mut account = json!({"id": "acc-html", "refresh_token": "old-rt"});
        let resp = json!({"code": 502, "message": "服务端返回 HTML 错误页：502 Bad Gateway"});

        assert!(!apply_refresh_response(&mut account, &resp));
        assert!(!has_relogin_marker(&account), "网关错误页不得写标记");
    }

    /// 真实链路回归（issue #189）：`http_request` 分类出的 HTML 失败对象进刷新分级后，
    /// 仍然是「暂时失败」——**不是** `Revoked`，且原因文案里没有网页正文。
    ///
    /// 分类器把文案换成了固定短句（不再含 `<`），所以判定必须靠 `failureKind`；
    /// 这条用例钉住「换了文案也没把 401 登录页判成吊销」。
    #[test]
    fn classified_login_page_keeps_temporary_grading() {
        let login_page = crate::modules::config::failure_response(
            401,
            "<!DOCTYPE html><html><body><form id=\"login\">扫码登录</form></body></html>",
        );
        assert!(crate::modules::config::is_html_failure(&login_page));

        let failure = classify_refresh_failure(&login_page);
        assert_eq!(
            failure,
            RefreshFailure::Temporary,
            "401 的登录页是「暂时失败」，不是吊销：{login_page}"
        );
        assert!(!failure.marks_relogin());

        let mut account = json!({"id": "acc-classified-html", "refresh_token": "old-rt"});
        assert!(!apply_refresh_response(&mut account, &login_page));
        assert!(
            !has_relogin_marker(&account),
            "HTML 登录页不得写 needs_relogin"
        );

        let reason = account
            .get("refresh_failed_reason")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(reason.contains("暂时失败"), "{reason}");
        assert!(!reason.contains('<'), "原因文案不得含标签：{reason}");
        assert!(
            !reason.to_ascii_lowercase().contains("<html"),
            "原因文案不得含 <html：{reason}"
        );
    }

    /// 同一套分类器不能把**正常 JSON** 的 401 判丢：吊销依旧是吊销。
    ///
    /// 与上一条用例成对：证明「净化文案」只影响 HTML 分支，业务码分支一个都没动。
    #[test]
    fn classified_json_unauthorized_still_marks_relogin() {
        let json_401 = crate::modules::config::sanitize_error_payload(
            401,
            json!({"code": 401, "message": "Unauthorized"}),
        );
        assert_eq!(
            classify_refresh_failure(&json_401),
            RefreshFailure::Revoked,
            "JSON 里的 401 必须仍然是吊销：{json_401}"
        );

        let mut account = json!({"id": "acc-json-401", "refresh_token": "old-rt"});
        assert!(!apply_refresh_response(&mut account, &json_401));
        assert!(has_relogin_marker(&account));
    }

    /// JSON 里夹带整页 HTML（`{"code":401,"message":"<html>…"}`）：按网页处理，
    /// 不写标记，文案换成固定短句。
    #[test]
    fn json_error_carrying_html_stays_temporary() {
        let wrapped = crate::modules::config::sanitize_error_payload(
            401,
            json!({"code": 401, "message": "<html><body>扫码登录</body></html>"}),
        );
        let failure = classify_refresh_failure(&wrapped);
        assert_eq!(
            failure,
            RefreshFailure::Temporary,
            "夹带网页的 401 不得判成吊销：{wrapped}"
        );
    }

    /// 已带标记时又遇暂时失败：标记与原因原样保留（复核失败不能反过来洗白或改写）。
    #[test]
    fn refresh_response_temporary_failure_keeps_existing_marker() {
        let mut account = json!({
            "id": "acc-marked-offline",
            "refresh_token": "old-rt",
            "needs_relogin": true,
            "needs_relogin_reason": "续期被拒（code=12153）：invalid_grant",
            "needs_relogin_fail_count": 2,
        });
        let resp = json!({"code": -1, "message": "connection reset by peer"});

        assert!(!apply_refresh_response(&mut account, &resp));

        assert!(has_relogin_marker(&account));
        assert_eq!(
            account.get("needs_relogin_reason").and_then(Value::as_str),
            Some("续期被拒（code=12153）：invalid_grant"),
            "暂时失败不得覆盖原原因"
        );
        assert_eq!(
            account
                .get("needs_relogin_fail_count")
                .and_then(Value::as_i64),
            Some(2),
            "暂时失败不得累加吊销计数（否则退避会被网络抖动推长）"
        );
    }

    // -----------------------------------------------------------------------
    // 触发点闸门
    // -----------------------------------------------------------------------

    /// 复核判定：无标记 → 跳过；冷却中 → 跳过；过冷却 → 复核。
    #[test]
    fn verify_plan_picks_skip_reasons() {
        let now = 1_700_000_000_000i64;
        assert_eq!(
            verify_plan(&json!({"id": "clean"}), now),
            VerifyPlan::SkipNoMarker
        );
        assert_eq!(
            verify_plan(
                &json!({"id": "cooling", "needs_relogin": true, "needs_relogin_checked_at": now - 1_000}),
                now
            ),
            VerifyPlan::SkipCooldown
        );
        assert_eq!(
            verify_plan(
                &json!({"id": "ready", "needs_relogin": true, "needs_relogin_checked_at": now - 60_000}),
                now
            ),
            VerifyPlan::Proceed
        );
        // 历史标记（无 checked_at）→ 立即复核。
        assert_eq!(
            verify_plan(&json!({"id": "legacy", "needs_relogin": true}), now),
            VerifyPlan::Proceed
        );
    }

    /// 触发闸门：会内联刷新的账号**不再** spawn 复核（否则同账号并发两次刷新）。
    #[test]
    fn verify_is_not_spawned_when_inline_refresh_runs() {
        // stale=true 且带标记 → 内联刷新自己就是复核，不 spawn。
        assert!(!should_spawn_verify(true, true, true));
        // stale=true 但没有 RT → 内联也不会刷（无 RT 分支只写标记），同样不 spawn。
        assert!(!should_spawn_verify(true, false, true));
        // 不 stale：token 还早，靠旁路复核自愈。
        assert!(should_spawn_verify(false, true, true));
        // 不 stale 但没 RT：刷新必然失败，白读账号库。
        assert!(!should_spawn_verify(false, false, true));
        // 正常账号零额外请求。
        assert!(!should_spawn_verify(false, true, false));
    }

    /// 标记字段**不得**进入「本地字段继承白名单」。
    ///
    /// `account::LOCAL_ACCOUNT_FIELDS` 的语义是「`incoming` 缺该键时用旧值补回」
    /// （`inherit_local_fields`）。而续期成功清除标记靠的是**删键**，一旦把
    /// `needs_relogin` 之类加进白名单，删掉的键会被旧值原样补回 —— 标记复活，
    /// 上游 issue #190 会静默回归，且没有任何编译或运行期报错。这条用例把它变成红灯。
    #[test]
    fn marker_fields_are_not_local_account_fields() {
        for key in RELOGIN_FIELDS.iter().chain(TEMPORARY_FIELDS.iter()) {
            assert!(
                !crate::modules::account::LOCAL_ACCOUNT_FIELDS.contains(key),
                "`{key}` 不得进入 LOCAL_ACCOUNT_FIELDS：刷新成功靠删键清标记，\
                 白名单会把旧标记复活（issue #190 回归）"
            );
        }
        // 反证：白名单确实只装「展示类」字段，标记类字段一律不在此列。
        assert!(
            !crate::modules::account::LOCAL_ACCOUNT_FIELDS.is_empty(),
            "白名单为空说明已被清空，本用例失去意义，需同步调整"
        );
        // T01 的诊断字段同理：它只属于本次调用，进白名单就会被旧值补回、长期沉在盘上。
        assert!(
            !crate::modules::account::LOCAL_ACCOUNT_FIELDS.contains(&REFRESH_OUTCOME_FIELD),
            "`{REFRESH_OUTCOME_FIELD}` 不得进入 LOCAL_ACCOUNT_FIELDS"
        );
    }

    // -----------------------------------------------------------------------
    // 冷却 / 退避（R5-08 B）
    // -----------------------------------------------------------------------

    /// 60s 内不重复复核；过 60s 才允许；没有标记的账号永远不触发。
    #[test]
    fn verify_respects_sixty_second_cooldown() {
        let now = 1_700_000_000_000i64;
        let marked = |checked_at: Option<i64>| {
            let mut account = json!({"id": "acc-cd", "needs_relogin": true});
            if let Some(value) = checked_at {
                account["needs_relogin_checked_at"] = json!(value);
            }
            account
        };

        // 刚复核过（30s 前）→ 冷却中，不打接口。
        assert!(!should_verify_relogin(&marked(Some(now - 30_000)), now));
        // 边界：正好 60s → 允许。
        assert!(should_verify_relogin(&marked(Some(now - 60_000)), now));
        // 61s 前 → 允许。
        assert!(should_verify_relogin(&marked(Some(now - 61_000)), now));
        // 历史标记没有 checked_at → 视为从未复核，立即复核。
        assert!(should_verify_relogin(&marked(None), now));
        // 没有标记 → 永不触发（正常账号零额外请求）。
        assert!(!should_verify_relogin(
            &json!({"id": "acc-clean", "needs_relogin_checked_at": 0}),
            now
        ));
    }

    /// 退避阶梯：60s → 5min → 30min → … → 6h 封顶。
    #[test]
    fn verify_backoff_ladder_grows_and_caps() {
        let cooldown = |fail_count: i64| {
            verify_cooldown_ms(&json!({
                "needs_relogin": true,
                "needs_relogin_fail_count": fail_count,
            }))
        };
        assert_eq!(cooldown(0), 60_000, "无计数（历史标记）按首次冷却");
        assert_eq!(cooldown(1), 60_000);
        assert_eq!(cooldown(2), 300_000);
        assert_eq!(cooldown(3), 1_800_000);
        assert_eq!(cooldown(4), 3_600_000);
        assert_eq!(cooldown(99), 21_600_000, "上限 6h");
    }

    /// 退避生效在 `should_verify_relogin` 上：连续失败后 1 分钟内不再复核。
    #[test]
    fn cooldown_follows_backoff_after_repeated_failures() {
        let now = 1_700_000_000_000i64;
        let account = json!({
            "id": "acc-backoff",
            "needs_relogin": true,
            "needs_relogin_fail_count": 2,
            "needs_relogin_checked_at": now - 60_000,
        });
        // 第 2 次失败后的冷却窗口是 5min：过 60s 仍不复核。
        assert!(!should_verify_relogin(&account, now));
        assert!(should_verify_relogin(&account, now + 240_000));
    }

    // -----------------------------------------------------------------------
    // 并发单飞（R5-08 B）
    // -----------------------------------------------------------------------

    /// 两个并发复核只有一个拿到位（= 只打一次刷新接口）。
    #[test]
    fn verify_slot_is_single_flight() {
        let account_id = format!("acc-single-flight-{}", uuid::Uuid::new_v4());
        let barrier = Arc::new(Barrier::new(2));
        let (tx, rx) = std::sync::mpsc::channel();
        let mut handles = Vec::new();
        for _ in 0..2 {
            let account_id = account_id.clone();
            let barrier = Arc::clone(&barrier);
            let tx = tx.clone();
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                let slot = try_acquire_verify_slot(&account_id);
                let _ = tx.send(slot.is_some());
                // 两个线程都拿到结果之后再释放，保证第二条一定撞上在飞集合。
                std::thread::sleep(std::time::Duration::from_millis(80));
                drop(slot);
            }));
        }
        drop(tx);
        let acquired: Vec<bool> = rx.iter().collect();
        for handle in handles {
            let _ = handle.join();
        }
        assert_eq!(
            acquired.iter().filter(|value| **value).count(),
            1,
            "同账号并发复核只允许一次拿到位：{acquired:?}"
        );
        // 释放后可以再次复核（不会永久卡死）。
        let slot = try_acquire_verify_slot(&account_id);
        assert!(slot.is_some(), "释放后应当能重新占用");
    }

    /// 令牌 Drop 即释放：异常路径（提前 return）不会把账号永久锁在「在飞」。
    #[test]
    fn verify_slot_released_on_drop() {
        let account_id = format!("acc-slot-drop-{}", uuid::Uuid::new_v4());
        {
            let slot = try_acquire_verify_slot(&account_id);
            assert!(slot.is_some());
            assert!(
                try_acquire_verify_slot(&account_id).is_none(),
                "占位期间第二次获取必须失败"
            );
        }
        assert!(
            try_acquire_verify_slot(&account_id).is_some(),
            "Drop 后必须释放"
        );
    }

    // -----------------------------------------------------------------------
    // 保留原有断言
    // -----------------------------------------------------------------------

    #[test]
    fn refresh_url_follows_account_variant() {
        // 旧账号（无档位字段）→ 国内版，与改造前逐字一致。
        assert_eq!(
            refresh_url(&json!({"uid": "u-1"})),
            format!(
                "{}/v2/plugin/auth/token/refresh",
                crate::modules::variant::WbVariant::Cn.api_endpoint()
            )
        );
        assert_eq!(
            refresh_url(&json!({"uid": "u-1", "access_token": "t"})),
            "https://www.codebuddy.cn/v2/plugin/auth/token/refresh"
        );
        // 国际版账号 → 国际版域名。
        assert_eq!(
            refresh_url(&json!({"uid": "u-2", "variant": "ai"})),
            format!(
                "{}/v2/plugin/auth/token/refresh",
                crate::modules::variant::WbVariant::Ai.api_endpoint()
            )
        );
        assert_ne!(
            refresh_url(&json!({"variant": "ai"})),
            refresh_url(&json!({"variant": "cn"}))
        );
    }

    /// AC11：刷新请求头必须带 `X-Auth-Refresh-Source: plugin`，两个档位都覆盖。
    ///
    /// 断言为 `Some("plugin")`：头被删掉时为 `None`，用例立即失败，不会被写成恒真。
    #[test]
    fn refresh_headers_always_declare_plugin_refresh_source() {
        let cases = [
            // 国内版：旧账号（无档位字段）与显式档位各一例。
            json!({"uid": "u-cn", "access_token": "at", "refresh_token": "rt"}),
            json!({"uid": "u-cn", "access_token": "at", "variant": "cn"}),
            // 国际版：D1 的失败现场。
            json!({"uid": "u-ai", "access_token": "at", "variant": "ai"}),
        ];
        for account in cases {
            let headers = refresh_headers(&account, "rt-value");
            assert_eq!(
                headers.get("X-Auth-Refresh-Source").map(String::as_str),
                Some("plugin"),
                "刷新请求头缺少 X-Auth-Refresh-Source: plugin（{account}）"
            );
            // 刷新会话头不能被这次改动挤掉。
            assert_eq!(
                headers.get("X-Refresh-Token").map(String::as_str),
                Some("rt-value")
            );
            // 刷新不带 platform 查询参数/头：官方刷新同样不带（research §2 已推翻旧假设）。
            assert!(!headers.contains_key("platform"));
        }
    }

    // -----------------------------------------------------------------------
    // #202：抢不到凭据写入锁时的失败语义
    // -----------------------------------------------------------------------

    /// 抢不到锁：只放内存标记——不写 `needs_relogin`、不动凭据、不写 `refreshedAt`。
    ///
    /// 落盘由 `refresh_account_token` 保证（该分支不调用 `upsert_account`）；这里固定
    /// 返回值本身的形状，避免将来把「跳过」写成「吊销」或「成功」。
    #[test]
    fn credential_lock_busy_skips_refresh_without_marking_relogin() {
        let account = json!({
            "id": "acc-lock-busy",
            "access_token": "old-at",
            "refresh_token": "old-rt",
            "expiresAt": 1_700_000_000_000i64,
        });
        let skipped = refresh_skipped_by_credential_lock(
            account.clone(),
            auth_file::CREDENTIAL_LOCK_BUSY_MESSAGE.to_string(),
        );

        assert!(
            refresh_was_skipped(&skipped),
            "必须留下「本次没有刷新」的标记"
        );
        assert!(
            !has_relogin_marker(&skipped),
            "抢不到锁不是「需重新登录」：账号本身没被吊销"
        );
        // 凭据原样保留：调用方还会拿它继续发请求，不能被清空或改写成空串。
        assert_eq!(skipped["access_token"], "old-at");
        assert_eq!(skipped["refresh_token"], "old-rt");
        assert_eq!(skipped["expiresAt"], 1_700_000_000_000i64);
        assert!(
            skipped.get("refreshedAt").is_none(),
            "没有刷新就不能写刷新时刻"
        );
        // 与「暂时失败」同形：保活据此报 temporary_failed，而不是 ok。
        assert!(skipped
            .get("refresh_failed_at")
            .and_then(Value::as_i64)
            .is_some());
        assert_eq!(
            skipped["refresh_failed_reason"],
            auth_file::CREDENTIAL_LOCK_BUSY_MESSAGE
        );
        // 正常账号不带这个标记，否则复核/保活会把正常结果也当成「跳过」。
        assert!(!refresh_was_skipped(&account));
    }

    /// 「刷新令牌」入口的响应投影：跳过时带可识别字段，正常刷新一个字都不多。
    #[test]
    fn refresh_result_meta_exposes_skip_marker_only_when_skipped() {
        let account = json!({
            "id": "acc-202-meta",
            "uid": "uid-202",
            "nickname": "示例账号",
            "access_token": "old-at",
            "refresh_token": "old-rt",
        });

        // 正常刷新：响应形态与改造前完全一致（不多出任何字段）。
        let normal = refresh_result_meta(&account);
        assert!(normal.get("refreshSkipped").is_none());
        assert!(normal.get("refreshSkippedReason").is_none());
        assert_eq!(normal["uid"], "uid-202");

        // 跳过：两个字段都要能被前端读到，且原因里带「未写入任何凭据」。
        let skipped = refresh_skipped_by_credential_lock(
            account,
            auth_file::CREDENTIAL_LOCK_BUSY_MESSAGE.to_string(),
        );
        assert_eq!(
            refresh_skip_reason(&skipped),
            Some(REFRESH_SKIPPED_LOCK_BUSY),
            "机器可读取值必须稳定"
        );
        let meta = refresh_result_meta(&skipped);
        assert_eq!(meta["refreshSkipped"], REFRESH_SKIPPED_LOCK_BUSY);
        assert_eq!(
            meta["refreshSkippedReason"],
            auth_file::CREDENTIAL_LOCK_BUSY_MESSAGE
        );
        // 元数据本身不含 token：响应投影不能把凭据漏给浏览器。
        assert!(meta.get("access_token").is_none());
        assert!(meta.get("refresh_token").is_none());
    }

    // -----------------------------------------------------------------------
    // T01：当前账号不再被抢刷（adopt / skip / 非当前账号回归）
    // -----------------------------------------------------------------------

    /// 环境变量串行守卫：`WB_SHIFT_HOME` 是**进程级**状态，而 `cargo test` 默认多线程并行，
    /// 两个用例同时改它就会读到对方的沙箱（一个用例的 live 文件落进另一个的目录）。
    ///
    /// 用 `tokio::sync::Mutex` 而不是 `std::sync::Mutex`：守卫必须**跨 await 持有**
    /// （被测函数内部要 await 一次刷新请求），而那正是 `std` 版会触发
    /// `clippy::await_holding_lock` 的形态——异步互斥量表达的本来就是「跨 await 的临界区」，
    /// 不需要额外豁免。`const_new` 让它在 `static` 里可常量初始化。
    ///
    /// 顺带丢掉中毒语义：`tokio` 的锁不中毒，某个用例 panic 不会让后来者拿到
    /// `PoisonError`，因此也不必再走 `locks::recover` 那条「中毒后取回 `()`」的路径。
    static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    async fn lock_env() -> tokio::sync::MutexGuard<'static, ()> {
        ENV_LOCK.lock().await
    }

    /// 临时家目录夹具：把 `WB_SHIFT_HOME` 指向临时目录，`Drop` 时恢复原值并删掉目录。
    ///
    /// 必须**恢复原值**而不是一律 `remove_var`：同进程里别的入口（外部脚本、其它用例）
    /// 可能已经设过它，无条件删掉会让后续代码读写真实家目录。
    struct SandboxHome {
        root: PathBuf,
        previous: Option<OsString>,
    }

    impl SandboxHome {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "workbuddy-shift-refresh-{}",
                uuid::Uuid::new_v4().simple()
            ));
            std::fs::create_dir_all(&root).expect("创建沙箱家目录");
            let previous = std::env::var_os(crate::modules::config::HOME_ENV_VAR);
            std::env::set_var(crate::modules::config::HOME_ENV_VAR, &root);
            Self { root, previous }
        }
    }

    impl Drop for SandboxHome {
        fn drop(&mut self) {
            match &self.previous {
                Some(value) => std::env::set_var(crate::modules::config::HOME_ENV_VAR, value),
                None => std::env::remove_var(crate::modules::config::HOME_ENV_VAR),
            }
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    /// 在沙箱里写一份 live 认证文件（路径由**生产实现**算，`WB_SHIFT_HOME` 已生效）。
    fn write_live_auth_file(variant: WbVariant, content: &str) -> PathBuf {
        let path = variant.auth_file_path();
        std::fs::create_dir_all(path.parent().expect("认证文件必须有父目录"))
            .expect("创建认证文件目录");
        std::fs::write(&path, content).expect("写入 live 认证文件");
        path
    }

    /// live 认证文件的 JSON 文本（字段形状与官方客户端一致：`account.uid` + `auth.*`）。
    fn live_auth_json(
        uid: &str,
        access_token: &str,
        refresh_token: &str,
        last_refresh_time: i64,
        expires_at: i64,
    ) -> String {
        serde_json::to_string_pretty(&json!({
            "account": {"uid": uid},
            "auth": {
                "accessToken": access_token,
                "refreshToken": refresh_token,
                "lastRefreshTime": last_refresh_time,
                "expiresAt": expires_at,
            },
        }))
        .expect("序列化 live 认证文件")
    }

    /// 验收 1：库内旧、live 新（同一个 uid）→ adopt：回填账号库、**0 次刷新请求**、
    /// live 文件一个字节都不动。
    #[tokio::test]
    async fn adopt_when_live_credentials_are_newer() {
        // 守卫跨 await 持有：`WB_SHIFT_HOME` 是进程级变量，放锁后另一个并行用例会改掉它，
        // 本用例就会读到（并写进）对方的沙箱。
        let _env = lock_env().await;
        let _sandbox = SandboxHome::new();
        let variant = WbVariant::Cn;

        // 13 位毫秒基准（现实的 `refreshedAt` / `expiresAt`，且必须是过去时刻：
        // `normalize_refresh_ts` 会把未来时刻当「未知」丢弃）。
        const STORE_REFRESHED_AT: i64 = 1_700_000_000_000;
        const LIVE_REFRESHED_AT: i64 = STORE_REFRESHED_AT + 60_000;
        const LIVE_EXPIRES_AT: i64 = STORE_REFRESHED_AT + 7_200_000;

        let account = json!({
            "id": "acc-adopt",
            "uid": "uid-adopt",
            "variant": "cn",
            "access_token": "at-old",
            "refresh_token": "rt-old",
            "expiresAt": STORE_REFRESHED_AT + 3_600_000i64,
            "refreshedAt": STORE_REFRESHED_AT,
        });
        crate::modules::account::save_accounts(std::slice::from_ref(&account))
            .expect("写入沙箱账号库");

        let live_path = write_live_auth_file(
            variant,
            &live_auth_json(
                "uid-adopt",
                "at-new",
                "rt-new",
                LIVE_REFRESHED_AT,
                LIVE_EXPIRES_AT,
            ),
        );
        let live_before = std::fs::read(&live_path).expect("读取 live 认证文件");

        let calls_before = REFRESH_HTTP_CALLS.load(Ordering::SeqCst);
        let refreshed = refresh_account_token(account).await;

        assert_eq!(
            refresh_outcome(&refreshed),
            Some(REFRESH_OUTCOME_ADOPTED),
            "同一个 uid 且 live 更新时必须走 adopt"
        );
        assert_eq!(
            REFRESH_HTTP_CALLS.load(Ordering::SeqCst),
            calls_before,
            "adopt 绝不允许发刷新请求（否则会顶掉客户端手上的新令牌）"
        );
        assert_eq!(
            std::fs::read(&live_path).expect("读取 live 认证文件"),
            live_before,
            "adopt 只读 live：一个字节都不许写"
        );

        let stored = crate::modules::account::load_accounts();
        assert_eq!(stored.len(), 1, "adopt 不得追加/删除账号");
        assert_eq!(stored[0]["access_token"], "at-new", "库内 AT 必须追上 live");
        assert_eq!(
            stored[0]["refresh_token"], "rt-new",
            "库内 RT 必须追上 live"
        );
        assert_eq!(stored[0]["refreshedAt"], LIVE_REFRESHED_AT);
        assert_eq!(stored[0]["expiresAt"], LIVE_EXPIRES_AT);
        assert!(
            stored[0].get(REFRESH_OUTCOME_FIELD).is_none(),
            "诊断字段只在内存里：落盘就等于给账号库加了一个永久字段"
        );
        assert!(!has_relogin_marker(&stored[0]));
    }

    /// 验收 2：live 文件存在但内容不是合法 JSON → 跳过本次：不发请求、不改账号库、
    /// 不写 `needs_relogin`（文件可能正在被官方客户端写，任何结论都不可靠）。
    #[tokio::test]
    async fn skip_when_live_unreadable() {
        let _env = lock_env().await;
        let _sandbox = SandboxHome::new();
        let variant = WbVariant::Cn;

        let account = json!({
            "id": "acc-skip",
            "uid": "uid-skip",
            "variant": "cn",
            "access_token": "at-skip",
            "refresh_token": "rt-skip",
            "expiresAt": 1_700_000_000_000i64,
        });
        crate::modules::account::save_accounts(std::slice::from_ref(&account))
            .expect("写入沙箱账号库");
        let accounts_before =
            std::fs::read(crate::modules::config::accounts_file()).expect("读取账号库");

        let live_path = write_live_auth_file(variant, "{ 这不是合法 JSON");
        let live_before = std::fs::read(&live_path).expect("读取 live 认证文件");

        let calls_before = REFRESH_HTTP_CALLS.load(Ordering::SeqCst);
        let skipped = refresh_account_token(account).await;

        assert_eq!(
            refresh_outcome(&skipped),
            Some(REFRESH_OUTCOME_SKIPPED_LIVE_UNREADABLE),
            "文件在但读不出来必须跳过，不能退化成照常刷新"
        );
        assert_eq!(
            REFRESH_HTTP_CALLS.load(Ordering::SeqCst),
            calls_before,
            "跳过时不得发刷新请求"
        );
        assert!(
            !refresh_was_skipped(&skipped),
            "live 读不出来是另一种跳过，不要挤占「抢不到锁」的稳定标记"
        );
        assert!(
            !has_relogin_marker(&skipped),
            "live 读不出来不是「需重新登录」：账号本身没被吊销"
        );
        assert!(
            skipped.get("refresh_failed_at").is_none(),
            "跳过不是失败：不写暂时失败诊断"
        );
        assert_eq!(
            std::fs::read(crate::modules::config::accounts_file()).expect("读取账号库"),
            accounts_before,
            "跳过必须逐字节不动账号库"
        );
        assert_eq!(
            std::fs::read(&live_path).expect("读取 live 认证文件"),
            live_before,
            "跳过不得改写 live 认证文件"
        );
    }

    /// 验收 3（回归）：live 属于**别的**账号 → 与改造前逐字一致：照常去刷新。
    ///
    /// 网络部分换成「本机不可达端口」（`REFRESH_URL_OVERRIDE` 按账号身份命中），
    /// 因此只验证「尝试过刷新」，绝不触碰真实网关。
    #[tokio::test]
    async fn non_current_account_still_refreshes() {
        let _env = lock_env().await;
        let _sandbox = SandboxHome::new();
        let variant = WbVariant::Cn;

        let account = json!({
            "id": "acc-t01-non-current",
            "uid": "uid-store",
            "variant": "cn",
            "access_token": "at-store",
            "refresh_token": "rt-store",
            "expiresAt": 1_700_000_000_000i64,
        });
        crate::modules::account::save_accounts(std::slice::from_ref(&account))
            .expect("写入沙箱账号库");
        // 客户端当前登录的是另一个账号（uid 不同）。
        write_live_auth_file(
            variant,
            &live_auth_json(
                "uid-other",
                "at-other",
                "rt-other",
                1_700_000_100_000,
                1_700_000_900_000,
            ),
        );

        // 覆盖值按账号 id 匹配：即使本用例 panic 未清理，别的用例也读不到它。
        let _ = crate::modules::locks::recover(&REFRESH_URL_OVERRIDE).replace((
            "acc-t01-non-current".to_string(),
            "http://127.0.0.1:1/v2/plugin/auth/token/refresh".to_string(),
        ));
        let calls_before = REFRESH_HTTP_CALLS.load(Ordering::SeqCst);
        let fresh = refresh_account_token(account).await;
        let _ = crate::modules::locks::recover(&REFRESH_URL_OVERRIDE).take();

        assert_eq!(
            refresh_outcome(&fresh),
            Some(REFRESH_OUTCOME_REFRESHED),
            "非当前账号仍走改造前的刷新路径"
        );
        assert_eq!(
            REFRESH_HTTP_CALLS.load(Ordering::SeqCst),
            calls_before + 1,
            "必须真的尝试过刷新（连不上是本机端口，不是没发）"
        );
        // 连接失败 = 暂时失败：与改造前同一分级（不写 needs_relogin，保留原凭据）。
        assert!(
            !has_relogin_marker(&fresh),
            "连不上不是吊销：不得标「需重新登录」"
        );
        assert_eq!(
            fresh["access_token"], "at-store",
            "失败时原凭据必须原样保留"
        );
        assert_eq!(fresh["refresh_token"], "rt-store");
        assert!(
            fresh.get("refresh_failed_at").is_some(),
            "暂时失败要留诊断字段，供积分页/保活区分「稍后重试」"
        );
    }

    // -----------------------------------------------------------------------
    // R6-01：加密信封永不覆盖明文，按来源分流
    // -----------------------------------------------------------------------

    /// live 文件的 JSON 文本，token 允许是任意 JSON（明文串或 `$wbEncrypted` 信封）。
    ///
    /// 与 [`live_auth_json`] 同形，只有 token 的类型不同：R6-01 的判定完全依赖
    /// 「值本身是不是信封对象」，所以这里必须能把对象塞进 `accessToken`。
    fn live_auth_json_values(
        uid: &str,
        access_token: Value,
        refresh_token: Value,
        last_refresh_time: i64,
        expires_at: i64,
    ) -> String {
        serde_json::to_string_pretty(&json!({
            "account": {"uid": uid},
            "auth": {
                "accessToken": access_token,
                "refreshToken": refresh_token,
                "lastRefreshTime": last_refresh_time,
                "expiresAt": expires_at,
            },
        }))
        .expect("序列化 live 认证文件")
    }

    /// 一份 WorkBuddy 5.6 形态的加密信封（`$wbEncrypted` 包装；本工具不解密，只看形态）。
    fn envelope() -> Value {
        json!({
            "$wbEncrypted": true,
            "envelope": "AAECAwQFBgcICQoLDA0ODw==",
            "keyId": "0123456789abcdef",
            "suite": 1,
        })
    }

    /// 把某个账号写进沙箱账号库，返回写入后的账号库字节（用于「一个字节都不许动」的断言）。
    fn seed_accounts(account: &Value) -> Vec<u8> {
        crate::modules::account::save_accounts(std::slice::from_ref(account))
            .expect("写入沙箱账号库");
        std::fs::read(crate::modules::config::accounts_file()).expect("读取账号库")
    }

    /// 单测里把刷新请求引到本机不可达端口（只对指定账号生效，绝不打真实网关）。
    fn override_refresh_url(account_id: &str) {
        let _ = crate::modules::locks::recover(&REFRESH_URL_OVERRIDE).replace((
            account_id.to_string(),
            "http://127.0.0.1:1/v2/plugin/auth/token/refresh".to_string(),
        ));
    }

    fn clear_refresh_url_override() {
        let _ = crate::modules::locks::recover(&REFRESH_URL_OVERRIDE).take();
    }

    /// 验收 1：live 是信封、账号库明文未过期 → 保留明文。
    ///
    /// 三条同时成立才算过：账号库**逐字节不变**（既没被信封覆盖、也没被重排 JSON）、
    /// **0 次刷新请求**、内存 outcome 是 `kept_plaintext`。
    #[tokio::test]
    async fn keep_plaintext_when_live_is_envelope_and_stored_plaintext_is_valid() {
        let _env = lock_env().await;
        let _sandbox = SandboxHome::new();
        let variant = WbVariant::Cn;

        let account = json!({
            "id": "acc-r6-keep",
            "uid": "uid-r6-keep",
            "variant": "cn",
            "access_token": "at-plain",
            "refresh_token": "rt-plain",
            // 明文仍有效（未来时刻），因此信封毫无价值。
            "expiresAt": now_ms() + 86_400_000i64,
        });
        let accounts_before = seed_accounts(&account);
        let live_path = write_live_auth_file(
            variant,
            &live_auth_json_values(
                "uid-r6-keep",
                envelope(),
                envelope(),
                now_ms() - 60_000,
                now_ms() + 7_200_000,
            ),
        );
        let live_before = std::fs::read(&live_path).expect("读取 live 认证文件");

        let calls_before = REFRESH_HTTP_CALLS.load(Ordering::SeqCst);
        let kept = refresh_account_token(account).await;

        assert_eq!(
            refresh_outcome(&kept),
            Some(REFRESH_OUTCOME_KEPT_PLAINTEXT),
            "live 是信封、库内明文仍有效时必须保留明文"
        );
        assert_eq!(
            REFRESH_HTTP_CALLS.load(Ordering::SeqCst),
            calls_before,
            "保留明文不得发刷新请求"
        );
        assert_eq!(
            std::fs::read(crate::modules::config::accounts_file()).expect("读取账号库"),
            accounts_before,
            "保留明文必须逐字节不动账号库（信封覆盖明文正是 R6-01 的原始故障）"
        );
        assert_eq!(
            std::fs::read(&live_path).expect("读取 live 认证文件"),
            live_before,
            "不得改写 live 认证文件"
        );
        assert!(!has_relogin_marker(&kept), "保留明文不是失败，不得写标记");
        // 事件日志：账号序号从 1 开始，action 与 outcome 同名。
        let events = crate::modules::credential_events::recent(10);
        assert_eq!(events[0]["action"], json!("kept_plaintext"));
        assert_eq!(events[0]["account"], json!(1));
        assert_eq!(events[0]["variant"], json!("cn"));
    }

    /// 验收 2：live 是信封、明文已过期、来源 `oauth` → 独立会话照常刷新。
    ///
    /// 网络换成「本机不可达端口」，只验证**确实发出了刷新请求**（计数 +1），绝不碰真实网关。
    #[tokio::test]
    async fn oauth_account_still_refreshes_when_live_is_envelope() {
        let _env = lock_env().await;
        let _sandbox = SandboxHome::new();
        let variant = WbVariant::Cn;

        let account = json!({
            "id": "acc-r6-oauth",
            "uid": "uid-r6-oauth",
            "variant": "cn",
            "credentialSource": "oauth",
            // 明文已过期 → 信封出现时 oauth 账号要自己续期（独立会话）。
            "access_token": "at-expired",
            "refresh_token": "rt-live",
            "expiresAt": 1_700_000_000_000i64,
        });
        seed_accounts(&account);
        write_live_auth_file(
            variant,
            &live_auth_json_values(
                "uid-r6-oauth",
                envelope(),
                envelope(),
                now_ms() - 60_000,
                now_ms() + 7_200_000,
            ),
        );
        override_refresh_url("acc-r6-oauth");

        let calls_before = REFRESH_HTTP_CALLS.load(Ordering::SeqCst);
        let refreshed = refresh_account_token(account).await;
        clear_refresh_url_override();

        assert_eq!(
            refresh_outcome(&refreshed),
            Some(REFRESH_OUTCOME_REFRESHED),
            "oauth 账号必须走改造前的刷新路径"
        );
        assert_eq!(
            REFRESH_HTTP_CALLS.load(Ordering::SeqCst),
            calls_before + 1,
            "必须真的尝试过刷新（连不上是本机端口，不是没发）"
        );
        // 连不上 = 暂时失败：不写「需重新登录」，原凭据原样保留。
        assert!(!has_relogin_marker(&refreshed));
        assert_eq!(refreshed["access_token"], "at-expired");
    }

    /// 验收 3：live 是信封、明文已过期、来源 `local` → 跳过。
    ///
    /// `local` 与客户端共用 refresh 会话，用库里的旧 RT 去刷会把客户端顶掉；因此
    /// 0 请求 + 账号库逐字节不变 + outcome `skipped_encrypted`。
    #[tokio::test]
    async fn local_account_skips_when_live_is_envelope() {
        let _env = lock_env().await;
        let _sandbox = SandboxHome::new();
        let variant = WbVariant::Cn;

        let account = json!({
            "id": "acc-r6-local",
            "uid": "uid-r6-local",
            "variant": "cn",
            "credentialSource": "local",
            "access_token": "at-expired",
            "refresh_token": "rt-shared",
            "expiresAt": 1_700_000_000_000i64,
        });
        let accounts_before = seed_accounts(&account);
        write_live_auth_file(
            variant,
            &live_auth_json_values(
                "uid-r6-local",
                envelope(),
                envelope(),
                now_ms() - 60_000,
                now_ms() + 7_200_000,
            ),
        );

        let calls_before = REFRESH_HTTP_CALLS.load(Ordering::SeqCst);
        let skipped = refresh_account_token(account).await;

        assert_eq!(
            refresh_outcome(&skipped),
            Some(REFRESH_OUTCOME_SKIPPED_ENCRYPTED),
            "local 账号遇到 live 信封必须跳过（不能抢刷客户端会话）"
        );
        assert_eq!(
            REFRESH_HTTP_CALLS.load(Ordering::SeqCst),
            calls_before,
            "跳过不得发刷新请求"
        );
        assert_eq!(
            std::fs::read(crate::modules::config::accounts_file()).expect("读取账号库"),
            accounts_before,
            "跳过必须逐字节不动账号库"
        );
        assert!(
            !has_relogin_marker(&skipped),
            "跳过不是「需重新登录」：账号本身没被吊销"
        );
    }

    /// 判不出来源（无 `credentialSource`、`auth_raw` 也不是认证文件形状）时按 `local` 保守处理。
    #[tokio::test]
    async fn unknown_source_is_treated_as_local_when_live_is_envelope() {
        let _env = lock_env().await;
        let _sandbox = SandboxHome::new();
        let variant = WbVariant::Cn;

        let account = json!({
            "id": "acc-r6-unknown",
            "uid": "uid-r6-unknown",
            "variant": "cn",
            "access_token": "at-expired",
            "refresh_token": "rt-shared",
            "expiresAt": 1_700_000_000_000i64,
        });
        seed_accounts(&account);
        write_live_auth_file(
            variant,
            &live_auth_json_values(
                "uid-r6-unknown",
                envelope(),
                envelope(),
                now_ms() - 60_000,
                now_ms() + 7_200_000,
            ),
        );

        let calls_before = REFRESH_HTTP_CALLS.load(Ordering::SeqCst);
        let skipped = refresh_account_token(account).await;
        assert_eq!(
            refresh_outcome(&skipped),
            Some(REFRESH_OUTCOME_SKIPPED_ENCRYPTED)
        );
        assert_eq!(REFRESH_HTTP_CALLS.load(Ordering::SeqCst), calls_before);
    }

    /// 验收 5：`apply_live_credentials` 的防御分支——信封不得覆盖明文字符串。
    ///
    /// 直接调私有函数，模拟「将来新增调用点绕过了 `decide_live_action`」。
    #[test]
    fn apply_live_credentials_refuses_to_overwrite_plaintext_with_envelope() {
        let mut account = json!({
            "id": "acc-r6-guard",
            "access_token": "at-plain",
            "refresh_token": "rt-plain",
            "expiresAt": 1_700_000_000_000i64,
        });
        let before = account.clone();
        let credentials = LiveCredentials {
            access_token: envelope(),
            refresh_token: Some(envelope()),
            expires_at: Some(1_800_000_000_000),
            refreshed_at: Some(1_790_000_000_000),
            refresh_expires_at: Some(1_900_000_000_000),
        };

        apply_live_credentials(&mut account, &credentials);

        assert_eq!(account, before, "防御分支必须一个字段都不写");
        assert_eq!(account["access_token"], "at-plain");
    }

    /// 防御分支不该误伤正常 adopt：live 是明文时照常回填（回归 T01 行为）。
    #[test]
    fn apply_live_credentials_still_applies_plaintext() {
        let mut account = json!({"id": "acc-r6-apply", "access_token": "at-old"});
        let credentials = LiveCredentials {
            access_token: json!("at-new"),
            refresh_token: Some(json!("rt-new")),
            expires_at: Some(1_800_000_000_000),
            refreshed_at: Some(1_790_000_000_000),
            refresh_expires_at: None,
        };

        apply_live_credentials(&mut account, &credentials);

        assert_eq!(account["access_token"], "at-new");
        assert_eq!(account["refresh_token"], "rt-new");
        assert_eq!(account["expiresAt"], json!(1_800_000_000_000i64));
    }

    /// 信封判定的回归：必须同时认 `access_token` 与 `refresh_token` 两个字段。
    #[test]
    fn envelope_detection_covers_both_credential_fields() {
        assert!(has_envelope_credential(
            &json!({"refresh_token": envelope()})
        ));
        assert!(has_envelope_credential(
            &json!({"access_token": envelope()})
        ));
        assert!(!has_envelope_credential(
            &json!({"access_token": "at", "refresh_token": "rt"})
        ));
        assert!(!has_envelope_credential(&json!({})));
    }

    // -----------------------------------------------------------------------
    // R6-02：信封 refresh token 不再被误标「缺少 refresh token」
    // -----------------------------------------------------------------------

    /// 验收 6：信封账号 → 不写 `needs_relogin`、不 upsert、0 请求。
    #[tokio::test]
    async fn envelope_credentials_are_skipped_without_marking_relogin() {
        let _env = lock_env().await;
        let _sandbox = SandboxHome::new();

        let account = json!({
            "id": "acc-r6-envelope",
            "uid": "uid-r6-envelope",
            "variant": "cn",
            "access_token": envelope(),
            "refresh_token": envelope(),
            "expiresAt": 1_700_000_000_000i64,
        });
        let accounts_before = seed_accounts(&account);

        let calls_before = REFRESH_HTTP_CALLS.load(Ordering::SeqCst);
        let skipped = refresh_account_token(account).await;

        assert_eq!(
            refresh_outcome(&skipped),
            Some(REFRESH_OUTCOME_SKIPPED_ENCRYPTED)
        );
        assert_eq!(REFRESH_HTTP_CALLS.load(Ordering::SeqCst), calls_before);
        assert!(
            !has_relogin_marker(&skipped),
            "信封不是「缺少 refresh token」：不得写 needs_relogin"
        );
        assert!(
            skipped.get("needs_relogin_reason").is_none(),
            "更不得写下误导性原因"
        );
        assert_eq!(
            std::fs::read(crate::modules::config::accounts_file()).expect("读取账号库"),
            accounts_before,
            "不得 upsert（账号库逐字节不变）"
        );
    }

    /// 验收 7a：历史误标 + 信封 → 标记被**精确**清除（且真的落盘）。
    #[tokio::test]
    async fn stale_mislabel_is_cleared_for_envelope_accounts() {
        let _env = lock_env().await;
        let _sandbox = SandboxHome::new();

        let mut account = json!({
            "id": "acc-r6-mislabel",
            "uid": "uid-r6-mislabel",
            "variant": "cn",
            "access_token": envelope(),
            "refresh_token": envelope(),
            "expiresAt": 1_700_000_000_000i64,
            "needs_relogin": true,
            "needs_relogin_since": 1_700_000_000_000i64,
            "needs_relogin_fail_count": 7,
            "needs_relogin_last_result": "failed",
        });
        account["needs_relogin_reason"] = json!(MISLABELED_RELOGIN_REASONS[0]);
        seed_accounts(&account);

        let _ = refresh_account_token(account).await;

        let stored = crate::modules::account::load_accounts();
        assert_eq!(stored.len(), 1);
        for key in RELOGIN_FIELDS.iter().chain(TEMPORARY_FIELDS.iter()) {
            assert!(
                stored[0].get(*key).is_none(),
                "历史误标必须连同配套字段一起清掉：`{key}` 仍在"
            );
        }
        // 凭据原样保留：清标记不是「改凭据」。
        assert!(is_envelope(&stored[0], "access_token"));
    }

    /// 验收 7b：同一条误标文案挂在**明文**账号上（真吊销 / 真缺 RT）→ 保留标记。
    #[test]
    fn mislabel_is_kept_for_plaintext_accounts() {
        let mut account = json!({
            "id": "acc-r6-revoked",
            "access_token": "at",
            "refresh_token": "rt",
            "needs_relogin": true,
        });
        account["needs_relogin_reason"] = json!(MISLABELED_RELOGIN_REASONS[0]);

        assert!(
            !clear_mislabeled_relogin(&mut account),
            "明文账号上的「缺少 refresh token」可能是真的，不得清"
        );
        assert!(has_relogin_marker(&account));
        assert_eq!(
            account["needs_relogin_reason"],
            json!(MISLABELED_RELOGIN_REASONS[0])
        );
    }

    /// 验收 7c：原因不是工具生成的那两条（服务端吊销）→ 一律不动。
    #[test]
    fn other_relogin_reasons_are_never_touched() {
        for reason in [
            "续期被拒（code=12153）：invalid_grant: Invalid refresh token",
            // 前缀相同但**不是**逐字相等：不许按「包含」放宽。
            "缺少 refresh token，无法刷新",
            "",
        ] {
            let mut account = json!({
                "access_token": envelope(),
                "refresh_token": envelope(),
                "needs_relogin": true,
                "needs_relogin_reason": reason,
            });
            assert!(
                !clear_mislabeled_relogin(&mut account),
                "只允许清工具生成的两条精确文案，`{reason}` 不得清"
            );
            assert!(has_relogin_marker(&account));
        }
    }

    /// 验收 8：保活汇总里信封账号计 `skipped`，**不计** `failed`，也不写标记。
    #[tokio::test]
    async fn keepalive_counts_envelope_accounts_as_skipped() {
        let _env = lock_env().await;
        let _sandbox = SandboxHome::new();

        let account = json!({
            "id": "acc-r6-keepalive",
            "uid": "uid-r6-keepalive",
            "variant": "cn",
            "access_token": envelope(),
            "refresh_token": envelope(),
            // 过期 → 保活一定想处理它（默认 keepalive_days=0 时本就无条件处理）。
            "expiresAt": 1_700_000_000_000i64,
        });
        seed_accounts(&account);

        let calls_before = REFRESH_HTTP_CALLS.load(Ordering::SeqCst);
        let summary = run_keepalive_cycle().await;
        let refreshed = summary["refreshed"].as_array().cloned().unwrap_or_default();

        assert_eq!(refreshed.len(), 1, "保活必须扫到该账号：{summary}");
        assert_eq!(
            refreshed[0]["status"], "skipped",
            "信封账号计 skipped（不是 failed）：{summary}"
        );
        assert_eq!(
            REFRESH_HTTP_CALLS.load(Ordering::SeqCst),
            calls_before,
            "保活不得对信封账号发刷新请求"
        );
        let stored = crate::modules::account::load_accounts();
        assert!(!has_relogin_marker(&stored[0]), "不得写「需重新登录」");
    }

    // -----------------------------------------------------------------------
    // R6-06 契约：`__at_rest` 临时副本绝不刷新
    // -----------------------------------------------------------------------

    /// 验收 10：临时副本走 `refresh_account_token` / `ensure_fresh_token` 都是 0 请求、不改库。
    #[tokio::test]
    async fn at_rest_temp_account_is_never_refreshed() {
        let _env = lock_env().await;
        let _sandbox = SandboxHome::new();

        let account = json!({
            "id": "acc-r6-at-rest",
            "uid": "uid-r6-at-rest",
            "variant": "cn",
            // 临时副本的凭据是解密出的明文，且必然「临期」——最容易触发刷新的形态。
            "access_token": "at-decrypted",
            "refresh_token": "rt-decrypted",
            "expiresAt": 1_700_000_000_000i64,
            "__at_rest": true,
        });
        // 账号库里是**去掉标记**的同一账号（临时副本永远不落盘）。
        let accounts_before = seed_accounts(&json!({
            "id": "acc-r6-at-rest",
            "uid": "uid-r6-at-rest",
            "variant": "cn",
            "access_token": "at-decrypted",
            "refresh_token": "rt-decrypted",
            "expiresAt": 1_700_000_000_000i64,
        }));

        assert!(is_at_rest_temp(&account), "标记必须可被守卫读到");

        let calls_before = REFRESH_HTTP_CALLS.load(Ordering::SeqCst);
        let refreshed = refresh_account_token(account.clone()).await;
        assert_eq!(
            refresh_outcome(&refreshed),
            Some(REFRESH_OUTCOME_SKIPPED_AT_REST)
        );

        let ensured = ensure_fresh_token(account, &json!({"lazy_refresh_hours": 24})).await;
        assert_eq!(
            refresh_outcome(&ensured),
            Some(REFRESH_OUTCOME_SKIPPED_AT_REST),
            "ensure_fresh_token 也必须在入口直接返回"
        );

        assert_eq!(
            REFRESH_HTTP_CALLS.load(Ordering::SeqCst),
            calls_before,
            "临时副本绝不允许发刷新请求（会轮换掉客户端的令牌）"
        );
        assert_eq!(
            std::fs::read(crate::modules::config::accounts_file()).expect("读取账号库"),
            accounts_before,
            "临时副本不得写进账号库"
        );
    }

    /// 普通账号不带 `__at_rest`，守卫不得误伤（回归）。
    #[test]
    fn at_rest_guard_ignores_normal_accounts() {
        assert!(!is_at_rest_temp(&json!({"id": "acc"})));
        assert!(!is_at_rest_temp(&json!({"id": "acc", "__at_rest": false})));
        assert!(!is_at_rest_temp(&json!({"id": "acc", "__at_rest": "true"})));
        assert!(is_at_rest_temp(&json!({"id": "acc", "__at_rest": true})));
    }
}
