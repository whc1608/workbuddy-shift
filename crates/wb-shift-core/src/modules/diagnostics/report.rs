//! 诊断报告：把白名单事实拼成一段可读文本，并提供 `.txt` 落盘。
//!
//! 两个入口：
//!
//! - [`build_report`]（纯函数式装配，事实全部来自 [`ReportSources`]）——单测用它喂临时
//!   `error.log` 与手写事实，全程不碰用户目录；
//! - [`build_report_default`]（生产入口，两个宿主共用）——用 [`default_sources`] 读真实路径。
//!
//! 装配完之后**整体再过一遍 [`redact`]**：动态内容先在装配阶段脱敏一次，整段再兜一次，
//! 这样即使将来有人在某个分段里塞了新字段，也不会因为忘了脱敏而漏出去。替换次数取自
//! 这最后一遍，写进报告头让用户确认「确实脱敏过」。
//!
//! 分段顺序**固定**（便于 diff）：时间 / 环境 / WorkBuddy / 账号 / 凭据健康 / 刷新与保活 /
//! 客户端 / 限额 hook / 会话关联 / 统计与自动化 / 设置摘要 / 凭据事件 / 最近错误日志 /
//! 诊断生成。

use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::modules::config::{atomic_write, now_ms, store_dir};
use crate::modules::error_log::{error_log_path, ERROR_LOG_MAX_LINES};

use super::facts;
use super::redact::redact;

/// 允许落盘的扩展名（小写比较）。
pub const REPORT_EXTENSIONS: [&str; 2] = ["txt", "md"];

/// 诊断包体积上限（4 MiB）。超过时截断日志段并在报告里注明。
pub const MAX_REPORT_BYTES: usize = 4 * 1024 * 1024;

/// 报告里默认回看的日志条数（R6-09 定为 50）。
pub const DEFAULT_LOG_LINES: usize = 50;

/// 单条 detail 在报告里最多保留的行数。
pub const LOG_DETAIL_LINES: usize = 6;

/// 报告里保留的凭据事件条数（R6-03 定为最近 20 条）。
pub const CREDENTIAL_EVENT_LINES: usize = 20;

/// 单 probe 超时（写死，不暴露配置）。
pub const PROBE_TIMEOUT_MS: u64 = 800;

/// 宿主侧探测缓存时长（写死，不暴露配置）。
pub const PROBE_CACHE_TTL_MS: i64 = 30_000;

/// 宿主形态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostForm {
    /// 桌面 App（Tauri）。
    Desktop,
    /// 浏览器里的 WebUI。
    WebUi,
    /// 只跑后端的命令行形态。
    Cli,
}

impl HostForm {
    /// 写进报告的稳定取值。
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Desktop => "desktop",
            Self::WebUi => "webui",
            Self::Cli => "cli",
        }
    }

    /// 报告里给人看的那一行。
    pub fn label(self) -> &'static str {
        match self {
            Self::Desktop => "桌面版（Tauri）",
            Self::WebUi => "网页版（浏览器访问本机服务）",
            Self::Cli => "命令行 / 仅服务端",
        }
    }
}

/// 客户端探测状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProbeStatus {
    /// 正常返回。
    #[default]
    Ok,
    /// 超时（> [`PROBE_TIMEOUT_MS`]）。
    Timeout,
    /// 探测过程出错（含 panic）。
    Error,
}

impl ProbeStatus {
    /// 写进报告的稳定取值。
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Timeout => "timeout",
            Self::Error => "error",
        }
    }

    /// 报告里给人看的那一段。
    pub fn label(self) -> &'static str {
        match self {
            Self::Ok => "正常",
            Self::Timeout => "探测超时（>800ms）",
            Self::Error => "探测失败",
        }
    }
}

/// 单个客户端的探测结果（**只有**装没装 / 版本 / 运行态 / 探测状态，不含路径与账号）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientProbe {
    /// 客户端显示名（固定文案，来自宿主）。
    pub name: String,
    /// 是否检测到该客户端。
    pub installed: bool,
    /// 客户端版本（取不到时 `None`）。
    pub version: Option<String>,
    /// 是否在运行（`None` = 未探测）。
    pub running: Option<bool>,
    /// 探测状态。
    pub probe_status: ProbeStatus,
    /// 探测耗时毫秒。
    pub elapsed_ms: Option<u64>,
}

impl ClientProbe {
    /// 新建一条「正常」探测结果（旧测试与简单场景可用）。
    pub fn new(name: impl Into<String>, installed: bool, version: Option<String>) -> Self {
        Self {
            name: name.into(),
            installed,
            version,
            running: None,
            probe_status: ProbeStatus::Ok,
            elapsed_ms: None,
        }
    }
}

/// 限额 hook 的单个注册目标。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookTarget {
    /// 目标标签（固定文案）。
    pub label: String,
    /// 该处配置是否存在。
    pub exists: bool,
    /// 是否已注册本工具 hook。
    pub installed: bool,
}

