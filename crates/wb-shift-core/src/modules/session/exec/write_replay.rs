//! 同步阶段推进与中断恢复重放：正常执行与恢复共用同一段阶段化写入编排。
use crate::modules::usage_origin;

use super::*;
/// 仅同步归档的阶段推进：`Prepared → DbWritten → Completed`。
///
/// 跳过 `BodyWritten` 与 `LinksCommitted`：本次没有正文写入，也不产生新基线，
/// 不能伪造「正文已写入」或「基线已提交」的含义。
pub(in crate::modules::session) fn run_status_only_phases(
    paths: &SessionPaths,
    operation: &mut Operation,
    manifest: &SyncBackupManifest,
    body: SyncBodyState,
) -> Result<(), String> {
    // 1) 正文：只核验仍是本次读取的目标原文；任何写入（含补写与回滚）都不允许。
    match body {
        SyncBodyState::Ours | SyncBodyState::PreSync => {
            let target_body_path = validated_target_body_path(paths, manifest)?;
            match session_link::read_content_snapshot(
                &target_body_path,
                &manifest.target.session_id,
            ) {
                ContentState::Ready(content)
                    if content.normalized.total_digest == operation.expected_content_digest => {}
                ContentState::Ready(_) => {
                    return Err("目标内容与本次保存不一致，状态同步未执行".to_string())
                }
                ContentState::Missing => return Err("目标内容丢失，状态同步未执行".to_string()),
                ContentState::Unavailable(reason) => {
                    return Err(format!("目标内容无法验证（{reason}），状态同步未执行"))
                }
            }
        }
        SyncBodyState::Gone => return Err("目标内容不存在，状态同步未执行".to_string()),
        SyncBodyState::Unknown(reason) => return Err(reason),
    }
    // 2) 状态：归档是本次唯一动作，必须带可核验的覆盖前状态。
    let Some(status_change) = status_change_of(manifest)? else {
        return Err("备份清单未记录本次归档状态，状态同步未执行".to_string());
    };
    update_target_session_row(
        paths,
        &operation.target,
        manifest.db.new_updated_at,
        manifest.db.target_row.as_ref(),
        Some(&status_change),
        // 仅同步归档：正文零写入，因此**不**动活跃时间（保持目标行现值）。拿目标正文
        // 现算会把这一列改成本次操作的时间，等于给「只归档」这个动作伪造了活跃信息。
        None,
    )?;
    advance_operation(paths, operation, OpPhase::DbWritten)?;
    // 3) 不提交基线、不推进目标成员 lastSyncedAt：正文与配对基线保持原值。
    advance_operation(paths, operation, OpPhase::Completed)?;
    Ok(())
}

