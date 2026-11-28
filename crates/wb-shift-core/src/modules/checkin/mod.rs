//! 签到：状态查询 / 执行签到 / 自动签到调度。
//!
//! 对照 server.py `get_checkin_status` / `perform_checkin` /
//! `checkin_account` / `run_checkin_cycle` / `_checkin_request` /
//! `_is_unauthorized`。
//!
//! 档位策略：签到仅国内版可用（`WbVariant::supports_checkin`）。国际版没有签到
//! 接口，自动周期、一键签到与单账号签到都在发请求前统一跳过，绝不发起任何请求；
//! 下游的 inactive / statusUnsupported 判定保留为防御，不依赖它们拦截国际版。
//!
//! 时间段（`checkin_start` / `checkin_end`，本地时区）：每个账号每天在窗口内按
//! (本地日期, 账号 id) 确定性抽一个目标分钟；只有到点未办的账号才发请求，未到点整轮
//! 不发任何请求。字段为空/非法时自动调度退化为默认窗口 8:00–22:00（`schedule_window`），
//! 并改用「只看今日」口径（`DueScope::TodayOnly`）：窗口内今天的目标没到不处理；窗口外
//! 每轮限量补签（`OUTSIDE_WINDOW_BATCH = 3`），夜间开机不漏签、也不批量提交。
//! 只换窗口不换口径不够——跨日补签会让「今天晚点才轮到」的账号在上午就被昨天那轮顶成
//! 到点，分散效果归零。手动入口（`run_checkin_all` 的 `respect_window`）语义不变：
//! 未配置 = 不限制、立即签。
//!
//! 账号页「刷新并签到」按 `respect_window=true` 调用 `run_checkin_all`：窗口生效且
//! 当前时刻不在 `[start, end)` 内时整轮跳过，逐账号返回 `skipped` /
//! `outside_checkin_window`，不查状态、不提交、不写日志。设置页 / 托盘 / 单账号
//! 手动入口不传该参数（默认 `false`），保持「立即签到」语义。

use chrono::{Local, TimeZone, Timelike};
use serde_json::{json, Value};
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use crate::modules::account::{
    account_display_name, build_auth_headers, load_accounts, variant_of,
};
use crate::modules::config::{
    add_checkin_log, http_request, is_route_missing, load_checkin_config, load_checkin_logs,
    now_ms, parse_clock, RunFlagGuard, CHECKIN_API_PREFIX,
};
use crate::modules::refresh::{ensure_fresh_token, refresh_account_token};
use crate::modules::variant::WbVariant;

mod cycle;
mod logs;
mod status;
mod window;

pub use cycle::{
    accounts_checked_in_today, all_accounts_checked_in_today, checkin_account, run_checkin_all,
    run_checkin_cycle, CheckinCycleMode, CHECKIN_RECOVERY_INTERVAL,
};
pub use logs::{checkin_logs_with_variant, date_str, load_checkin_logs_with_variant};
pub use status::{get_checkin_status, get_checkin_status_for_display, perform_checkin};
pub use window::next_cycle_delay;

#[cfg(test)]
mod tests {
    use super::*;

    use super::cycle::*;
    use super::status::*;
    use super::window::*;
    include!("tests/fixtures.rs");
    include!("tests/status.rs");
    include!("tests/window.rs");
    include!("tests/cycle.rs");
}
