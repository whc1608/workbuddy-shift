//! 会话列表与按需复制（路径 B：生成新 id，云端可正常同步）。
//!
//! 对照 server.py `current_user_uid` / `list_sessions_for_user` /
//! `_find_project_jsonl` / `copy_session_to_user` / `_register_edge_sync_mapping` /
//! `copy_sessions_for_switch` / `backup_workbuddy_db` / `workbuddy_db_path`。
//!
//! WorkBuddy 5.x 数据三件套（缺一不可）：
//!   1) 正文：`~/.workbuddy/projects/{workspace}/{cid}.jsonl`（JSONL 含 sessionId 字段）
//!   2) 元数据：`~/.workbuddy/workbuddy.db` sessions 表（id = conversation id = UUID）
//!   3) 云端映射：`~/.workbuddy/edge-sync-mapping-v{N}.db` edge_sync_mapping
//!      （文件名版本号由客户端演进，按最大版本号动态发现）
//!      （session_id=conversation_id，msg_channel=convmsg:{uid} 决定云端归属）
//!      **不由本工具写入**：复制只写 1)、2)，映射行由 edge-sync 扩展在客户端下次
//!      启动迁移副本时自行写入（预写会让客户端判定"已迁移"跳过上传，云端缺会话）。
//!
//! 复制收口（design §4）：所有复制入口统一走 [`copy_sessions_for_switch`]，在同一把
//! 档位操作锁内先恢复未完成操作、再查询关联组；同一逻辑会话只保留一个有效副本，
//! 目标 UUID 在任何副本写入前持久化，任一阶段失败都不报告完整成功，恢复复用同一
//! UUID 且不产生第二个副本。
//!
//! 同步契约（design §5 / §6）：[`session_links_preview`] 只读预览双方共同参与的关联组并
//! 下发预览凭据；[`sync_sessions_for_switch`] 在执行前重新加载账号身份、成员、基线与
//! 正文并逐项核对凭据，然后按「备份 → 正文 → 数据库 → 组表」逐个阶段写入。
//!
//! 写入前置条件（design §5.4）：本次同步的备份必须成功且可核验（唯一目录、数据库
//! 一致性快照、目标正文逐文件备份 + 摘要、恢复清单）。备份不可信时**零写入**：不碰
//! 目标正文、不改数据库、不提交基线。任一阶段中断都保留未完成操作，下次切换按同一份
//! 清单补完（复用同一目标 UUID 与新基线引用），不把未完成的写入报告成 `synced`。

use rusqlite::backup::{Backup, StepResult};
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::modules::account;
use crate::modules::config::now_ms;
use crate::modules::process;
use crate::modules::session_backup::{
    self, BackupLifecycle, CleanupOutcome, CLEANUP_STATE_SAFE_TERMINATED,
    OPERATION_LIFECYCLE_VERSION,
};
use crate::modules::session_link::{
    self, full_digest_of, BaselineState, ContentSnapshot, ContentState, LinkGroup, LinkMember,
    LinkStore, MemberState, NormalizedContent, OpPhase, Operation, OperationMember, PreviewBinding,
    PreviewMemberBinding, RecoveryIssue, RecoveryReport, StoreState, SyncDecision, SyncMode,
    SyncVerdict, NORMALIZATION_VERSION, OPERATION_VERSION,
};
use crate::modules::variant::WbVariant;

// 会话业务的内部模块边界：路径（paths）、查询与数据库辅助（query）、复制（copy）、
// 同步预览与计划（sync）、备份与执行（exec）、未完成操作恢复（recovery）。
// 公共入口统一在下方 re-export，外部仍按 `session::` 使用。
mod copy;
mod discard;
mod exec;
mod paths;
mod query;
mod recovery;
mod run;
mod sync;

pub use copy::{
    copy_sessions_cross, copy_sessions_cross_opts, copy_sessions_for_switch,
    copy_sessions_for_switch_opts, CopyOptions,
};
pub use paths::{workbuddy_db_path, LinkNamespace, SessionPaths};
use query::*;
pub use query::{
    current_user_uid, list_sessions_for_account, list_sessions_for_user, session_copy_supported_at,
};
pub(crate) use query::{
    current_user_uid_at, find_project_jsonl, list_sessions_for_user_at, open_db, table_exists,
};
// 会话行存活索引与「组内是否还有存活成员」判定：`session_groups` 的列表过滤与批量清理复用。
pub use discard::{
    discard_unfinished_sync, pending_session_operations, pending_session_operations_with,
    DiscardedOperation, PendingWrite,
};
pub(crate) use query::LiveSessionRows;
pub(crate) use recovery::recover_pending_session_operations;
pub use run::sync_sessions_cross;
pub(crate) use run::sync_sessions_for_switch;
pub(crate) use sync::{group_any_alive, SyncSelection, REASON_PREVIEW_STALE};
pub use sync::{parse_sync_selections, session_links_preview, session_links_preview_cross};