/// 阶段化写入：正文 → 数据库 → 组表 → completed；每步先落阶段再推进，可恢复。
///
/// 正常执行与恢复共用同一段代码：阶段只前进不回退，已越过的阶段不重放（只核验产物），
/// 因此恢复不会产生第二份写入。仅同步归档走 [`run_status_only_phases`]，不进正文分支。
pub(in crate::modules::session) fn run_sync_phases(
    paths: &SessionPaths,
    operation: &mut Operation,
    backup_dir: &Path,
    manifest: &SyncBackupManifest,
    body: SyncBodyState,
) -> Result<(), String> {
    // 仅同步归档：正文零写入，也不提交新基线。必须在这里分流——复用正文分支时
    // `Gone` 会把备份里的目标原文写回业务正文，等于补写正文。
    if manifest.mode == SyncMode::StatusOnly {
        return run_status_only_phases(paths, operation, manifest, body);
    }
    let normalized = match body {
        // 已写入：只核验现场，不重写。
        SyncBodyState::Ours => {
            let target_body_path = validated_target_body_path(paths, manifest)?;
            match session_link::read_content_snapshot(
                &target_body_path,
                &manifest.target.session_id,
            ) {
                ContentState::Ready(content)
                    if content.normalized.total_digest == operation.expected_content_digest =>
                {
                    content.normalized
                }
                ContentState::Ready(_) => {
                    return Err("目标内容与本次保存不一致，已停止恢复".to_string())
                }
                ContentState::Missing => return Err("目标内容丢失，已停止恢复".to_string()),
                ContentState::Unavailable(reason) => {
                    return Err(format!("目标内容无法验证（{reason}），已停止恢复"))
                }
            }
        }
        SyncBodyState::PreSync | SyncBodyState::Gone => {
            let normalized = apply_sync_body(
                paths,
                backup_dir,
                manifest,
                &operation.expected_content_digest,
            )?;
            advance_operation(paths, operation, OpPhase::BodyWritten)?;
            normalized
        }
        SyncBodyState::Unknown(reason) => return Err(reason),
    };

    // 数据库步骤必须**恰好执行一次**：本次时间戳已落库说明这一步已由本操作提交过，
    // 重跑会把首次执行时合法跳过的归档副作用补上（当时目标是 active，现在回到
    // completed 又变成了「可归档」）——等于在恢复里执行了用户当次没同意的动作。
    // 仅同步归档不受影响：它的 `db_applied` 已确认状态就是本次新值，重跑等价。
    let db_already_applied = manifest.mode != SyncMode::StatusOnly
        && session_row_snapshot(paths, &operation.target.session_id)?
            .and_then(|row| row.updated_at)
            == Some(manifest.db.new_updated_at);
    if db_already_applied {
        // 行仍归属目标账号且未删除即可视为本操作的产物，不写库、不改状态。
        let owner = session_row_owner(paths, &operation.target.session_id)
            .ok_or_else(|| "目标会话记录不存在或已被删除，已停止恢复".to_string())?;
        if owner != operation.target.uid {
            return Err("目标会话记录归属异常，已停止恢复".to_string());
        }
    } else {
        let status_change = planned_status_change(paths, operation, manifest)?;
        update_target_session_row(
            paths,
            &operation.target,
            manifest.db.new_updated_at,
            manifest.db.target_row.as_ref(),
            status_change.as_ref(),
            // 时间来自清单里那份已核验摘要的待写入正文（不重新读目标文件）。旧清单没有
            // 这个字段时为 None：跳过写该列、保持现值，升级后重放老操作不受影响。
            manifest.incoming.latest_activity_at,
        )?;
    }
    advance_operation(paths, operation, OpPhase::DbWritten)?;

    if operation.phase < OpPhase::LinksCommitted {
        commit_sync_baseline(paths, manifest, &normalized)?;
        advance_operation(paths, operation, OpPhase::LinksCommitted)?;
    } else {
        verify_sync_baseline_committed(paths, manifest)?;
    }
    advance_operation(paths, operation, OpPhase::Completed)?;
    Ok(())
}