/// 限额 hook 的安装状态（只保留布尔与计数 / 标签，不保留任何路径）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HookState {
    /// 脚本与至少一处配置都就绪。
    pub installed: bool,
    /// 已注册该 hook 的客户端配置处数。
    pub registered_targets: usize,
    /// 探测到的客户端配置总处数。
    pub total_targets: usize,
    /// 脚本文件是否存在。
    pub script_exists: bool,
    /// 各注册目标。
    pub targets: Vec<HookTarget>,
}

/// 报告需要的外部事实（宿主层填）。**没有任何机密字段**。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportInput {
    /// 宿主形态。
    pub host_form: HostForm,
    /// 界面语言（`zh-CN` / `en-US` …）。
    pub locale: String,
    /// 账号库里的账号条数。
    pub account_count: usize,
    /// 各客户端探测结果。
    pub clients: Vec<ClientProbe>,
}

impl ReportInput {
    /// 只带必填项的最小构造（客户端列表为空）。
    pub fn new(host_form: HostForm, locale: impl Into<String>, account_count: usize) -> Self {
        Self {
            host_form,
            locale: locale.into(),
            account_count,
            clients: Vec::new(),
        }
    }
}

/// 单个 probe 的耗时与状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeTiming {
    /// probe 名称。
    pub name: String,
    /// 状态。
    pub status: ProbeStatus,
    /// 耗时毫秒。
    pub elapsed_ms: Option<u64>,
}

/// `[诊断生成]` 段的元数据（宿主计时 + 装配层回填）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GenerationFacts {
    /// 事实采集总耗时（毫秒）。
    pub total_ms: u64,
    /// 各 probe 耗时 / 状态。
    pub probes: Vec<ProbeTiming>,
    /// 是否命中 30s 缓存。
    pub cache_used: bool,
    /// 超时项名称。
    pub timeouts: Vec<String>,
}

/// 装配所需的结构化事实（全部由 [`facts`] 投影，非机密）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ReportFacts {
    /// 账号事实。
    pub accounts: Vec<facts::AccountFact>,
    /// 账号库体量。
    pub account_store: Option<facts::AccountStoreFact>,
    /// WorkBuddy 本体（每档位一条）。
    pub workbuddy: Vec<facts::WorkBuddyFacts>,
    /// 会话关联。
    pub link: Option<facts::LinkFacts>,
    /// 统计与自动化。
    pub stats: Option<facts::StatsFacts>,
    /// 设置摘要。
    pub settings: Vec<facts::SettingFact>,
    /// 凭据配置（`credential.json`）。
    pub credential_config: Option<facts::CredentialConfigFacts>,
    /// 凭据事件。
    pub credential_events: Vec<facts::CredentialEvent>,
    /// 生成元数据。
    pub generation: GenerationFacts,
}

/// 报告的数据来源。
#[derive(Debug, Clone, Default)]
pub struct ReportSources {
    /// `error.log` 的路径。
    pub error_log: PathBuf,
    /// 回看多少条日志。
    pub error_log_limit: usize,
    /// 限额 hook 状态；`None` 表示读不到。
    pub hook: Option<HookState>,
    /// 工具数据目录是否**疑似**可写；`None` = 未探测。
    pub store_writable: Option<bool>,
    /// 生成时间（毫秒）。
    pub generated_at_ms: i64,
    /// 本地时区偏移（秒）；生产路径用 `Local::now().offset()`。
    pub utc_offset_seconds: i32,
    /// 结构化事实。
    pub facts: ReportFacts,
}

/// 生产来源：真实数据目录 + 真实设置 + 真实 hook 状态（不做客户端探测，客户端由宿主传入）。
pub fn default_sources() -> ReportSources {
    collect_cheap_sources(
        now_ms(),
        local_offset_seconds(),
        read_hook_state(),
        default_workbuddy_facts(),
        GenerationFacts::default(),
    )
}

/// 采集「便宜段」事实（账号 / 设置 / 日志路径 / 关联 / 统计 / 凭据事件）；
/// 客户端探测、hook、WorkBuddy 事实由调用方传入（可缓存）。
pub fn collect_cheap_sources(
    generated_at_ms: i64,
    utc_offset_seconds: i32,
    hook: Option<HookState>,
    workbuddy: Vec<facts::WorkBuddyFacts>,
    generation: GenerationFacts,
) -> ReportSources {
    // 账号只读一次：`account_facts` 与 `account_store_fact` 共用同一份快照，
    // 避免两次 `load_accounts()` 之间账号库被改动导致计数与列表对不上。
    let accounts = crate::modules::account::load_accounts();
    let mut account_facts_list = facts::account_facts(&accounts, generated_at_ms);
    let credential_events = facts::credential_events_recent(CREDENTIAL_EVENT_LINES);
    facts::attach_account_events(&mut account_facts_list, &credential_events, generated_at_ms);
    let bundle = crate::modules::config::export_settings_bundle();
    ReportSources {
        error_log: error_log_path(),
        error_log_limit: DEFAULT_LOG_LINES,
        hook,
        store_writable: probe_store_writable(),
        generated_at_ms,
        utc_offset_seconds,
        facts: ReportFacts {
            accounts: account_facts_list,
            account_store: Some(facts::account_store_fact(
                &crate::modules::config::accounts_file(),
                accounts.len(),
            )),
            workbuddy,
            link: facts::link_facts(),
            stats: Some(facts::stats_facts(generated_at_ms)),
            settings: facts::settings_facts(&bundle),
            credential_config: facts::credential_config_facts(),
            credential_events,
            generation,
        },
    }
}

