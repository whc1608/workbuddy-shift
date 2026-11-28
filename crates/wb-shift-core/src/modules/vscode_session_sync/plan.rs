//! 同步计划：sync_selected 入口、SyncItemPlan 与逐项计划/消息文件校验。

use super::*;

/// 执行勾选的同步项；返回 `{synced, skipped, errors}` 报告。
///
/// **前提**：调用方已确认编辑器完全退出（写入会被运行中的插件覆盖）。本函数自行复查一次，
/// 并在执行前重新加载身份 / 成员 / 基线 / 正文后用预览凭据逐项核对：
/// 任一变化都跳过该项（原因码 [`REASON_PREVIEW_STALE`]），绝不静默写入。
pub fn sync_selected(target_acc: &Value, selections: &[SyncSelection]) -> Result<Value, String> {
    if selections.is_empty() {
        return Ok(empty_report());
    }
    if vscode_ext::is_vscode_running() {
        return Err(
            "检测到 VS Code 正在运行，请先完全退出后再同步会话，否则写入会被 VS Code 覆盖。"
                .to_string(),
        );
    }
    let root = vscode_session::ext_data_root()
        .ok_or_else(|| "未找到 VS Code CodeBuddy 插件数据目录，无法同步会话".to_string())?;
    let source_uid = vscode_ext::active_ext_uid()
        .filter(|uid| !uid.trim().is_empty())
        .ok_or_else(|| {
            "未检测到 VS Code CodeBuddy 插件当前登录账号，无法同步会话。请先在 VS Code 中登录该插件后重试。"
                .to_string()
        })?;
    sync_selected_at(
        &root,
        &SessionPaths::for_vscode_ext(),
        &source_uid,
        target_acc,
        selections,
    )
}

/// [`sync_selected`] 的可测实现：显式传入数据根、存储路径与来源 uid（VS Code 插件数据仓）。
pub fn sync_selected_at(
    root: &Path,
    paths: &SessionPaths,
    source_uid: &str,
    target_acc: &Value,
    selections: &[SyncSelection],
) -> Result<Value, String> {
    sync_selected_in(
        VSCODE_STORE,
        root,
        paths,
        source_uid,
        target_acc,
        selections,
    )
}

/// [`sync_selected`] 的数据仓参数化版本（VS Code 插件：不按 variant 过滤）。
pub fn sync_selected_in(
    spec: SessionStoreSpec,
    root: &Path,
    paths: &SessionPaths,
    source_uid: &str,
    target_acc: &Value,
    selections: &[SyncSelection],
) -> Result<Value, String> {
    sync_selected_in_filtered(spec, root, paths, source_uid, target_acc, selections, None)
}

/// IDE 共用关联表时的同步：拒绝另一档位的组（即使成员 uid 碰巧相同）。
pub(crate) fn sync_selected_in_for_variant(
    spec: SessionStoreSpec,
    root: &Path,
    paths: &SessionPaths,
    source_uid: &str,
    target_acc: &Value,
    selections: &[SyncSelection],
    variant: WbVariant,
) -> Result<Value, String> {
    sync_selected_in_filtered(
        spec,
        root,
        paths,
        source_uid,
        target_acc,
        selections,
        Some(variant),
    )
}

pub(super) fn sync_selected_in_filtered(
    spec: SessionStoreSpec,
    root: &Path,
    paths: &SessionPaths,
    source_uid: &str,
    target_acc: &Value,
    selections: &[SyncSelection],
    variant_filter: Option<WbVariant>,
) -> Result<Value, String> {
    if selections.is_empty() {
        return Ok(empty_report());
    }
    let target_uid = target_uid_of(target_acc)?;
    if source_uid == target_uid {
        return Err("当前账号与目标账号相同，无需同步会话".to_string());
    }
    let store = match session_link::load_store(paths) {
        StoreState::Ready(store) => store,
        StoreState::Missing => {
            return Err("同步记录不存在或不可用，无法同步会话".to_string());
        }
        StoreState::Unavailable(reason) => return Err(reason),
    };
    let source_index = conversation_index(spec, root, source_uid);
    let target_index = conversation_index(spec, root, &target_uid);
    let context = SyncContext {
        spec,
        paths,
        root,
        source_uid,
        target_uid: &target_uid,
        source_index: &source_index,
        target_index: &target_index,
        variant_filter,
    };

    let mut synced: Vec<Value> = Vec::new();
    let mut skipped: Vec<Value> = Vec::new();
    let mut errors: Vec<Value> = Vec::new();
    for selection in selections {
        let group_id = selection.group_id.clone();
        match plan_sync_item(&context, &store, selection) {
            SyncItemOutcome::Validated(plan) => match execute_sync_item(paths, &plan) {
                Ok(item) => synced.push(item),
                Err(error) => errors.push(json!({ "groupId": group_id, "error": error })),
            },
            SyncItemOutcome::Skipped { message, verdict } => skipped.push(json!({
                "groupId": group_id,
                "status": "skipped",
                "reasonCode": REASON_PREVIEW_STALE,
                "message": message,
                "verdict": verdict.map(SyncVerdict::as_str),
            })),
            SyncItemOutcome::Rejected { message } => {
                errors.push(json!({ "groupId": group_id, "error": message }))
            }
        }
    }
    Ok(json!({ "synced": synced, "skipped": skipped, "errors": errors }))
}

