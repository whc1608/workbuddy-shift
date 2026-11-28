//! 会话复制关联组、配对基线与操作日志（design §2 / §3.1 / §4 的存储与内容层）。
//!
//! 落点（全部位于工具存储根 `~/.workbuddy-shift` 下，路径由
//! [`crate::modules::session::SessionPaths`] 注入）：
//!   - `session_links.json`                  关联组主表（version + revision + groups）
//!   - `session-links/baselines/{ref}.json`  配对基线（有序行摘要 + 总摘要 + 记录数）
//!   - `session-links/operations/{id}.json`  复制操作日志（阶段 + 预分配目标 UUID）
//!   - `session-links/previews/{id}.json`    预览凭据（服务端保存的版本绑定，一次性的）
//!   - `locks/session-ops-{variant}.lock`    档位操作锁（跨进程，覆盖整个会话操作）
//!   - `locks/session-links.lock`            关联存储短时全局锁（读改写在锁内进行）
//!
//! 锁顺序固定为「档位操作锁 → 关联存储锁」，任何调用方不得反向获取，避免死锁。
//! `atomic_write` 只防半写，不能代替互斥；关联的读改写必须在存储锁内完成。
//!
//! 读取语义区分 Missing / Ready / Unavailable：只有「首次使用且没有任何未完成痕迹」
//! 才按 Missing 初始化；损坏、权限失败、未知版本、主文件缺失但残留未完成操作或基线
//! 一律 Unavailable——保留现场并禁止写入，不得降级成空表后保存。预览凭据不算痕迹
//! （一次性、可随时重发），主文件缺失时不得据此拒绝初始化。
//!
//! 判定（design §3.2）由 [`decide_sync`] 承担：给定双方内容状态与配对共同基线即可确定
//! 结果，不做任何 IO；差集多重集只用于解释记录数，不参与自动勾选。
//! 目标内容被来源内容完整包含时（目标为来源的严格有序前缀）直接判快进，不依赖配对
//! 基线——对齐 git fast-forward 的 ancestor 语义，追加同步零覆盖。

mod content;
mod content_repair;
mod link_store;
mod operation_log;
mod preview;
mod sync_decision;

// 本模块的 re-export 只服务本 crate（`session` / `session_backup` 等模块），没有跨 crate 使用者，
// 因此内部项一律 `pub(crate)`；只有其它模块的单测仍按旧路径 `session_link::X` 取的少数项保留 pub
// （见下方单独分组，生产路径不直接使用），本模块单测则直接 import 对应子模块。
// 预览摘要缓存（R5-16）：只读判定可命中，执行路径不提供正文因此无法复用。
pub(crate) use content::{
    fingerprint_cache, stamp_content, BoundedCache, ContentStamp, FingerprintedContent,
};
// 单测专用的真实读取计数：证明预览缓存命中时不再触发正文读取（R5-16 验收口径）。
#[cfg(test)]
pub(crate) use content::real_read_count;
pub(crate) use content::{
    full_digest_of, line_digest_of, normalize_jsonl, normalize_jsonl_with_latest,
    normalize_last_incomplete, read_content_snapshot, to_hex, total_digest_of, ContentSnapshot,
    ContentState, ListFacts, NormalizedContent, NORMALIZATION_VERSION, SESSION_ID_MARKER,
};
// 正文记录的毫秒时间戳：只服务 `sessions.last_activity_at`（复制/同步写库时取正文最新值），
// 与内容判定口径无关。
pub(crate) use content::latest_record_timestamp;
// 复制路径专用的容错修复（严格校验失败的兜底，只读源内容、只产出内存副本）。
pub(crate) use content_repair::{repair_jsonl_for_copy, JsonlRepair};
pub(crate) use link_store::{
    active_member_for, add_active_member, find_group_for_identity, find_member, find_pair_base,
    inheritable_baseline, load_baseline, load_pair_baseline, load_store, member_variant,
    representative_member, save_baseline, set_member_state, set_pair_base, with_link_store_write,
    BaselineState, LinkGroup, LinkMember, LinkStore, MemberState, StoreState,
};
// 旧公开路径：其它模块的单测仍按 `session_link::X` 取这些项（生产路径不直接使用）。
pub use link_store::{
    pair_key, validate_store, BaselineRecord, PairBase, BASELINE_VERSION, LINK_STORE_VERSION,
};
pub(crate) use operation_log::{
    find_pending_operation, pending_operations, prune_operations, save_operation, scan_operations,
    try_acquire_client_ops_lock, try_acquire_variant_ops_lock, OpPhase, Operation, OperationMember,
    RecoveryIssue, RecoveryReport, TemporaryFileIssue, KEEP_COMPLETED_OPERATIONS,
    LOCK_BUSY_MESSAGE_PREFIX, LOCK_UNAVAILABLE_MESSAGE_PREFIX, OPERATION_VERSION,
    OP_SCAN_PROBLEM_PREFIX,
};
// 旧公开路径：其它模块的单测仍按 `session_link::X` 取这些项（生产路径不直接使用）。
// `FileLock` 一并导出：`auth_file` 的凭据写入锁（issue #202）要持有同一个 RAII 守卫类型，
// 锁语义（try_lock + Busy/Unavailable 分类 + Drop 释放）仍只有本模块一份实现。
pub use operation_log::{try_lock_file, FileLock, LockError};
pub(crate) use preview::{
    group_fingerprint, load_preview_token, save_preview_token, verify_preview, PreviewBinding,
    PreviewMemberBinding,
};
// 旧公开路径：其它模块的单测仍按 `session_link::X` 取该常量（生产路径不直接使用）。
pub use preview::PREVIEW_TOKEN_VERSION;
pub(crate) use sync_decision::{
    decide_sync, unverifiable_sides, SyncDecision, SyncMode, SyncVerdict,
};

#[cfg(test)]
mod tests;
