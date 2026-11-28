//! 国际版每日活跃会话（网页通道）：模块入口与共享约定。
//!
//! # 为什么需要它
//!
//! 国际版**没有签到**：`/billing/meter/daily-checkin` 对国际版账号恒返回活动未开启
//! （见 [`crate::modules::variant::WbVariant::supports_checkin`]）。国际版能拿到官方
//! 「每日活跃」奖励的唯一链路，是在网页控制台建一条 agent 会话并把它**真正跑完**。
//!
//! 关键结论（照抄上游实测结论，不要凭直觉改）：
//!
//! - 桌面端身份发 `chat/completions` **不计入**每日活跃，只有跑完的网页会话算；
//! - 只建会话不接沙箱，会话会永远停在 `CREATING`、零输出，同样不算；
//! - 上游对单账号的活跃会话（`status=working`）有硬上限，建了必须收尾，否则攒满后
//!   建会话直接 429 / 10105；
//! - 必须补一条 `session/set_model`，否则跑的是账号默认模型（Auto，按付费计费）。
//!
//! # 一次打卡 = 一个会话的五步
//!
//! 建会话 → 取沙箱地址 / token → ACP（`initialize` / `session/load` /
//! `session/set_model` / `session/prompt`）→ 轮询到 `completed` → 收尾。
//! 实现在 [`protocol`]；DNS 兜底在 [`dns`]；去重状态在 [`state`]；
//! HTTP 抽象在 [`transport`]。
//!
//! # 边界（本阶段只做核心）
//!
//! - **只在手动触发时执行**：本模块**没有**任何定时器 / 调度循环，也不会挂到签到
//!   的周期里；没有显式调用 [`run_daily_activity_once`] 就一个请求都不会发。
//! - **默认关闭**：调用方不传账号 = 什么都不做，不存在「启动即联网」的路径。
//! - **不切换账号**：只用账号自身的 `access_token` / `uid` 直连网页通道，当前登录态
//!   不受影响。
//! - **不打印、不落盘 prompt 内容与 token**：错误文案里连完整 URL query 都不带
//!   （见 [`scrub`]）；状态文件只记「哪天、哪个模型成功过」。
//! - **收尾对称**：成功 `stop`（会话留在历史里，只改状态），失败 `delete`（不在网页端
//!   堆测试会话）。提前返回同样走收尾，见 [`protocol`] 的 `attempt_with_model`。
//!
//! # 与签到的关系
//!
//! 签到走 `billing` 体系、由服务端 `active` 开关决定；每日活跃走
//! `/console/as/conversations/` + ACP，没有开关，会话 `completed` 即计分。两者互不
//! 影响，本模块不触碰任何签到接口。

mod dns;
mod protocol;
mod state;
mod transport;

use std::path::Path;

use serde_json::{json, Value};

use crate::modules::account::{envelope_token_error, variant_of};
use crate::modules::config::now_ms;
use crate::modules::variant::WbVariant;

pub use dns::SANDBOX_SUFFIX;
pub use protocol::{AccountRunResult, Limits, MODEL_CANDIDATES};
pub use state::{local_today, state_path, ActivityState};
pub use transport::{
    BoxFuture, EventStream, HttpRequest, HttpResponse, HttpTransport, Method, Transport,
    WEB_USER_AGENT,
};

/// 不打印 token / query 的错误文案脱敏：把文本里每个 URL 片段截到 `?` / `#` 之前。
///
/// 沙箱地址（`link`）可能带 `?token=...`；一旦把整段地址拼进错误文案，token 就跟着
/// 进了日志与界面。所有对外文案在拼好之后都必须过一遍这里。
pub(crate) fn scrub(text: &str) -> String {
    text.split_whitespace()
        .map(scrub_piece)
        .collect::<Vec<_>>()
        .join(" ")
}

fn scrub_piece(piece: &str) -> String {
    let Some(start) = piece.find("http://").or_else(|| piece.find("https://")) else {
        return piece.to_string();
    };
    let rest = &piece[start..];
    match rest.find(['?', '#']) {
        Some(index) => format!("{}{}", &piece[..start], &rest[..index]),
        None => piece.to_string(),
    }
}

/// 对单个地址脱敏，只保留 `scheme://host/path`。
///
/// 对外公开：宿主（Tauri command / axum handler）把错误往上抛时可能还要拼地址，
/// 统一走这里，避免各写一份 `split('?')`。
pub fn redact_url(url: &str) -> String {
    scrub_piece(url)
}

