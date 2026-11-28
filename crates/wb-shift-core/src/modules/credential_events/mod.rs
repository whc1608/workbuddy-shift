//! 凭据事件环形日志（`<store_root>/credential-events.jsonl`）。
//!
//! # 契约（跨轨道冻结，R6-01/R6-03 共用）
//!
//! - **路径**：`<store_root>/credential-events.jsonl`（`~/.workbuddy-shift/`，
//!   [`crate::modules::config::store_dir`]），UTF-8、每行一个 JSON 对象；
//! - **字段固定五个 + 可选 `count`**：`ts`（epoch 毫秒）、`account`（账号序号，int）、
//!   `variant`（`"cn"` / `"ai"`）、`action`（枚举）、`reason`（枚举）；节流窗口内重复时
//!   追加可选整数 `count`（≥2 才写，旧文件没有该字段按 1 读）。
//!   *不写 token、不写自由文本、不写昵称/邮箱/uid/路径*——诊断包会把它原样贴给用户，
//!   任何一项越界都等于泄露凭据或身份；
//! - **上限 200 条**（[`MAX_EVENTS`]）：超出后原子重写只留最新 200 条；`count` 不改变
//!   上限口径，仍是**行数**（一次累加只改一行，不新增行）；
//! - **写入语义**：读已有行 → 追加/必要时重写，全程持进程内写锁（并发上报不得互相覆盖）；
//! - **节流但可计数**：同账号 + 同档位 + 同 action 在 [`THROTTLE_MS`]（60s）内不新增行，
//!   而是把该键**最近一条**记录的 `count` 加 1（`ts` 保持不变，窗口锚点不滑动）。目的是让
//!   「每次启动都 adopt / 每次保活都 skip」不会把 200 条环形窗口刷满、把真正的异常挤掉，
//!   同时「连续 5 次解密失败」在诊断里仍能看到 `×5`；
//! - **失败静默**：写不进去只 `eprintln!`，绝不让业务失败、绝不 panic（记事件不能
//!   影响刷新/保活）。
//!
//! # 写入方 / 读取方
//!
//! - 写入：[`crate::modules::refresh`]（每个凭据决策点）与 R6-06 解密通道（`decrypt_*`）；
//!   两边都必须走本模块的 [`record`] / [`record_for_account`]，**禁止各写一份**；
//! - 读取：诊断包（R6-03/R6-09）只读本文件，用 [`recent`]；缺失/损坏一律当「无记录」，
//!   不报错、不 panic。
//!
//! # 与 `refresh_outcome` 的关系
//!
//! `action` 的取值与 `refresh::refresh_outcome` 同名（`adopted` / `kept_plaintext` /
//! `skipped_encrypted` / `refreshed`），不要让两套词表各漂各的。

use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::modules::config::{atomic_write, now_ms, store_dir};

/// 环形上限：写入后超过该条数即原子重写，只保留最新 200 条。
pub const MAX_EVENTS: usize = 200;

/// 同账号 + 同档位 + 同 action 的写入节流窗口（60 秒）。
pub const THROTTLE_MS: i64 = 60_000;

/// 文件名（落在工具存储根下）。
pub const FILE_NAME: &str = "credential-events.jsonl";

// ---------------------------------------------------------------------------
// 枚举（逐字冻结，C 轨按这份读）
// ---------------------------------------------------------------------------

/// 采用了 live 认证文件的新凭据（没发刷新请求）。
pub const ACTION_ADOPTED: &str = "adopted";
/// live 是加密信封、账号库明文仍有效：保留明文，不回填、不请求。
pub const ACTION_KEPT_PLAINTEXT: &str = "kept_plaintext";
/// 凭据是加密信封：不发请求、不改账号库。
pub const ACTION_SKIPPED_ENCRYPTED: &str = "skipped_encrypted";
/// 走完了刷新请求（成功 / 暂时失败 / 吊销都在此，结论看 `reason`）。
pub const ACTION_REFRESHED: &str = "refreshed";
/// 刷新失败（只记需要用户处置的类别，暂时失败不记，避免噪声）。
pub const ACTION_REFRESH_FAILED: &str = "refresh_failed";
/// 首次识别到加密信封（可选，同一次决策只记一条主 action）。
pub const ACTION_ENVELOPE_DETECTED: &str = "envelope_detected";
/// R6-06：解密成功。
pub const ACTION_DECRYPT_OK: &str = "decrypt_ok";
/// R6-06：解密失败。
pub const ACTION_DECRYPT_FAILED: &str = "decrypt_failed";

