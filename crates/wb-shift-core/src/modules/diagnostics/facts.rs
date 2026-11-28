//! 诊断包的**事实投影层**：把宿主侧持有的原始数据（账号 JSON、设置包、认证文件、
//! 客户端探测结果、凭据事件日志……）投影成**枚举 / 布尔 / 数字 / 短文本**。
//!
//! # 为什么单拆一层
//!
//! R6-09 要求诊断包「尽可能详细」，但 diagnostics 的装配层必须保持纯格式化：一旦
//! 装配函数里直接读账号 / 令牌 / 会话，脱敏边界就只能靠评审盯。这里把「读」与
//! 「投影」收敛到本文件，投影产出全部是非机密事实（序号、档位、形态枚举、剩余时长、
//! 布尔、计数），装配层只消费这些产物。
//!
//! 硬约束：
//! - **绝不输出 token / 路径 / 昵称 / 邮箱 / uid** —— token 只看类型（明文 / 信封 / 缺失）；
//! - 所有读取都是**只读且可失败**：文件缺失 / 损坏 / DB 打不开一律降级为 `None` / 计数 0，
//!   由装配层写「未配置 / 读取失败 / 无记录」，绝不让整份报告失败；
//! - 凭据事件日志按 0.1.11 契约读 `<store_root>/credential-events.jsonl`（C 轨只读该文件），
//!   不依赖 P1 的 Rust helper，缺失即「无记录」。

use serde_json::Value;
use std::path::Path;

use crate::modules::config::store_dir;

// ---------------------------------------------------------------------------
// Token / 到期形态
// ---------------------------------------------------------------------------

/// token 字段形态（只看类型，不取值）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TokenKind {
    /// 明文字符串。
    Plain,
    /// `$wbEncrypted` 信封对象。
    Envelope,
    /// 字段缺失 / 空串 / 其它类型。
    #[default]
    Missing,
}

impl TokenKind {
    /// 报告里的稳定取值。
    pub fn label(self) -> &'static str {
        match self {
            Self::Plain => "明文",
            Self::Envelope => "信封",
            Self::Missing => "缺失",
        }
    }

    /// `[凭据健康]` 里 refresh 列的取值：`有` / `信封` / `缺失`。
    ///
    /// 与 access 列不同，这里只关心「能不能用来刷新」，因此明文写「有」。
    pub fn refresh_label(self) -> &'static str {
        match self {
            Self::Plain => "有",
            Self::Envelope => "信封",
            Self::Missing => "缺失",
        }
    }
}

/// access token 的到期状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AccessState {
    /// 明文且未过期，附带剩余毫秒。
    Valid(i64),
    /// 明文但已过期，附带超期毫秒（负数取绝对值）。
    Expired(i64),
    /// 信封 / 缺失 / 无 expiresAt：判不出。
    #[default]
    Unknown,
}

impl AccessState {
    /// 渲染成 `有效(剩余 5h)` / `已过期(2h 前)` / `未知`。
    pub fn label(self) -> String {
        match self {
            Self::Valid(remaining) => format!("有效(剩余 {})", humanize_span(remaining)),
            Self::Expired(past) => format!("已过期({} 前)", humanize_span(past)),
            Self::Unknown => "未知".to_string(),
        }
    }
}

/// 是否「需重新登录」+ 原因类别（只输出枚举，不输出自由文本）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ReloginState {
    /// 是否需要重新登录。
    pub needed: bool,
}

impl ReloginState {
    /// `需重登=是/否`。
    pub fn label(self) -> &'static str {
        if self.needed {
            "是"
        } else {
            "否"
        }
    }
}

/// 毫秒跨度 → 人能读的粗粒度时长（`5h` / `28d` / `12m` / `未知`）。
pub fn humanize_span(ms: i64) -> String {
    let ms = ms.abs();
    if ms <= 0 {
        return "未知".to_string();
    }
    let minutes = ms / 60_000;
    if minutes < 1 {
        return "<1m".to_string();
    }
    let hours = minutes / 60;
    if hours < 1 {
        return format!("{minutes}m");
    }
    let days = hours / 24;
    if days < 1 {
        return format!("{hours}h");
    }
    format!("{days}d")
}

