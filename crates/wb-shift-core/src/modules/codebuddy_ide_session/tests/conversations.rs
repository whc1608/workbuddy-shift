// 新版会话库（`~/.codebuddy/codebuddy.db`）的单测：临时库 + 临时正文目录。

use crate::modules::codebuddy_ide_session::conversations::{
    conversation_account_uids_in, list_sessions_in, migrate_sessions_in,
};
use rusqlite::Connection;
use std::path::PathBuf;

const SRC: &str = "a6efe09f-9093-4606-90c2-c69a4dc696e9";
const DST: &str = "536d52a9-394e-4e32-9213-d39a58da4d64";
const ID_A: &str = "aaaaaaaa-1111-2222-3333-444444444444";
const ID_B: &str = "bbbbbbbb-1111-2222-3333-444444444444";
const ID_DST: &str = "cccccccc-1111-2222-3333-444444444444";

fn temp_root(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "wb-shift-conv-{name}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn seed_db(path: &Path) {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(&format!(
        "CREATE TABLE conversations (
            id TEXT PRIMARY KEY, user_id TEXT, workspace_key TEXT, title TEXT, custom_title TEXT,
            type TEXT, created_at INTEGER, last_message_at INTEGER, deleted_at INTEGER,
            archived_at INTEGER, revision INTEGER);
         CREATE TABLE conversation_runtime_sources (
            user_id TEXT, conversation_id TEXT, runtime_session_id TEXT);
         INSERT INTO conversations VALUES ('{ID_A}', '{SRC}', 'uri-v1:file:///F%3A/a', '会话一', NULL, 'craft', 1000, 2000, NULL, NULL, 0);
         INSERT INTO conversations VALUES ('{ID_B}', '{SRC}', 'uri-v1:file:///F%3A/b', NULL, '自定义标题', 'craft', 1000, 3000, NULL, NULL, 0);
         INSERT INTO conversations VALUES ('{ID_DST}', '{DST}', 'uri-v1:file:///F%3A/c', '别人的', NULL, 'craft', 1000, 4000, NULL, NULL, 0);
         INSERT INTO conversations VALUES ('dddddddd-1111-2222-3333-444444444444', '{SRC}', 'uri-v1:file:///F%3A/d', '已删除', NULL, 'craft', 1000, 5000, 1, NULL, 0);
         INSERT INTO conversation_runtime_sources VALUES ('{SRC}', '{ID_A}', 'runtime-1');"
    ))
    .unwrap();
}