/// 全部 action 取值（白名单，顺序固定用于文档与单测）。
pub const ACTIONS: [&str; 8] = [
    ACTION_ADOPTED,
    ACTION_KEPT_PLAINTEXT,
    ACTION_SKIPPED_ENCRYPTED,
    ACTION_REFRESHED,
    ACTION_REFRESH_FAILED,
    ACTION_ENVELOPE_DETECTED,
    ACTION_DECRYPT_OK,
    ACTION_DECRYPT_FAILED,
];

/// 无异常的默认原因。
pub const REASON_OK: &str = "ok";
/// 缺 refresh token（且**不是**信封：信封走 `skipped_encrypted`/`ok`）。
pub const REASON_MISSING_REFRESH_TOKEN: &str = "missing_refresh_token";
/// 刷新被服务端拒绝（refresh token 已吊销 / 过期）。
pub const REASON_INVALID_GRANT: &str = "invalid_grant";
/// R6-06：客户端原生绑定不可用（未安装 / binding 改名 / fuse 禁用）。
pub const REASON_DECRYPT_BINDING_MISSING: &str = "decrypt_binding_missing";
/// R6-06：信封 `keyId` 与本机派生 key 不匹配。
pub const REASON_DECRYPT_KEY_MISMATCH: &str = "decrypt_key_mismatch";
/// R6-06：AES-GCM 校验失败。
pub const REASON_DECRYPT_GCM_FAILED: &str = "decrypt_gcm_failed";
/// R6-06：取密钥 / 解密超时。
pub const REASON_DECRYPT_TIMEOUT: &str = "decrypt_timeout";
/// R6-06：该平台探测未通过，已关闭解密。
pub const REASON_PLATFORM_DISABLED: &str = "platform_disabled";
/// R6-06：写操作解密开关关闭。
pub const REASON_SWITCH_DISABLED: &str = "switch_disabled";

/// 全部 reason 取值（白名单，顺序固定用于文档与单测）。
pub const REASONS: [&str; 9] = [
    REASON_OK,
    REASON_MISSING_REFRESH_TOKEN,
    REASON_INVALID_GRANT,
    REASON_DECRYPT_BINDING_MISSING,
    REASON_DECRYPT_KEY_MISMATCH,
    REASON_DECRYPT_GCM_FAILED,
    REASON_DECRYPT_TIMEOUT,
    REASON_PLATFORM_DISABLED,
    REASON_SWITCH_DISABLED,
];

/// 「追加 → 必要时重写」是读改写，必须串行：并发上报会互相覆盖对方刚写的行。
static WRITE_LOCK: Mutex<()> = Mutex::new(());

/// 事件日志文件路径（工具存储根下，与 `accounts.json` 同目录）。
pub fn events_path() -> PathBuf {
    events_path_in(&store_dir())
}

/// 指定存储根下的事件日志路径（单测注入用）。
pub fn events_path_in(store_root: &Path) -> PathBuf {
    store_root.join(FILE_NAME)
}

/// action 白名单归一化：未知取值返回 `None`（拒绝落盘，而不是写进任意字符串）。
fn normalize_action(action: &str) -> Option<&'static str> {
    ACTIONS
        .iter()
        .find(|known| **known == action.trim())
        .copied()
}

/// reason 白名单归一化：未知取值返回 `None`（同上）。
fn normalize_reason(reason: &str) -> Option<&'static str> {
    REASONS
        .iter()
        .find(|known| **known == reason.trim())
        .copied()
}

/// 档位归一化：只认 `"ai"`，其余（含空串、未知）一律记为 `"cn"`——不落任意字符串。
fn normalize_variant(variant: &str) -> &'static str {
    if variant.trim().eq_ignore_ascii_case("ai") {
        "ai"
    } else {
        "cn"
    }
}

/// 记录一条凭据事件。
///
/// `account_index` 是**账号序号，从 1 开始**（`load_accounts` 的位置 + 1），
/// `variant` 取 `"cn"` / `"ai"`。序号 0 或未知的 action/reason 直接不写（避免错号/脏值）。
/// 写失败只打日志，调用方永远不会因为「记事件」而失败。
pub fn record(account_index: u32, variant: &str, action: &str, reason: &str) {
    if let Err(error) = record_at(
        &events_path(),
        account_index,
        variant,
        action,
        reason,
        now_ms(),
    ) {
        eprintln!("[credential-events] 写入失败（已忽略）：{error}");
    }
}

