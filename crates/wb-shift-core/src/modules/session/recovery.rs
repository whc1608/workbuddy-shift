//! 未完成会话操作的恢复（design §4.2 / §4.5）。
//!
//! 恢复先检查实际状态再决定下一步，不盲目重放：正文、数据库行、基线分别核验，
//! 已完成的阶段不重放，未知内容不覆盖；无法安全推进时只上报 `needsRecovery`。

use super::copy::*;
use super::exec::*;
use super::*;

/// 恢复某档位全部未完成操作（需已持有档位操作锁）。
///
/// 恢复先检查实际状态再决定下一步，不盲目重放；中间产物被改动或丢失时只上报
/// needsRecovery，不覆盖未知内容。
///
/// 跨档操作的源档数据根按 [`SessionPaths::for_variant`] 解析；单测与需要注入
/// 临时目录的宿主用 [`recover_pending_session_operations_at_with`]。
pub fn recover_pending_session_operations_at(
    paths: &SessionPaths,
    variant: WbVariant,
) -> RecoveryReport {
    recover_pending_session_operations_at_with(paths, variant, SessionPaths::for_variant)
}

/// [`recover_pending_session_operations_at`] 的可注入版本：`resolve_source_paths`
/// 用于解析跨档操作（`sourceVariant` 与操作档不同）的源档数据根。
pub fn recover_pending_session_operations_at_with<F>(
    paths: &SessionPaths,
    variant: WbVariant,
    resolve_source_paths: F,
) -> RecoveryReport
where
    F: Fn(WbVariant) -> SessionPaths,
{
    let mut report = RecoveryReport::default();
    let scan = session_link::scan_operations(paths);
    // 扫描不完整（目录不可读/枚举失败）不能按「没有未完成操作」继续写：与解析失败
    // 同口径阻断，避免绕过 pending 去重后写出第二个副本。
    if !scan.complete {
        report.needs_recovery.push(RecoveryIssue {
            operation_id: "operation-scan".to_string(),
            reason: format!(
                "{UNPARSEABLE_OPERATION_REASON}（操作记录无法读取），已停止恢复以免产生重复复制"
            ),
            retryable: false,
        });
    }
    for problem in scan.problems {
        report.needs_recovery.push(RecoveryIssue {
            operation_id: problem.clone(),
            reason: format!(
                "{UNPARSEABLE_OPERATION_REASON}（{problem}），已停止恢复以免产生重复复制"
            ),
            retryable: false,
        });
    }
    for operation in scan
        .operations
        .into_iter()
        .filter(|operation| operation.variant == variant && operation.phase.is_unfinished())
    {
        // 跨档操作（源档与操作档不同）按源档解析读侧数据根；同档与旧记录视为同档。
        let source_paths = operation
            .source_variant
            .filter(|source_variant| *source_variant != operation.variant)
            .map(&resolve_source_paths);
        match recover_operation(paths, variant, operation, source_paths.as_ref()) {
            RecoverOutcome::Recovered(id) => report.recovered.push(id),
            RecoverOutcome::Abandoned(id) => report.abandoned.push(id),
            RecoverOutcome::NeedsRecovery {
                id,
                reason,
                retryable,
            } => {
                report.needs_recovery.push(RecoveryIssue {
                    operation_id: id,
                    reason,
                    retryable,
                });
            }
        }
    }
    // 恢复之后补清理：回收可安全回收的临时备份残留，保护其余并上报（design §6）。
    report.temporary_files = session_backup::maintain(paths, variant);
    report
}

/// 生产入口：自行获取档位操作锁后恢复（被占用时返回明确错误，不排队）。
pub fn recover_pending_session_operations(variant: WbVariant) -> Result<RecoveryReport, String> {
    let paths = SessionPaths::for_variant(variant);
    let _lock = session_link::try_acquire_variant_ops_lock(&paths, variant)?;
    Ok(recover_pending_session_operations_at_with(
        &paths,
        variant,
        SessionPaths::for_variant,
    ))
}

pub(super) enum RecoverOutcome {
    Recovered(String),
    Abandoned(String),
    NeedsRecovery {
        id: String,
        reason: String,
        /// 既有宿主契约：true 允许账号切换和 App 启动，不能仅表示故障可重试。
        /// 同步尚未恢复一致时必须为 false，即使稍后重试可能成功。
        retryable: bool,
    },
}