// ---------------------------------------------------------------------------
// 账号事实
// ---------------------------------------------------------------------------

/// 单个账号的脱敏事实。**不含任何身份或令牌值**。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AccountFact {
    /// 报告里的序号（`#1` 起）。
    pub index: usize,
    /// 档位标签：国内版 / 国际版。
    pub variant: String,
    /// 来源：`oauth` / `local` / `import` / `unknown`。
    pub source: String,
    /// 该来源是否是推断值（历史账号）——是则报告里加 `?`。
    pub source_inferred: bool,
    /// access token 形态。
    pub token_kind: TokenKind,
    /// access 到期状态。
    pub access: AccessState,
    /// refresh token 形态。
    pub refresh: TokenKind,
    /// refresh token 是否已过期（无 `refreshTokenExpiresAt` 时为 `false`，即按「未过期」）。
    pub refresh_expired: bool,
    /// 是否需重新登录。
    pub relogin: ReloginState,
    /// 是否企业版。
    pub enterprise: bool,
    /// 是否有备注。
    pub has_note: bool,
    /// 创建距今（毫秒，`None` = 未知）。
    pub age_ms: Option<i64>,
    /// 最近一次凭据事件的短文本（`refreshed(2h 前)`）；由 [`attach_account_events`] 回填。
    pub last_event: Option<String>,
}

/// 账号列表投影。`accounts` 是已由宿主 `load_accounts()` 读出的原始 JSON。
pub fn account_facts(accounts: &[Value], now_ms: i64) -> Vec<AccountFact> {
    accounts
        .iter()
        .enumerate()
        .map(|(i, acc)| account_fact(i, acc, now_ms))
        .collect()
}

fn account_fact(index: usize, acc: &Value, now_ms: i64) -> AccountFact {
    let variant = variant_label(acc);
    let access_kind = token_kind(acc.get("access_token"));
    let refresh = token_kind(acc.get("refresh_token"));
    let access = access_state(&access_kind, acc, now_ms);
    let refresh_expired = refresh == TokenKind::Plain
        && acc
            .get("refreshTokenExpiresAt")
            .and_then(Value::as_i64)
            .is_some_and(|expires_at| expires_at <= now_ms);
    let (source, source_inferred) = credential_source(acc);
    let relogin = ReloginState {
        needed: acc
            .get("needs_relogin")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    };
    let enterprise = acc.get("enterpriseId").is_some() || acc.get("enterprise_id").is_some();
    let has_note = acc
        .get("note")
        .and_then(Value::as_str)
        .is_some_and(|note| !note.trim().is_empty());
    let age_ms = acc
        .get("createdAt")
        .and_then(Value::as_i64)
        .map(|created| now_ms - created);
    AccountFact {
        index: index + 1,
        variant,
        source,
        source_inferred,
        token_kind: access_kind,
        access,
        refresh,
        refresh_expired,
        relogin,
        enterprise,
        has_note,
        age_ms,
        last_event: None,
    }
}

/// 判断单个 token 字段的形态（只看类型）。
fn token_kind(value: Option<&Value>) -> TokenKind {
    match value {
        Some(Value::String(s)) if !s.trim().is_empty() => TokenKind::Plain,
        Some(Value::Object(map)) if map.contains_key("$wbEncrypted") => TokenKind::Envelope,
        _ => TokenKind::Missing,
    }
}

/// 计算 access 到期状态：只有明文 + 有 `expiresAt` 才能判。
fn access_state(kind: &TokenKind, acc: &Value, now_ms: i64) -> AccessState {
    if *kind != TokenKind::Plain {
        return AccessState::Unknown;
    }
    let Some(expires_at) = acc.get("expiresAt").and_then(Value::as_i64) else {
        return AccessState::Unknown;
    };
    let delta = expires_at - now_ms;
    if delta > 0 {
        AccessState::Valid(delta)
    } else {
        AccessState::Expired(-delta)
    }
}