/// 便捷入口：按账号 uid 在 [`crate::modules::account::load_accounts`] 中的位置解析序号。
///
/// 找不到该账号时**不写**（写一个错号比丢一条事件严重得多：诊断会把状态贴到别的账号头上）。
pub fn record_for_account(account: &Value, action: &str, reason: &str) {
    let uid = crate::modules::account::get_str(account, "uid");
    let id = crate::modules::account::get_str(account, "id");
    let index = crate::modules::account::load_accounts()
        .iter()
        .position(|stored| {
            let stored_uid = crate::modules::account::get_str(stored, "uid");
            let stored_id = crate::modules::account::get_str(stored, "id");
            match (&uid, &id) {
                // uid 是账号的真实身份：优先按 uid 命中。
                (Some(uid), _) if stored_uid.as_deref() == Some(uid.as_str()) => true,
                (None, Some(id)) => stored_id.as_deref() == Some(id.as_str()),
                _ => false,
            }
        });
    let Some(index) = index else {
        eprintln!("[credential-events] 账号不在账号库中，跳过事件记录");
        return;
    };
    let variant = crate::modules::account::variant_of(account).as_str();
    record(index as u32 + 1, variant, action, reason);
}

/// 最近 `limit` 条事件，**新的在前**（倒序）。
///
/// 文件缺失 / 损坏 / 单行不是 JSON 时按「无记录」处理：诊断读取绝不允许因此报错。
pub fn recent(limit: usize) -> Vec<Value> {
    recent_at(&events_path(), limit)
}

/// 读已有行；文件缺失或读不出（非 UTF-8 等）按空处理——事件可以丢，不能因此报错。
fn read_lines(path: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(str::to_string)
        .collect()
}

/// 解析一行；不是合法 JSON 对象时返回 `None`（坏行跳过，不牵连整份文件）。
fn parse_line(line: &str) -> Option<Value> {
    match serde_json::from_str::<Value>(line) {
        Ok(value) if value.is_object() => Some(value),
        _ => None,
    }
}

/// 该键（账号 + 档位 + action）最后一条记录的下标与 `ts`；没有记录返回 `None`。
fn last_entry_for_key(
    lines: &[String],
    account_index: u32,
    variant: &str,
    action: &str,
) -> Option<(usize, i64)> {
    for (index, line) in lines.iter().enumerate().rev() {
        let Some(entry) = parse_line(line) else {
            continue;
        };
        let same = entry.get("account").and_then(Value::as_i64) == Some(i64::from(account_index))
            && entry.get("variant").and_then(Value::as_str) == Some(variant)
            && entry.get("action").and_then(Value::as_str) == Some(action);
        if same {
            return entry
                .get("ts")
                .and_then(Value::as_i64)
                .map(|ts| (index, ts));
        }
    }
    None
}

/// 追加一行；超出上限时原子重写保留最新 [`MAX_EVENTS`] 条。调用方需持写锁。
fn append_line_locked(path: &Path, line: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut lines = read_lines(path);
    lines.push(line.to_string());
    if lines.len() > MAX_EVENTS {
        // 环形保留：临时文件 + rename 原子替换，避免重写中途留下半截文件。
        lines.drain(..lines.len() - MAX_EVENTS);
        return atomic_write(path, &format!("{}\n", lines.join("\n")));
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(file, "{line}")
}

/// 命中节流窗口：把 `lines[index]` 的 `count` 加 1 后原子重写整份文件。调用方需持写锁。
///
/// - `ts` **保持不变**：窗口锚点不随重复上报滑动，否则高频事件会永远被累加、永不产生新行；
/// - 坏行原样写回（解析失败的历史行不能因为一次累加就消失）；
/// - 行数不变，[`MAX_EVENTS`] 的「按行数」口径不受影响；文件 ≤ 200 行，整体重写成本可接受。
fn bump_count_locked(path: &Path, lines: &[String], index: usize) -> std::io::Result<()> {
    let mut entry = parse_line(&lines[index]).ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "节流命中的行已不是 JSON")
    })?;
    let next = entry
        .get("count")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .max(1)
        .saturating_add(1);
    entry["count"] = json!(next);
    let mut rewritten = lines.to_vec();
    rewritten[index] = serde_json::to_string(&entry)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    atomic_write(path, &format!("{}\n", rewritten.join("\n")))
}

