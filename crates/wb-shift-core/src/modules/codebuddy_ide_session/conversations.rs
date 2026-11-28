//! 新版 CodeBuddy IDE（CLI 内核）会话库：列表与账号归属迁移。
//!
//! 2026-11-27 起 IDE 改用内置 CLI 内核，会话不再写
//! `<扩展数据根>/<uid>/CodeBuddyIDE/<uid>/history/<工作区>/index.json`，而是写
//! `~/.codebuddy/projects/<工作区>/<sessionId>.jsonl`，账号归属记在
//! `~/.codebuddy/codebuddy.db` 的 `conversations.user_id` 上（`conversation_runtime_sources`
//! 同列）。因此：
//!
//! - **列表**要同时读旧目录树（[`super::list`]）与这份库，否则升级后新建的会话在「复制会话」
//!   里完全不可见；
//! - **复制**不再搬文件（正文是所有账号共用的一份 JSONL），只把目标会话的 `user_id`
//!   从源账号改到目标账号，并在事务内同步 `conversation_runtime_sources`。
//!
//! 写入前提与旧库一致：IDE 必须已关闭，否则运行中的 IDE 会用内存状态覆盖；CLI 若正在跑，
//! 由 SQLite 的 busy timeout 兜底，超时后返回可读的错误提示，不做重试风暴。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use serde_json::{json, Value};

use crate::modules::config::{backup_dir, home_dir, utc_iso};
use crate::modules::vscode_session::is_safe_uid;

/// CLI 与新版 IDE 共用的用户目录（`~/.codebuddy`）。
const CLI_HOME_DIR: &str = ".codebuddy";
/// 会话库文件名。
const DB_FILE: &str = "codebuddy.db";
/// 会话正文目录（`<CLI_HOME>/projects/<工作区>/<sessionId>.jsonl`）。
const PROJECTS_DIR: &str = "projects";
/// 等待 CLI 释放写锁的上限；超过即报错让用户退出客户端，不无限挂起。
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);
/// 复制项的 `store` 取值：新版会话库。旧格式不写该字段，按 legacy 处理。
pub(super) const STORE_DB: &str = "db";

/// CLI 用户目录下的会话库路径；目录或文件不存在时 `None`。
fn db_path() -> Option<PathBuf> {
    let path = home_dir().join(CLI_HOME_DIR).join(DB_FILE);
    path.is_file().then_some(path)
}

fn projects_root() -> Option<PathBuf> {
    let path = home_dir().join(CLI_HOME_DIR).join(PROJECTS_DIR);
    path.is_dir().then_some(path)
}

/// 新版会话库是否可读（仅用于「列表要不要尝试合并」的快速判断）。
pub(super) fn available() -> bool {
    db_path().is_some()
}

/// 会话 id 的合法形状：会话库用 UUID，这里只放行字母数字与 `-`，避免把脏数据拼进 SQL 参数之外的地方。
fn is_safe_conversation_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
}

/// 稳定哈希：同一工作区每次列出的分组键必须一致（FNV-1a 两个种子拼成 32 位 hex，
/// 与旧库的 md5 分组键保持同形，前端不区分来源）。
fn stable_workspace_hash(input: &str) -> String {
    fn fnv1a(bytes: &[u8], seed: u64) -> u64 {
        let mut hash = seed;
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash
    }
    format!(
        "{:016x}{:016x}",
        fnv1a(input.as_bytes(), 0xcbf2_9ce4_8422_2325),
        fnv1a(input.as_bytes(), 0x9e37_79b9_7f4a_7c15)
    )
}

/// 正文索引：`sessionId → jsonl 路径`。
///
/// 目录布局有两种（`projects/<id>.jsonl` 与 `projects/<工作区>/<id>.jsonl`），只扫两层，
/// 不递归：CLI 自己也只写两层。
fn body_index(root: &Path) -> HashMap<String, PathBuf> {
    let mut index = HashMap::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return index;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let Ok(children) = std::fs::read_dir(&path) else {
                continue;
            };
            for child in children.flatten() {
                let file = child.path();
                if file.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
                    continue;
                }
                if let Some(stem) = file.file_stem().and_then(|stem| stem.to_str()) {
                    index.insert(stem.to_string(), file);
                }
            }
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("jsonl") {
            if let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) {
                index.insert(stem.to_string(), path);
            }
        }
    }
    index
}

