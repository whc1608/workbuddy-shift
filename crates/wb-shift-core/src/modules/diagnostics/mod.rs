//! 诊断包：把排障需要的事实汇总成**一份可直接粘贴进 issue 的脱敏文本**。
//!
//! # 为什么需要
//!
//! 用户来提 issue 时给不出可复现信息（版本？宿主？哪个客户端没装？日志最后一条说什
//! 么？），作者只能来回问三轮。这里把「作者必问的那几项」一次性收齐并脱敏，用户点一下
//! 导出、整段粘贴即可。
//!
//! # 分层（R6-03 / R6-09）
//!
//! - [`facts`]：**事实投影层**，把宿主持有的原始数据（账号 JSON、设置包、认证文件、
//!   客户端探测、凭据事件……）投影成枚举 / 布尔 / 数字 / 短文本；所有读取都是只读且可
//!   失败，降级为 `None` / 计数 0。**绝不输出 token / 路径 / 昵称 / 邮箱 / uid**。
//! - [`report`]：装配层，只消费 [`facts`] 的产物 + 宿主传入的纯值，负责分段、格式化、
//!   最终整段脱敏，并做 `.txt` 落盘校验。
//! - [`redact`]：脱敏纯函数，逐片段单测。
//!
//! 结构上「读账号 / 令牌」只出现在 [`facts`] 一处，装配层拿到的都是投影后的事实，
//! 因此「不小心把 accounts.json 的字段带出来」不会发生在装配阶段。
//!
//! # 外形
//!
//! 输出**单个 `.txt`**（分段标题 + 缩进），不做 zip：issue 正文里 zip 没法看。
//!
//! # 时间口径（R6-04）
//!
//! 头部与文件名都用**本地时间 + 偏移**（如 `2026-11-28 00:53:36 +08:00`），存储层 `ts`
//! 仍是 epoch 毫秒，不动契约。

pub mod facts;
#[allow(clippy::module_inception)]
mod redact;
mod report;

pub use self::facts::{
    account_facts, client_facts, credential_events_recent, settings_facts, workbuddy_facts,
    AccessState, AccountFact, AccountStoreFact, ClientFact, CredentialConfigFacts, CredentialEvent,
    LinkFacts, ReloginState, SettingFact, StatsFacts, TokenKind, WorkBuddyFacts,
};
pub use self::redact::{
    redact, redact_text, redact_with_user, Redacted, MASK_EMAIL, MASK_NUM, MASK_PATH, MASK_TOKEN,
    MASK_URL, MASK_USER, MASK_UUID,
};
pub use self::report::{
    account_bucket, build_report, build_report_default, client_probe_from_status,
    collect_cheap_sources, default_sources, format_timestamp_with_offset, format_utc,
    hook_state_from_status, local_offset_seconds, probe_clients, save_report, ClientProbe,
    GenerationFacts, HookState, HookTarget, HostForm, ProbeSpec, ProbeStatus, ProbeTiming,
    ReportFacts, ReportInput, ReportSources, CREDENTIAL_EVENT_LINES, DEFAULT_LOG_LINES,
    LOG_DETAIL_LINES, MAX_REPORT_BYTES, PROBE_CACHE_TTL_MS, PROBE_TIMEOUT_MS, REPORT_EXTENSIONS,
};

#[cfg(test)]
mod tests;