/// 一次写入所需的已核对信息（校验与写入之间不再回读来源，避免 TOCTOU）。
pub(crate) struct SyncItemPlan {
    pub(super) group_id: String,
    pub(super) mode: SyncMode,
    pub(super) verdict: SyncVerdict,
    pub(super) source_member: LinkMember,
    pub(super) target_member: LinkMember,
    pub(super) source_dir: PathBuf,
    pub(super) target_dir: PathBuf,
    /// 目标会话所属工作区（备份目录布局用）。
    pub(super) workspace_hash: String,
    pub(super) source_conversation_id: String,
    pub(super) target_conversation_id: String,
    pub(super) target_uid: String,
    /// 备份根目录名（`<工具存储根>/backups/<backup_kind>/<utc_iso>/`）。
    pub(super) backup_kind: &'static str,
    /// R6-07：该数据仓的归属命名空间（`ns_for_client_dir(context.spec.client_dir)`）。
    ///
    /// `None` 表示未知客户端：不记归属索引，退化为 R6-07 之前的行为。
    /// 执行阶段拿不到 `SessionStoreSpec`，因此在这里（`plan_sync_item` 里有 `context.spec`）
    /// 一次算好带下来。
    pub(super) ns: Option<&'static str>,
    pub(super) source_content: ContentSnapshot,
    pub(super) target_content: ContentSnapshot,
}

/// 单条同步选择的校验结果。
pub(super) enum SyncItemOutcome {
    Validated(Box<SyncItemPlan>),
    /// 版本变化 / 前置条件不满足：跳过该项，不沿用用户旧选择。
    Skipped {
        message: String,
        verdict: Option<SyncVerdict>,
    },
    /// 入参或凭据非法、模式越权：拒绝，不静默执行。
    Rejected {
        message: String,
    },
}