/// 两档位的 WorkBuddy 事实（生产路径）。
fn default_workbuddy_facts() -> Vec<facts::WorkBuddyFacts> {
    ["cn", "ai"]
        .iter()
        .filter_map(|variant| facts::workbuddy_facts(variant))
        .collect()
}

/// 用生产来源生成报告（不做并行探测；宿主应自行并行探测后走 [`build_report`]）。
pub fn build_report_default(input: &ReportInput) -> String {
    build_report(input, &default_sources())
}

/// 把白名单事实装配成一段可读文本（纯函数，不做 IO 之外的决定）。
///
/// 超过 [`MAX_REPORT_BYTES`] 时**丢掉错误日志段**再装配一次，并在报告里注明截断，
/// 保证落盘（`.txt` 有 4 MiB 上限）永远不会因为日志过长而失败。
pub fn build_report(input: &ReportInput, sources: &ReportSources) -> String {
    let full = assemble_report(input, sources, false);
    if full.len() <= MAX_REPORT_BYTES {
        return full;
    }
    assemble_report(input, sources, true)
}

/// 装配主体；`omit_log` 为真时跳过错误日志段（体积兜底路径）。
fn assemble_report(input: &ReportInput, sources: &ReportSources, omit_log: bool) -> String {
    let facts = &sources.facts;
    let offset = sources.utc_offset_seconds;
    let mut body: Vec<String> = Vec::new();

    // ---- [时间] ----
    push_section(&mut body, "时间");
    body.push(format!(
        "  本地时间: {}",
        format_timestamp_with_offset(sources.generated_at_ms, offset)
    ));
    body.push(format!("  UTC: {}", format_utc(sources.generated_at_ms)));
    body.push(format!("  时区偏移: {}", format_offset(offset)));

    // ---- [环境] ----
    push_section(&mut body, "环境");
    body.push(format!(
        "  应用版本: {}",
        crate::modules::update::APP_VERSION
    ));
    body.push(format!("  构建 profile: {}", build_profile()));
    body.push(format!("  宿主形态: {}", input.host_form.label()));
    body.push(format!("  宿主标识: {}", input.host_form.as_wire()));
    body.push(format!("  界面语言: {}", input.locale));
    body.push(format!(
        "  操作系统: {} / {}",
        std::env::consts::OS,
        std::env::consts::ARCH
    ));
    body.push(format!(
        "  数据目录名: {}",
        store_dir()
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "-".to_string())
    ));
    body.push(format!(
        "  工具数据目录: {}",
        match sources.store_writable {
            Some(true) => "存在（未检测到只读标记）",
            Some(false) => "不可写（可能是权限 / 占用 / 安全软件拦截）",
            None => "未探测",
        }
    ));

    // ---- [WorkBuddy] ----
    push_section(&mut body, "WorkBuddy");
    if facts.workbuddy.is_empty() {
        body.push("  （未探测）".to_string());
    } else {
        for wb in &facts.workbuddy {
            body.push(format!(
                "  {}: 安装={} 运行={} 版本={} 认证文件={}",
                wb.variant,
                yes_no(wb.installed),
                yes_no(wb.running),
                wb.version
                    .as_deref()
                    .filter(|v| !v.trim().is_empty())
                    .unwrap_or("版本未知"),
                workbuddy_auth_label(wb),
            ));
            body.push(format!(
                "    token={} access={}",
                wb.token_kind,
                workbuddy_access_label(wb)
            ));
        }
    }

    // ---- [账号] ----
    push_section(&mut body, "账号");
    body.push(format!("  账号数量: {} 个", input.account_count));
    if let Some(store) = &facts.account_store {
        body.push(format!(
            "  账号库: 大小={} 备份={} 个",
            store
                .size_bytes
                .map(|b| format!("{b} B"))
                .unwrap_or_else(|| "读取失败".to_string()),
            store.backups
        ));
    }

    // ---- [客户端] ----
    push_section(&mut body, "客户端");
    // WorkBuddy 本体也算「客户端」：用户最想看的就是「是不是 5.6」。
    for wb in &facts.workbuddy {
        let version = wb
            .version
            .as_deref()
            .filter(|v| !v.trim().is_empty())
            .map(|v| format!("（版本 {v}）"))
            .unwrap_or_else(|| "（版本未知）".to_string());
        body.push(format!(
            "WorkBuddy（{}）: {}{} 运行={}",
            wb.variant,
            if wb.installed {
                "已安装"
            } else {
                "未安装"
            },
            version,
            yes_no(wb.running)
        ));
    }
    if input.clients.is_empty() {
        body.push("  （未提供探测结果）".to_string());
    } else {
        for probe in &input.clients {
            body.push(format!("  {}", client_line(probe)));
        }
    }

    // ---- [限额 hook] ----
    push_section(&mut body, "限额 hook");
    match &sources.hook {
        Some(hook) => {
            body.push(format!("  已安装: {}", yes_no(hook.installed)));
            body.push(format!("  脚本存在: {}", yes_no(hook.script_exists)));
            body.push(format!(
                "  已注册配置: {}/{} 处",
                hook.registered_targets, hook.total_targets
            ));
            for target in &hook.targets {
                body.push(format!(
                    "    {}: 存在={} 已注册={}",
                    target.label,
                    yes_no(target.exists),
                    yes_no(target.installed)
                ));
            }
        }
        None => body.push("  状态: 读取失败".to_string()),
    }

    // ---- [会话关联] ----
    push_section(&mut body, "会话关联");
    match &facts.link {
        Some(link) => {
            body.push(format!("  组数: {}", link.groups));
            body.push(format!(
                "  成员: {}（active {} / lost {}）",
                link.members, link.active, link.lost
            ));
            body.push(format!("  跨档组: {}", link.cross_variant_groups));
            body.push(format!("  3+ 成员组: {}", link.large_groups));
        }
        None => body.push("  无记录".to_string()),
    }

    // ---- [统计与自动化] ----
    push_section(&mut body, "统计与自动化");
    match &facts.stats {
        Some(stats) => push_stats(&mut body, stats),
        None => body.push("  读取失败".to_string()),
    }

    // ---- [设置摘要] ----
    push_section(&mut body, "设置摘要");
    push_settings_section(&mut body, facts);

    // ---- [凭据事件] ----
    push_section(&mut body, "凭据事件");
    if facts.credential_events.is_empty() {
        body.push("  无记录".to_string());
    } else {
        for event in &facts.credential_events {
            // 节流窗口内的重复被合并成一行 `count`；`×N` 只在真的压过时才出现。
            let repeat = if event.count >= 2 {
                format!(" ×{}", event.count)
            } else {
                String::new()
            };
            body.push(format!(
                "  #{} {} {}/{}{repeat}",
                event.account,
                format_timestamp_with_offset(event.ts, offset),
                event.action,
                event.reason
            ));
        }
    }

    // ---- [凭据健康] ----
    push_section(&mut body, "凭据健康");
    push_credential_health(&mut body, facts);

    // ---- [刷新与保活] ----
    push_section(&mut body, "刷新与保活");
    push_refresh_section(&mut body, facts, sources.generated_at_ms);

    // ---- [最近错误日志] ----
    push_section(&mut body, "最近错误日志");
    if omit_log {
        body.push(format!(
            "  （报告已超过 {} MiB 上限，本段整体省略；请用「重新生成」前的较短日志复现）",
            MAX_REPORT_BYTES / 1024 / 1024
        ));
    } else {
        body.push(format!(
            "  回看: 最近 {} 条（按 kind+message 聚合）",
            sources.error_log_limit
        ));
    }
    let log_lines_read = if omit_log {
        0
    } else {
        push_error_log(
            &mut body,
            &sources.error_log,
            sources.error_log_limit,
            offset,
        )
    };

    // [诊断生成] 里的「脱敏替换数」必须是真实数字，因此先对已完成的分段脱敏一次拿到
    // 计数，再追加生成段，最后整体再兜一遍（第二遍已无可替换片段，计数幂等）。
    let pre_redacted = redact(&body.join("\n"));
    push_section(&mut body, "诊断生成");
    push_generation(
        &mut body,
        &facts.generation,
        pre_redacted.text.len(),
        log_lines_read,
        pre_redacted.replacements,
        &input.clients,
    );

    let redacted = redact(&body.join("\n"));
    let mut header = String::new();
    header.push_str("WorkBuddy Shift 诊断包\n");
    header.push_str(&format!(
        "生成时间: {}（{}）\n",
        format_timestamp_with_offset(sources.generated_at_ms, offset),
        format_utc(sources.generated_at_ms)
    ));
    header.push_str(&format!(
        "脱敏: 已替换 {} 处敏感片段\n",
        pre_redacted.replacements
    ));
    header.push_str(
        "说明: 本文件不含账号、令牌、会话内容、项目名与原始路径；路径 / UUID / URL 路径 /\n\
         邮箱 / 令牌 / 用户名 / 长数字已替换为占位符。可整段粘贴到 GitHub issue。\n",
    );
    header.push('\n');
    format!("{header}{}\n", redacted.text)
}