fn open_read_only(path: &Path) -> Result<Connection, String> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| format!("打开 CodeBuddy 会话库失败：{error}"))?;
    conn.busy_timeout(BUSY_TIMEOUT)
        .map_err(|error| format!("设置会话库忙等超时失败：{error}"))?;
    Ok(conn)
}

fn has_conversations_table(conn: &Connection) -> bool {
    conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'conversations'",
        [],
        |row| row.get::<_, i64>(0),
    )
    .map(|count| count > 0)
    .unwrap_or(false)
}

/// 某表是否存在（新旧 CLI 的表集合会变，缺表时按「该能力不可用」降级，不把整次迁移判失败）。
fn has_table(conn: &Connection, name: &str) -> bool {
    conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
        params![name],
        |row| row.get::<_, i64>(0),
    )
    .map(|count| count > 0)
    .unwrap_or(false)
}

/// 某列是否存在（`revision` 等字段随 CLI 版本增减）。
fn has_column(conn: &Connection, table: &str, column: &str) -> bool {
    conn.prepare(&format!("PRAGMA table_info({table})"))
        .and_then(|mut statement| {
            let names = statement.query_map([], |row| row.get::<_, String>(1))?;
            Ok(names.flatten().any(|name| name == column))
        })
        .unwrap_or(false)
}

/// 列出某账号（`conversations.user_id`）在新版会话库里的会话。
///
/// 返回的行与旧库列表同形（`id` / `workspaceHash` / `title` / `updatedAt` / `type` /
/// `hasHistory`），额外带 `"store": "db"`，供前端原样回传给复制请求。
/// 库不存在、旧版 CLI 没有该表时返回空数组——调用方据此保持「只有旧会话」的行为。
pub(super) fn list_sessions(uid: &str) -> Vec<Value> {
    let Some(path) = db_path() else {
        return Vec::new();
    };
    list_sessions_in(&path, projects_root().as_deref(), uid)
}

/// 列表内核（会话库路径与正文根已解析），供单测注入临时目录。
pub(super) fn list_sessions_in(path: &Path, projects: Option<&Path>, uid: &str) -> Vec<Value> {
    if !is_safe_uid(uid) || !path.is_file() {
        return Vec::new();
    }
    let Ok(conn) = open_read_only(path) else {
        return Vec::new();
    };
    if !has_conversations_table(&conn) {
        return Vec::new();
    }
    let bodies = projects.map(body_index).unwrap_or_default();
    let mut statement = match conn.prepare(
        "SELECT id, \
                COALESCE(NULLIF(custom_title, ''), NULLIF(title, '')) AS label, \
                COALESCE(workspace_key, '') AS workspace_key, \
                COALESCE(type, '') AS ctype, \
                COALESCE(last_message_at, created_at, 0) AS updated \
         FROM conversations \
         WHERE user_id = ?1 AND deleted_at IS NULL AND archived_at IS NULL \
         ORDER BY updated DESC",
    ) {
        Ok(statement) => statement,
        Err(_) => return Vec::new(),
    };
    let rows = statement.query_map(params![uid], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, i64>(4)?,
        ))
    });
    let Ok(rows) = rows else {
        return Vec::new();
    };
    let mut sessions = Vec::new();
    for row in rows.flatten() {
        let (id, label, workspace_key, ctype, updated) = row;
        if !is_safe_conversation_id(&id) {
            continue;
        }
        let title = label
            .map(|text| text.trim().to_string())
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| "(无标题)".to_string());
        let has_history = bodies
            .get(&id)
            .and_then(|path| std::fs::metadata(path).ok())
            .map(|meta| meta.len() > 0)
            .unwrap_or(false);
        let group_key = if workspace_key.trim().is_empty() {
            "unknown".to_string()
        } else {
            workspace_key
        };
        sessions.push(json!({
            "id": id,
            "workspaceHash": stable_workspace_hash(&group_key),
            "title": title,
            "updatedAt": updated.max(0),
            "type": ctype,
            "hasHistory": has_history,
            "store": STORE_DB,
        }));
    }
    sessions
}

/// 把勾选会话在会话库里的账号归属从 `source_uid` 改到 `target_uid`。
///
/// 写前先 `VACUUM INTO` 出一份一致性备份（WAL 中的未落盘内容也会包含在内）；随后在单个
/// 事务里改 `conversations.user_id` 与 `conversation_runtime_sources.user_id`。
/// 正文 JSONL 是各账号共用的，不搬文件、不改 id，所以「沿用会话 id」的既有语义保持不变。
pub(super) fn migrate_sessions(
    source_uid: &str,
    target_uid: &str,
    ids: &[String],
) -> Result<Value, String> {
    let path =
        db_path().ok_or_else(|| "未找到 CodeBuddy 会话库（~/.codebuddy/codebuddy.db）".to_string())?;
    let backup_root = backup_dir().join("codebuddy-ide-db").join(utc_iso());
    migrate_sessions_in(&path, &backup_root, source_uid, target_uid, ids)
}

