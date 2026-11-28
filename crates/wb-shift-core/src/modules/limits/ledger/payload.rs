//! 账号归因与 payload 组装：会话→账号、UID→账号、活跃账号与 Resolved 产出。

use super::*;

// ---------------------------------------------------------------------------
// 汇总
// ---------------------------------------------------------------------------

/// sessionId → 账号 id。
///
/// 归因失败（`sessions` 表缺失、会话不在库、uid 未收录）时返回空映射，调用方据此丢弃
/// 该事件——宁可少显示，不可显示错账号。hook 通路（`rate_limit_events.rs`）的
/// WorkBuddy 兜底归因复用本函数。
pub(crate) fn account_by_session(
    variant: WbVariant,
    session_ids: &BTreeSet<String>,
) -> HashMap<String, String> {
    let mut mapping = HashMap::new();
    if session_ids.is_empty() {
        return mapping;
    }
    let uid_to_account: HashMap<String, String> = account::load_accounts()
        .iter()
        .filter(|acc| account::variant_of(acc) == variant)
        .filter_map(|acc| Some((account::get_str(acc, "uid")?, account::get_str(acc, "id")?)))
        .collect();
    if uid_to_account.is_empty() {
        return mapping;
    }
    let db = workbuddy_db_path(variant);
    if !db.is_file() {
        return mapping;
    }
    let Some(conn) = open_db(&db, true) else {
        return mapping;
    };
    if !table_exists(&conn, "sessions") {
        return mapping;
    }
    // 批量查询：逐条查库在事件多时会成为主要开销。
    let placeholders = vec!["?"; session_ids.len()].join(",");
    let sql = format!("SELECT id, user_id FROM sessions WHERE id IN ({placeholders})");
    let Ok(mut statement) = conn.prepare(&sql) else {
        return mapping;
    };
    let Ok(rows) = statement.query_map(rusqlite::params_from_iter(session_ids.iter()), |row| {
        Ok((
            row.get::<_, Option<String>>(0)?,
            row.get::<_, Option<String>>(1)?,
        ))
    }) else {
        return mapping;
    };
    for (session_id, user_id) in rows.flatten() {
        let (Some(session_id), Some(user_id)) = (session_id, user_id) else {
            continue;
        };
        if let Some(account_id) = uid_to_account.get(&user_id) {
            mapping.insert(session_id, account_id.clone());
        }
    }
    mapping
}

/// 账号归因：归因不到账号的事件直接丢弃（不错归给任何账号）。
pub(crate) fn resolve(variant: WbVariant, events: &[Event], source: &str) -> Vec<Resolved> {
    let session_ids: BTreeSet<String> = events
        .iter()
        .filter_map(|event| event.session_id.clone())
        .collect();
    let mapping = account_by_session(variant, &session_ids);
    events
        .iter()
        .filter_map(|event| {
            let account_id = event
                .session_id
                .as_deref()
                .and_then(|session| mapping.get(session))?;
            Some(Resolved {
                account_id: account_id.clone(),
                model: event.model.clone(),
                reset_at: event.reset_at,
                first_seen_at: event.first_seen_at,
                hit_count: event.hit_count,
                source: source.to_string(),
            })
        })
        .collect()
}

/// 来源状态文件里的「当前生效账号」。
///
/// `activeAccountId` 是**账号库的 `id`，不是 uid**（三个状态文件均如此）。
pub(crate) struct ActiveAccount {
    pub(crate) account_id: Option<String>,
    pub(crate) updated_at: Option<i64>,
}

impl ActiveAccount {
    /// 读该来源的状态文件；文件缺失/损坏时回落不可用（返回空，不是错误）。
    pub(crate) fn load(path: &Path) -> Self {
        let state = std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .unwrap_or_else(|| json!({}));
        Self {
            account_id: state
                .get("activeAccountId")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .map(str::to_string),
            updated_at: state.get("updatedAt").and_then(Value::as_i64),
        }
    }