/// 推入一个分段标题（前面空一行，首段不加）。
fn push_section(body: &mut Vec<String>, title: &str) {
    if !body.is_empty() {
        body.push(String::new());
    }
    body.push(format!("[{title}]"));
}

fn yes_no(flag: bool) -> &'static str {
    if flag {
        "是"
    } else {
        "否"
    }
}

fn build_profile() -> &'static str {
    if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    }
}

/// WorkBuddy 认证文件一行（不含路径/账号名）。
fn workbuddy_auth_label(wb: &facts::WorkBuddyFacts) -> String {
    if !wb.auth_present {
        return "不存在".to_string();
    }
    if !wb.auth_parseable {
        return "不可解析".to_string();
    }
    let size = wb
        .auth_bytes
        .map(|b| format!("{b} B"))
        .unwrap_or_else(|| "-".to_string());
    let count = wb
        .account_count
        .map(|c| format!("{c} 条"))
        .unwrap_or_else(|| "-".to_string());
    format!("存在（{size}，{count}）")
}

/// WorkBuddy 认证文件里的 access 是否可判（信封 / 缺失都只能写「未知」）。
fn workbuddy_access_label(wb: &facts::WorkBuddyFacts) -> String {
    if !wb.auth_present {
        return "未知".to_string();
    }
    match wb.token_kind.as_str() {
        "明文" => "明文".to_string(),
        // 信封 / 混合：token 存在但不可直接判到期，写「未知」并带上形态。
        other => format!("未知（{other}）"),
    }
}

