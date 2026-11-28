//! 诊断包：桌面端命令（生成报告 + 落盘）。
//!
//! # 为什么这一层只做「凑事实」
//!
//! `wb_shift_core::modules::diagnostics` 装配层只消费投影后的纯值事实（见该模块
//! `facts` 子模块）。这一层负责：
//!
//! - 并行探测 5 个客户端（[`diagnostics::probe_clients`]，单 probe 800ms 超时）；
//! - 30s 缓存客户端探测 / hook 状态 / WorkBuddy 事实（`force=true` 跳过）；
//! - 计时并把 `elapsedMs` 写进响应。
//!
//! # R6-10：为什么改 async
//!
//! Tauri 同步命令在主线程执行，生成期间消息循环被占住（窗口「卡住」）。改成
//! `async fn` + `spawn_blocking` 后，重活跑在线程池里，UI 不阻塞。

use std::sync::{Mutex, OnceLock};

use serde_json::{json, Value};
use wb_shift_core::modules::{
    account, codebuddy_cli, codebuddy_cn_ide, codebuddy_ide, config, diagnostics, jetbrains,
    vscode_ext,
};

/// 30s 探测缓存：客户端探测 + hook 状态 + WorkBuddy 事实。
#[derive(Default)]
struct ProbeCache {
    at_ms: i64,
    hook: Option<diagnostics::HookState>,
    workbuddy: Vec<diagnostics::WorkBuddyFacts>,
    clients: Vec<diagnostics::ClientProbe>,
}

static PROBE_CACHE: OnceLock<Mutex<ProbeCache>> = OnceLock::new();

fn cache() -> &'static Mutex<ProbeCache> {
    PROBE_CACHE.get_or_init(|| Mutex::new(ProbeCache::default()))
}

/// 5 个客户端的探测规格（与设置页「支持工具」入口对齐）。
fn probe_specs() -> Vec<diagnostics::ProbeSpec> {
    vec![
        diagnostics::ProbeSpec::new("CodeBuddy CLI", codebuddy_cli::status),
        diagnostics::ProbeSpec::new("CodeBuddy IDE", codebuddy_ide::status),
        diagnostics::ProbeSpec::new("CodeBuddy IDE（国内版）", codebuddy_cn_ide::status),
        diagnostics::ProbeSpec::new("VS Code 插件", vscode_ext::status),
        diagnostics::ProbeSpec::new("JetBrains 插件", jetbrains::status),
    ]
}

/// 两档位的 WorkBuddy 事实。两档位各起一个线程：`is_workbuddy_running` 是进程枚举，
/// 串行跑两遍会把「首次预览」从 1s 拖到 2s+。
fn workbuddy_facts() -> Vec<diagnostics::WorkBuddyFacts> {
    std::thread::scope(|scope| {
        let handles: Vec<_> = ["cn", "ai"]
            .into_iter()
            .map(|variant| scope.spawn(move || diagnostics::workbuddy_facts(variant)))
            .collect();
        handles
            .into_iter()
            .filter_map(|handle| handle.join().ok().flatten())
            .collect()
    })
}