/// 档位标签：`variant` 字段优先，缺省国内版。
fn variant_label(acc: &Value) -> String {
    match acc.get("variant").and_then(Value::as_str) {
        Some("ai") | Some("intl") | Some("global") => "国际版".to_string(),
        _ => "国内版".to_string(),
    }
}

/// 来源：显式 `credentialSource` 优先；缺失时按 0.1.11 规则推断并标记为推断值。
fn credential_source(acc: &Value) -> (String, bool) {
    if let Some(source) = acc.get("credentialSource").and_then(Value::as_str) {
        let normalized = match source {
            "oauth" => "oauth",
            "local" => "local",
            "import" => "import",
            _ => "unknown",
        };
        return (normalized.to_string(), false);
    }
    // 历史账号推断：同时含 `auth` / `account` 根结构 → 本机导入（local）；
    // 其余（token 响应形状，明文 access）→ oauth。判不出按 local 保守处理。
    let looks_local = acc.get("auth").is_some() && acc.get("account").is_some();
    if looks_local {
        ("local".to_string(), true)
    } else if token_kind(acc.get("access_token")) == TokenKind::Plain {
        ("oauth".to_string(), true)
    } else {
        ("local".to_string(), true)
    }
}

/// 把凭据事件回填到账号事实的「最近刷新」列（按账号序号匹配最近一条）。
pub fn attach_account_events(facts: &mut [AccountFact], events: &[CredentialEvent], now_ms: i64) {
    for fact in facts.iter_mut() {
        // 事件按时间升序；取该账号最后一条。
        if let Some(event) = events.iter().rev().find(|e| e.account == fact.index) {
            fact.last_event = Some(format!(
                "{}({} 前)",
                event.action,
                humanize_span(now_ms - event.ts)
            ));
        }
    }
}

// ---------------------------------------------------------------------------
// 账号库与凭据配置（只读文件）
// ---------------------------------------------------------------------------

/// 账号库文件的体量与备份数（不含内容）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AccountStoreFact {
    /// 账号总数。
    pub total: usize,
    /// 账号库文件字节数（`None` = 不存在）。
    pub size_bytes: Option<u64>,
    /// 备份文件数。
    pub backups: usize,
}

/// 读账号库文件体量；账号总数由调用方传入（避免在这里再解析一次）。
pub fn account_store_fact(accounts_file: &Path, total: usize) -> AccountStoreFact {
    let size_bytes = std::fs::metadata(accounts_file).ok().map(|m| m.len());
    let backups = accounts_file
        .parent()
        .map(|dir| dir.join("backups"))
        .filter(|dir| dir.is_dir())
        .map(|dir| {
            std::fs::read_dir(dir)
                .map(|entries| entries.flatten().count())
                .unwrap_or(0)
        })
        .unwrap_or(0);
    AccountStoreFact {
        total,
        size_bytes,
        backups,
    }
}

/// `<store_root>/credential.json` 的投影（布尔 / 短枚举，不含版本以外的自由文本）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CredentialConfigFacts {
    /// 只读开关。
    pub decrypt_read_only: bool,
    /// 写开关。
    pub decrypt_write: bool,
    /// 首次提示是否已展示。
    pub first_prompt_shown: bool,
    /// 平台探测状态：`unknown` / `ok` / `failed`。
    pub platform_probe: String,
    /// 平台探测时的客户端版本（取不到 = `None`）。
    pub platform_probe_version: Option<String>,
}

/// 读 `<store_root>/credential.json`；缺失 / 损坏返回 `None`（报告写「未配置」）。
pub fn credential_config_facts() -> Option<CredentialConfigFacts> {
    read_credential_json(&store_dir().join("credential.json"))
}