/// 目标正文现状判定（恢复的第一步）。
pub(super) enum BodyCheck {
    /// 正文与操作记录一致。
    Verified(NormalizedContent),
    /// 尚未写入（操作停在 Prepared 阶段）。
    Absent,
    /// 中间产物被改动/丢失：停止恢复。
    NeedsRecovery(String),
}

pub(super) fn check_target_body(paths: &SessionPaths, operation: &Operation) -> BodyCheck {
    match find_project_jsonl(paths, &operation.target.session_id)
        .as_deref()
        .map(|path| session_link::read_content_snapshot(path, &operation.target.session_id))
    {
        Some(ContentState::Ready(content)) => {
            if content.normalized.total_digest == operation.expected_content_digest {
                BodyCheck::Verified(content.normalized)
            } else {
                BodyCheck::NeedsRecovery(
                    "目标内容与操作记录不一致（可能被其它程序改动），已停止恢复".to_string(),
                )
            }
        }
        Some(ContentState::Unavailable(reason)) => {
            BodyCheck::NeedsRecovery(format!("目标内容不可验证（{reason}），已停止恢复"))
        }
        Some(ContentState::Missing) | None => {
            if operation.phase >= OpPhase::BodyWritten {
                BodyCheck::NeedsRecovery("目标内容丢失，已停止恢复".to_string())
            } else {
                BodyCheck::Absent
            }
        }
    }
}

/// 恢复单个操作：按「持久化阶段 + 实际状态」逐阶段判断，已经越过的阶段不重放
/// （不重复登记映射、不重写关联存储与基线）；校验不因跳过重放而放松。
pub(super) fn recover_operation(
    paths: &SessionPaths,
    variant: WbVariant,
    operation: Operation,
    source_paths: Option<&SessionPaths>,
) -> RecoverOutcome {
    if operation.kind == OPERATION_KIND_SYNC {
        return recover_sync_operation(paths, operation);
    }
    recover_copy_operation(paths, variant, operation, source_paths)
}