/// 取探测结果：命中 30s 缓存则复用，`force` 或过期则重新探测。
fn probes(
    force: bool,
) -> (
    Vec<diagnostics::ClientProbe>,
    Option<diagnostics::HookState>,
    Vec<diagnostics::WorkBuddyFacts>,
    bool,
) {
    let now = config::now_ms();
    let guard = cache();
    let cache = guard
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if !force
        && cache.at_ms > 0
        && now.saturating_sub(cache.at_ms) < diagnostics::PROBE_CACHE_TTL_MS
    {
        return (
            cache.clients.clone(),
            cache.hook.clone(),
            cache.workbuddy.clone(),
            true,
        );
    }
    drop(cache);
    // 三组贵探测彼此独立（客户端 5 个 / hook 3 处配置 / WorkBuddy 2 档位），
    // 并发跑：总耗时 = 最慢的一组，而不是三者相加。
    let (clients, hook, workbuddy) = std::thread::scope(|scope| {
        let clients = scope
            .spawn(|| diagnostics::probe_clients(probe_specs(), diagnostics::PROBE_TIMEOUT_MS));
        let hook = scope.spawn(|| {
            diagnostics::hook_state_from_status(
                &wb_shift_core::modules::rate_limit_hook::hook_status(),
            )
        });
        let workbuddy = scope.spawn(workbuddy_facts);
        (
            clients.join().unwrap_or_default(),
            hook.join().unwrap_or_default(),
            workbuddy.join().unwrap_or_default(),
        )
    });
    let mut cache = guard
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *cache = ProbeCache {
        at_ms: now,
        hook: hook.clone(),
        workbuddy: workbuddy.clone(),
        clients: clients.clone(),
    };
    (clients, hook, workbuddy, false)
}

/// 语言标签归一化：空串 / 纯空白按 `unknown` 处理。
fn normalize_locale(locale: Option<String>) -> String {
    locale
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

/// 生成报告文本 + 响应（含 `elapsedMs`）。
fn build_payload(locale: Option<String>, force: bool) -> Value {
    let started = std::time::Instant::now();
    let now = config::now_ms();
    let accounts = account::load_accounts();
    let (clients, hook, workbuddy, cache_used) = probes(force);
    // 便宜段（账号 / 设置 / 凭据事件 / 日志）每次新鲜生成；`generation` 先占位，
    // 采完之后回填真实耗时，保证 `[诊断生成]` 里的总耗时覆盖整条链路。
    let mut sources = diagnostics::collect_cheap_sources(
        now,
        diagnostics::local_offset_seconds(),
        hook,
        workbuddy,
        diagnostics::GenerationFacts::default(),
    );
    sources.facts.generation = diagnostics::GenerationFacts {
        total_ms: started.elapsed().as_millis() as u64,
        probes: clients
            .iter()
            .map(|c| diagnostics::ProbeTiming {
                name: c.name.clone(),
                status: c.probe_status,
                elapsed_ms: c.elapsed_ms,
            })
            .collect(),
        cache_used,
        timeouts: clients
            .iter()
            .filter(|c| c.probe_status == diagnostics::ProbeStatus::Timeout)
            .map(|c| c.name.clone())
            .collect(),
    };
    let input = diagnostics::ReportInput {
        host_form: diagnostics::HostForm::Desktop,
        locale: normalize_locale(locale),
        account_count: accounts.len(),
        clients,
    };
    let text = diagnostics::build_report(&input, &sources);
    json!({
        "text": text,
        "generatedAt": now,
        "bytes": text.len(),
        "elapsedMs": started.elapsed().as_millis() as u64,
    })
}

/// 生成脱敏诊断包（只读，不落盘）。前端「预览」与「复制」都调它。
#[tauri::command]
pub async fn get_diagnostics_report(locale: Option<String>, force: Option<bool>) -> Value {
    let force = force.unwrap_or(false);
    let fallback_at = config::now_ms();
    tauri::async_runtime::spawn_blocking(move || build_payload(locale, force))
        .await
        .unwrap_or_else(|error| {
            json!({
                "text": format!("诊断生成失败：{error}"),
                "generatedAt": fallback_at,
                "bytes": 0,
                "elapsedMs": 0,
            })
        })
}

/// 把诊断包写到用户选定的路径（`.txt` / `.md`）。
#[tauri::command]
pub async fn export_diagnostics(
    path: String,
    locale: Option<String>,
    force: Option<bool>,
) -> Result<Value, String> {
    let force = force.unwrap_or(false);
    tauri::async_runtime::spawn_blocking(move || {
        let built = build_payload(locale, force);
        let text = built
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        diagnostics::save_report(&path, &text)
    })
    .await
    .map_err(|error| format!("导出任务失败：{error}"))?
}
