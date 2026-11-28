//! diagnostics 域的 HTTP handler。
//!
//! 与 `src-tauri/src/commands/diagnostics.rs` 一一对应：同一份
//! `wb_shift_core::modules::diagnostics` 装配路径，两个宿主只负责「凑事实」。
//!
//! R6-10：并行探测（[`diagnostics::probe_clients`]，单 probe 800ms 超时）+ 30s 缓存，
//! `force=true` 跳过缓存；响应带 `elapsedMs`。
//!
//! **网页版不落盘**：`GET /api/diagnostics` 只返回文本，前端自己复制。服务端不往用户的
//! 机器上写任意路径（与前端 `canPersistErrorLog()` 的既有约定一致）。

use std::sync::{Mutex, OnceLock};

use axum::extract::RawQuery;
use axum::http::StatusCode;
use axum::response::Response;
use serde_json::Value;

use crate::api::*;
use wb_shift_core::modules::{
    account, codebuddy_cli, codebuddy_cn_ide, codebuddy_ide, config, diagnostics, jetbrains,
    rate_limit_hook, vscode_ext,
};

/// 30s 探测缓存（与桌面端同口径）。
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
        let hook =
            scope.spawn(|| diagnostics::hook_state_from_status(&rate_limit_hook::hook_status()));
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

/// GET /api/diagnostics —— 脱敏诊断包全文（只读）。
pub(crate) async fn api_diagnostics_report(RawQuery(query): RawQuery) -> Response {
    let locale = normalize_locale(query_param(query.as_deref(), "locale"));
    let force = matches!(
        query_param(query.as_deref(), "force").as_deref(),
        Some("1") | Some("true")
    );
    let result = tokio::task::spawn_blocking(move || build_payload(locale, force)).await;
    match result {
        Ok(value) => json_ok(value),
        Err(error) => json_err(error.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
    }
}

fn build_payload(locale: String, force: bool) -> Value {
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
        host_form: diagnostics::HostForm::WebUi,
        locale,
        account_count: accounts.len(),
        clients,
    };
    let text = diagnostics::build_report(&input, &sources);
    serde_json::json!({
        "text": text,
        "generatedAt": now,
        "bytes": text.len(),
        "elapsedMs": started.elapsed().as_millis() as u64,
    })
}

/// 语言标签归一化：空串 / 纯空白按 `unknown` 处理。
fn normalize_locale(locale: Option<String>) -> String {
    locale
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}