/// 执行单条同步：备份 → 阶段化写入 → completed。
///
/// 任一阶段失败都保留未完成操作（同一目标 UUID 与新基线引用，下次切换按清单补完），
/// 绝不把未完成的写入报告成 `synced`。
pub(in crate::modules::session) fn execute_sync_item(
    context: &SyncContext,
    plan: &SyncWritePlan,
    mode: SyncMode,
    verdict: SyncVerdict,
) -> Result<Value, String> {
    let paths = context.paths;
    // 同一目标上仍有未完成写入：本轮不得再写一次（等恢复完成）。
    if let Some(operation) = context.pending.iter().find(|operation| {
        operation.phase.is_unfinished()
            && operation.target.session_id == plan.target_member.session_id
    }) {
        return Err(format!(
            "上一次会话保存尚未完成（操作 {}）：{}，本次未保存",
            operation.operation_id,
            operation
                .last_error
                .clone()
                .unwrap_or_else(|| "等待恢复".to_string())
        ));
    }
    let target_body_path = find_project_jsonl(paths, &plan.target_member.session_id)
        .ok_or_else(|| "目标内容不存在，未同步".to_string())?;
    let new_updated_at = now_ms();
    let last_synced_at = now_ms();
    // 仅同步归档不产生新正文，也就没有新配对基线：显式记为 None，不用空引用占位。
    let new_baseline_ref = (mode != SyncMode::StatusOnly).then(|| uuid::Uuid::new_v4().to_string());
    // 预分配身份：维护记录（allocating）先于备份与业务写入（design §3）。
    let target_title =
        session_row_info(paths, &plan.target_member.session_id).map(|(title, _)| title);
    let mut lifecycle = session_backup::begin_operation(
        paths,
        context.variant,
        OPERATION_KIND_SYNC,
        Some(plan.target_member.session_id.clone()),
        target_title,
    )?;
    // 备份是一道门禁：备份不可信就零写入（不碰正文、不改数据库、不提交基线）。
    let backup = match create_sync_backup(
        paths,
        context.variant,
        &lifecycle.operation_id,
        plan,
        &target_body_path,
        new_updated_at,
        new_baseline_ref.as_deref(),
        last_synced_at,
    ) {
        Ok(backup) => backup,
        Err(error) => {
            // 受控失败分支：protected 之前未写业务，可安全回收准备残留。
            session_backup::reclaim_unwritten(paths, context.variant, &mut lifecycle, &error);
            return Err(error);
        }
    };
    // 备份完备先转 protected：此步失败禁止业务写入，残留保守保留待下次维护。
    session_backup::mark_protected(paths, &mut lifecycle)?;

    let mut operation = Operation {
        version: OPERATION_VERSION,
        operation_id: lifecycle.operation_id.clone(),
        kind: OPERATION_KIND_SYNC.to_string(),
        variant: context.variant,
        // 同档同步写 None（与旧记录逐字兼容）；跨档同步显式记录源档（审计与恢复依据）。
        source_variant: (context.source_variant != context.variant)
            .then_some(context.source_variant),
        group_id: plan.group_id.clone(),
        source: OperationMember {
            account_id: context.source_account_id.clone(),
            uid: plan.source_member.uid.clone(),
            session_id: plan.source_member.session_id.clone(),
        },
        target: OperationMember {
            account_id: context.target_account_id.clone(),
            uid: plan.target_member.uid.clone(),
            session_id: plan.target_member.session_id.clone(),
        },
        expected_content_digest: plan.incoming.total_digest.clone(),
        expected_record_count: plan.incoming.record_count,
        phase: OpPhase::Prepared,
        backup: Some(backup.manifest_file.to_string_lossy().to_string()),
        lifecycle_version: Some(OPERATION_LIFECYCLE_VERSION),
        cleanup_state: None,
        last_error: None,
        // 重放/同步写入不搬会话账本：本字段只对复制操作有意义。
        aux_skipped: false,
        created_at: now_ms(),
        updated_at: now_ms(),
    };
    session_link::save_operation(paths, &operation)?;

    let body = classify_sync_body(
        &target_body_path,
        &plan.target_member.session_id,
        &plan.target_snapshot.full_digest,
        &operation.expected_content_digest,
    );
    if let Err(error) = run_sync_phases(paths, &mut operation, &backup.dir, &backup.manifest, body)
    {
        fail_operation(paths, &mut operation, &error);
        return Err(error);
    }
    // R6-07 写入点 2/4：WorkBuddy **同步**的归属交接。
    //
    // `session_groups::sync_all` / `session::sync_sessions_cross` 最终都走到这里（
    // 恢复重放也走这里），因此**只埋一次**。位置在 `run_sync_phases` 成功之后：
    // 此时操作已推进到 `Completed`，本次同步只会完成一次。
    //
    // `StatusOnly`（仅同步归档）零正文写入、不产生作者变更 → 必须跳过，否则会给一个
    // 正文没变的会话凭空加一段归属。boundary 取 `max(本次正文最新活跃时间, now)`；
    // 活跃时间来自清单里那份已核验摘要（`plan.incoming_text` 的字节），不重新读目标文件。
    if mode != SyncMode::StatusOnly {
        usage_origin::record_sync_quiet(
            &paths.store_root,
            usage_origin::namespace_for_variant(context.variant),
            &operation.target.session_id,
            usage_origin::namespace_for_variant(
                operation.source_variant.unwrap_or(context.variant),
            ),
            &operation.source.session_id,
            backup
                .manifest
                .incoming
                .latest_activity_at
                .map_or_else(now_ms, |latest| latest.max(now_ms())),
            &operation.source.uid,
        );
    }
    let _ = session_link::prune_operations(
        paths,
        context.variant,
        session_link::KEEP_COMPLETED_OPERATIONS,
    );
    // 业务可靠完成后才授权清理；清理失败不回滚业务、不报告同步失败。
    let cleanup = finish_backup_cleanup(paths, context.variant, &mut lifecycle);
    let (backup_path, cleanup_state, cleanup_error) = report_cleanup(&cleanup, &backup.dir);
    let manifest_path = backup_path
        .as_ref()
        .map(|path| format!("{path}/manifest.json"));
    let mut item = json!({
        "groupId": plan.group_id,
        "status": "synced",
        "verdict": verdict.as_str(),
        "mode": mode.as_str(),
        "sourceSessionId": plan.source_member.session_id,
        "targetSessionId": plan.target_member.session_id,
        "recordCount": {
            "source": plan.source_records(),
            "targetBefore": plan.target_snapshot.normalized.record_count,
            "target": plan.incoming.record_count,
        },
        "updatedAt": backup.manifest.db.new_updated_at,
        "backup": backup_path,
        "backupManifest": manifest_path,
        "cleanupState": cleanup_state,
        "message": plan.reason,
    });
    if let Some(error) = cleanup_error {
        item["cleanupError"] = json!(error);
    }
    Ok(item)
}