fn client_line(probe: &ClientProbe) -> String {
    let installed = if probe.installed {
        "已安装"
    } else {
        "未安装"
    };
    let version = probe
        .version
        .as_deref()
        .filter(|v| !v.trim().is_empty())
        .map(|v| format!("（版本 {v}）"))
        .unwrap_or_default();
    let running = match probe.running {
        Some(true) => " 运行=是",
        Some(false) => " 运行=否",
        None => "",
    };
    let status = if probe.probe_status == ProbeStatus::Ok {
        String::new()
    } else {
        format!(" [{}]", probe.probe_status.label())
    };
    let elapsed = probe
        .elapsed_ms
        .map(|ms| format!(" {ms}ms"))
        .unwrap_or_default();
    format!(
        "{}: {installed}{version}{running}{status}{elapsed}",
        probe.name
    )
}

/// `[凭据健康]`：每个账号一行，**不含 token / 路径 / 昵称 / 邮箱 / uid**。
///
/// 行格式固定：`#1 国内版 来源=oauth token=明文 access=有效(剩余 5h) refresh=有
/// 需重登=否 最近刷新=refreshed(2h 前)`。
/// 账号库没有 `credentialSource` 时来源是推断出来的，追加中文后缀：`来源=local（推断）`。
fn push_credential_health(body: &mut Vec<String>, facts: &ReportFacts) {
    if facts.accounts.is_empty() {
        body.push("  （没有账号）".to_string());
        return;
    }
    for acc in &facts.accounts {
        let source = if acc.source_inferred {
            format!("{}（推断）", acc.source)
        } else {
            acc.source.clone()
        };
        let refresh_label = if acc.refresh_expired {
            format!("{}(已过期)", acc.refresh.refresh_label())
        } else {
            acc.refresh.refresh_label().to_string()
        };
        let mut line = format!(
            "  #{} {} 来源={} token={} access={} refresh={} 需重登={}",
            acc.index,
            acc.variant,
            source,
            acc.token_kind.label(),
            acc.access.label(),
            refresh_label,
            acc.relogin.label(),
        );
        if let Some(event) = &acc.last_event {
            line.push_str(&format!(" 最近刷新={event}"));
        }
        body.push(line);
        let mut extra: Vec<String> = Vec::new();
        if acc.enterprise {
            extra.push("企业版".to_string());
        }
        if acc.has_note {
            extra.push("有备注".to_string());
        }
        if let Some(age) = acc.age_ms {
            extra.push(format!("创建于 {} 前", facts::humanize_span(age)));
        }
        if !extra.is_empty() {
            body.push(format!("    {}", extra.join(" ")));
        }
    }
}

fn push_refresh_section(body: &mut Vec<String>, facts: &ReportFacts, now: i64) {
    let events = &facts.credential_events;
    let latest = events.iter().map(|e| e.ts).max();
    match latest {
        Some(ts) => body.push(format!(
            "  最近一轮保活: {} 前",
            facts::humanize_span(now - ts)
        )),
        None => body.push("  最近一轮保活: 未知".to_string()),
    }
    let counts = facts::outcome_counts(events);
    let counts_line = counts
        .iter()
        .map(|(name, count)| format!("{name}={count}"))
        .collect::<Vec<_>>()
        .join(" ");
    body.push(format!("  outcome 计数: {counts_line}"));
    // 惰性刷新时间没有独立事件来源，只能写未知。
    body.push("  最近一次惰性刷新: 未知".to_string());
}

fn push_stats(body: &mut Vec<String>, stats: &facts::StatsFacts) {
    body.push(format!(
        "  官方用量: 状态={} 账号数={}",
        stats.official_status.as_deref().unwrap_or("未知"),
        stats
            .official_accounts
            .map(|n| n.to_string())
            .unwrap_or_else(|| "未知".to_string())
    ));
    body.push(format!(
        "  本地账本: 状态={} 行数={}",
        stats.ledger_status.as_deref().unwrap_or("未知"),
        stats
            .ledger_rows
            .map(|n| n.to_string())
            .unwrap_or_else(|| "未知".to_string())
    ));
    body.push(match stats.checkin_last_ms {
        Some(_) => format!(
            "  签到: 今天 {} 个账号（最近一轮见时间）",
            stats.checkin_today.unwrap_or(0)
        ),
        None => format!(
            "  签到: 未知（今日 {} 个）",
            stats.checkin_today.unwrap_or(0)
        ),
    });
    body.push("  旅行 / 自动轮换 / 每日活跃: 未知".to_string());
}