/// 重新校验单条选择：凭据、身份、成员、基线、正文逐项核对（design §7.3）。
pub(super) fn plan_sync_item(
    context: &SyncContext<'_>,
    store: &LinkStore,
    selection: &SyncSelection,
) -> SyncItemOutcome {
    // 凭据必须是我们服务端保存过的：伪造的 id 读不到，直接拒绝。
    let Some(token) = session_link::load_preview_token(context.paths, &selection.preview_token)
    else {
        return SyncItemOutcome::Rejected {
            message: "检查结果不存在或已失效，请重新检查后再操作".to_string(),
        };
    };
    let binding = &token.binding;
    if binding.group_id != selection.group_id
        || !passes_variant_filter(context.variant_filter, binding.variant)
    {
        return SyncItemOutcome::Rejected {
            message: "检查结果与所选会话不匹配，已拒绝".to_string(),
        };
    }
    let skip = |message: String| SyncItemOutcome::Skipped {
        message,
        verdict: Some(binding.verdict),
    };
    if binding.source.uid != context.source_uid || binding.target.uid != context.target_uid {
        return skip("账号已变化，检查结果已失效".to_string());
    }
    let Some(group) = store.groups.iter().find(|group| {
        group.id == selection.group_id
            && passes_variant_filter(context.variant_filter, group.variant)
    }) else {
        return skip("会话的关联关系已不存在，检查结果已失效".to_string());
    };
    let (Some(source_member), Some(target_member)) = (
        session_link::active_member_for(group, context.source_uid),
        session_link::active_member_for(group, context.target_uid),
    ) else {
        return skip("对应的会话已失效，检查结果已失效".to_string());
    };
    let (Some(source_dir), Some(target_dir)) = (
        context.dir_of(context.source_uid, &source_member.session_id),
        context.dir_of(context.target_uid, &target_member.session_id),
    ) else {
        return skip("找不到会话所在的工作区，检查结果已失效".to_string());
    };
    // 重新加载正文、基线与判定，再与凭据逐项核对；任一变化都跳过（含显式覆盖）。
    let source_content =
        vscode_session_link::read_session_content(&source_dir, &source_member.session_id);
    let target_content =
        vscode_session_link::read_session_content(&target_dir, &target_member.session_id);
    let baseline = session_link::load_pair_baseline(
        context.paths,
        group,
        &source_member.member_id,
        &target_member.member_id,
    );
    let decision = session_link::decide_sync(&source_content, &target_content, &baseline);
    let live = live_preview_binding(
        group,
        source_member,
        target_member,
        &source_content,
        &target_content,
        &baseline,
        decision.verdict,
    );
    let mismatches = session_link::verify_preview(&token, &live);
    if !mismatches.is_empty() {
        // 与桌面端同一口径：凭据核对排在模式门禁之前，若只报「内容已变化」，用户看不到
        // 「哪一侧、坏在哪」。至少一侧不可验证时补上当前状态；两侧都 Ready（只是内容变多
        // 变少）不补——那种情况没有「坏了」可讲。
        let detail = session_link::unverifiable_sides(&source_content, &target_content)
            .map(|sides| format!("（当前内容状态：{sides}）"))
            .unwrap_or_default();
        return skip(format!("检查结果已失效：{}{detail}", mismatches.join("；")));
    }
    if !decision.verdict.allows(selection.mode) {
        return SyncItemOutcome::Rejected {
            // 前缀逐字保留（前端与既有测试按它匹配），只追加判定自己算出的原因。
            message: format!(
                "该组判定为 {}，不允许以 {} 模式同步：{}",
                decision.verdict.as_str(),
                selection.mode.as_str(),
                decision.reason
            ),
        };
    }
    // 判定非 unknown 必然双方正文可验证（decide_sync 的前置条件），这里仍然显式兜底。
    let (ContentState::Ready(source_snapshot), ContentState::Ready(target_snapshot)) =
        (&source_content, &target_content)
    else {
        let sides = session_link::unverifiable_sides(&source_content, &target_content)
            .unwrap_or_else(|| "两侧内容都可验证，无法给出具体原因".to_string());
        return skip(format!("双方内容不可验证，检查结果已失效（{sides}）"));
    };
    SyncItemOutcome::Validated(Box::new(SyncItemPlan {
        group_id: selection.group_id.clone(),
        mode: selection.mode,
        verdict: decision.verdict,
        source_member: source_member.clone(),
        target_member: target_member.clone(),
        source_dir,
        target_dir,
        workspace_hash: context
            .locator_of(context.target_uid, &target_member.session_id)
            .map(|locator| locator.workspace_hash.clone())
            .unwrap_or_default(),
        source_conversation_id: source_member.session_id.clone(),
        target_conversation_id: target_member.session_id.clone(),
        target_uid: context.target_uid.to_string(),
        backup_kind: context.spec.backup_kind,
        // R6-07：命名空间与读侧（`token_stats::ide_client_source`）、写入侧
        // （`vscode_session::copy_sessions_in_with`）共用同一份 `client_dir` 映射。
        ns: crate::modules::usage_origin::IdeClientSource::from_client_dir(context.spec.client_dir)
            .map(|client| client.namespace()),
        source_content: source_snapshot.clone(),
        target_content: target_snapshot.clone(),
    }))
}

/// 规划一条新增消息文件：确定性 id + 冲突 salt 重试（design §6.1）。
pub(super) fn plan_message_file(
    plan: &SyncItemPlan,
    index: usize,
    digest: &str,
    source_id: &str,
    value: &Value,
    translation: &mut BTreeMap<String, String>,
) -> Result<PlannedFile, String> {
    for salt in 0..SALT_LIMIT {
        let id = vscode_session_link::derive_message_id(
            &plan.target_uid,
            &plan.target_conversation_id,
            index,
            digest,
            salt,
        );
        translation.insert(source_id.to_string(), id.clone());
        let content = vscode_session_link::translate_message(value, translation).to_string();
        let path = plan.target_dir.join("messages").join(format!("{id}.json"));
        match std::fs::read_to_string(&path) {
            // 目标已有同名文件且内容不同：换 salt 重试，不覆盖别人的内容。
            Ok(existing) if existing != content => continue,
            // 已应用过同一份内容：本次不写（重跑幂等）。
            Ok(_) => {
                return Ok(PlannedFile {
                    id,
                    path,
                    content,
                    already_applied: true,
                })
            }
            Err(_) => {
                return Ok(PlannedFile {
                    id,
                    path,
                    content,
                    already_applied: false,
                })
            }
        }
    }
    Err("目标账号已存在同名消息且内容不同，已停止同步".to_string())
}

/// 索引里的请求 id 序列。
pub(super) fn request_ids_of(index: &Value) -> Vec<String> {
    index
        .get("requests")
        .and_then(Value::as_array)
        .map(|requests| {
            requests
                .iter()
                .filter_map(|request| {
                    request
                        .get("id")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|id| !id.is_empty())
                        .map(str::to_string)
                })
                .collect()
        })
        .unwrap_or_default()
}
