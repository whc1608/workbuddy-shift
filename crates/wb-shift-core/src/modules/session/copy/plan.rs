//! 复制流程编排：锁与恢复顺序、逐会话循环与结果报告。
//!
//! 对外入口与开关参数在 `entry.rs`；这里只做「拿锁 → 恢复 → 逐会话复制 → 组装报告」，
//! 末尾按需挂接增强数据（置顶、工作记忆）。
use super::super::recovery::recover_pending_session_operations_at_with;
use super::entry::{CopyOptions, CopySide};
// 显式导入：`plan` 与 `same_content` 互相引用（后者要 `CopyContext`），glob 化会让
// `same_content` 的转出在本文件被解析成空集。
use super::same_content::SessionRowIndex;
use super::*;

/// 国际版支持性探测的统一封装：只有国际版需要数据根同构检查（design D6）。
pub(in crate::modules::session) fn ensure_session_copy_supported(
    paths: &SessionPaths,
    variant: WbVariant,
) -> Result<(), String> {
    if variant == WbVariant::Ai && !session_copy_supported_at(&paths.data_root) {
        return Err(format!(
            "{SESSION_COPY_UNSUPPORTED}（档位 {}）",
            variant.as_str()
        ));
    }
    Ok(())
}

/// 同档与跨档复制的统一流程：双档锁 → 恢复 → 逐会话复制 → 报告。
///
/// `resolve_source_paths` 用于恢复未完成的**跨档**操作时解析其源档数据根；
/// 同档入口传 [`SessionPaths::for_variant`]，跨档入口可注入与两条 side 一致的映射，
/// 单测由此避免触碰真实目录。
pub(in crate::modules::session) fn run_copy(
    source: &CopySide<'_>,
    target: &CopySide<'_>,
    session_ids: &[String],
    options: CopyOptions,
    is_app_running: impl Fn(WbVariant) -> bool,
    resolve_source_paths: impl Fn(WbVariant) -> SessionPaths,
) -> Result<Value, String> {
    // 档位操作锁覆盖「恢复 → 查询关联 → 写副本 → 提交关联」全过程，
    // 并发请求与中断重试因此不会各自写出第二个副本。
    // 跨档操作同时持有两档锁，获取顺序固定为枚举序（Cn → Ai），任何代码路径不得
    // 反序，避免两个方向的跨档操作互相等待（design §2.5）。
    let mut _ops_locks = Vec::new();
    for variant in WbVariant::ALL {
        if variant == source.variant {
            _ops_locks.push(session_link::try_acquire_variant_ops_lock(
                source.paths,
                variant,
            )?);
        } else if variant == target.variant {
            _ops_locks.push(session_link::try_acquire_variant_ops_lock(
                target.paths,
                variant,
            )?);
        }
    }
    // 复查：锁前未运行、拿锁后目标账号被登录进 App 的情况在这里被拦住，不写入任何产物。
    if app_running_blocks_target_write(target.paths, target.variant, &target.uid, &is_app_running) {
        return Err(SESSION_COPY_APP_RUNNING.to_string());
    }
    // 「仍然复制（不带用量）」的重试：上一次失败时操作停在 BodyWritten，本次请求要把
    // 忽略标记**先落到操作记录**，随后的恢复重放才会跳过账本并补完同一条会话。
    if options.ignore_aux {
        mark_aux_skipped_for_requests(target, source, session_ids);
    }
    let recovery = recover_pending_session_operations_at_with(
        target.paths,
        target.variant,
        resolve_source_paths,
    );
    let pending = session_link::pending_operations(target.paths, target.variant);

    let context = CopyContext {
        paths: target.paths,
        variant: target.variant,
        source_paths: source.paths,
        source_variant: source.variant,
        source_uid: &source.uid,
        source_account_id: source.account_id.clone(),
        target_uid: &target.uid,
        target_account_id: target.account_id.clone(),
        pending: &pending,
        ignore_aux: options.ignore_aux,
        // R6-08：账本搬运开关原样下传（默认 false → 副本不带来源历史消耗）。
        copy_usage: options.copy_usage,
        copy_interruptions: options.copy_interruptions,
        // 整批评审用同一份会话行索引（见 `copy/same_content.rs`）。
        rows: SessionRowIndex::default(),
    };

    let mut copied: Vec<Value> = Vec::new();
    let mut already_linked: Vec<Value> = Vec::new();
    let mut errors: Vec<Value> = Vec::new();
    // 可「显式忽略」的失败（账本写入）：前端据此给「仍然复制」按钮。
    let mut aux_failed: Vec<Value> = Vec::new();
    // 解析失败的操作日志无法对应到具体会话：继续复制会绕过 pending 去重，
    // 可能对同一请求再写出第二个副本。
    if let Some(issue) = recovery
        .needs_recovery
        .iter()
        .find(|issue| !issue.retryable && issue.reason.contains(UNPARSEABLE_OPERATION_REASON))
    {
        for cid in session_ids {
            errors.push(json!({"id": cid, "error": issue.reason.clone()}));
        }
    } else {
        for cid in session_ids {
            match copy_one_session(&context, cid) {
                Ok(CopyOutcome::Copied {
                    new_id,
                    group_id,
                    backup,
                    cleanup_state,
                    cleanup_error,
                    source_repaired,
                    aux_note,
                    space_note,
                    expert,
                }) => {
                    let mut item = json!({
                        "id": cid,
                        "newId": new_id,
                        "groupId": group_id,
                        "backup": backup,
                        "cleanupState": cleanup_state,
                    });
                    if let Some(error) = cleanup_error {
                        item["cleanupError"] = json!(error);
                    }
                    // 源正文有坏行、复制时做了容错处理：必须让用户知道副本与原会话不逐字相同。
                    if let Some(note) = source_repaired {
                        item["sourceRepaired"] = json!(note);
                    }
                    // 目标库缺账本表/列：不算失败，但要让用户知道「共消耗」为什么还是少。
                    if let Some(note) = aux_note {
                        item["auxNote"] = json!(note);
                    }
                    // 同一目录命中多个目标空间：本次按目录新建了分组，别让用户以为串组了。
                    if let Some(note) = space_note {
                        item["spaceNote"] = json!(note);
                    }
                    // 专家在目标账号不可用只提示：默认仍勾选、仍复制（R5-13 的决策）。
                    if expert.state != "unknown" {
                        item["expert"] = json!({
                            "state": expert.state,
                            "plugin": expert.plugin,
                            "name": expert.name,
                        });
                    }
                    copied.push(item);
                }
                Ok(CopyOutcome::AlreadyLinked {
                    session_id,
                    group_id,
                }) => already_linked.push(json!({
                    "id": cid,
                    "sessionId": session_id,
                    "groupId": group_id,
                })),
                Err(error) => {
                    // 账本写入失败是唯一带「显式忽略」出口的失败：报告里点明可忽略，
                    // 前端据此给「仍然复制（不带用量与中断记录）」按钮。
                    if is_aux_write_failure(&error) {
                        aux_failed.push(json!(cid));
                    }
                    errors.push(json!({"id": cid, "error": error}));
                }
            }
        }
    }

    // 本次请求之后仍存在未完成操作（含本次刚留下的）→ 必须提示恢复需求。
    let unfinished_after = session_link::pending_operations(target.paths, target.variant);
    let unusable = !recovery.is_clean() || !unfinished_after.is_empty();
    let mut report = json!({
        "sourceUid": &source.uid,
        "targetUid": &target.uid,
        "copied": copied,
        "alreadyLinked": already_linked,
    });
    // 跨档复制在报告里带两侧档位；同档报告保持原字段（逐字不变）。
    if source.variant != target.variant {
        report["sourceVariant"] = json!(source.variant.as_str());
        report["targetVariant"] = json!(target.variant.as_str());
    }
    if !errors.is_empty() {
        report["errors"] = json!(errors);
    }
    // 只有账本写入失败才可忽略：给出「仍然复制（不带用量与中断记录）」入口所需的
    // 会话 id；其它失败一律没有这个出口（不该用忽略掩盖数据缺失）。
    if !aux_failed.is_empty() {
        report["canIgnoreAux"] = json!(true);
        report["auxFailed"] = json!(aux_failed);
    }
    // 本次请求本身带了忽略标记：让前端能把「已按忽略处理」如实提示出来。
    if options.ignore_aux {
        report["auxSkipped"] = json!(true);
    }
    // 恢复重放补完的会话：重试「仍然复制」时它们不会出现在 copied 里（已是既有副本），
    // 不说明的话用户会以为没生效。
    if !recovery.recovered.is_empty() {
        report["recovered"] = json!(recovery.recovered);
    }
    if unusable {
        report["needsRecovery"] = json!(true);
    }
    // 本轮复制之后再扫一遍：当前项的清理失败/保护残留必须出现在报告里，
    // 不能只用请求开始时的维护快照（否则成功项 pending 只在 copied[] 上）。
    // 维护可能补清成功：成功项上的 pending 路径必须改写成 null，避免虚假可还原位置。
    report["temporaryFiles"] = json!(session_backup::maintain(target.paths, target.variant));
    if let Some(items) = report.get_mut("copied").and_then(Value::as_array_mut) {
        reconcile_reported_cleanup(items);
    }
    // 置顶状态不在 `sessions` 表里，得单独同步（见 `copy/pinned.rs`）。复制与「已关联」
    // 两种结果都提供 source→target 的 id 映射；置顶是增强数据，失败只落报告字段，
    // 绝不影响正文/DB 结果，所以放在所有关键写入之后。
    let mut pinned_pairs: Vec<(String, String)> = Vec::new();
    if let Some(items) = report.get("copied").and_then(Value::as_array) {
        for item in items {
            if let (Some(from), Some(to)) = (
                item.get("id").and_then(Value::as_str),
                item.get("newId").and_then(Value::as_str),
            ) {
                pinned_pairs.push((from.to_string(), to.to_string()));
            }
        }
    }
    if let Some(items) = report.get("alreadyLinked").and_then(Value::as_array) {
        for item in items {
            if let (Some(from), Some(to)) = (
                item.get("id").and_then(Value::as_str),
                item.get("sessionId").and_then(Value::as_str),
            ) {
                pinned_pairs.push((from.to_string(), to.to_string()));
            }
        }
    }
    report["pinned"] = sync_pinned(
        source.paths,
        &source.uid,
        target.paths,
        &target.uid,
        &pinned_pairs,
    )
    .to_json();
    // 工作记忆（`MEMORY.md`）按操作勾选：用户没勾就**完全不执行**——连源文件都不读，
    // 报告里也不出现 `memory` 字段（见 `copy/memory.rs`）。同样放在所有关键写入之后：
    // 失败只落 `skippedReason`，不影响正文 / DB / 复制结果。
    if options.copy_memory {
        report["memory"] =
            sync_memory(source.paths, &source.uid, target.paths, &target.uid).to_json();
    }
    Ok(report)
}