/// 对全部给定账号跑一次每日活跃会话（**手动入口**）。
///
/// 返回 `{"date": "<本地日期>", "accounts": [逐账号结果...]}`；单个账号失败只会出现
/// 在该账号的结果里，不会打断其它账号。`Err` 只用于「连 HTTP 客户端都构造不出来」
/// 这类整体性失败——真正的业务失败一律走逐账号结果。
///
/// 这是本模块唯一的网络入口：不接调度、不加轮询定时器。宿主（Tauri command /
/// axum handler）在下一阶段负责把它接到手动按钮上。
///
/// 日期与各账号的「今日已打卡」判定都在这里统一取一次，跨零点不会一半按今天、
/// 一半按昨天。
pub async fn run_daily_activity_once(accounts: &[Value]) -> Result<Value, String> {
    let transport = HttpTransport::new()?;
    Ok(run_daily_activity_injected(accounts, &transport, &state_path()).await)
}

/// 注入传输层与状态文件路径的入口：单测用它跑桩，生产用 [`run_daily_activity_once`]。
///
/// 之所以把 `state_file` 也做成参数：单测必须能写临时目录，不能碰真实
/// `~/.workbuddy-shift/daily_activity_state.json`。
pub async fn run_daily_activity_injected(
    accounts: &[Value],
    transport: &dyn Transport,
    state_file: &Path,
) -> Value {
    run_daily_activity_with_limits(accounts, transport, state_file, &Limits::default()).await
}

/// 同上，但超时/重试上限可注入（单测把轮询间隔压到毫秒级，避免用例变慢）。
pub(crate) async fn run_daily_activity_with_limits(
    accounts: &[Value],
    transport: &dyn Transport,
    state_file: &Path,
    limits: &Limits,
) -> Value {
    let today = local_today();
    let mut rows: Vec<Value> = Vec::with_capacity(accounts.len());
    for account in accounts {
        let result = run_one(transport, account, state_file, &today, limits).await;
        rows.push(serde_json::to_value(&result).unwrap_or(Value::Null));
    }
    json!({"date": today, "accounts": rows})
}

/// 单账号：档位 / 凭据 / 去重三道前置检查，再交给 [`protocol::run_account`]。
///
/// 三道检查都**不发任何请求**：国际版以外的账号（国内版走签到）、缺凭据的账号
/// （加密信封态 / 未登录）和当天已完成过的账号在这里就被挡掉。
async fn run_one(
    transport: &dyn Transport,
    account: &Value,
    state_file: &Path,
    today: &str,
    limits: &Limits,
) -> AccountRunResult {
    let key = state::account_key(account);
    if variant_of(account) != WbVariant::Ai {
        return AccountRunResult::unsupported(key, "每日活跃会话仅国际版可用；国内版走签到");
    }
    if key.is_empty() {
        return AccountRunResult::unsupported(key, "账号缺少 id/uid，无法记录每日去重状态");
    }
    // R6-06：信封账号 + 写开关打开时，用本机 live 凭据解出临时副本再跑；
    // 解不出来才落到下面的信封提示（顺序不能反）。
    let resolved = crate::modules::at_rest::usable_account(
        account,
        crate::modules::at_rest::AccessMode::Write,
    );
    let account = resolved.as_ref().unwrap_or(account);
    if let Some(notice) = envelope_token_error(account) {
        return AccountRunResult::unsupported(key, notice);
    }
    if !has_plain_token(account) {
        return AccountRunResult::unsupported(key, "账号缺少 access_token（请重新登录后再试）");
    }
    if state::done_today(state_file, &key, today) {
        return AccountRunResult::skipped(key);
    }

    let mut result = protocol::run_account(transport, account, &key, limits).await;
    if result.ok {
        let model = result.model.clone().unwrap_or_default();
        if let Err(error) = state::record_success(state_file, &key, today, &model, now_ms()) {
            // 会话已经跑完，活跃已经拿到：记状态失败只作为提示，不改判结果。
            result.notice = Some(format!("已完成，但写入每日状态失败: {error}"));
        }
    }
    result
}

/// 是否持有非空的明文 `access_token`（加密信封态由 `envelope_token_error` 先行拦截）。
fn has_plain_token(account: &Value) -> bool {
    account
        .get("access_token")
        .and_then(Value::as_str)
        .map(str::trim)
        .map(|token| !token.is_empty())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    include!("tests/fixtures.rs");
    include!("tests/dns.rs");
    include!("tests/state.rs");
    include!("tests/protocol.rs");
}