fn push_settings_section(body: &mut Vec<String>, facts: &ReportFacts) {
    if facts.settings.is_empty() {
        body.push("  （没有可导出的配置）".to_string());
    } else {
        for setting in &facts.settings {
            body.push(format!(
                "  {}.{}: {}",
                setting.group, setting.key, setting.value
            ));
        }
    }
    match &facts.credential_config {
        Some(cfg) => {
            body.push(format!(
                "  credential.decryptReadOnly: {}",
                cfg.decrypt_read_only
            ));
            body.push(format!("  credential.decryptWrite: {}", cfg.decrypt_write));
            body.push(format!(
                "  credential.firstPromptShown: {}",
                cfg.first_prompt_shown
            ));
            body.push(format!(
                "  credential.platformProbe: {}",
                cfg.platform_probe
            ));
            if let Some(version) = &cfg.platform_probe_version {
                body.push(format!("  credential.platformProbeVersion: {version}"));
            }
        }
        None => body.push("  credential: 未配置".to_string()),
    }
}

fn push_generation(
    body: &mut Vec<String>,
    generation: &GenerationFacts,
    report_bytes: usize,
    log_lines_read: usize,
    redactions: usize,
    clients: &[ClientProbe],
) {
    body.push(format!("  事实采集耗时: {}ms", generation.total_ms));
    body.push(format!(
        "  缓存: {}",
        if generation.cache_used {
            "命中（30s 内）"
        } else {
            "未命中"
        }
    ));
    let timeouts: Vec<String> = if generation.timeouts.is_empty() {
        clients
            .iter()
            .filter(|c| c.probe_status == ProbeStatus::Timeout)
            .map(|c| c.name.clone())
            .collect()
    } else {
        generation.timeouts.clone()
    };
    body.push(format!(
        "  超时项: {}",
        if timeouts.is_empty() {
            "无".to_string()
        } else {
            timeouts.join(" / ")
        }
    ));
    for timing in &generation.probes {
        body.push(format!(
            "    probe {}: {} {}",
            timing.name,
            timing.status.as_wire(),
            timing
                .elapsed_ms
                .map(|ms| format!("{ms}ms"))
                .unwrap_or_else(|| "-".to_string())
        ));
    }
    if generation.probes.is_empty() {
        for probe in clients {
            body.push(format!(
                "    probe {}: {} {}",
                probe.name,
                probe.probe_status.as_wire(),
                probe
                    .elapsed_ms
                    .map(|ms| format!("{ms}ms"))
                    .unwrap_or_default()
            ));
        }
    }
    body.push(format!("  报告字节数(约): {report_bytes}"));
    body.push(format!("  读取日志行数: {log_lines_read}"));
    body.push(format!("  脱敏替换数: {redactions}"));
}

/// 账号数量的区间分桶（保留旧入口，报告里仍用到）。
pub fn account_bucket(count: usize) -> &'static str {
    match count {
        0 => "0",
        1..=5 => "1-5",
        6..=10 => "6-10",
        11..=20 => "11-20",
        21..=50 => "21-50",
        _ => "50+",
    }
}

/// 日志聚合累加器。
struct LogAgg {
    kind: String,
    message: String,
    count: usize,
    last_ts: i64,
    details: Vec<String>,
}

