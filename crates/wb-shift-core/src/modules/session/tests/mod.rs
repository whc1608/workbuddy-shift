//! 会话复制/关联/恢复的端到端单测。
//!
//! 所有用例都在临时目录里构造数据根与存储根，绝不读写真实的 `~/.workbuddy-shift`
//! 或 WorkBuddy 数据目录。
//!
//! 用例按主题拆到同目录的片段文件，由 `include!` 平铺进本模块：测试全名保持
//! `modules::session::tests::*` 与拆分前逐字一致（cargo test --list 基线），
//! 片段文件各自保留原有分节注释，便于按主题查找。

use super::copy::*;
use super::exec::*;
use super::recovery::{
    recover_pending_session_operations_at, recover_pending_session_operations_at_with,
};
use super::run::*;
use super::sync::*;
use super::*;
use crate::modules::session_link::{LinkStore, MemberState, Operation, StoreState};
use serde_json::json;

struct Env {
    root: PathBuf,
    paths: SessionPaths,
}

impl Env {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "wb_shift_copy_test_{}_{name}",
            uuid::Uuid::new_v4().simple()
        ));
        let paths = SessionPaths {
            store_root: root.join("store"),
            data_root: root.join("data"),
            auth_file: root.join("auth.info"),
            link_namespace: LinkNamespace::WorkBuddy,
        };
        std::fs::create_dir_all(paths.projects_dir().join("ws-a")).unwrap();
        Env { root, paths }
    }

    fn paths(&self) -> SessionPaths {
        self.paths.clone()
    }

    fn set_login(&self, uid: &str) {
        std::fs::write(
            &self.paths.auth_file,
            json!({"account": {"uid": uid}}).to_string(),
        )
        .unwrap();
    }

    fn create_db(&self) {
        let conn = Connection::open(self.paths.workbuddy_db()).unwrap();
        conn.execute_batch(
            "CREATE TABLE sessions (
                id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL,
                title TEXT,
                custom_title TEXT,
                cwd TEXT,
                created_at INTEGER,
                updated_at INTEGER,
                deleted_at INTEGER,
                is_playground INTEGER
            );",
        )
        .unwrap();
    }

    fn create_edge_db(&self, variant: WbVariant) {
        let conn = Connection::open(self.paths.edge_sync_db(variant)).unwrap();
        conn.execute_batch(
            "CREATE TABLE edge_sync_mapping (
                session_id TEXT,
                conversation_id TEXT,
                msg_channel TEXT,
                created_at INTEGER
            );",
        )
        .unwrap();
    }

    /// 带 `sessions.status` 列的表：真实库有该列，默认建表没有。
    ///
    /// 默认值与真实库一致（`Pending`，不在终态白名单内），避免「缺省即可归档」。
    fn create_db_with_status(&self) {
        let conn = Connection::open(self.paths.workbuddy_db()).unwrap();
        conn.execute_batch(
            "CREATE TABLE sessions (
                id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL,
                title TEXT,
                custom_title TEXT,
                status TEXT NOT NULL DEFAULT 'Pending',
                cwd TEXT,
                created_at INTEGER,
                updated_at INTEGER,
                deleted_at INTEGER,
                is_playground INTEGER
            );",
        )
        .unwrap();
    }

    fn add_session(&self, id: &str, uid: &str, title: &str) {
        let conn = Connection::open(self.paths.workbuddy_db()).unwrap();
        conn.execute(
            "INSERT INTO sessions (id, user_id, title, custom_title, cwd, created_at, updated_at, deleted_at, is_playground)
             VALUES (?1, ?2, ?3, NULL, '/ws/a', 1000, 2000, NULL, 0)",
            rusqlite::params![id, uid, title],
        )
        .unwrap();
    }

    fn add_body(&self, cid: &str, text: &str) -> PathBuf {
        let path = self
            .paths
            .projects_dir()
            .join("ws-a")
            .join(format!("{cid}.jsonl"));
        std::fs::write(&path, text).unwrap();
        path
    }

    fn body_path(&self, cid: &str) -> PathBuf {
        self.paths
            .projects_dir()
            .join("ws-a")
            .join(format!("{cid}.jsonl"))
    }

    fn delete_row(&self, id: &str) {
        let conn = Connection::open(self.paths.workbuddy_db()).unwrap();
        conn.execute("DELETE FROM sessions WHERE id = ?1", [id])
            .unwrap();
    }

    /// 软删除：真实库未删除时 `deleted_at` 是 NULL，删除后写时间戳。
    fn soft_delete_row(&self, id: &str) {
        let conn = Connection::open(self.paths.workbuddy_db()).unwrap();
        conn.execute(
            "UPDATE sessions SET deleted_at = 1700000000000 WHERE id = ?1",
            [id],
        )
        .unwrap();
    }

    /// 改写会话行归属（模拟行被其它账号接管）。
    fn set_row_owner(&self, id: &str, uid: &str) {
        let conn = Connection::open(self.paths.workbuddy_db()).unwrap();
        conn.execute(
            "UPDATE sessions SET user_id = ?1 WHERE id = ?2",
            rusqlite::params![uid, id],
        )
        .unwrap();
    }

    /// 设置会话状态（模拟用户归档/取消归档、App 写入活跃态）。
    fn set_status(&self, id: &str, status: &str) {
        let conn = Connection::open(self.paths.workbuddy_db()).unwrap();
        conn.execute(
            "UPDATE sessions SET status = ?1 WHERE id = ?2",
            rusqlite::params![status, id],
        )
        .unwrap();
    }

    /// 设置更新时间戳（构造「本次时间戳已落库」的中断现场）。
    fn set_updated_at(&self, id: &str, updated_at: i64) {
        let conn = Connection::open(self.paths.workbuddy_db()).unwrap();
        conn.execute(
            "UPDATE sessions SET updated_at = ?1 WHERE id = ?2",
            rusqlite::params![updated_at, id],
        )
        .unwrap();
    }

    /// 给 `sessions` 补 `last_activity_at` 列（真实库有该列，默认建表没有）。
    ///
    /// 默认建表刻意不含它：老库兼容用例靠「没有这一列」证明代码不写死该列。
    fn add_activity_column(&self) {
        let conn = Connection::open(self.paths.workbuddy_db()).unwrap();
        conn.execute(
            "ALTER TABLE sessions ADD COLUMN last_activity_at INTEGER",
            [],
        )
        .unwrap();
    }

    /// 直接写一行的 `last_activity_at`（构造「目标侧更新/更旧」的现场）。
    fn set_activity(&self, id: &str, at: i64) {
        let conn = Connection::open(self.paths.workbuddy_db()).unwrap();
        conn.execute(
            "UPDATE sessions SET last_activity_at = ?1 WHERE id = ?2",
            rusqlite::params![at, id],
        )
        .unwrap();
    }

    /// 把一行的 `last_activity_at` 置为 NULL（构造「客户端从未写过这一列」的现场）。
    fn clear_activity(&self, id: &str) {
        let conn = Connection::open(self.paths.workbuddy_db()).unwrap();
        conn.execute(
            "UPDATE sessions SET last_activity_at = NULL WHERE id = ?1",
            [id],
        )
        .unwrap();
    }

    /// 一行的 `last_activity_at`；列不存在或值为 NULL 都按 None 处理。
    fn activity_of(&self, id: &str) -> Option<i64> {
        let conn = Connection::open(self.paths.workbuddy_db()).unwrap();
        conn.query_row(
            "SELECT last_activity_at FROM sessions WHERE id = ?1",
            [id],
            |row| row.get::<_, Option<i64>>(0),
        )
        .ok()
        .flatten()
    }

    fn status_of(&self, id: &str) -> Option<String> {
        let conn = Connection::open(self.paths.workbuddy_db()).unwrap();
        conn.query_row("SELECT status FROM sessions WHERE id = ?1", [id], |row| {
            row.get::<_, Option<String>>(0)
        })
        .ok()
        .flatten()
    }

    fn target(&self, uid: &str) -> Value {
        json!({"id": format!("acc-{uid}"), "uid": uid, "variant": "cn"})
    }

    fn store(&self) -> LinkStore {
        match session_link::load_store(&self.paths) {
            StoreState::Ready(store) => store,
            other => panic!("同步记录应为 Ready，实际 {other:?}"),
        }
    }

    fn body_files(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.paths.projects_dir().join("ws-a"))
            .unwrap()
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().to_string();
                name.ends_with(".jsonl").then_some(name)
            })
            .collect();
        names.sort();
        names
    }

    /// 基线目录里的 `*.json` 数量（判断恢复是否新增基线）。
    fn baseline_files(&self) -> usize {
        std::fs::read_dir(self.paths.baselines_dir())
            .map(|entries| {
                entries
                    .flatten()
                    .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
                    .count()
            })
            .unwrap_or(0)
    }

    /// 云端映射表行数（判断恢复是否重复登记）。
    fn mapping_rows(&self) -> usize {
        let conn = Connection::open(self.paths.edge_sync_db(WbVariant::Cn)).unwrap();
        conn.query_row("SELECT COUNT(*) FROM edge_sync_mapping", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap() as usize
    }

    fn rows_for(&self, uid: &str) -> Vec<String> {
        let conn = Connection::open(self.paths.workbuddy_db()).unwrap();
        let mut stmt = conn
            .prepare("SELECT id FROM sessions WHERE user_id = ?1 AND deleted_at IS NULL")
            .unwrap();
        let mut rows: Vec<String> = stmt
            .query_map([uid], |row| row.get::<_, String>(0))
            .unwrap()
            .flatten()
            .collect();
        rows.sort();
        rows
    }

    fn first_copy_id(&self, report: &Value) -> String {
        report["copied"][0]["newId"].as_str().unwrap().to_string()
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn body_text(cid: &str) -> String {
    format!(
        "{}\n{}\n",
        json!({"type": "user", "sessionId": cid, "text": "你好"}),
        json!({"type": "assistant", "sessionId": cid, "text": "hi"})
    )
}

/// 一个可用的国内版环境：源账号 uid-a 有一个带正文的会话 sess-1。
fn ready_env(name: &str) -> Env {
    let env = Env::new(name);
    env.create_db();
    env.create_edge_db(WbVariant::Cn);
    env.set_login("uid-a");
    env.add_session("sess-1", "uid-a", "标题一");
    env.add_body("sess-1", &body_text("sess-1"));
    env
}

fn copy(env: &Env, target_uid: &str, ids: &[&str]) -> Value {
    let ids: Vec<String> = ids.iter().map(|id| id.to_string()).collect();
    copy_sessions_for_switch_at(
        &env.paths(),
        WbVariant::Cn,
        &env.target(target_uid),
        &ids,
        false,
        |_| false,
    )
    .unwrap()
}

// ---- 旧签名 shim（测试专用） ----

/// 显式打开「连账本一起复制」（R6-08 之前的旧口径）的复制入口。
///
/// 只有验证「开关打开时账本仍逐字搬运」的用例用它；生产默认口径恒为不搬。
fn copy_with_usage(env: &Env, target_uid: &str, ids: &[&str]) -> Value {
    let ids: Vec<String> = ids.iter().map(|id| id.to_string()).collect();
    copy_sessions_for_switch_at_with_options(
        &env.paths(),
        WbVariant::Cn,
        &env.target(target_uid),
        &ids,
        CopyOptions::new(false, false).with_copy_usage(true),
        |_| false,
    )
    .unwrap()
}

/// `copy_sessions_for_switch_at` 的旧签名（`ignore_aux: bool`）包装：多数用例只关心
/// 「不忽略账本」，不必每处都写 `CopyOptions`（生产侧统一走 `*_with_options`）。
fn copy_sessions_for_switch_at(
    paths: &SessionPaths,
    variant: WbVariant,
    target_acc: &Value,
    session_ids: &[String],
    ignore_aux: bool,
    is_app_running: impl Fn(WbVariant) -> bool,
) -> Result<Value, String> {
    copy_sessions_for_switch_at_with_options(
        paths,
        variant,
        target_acc,
        session_ids,
        CopyOptions::new(ignore_aux, false),
        is_app_running,
    )
}

/// `copy_sessions_cross_at` 的旧签名包装（同上）。
fn copy_sessions_cross_at(
    source: CopySide<'_>,
    target: CopySide<'_>,
    session_ids: &[String],
    ignore_aux: bool,
    is_app_running: impl Fn(WbVariant) -> bool,
    resolve_source_paths: impl Fn(WbVariant) -> SessionPaths,
) -> Result<Value, String> {
    copy_sessions_cross_at_with_options(
        source,
        target,
        session_ids,
        CopyOptions::new(ignore_aux, false),
        is_app_running,
        resolve_source_paths,
    )
}

// ---- 主题片段（按原文件顺序拼回，测试名与排序均不变） ----

include!("query.rs");

include!("query_spaces.rs");

include!("copy.rs");

include!("copy_same_content.rs");

include!("pinned.rs");

include!("run.rs");

include!("sync.rs");

include!("sync_plan_reasons.rs");

include!("exec.rs");

include!("recovery.rs");

include!("copy_repair_qa.rs");

include!("copy_repair_qa3.rs");

include!("memory.rs");