    /// 该状态文件能否作为这一事件的归因依据。
    ///
    /// 必须 `updatedAt ≤ 事件时刻`：IDE 的 `detect_current_account()` 匹配成功时也会回写状态
    /// 文件（`codebuddy_ide.rs` / `codebuddy_cn_ide.rs`），使 `updatedAt` 晚于事件却并非账号
    /// 切换。缺 `updatedAt` 时无从证实「当时生效的是谁」，同样不用。
    pub(crate) fn account_for(&self, first_seen_at: i64) -> Option<&str> {
        let account_id = self.account_id.as_deref()?;
        (self.updated_at? <= first_seen_at).then_some(account_id)
    }
}

/// 账号库 `uid → 账号 id` 映射（CLI 与两个 IDE 的日志内鉴权 uid 归因用）。
///
/// 不按档位过滤：CLI 跨两档位共用同一套账号库，IDE 侧写入的账号也未必与日志来源档位一致；
/// uid 在账号库内唯一（导入即按 uid 去重），过滤只会漏、不会错。
pub(crate) fn account_id_by_uid() -> HashMap<String, String> {
    account::load_accounts()
        .iter()
        .filter_map(|acc| Some((account::get_str(acc, "uid")?, account::get_str(acc, "id")?)))
        .collect()
}

/// 新来源（CLI / 两个 IDE）的账号归因。
///
/// ① 事件 uid → 账号库 `uid → id`（日志内鉴权行，时态正确）；② 回落该来源状态文件的
/// `activeAccountId`（带 `updatedAt ≤ 事件时刻` 门控）；③ 两者都取不到时，用客户端自己
/// 记的会话归属兜底（[`session_owner_account`]，见其注释）；④ 兜底也取不到 → 丢弃该事件，
/// 不错归给任何账号。
///
/// 第 ③ 条不可省：CLI 的鉴权行只写在 `AuthDoInitProbe` 命中的日志里，而轮换状态文件
/// （`~/.codebuddy-rotate/state.json`）只在**用户用本工具切过 CLI 账号**后才存在。日常
/// 「一直用同一个账号」的机器上两条链同时为空，429 会被整条丢掉 —— 账号卡片不出限额标记，
/// 用户只看到模型报限额（2026-11-28 本机实证）。
pub(crate) fn resolve_by_uid(
    events: &[Event],
    uid_to_account: &HashMap<String, String>,
    fallback: &ActiveAccount,
    source: &str,
) -> Vec<Resolved> {
    // 只对「前两条链都取不到」的事件查会话库：一次批量查询，不逐条开库。
    let unresolved: Vec<String> = events
        .iter()
        .filter(|event| {
            event
                .uid
                .as_deref()
                .and_then(|uid| uid_to_account.get(uid))
                .is_none()
                && fallback.account_for(event.first_seen_at).is_none()
        })
        .filter_map(|event| event.session_id.clone())
        .collect();
    let owners = session_owner_account(&unresolved);
    events
        .iter()
        .filter_map(|event| {
            let account_id = event
                .uid
                .as_deref()
                .and_then(|uid| uid_to_account.get(uid))
                .map(String::as_str)
                .or_else(|| fallback.account_for(event.first_seen_at))
                .or_else(|| {
                    event
                        .session_id
                        .as_deref()
                        .and_then(|id| owners.get(id))
                        .map(String::as_str)
                })?;
            Some(Resolved {
                account_id: account_id.to_string(),
                model: event.model.clone(),
                reset_at: event.reset_at,
                first_seen_at: event.first_seen_at,
                hit_count: event.hit_count,
                source: source.to_string(),
            })
        })
        .collect()
}

/// 会话 id → 账号库 id（会话库归属 + `uid → id` 映射）。
///
/// 归属由 CLI / IDE 自己写在 `~/.codebuddy/codebuddy.db` 的 `conversations.user_id`，与
/// 「谁在持 key」是同一事实；这里只做一次读，库缺失或会话不在库里时返回空表。
fn session_owner_account(session_ids: &[String]) -> HashMap<String, String> {
    if session_ids.is_empty() {
        return HashMap::new();
    }
    let owners = crate::modules::codebuddy_ide_session::conversation_account_uids(session_ids);
    if owners.is_empty() {
        return HashMap::new();
    }
    let uid_to_account = account_id_by_uid();
    owners
        .into_iter()
        .filter_map(|(session, uid)| Some((session, uid_to_account.get(&uid)?.clone())))
        .collect()
}