/// 可注入路径版本（单测用）。
pub fn read_credential_json(path: &Path) -> Option<CredentialConfigFacts> {
    let text = std::fs::read_to_string(path).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    let bool_of = |key: &str| value.get(key).and_then(Value::as_bool).unwrap_or(false);
    let platform_probe = value
        .get("platformProbe")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    let platform_probe_version = value
        .get("platformProbeVersion")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|v| !v.is_empty() && v.len() <= 32 && !v.contains(['/', '\\']))
        .map(str::to_string);
    Some(CredentialConfigFacts {
        decrypt_read_only: bool_of("decryptReadOnly"),
        decrypt_write: bool_of("decryptWrite"),
        first_prompt_shown: bool_of("firstPromptShown"),
        platform_probe,
        platform_probe_version,
    })
}

// ---------------------------------------------------------------------------
// WorkBuddy 本体
// ---------------------------------------------------------------------------

/// WorkBuddy 本体的只读事实（不含路径 / 账号名 / uid）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkBuddyFacts {
    /// 档位标签（国内版 / 国际版）。
    pub variant: String,
    /// 是否检测到安装。
    pub installed: bool,
    /// 进程是否在运行。
    pub running: bool,
    /// 客户端版本（取不到 = `None` → 报告写「版本未知」）。
    pub version: Option<String>,
    /// 认证文件是否存在。
    pub auth_present: bool,
    /// 认证文件是否可解析。
    pub auth_parseable: bool,
    /// 认证文件字节数。
    pub auth_bytes: Option<u64>,
    /// 认证文件里的账号条数。
    pub account_count: Option<usize>,
    /// token 总体形态（明文 / 信封 / 缺失 / 混合）。
    pub token_kind: String,
}

/// 采集指定档位的 WorkBuddy 事实；版本用共享的唯一实现读取，取不到不报错。
pub fn workbuddy_facts(variant: &str) -> Option<WorkBuddyFacts> {
    use crate::modules::auth_file;
    use crate::modules::process;
    use crate::modules::variant::WbVariant;

    let wb = if variant.eq_ignore_ascii_case("ai") {
        WbVariant::Ai
    } else {
        WbVariant::Cn
    };
    let label = if matches!(wb, WbVariant::Ai) {
        "国际版"
    } else {
        "国内版"
    };
    let app_path = auth_file::workbuddy_app_path(wb);
    let installed = !app_path.as_os_str().is_empty() && app_path.exists();
    let version = process::client_version(&app_path);
    let running = process::is_workbuddy_running(wb);
    let auth = auth_file::read_auth_file(wb);
    let auth_present = auth.is_some();
    let (account_count, token_kind, auth_bytes) = match &auth {
        Some(value) => {
            let count = value
                .get("accounts")
                .and_then(Value::as_array)
                .map(Vec::len)
                .or_else(|| value.get("account").map(|_| 1usize));
            let kind = auth_token_kind(value);
            let bytes = std::fs::metadata(wb.auth_file_path()).ok().map(|m| m.len());
            (count, kind, bytes)
        }
        None => (None, "缺失", None),
    };
    Some(WorkBuddyFacts {
        variant: label.to_string(),
        installed,
        running,
        version,
        auth_present,
        auth_parseable: auth.is_some(),
        auth_bytes,
        account_count,
        token_kind: token_kind.to_string(),
    })
}

/// 认证文件里 token 的总体形态：扫描 `auth` 子对象，全部明文 / 全部信封 / 混合。
fn auth_token_kind(value: &Value) -> &'static str {
    let Some(auth) = value.get("auth").and_then(Value::as_object) else {
        return "缺失";
    };
    let mut plain = false;
    let mut envelope = false;
    for key in ["accessToken", "refreshToken"] {
        match auth.get(key) {
            Some(Value::String(s)) if !s.trim().is_empty() => plain = true,
            Some(Value::Object(map)) if map.contains_key("$wbEncrypted") => envelope = true,
            _ => {}
        }
    }
    match (plain, envelope) {
        (true, true) => "混合",
        (true, false) => "明文",
        (false, true) => "信封",
        (false, false) => "缺失",
    }
}

