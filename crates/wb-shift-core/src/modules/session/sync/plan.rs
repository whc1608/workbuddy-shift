//! 同步写入计划：逐项重新校验凭据与内容版本，输出可执行计划或跳过 / 拒绝原因。
use super::*;
// 侧别与原因的措辞只有一份实现（`session_link::sync_decision`），VS Code 入口共用。
use crate::modules::session_link::unverifiable_sides;

/// 校验通过后的执行计划：写入阶段与报告所需的全部已核对信息。
///
/// 正文快照来自同一次读取（在档位锁内、App 已停止写入之后），执行时不再回读来源，
/// 避免执行与校验之间的 TOCTOU。
pub(in crate::modules::session) struct SyncWritePlan {
    pub(in crate::modules::session) group_id: String,
    pub(in crate::modules::session) mode: SyncMode,
    pub(in crate::modules::session) verdict: SyncVerdict,
    pub(in crate::modules::session) source_member: LinkMember,
    pub(in crate::modules::session) target_member: LinkMember,
    pub(in crate::modules::session) source_snapshot: ContentSnapshot,
    pub(in crate::modules::session) target_snapshot: ContentSnapshot,
    /// 待写入目标的新正文：来源正文按本副本 sessionId 替换为目标 sessionId。
    pub(in crate::modules::session) incoming_text: String,
    /// 待写入正文的归一化内容（执行后据此复算摘要核验）。
    pub(in crate::modules::session) incoming: NormalizedContent,
    /// 提交前该成员对的基线引用（清单里记录，便于人工比对）。
    pub(in crate::modules::session) old_baseline_ref: Option<String>,
    /// 本次是否把目标对齐为 `archived`：状态模式下是唯一动作，正文模式下是副作用。
    pub(in crate::modules::session) archive_target: bool,
    /// 目标行在写入前的 `status`：事务内条件 UPDATE 的前置值；缺列时为 None。
    pub(in crate::modules::session) target_status_before: Option<String>,
    pub(in crate::modules::session) reason: String,
}

impl SyncWritePlan {
    pub(in crate::modules::session) fn source_records(&self) -> usize {
        self.source_snapshot.normalized.record_count
    }
}

/// 单条同步选择的校验结果。
pub(in crate::modules::session) enum SyncItemOutcome {
    /// 全部前置条件通过，可进入写入阶段（计划体较大，装箱避免枚举体积失衡）。
    Validated {
        mode: SyncMode,
        verdict: SyncVerdict,
        plan: Box<SyncWritePlan>,
    },
    /// 版本变化/前置条件不满足：跳过该项，不沿用用户旧选择。
    Skipped {
        reason: &'static str,
        message: String,
        verdict: Option<SyncVerdict>,
    },
    /// 入参或凭据非法、模式越权：拒绝，不静默执行。
    Rejected { message: String },
}