/// 写入指定路径（单测注入 `now` / `path`）。返回 IO 结果，由 [`record`] 决定是否忽略。
fn record_at(
    path: &Path,
    account_index: u32,
    variant: &str,
    action: &str,
    reason: &str,
    at_ms: i64,
) -> std::io::Result<()> {
    let Some(action) = normalize_action(action) else {
        eprintln!("[credential-events] 未知 action，跳过记录");
        return Ok(());
    };
    let Some(reason) = normalize_reason(reason) else {
        eprintln!("[credential-events] 未知 reason，跳过记录");
        return Ok(());
    };
    if account_index == 0 {
        eprintln!("[credential-events] 账号序号必须从 1 开始，跳过记录");
        return Ok(());
    }
    let variant = normalize_variant(variant);
    let entry = json!({
        "ts": at_ms,
        "account": account_index,
        "variant": variant,
        "action": action,
        "reason": reason,
    });
    let line = serde_json::to_string(&entry)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    // 锁中毒不应让「记事件」变成 panic 源：取回内部守卫继续写（与 error_log 同口径）。
    let _guard = WRITE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    // 节流必须在锁内判：否则两个并发写会同时通过「最后一条」检查，各写一条。
    let lines = read_lines(path);
    if let Some((index, last_ts)) = last_entry_for_key(&lines, account_index, variant, action) {
        if at_ms.saturating_sub(last_ts) < THROTTLE_MS {
            // 窗口内重复：不新增行，累加最近一条的 `count`（可计数地节流）。
            return bump_count_locked(path, &lines, index);
        }
    }
    append_line_locked(path, &line)
}

