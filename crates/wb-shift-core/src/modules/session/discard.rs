//! 手动结束「卡住」的同步写入（设置页「未完成的写入」）。
//!
//! 自动恢复（[`super::recovery`]）会在每次切号前跑一遍：能补完的补完，能安全回滚的回滚。
//! 剩下少数现场它必须拒绝——目标内容被其它程序改写、备份清单损坏、目标行归属异常——
//! 这些记录会**一直挡住**同一条会话之后的同步，而用户在此之前没有任何办法。
//!
//! 本模块只做一件事：把那条记录置为放弃。**不碰正文、不碰目标会话行**，因此它不是
//! 「回滚」也不是「修复」；是否保留备份由现场状态决定（见 [`discard_unfinished_sync`]）。
//!
//! 不提供「复制」的中断记录：复制牵涉新建的会话行与映射，结束会留下孤儿行，那是另一个故障。

use super::exec::*;
use super::recovery::{abandon_operation_with, cleanup_finished_operation};
use super::*;

/// 用户手动结束一次「卡住」的同步写入的结果。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscardedOperation {
    pub operation_id: String,
    /// 是否保留了备份材料。`true` 时维护入口会继续把这条备份列为「待人工确认」。
    pub backup_kept: bool,
    /// 结束后的状态说明，直接展示给用户。
    pub note: String,
}

/// 用户手动结束一次未完成的同步写入：**只改操作记录**，不碰正文、不碰目标会话行。
///
/// 为什么不提供「复制」：复制的中断记录牵涉新建的会话行与映射，手动结束会留下孤儿行
/// （库里有记录、本地无正文），那是另一个故障。同步的中间态只影响这一条会话的正文。
///
/// 安全边界按**现场状态**分档，而不是让用户自己判断：
/// - 正文仍是写入前的样子（`Before`）且未推进到写库：这次写入什么都没留下，结束得很干净，
///   备份随之失去价值，交回维护入口回收；
/// - 正文已不存在（`Gone`）：没有可回滚的东西，但备份是覆盖前内容的唯一副本，**保留**；
/// - 正文与「本次写入」及「覆盖前」都不一致（`Unknown`，多因客户端在切号过程中又写了内容）：
///   这是真正会永久挡住同步的情形，允许结束，但**保留备份**，提示可手动还原；
/// - 正文正好是本次写入的完整内容（`Ours`）：只差最后一步记录，下次切号会自动补完，
///   不该由用户放弃，直接拒绝并说明。
pub fn discard_unfinished_sync(
    paths: &SessionPaths,
    operation_id: &str,
) -> Result<DiscardedOperation, String> {
    let scan = session_link::scan_operations(paths);
    if !scan.complete {
        return Err(format!(
            "{}{}",
            session_link::OP_SCAN_PROBLEM_PREFIX,
            scan.problems.join("；")
        ));
    }
    let mut operation = scan
        .operations
        .into_iter()
        .find(|operation| operation.operation_id == operation_id)
        .ok_or_else(|| "找不到这条操作记录，可能已被清理".to_string())?;
    if !operation.phase.is_unfinished() {
        return Err("这次写入已经结束，不需要再处理".to_string());
    }
    if operation.kind != OPERATION_KIND_SYNC {
        return Err("只支持结束未完成的同步；复制的中断记录请按提示恢复，不要手动结束".to_string());
    }

    let body = sync_body_state_of(paths, &operation);
    let id = operation.operation_id.clone();
    match (&body, operation.phase) {
        // 正文没被写过：结束即干净终止，备份没有保留价值。
        //
        // `Ours` 出现在 `Prepared` 是正常现象：状态同步（`StatusOnly`）根本不写正文，
        // 正文自然等于「本次应有的内容」；此时它同样什么都没改。
        (SyncBodyState::PreSync | SyncBodyState::Ours, phase) if phase < OpPhase::BodyWritten => {
            abandon_operation_with(
                paths,
                &mut operation,
                "用户手动结束了这次未完成的同步：目标内容未被改写",
            );
            cleanup_finished_operation(paths, operation.variant, &operation);
            Ok(DiscardedOperation {
                operation_id: id,
                backup_kept: false,
                note: "已结束。目标会话的内容没有被这次同步改动过。".to_string(),
            })
        }
        // 正文本就是本次写入的内容、且已推进到写正文：只差最后一步记录，
        // 自动恢复能补完，不该由用户放弃。
        (SyncBodyState::Ours, _) => Err(
            "目标正文已完整写入，只差最后的记录；下次切号会自动补完，不需要手动结束".to_string(),
        ),
        // 正文已不存在，或已被其它程序改写：允许结束，但备份是唯一可回滚的副本。
        (SyncBodyState::Gone, _) | (SyncBodyState::Unknown(_), _) | (SyncBodyState::PreSync, _) => {
            operation.phase = OpPhase::Abandoned;
            // 不写 safeTerminated：维护入口据此**不回收**这份备份。
            operation.cleanup_state = None;
            operation.last_error = Some(match &body {
                SyncBodyState::Gone => {
                    "用户手动结束了这次未完成的同步：目标内容已不存在，备份已保留".to_string()
                }
                _ => "用户手动结束了这次未完成的同步：目标内容可能被改写过，备份已保留".to_string(),
            });
            operation.updated_at = now_ms();
            session_link::save_operation(paths, &operation)?;
            Ok(DiscardedOperation {
                operation_id: id,
                backup_kept: true,
                note: "已结束，同步不会再被这条记录挡住。目标内容可能不是同步后的状态，\
备份已保留在工具数据目录的 backups 下，需要时可按备份手动还原。"
                    .to_string(),
            })
        }
    }
}