// ---------------------------------------------------------------------------
// 客户端投影
// ---------------------------------------------------------------------------

/// 单个客户端的事实（报告里一行）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientFact {
    /// 客户端显示名。
    pub name: String,
    /// 是否安装。
    pub installed: bool,
    /// 是否在运行（`None` = 未探测）。
    pub running: Option<bool>,
    /// 版本（`None` = 未知）。
    pub version: Option<String>,
    /// 探测状态标签（`ok` / `timeout` / `error`）。
    pub status: String,
    /// 探测耗时毫秒。
    pub elapsed_ms: Option<u64>,
}

/// 客户端投影：把探测结果压成报告可直接打印的事实列表。
pub fn client_facts(clients: &[super::report::ClientProbe]) -> Vec<ClientFact> {
    clients
        .iter()
        .map(|probe| ClientFact {
            name: probe.name.clone(),
            installed: probe.installed,
            running: probe.running,
            version: probe.version.clone(),
            status: probe.probe_status.as_wire().to_string(),
            elapsed_ms: probe.elapsed_ms,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// 会话关联
// ---------------------------------------------------------------------------

/// 会话关联组的事实（只给计数，不给 session id）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LinkFacts {
    /// 关联组数。
    pub groups: usize,
    /// 成员总数。
    pub members: usize,
    /// active 成员数。
    pub active: usize,
    /// lost 成员数。
    pub lost: usize,
    /// 跨档组数。
    pub cross_variant_groups: usize,
    /// 3 个及以上成员的组数。
    pub large_groups: usize,
}

/// 读 `<store_root>/session_links.json` 投影关联组计数；缺失 / 形状不符返回 `None`。
pub fn link_facts() -> Option<LinkFacts> {
    read_link_json(&store_dir().join("session_links.json"))
}

/// 可注入路径版本（单测用）。
pub fn read_link_json(path: &Path) -> Option<LinkFacts> {
    let text = std::fs::read_to_string(path).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    let groups = value.get("groups")?.as_array()?;
    let mut facts = LinkFacts {
        groups: groups.len(),
        ..LinkFacts::default()
    };
    for group in groups {
        let members = group.get("members").and_then(Value::as_array);
        let member_len = members.map(Vec::len).unwrap_or(0);
        facts.members += member_len;
        if member_len >= 3 {
            facts.large_groups += 1;
        }
        if let Some(members) = members {
            let mut variants: Vec<&str> = Vec::new();
            for member in members {
                match member.get("state").and_then(Value::as_str) {
                    Some("active") => facts.active += 1,
                    Some("lost") => facts.lost += 1,
                    _ => {}
                }
                if let Some(variant) = member.get("variant").and_then(Value::as_str) {
                    if !variants.contains(&variant) {
                        variants.push(variant);
                    }
                }
            }
            if variants.len() > 1 {
                facts.cross_variant_groups += 1;
            }
        }
    }
    Some(facts)
}

// ---------------------------------------------------------------------------
// 统计与自动化
// ---------------------------------------------------------------------------

/// 统计与自动化的最近状态（能取多少取多少，缺的写「未知」）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StatsFacts {
    /// 官方用量缓存状态。
    pub official_status: Option<String>,
    /// 官方用量覆盖的账号数。
    pub official_accounts: Option<usize>,
    /// 本地账本状态。
    pub ledger_status: Option<String>,
    /// 本地账本行数。
    pub ledger_rows: Option<u64>,
    /// 签到最近一轮时间（毫秒）。
    pub checkin_last_ms: Option<i64>,
    /// 签到今天账号数。
    pub checkin_today: Option<usize>,
}

/// 采集统计事实：官方用量 / 账本 / 签到日志，全部只读本地缓存，不联网。
pub fn stats_facts(now_ms: i64) -> StatsFacts {
    use crate::modules::{checkin, credit_ledger, official_usage};
    let official = official_usage::cached_official_usage();
    let official_status = official
        .get("status")
        .and_then(Value::as_str)
        .map(str::to_string);
    let official_accounts = official
        .get("accounts")
        .and_then(Value::as_array)
        .map(Vec::len);
    let ledger = credit_ledger::snapshot(now_ms);
    let ledger_status = ledger
        .get("status")
        .and_then(Value::as_str)
        .map(str::to_string);
    let ledger_rows = ledger.get("rows").and_then(Value::as_u64);
    let logs = checkin::load_checkin_logs_with_variant();
    let checkin_last_ms = logs
        .iter()
        .filter_map(|entry| entry.get("ts").and_then(Value::as_i64))
        .max();
    let today = checkin::date_str(Some(now_ms));
    let checkin_today = Some(
        logs.iter()
            .filter(|entry| {
                entry
                    .get("ts")
                    .and_then(Value::as_i64)
                    .map(|ts| checkin::date_str(Some(ts)) == today)
                    == Some(true)
            })
            .count(),
    );
    StatsFacts {
        official_status,
        official_accounts,
        ledger_status,
        ledger_rows,
        checkin_last_ms,
        checkin_today,
    }
}

// ---------------------------------------------------------------------------
// 凭据事件（0.1.11 契约）
// ---------------------------------------------------------------------------

/// 一条凭据事件（0.1.11 字段固定）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CredentialEvent {
    /// 时间（epoch ms）。
    pub ts: i64,
    /// 账号序号（1 起）。
    pub account: usize,
    /// 档位：`cn` / `ai`。
    pub variant: String,
    /// action 枚举。
    pub action: String,
    /// reason 枚举。
    pub reason: String,
}