#[test]
fn list_new_store_sessions_only_for_current_account() {
    let root = temp_root("list");
    let db = root.join("codebuddy.db");
    seed_db(&db);
    let projects = root.join("projects");
    std::fs::create_dir_all(projects.join("workspace-a")).unwrap();
    std::fs::write(projects.join("workspace-a").join(format!("{ID_A}.jsonl")), b"{}\n").unwrap();
    // 顶层布局 + 空文件：仍要列出，但 hasHistory 为 false。
    std::fs::write(projects.join(format!("{ID_B}.jsonl")), b"").unwrap();

    let rows = list_sessions_in(&db, Some(&projects), SRC);
    assert_eq!(rows.len(), 2, "只应列当前账号且未删除的会话: {rows:?}");

    let first = rows
        .iter()
        .find(|row| row["id"] == ID_B)
        .expect("自定义标题的会话应出现");
    assert_eq!(first["title"], "自定义标题");
    assert_eq!(first["hasHistory"], false);
    assert_eq!(first["store"], "db");

    let second = rows
        .iter()
        .find(|row| row["id"] == ID_A)
        .expect("有正文的会话应出现");
    assert_eq!(second["hasHistory"], true);
    assert_eq!(second["workspaceHash"].as_str().unwrap().len(), 32);

    // 其他账号与已删除的会话不得出现。
    assert!(rows.iter().all(|row| row["id"] != ID_DST));
    assert!(rows.iter().all(|row| row["id"] != "dddddddd-1111-2222-3333-444444444444"));

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn migrate_moves_ownership_and_backs_up() {
    let root = temp_root("migrate");
    let db = root.join("codebuddy.db");
    seed_db(&db);
    let backup_root = root.join("backups");
    let ids = vec![
        ID_A.to_string(),
        ID_B.to_string(),
        ID_DST.to_string(),
        "bad id".to_string(),
    ];

    let report = migrate_sessions_in(&db, &backup_root, SRC, DST, &ids).unwrap();
    assert_eq!(report["copied"].as_array().unwrap().len(), 2, "{report:?}");
    let skipped = report["skipped"].as_array().unwrap();
    assert!(skipped.iter().any(|item| item["conversationId"] == ID_DST));
    let errors = report["errors"].as_array().unwrap();
    assert!(errors.iter().any(|item| item["conversationId"] == "bad id"));
    assert!(backup_root.join("codebuddy.db").is_file(), "迁移前应留下备份");

    let conn = Connection::open(&db).unwrap();
    let owner: String = conn
        .query_row(
            "SELECT user_id FROM conversations WHERE id = ?1",
            rusqlite::params![ID_A],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(owner, DST);
    let source_owner: String = conn
        .query_row(
            "SELECT user_id FROM conversation_runtime_sources WHERE conversation_id = ?1",
            rusqlite::params![ID_A],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(source_owner, DST);
    let revision: i64 = conn
        .query_row(
            "SELECT revision FROM conversations WHERE id = ?1",
            rusqlite::params![ID_A],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(revision, 1, "迁移应递增 revision，让客户端识别变更");
    drop(conn);

    // 重复迁移：全部走 skipped，不重复计数。
    let again = migrate_sessions_in(&db, &backup_root, SRC, DST, &[ID_A.to_string()]).unwrap();
    assert_eq!(again["copied"].as_array().unwrap().len(), 0);
    assert_eq!(again["skipped"].as_array().unwrap().len(), 1);

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn migrate_rejects_missing_table_and_unsafe_uid() {
    let root = temp_root("guard");
    let db = root.join("codebuddy.db");
    Connection::open(&db)
        .unwrap()
        .execute_batch("CREATE TABLE other (id TEXT);")
        .unwrap();
    let error = migrate_sessions_in(&db, &root.join("backups"), SRC, DST, &[ID_A.to_string()])
        .unwrap_err();
    assert!(error.contains("会话库表"), "{error}");

    let error = migrate_sessions_in(&db, &root.join("backups"), "../bad", DST, &[])
        .unwrap_err();
    assert!(error.contains("uid 非法"), "{error}");

    let _ = std::fs::remove_dir_all(&root);
}

/// 限额归因兜底：会话 id → 归属 uid。只返回库里真实存在、且有 uid 的会话。
#[test]
fn conversation_account_uids_maps_known_sessions_only() {
    let root = temp_root("owner");
    let db = root.join("codebuddy.db");
    seed_db(&db);

    let ids = vec![
        ID_A.to_string(),
        ID_DST.to_string(),
        "not-in-db".to_string(),
        "bad id!".to_string(),
        String::new(),
    ];
    let owners = conversation_account_uids_in(&db, &ids);
    assert_eq!(owners.len(), 2, "{owners:?}");
    assert_eq!(owners.get(ID_A).map(String::as_str), Some(SRC));
    assert_eq!(owners.get(ID_DST).map(String::as_str), Some(DST));
    assert!(!owners.contains_key("not-in-db"));
    assert!(!owners.contains_key("bad id!"));

    // 空入参、库不存在、库没有该表时都返回空表（调用方据此继续丢弃事件）。
    assert!(conversation_account_uids_in(&db, &[]).is_empty());
    assert!(conversation_account_uids_in(&root.join("missing.db"), &ids).is_empty());
    let plain = root.join("plain.db");
    Connection::open(&plain)
        .unwrap()
        .execute_batch("CREATE TABLE other (id TEXT);")
        .unwrap();
    assert!(conversation_account_uids_in(&plain, &ids).is_empty());

    let _ = std::fs::remove_dir_all(&root);
}