/// 追加最近若干条错误日志（按 `(kind,message)` 聚合）；返回读取的日志行数。
fn push_error_log(body: &mut Vec<String>, path: &Path, limit: usize, offset: i32) -> usize {
    let Ok(text) = std::fs::read_to_string(path) else {
        body.push("  （没有日志文件，或读不出来）".to_string());
        return 0;
    };
    let lines: Vec<&str> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    if lines.is_empty() {
        body.push("  （日志为空）".to_string());
        return 0;
    }
    let limit = limit.clamp(1, ERROR_LOG_MAX_LINES);
    let start = lines.len().saturating_sub(limit);
    let tail = &lines[start..];
    let mut order: Vec<String> = Vec::new();
    let mut groups: HashMap<String, LogAgg> = HashMap::new();
    for line in tail {
        // 只有 JSON **对象** 才当成结构化日志；纯字符串等形状按原文成组，
        // 否则会出现「kind=backend、message 为空、时间未知」的空壳行。
        let parsed = serde_json::from_str::<Value>(line)
            .ok()
            .filter(Value::is_object);
        let (kind, message, ts, detail) = match parsed {
            Some(entry) => (
                entry
                    .get("kind")
                    .and_then(Value::as_str)
                    .unwrap_or("backend")
                    .to_string(),
                entry
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                entry.get("ts").and_then(Value::as_i64).unwrap_or(0),
                entry
                    .get("detail")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
            ),
            // 非 JSON 行：单独成组，原文入 message（脱敏仍兜一遍）。
            // 纯 JSON 字符串行去掉外层引号，粘贴进 issue 才像日志。
            None => (
                "raw".to_string(),
                serde_json::from_str::<String>(line).unwrap_or_else(|_| line.trim().to_string()),
                0,
                String::new(),
            ),
        };
        let key = format!("{kind}\u{0}{message}");
        let agg = groups.entry(key.clone()).or_insert_with(|| {
            order.push(key.clone());
            LogAgg {
                kind,
                message,
                count: 0,
                last_ts: 0,
                details: Vec::new(),
            }
        });
        agg.count += 1;
        agg.last_ts = agg.last_ts.max(ts);
        if agg.details.len() < LOG_DETAIL_LINES {
            for detail_line in detail.lines().take(LOG_DETAIL_LINES) {
                if detail_line.trim().is_empty() {
                    continue;
                }
                if agg.details.len() >= LOG_DETAIL_LINES {
                    break;
                }
                agg.details.push(detail_line.to_string());
            }
        }
    }
    let now = now_ms();
    for key in order {
        let Some(agg) = groups.get(&key) else {
            continue;
        };
        let stamp = if agg.last_ts > 0 {
            format!(
                "{}（{} 前）",
                format_timestamp_with_offset(agg.last_ts, offset),
                facts::humanize_span(now - agg.last_ts)
            )
        } else {
            "时间未知".to_string()
        };
        let count_suffix = if agg.count > 1 {
            format!(" ×{}", agg.count)
        } else {
            String::new()
        };
        body.push(format!(
            "  [{}] {} {}{}",
            agg.kind, agg.message, stamp, count_suffix
        ));
        for detail in &agg.details {
            body.push(format!("      {detail}"));
        }
    }
    tail.len()
}

// ---------------------------------------------------------------------------
// 时间格式化（R6-04）
// ---------------------------------------------------------------------------

/// 毫秒时间戳 → `YYYY-MM-DD HH:MM:SS +08:00`（**纯函数**，不读环境时区）。
///
/// 生产路径传 [`local_offset_seconds`]；单测传固定 offset，不依赖 CI 时区。
pub fn format_timestamp_with_offset(ms: i64, offset_seconds: i32) -> String {
    match chrono::DateTime::from_timestamp_millis(ms) {
        Some(utc) => {
            let offset = chrono::FixedOffset::east_opt(offset_seconds).unwrap_or_else(|| {
                chrono::FixedOffset::east_opt(0).expect("zero offset always valid")
            });
            utc.with_timezone(&offset)
                .format("%Y-%m-%d %H:%M:%S %:z")
                .to_string()
        }
        None => format!("{ms}ms"),
    }
}

/// 毫秒时间戳 → `YYYY-MM-DD HH:MM:SS UTC`。
pub fn format_utc(ms: i64) -> String {
    match chrono::DateTime::from_timestamp_millis(ms) {
        Some(stamp) => stamp.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
        None => format!("{ms}ms"),
    }
}

/// 当前本地时区偏移（秒）。
pub fn local_offset_seconds() -> i32 {
    chrono::Local::now().offset().local_minus_utc()
}

/// `+08:00` / `-05:00` 形态。
fn format_offset(offset_seconds: i32) -> String {
    let sign = if offset_seconds < 0 { '-' } else { '+' };
    let abs = offset_seconds.abs();
    let hours = abs / 3600;
    let minutes = (abs % 3600) / 60;
    format!("{sign}{hours:02}:{minutes:02}")
}

// ---------------------------------------------------------------------------
// 并行客户端探测（R6-10）
// ---------------------------------------------------------------------------

/// 一个待探测的客户端：名称 + 返回 `status()` 载荷的闭包。
pub struct ProbeSpec {
    /// 客户端显示名。
    pub name: String,
    /// 探测闭包（返回与 `status()` 同形状的 JSON）。
    pub probe: Box<dyn Fn() -> Value + Send>,
}

impl ProbeSpec {
    /// 构造一个探测规格。
    pub fn new<F>(name: impl Into<String>, probe: F) -> Self
    where
        F: Fn() -> Value + Send + 'static,
    {
        Self {
            name: name.into(),
            probe: Box::new(probe),
        }
    }
}