/// 读最近的凭据事件（按契约 JSONL），缺失 / 损坏返回空（报告写「无记录」）。
pub fn credential_events_recent(limit: usize) -> Vec<CredentialEvent> {
    read_credential_events(&store_dir().join("credential-events.jsonl"), limit)
}

/// 可注入路径版本（单测用）。逐行解析，跳过非法行，截取最后 `limit` 条。
pub fn read_credential_events(path: &Path, limit: usize) -> Vec<CredentialEvent> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut events: Vec<CredentialEvent> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(|value| {
            Some(CredentialEvent {
                ts: value.get("ts").and_then(Value::as_i64)?,
                account: value.get("account").and_then(Value::as_u64)? as usize,
                variant: value
                    .get("variant")
                    .and_then(Value::as_str)
                    .unwrap_or("cn")
                    .to_string(),
                action: value
                    .get("action")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_string(),
                reason: value
                    .get("reason")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_string(),
            })
        })
        .collect();
    if events.len() > limit {
        events.drain(..events.len() - limit);
    }
    events
}

/// 各 outcome 计数（`refreshed` / `adopted` / `kept_plaintext` / `skipped_encrypted` /
/// `temporarily_failed` / `failed`），供 `[刷新与保活]` 使用。
pub fn outcome_counts(events: &[CredentialEvent]) -> Vec<(&'static str, usize)> {
    const OUTCOMES: [&str; 6] = [
        "refreshed",
        "adopted",
        "kept_plaintext",
        "skipped_encrypted",
        "temporarily_failed",
        "failed",
    ];
    OUTCOMES
        .iter()
        .map(|name| {
            let count = events.iter().filter(|event| event.action == *name).count();
            (*name, count)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// 设置投影
// ---------------------------------------------------------------------------

/// 一条设置事实：分组 + 键名 + 投影后的短值。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingFact {
    /// 配置分组（`checkin` / `travel` / `rotate` / `rateLimit` / `credential`）。
    pub group: String,
    /// 键名。
    pub key: String,
    /// 投影后的短值（布尔 / 数字 / 短枚举 / `N 项`）。
    pub value: String,
}

/// 把设置包投影成「分组.键: 值」，只保留布尔 / 数字 / 短枚举；数组只给长度。
///
/// 跳过：数组（除 `excludedAccountIds` 给长度）、对象、路径 / URL / 长 id 字符串。
pub fn settings_facts(bundle: &Value) -> Vec<SettingFact> {
    let mut out = Vec::new();
    let Some(configs) = bundle.get("configs").and_then(Value::as_object) else {
        return out;
    };
    for (group, value) in configs {
        let Some(fields) = value.as_object() else {
            continue;
        };
        for (key, field) in fields {
            if let Some(short) = project_setting(key, field) {
                out.push(SettingFact {
                    group: group.clone(),
                    key: key.clone(),
                    value: short,
                });
            }
        }
    }
    out
}

/// 单个设置值的投影；返回 `None` 表示「跳过」。
///
/// 白名单口径：布尔 / 数字 / 短枚举；数组只在「明确是账号 id 列表」时给长度，
/// 其余数组、对象、路径、URL、长 id 一律跳过。
fn project_setting(key: &str, value: &Value) -> Option<String> {
    match value {
        Value::Bool(flag) => Some(if *flag { "true" } else { "false" }.to_string()),
        Value::Number(number) => Some(number.to_string()),
        Value::Array(items) => {
            if is_id_list(key) {
                Some(format!("{} 项", items.len()))
            } else {
                None
            }
        }
        Value::String(text) => project_short_enum(text),
        Value::Null => None,
        // 嵌套对象一律跳过：形状不可控，容易夹带 id / 路径。
        Value::Object(_) => None,
    }
}

/// 是否是「账号 id 列表」这类只该给长度的键（`excludedAccountIds` / `excluded_account_ids`）。
fn is_id_list(key: &str) -> bool {
    let lowered = key.to_ascii_lowercase();
    lowered.contains("ids") || lowered.contains("_id")
}

/// 短枚举字符串：长度 ≤ 24、无路径 / URL / 邮箱 / 长 id 特征才保留。
fn project_short_enum(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.len() > 24 {
        return None;
    }
    if trimmed.contains(['/', '\\', '@', ':', ' ']) {
        return None;
    }
    // 纯字母数字 + `_` / `-` 视为短枚举（如 `daily` / `ok` / `info`）。
    if trimmed
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
    {
        Some(trimmed.to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn token_kind_only_reads_type() {
        assert_eq!(token_kind(Some(&json!("plain-token"))), TokenKind::Plain);
        assert_eq!(
            token_kind(Some(&json!({"$wbEncrypted": "x", "envelope": "y"}))),
            TokenKind::Envelope
        );
        assert_eq!(token_kind(Some(&json!(""))), TokenKind::Missing);
        assert_eq!(token_kind(None), TokenKind::Missing);
    }

    #[test]
    fn access_state_uses_expires_at() {
        let now = 1_000_000i64;
        let valid = json!({"access_token": "tok", "expiresAt": now + 5 * 3_600_000i64});
        assert_eq!(
            access_state(&TokenKind::Plain, &valid, now),
            AccessState::Valid(5 * 3_600_000)
        );
        let expired = json!({"access_token": "tok", "expiresAt": now - 7_200_000i64});
        assert_eq!(
            access_state(&TokenKind::Plain, &expired, now),
            AccessState::Expired(7_200_000)
        );
        // 信封永远未知。
        assert_eq!(
            access_state(&TokenKind::Envelope, &expired, now),
            AccessState::Unknown
        );
    }

    #[test]
    fn account_fact_never_carries_identity() {
        let accounts = vec![
            json!({
                "access_token": "aaa", "refresh_token": "bbb",
                "expiresAt": 2_000_000i64, "credentialSource": "oauth",
                "variant": "cn", "email": "a@b.com", "uid": "u1", "nickname": "n"
            }),
            json!({
                "access_token": {"$wbEncrypted": "z"}, "variant": "ai"
            }),
        ];
        let facts = account_facts(&accounts, 1_000_000);
        assert_eq!(facts.len(), 2);
        assert_eq!(facts[0].index, 1);
        assert_eq!(facts[0].variant, "国内版");
        assert_eq!(facts[0].source, "oauth");
        assert!(!facts[0].source_inferred);
        assert_eq!(facts[0].token_kind, TokenKind::Plain);
        assert_eq!(facts[1].index, 2);
        assert_eq!(facts[1].variant, "国际版");
        assert_eq!(facts[1].token_kind, TokenKind::Envelope);
        // 结构里没有任何身份字段。
        let debug = format!("{:?}", facts);
        assert!(!debug.contains("a@b.com"));
        assert!(!debug.contains("u1"));
        assert!(!debug.contains("aaa"));
    }

    #[test]
    fn settings_skip_arrays_ids_paths_but_keep_scalars() {
        let bundle = json!({"configs": {
            "checkin": {"enabled": true, "start_hour": 8, "mode": "daily"},
            "travel": {"note": "C:/secret/path", "url": "https://x.com/a", "enabled": false},
            "rotate": {"excludedAccountIds": ["a", "b", "c"], "threshold": 5},
            "limits": {"tags": ["x", "y"], "nested": {"id": "abc"}},
        }});
        let facts = settings_facts(&bundle);
        let get = |g: &str, k: &str| {
            facts
                .iter()
                .find(|f| f.group == g && f.key == k)
                .map(|f| f.value.clone())
        };
        assert_eq!(get("checkin", "enabled"), Some("true".into()));
        assert_eq!(get("checkin", "start_hour"), Some("8".into()));
        assert_eq!(get("checkin", "mode"), Some("daily".into()));
        assert_eq!(get("travel", "enabled"), Some("false".into()));
        // 账号 id 列表只给长度（R6-09）。
        assert_eq!(get("rotate", "excludedAccountIds"), Some("3 项".into()));
        assert_eq!(get("rotate", "threshold"), Some("5".into()));
        // 路径 / URL / 普通数组 / 嵌套对象一律跳过。
        assert!(get("travel", "note").is_none());
        assert!(get("travel", "url").is_none());
        assert!(get("limits", "tags").is_none());
        assert!(get("limits", "nested").is_none());
    }

    #[test]
    fn credential_events_reads_tail_and_tolerates_corruption() {
        let dir = std::env::temp_dir().join(format!(
            "wb-shift-events-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("credential-events.jsonl");
        let mut lines = Vec::new();
        for i in 0..5 {
            lines.push(format!(
                r#"{{"ts":{},"account":1,"variant":"cn","action":"refreshed","reason":"ok"}}"#,
                1000 + i
            ));
        }
        lines.insert(2, "not json".to_string());
        std::fs::write(&path, lines.join("\n")).expect("write");
        let events = read_credential_events(&path, 3);
        assert_eq!(events.len(), 3);
        assert_eq!(events.last().unwrap().ts, 1004);
        let counts = outcome_counts(&events);
        assert_eq!(counts.iter().find(|(n, _)| *n == "refreshed").unwrap().1, 3);
        // 缺失 → 空。
        assert!(read_credential_events(&dir.join("nope.jsonl"), 3).is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn link_facts_counts_groups() {
        let dir = std::env::temp_dir().join(format!(
            "wb-shift-links-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("session_links.json");
        std::fs::write(
            &path,
            serde_json::to_string(&json!({"groups": [
                {"members": [
                    {"state": "active", "variant": "cn"},
                    {"state": "lost", "variant": "ai"},
                    {"state": "active", "variant": "cn"}
                ]},
                {"members": [{"state": "active", "variant": "cn"}]}
            ]}))
            .unwrap(),
        )
        .expect("write");
        let facts = read_link_json(&path).expect("some");
        assert_eq!(facts.groups, 2);
        assert_eq!(facts.members, 4);
        assert_eq!(facts.active, 3);
        assert_eq!(facts.lost, 1);
        assert_eq!(facts.large_groups, 1);
        assert_eq!(facts.cross_variant_groups, 1);
        std::fs::remove_dir_all(&dir).ok();
    }
}