/// 复制上下文中不变的输入（避免逐会话重复解析）。
///
/// 源 / 目标解耦：`paths` 恒为目标档（写侧），`source_paths` 恒为源档（读侧）；
/// 同档复制时两者相同，跨档复制时指向不同数据根。
pub(in crate::modules::session) struct CopyContext<'a> {
    pub(in crate::modules::session) paths: &'a SessionPaths,
    pub(in crate::modules::session) variant: WbVariant,
    pub(in crate::modules::session) source_paths: &'a SessionPaths,
    pub(in crate::modules::session) source_variant: WbVariant,
    pub(in crate::modules::session) source_uid: &'a str,
    pub(in crate::modules::session) source_account_id: Option<String>,
    pub(in crate::modules::session) target_uid: &'a str,
    pub(in crate::modules::session) target_account_id: Option<String>,
    pub(in crate::modules::session) pending: &'a [Operation],
    /// 用户显式忽略账本（用量 + 中断记录）：本次新建的操作记录带 `auxSkipped`。
    pub(in crate::modules::session) ignore_aux: bool,
    /// R6-08：是否把来源账号的 `session_usage` 搬到副本（默认 `false` = 不搬）。
    pub(in crate::modules::session) copy_usage: bool,
    /// R6-08：是否搬中断记录（默认 `true`，随 `ignore_aux` 反向取值）。
    pub(in crate::modules::session) copy_interruptions: bool,
    /// 会话行索引（懒加载）：复制前的内容级去重需要按账号查会话列表，
    /// 整批复制复用同一份结果，避免逐会话重复打开数据库与扫描 `projects/`。
    pub(in crate::modules::session) rows: SessionRowIndex,
}