/// 并行探测客户端：每个 probe 一个线程，`mpsc::recv_timeout` 收集，单 probe 超时 `timeout_ms`。
///
/// 顺序与 `specs` 一致（先全部 spawn，再按序 recv），因此结果稳定可断言。
/// 超时写 [`ProbeStatus::Timeout`]，panic / 错误写 [`ProbeStatus::Error`]，**绝不传播**。
pub fn probe_clients(specs: Vec<ProbeSpec>, timeout_ms: u64) -> Vec<ClientProbe> {
    let mut receivers = Vec::with_capacity(specs.len());
    let names: Vec<String> = specs.iter().map(|s| s.name.clone()).collect();
    for spec in specs {
        let (tx, rx) = mpsc::channel::<Result<Value, ()>>();
        let probe = spec.probe;
        std::thread::spawn(move || {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(probe));
            let _ = tx.send(match outcome {
                Ok(value) => Ok(value),
                Err(_) => Err(()),
            });
        });
        receivers.push(rx);
    }
    let timeout = Duration::from_millis(timeout_ms);
    let mut out = Vec::with_capacity(names.len());
    for (name, rx) in names.into_iter().zip(receivers) {
        let started = Instant::now();
        match rx.recv_timeout(timeout) {
            Ok(Ok(value)) => {
                let mut probe = client_probe_from_status(&name, &value);
                probe.elapsed_ms = Some(started.elapsed().as_millis() as u64);
                out.push(probe);
            }
            Ok(Err(())) => {
                let mut probe = ClientProbe::new(name, false, None);
                probe.probe_status = ProbeStatus::Error;
                probe.elapsed_ms = Some(started.elapsed().as_millis() as u64);
                out.push(probe);
            }
            Err(_) => {
                let mut probe = ClientProbe::new(name, false, None);
                probe.probe_status = ProbeStatus::Timeout;
                probe.elapsed_ms = Some(timeout_ms);
                out.push(probe);
            }
        }
    }
    out
}

/// 从客户端 `status()` 载荷取「装没装 / 版本 / 运行态」（两个宿主共用）。
///
/// 各客户端形状不统一（`codebuddy_cli` 用 `configured`，其余用 `installed`，VS Code 插件
/// 另有 `extensionInstalled`），按固定优先级取第一个存在的布尔；取不到一律「未安装」。
pub fn client_probe_from_status(name: &str, status: &Value) -> ClientProbe {
    let installed = ["installed", "extensionInstalled", "configured"]
        .iter()
        .find_map(|key| status.get(*key).and_then(Value::as_bool))
        .unwrap_or(false);
    let version = status
        .get("version")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let running = status.get("running").and_then(Value::as_bool);
    ClientProbe {
        name: name.to_string(),
        installed,
        version,
        running,
        probe_status: ProbeStatus::Ok,
        elapsed_ms: None,
    }
}

/// 限额 hook 状态 → [`HookState`]（只留布尔 / 计数 / 标签，路径丢在解析这一步）。
fn read_hook_state() -> Option<HookState> {
    let status = crate::modules::rate_limit_hook::hook_status();
    hook_state_from_status(&status)
}

/// 从 `hook_status()` 载荷投影 [`HookState`]（两个宿主共用；可注入，便于单测）。
pub fn hook_state_from_status(status: &Value) -> Option<HookState> {
    let installed = status.get("installed").and_then(Value::as_bool)?;
    let script_exists = status
        .get("scriptExists")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let targets: Vec<HookTarget> = status
        .get("targets")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| HookTarget {
                    label: item
                        .get("label")
                        .and_then(Value::as_str)
                        .unwrap_or("-")
                        .to_string(),
                    exists: item.get("exists").and_then(Value::as_bool).unwrap_or(false),
                    installed: item
                        .get("installed")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                })
                .collect()
        })
        .unwrap_or_default();
    let total_targets = targets.len();
    let registered_targets = targets.iter().filter(|t| t.installed).count();
    Some(HookState {
        installed,
        registered_targets,
        total_targets,
        script_exists,
        targets,
    })
}

/// 工具数据目录状态：**纯只读判断，任何情况下都不创建 / 不修改 / 不删除文件**。
fn probe_store_writable() -> Option<bool> {
    let dir = store_dir();
    let meta = std::fs::metadata(&dir).ok()?;
    if !meta.is_dir() || meta.permissions().readonly() {
        return Some(false);
    }
    Some(std::fs::read_dir(&dir).is_ok())
}

/// 保存诊断文本到用户选定的路径（桌面宿主专用）。
pub fn save_report(path: &str, content: &str) -> Result<Value, String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("缺少保存路径".to_string());
    }
    let target = Path::new(trimmed);
    let extension = target
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_lowercase)
        .unwrap_or_default();
    if !REPORT_EXTENSIONS.contains(&extension.as_str()) {
        return Err(format!("只允许保存 .txt / .md 文件（收到 .{extension}）"));
    }
    let bytes = content.len();
    if bytes == 0 {
        return Err("诊断内容为空".to_string());
    }
    if bytes > MAX_REPORT_BYTES {
        return Err(format!(
            "内容超过 {} MiB 上限（{} 字节）",
            MAX_REPORT_BYTES / 1024 / 1024,
            bytes
        ));
    }
    if let Some(parent) = target.parent() {
        if !parent.as_os_str().is_empty() && !parent.is_dir() {
            return Err(format!("目标目录不存在：{}", parent.display()));
        }
    }
    atomic_write(target, content).map_err(|error| format!("写入失败：{error}"))?;
    Ok(serde_json::json!({
        "ok": true,
        "path": target.to_string_lossy(),
        "bytes": bytes,
        "savedAt": now_ms(),
    }))
}