/// 目标正文相对本次同步的现场状态；备份不可用时一律按「不可验证」处理（保守）。
fn sync_body_state_of(paths: &SessionPaths, operation: &Operation) -> SyncBodyState {
    let Some(backup_dir) = operation
        .backup
        .clone()
        .map(PathBuf::from)
        .and_then(|manifest_file| manifest_file.parent().map(Path::to_path_buf))
    else {
        return SyncBodyState::Unknown("同步备份位置缺失".to_string());
    };
    let Some(manifest) = load_sync_manifest(&backup_dir) else {
        return SyncBodyState::Unknown("同步备份清单缺失或损坏".to_string());
    };
    if let Err(reason) = verify_sync_backup(&backup_dir, &manifest) {
        return SyncBodyState::Unknown(reason);
    }
    let target_body_path = match validated_target_body_path(paths, &manifest) {
        Ok(path) => path,
        Err(reason) => return SyncBodyState::Unknown(reason),
    };
    classify_sync_body(
        &target_body_path,
        &operation.target.session_id,
        &manifest.target.body_raw_digest,
        &operation.expected_content_digest,
    )
}

/// 一条未完成的会话写入（设置页「未完成的写入」列表的数据源）。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingWrite {
    pub operation_id: String,
    pub kind: String,
    pub target_session_id: String,
    /// 目标会话标题；会话行已不存在时为 `None`。
    pub title: Option<String>,
    /// 操作所属档位（`cn` / `ai` / …）。
    pub variant: WbVariant,
    pub updated_at: i64,
    pub last_error: Option<String>,
    /// 是否可由 [`discard_unfinished_sync`] 结束：目前只支持同步；复制的中断记录牵涉新建的
    /// 会话行与映射，结束会留下孤儿行，因此不提供。
    pub discardable: bool,
}

/// 列出全部档位未完成的会话写入（只读，按更新时间倒序）。
///
/// 不按档位过滤：操作目录是四个入口共用的（`~/.workbuddy-shift/session-links`），
/// 而「这条会话被挡住了」在界面上是一件事，不需要用户先选档位再找。标题按操作自身的
/// 档位解析（`sessions` 表在每个档位的数据根里）。
pub fn pending_session_operations(paths: &SessionPaths) -> Result<Vec<PendingWrite>, String> {
    pending_session_operations_with(paths, SessionPaths::for_variant)
}

/// [`pending_session_operations`] 的可注入版本：`resolve` 负责把操作档位映射到数据根，
/// 单测注入临时目录（绝不触碰真实的 `~/.workbuddy` / `~/.workbuddy-shift`）。
pub fn pending_session_operations_with<F>(
    paths: &SessionPaths,
    resolve: F,
) -> Result<Vec<PendingWrite>, String>
where
    F: Fn(WbVariant) -> SessionPaths,
{
    let scan = session_link::scan_operations(paths);
    if !scan.complete {
        return Err(format!(
            "{}{}",
            session_link::OP_SCAN_PROBLEM_PREFIX,
            scan.problems.join("；")
        ));
    }
    let mut items: Vec<PendingWrite> = scan
        .operations
        .into_iter()
        .filter(|operation| operation.phase.is_unfinished())
        .map(|operation| {
            let variant_paths = resolve(operation.variant);
            let title = super::sync::session_row_info(&variant_paths, &operation.target.session_id)
                .map(|(title, custom)| {
                    if custom.trim().is_empty() {
                        title
                    } else {
                        custom
                    }
                })
                .filter(|title| !title.trim().is_empty());
            PendingWrite {
                discardable: operation.kind == OPERATION_KIND_SYNC,
                operation_id: operation.operation_id,
                kind: operation.kind,
                target_session_id: operation.target.session_id,
                title,
                variant: operation.variant,
                updated_at: operation.updated_at,
                last_error: operation.last_error,
            }
        })
        .collect();
    items.sort_by_key(|item| std::cmp::Reverse(item.updated_at));
    Ok(items)
}