/// [`recent`] 的可测实现：新的在前，坏行跳过，文件缺失返回空 Vec。
fn recent_at(path: &Path, limit: usize) -> Vec<Value> {
    if limit == 0 {
        return Vec::new();
    }
    let mut entries: Vec<Value> = read_lines(path)
        .iter()
        .filter_map(|line| parse_line(line))
        .collect();
    entries.reverse();
    entries.truncate(limit);
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "workbuddy-shift-credential-events-{name}-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(FILE_NAME);
        (dir, path)
    }

    /// 一条记录只有五个字段，且不含任何自由文本。
    #[test]
    fn event_has_exactly_the_contract_fields() {
        let (dir, path) = temp_path("fields");
        record_at(&path, 1, "cn", ACTION_ADOPTED, REASON_OK, 1_700_000_000_000).unwrap();

        let entries = recent_at(&path, 10);
        assert_eq!(entries.len(), 1);
        let entry = entries[0].as_object().unwrap();
        let mut keys: Vec<&String> = entry.keys().collect();
        keys.sort();
        assert_eq!(keys, ["account", "action", "reason", "ts", "variant"]);
        assert_eq!(entry["ts"], json!(1_700_000_000_000i64));
        assert_eq!(entry["account"], json!(1));
        assert_eq!(entry["variant"], json!("cn"));
        assert_eq!(entry["action"], json!("adopted"));
        assert_eq!(entry["reason"], json!("ok"));
        assert!(
            !entry.contains_key("count"),
            "首次写入（计数 1）不写 count 字段"
        );

        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// 未知 action / reason / 序号 0 一律不写（避免脏值进诊断）。
    #[test]
    fn unknown_enums_and_zero_index_are_not_written() {
        let (dir, path) = temp_path("reject");
        record_at(&path, 1, "cn", "not-an-action", REASON_OK, 1).unwrap();
        record_at(&path, 1, "cn", ACTION_ADOPTED, "free text", 1).unwrap();
        record_at(&path, 0, "cn", ACTION_ADOPTED, REASON_OK, 1).unwrap();
        assert!(recent_at(&path, 10).is_empty());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// 60s 内同键重复不写；换键或过窗都写。
    #[test]
    fn same_key_is_throttled_for_sixty_seconds() {
        let (dir, path) = temp_path("throttle");
        record_at(&path, 2, "cn", ACTION_KEPT_PLAINTEXT, REASON_OK, 1_000_000).unwrap();
        // 同键 59.999s 后：仍在窗口内。
        record_at(&path, 2, "cn", ACTION_KEPT_PLAINTEXT, REASON_OK, 1_059_999).unwrap();
        assert_eq!(recent_at(&path, 10).len(), 1, "60s 内同键只记一条");

        // 换 action：不同键，不受影响。
        record_at(
            &path,
            2,
            "cn",
            ACTION_SKIPPED_ENCRYPTED,
            REASON_OK,
            1_060_000,
        )
        .unwrap();
        // 换账号：不同键。
        record_at(&path, 3, "cn", ACTION_KEPT_PLAINTEXT, REASON_OK, 1_060_000).unwrap();
        assert_eq!(recent_at(&path, 10).len(), 3);

        // 同键正好过窗（60s）→ 允许写第二条。
        record_at(&path, 2, "cn", ACTION_KEPT_PLAINTEXT, REASON_OK, 1_060_000).unwrap();
        assert_eq!(recent_at(&path, 10).len(), 4);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// 环形上限：写满 250 条只留最新 200 条，淘汰的是最旧的。
    #[test]
    fn ring_keeps_only_the_newest_two_hundred_events() {
        let (dir, path) = temp_path("ring");
        for index in 0..250u32 {
            // 账号序号每轮都变（1..=250），天然绕开 60s 节流。
            record_at(
                &path,
                index + 1,
                "cn",
                ACTION_REFRESHED,
                REASON_OK,
                i64::from(index),
            )
            .unwrap();
        }
        let lines: Vec<String> = std::fs::read_to_string(&path)
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect();
        assert_eq!(lines.len(), MAX_EVENTS);

        let entries = recent_at(&path, MAX_EVENTS + 50);
        assert_eq!(entries.len(), MAX_EVENTS);
        // 新的在前：第一条是最后写入的那条，最后一条是最早保留的那条。
        assert_eq!(entries[0]["account"], json!(250));
        assert_eq!(entries[MAX_EVENTS - 1]["account"], json!(51));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// 文件缺失 / 坏行：`recent_at` 返回空或跳过，绝不报错。
    #[test]
    fn missing_or_broken_file_is_treated_as_no_records() {
        let (dir, path) = temp_path("missing");
        assert!(recent_at(&path, 10).is_empty(), "文件缺失返回空 Vec");

        std::fs::write(&path, "not json\n{\"ts\":1}\n").unwrap();
        let entries = recent_at(&path, 10);
        assert_eq!(entries.len(), 1, "坏行被跳过，合法行仍可读");
        assert_eq!(entries[0]["ts"], json!(1));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// 写入失败只返回 Err，不 panic（公开入口 `record` 会吞掉它）。
    #[test]
    fn write_failure_is_returned_not_panicked() {
        let (dir, _) = temp_path("blocked");
        let blocker = dir.join("blocker");
        std::fs::write(&blocker, "not a directory").unwrap();
        let result = record_at(
            &blocker.join(FILE_NAME),
            1,
            "cn",
            ACTION_ADOPTED,
            REASON_OK,
            1,
        );
        assert!(result.is_err(), "父路径不可用时必须返回 Err");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// 档位归一化：只认 `ai`，其余（含未知）记 `cn`，不落任意字符串。
    #[test]
    fn variant_is_normalized() {
        assert_eq!(normalize_variant("ai"), "ai");
        assert_eq!(normalize_variant("AI"), "ai");
        assert_eq!(normalize_variant("cn"), "cn");
        assert_eq!(normalize_variant(""), "cn");
        assert_eq!(normalize_variant("enterprise"), "cn");
    }

    /// 枚举白名单与 refresh_outcome 同名：两套词表不许各漂各的。
    #[test]
    fn actions_cover_refresh_outcomes() {
        for outcome in [
            "adopted",
            "kept_plaintext",
            "skipped_encrypted",
            "refreshed",
        ] {
            assert!(
                ACTIONS.contains(&outcome),
                "`{outcome}` 必须同时是凭据事件的 action 取值"
            );
        }
    }

    /// 窗口内同键 5 次：只有 1 行，`count=5`，`ts` 停在第一次（窗口锚点不滑动）。
    #[test]
    fn repeated_same_key_accumulates_count() {
        let (dir, path) = temp_path("count");
        for index in 0..5i64 {
            record_at(
                &path,
                7,
                "cn",
                ACTION_DECRYPT_FAILED,
                REASON_DECRYPT_BINDING_MISSING,
                1_000_000 + index * 1_000,
            )
            .unwrap();
        }
        let lines = read_lines(&path);
        assert_eq!(lines.len(), 1, "窗口内同键只留一行");
        let entry = parse_line(&lines[0]).expect("合法行");
        assert_eq!(entry["count"], json!(5));
        assert_eq!(entry["ts"], json!(1_000_000), "ts 不随重复上报滑动");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// 过窗后重新计数：新行不带 `count`（等价于 1），旧行保留自己的 `count`。
    #[test]
    fn count_restarts_after_throttle_window() {
        let (dir, path) = temp_path("count-window");
        record_at(
            &path,
            1,
            "cn",
            ACTION_DECRYPT_FAILED,
            REASON_DECRYPT_TIMEOUT,
            1_000_000,
        )
        .unwrap();
        record_at(
            &path,
            1,
            "cn",
            ACTION_DECRYPT_FAILED,
            REASON_DECRYPT_TIMEOUT,
            1_030_000,
        )
        .unwrap();
        record_at(
            &path,
            1,
            "cn",
            ACTION_DECRYPT_FAILED,
            REASON_DECRYPT_TIMEOUT,
            1_061_000,
        )
        .unwrap();

        let lines = read_lines(&path);
        assert_eq!(lines.len(), 2, "超过 60s 后必须新起一行");
        let first = parse_line(&lines[0]).expect("合法行");
        let second = parse_line(&lines[1]).expect("合法行");
        assert_eq!(first["count"], json!(2));
        assert!(second.get("count").is_none(), "新窗口的第一条不写 count");
        assert_eq!(second["ts"], json!(1_061_000));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// 换 action / 换账号是不同键：各自独立，不共享计数。
    #[test]
    fn different_keys_are_not_merged() {
        let (dir, path) = temp_path("count-keys");
        record_at(
            &path,
            1,
            "cn",
            ACTION_DECRYPT_FAILED,
            REASON_DECRYPT_TIMEOUT,
            1_000_000,
        )
        .unwrap();
        record_at(
            &path,
            1,
            "cn",
            ACTION_DECRYPT_FAILED,
            REASON_DECRYPT_TIMEOUT,
            1_001_000,
        )
        .unwrap();
        record_at(&path, 1, "cn", ACTION_DECRYPT_OK, REASON_OK, 1_002_000).unwrap();
        record_at(
            &path,
            2,
            "cn",
            ACTION_DECRYPT_FAILED,
            REASON_DECRYPT_TIMEOUT,
            1_003_000,
        )
        .unwrap();
        record_at(
            &path,
            1,
            "ai",
            ACTION_DECRYPT_FAILED,
            REASON_DECRYPT_TIMEOUT,
            1_004_000,
        )
        .unwrap();

        let lines = read_lines(&path);
        assert_eq!(lines.len(), 4, "键不同必须各占一行");
        let counts: Vec<Option<u64>> = lines
            .iter()
            .filter_map(|line| parse_line(line))
            .map(|entry| entry.get("count").and_then(Value::as_u64))
            .collect();
        assert_eq!(counts, vec![Some(2), None, None, None]);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// 旧文件（没有 `count` 字段）仍可读，并在下一次同键上报时按 1 起累加。
    #[test]
    fn legacy_line_without_count_is_still_readable() {
        let (dir, path) = temp_path("legacy");
        std::fs::write(
            &path,
            "{\"ts\":1000000,\"account\":1,\"variant\":\"cn\",\"action\":\"decrypt_failed\",\"reason\":\"decrypt_timeout\"}\n",
        )
        .unwrap();
        assert_eq!(recent_at(&path, 10).len(), 1, "旧行照常可读");

        record_at(
            &path,
            1,
            "cn",
            ACTION_DECRYPT_FAILED,
            REASON_DECRYPT_TIMEOUT,
            1_000_500,
        )
        .unwrap();
        let lines = read_lines(&path);
        assert_eq!(lines.len(), 1);
        let entry = parse_line(&lines[0]).expect("合法行");
        assert_eq!(entry["count"], json!(2), "旧行按 1 起算");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// 累加重写不能把坏行丢掉：历史解析失败的行原样保留。
    #[test]
    fn count_rewrite_keeps_broken_lines() {
        let (dir, path) = temp_path("count-broken");
        let valid =
            "{\"ts\":1000000,\"account\":1,\"variant\":\"cn\",\"action\":\"decrypt_failed\",\"reason\":\"decrypt_timeout\"}";
        std::fs::write(&path, format!("not json\n{valid}\n")).unwrap();

        record_at(
            &path,
            1,
            "cn",
            ACTION_DECRYPT_FAILED,
            REASON_DECRYPT_TIMEOUT,
            1_000_500,
        )
        .unwrap();
        let lines = read_lines(&path);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "not json", "坏行必须原样保留");
        let entry = parse_line(&lines[1]).expect("合法行");
        assert_eq!(entry["count"], json!(2));

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