/// 重新校验单条选择：凭据、身份、成员、基线、正文逐项核对（design §5.2）。
pub(in crate::modules::session) fn plan_sync_selection(
    context: &SyncContext,
    store: Option<&LinkStore>,
    selection: &SyncSelection,
) -> SyncItemOutcome {
    let paths = context.paths;
    let source_paths = context.source_paths;
    let (source_uid, target_uid) = (context.source_uid, context.target_uid);
    // 凭据必须是我们服务端保存过的：伪造的 id 读不到，直接拒绝。
    let Some(token) = session_link::load_preview_token(paths, &selection.preview_token) else {
        return SyncItemOutcome::Rejected {
            message: "检查结果不存在或已失效，请重新检查后再操作".to_string(),
        };
    };
    let binding = &token.binding;
    // 组级 variant 是创建档位，不作为过滤（跨档组两方向都可同步）；
    // 身份由凭据的组成员绑定与指纹校验（verify_preview）逐项兜底。
    if binding.group_id != selection.group_id {
        return SyncItemOutcome::Rejected {
            message: "检查结果与所选会话不匹配，已拒绝".to_string(),
        };
    }
    let skip = |message: String| SyncItemOutcome::Skipped {
        reason: REASON_PREVIEW_STALE,
        message,
        verdict: Some(binding.verdict),
    };
    // 来源身份取登录态、目标身份取入参：与预览不一致说明账号已经变了。
    if binding.source.uid != source_uid || binding.target.uid != target_uid {
        return skip("账号已变化，检查结果已失效".to_string());
    }
    let Some(store) = store else {
        return skip("同步记录不存在或不可用，检查结果已失效".to_string());
    };
    // 组查找只按 id：跨档组的组级 variant 是创建档位，不作为身份过滤；
    // 两侧成员身份由 active_member_for 与凭据中的成员绑定逐项校验。
    let Some(group) = store
        .groups
        .iter()
        .find(|group| group.id == selection.group_id)
    else {
        return skip("会话的关联关系已不存在，检查结果已失效".to_string());
    };
    let (Some(source_member), Some(target_member)) = (
        session_link::active_member_for(group, source_uid),
        session_link::active_member_for(group, target_uid),
    ) else {
        return skip("对应的会话已失效，检查结果已失效".to_string());
    };
    // 会话行缺失/归属异常：成员实际已失效（与预览的判定口径一致），提前拦下不写。
    // 源行在源档读、目标行在目标档读（跨档时不同数据库）。
    if !member_row_owned_by(source_paths, source_member)
        || !member_row_owned_by(paths, target_member)
    {
        return skip("会话记录缺失或归属异常，检查结果已失效".to_string());
    }
    // 重新加载正文、基线与判定，再与凭据逐项核对；任一变化都跳过（含显式覆盖）。
    let source_content = member_content_state(source_paths, &source_member.session_id);
    let target_content = member_content_state(paths, &target_member.session_id);
    let baseline = session_link::load_pair_baseline(
        paths,
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
        &preview_archive_state(
            source_paths,
            &source_member.session_id,
            paths,
            &target_member.session_id,
        ),
    );
    let mismatches = session_link::verify_preview(&token, &live);
    if !mismatches.is_empty() {
        // 这里是 issue #3 的真实路径：预览时正文完好、之后正文损坏，凭据核对会先命中
        // （它排在模式门禁之前），于是用户只看得到「内容已变化」，看不到「坏在第几行」。
        // 至少一侧不可验证时把当前内容状态补在后面；两侧都 Ready 只是内容变多变少时不补，
        // 那种情况没有「哪一侧坏了」可讲，多说反而误导。
        let sides = unverifiable_sides(&source_content, &target_content);
        let detail = sides
            .map(|sides| format!("（当前内容状态：{sides}）"))
            .unwrap_or_default();
        return skip(format!("检查结果已失效：{}{detail}", mismatches.join("；")));
    }
    // 本次是否随写入把目标对齐为归档：凭据里授权过才允许，且必须在本次重新核验。
    let archive_authorized = binding.archive_action.as_deref() == Some(ARCHIVE_ACTION_STATUS_ONLY);
    // 判定已按当前内容重算：mode 必须仍然成立，unknown 不得被覆盖绕过。
    // `StatusOnly` 不属于任何正文判定（`allows` 恒为 false），走独立的归档资格门禁。
    if selection.mode == SyncMode::StatusOnly {
        if !archive_authorized {
            return SyncItemOutcome::Rejected {
                message: "该组本次不提供仅同步归档的操作，已拒绝".to_string(),
            };
        }
        if !matches!(
            decision.verdict,
            SyncVerdict::Identical | SyncVerdict::Ahead
        ) {
            return SyncItemOutcome::Rejected {
                message: format!("该组判定为 {}，不允许仅同步归档", decision.verdict.as_str()),
            };
        }
    } else if !decision.verdict.allows(selection.mode) {
        // 前缀逐字保留（前端提示与既有测试按 `该组判定为 X，不允许以 Y 模式同步` 匹配），
        // 只在后面补上判定自己的原因：`decide_sync` 已经算出了「两边各改了什么」「哪一侧
        // 内容无法确认、坏在第几行」这些可读事实，丢掉它们用户只剩一句没有信息量的话，
        // 只能反复回报同一个 issue（issue #3 三轮仍未定位）。
        return SyncItemOutcome::Rejected {
            message: format!(
                "该组判定为 {}，不允许以 {} 模式同步：{}",
                decision.verdict.as_str(),
                selection.mode.as_str(),
                decision.reason
            ),
        };
    }
    // 判定非 unknown 必然双方正文可验证（decide_sync 的前置条件），这里仍然显式兜底。
    // 兜底文案同样要带上「哪一侧、为什么」：正文末行半截、成员行缺失、预览过期是不同的
    // 故障，用户与排障者必须能从这句话里分辨出来。
    let (ContentState::Ready(source_snapshot), ContentState::Ready(target_snapshot)) =
        (&source_content, &target_content)
    else {
        // 两侧都不可验证时才走到这里；真出现「两侧都可验证」的误用，也不能编造原因。
        return skip(match unverifiable_sides(&source_content, &target_content) {
            Some(sides) => format!("双方内容不可验证，检查结果已失效（{sides}）"),
            None => "双方内容不可验证，检查结果已失效".to_string(),
        });
    };
    // 归档资格在写入前重新核验（来源仍已归档、目标仍在终态白名单）；任一项不满足
    // 整体判为预览失效，不静默执行另一组副作用。
    let target_status_before = if archive_authorized {
        let source_status = match read_member_status(source_paths, &source_member.session_id) {
            Ok(status) => status,
            Err(error) => return SyncItemOutcome::Rejected { message: error },
        };
        let target_status = match read_member_status(paths, &target_member.session_id) {
            Ok(status) => status,
            Err(error) => return SyncItemOutcome::Rejected { message: error },
        };
        if !archive_qualifies(&source_status, &target_status) {
            return skip("会话的归档状态已变化，检查结果已失效".to_string());
        }
        match target_status {
            SessionStatus::Present(status) => status,
            _ => return skip("会话的归档状态已变化，检查结果已失效".to_string()),
        }
    } else {
        None
    };
    // 正文模式：目标正文 = 来源正文，只把本副本 sessionId 换成目标 sessionId。
    // 状态模式：正文零写入，待写入内容就是目标自己的快照（恢复据此核验「正文没被改」）。
    let (incoming_text, incoming) = if selection.mode == SyncMode::StatusOnly {
        (
            target_snapshot.text.clone(),
            target_snapshot.normalized.clone(),
        )
    } else {
        let mut incoming_text = source_snapshot
            .text
            .replace(&source_member.session_id, &target_member.session_id);
        // 来源会话已经结束，却把最后一条 assistant 消息留在 `incomplete`——这正是
        // 「副本永久思考中」的那个残缺，同步（覆盖与追加都走这条路）不能原样搬过去。
        // 来源仍在运行时按原字节搬运（见 [`source_settled_for_normalize`]）。
        //
        // 归一放在**这里**而不是写入阶段：下面紧接着按同一份 `incoming_text` 重算
        // `incoming`，于是备份清单的原始/归一化摘要、`expected_content_digest`、
        // 写后校验与配对基线全部自动落在归一后的内容上，不需要第二套基准。
        if source_settled_for_normalize(source_paths, &source_member.session_id) {
            incoming_text = normalize_body_tail(&incoming_text, NORMALIZE_REASON_SYNC);
        }
        let incoming =
            match session_link::normalize_jsonl(&incoming_text, &target_member.session_id) {
                Ok(normalized) => normalized,
                Err(reason) => {
                    return SyncItemOutcome::Rejected {
                        message: format!("目标内容无法按来源内容生成（{reason}），已拒绝"),
                    }
                }
            };
        (incoming_text, incoming)
    };
    SyncItemOutcome::Validated {
        mode: selection.mode,
        verdict: decision.verdict,
        plan: Box::new(SyncWritePlan {
            group_id: selection.group_id.clone(),
            mode: selection.mode,
            verdict: decision.verdict,
            source_member: source_member.clone(),
            target_member: target_member.clone(),
            source_snapshot: source_snapshot.clone(),
            target_snapshot: target_snapshot.clone(),
            incoming_text,
            incoming,
            old_baseline_ref: live.baseline_ref,
            archive_target: archive_authorized,
            target_status_before,
            reason: decision.reason,
        }),
    }
}

/// 同步上下文中不变的输入（避免逐项重复解析身份）。
///
/// 源 / 目标解耦：`paths` 恒为目标档（写侧），`source_paths` 恒为源档（读侧）；
/// 同档同步时两者相同，跨档同步时指向不同数据根。
pub(in crate::modules::session) struct SyncContext<'a> {
    pub(in crate::modules::session) paths: &'a SessionPaths,
    pub(in crate::modules::session) variant: WbVariant,
    pub(in crate::modules::session) source_paths: &'a SessionPaths,
    pub(in crate::modules::session) source_variant: WbVariant,
    pub(in crate::modules::session) source_uid: &'a str,
    pub(in crate::modules::session) source_account_id: Option<String>,
    pub(in crate::modules::session) target_uid: &'a str,
    pub(in crate::modules::session) target_account_id: Option<String>,
    /// 本次进入写入前仍未完成的操作（同一目标上不得再写一次）。
    pub(in crate::modules::session) pending: &'a [Operation],
}