/// 档位不支持会话复制时的统一错误文案。
pub const SESSION_COPY_UNSUPPORTED: &str = "该档位暂不支持会话复制";

/// 目标账号是当前登录账号、且 WorkBuddy 正在运行时的统一错误文案。
///
/// 写"非当前登录账号"的副本实测免关安全（`research/verification-app-running-write.md`
/// 探针 5）：客户端不使用该账号的数据、切号后自然可见；写"当前登录账号"仍必须等
/// App 停止写入——运行实例看不到写入，继续对话会让会话状态分叉。
pub const SESSION_COPY_APP_RUNNING: &str =
    "目标账号正在 WorkBuddy 中使用，已阻止修改会话数据；请先退出 WorkBuddy 后重试";

/// 目标档客户端运行时的写入门禁：仅当目标账号是该档位**当前登录账号**时拦截。
///
/// 无法读取登录态时保守拦截（宁可要求退出，不做无法判定的写入）。
fn app_running_blocks_target_write(
    paths: &SessionPaths,
    variant: WbVariant,
    target_uid: &str,
    is_app_running: &impl Fn(WbVariant) -> bool,
) -> bool {
    if !is_app_running(variant) {
        return false;
    }
    match current_user_uid_at(&paths.auth_file) {
        Some(current) => current == target_uid,
        None => true,
    }
}

/// 操作日志无法解析时的原因前缀（恢复与复制共用，避免漏报后写出第二个副本）。
const UNPARSEABLE_OPERATION_REASON: &str = "操作记录无法解析";

// ---------------------------------------------------------------------------
// 会话状态的共享口径（复制与同步共用，禁止各自维护一份）
// ---------------------------------------------------------------------------

/// `sessions.status` 的归档取值。
const SESSION_STATUS_ARCHIVED: &str = "archived";
/// 允许被传导为归档的**目标终态**白名单。
///
/// `completed` / `error` / `terminated` 是明确的终态；`active` 也在内是**有意**的：
/// 跨账号关联会话在目标侧常因历史中断停在 `active`（进程早已不在），来源侧归档后
/// 这类会话会**永远**同步不过去。目标侧分不出「真的在用」与「中断残留」，而归档
/// 本身可逆且用户可见，因此选择跟随来源的归档意图，而不是让副本永久失同步。
///
/// `working`（生成中）必须排除：那是正在产出的会话，收进归档会打断使用。精确匹配
/// 而非「非 working 即允许」——NULL、未知值与缺列都不授予归档权限（真实库存在
/// `Pending` 这类合法但语义未定的默认值）。
const ARCHIVE_TARGET_TERMINAL_STATUSES: [&str; 4] = ["completed", "error", "terminated", "active"];
/// 会话「已经结束」的 `sessions.status` 取值集合：终态（会话的最终结果）与归档态
/// （用户显式做过的整理动作）。
///
/// 两处共用同一份定义，避免口径分叉：
/// - 复制：副本行落在**集合内**就原样保留，集合外（`working` / `active` / `Pending` 等
///   语义未定值 / 未来新增状态）一律归一为 `completed`——见 `copy/db.rs::normalize_copy_row_status`；
/// - 同步：只有来源落在**集合内**才归一正文末条 `incomplete`——见
///   `sync/wait.rs::source_settled_for_normalize`。来源仍在运行时 `incomplete` 是真实状态。
///
/// `archived` 不在 [`ARCHIVE_TARGET_TERMINAL_STATUSES`] 里是**有意**的：那个常量描述的是
/// 归档动作的**目标状态**（能把副本推到哪些状态），不是「所有已经结束的状态」。两份
/// 语义不同，不要合并成一个常量。
const SETTLED_ROW_STATUSES: [&str; 4] = ["completed", "error", "terminated", "archived"];

/// 操作日志的 kind 取值。
const OPERATION_KIND_COPY: &str = "copy";
const OPERATION_KIND_SYNC: &str = "sync";

/// 「只前进不回退」的活跃时间：取两个候选里的较大值，任一缺失时取另一个。
///
/// 复制（`copy/db.rs`）与同步（`exec/write.rs`）共用这一份口径：客户端的
/// `sessions.last_activity_at` 驱动侧栏「活跃时间」，只能往前走——回退会让用户以为
/// 最近的对话没发生过。`None` 表示「没有这个候选」，不参与比较（不是 0）。
///
/// 本函数只做数值比较，**不判断候选是否可用**：0/负数在解析层
/// （`record_timestamp_of` 只认正数）与写库层（`clamp_incoming_activity`）已被过滤，
/// 调用方传进来的应当已是可用时间。
fn advance_activity_at(current: Option<i64>, candidate: Option<i64>) -> Option<i64> {
    match (current, candidate) {
        (Some(current), Some(candidate)) => Some(current.max(candidate)),
        (Some(current), None) => Some(current),
        (None, candidate) => candidate,
    }
}

#[cfg(test)]
mod tests;