pub(super) fn migrate_sessions_in(
    path: &Path,
    backup_root: &Path,
    source_uid: &str,
    target_uid: &str,
    ids: &[String],
) -> Result<Value, String> {
    if !is_safe_uid(source_uid) || !is_safe_uid(target_uid) {
        return Err("账号 uid 非法，无法迁移会话".to_string());
    }
    let conn = Connection::open(path).map_err(|error| format!("打开 CodeBuddy 会话库失败：{error}"))?;
    conn.busy_timeout(BUSY_TIMEOUT)
        .map_err(|error| format!("设置会话库忙等超时失败：{error}"))?;
    if !has_conversations_table(&conn) {
        return Err("当前 CodeBuddy CLI 版本没有会话库表，无法迁移会话".to_string());
    }

    std::fs::create_dir_all(backup_root)
        .map_err(|error| format!("创建会话库备份目录失败：{error}"))?;
    let backup = backup_root.join("codebuddy.db");
    if backup.exists() {
        let _ = std::fs::remove_file(&backup);
    }
    // VACUUM INTO 会连同 WAL 里的内容生成一份一致快照，比直接复制文件可靠。
    conn.execute("VACUUM INTO ?1", params![backup.to_string_lossy()])
        .map_err(|error| format!("备份 CodeBuddy 会话库失败：{error}（请退出 CodeBuddy CLI 后重试）"))?;

    let mut copied: Vec<Value> = Vec::new();
    let mut skipped: Vec<Value> = Vec::new();
    let mut errors: Vec<Value> = Vec::new();
    // `revision` / `conversation_runtime_sources` 是「尽力而为」的附加信息：旧版 CLI 没有时
    // 只改归属，不因此整次失败。
    let update_conversation = if has_column(&conn, "conversations", "revision") {
        "UPDATE conversations SET user_id = ?2, revision = COALESCE(revision, 0) + 1 \
         WHERE id = ?1 AND user_id = ?3"
    } else {
        "UPDATE conversations SET user_id = ?2 WHERE id = ?1 AND user_id = ?3"
    };
    let has_runtime_sources = has_table(&conn, "conversation_runtime_sources")
        && has_column(&conn, "conversation_runtime_sources", "user_id");
    let tx = conn
        .unchecked_transaction()
        .map_err(|error| format!("开始会话库事务失败：{error}"))?;
    for id in ids {
        if !is_safe_conversation_id(id) {
            errors.push(json!({ "conversationId": id, "error": "会话 id 非法" }));
            continue;
        }
        let changed = tx
            .execute(update_conversation, params![id, target_uid, source_uid])
            .map_err(|error| format!("写入 CodeBuddy 会话库失败：{error}（请退出 CodeBuddy CLI / IDE 后重试）"))?;
        if changed == 0 {
            let owner: Option<Option<String>> = tx
                .query_row(
                    "SELECT user_id FROM conversations WHERE id = ?1",
                    params![id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|error| format!("读取会话归属失败：{error}"))?;
            match owner {
                None => errors.push(json!({ "conversationId": id, "error": "会话不存在" })),
                Some(Some(uid)) if uid == target_uid => {
                    skipped.push(json!({ "conversationId": id, "reason": "已属于目标账号" }))
                }
                _ => skipped.push(json!({ "conversationId": id, "reason": "不属于当前账号" })),
            }
            continue;
        }
        if has_runtime_sources {
            tx.execute(
                "UPDATE conversation_runtime_sources SET user_id = ?2 \
                 WHERE conversation_id = ?1 AND user_id = ?3",
                params![id, target_uid, source_uid],
            )
            .map_err(|error| format!("写入会话运行时来源失败：{error}"))?;
        }
        copied.push(json!({
            "workspaceHash": "",
            "oldId": id,
            "newId": id,
            "messages": 0,
        }));
    }
    tx.commit()
        .map_err(|error| format!("提交会话库事务失败：{error}"))?;

    Ok(json!({
        "sourceUid": source_uid,
        "targetUid": target_uid,
        "copied": copied,
        "skipped": skipped,
        "errors": errors,
        "backup": backup.to_string_lossy(),
    }))
}