/// 恢复一次同步（design §5.6）：先校验现场，再按清单补完，绝不覆盖未知内容。
///
/// 顺序：备份必须完好 → 目标正文只能是「本次写入的」或「覆盖前的」→ 目标行归属与
/// 更新时间必须可安全识别 → 复用同一段阶段代码补完。任一项不满足即停止并上报
/// needsRecovery（阻断启动，宿主必须等待恢复一致后才能启动 App）。
pub(super) fn recover_sync_operation(
    paths: &SessionPaths,
    mut operation: Operation,
) -> RecoverOutcome {
    let operation_id = operation.operation_id.clone();
    let needs = |reason: String, retryable: bool| RecoverOutcome::NeedsRecovery {
        id: operation_id.clone(),
        reason,
        retryable,
    };

    // 1) 备份必须完好：没有可验证的备份就没有恢复依据，也不得盲目重放。
    // 操作日志里记录的是本次的备份清单路径，备份目录是它的父目录。
    let Some(backup_dir) = operation
        .backup
        .clone()
        .map(PathBuf::from)
        .and_then(|manifest_file| manifest_file.parent().map(Path::to_path_buf))
    else {
        return needs(
            "同步备份位置缺失，已停止恢复，请手动处理".to_string(),
            false,
        );
    };
    let Some(manifest) = load_sync_manifest(&backup_dir) else {
        return needs(
            "同步备份清单缺失或损坏，已停止恢复，请手动处理".to_string(),
            false,
        );
    };
    if let Err(reason) = verify_sync_backup(&backup_dir, &manifest) {
        return needs(reason, false);
    }
    let target_body_path = match validated_target_body_path(paths, &manifest) {
        Ok(path) => path,
        Err(reason) => return needs(reason, false),
    };

    // 2) 目标正文：只接受「本次写入的内容」或「覆盖前的内容」。
    let body = classify_sync_body(
        &target_body_path,
        &operation.target.session_id,
        &manifest.target.body_raw_digest,
        &operation.expected_content_digest,
    );
    match &body {
        // 后续无关修改（用户/官方 App 追加过内容）或内容不可验证：停止恢复，不覆盖。
        SyncBodyState::Unknown(reason) => return needs(reason.clone(), false),
        // 状态同步不写正文，也永远不会推进到 BodyWritten：正文丢失后若按正文口径
        // 拦截，每次启动都会重跑并失败，等于永久阻断启动。已提交的状态予以保留，
        // 只放弃本次归档意图（不写库、不补正文、不重建文件）。
        // 必须排在「已写正文却丢失」的通用拦截之前——DbWritten 晚于 BodyWritten，
        // 否则归档已提交的现场会先被通用分支截走，永远走不到这里。
        SyncBodyState::Gone if manifest.mode == SyncMode::StatusOnly => {
            abandon_operation_with(
                paths,
                &mut operation,
                "目标内容已不存在，本次仅同步归档已放弃，已提交的状态保持不变",
            );
            return RecoverOutcome::Abandoned(operation_id);
        }
        SyncBodyState::Gone if operation.phase >= OpPhase::BodyWritten => {
            return needs("目标内容丢失，已停止恢复".to_string(), false);
        }
        _ => {}
    }

    // 3) 目标行：归属与更新时间必须可安全识别。
    let row = match session_row_snapshot(paths, &operation.target.session_id) {
        Ok(row) => row,
        Err(reason) => return needs(reason, false),
    };
    let row_absent = match &row {
        None => true,
        // 已删除的会话（软删除）同样没有可更新的行：会话在 App 里已经不存在。
        Some(row) => row.deleted_at.is_some(),
    };
    if row_absent {
        // 没有可更新的行就无法补完。若残留的是本次写入的正文，按清单回滚成覆盖前
        // 内容，不留无行的半成品；这不算成功，记为放弃（无需人工处理，不阻断启动）。
        // 仅同步归档不写正文，也就没有可回滚的正文：写回即等于补写业务文件。
        if matches!(body, SyncBodyState::Ours) && manifest.mode != SyncMode::StatusOnly {
            if let Err(error) = restore_sync_backup_body(paths, &backup_dir, &manifest) {
                return needs(
                    format!("目标会话记录已不存在且内容回滚失败（{error}），请手动处理"),
                    false,
                );
            }
        }
        abandon_operation_with(
            paths,
            &mut operation,
            "目标会话记录已不存在（或已删除），本次同步已从备份回滚，未保存会话内容",
        );
        return RecoverOutcome::Abandoned(operation_id);
    }
    let Some(row) = row else {
        // row_absent 已经把 None 分支处理掉，这里只是形式上的兜底。
        return needs("目标会话记录无法读取，已停止恢复".to_string(), false);
    };
    if row.user_id != operation.target.uid {
        return needs("目标会话记录归属异常，已停止恢复".to_string(), false);
    }
    let before_row = manifest.db.target_row.as_ref();
    let before_updated_at = before_row.and_then(|row| row.updated_at);
    let applied = row.updated_at == Some(manifest.db.new_updated_at);
    // 状态相等只在「状态就是全部动作」时才要求（仅同步归档）。正文模式的归档是
    // 副作用：目标被打开变成 active 属于与本次正文同步无关的変化，不能因此把一次
    // 正常的正文恢复升级成人工处理——由下面的状态写入决策决定是否跳过归档。
    let status_matters = manifest.mode == SyncMode::StatusOnly;
    let untouched = before_row.is_some_and(|before| {
        before.updated_at == row.updated_at && (!status_matters || before.status == row.status)
    });
    // 状态到底有没有写上去，不能只看时间戳：用户取消归档或改为活跃态时时间戳可能
    // 仍是本次值。但正文模式里归档只是副作用，允许被合法跳过（见
    // planned_status_change：目标变为活跃态时跳过），跳过与否每次都由现场状态重新
    // 推导、结果一致，因此只要本次时间戳已落库，这一步就算完成。
    // 仅同步归档里状态是全部动作，必须额外确认状态也等于本次新值。
    let db_applied = match (
        manifest.mode == SyncMode::StatusOnly,
        manifest.db.new_status.as_deref(),
    ) {
        (true, Some(expected)) => applied && row.status.as_deref() == Some(expected),
        _ => applied,
    };
    if !(db_applied || untouched) && before_updated_at.is_some() {
        // 既不是本次写入的值、也不是覆盖前的值：被其它程序改动过，不覆盖。
        return needs(
            "目标会话记录的状态或更新时间与本次保存及覆盖前值都不一致（可能被其它程序改动），已停止恢复"
                .to_string(),
            false,
        );
    }

    // 4) 复用同一段阶段代码补完（阶段只前进，已越过的阶段只核验不重放）。
    match run_sync_phases(paths, &mut operation, &backup_dir, &manifest, body) {
        Ok(()) => RecoverOutcome::Recovered(operation_id),
        Err(error) => {
            fail_operation(paths, &mut operation, &error);
            // Retry may succeed later, but the unfinished sync must block startup now.
            needs(error, false)
        }
    }
}