/// 把「忽略账本」标记落到**未完成**操作的记录上（「仍然复制」的重试路径）。
///
/// 只改与本次请求一一对应的操作（同源会话、同目标账号、未完成），不做全量清理；
/// 保存失败不阻断本次流程——后面同一处会再失败一次，错误照样报给用户。
fn mark_aux_skipped_for_requests(
    target: &CopySide<'_>,
    source: &CopySide<'_>,
    session_ids: &[String],
) {
    for operation in session_link::pending_operations(target.paths, target.variant) {
        if operation.aux_skipped
            || operation.source.uid != source.uid
            || operation.target.uid != target.uid
            || !session_ids.contains(&operation.source.session_id)
        {
            continue;
        }
        let mut skipped = operation;
        skipped.aux_skipped = true;
        skipped.updated_at = now_ms();
        let _ = session_link::save_operation(target.paths, &skipped);
    }
}

/// 单个会话的一次复制结果。
pub(in crate::modules::session) enum CopyOutcome {
    Copied {
        new_id: String,
        group_id: String,
        /// 待清理位置（已清理为 None）；仅表示待清理，不是可撤销备份。
        backup: Option<String>,
        cleanup_state: String,
        cleanup_error: Option<String>,
        /// 源正文有坏行、复制时做了容错修复时的人话说明（报告里为 `sourceRepaired`）。
        /// `None` 表示源正文严格校验一次通过，副本与原会话逐字同源。
        source_repaired: Option<String>,
        /// 账本搬运的诊断说明（报告里为 `auxNote`）：目标客户端不支持时为 `Some`。
        aux_note: Option<&'static str>,
        /// 空间归属的诊断说明（报告里为 `spaceNote`）：同目录多分组时按目录新建。
        space_note: Option<&'static str>,
        /// 专家绑定判定（报告里为 `expert`）：只用于提示，不影响复制结果。
        expert: super::sync::expert_binding::ExpertBinding,
    },
    AlreadyLinked {
        session_id: String,
        group_id: String,
    },
}