/// 恢复一次复制（第一步的原有逻辑，保持不变）。
pub(super) fn recover_copy_operation(
    paths: &SessionPaths,
    variant: WbVariant,
    mut operation: Operation,
    source_paths: Option<&SessionPaths>,
) -> RecoverOutcome {
    let operation_id = operation.operation_id.clone();
    let needs = |reason: String, retryable: bool| RecoverOutcome::NeedsRecovery {
        id: operation_id.clone(),
        reason,
        retryable,
    };
    // 读侧数据根：跨档操作由调用方解析出的源档；同档与旧记录用目标档。
    let read_paths = source_paths.unwrap_or(paths);

    // 1) 目标正文：已写成则直接复用；未写成则用当前源内容补写；被改动则停止。
    //    第二个返回值是「本次正文最新记录时间戳」：补写分支拿得到内存里的待写入文本；
    //    已写成分支只有摘要、没有正文，因此给 None——**不**回读目标文件（那时客户端
    //    可能已经在写，回读既会 TOCTOU，也与「备份不可信时零写入」的口径冲突）。
    //    拿不到时复制侧沿用源行的活跃时间，其余行为不变。
    let (normalized, body_latest_at) = match check_target_body(paths, &operation) {
        BodyCheck::Verified(normalized) => (normalized, None),
        BodyCheck::NeedsRecovery(reason) => return needs(reason, false),
        BodyCheck::Absent => {
            let Some(source_path) = find_project_jsonl(read_paths, &operation.source.session_id)
            else {
                abandon_operation(paths, &mut operation);
                return RecoverOutcome::Abandoned(operation_id);
            };
            let source = match session_link::read_content_snapshot(
                &source_path,
                &operation.source.session_id,
            ) {
                ContentState::Ready(snapshot) => snapshot,
                _ => {
                    abandon_operation(paths, &mut operation);
                    return RecoverOutcome::Abandoned(operation_id);
                }
            };
            // 源内容在本机发生了变化也按当前内容继续（副本是快照复制，不是同步）：
            // 待写入文本与它的摘要都在下面一次算出，不再沿用操作记录里的旧摘要。
            let body = match copy_body(
                &source,
                &operation.source.session_id,
                &operation.target.session_id,
            ) {
                Ok(body) => body,
                Err(error) => {
                    fail_operation(paths, &mut operation, &error);
                    return needs(error, false);
                }
            };
            // 期望摘要先落盘再写正文（理由同 `finish_copy_from_body`）。
            operation.expected_content_digest = body.normalized.total_digest.clone();
            operation.expected_record_count = body.normalized.record_count;
            if let Err(error) = session_link::save_operation(paths, &operation) {
                return needs(error, true);
            }
            if let Err(error) = write_copy_body(
                &body,
                &source_path,
                read_paths,
                paths,
                &operation.target.session_id,
            ) {
                fail_operation(paths, &mut operation, &error);
                return needs(error, true);
            }
            if let Err(error) = advance_operation(paths, &mut operation, OpPhase::BodyWritten) {
                return needs(error, true);
            }
            let body_latest_at = session_link::latest_record_timestamp(&body.text);
            (body.normalized, body_latest_at)
        }
    };

    // 2) 数据库行：缺失则补写，归属异常则停止。
    match session_row_owner(paths, &operation.target.session_id) {
        Some(owner) if owner == operation.target.uid => {}
        Some(_) => {
            return needs("目标会话记录归属异常，已停止恢复".to_string(), false);
        }
        None => {
            if operation.phase >= OpPhase::DbWritten {
                return needs("目标会话记录丢失，已停止恢复".to_string(), false);
            }
            match insert_session_copy(
                read_paths,
                paths,
                &operation.target.session_id,
                &operation.source.session_id,
                &operation.source.uid,
                &operation.target.uid,
                operation.aux_skipped,
                // R6-08：恢复重放沿用默认口径（不搬来源历史消耗）。`Operation` 里没有这个
                // 字段（不扩持久化契约）：生产路径从不打开 `copy_usage`，默认值就是新语义。
                false,
                !operation.aux_skipped,
                body_latest_at,
            ) {
                Ok(DbCopyOutcome::Inserted { .. }) => {}
                Ok(outcome) => {
                    let error = format!("会话记录保存失败（{outcome:?}），保留操作待重试");
                    fail_operation(paths, &mut operation, &error);
                    return needs(error, false);
                }
                Err(error) => {
                    fail_operation(paths, &mut operation, &error);
                    return needs(error, true);
                }
            }
            if let Err(error) =
                verify_session_row(paths, &operation.target.session_id, &operation.target.uid)
            {
                return needs(error, false);
            }
        }
    }
    if let Err(error) = advance_operation(paths, &mut operation, OpPhase::DbWritten) {
        return needs(error, true);
    }

    // 3) 云端登记已交接给客户端（不预写 edge_sync_mapping）：这里只推进阶段，
    // 不写入、不核验映射行——该行由 edge-sync 扩展在客户端下次启动时写入，
    // 工具恢复时不能要求它已存在，否则会误报 needsRecovery。
    if operation.phase < OpPhase::MappingWritten {
        if let Err(error) = advance_operation(paths, &mut operation, OpPhase::MappingWritten) {
            return needs(error, true);
        }
    }

    // 4) 关联与基线：已提交过就不再 commit_links，避免重复写关联存储与基线；
    // 同样先核验组与成员仍在，不能把「跳过重放」当成「产物一定还在」。
    if operation.phase < OpPhase::LinksCommitted {
        match commit_links(paths, variant, &operation, &normalized) {
            Ok(group_id) => {
                operation.group_id = group_id;
            }
            Err(error) => {
                fail_operation(paths, &mut operation, &error);
                return needs(error, true);
            }
        }
        if let Err(error) = advance_operation(paths, &mut operation, OpPhase::LinksCommitted) {
            return needs(error, true);
        }
    } else if let Err(error) = committed_links_present(paths, &operation) {
        return needs(error, false);
    }
    if let Err(error) = advance_operation(paths, &mut operation, OpPhase::Completed) {
        return needs(error, true);
    }
    // R6-07：恢复补完的复制同样要写归属交接。与 `finish_copy_from_body` 里的埋点互斥
    // （本函数只处理 `phase < Completed` 的操作），因此不会双写；即使重复也是替换语义。
    record_copy_origin_quiet(
        paths,
        variant,
        &operation,
        body_latest_at.map_or_else(now_ms, |latest| latest.max(now_ms())),
    );
    cleanup_finished_operation(paths, variant, &operation);
    RecoverOutcome::Recovered(operation_id)
}

pub(super) fn abandon_operation(paths: &SessionPaths, operation: &mut Operation) {
    abandon_operation_with(
        paths,
        operation,
        "源会话已不可用，且没有复制出任何会话，已放弃该操作",
    );
}

/// 放弃一个未写入任何会话内容的操作（阶段置 Abandoned，不算成功）。
///
/// `cleanupState = safeTerminated` 是「已验证安全终止」的持久标记：只有带标记的
/// Abandoned 才允许维护入口回收对应临时备份（design §3）。
pub(super) fn abandon_operation_with(
    paths: &SessionPaths,
    operation: &mut Operation,
    reason: &str,
) {
    operation.phase = OpPhase::Abandoned;
    operation.cleanup_state = Some(CLEANUP_STATE_SAFE_TERMINATED.to_string());
    operation.last_error = Some(reason.to_string());
    operation.updated_at = now_ms();
    let _ = session_link::save_operation(paths, operation);
}

/// 恢复/放弃完成后立即回收该操作的临时备份（待清理推进失败时留给下次维护补转）。
pub(super) fn cleanup_finished_operation(
    paths: &SessionPaths,
    variant: WbVariant,
    operation: &Operation,
) {
    let Ok(Some(mut record)) =
        session_backup::load_lifecycle(paths, variant, &operation.operation_id)
    else {
        // 没有维护记录（旧操作/记录损坏）：维护扫描会按自身口径上报，这里不猜测。
        return;
    };
    if session_backup::mark_cleanup_pending(paths, &mut record, None).is_ok() {
        let _ = session_backup::cleanup_after_success(paths, variant, &record);
    }
}
