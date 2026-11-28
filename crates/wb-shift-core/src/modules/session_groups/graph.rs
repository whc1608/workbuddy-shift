//! 组状态图：共同基线/独立分支识别、成员状态归约与统一同步批次编排。

use super::*;

use std::sync::OnceLock;

/// 列表装配用的正文快照缓存：**不含正文原文**，只存摘要/预览/指纹。
///
/// 列表要读 500+ 份 MB 级正文，全文缓存实测要 3.4GB 内存（不可接受）；
/// 但每次重读 + 逐行 SHA256 又让列表耗时 34 秒。这里只缓存从正文派生的小结果
/// （两条摘要、预览、逐行指纹、时间戳），单条通常几十 KB，500 条约几十 MB。
/// 键含 path + len + mtime，客户端改写正文后自然失效。
const LIST_SNAPSHOT_CACHE_CAPACITY: usize = 768;

fn list_snapshot_cache() -> &'static session_link::BoundedCache<
    session_link::ContentStamp,
    crate::modules::session_link::ContentState,
> {
    static CACHE: OnceLock<
        session_link::BoundedCache<
            session_link::ContentStamp,
            crate::modules::session_link::ContentState,
        >,
    > = OnceLock::new();
    CACHE.get_or_init(|| session_link::BoundedCache::with_capacity(LIST_SNAPSHOT_CACHE_CAPACITY))
}

/// 列表专用正文读取：命中缓存时返回一个 `text` 为空、但 `list_facts` 与逐行指纹齐备的
/// 快照；未命中走正常读取，算好 facts 后把「去掉 text」的版本放进缓存。
///
/// 写路径（复制 / 同步）**不调用本函数**，仍用 `read_content_snapshot` 读全文，
/// 因此缓存里没有文本不会被写回；列表与详情只消费 facts，不受影响。
pub(super) fn read_list_snapshot(
    path: &std::path::Path,
    own_session_id: &str,
) -> session_link::ContentState {
    use crate::modules::session_link::{ContentSnapshot, ContentState, ListFacts};
    use std::sync::Arc;

    let stamp = session_link::stamp_content(path, own_session_id);
    if let Some(hit) = list_snapshot_cache().get(&stamp) {
        return hit;
    }
    let state = session_link::read_content_snapshot(path, own_session_id);
    let ContentState::Ready(snapshot) = &state else {
        // Missing / Unavailable 不缓存（文件可能马上出现或修复）。
        return state;
    };
    let facts = ListFacts {
        excerpt: excerpt_of(&state),
        preview: content_preview(&state),
        record_count: content_count_view(&state),
    };
    let cached = ContentState::Ready(ContentSnapshot {
        text: String::new(), // 列表/详情只消费 facts，写路径不走这里
        full_digest: snapshot.full_digest.clone(),
        normalized: snapshot.normalized.clone(),
        latest_at: snapshot.latest_at,
        list_facts: Some(Arc::new(facts)),
    });
    list_snapshot_cache().insert(stamp, cached.clone());
    cached
}

/// 记录数（与 view.rs 的 `content_count` 同口径，避免跨模块循环引用）。
fn content_count_view(content: &session_link::ContentState) -> Option<usize> {
    match content {
        session_link::ContentState::Ready(snapshot) => Some(snapshot.normalized.record_count),
        _ => None,
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupUnifyTarget {
    pub target_member_id: String,
    pub preview_token: String,
    pub mode: String,
}

/// Apply an explicit group choice in one lifecycle window. WorkBuddy closes only a running
/// variant whose current login is a write target; the VS Code plugin closes the editor when the
/// caller was authorized (`restart`). Only clients closed here are reopened. Pair kernels still
/// validate every preview token.
pub fn sync_unify_batch(
    client: SessionClient,
    scope: Option<WbVariant>,
    group_id: &str,
    source_member_id: &str,
    targets: &[GroupUnifyTarget],
    restart: bool,
) -> Result<Value, String> {
    match client {
        SessionClient::Workbuddy => {
            sync_unify_batch_workbuddy(scope, group_id, source_member_id, targets)
        }
        SessionClient::VscodeExt => {
            sync_unify_batch_editor(client, scope, group_id, source_member_id, targets, restart)
        }
        SessionClient::Jetbrains => {
            sync_unify_batch_editor(client, scope, group_id, source_member_id, targets, restart)
        }
        SessionClient::CodebuddyIde => {
            Err("仅 WorkBuddy 与插件类会话（VS Code / JetBrains）支持整组统一".to_string())
        }
    }
}

/// Resolve + validate every target before anything is closed or written.
pub(super) fn prepare_unify_targets(
    client: SessionClient,
    scope: Option<WbVariant>,
    group_id: &str,
    source_member_id: &str,
    targets: &[GroupUnifyTarget],
) -> Result<Vec<(PairContext, SyncSelection)>, String> {
    if targets.is_empty() {
        return Err("没有需要同步的目标副本".to_string());
    }
    let mut seen = HashSet::new();
    let mut prepared = Vec::with_capacity(targets.len());
    for target in targets {
        if !seen.insert(target.target_member_id.as_str()) {
            return Err("目标副本重复，请重新检查会话".to_string());
        }
        let pair = resolve_pair(
            client,
            scope,
            group_id,
            source_member_id,
            &target.target_member_id,
        )?;
        let selection = parse_selection(group_id, &target.preview_token, &target.mode)?;
        prepared.push((pair, selection));
    }
    Ok(prepared)
}

/// WorkBuddy：只关闭「运行中且当前登录是写入目标」的档位，只重开本次关闭的档位。
pub(super) fn sync_unify_batch_workbuddy(
    scope: Option<WbVariant>,
    group_id: &str,
    source_member_id: &str,
    targets: &[GroupUnifyTarget],
) -> Result<Value, String> {
    let client = SessionClient::Workbuddy;
    let prepared = prepare_unify_targets(client, scope, group_id, source_member_id, targets)?;

    let restart_variants = WbVariant::ALL
        .into_iter()
        .filter(|variant| {
            prepared.iter().any(|(pair, _)| {
                account::variant_of(&pair.target_account) == *variant
                    && running_target_needs_restart(*variant, &pair.target.uid)
            })
        })
        .collect::<Vec<_>>();
    let mut closed = Vec::new();
    for variant in restart_variants {
        if let Err(error) = process::close_workbuddy(variant, 20) {
            let reopen_errors = reopen_workbuddy_variants(&closed);
            let suffix = if reopen_errors.is_empty() {
                String::new()
            } else {
                format!(
                    "；此前关闭的客户端重新打开失败：{}",
                    reopen_errors.join("；")
                )
            };
            return Err(format!(
                "关闭 {}失败：{error}{suffix}",
                workbuddy_variant_label(variant)
            ));
        }
        closed.push(variant);
    }

    let mut combined = json!({ "client": client.as_str(), "groupId": group_id, "synced": [], "skipped": [], "errors": [], "needsRecovery": false, "temporaryFiles": [], "restartedVariants": [] });
    for (pair, selection) in prepared {
        match run_pair_sync(client, scope, &pair, &[selection], false) {
            Ok(report) => {
                merge_sync_report(&mut combined, report);
                if combined["needsRecovery"] == true {
                    break;
                }
            }
            Err(error) => {
                combined["errors"].as_array_mut().unwrap().push(json!({
                    "error": format!("{}：{error}", pair.target.uid),
                }));
                break;
            }
        }
    }

    if combined["needsRecovery"] == true {
        combined["errors"].as_array_mut().unwrap().push(json!({
            "error": "会话写入待恢复，已暂停重新打开客户端；请先处理恢复提示",
        }));
        return Ok(combined);
    }
    for variant in &closed {
        match session::recover_pending_session_operations(*variant) {
            Ok(recovery) if !crate::modules::switch::recovery_blocks_startup(&recovery) => {}
            Ok(recovery) => {
                combined["needsRecovery"] = json!(true);
                combined["errors"].as_array_mut().unwrap().push(json!({
                    "error": format!("{}仍有待恢复的会话写入（{}），已暂停重新打开客户端", workbuddy_variant_label(*variant), crate::modules::switch::recovery_blocking_detail(&recovery)),
                }));
            }
            Err(error) => {
                combined["needsRecovery"] = json!(true);
                combined["errors"].as_array_mut().unwrap().push(json!({
                    "error": format!("{}无法检查会话恢复状态（{error}），已暂停重新打开客户端", workbuddy_variant_label(*variant)),
                }));
            }
        }
    }
    if combined["needsRecovery"] == true {
        return Ok(combined);
    }
    let mut reopened = Vec::new();
    for variant in closed {
        if let Err(error) = process::launch_workbuddy(variant, None) {
            combined["errors"].as_array_mut().unwrap().push(json!({
                "error": format!("{}会话已处理，但客户端重新打开失败：{error}", workbuddy_variant_label(variant)),
            }));
        } else {
            reopened.push(variant.as_str());
        }
    }
    combined["restartedVariants"] = json!(reopened);
    Ok(combined)
}

/// 编辑器插件（VS Code / JetBrains）：全部校验通过后打开宿主生命周期窗口（`restart=true`
/// 时关闭），窗口内跑完全部目标再收尾重开；插件侧没有 WorkBuddy 的 `needsRecovery` 语义。
///
/// 两个宿主的差异全部收敛在 [`prepare_editor_window`] 里（怎么关、关什么），这里只负责
/// 批次编排，因此不需要第二份实现。
pub(super) fn sync_unify_batch_editor(
    client: SessionClient,
    scope: Option<WbVariant>,
    group_id: &str,
    source_member_id: &str,
    targets: &[GroupUnifyTarget],
    restart: bool,
) -> Result<Value, String> {
    let prepared = prepare_unify_targets(client, scope, group_id, source_member_id, targets)?;
    let target_uids: Vec<&str> = prepared
        .iter()
        .map(|(pair, _)| pair.target.uid.as_str())
        .collect();
    let guard = prepare_editor_window(client, restart, false, &target_uids)?;
    let editor_prepared = guard.is_some();
    let mut combined = json!({ "client": client.as_str(), "groupId": group_id, "synced": [], "skipped": [], "errors": [], "needsRecovery": false, "temporaryFiles": [] });
    for (pair, selection) in prepared {
        match run_pair_sync(client, scope, &pair, &[selection], editor_prepared) {
            Ok(report) => merge_sync_report(&mut combined, report),
            Err(error) => {
                combined["errors"].as_array_mut().unwrap().push(json!({
                    "error": format!("{}：{error}", pair.target.uid),
                }));
                break;
            }
        }
    }
    settle_editor_window(guard, Ok(combined))
}

/// Recompute the group source on the server and sync only targets proven fast-forward safe.
pub fn sync_safe_batch(
    client: SessionClient,
    scope: Option<WbVariant>,
    group_id: &str,
    restart: bool,
) -> Result<Value, String> {
    let detail = detail(client, scope, group_id)?;
    let source_member_id = detail
        .get("safeSourceMemberId")
        .and_then(Value::as_str)
        .ok_or_else(|| "该组无法确认安全同步来源".to_string())?
        .to_string();
    let targets = detail
        .get("members")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|member| {
            member.get("memberId").and_then(Value::as_str) != Some(source_member_id.as_str())
                && member.get("versionStatus").and_then(Value::as_str) == Some("behind")
        })
        .filter_map(|member| {
            member
                .get("memberId")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect::<Vec<_>>();
    let mut combined = json!({ "client": client.as_str(), "groupId": group_id, "synced": [], "skipped": [], "errors": [], "needsRecovery": false, "temporaryFiles": [] });
    // 准备阶段：全部目标的复核与凭据都在关闭编辑器之前完成（复核失败/不再安全快进的项照旧跳过），
    // 避免为注定失败的请求打断用户。
    let mut prepared = Vec::new();
    for target_member_id in targets {
        let pair = resolve_pair(
            client,
            scope,
            group_id,
            &source_member_id,
            &target_member_id,
        )?;
        let preview = preview_resolved_pair(client, scope, &pair)?;
        if preview.get("verdict").and_then(Value::as_str) != Some("fastForward") {
            combined["skipped"].as_array_mut().unwrap().push(json!({
                "groupId": group_id,
                "targetMemberId": target_member_id,
                "reasonCode": "recheckNotFastForward",
                "message": "重新检查后该副本不再属于安全快进范围",
            }));
            continue;
        }
        let token = preview
            .get("previewToken")
            .and_then(Value::as_str)
            .ok_or_else(|| "重新预览未返回可执行凭据".to_string())?;
        let selection = parse_selection(group_id, token, "fastForward")?;
        prepared.push((pair, selection));
    }
    let target_uids: Vec<&str> = prepared
        .iter()
        .map(|(pair, _)| pair.target.uid.as_str())
        .collect();
    let guard = prepare_editor_window(client, restart, false, &target_uids)?;
    let editor_prepared = guard.is_some();
    let result = (|| -> Result<Value, String> {
        for (pair, selection) in prepared {
            let report = run_pair_sync(client, scope, &pair, &[selection], editor_prepared)?;
            merge_sync_report(&mut combined, report);
        }
        Ok(combined)
    })();
    settle_editor_window(guard, result)
}

/// Recognize one present shared snapshot followed by independent append-only branches.
/// This uses normalized ordered content, so timestamps and record counts cannot invent ancestry.
/// A more complex history stays in the generic divergence state.
///
/// 摘要取的是**比较口径**（`comparable_line_digests`，与 [`session_link::decide_sync`] 同一套）：
/// 复制与同步写目标时会把末条 assistant 的 `incomplete` 归一成 `completed` 并留痕，那行因此在
/// 原始摘要上与来源不同，但它不是「用户改动」。若这里仍比原始摘要，三成员组（老副本 + 来源
/// `incomplete` 尾 + 归一后的新副本）会被判成「独立分支」并强行把组徽标改成 `diverge`，与同一
/// 组两两 `decide_sync` 得出的 `behind`（可安全同步）自相矛盾——issue #3 最后一条。
#[derive(Debug, PartialEq, Eq)]
pub(super) struct CommonBaseBranches {
    pub(super) common: Vec<usize>,
    pub(super) branches: Vec<Vec<usize>>,
}

pub(super) fn common_base_branches(
    views: &[MemberView],
    active: &[usize],
) -> Option<CommonBaseBranches> {
    if active.len() < 3 || active.len() != views.len() {
        return None;
    }
    let lines = |index: usize| match &views[index].content {
        ContentState::Ready(snapshot) => {
            Some(snapshot.normalized.comparable_line_digests.as_slice())
        }
        _ => None,
    };
    let mut versions: Vec<Vec<usize>> = Vec::new();
    for &index in active {
        let content = lines(index)?;
        if let Some(version) = versions
            .iter_mut()
            .find(|version| lines(version[0]) == Some(content))
        {
            version.push(index);
        } else {
            versions.push(vec![index]);
        }
    }
    if versions.len() < 3 {
        return None;
    }
    let common_index = versions.iter().position(|version| {
        let common = lines(version[0]).expect("active members have readable content");
        versions
            .iter()
            .filter(|other| *other != version)
            .all(|other| {
                let branch = lines(other[0]).expect("active members have readable content");
                branch.len() > common.len() && branch.starts_with(common)
            })
    })?;
    let common = versions.remove(common_index);
    // Each remaining version must be independently advanced from the shared snapshot.
    if versions.iter().enumerate().any(|(index, branch)| {
        versions.iter().skip(index + 1).any(|other| {
            let left = lines(branch[0]).expect("active members have readable content");
            let right = lines(other[0]).expect("active members have readable content");
            left.starts_with(right) || right.starts_with(left)
        })
    }) {
        return None;
    }
    Some(CommonBaseBranches {
        common,
        branches: versions,
    })
}

/// Reduce pairwise decisions into stable member states and a group summary.
///
/// `views` are already ordered by account key, UID, and member ID. The reducer is deliberately
/// independent of storage and process state so the conservative source-selection contract can be
/// covered with deterministic unit tests.
pub(super) fn aggregate_group_state(
    views: &mut [MemberView],
    active: &[usize],
    matrix: &HashMap<(usize, usize), (SyncVerdict, String)>,
) -> (Option<usize>, &'static str) {
    let mut safe_candidates = active
        .iter()
        .copied()
        .filter(|&candidate| {
            if !matches!(&views[candidate].content, ContentState::Ready(_)) {
                return false;
            }
            active
                .iter()
                .copied()
                .filter(|&other| other != candidate)
                .all(|other| {
                    matrix.get(&(candidate, other)).is_some_and(|(verdict, _)| {
                        matches!(verdict, SyncVerdict::Identical | SyncVerdict::FastForward)
                    })
                })
        })
        .collect::<Vec<_>>();
    safe_candidates.sort_by(|&left, &right| {
        views[left]
            .account_key
            .cmp(&views[right].account_key)
            .then_with(|| views[left].member.uid.cmp(&views[right].member.uid))
            .then_with(|| {
                views[left]
                    .member
                    .member_id
                    .cmp(&views[right].member.member_id)
            })
    });
    let safe_source = safe_candidates.first().copied();

    let mut has_behind = false;
    let mut has_diverge = false;
    let mut has_missing = false;
    for (index, view) in views.iter_mut().enumerate() {
        match view.member.state {
            MemberState::Stale => {
                view.version_status = "stale";
                view.reason = "该副本已标记为失效".to_string();
            }
            MemberState::Superseded => {
                view.version_status = "superseded";
                view.reason = "该副本已被更新成员替代".to_string();
            }
            MemberState::Active => match &view.content {
                ContentState::Missing => {
                    view.version_status = "missing";
                    view.reason = "会话内容不存在，无法同步".to_string();
                    has_missing = true;
                }
                ContentState::Unavailable(reason) => {
                    view.version_status = "unknown";
                    view.reason = reason.clone();
                }
                ContentState::Ready(_) => {
                    if let Some(source_index) = safe_source {
                        if index == source_index
                            || matrix
                                .get(&(source_index, index))
                                .is_some_and(|(verdict, _)| *verdict == SyncVerdict::Identical)
                        {
                            view.version_status = "latest";
                            if index != source_index {
                                view.reason = "内容与操作来源一致".to_string();
                            }
                        } else if let Some((SyncVerdict::FastForward, reason)) =
                            matrix.get(&(source_index, index))
                        {
                            view.version_status = "behind";
                            view.reason = reason.clone();
                            has_behind = true;
                        } else {
                            view.version_status = "unknown";
                            view.reason = "重新检查后无法确认此成员状态".to_string();
                        }
                    } else {
                        let reasons = active
                            .iter()
                            .filter_map(|other| matrix.get(&(*other, index)))
                            .collect::<Vec<_>>();
                        if reasons
                            .iter()
                            .any(|(verdict, _)| *verdict == SyncVerdict::Diverge)
                        {
                            view.version_status = "diverge";
                            view.reason = reasons
                                .iter()
                                .find(|(verdict, _)| *verdict == SyncVerdict::Diverge)
                                .map(|(_, reason)| reason.clone())
                                .unwrap_or_default();
                            has_diverge = true;
                        } else {
                            view.version_status = "unknown";
                            view.reason = reasons
                                .first()
                                .map(|(_, reason)| reason.clone())
                                .unwrap_or_else(|| "缺少可比较的关联成员".to_string());
                        }
                    }
                }
            },
        }
    }

    let summary_status = if safe_source.is_some() {
        if has_behind {
            "behind"
        } else {
            "latest"
        }
    } else if has_diverge {
        "diverge"
    } else if has_missing {
        "missing"
    } else {
        // 无任何可判定的状态标记时同样回退为「无法确认」（原 `has_unknown` 分支与 else 同体）。
        "unknown"
    };

    (safe_source, summary_status)
}

/// 记录的角色标签：`user` → 用户、`assistant` → 助手，其余（tool / event / 缺字段）→ 记录。
///
/// 单一来源：详情预览（[`content_preview`]）与卡片摘要（[`excerpt_of`]）必须用同一套口径，
/// 否则同一条消息在两处会被标成不同的说话人。
fn speaker_label(record: &Value) -> &'static str {
    match record
        .get("role")
        .or_else(|| record.get("type"))
        .and_then(Value::as_str)
    {
        Some("user") => "用户",
        Some("assistant") => "助手",
        _ => "记录",
    }
}

/// 详情里「最近 N 条可读内容」的每条文本上限（字符数）。
///
/// 与列表摘要不同：详情是用户主动展开全文的入口，保留更长的上下文（原来的 240 字 → 480），
/// 但**必须**和摘要走同一套注入块清理（[`excerpt_body`]）——否则卡片上是一句人话，
/// 展开后却是 `<system-reminder>...` 原文，用户会以为工具坏了。
///
/// 240 → 480 的理由：模型回复里的 Markdown 表格（表头 + 分隔行 + 2~3 行数据）在 240 字内
/// 必被切断，前端只能把半张表渲染成纯文本；详情是用户主动展开的入口，给到 480 才看得全
/// 一张小表。列表摘要走的是另一套 [`EXCERPT_CHARS`]（120），不受这里影响，载荷不会变大。
const PREVIEW_CHARS: usize = 480;

pub(super) fn content_preview(content: &ContentState) -> Vec<Value> {
    let ContentState::Ready(snapshot) = content else {
        return Vec::new();
    };
    // 列表缓存命中时 text 为空，直接返回首次读取时算好的预览。
    if let Some(facts) = &snapshot.list_facts {
        return facts.preview.clone();
    }
    let mut recent = snapshot
        .text
        .lines()
        .rev()
        .filter_map(|line| {
            let record: Value = serde_json::from_str(line).ok()?;
            let raw = record_text(&record)?;
            // 同一套清理链：剥注入块 → 丢中间/末尾注入 span → 拆包装标签 → 兜底切残标签。
            // 无关的普通正文（含用户手打的尖括号）原样保留，判据见 excerpt_body。
            let text = excerpt_body(raw.trim());
            let text = text.trim();
            if text.is_empty() || is_tool_payload(text) {
                // 整条只有注入块 / 工具载荷（如 `<task-notification>`、`<teammate-message>`），
                // 展示出来就是一坨标签。直接跳过，继续往前找真正可读的消息。
                return None;
            }
            Some(json!({
                "speaker": speaker_label(&record),
                "text": text.chars().take(PREVIEW_CHARS).collect::<String>(),
            }))
        })
        .take(6)
        .collect::<Vec<_>>();
    recent.reverse();
    recent
}

/// 卡片摘要的截断长度（字符数）：约两行中文，够看出「这条会话聊了什么」。
const EXCERPT_CHARS: usize = 120;

/// 只对**摘要**生效的注入块清理：把产品自己拼进用户消息的上下文块剥掉。
///
/// 真实正文里，用户消息经常长这样（同一条记录内，注入块在前、用户真正说的话在最后）：
///
/// ```text
/// <system-reminder data-role="user-context">
/// <craft_mode>
/// You are now in Agent mode. Continue with the task in the new mode.
/// </craft_mode>
/// </system-reminder>
/// 真正的问题
/// ```
///
/// 原样取前 120 字得到的是一段 `system-reminder` 标签，卡片上等于没有摘要。因此摘要路径
/// 先剥注入块、再判空、再截断（顺序不能反：先截断会把闭合标签切掉，变成永远剥不干净）。
///
/// **只给摘要用**：详情预览（[`content_preview`]）是既有行为，保持原样输出。
///
/// `pub(super)` 是为了让 `tests` 子模块直接断言边界（不改可见性到 crate 外）。
pub(super) fn strip_injected_blocks(text: &str) -> &str {
    let mut rest = text;
    loop {
        let trimmed = rest.trim_start();
        let Some(tag) = leading_tag(trimmed) else {
            return trimmed;
        };
        // 保守判据：只认白名单里的注入标签（[`is_injected_tag`]）。
        // `<div>` / `<T>` / `a < b` 之类一律当正文保留。
        if !is_injected_tag(tag.name) {
            return trimmed;
        }
        // 落单的闭合标签（同名标签嵌套后的残留）：丢掉，不能当正文。
        if tag.closing {
            rest = &trimmed[tag.end..];
            continue;
        }
        match closing_tag_end(trimmed, tag.name) {
            Some(end) => {
                rest = &trimmed[end..];
            }
            // 已知注入标签（`system-reminder` / 队友通知 / 命令回显…）未闭合到文本末尾：
            // 整条记录都是注入，没有正文可留。
            None if is_pure_injection_tag(tag.name) => return "",
            // 其它 `*_mode` 找不到配对闭合标签：保守起见原样保留（可能是用户手打的文字）。
            None => return trimmed,
        }
    }
}

/// 这些标签包裹的是上下文注入 / 工具元信息，不是用户说的话：整块丢弃。
///
/// 判据只看标签名，大小写不敏感（`<Teammate-Message …>` 同样命中），属性随便带。
fn is_injected_tag(name: &str) -> bool {
    name.eq_ignore_ascii_case("system-reminder")
        || is_tool_injection_tag(name)
        || has_mode_suffix(name)
}

/// 工具注入的包装标签：内容是工具拼进来的元信息（队友通知、斜杠命令回显、命令输出等），
/// 用户从来没打过这些字，摘要里必须整块丢掉。
///
/// 现场残留（真实库，卡片上显示成一串带属性的标签）：
/// `<teammate-message teammate_id="system" summary="… completed: 摘要去掉包装标签">…</teammate-message>`
const TOOL_INJECTION_TAGS: [&str; 9] = [
    "teammate-message",
    "command-name",
    "command-message",
    "local-command-stdout",
    "local-command-stderr",
    "user-context",
    "additional-data",
    // 附件路径块：用户发图时客户端拼进来的本地路径，卡片上显示成 `C:\Users\…\x.png`。
    // 它和正文在**同一条记录**里（`<user_query>正文</user_query> <image_local_path>…`），
    // 只拆包装剥不掉，必须当注入块整块丢掉。
    "image_local_path",
    "image_path",
];

/// 是否工具注入的包装标签（大小写不敏感）。
fn is_tool_injection_tag(name: &str) -> bool {
    TOOL_INJECTION_TAGS
        .iter()
        .any(|tag| name.eq_ignore_ascii_case(tag))
}

/// 未找到配对闭合标签时可以直接判成「整条都是注入」的标签。
///
/// `*_mode` 不算在内：用户可能真手打了 `<craft_mode>` 却没闭合，宁可留着也不吃掉正文。
fn is_pure_injection_tag(name: &str) -> bool {
    name.eq_ignore_ascii_case("system-reminder") || is_tool_injection_tag(name)
}

/// 标签名是否以 `_mode` 结尾（大小写不敏感；不分配内存）。
fn has_mode_suffix(name: &str) -> bool {
    const SUFFIX: &str = "_mode";
    name.get(name.len().saturating_sub(SUFFIX.len())..)
        .is_some_and(|tail| tail.eq_ignore_ascii_case(SUFFIX))
}

/// 文本开头的一个标签：`<name …>` 或 `</name>`。
struct LeadingTag<'a> {
    name: &'a str,
    closing: bool,
    /// 标签结束（`>` 之后）的字节下标。
    end: usize,
}

/// 解析文本**开头**的标签；不是标签（或不像合法标签名）时返回 `None`。
fn leading_tag(text: &str) -> Option<LeadingTag<'_>> {
    let inner = text.strip_prefix('<')?;
    let gt = inner.find('>')?;
    let head = &inner[..gt];
    // `<>` / `<x/>`（自闭合，不包裹内容）不当作区块。
    if head.is_empty() || head.ends_with('/') {
        return None;
    }
    let closing = head.starts_with('/');
    let head = head.trim_start_matches('/').trim_start();
    // 属性（`data-role="…"`）前的第一个空白之前就是标签名。
    let name = head.split_whitespace().next().unwrap_or_default();
    // 标签名只允许字母 / 数字 / `-` / `_`：`<3 天` 这类普通文字不当标签。
    // 大写也认（注入标签的判定不区分大小写），名字本身的比较仍然按原样。
    let valid = !name.is_empty()
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_');
    if !valid {
        return None;
    }
    // `1`（`<`）+ `gt`（标签内长度）+ `1`（`>`）。
    Some(LeadingTag {
        name,
        closing,
        end: gt + 2,
    })
}

/// 找 `</name>`（允许 `</name >` 这种空白）并返回其**之后**的字节下标。
fn closing_tag_end(text: &str, name: &str) -> Option<usize> {
    let needle = format!("</{name}");
    let mut from = 0;
    while let Some(offset) = text[from..].find(&needle) {
        let start = from + offset;
        let after = &text[start + needle.len()..];
        if let Some(gt) = after.find('>') {
            // 闭合标签内部除空格外不该有别的东西，否则不是我们要找的那个。
            if after[..gt].chars().all(char::is_whitespace) {
                return Some(start + needle.len() + gt + 1);
            }
        }
        from = start + needle.len();
    }
    None
}

/// 外层包装标签最多剥几层：循环剥到「不再变化」为止，这里只是防死循环的上限
/// （`<a><b>x</b></a>` 两层就够，正常正文绝不会有四层包装）。
const WRAPPER_UNWRAP_DEPTH: usize = 4;

/// 摘要正文：按固定顺序做五步文本清理，列表摘要与详情预览共用同一条链。
///
/// 1. [`strip_injected_blocks`]：干掉**开头**的注入块；
/// 2. [`drop_injected_spans`]：干掉**正文中间/末尾**的注入块（附件路径、队友通知等）；
/// 3. [`unwrap_wrapper_tags`] + [`unwrap_leading_element`]：拆掉包装标签、保留元素内外的正文；
/// 4. [`strip_image_placeholders`]：去掉产品拼进用户消息的 `@image#N:文件名` 引用占位符；
/// 5. [`strip_residual_leading_tag`]：兜底切掉仍残留的开标签。
///
/// 顺序固定：占位符必须在 `is_tool_payload` 判定**之前**剥掉，这样「只有图片、没有文字」的
/// 消息会自然变成空文本，被列表摘要（[`excerpt_of`]）与详情预览（[`content_preview`]）当成
/// 「没有正文」跳过。
fn excerpt_body(text: &str) -> String {
    let cleaned = drop_injected_spans(strip_injected_blocks(text));
    let unwrapped = unwrap_wrapper_tags(&cleaned);
    let element = unwrap_leading_element(unwrapped);
    let stripped = strip_image_placeholders(&element);
    strip_residual_leading_tag(&stripped).to_string()
}

/// 去掉产品拼进用户消息的 `@image#<数字>:<文件名>` 引用占位符，保留后面的正文。
///
/// 真实形态（同一条记录里连续两个占位符 + 用户真正说的话）：
///
/// ```text
/// @image#1:Clipboard_Screenshot.png @image#2:Clipboard_Screenshot-1.png 真正的正文
/// ```
///
/// 这些占位符是给模型看的图片引用记号，不是用户说的话；原样上卡片会显示成一串
/// `@image#N:…`，可读性极差。句中 / 句尾的占位符同样剥掉：`根据这张图 @image#1:x.png 帮我分析`
/// 只剩 `根据这张图 帮我分析`（占位符删除后留下的连续空格折叠成一个）。
///
/// 判据刻意保守，只认**完整**占位符：`@image#` 字面量 + 至少一位数字 + `:` + **以图片扩展名
/// 结尾**的文件名（见 [`image_filename_end`]）。扩展名锚定是关键：真实库里存在
/// `@image#1:这里再帮我调整一下` 这种「占位符前缀 + 中文正文」，它不是图片文件名，必须原样
/// 保留（有回归用例钉着）。文件名的搜索范围止于下一个 `@image#`，连续占位符不会混成一个
/// 超长文件名；文件名本身可含中文、空格、连字符、下划线与点。
///
/// 因此：
///
/// - 普通 `@某人` 提及、缺数字的 `@image#x`、缺冒号的 `@image#1`、无图片扩展名的
///   `@image#1:中文正文` 都不命中，原样保留；
/// - 只有占位符没有正文时返回空串，由调用方按「这条没有正文」处理；
/// - 本地图片路径块 `<image_local_path>…</image_local_path>` 由 [`drop_injected_spans`] 负责，
///   这里不碰。
fn strip_image_placeholders(text: &str) -> String {
    const MARKER: &str = "@image#";
    if !text.contains(MARKER) {
        return text.trim().to_string();
    }
    let mut without = String::with_capacity(text.len());
    let mut cursor = 0usize;
    while let Some(offset) = text[cursor..].find(MARKER) {
        let start = cursor + offset;
        // 占位符之前的正文原样保留。
        without.push_str(&text[cursor..start]);
        let after_marker = &text[start + MARKER.len()..];
        // 数字都是 ASCII，字符数即字节数，可直接用于切片。
        let digits_len = after_marker
            .bytes()
            .take_while(|byte| byte.is_ascii_digit())
            .count();
        if digits_len == 0 || !after_marker[digits_len..].starts_with(':') {
            // 不是合法占位符（`@image#x.png` / `@image#1 个`）：连 marker 一起原样保留。
            without.push_str(MARKER);
            cursor = start + MARKER.len();
            continue;
        }
        let filename = &after_marker[digits_len + 1..];
        let Some(filename_len) = image_filename_end(filename) else {
            // 没有图片扩展名（`@image#1:这里再帮我调整一下`）：是中文正文，原样保留到冒号。
            without.push_str(MARKER);
            without.push_str(&after_marker[..digits_len + 1]);
            cursor = start + MARKER.len() + digits_len + 1;
            continue;
        };
        // 跳过整个占位符：marker + 数字 + 冒号 + 文件名。空白后的正文留给下一轮扫描。
        cursor = start + MARKER.len() + digits_len + 1 + filename_len;
    }
    without.push_str(&text[cursor..]);

    // 连续占位符被删后会留下多余空格，折叠成一个（不动换行，避免破坏正文结构）。
    let mut out = String::with_capacity(without.len());
    let mut prev_space = false;
    for ch in without.trim().chars() {
        if ch == ' ' {
            if prev_space {
                continue;
            }
            prev_space = true;
        } else {
            prev_space = false;
        }
        out.push(ch);
    }
    out
}

/// 在 `@image#N:` 之后的部分里找图片文件名的结束字节位置；找不到返回 `None`。
///
/// 判据：文件名必须以常见图片扩展名结尾，且该扩展名之后是空白 / **闭合引号**
/// （ASCII / 弯引号 / 全角引号，见 [`is_closing_quote`]）/ 下一个 `@image#` / 字符串结尾。
/// 真实库里文件名会被引号包住（`@image#1:“黄岑果证明·.jpg” 这条里的数据是否正确？`），
/// 闭合引号要一并剥掉，所以它也算进文件名长度（按字符的真实 UTF-8 长度，弯引号是多字节）。
/// 搜索范围先截到下一个 `@image#`，避免把连续占位符混成一个超长文件名；文件名本身允许
/// 中文、空格、连字符、下划线与点（取最后一个合法扩展名，于是 `a.backup.png` 整体算文件名）。
/// 没有扩展名时返回 `None`——这正是 `@image#1:这里再帮我调整一下` 的中文正文被保留下来的原因。
fn image_filename_end(text: &str) -> Option<usize> {
    const IMAGE_EXTS: [&str; 12] = [
        "png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "tiff", "tif", "ico", "heic", "avif",
    ];
    let limit = text.find("@image#").unwrap_or(text.len());
    let scope = &text[..limit];
    let mut found = None;
    let mut from = 0usize;
    while let Some(rel) = scope[from..].find('.') {
        let dot = from + rel;
        let tail = &scope[dot + 1..];
        let ext_len = tail
            .bytes()
            .take_while(|byte| byte.is_ascii_alphabetic())
            .count();
        if ext_len > 0 {
            let ext = &tail[..ext_len];
            let ext_end = dot + 1 + ext_len;
            // 真实库里文件名可能带引号（ASCII / 弯引号 / 全角引号）：只认紧跟扩展名的那一个
            // 闭合引号，按字符真实 UTF-8 长度算进文件名一并剥掉；正文里的引号不会被吃掉。
            let quote_len = scope[ext_end..]
                .chars()
                .next()
                .filter(|ch| is_closing_quote(*ch))
                .map(char::len_utf8)
                .unwrap_or(0);
            let end = ext_end + quote_len;
            let at_boundary =
                end == scope.len() || scope[end..].chars().next().is_some_and(char::is_whitespace);
            if at_boundary
                && IMAGE_EXTS
                    .iter()
                    .any(|known| ext.eq_ignore_ascii_case(known))
            {
                // 取最后出现的合法扩展名：文件名里可能含多个点（`a.backup.png`）。
                found = Some(end);
            }
        }
        from = dot + 1;
    }
    found
}

/// 图片文件名可能的闭合引号：ASCII / 弯引号 / 全角引号。
///
/// 各端拼占位符时引号形态不统一（真实库里 ASCII 与弯引号都出现过），这里一并认。判据仍然
/// 保守：只认**紧跟图片扩展名**的那一个，且其后必须是空白 / 结尾（见 [`image_filename_end`]），
/// 所以正文里正常使用的引号不会被吃掉。
fn is_closing_quote(ch: char) -> bool {
    matches!(ch, '"' | '\'' | '“' | '”' | '‘' | '’' | '＂' | '＇')
}

/// 丢掉**任意位置**的注入块（含紧跟正文之后的附件路径块）。
///
/// [`strip_injected_blocks`] 只处理开头的连续注入块；真实库里更常见的是「正文 + 注入块」
/// 拼在同一条记录里：
///
/// ```text
/// <user_query>@image#1:…调整一下</user_query> <image_local_path>C:/tmp/a.png</image_local_path>
/// ```
///
/// 附件路径块在中间/末尾，不丢就会跟着上卡片。判据仍然只认白名单标签
/// （[`is_injected_tag`]）且必须找到配对的闭合标签——找不到就原样保留，绝不吃正文。
fn drop_injected_spans(text: &str) -> String {
    if !text.contains('<') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0usize;
    while let Some(offset) = text[cursor..].find('<') {
        let start = cursor + offset;
        let Some(tag) = leading_tag(&text[start..]) else {
            out.push_str(&text[cursor..=start]);
            cursor = start + 1;
            continue;
        };
        let tag_end = start + tag.end;
        if tag.closing || !is_injected_tag(tag.name) {
            out.push_str(&text[cursor..tag_end]);
            cursor = tag_end;
            continue;
        }
        match closing_tag_end(&text[start..], tag.name) {
            Some(close_end) => {
                out.push_str(&text[cursor..start]);
                out.push(' ');
                cursor = start + close_end;
            }
            None => {
                out.push_str(&text[cursor..tag_end]);
                cursor = tag_end;
            }
        }
    }
    out.push_str(&text[cursor..]);
    out
}

/// 开头是 `<tag …>…</tag>`、后面还跟着别的正文时，**拆掉开头的元素、保留其后的正文**。
///
/// 语义与「整段就是一个包装对」一致：元素只是包装，元素里外都是用户说的话，所以取
/// 「内部 + 元素之后的剩余」而不是把后面的正文丢掉。例子：
///
/// - `<user_query>真实问题</user_query>（补充一句）` → `真实问题（补充一句）`
///
/// 判据保守：只认简单标签名、必须找到配对的闭合标签；不以标签开头的记录完全不走这里
/// （真人手打 `前一句 <user_query>x</user_query> 后一句` 原样保留）。
fn unwrap_leading_element(text: &str) -> String {
    let trimmed = text.trim();
    let Some(tag) = leading_tag(trimmed) else {
        return trimmed.to_string();
    };
    let Some(open_end) = trimmed.find('>').map(|index| index + 1) else {
        return trimmed.to_string();
    };
    let Some((inner_end, after_end)) = leading_element_bounds(trimmed, tag.name, open_end) else {
        return trimmed.to_string();
    };
    if inner_end <= open_end {
        return trimmed.to_string();
    }
    let inner = trimmed[open_end..inner_end].trim();
    let rest = trimmed[after_end..].trim();
    if rest.is_empty() {
        return inner.to_string();
    }
    // 只在「两侧都是 ASCII 字母数字」时补空格：中文紧贴（`</user_query>（补充一句）`）保持原样。
    let need_space = inner
        .chars()
        .last()
        .is_some_and(|ch| ch.is_ascii_alphanumeric())
        && rest
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_alphanumeric());
    if need_space {
        format!("{inner} {rest}")
    } else {
        format!("{inner}{rest}")
    }
}

/// 找开头元素配对的闭合标签，返回 `(内部结束位置, 闭合标签之后的位置)`。
///
/// 闭合标签内部除空格外不得有别的东西（`</foo bar>` 不是我们要找的那个）。
fn leading_element_bounds(text: &str, name: &str, from: usize) -> Option<(usize, usize)> {
    let needle = format!("</{name}");
    let mut cursor = from;
    while let Some(offset) = text[cursor..].find(&needle) {
        let start = cursor + offset;
        let after = &text[start + needle.len()..];
        if let Some(gt) = after.find('>') {
            if after[..gt].chars().all(char::is_whitespace) {
                return Some((start, start + needle.len() + gt + 1));
            }
        }
        cursor = start + needle.len();
    }
    None
}

/// 兜底：剥完仍以 `<tag …>` 开头、且文本里**存在**配对的 `</tag>` 时，把开头这个开标签切掉。
///
/// 现场残留（真实库）：
/// `<teammate-message teammate_id="system" summary="…">…</teammate-message> 补充一句`：
/// 配对的闭合标签不在末尾，「整段就是一个标签对」的判据不成立，但开头那串带属性的标签
/// 绝不该上卡片。
///
/// 判据仍然保守：
/// 1. 只切到开标签的 `>` **之后**，正文一个字都不动（宁可留着闭合标签的残影）；
/// 2. 找不到配对闭合标签时**一律不切**，否则 `比较 a < b 与 c > d` 会被当成标签吃掉；
/// 3. 名字必须是简单标签名（[`is_plain_tag_name`]），`<3 天` 不算；
/// 4. 最多切 [`WRAPPER_UNWRAP_DEPTH`] 次，切不动（没有进度）立刻收工。
fn strip_residual_leading_tag(text: &str) -> &str {
    let mut current = text.trim();
    for _ in 0..WRAPPER_UNWRAP_DEPTH {
        let Some(tag) = leading_tag(current) else {
            break;
        };
        // 落单的闭合标签 / 名字不像标签：不动。
        if tag.closing || !is_plain_tag_name(tag.name) {
            break;
        }
        // 没有配对闭合标签就不能切：正文里的 `<` 比较符不是标签。
        if closing_tag_end(current, tag.name).is_none() {
            break;
        }
        let rest = current[tag.end..].trim();
        if rest.len() == current.len() {
            break;
        }
        current = rest;
    }
    current
}

/// 整段被单个标签包住时去掉这层包装：`<user_query>真实问题</user_query>` → `真实问题`。
///
/// 真实库上出现过 `excerpt = "<user_query>总结一下…</user_query>"`：内容已经是用户真正说的话，
/// 但外层还留着产品拼的包装标签，卡片上看起来像标签残留。
///
/// 判据刻意保守，宁可留标签也不吃掉正文：
/// 1. 整段 `trim()` 后必须以 `<tag …>` 开头、以配对的 `</tag>` **结尾**（中途出现别的文本就不算
///    「整段就是一个标签对」，例如 `前一句 <user_query>x</user_query> 后一句` 原样保留）；
/// 2. 标签名必须是简单名 `[a-z][a-z0-9_]*`，属性允许（`<user_query lang="zh">`）；
/// 3. 内部不再以**同名**开标签开头（`<a><a>x</a>` 形状可疑，放弃剥离）；
/// 4. 循环剥到「不再变化」为止（上限 [`WRAPPER_UNWRAP_DEPTH`] 层），不用贪婪匹配
///    （闭合标签从末尾锚定，不搜最近的）。
///
/// 返回 `trim()` 过的内部文本；内部为空时返回 `""`，由调用方按「这条没有正文」处理。
fn unwrap_wrapper_tags(text: &str) -> &str {
    let mut current = text.trim();
    for _ in 0..WRAPPER_UNWRAP_DEPTH {
        // 开标签：不是标签 / 是闭合标签 / 名字不是简单名 → 收工。
        let Some(open) = leading_tag(current) else {
            break;
        };
        if open.closing || !is_wrapper_tag_name(open.name) {
            break;
        }
        // 闭合标签必须锚在末尾：`<a>x</a> 尾巴` 因为末尾不是 `</a>` 而不剥。
        let Some(close_start) = trailing_closing_tag_start(current, open.name) else {
            break;
        };
        if close_start < open.end {
            break;
        }
        let inner = &current[open.end..close_start];
        // 内部以同名开标签开头：形状可疑，保守放弃（不再二次猜测嵌套）。
        if matches!(leading_tag(inner.trim_start()), Some(tag) if !tag.closing && tag.name == open.name)
        {
            break;
        }
        let inner = inner.trim();
        if inner.len() == current.len() {
            break;
        }
        current = inner;
    }
    current
}

/// 包装标签名：简单名，`[a-z][a-z0-9_]*`（`<user_query>` / `<craft_mode>` 命中，`<3 天` 不命中）。
///
/// 比 [`is_plain_tag_name`] 严格：这里只认小写名 + 无 `-`，因为「整段剥包装」动的正文更多。
fn is_wrapper_tag_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if first.is_ascii_lowercase() => {}
        _ => return false,
    }
    chars.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
}

/// 兜底用的标签名判据：字母开头、只含字母 / 数字 / `-` / `_`、长度有上限。
///
/// 比 [`is_wrapper_tag_name`] 宽松：大小写都认、允许 `-`（`<teammate-message …>` 这种工具标签
/// 必须认出来），但仍然拦住 `<3 天`、`<--` 之类不是标签的文字。
fn is_plain_tag_name(name: &str) -> bool {
    const MAX_LEN: usize = 32;
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() => {}
        _ => return false,
    }
    name.len() <= MAX_LEN && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
}

/// 末尾的 `</name>`（允许 `</name >`）起点；末尾不是这个闭合标签时返回 `None`。
///
/// 只认**最后一个** `<`：文本以 `>` 结尾时，末端标签必然从最后一个 `<` 开始，因此不需要
/// 也不允许贪婪搜索中间出现的 `</name>`。
fn trailing_closing_tag_start(text: &str, name: &str) -> Option<usize> {
    if !text.ends_with('>') {
        return None;
    }
    let lt = text.rfind('<')?;
    let head = text[lt + 1..text.len() - 1].strip_prefix('/')?;
    (head.trim() == name).then_some(lt)
}

/// 列表卡片的单行摘要：正文里**最近一条用户消息**，没有用户消息时退到最近一条助手消息。
///
/// 口径与 [`content_preview`] 完全一致（同一个 `record_text` / [`speaker_label`]），只是
/// **只取 1 条、截到约 120 字**：列表一次要下发上百个组，把 6 条 × 480 字都放进去会让载荷
/// 无谓翻好几倍，而卡片只有两行可见。
///
/// 逐条判定（记录级过滤，**文本手术到此为止**）：
///
/// 1. 每条候选先过 [`excerpt_body`]（剥注入块 + 去外层包装标签 + 兜底切头标签）；
/// 2. 剥完为空 → 这条没有正文，继续往前找；
/// 3. 剥完**仍是工具 / 注入载荷**（[`is_tool_payload`]：夹着 `<tool-use-id>` / `<task-id>`
///    这类标签、或残留两个以上标签、或以 `</` 开头）→ 不参与摘要，继续往前找；
/// 4. 扫描中维护两个候选：`best_user`（最近一条**干净**的用户记录）与 `best_any`（最近一条
///    干净的助手记录）；优先 `best_user`，没有才 `best_any`，两者都没有（整组全是载荷 /
///    只有工具记录）才返回 `None`。
///
/// 真实库上 102 个组里有 25 个的第一条用户记录长这样（标签在**文本中间**，整段不是单个
/// 包装对，前面几条规则都碰不到）：
///
/// ```text
/// TeiPX5</task-id>
/// <tool-use-id>call_00_xvhfnbByUsBt9oUN4CDK0922</tool-use-id>
/// <status>failed</status>
/// ```
///
/// 这种记录宁可**不出摘要**也不能上卡片：卡片上显示 `</task-id><tool-use-id>…` 比没有摘要更糟。
/// 整组都是载荷时返回 `None`（前端不占位），而不是给空字符串让卡片多出一块空白。
///
/// 截断（[`excerpt_text`]）**一定在判定之后**：先截断会把闭合标签切掉，之后再也判不出载荷。
pub(super) fn excerpt_of(content: &ContentState) -> Option<String> {
    let ContentState::Ready(snapshot) = content else {
        return None;
    };
    // 列表缓存命中时 text 为空，直接返回首次读取时算好的摘要。
    if let Some(facts) = &snapshot.list_facts {
        return facts.excerpt.clone();
    }
    let mut best_user: Option<String> = None;
    let mut best_any: Option<String> = None;
    for line in snapshot.text.lines().rev() {
        let Ok(record) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(text) = record_text(&record) else {
            continue;
        };
        // 顺序固定：先剥注入块并去包装 → 判空 → 判载荷 → 最后才截断。
        let text = excerpt_body(&text);
        if text.is_empty() {
            continue;
        }
        // 工具调用 / 工具结果记录：内容是工具拼进来的载荷，拿来做摘要比不显示更糟。
        if is_tool_payload(text.as_ref()) {
            continue;
        }
        let excerpt = excerpt_text(text.as_ref());
        match speaker_label(&record) {
            // 逆序遍历：第一条干净的用户记录就是「最近一条用户消息」，用户消息优先于其它任何
            // 记录，再往前只有更旧的内容，可以直接收工。
            "用户" => {
                best_user = Some(excerpt);
                break;
            }
            // 助手消息只作兜底，且只记最近一条。
            "助手" if best_any.is_none() => best_any = Some(excerpt),
            // 工具结果 / 事件记录（说话人判不出来或判成「记录」）：一律不参与摘要——它们的
            // 正文常常是大段 JSON / 命令输出，拿来做摘要比不显示更糟（有回归用例钉着）。
            _ => {}
        }
    }
    best_user.or(best_any)
}

/// 工具载荷里固定出现的标签名：这些标签只会由工具调用 / 工具结果拼进记录，用户不会手打。
///
/// 现场残留（真实库，102 组里 25 组仍带标签）：
/// `TeiPX5</task-id>\n<tool-use-id>call_00_xvhfnbByUsBt9oUN4CDK0922</tool-use-id>\n<status>failed</status>`
///
/// 标签全在**文本中间**，整段不是单个包装对，[`strip_injected_blocks`] /
/// [`unwrap_wrapper_tags`] / [`strip_residual_leading_tag`] 都碰不到它们——所以这类记录只能
/// 整条不参与摘要（见 [`is_tool_payload`]），不再做文本手术。
const TOOL_PAYLOAD_TAGS: [&str; 6] = [
    "tool-use-id",
    "task-id",
    "tool_result",
    "function_results",
    "tool_use",
    "result",
];

/// 这条记录（已剥过注入块 / 包装标签）是否属于「工具 / 注入载荷」：满足任一条即算。
///
/// 1. 出现 [`TOOL_PAYLOAD_TAGS`] 里任一固定载荷标签（`<tool-use-id>` / `<task-id>` …）；
/// 2. 出现 **2 个以上**「标签形状」标记（工具块常常是连续好几个标签拼在一起）；
/// 3. 以 `</` 开头：记录被截断后只剩闭合标签残片（`</task-id>`）。
///
/// 判据刻意保守，宁可漏判（留着一条可疑摘要），也不误吞用户的话：
///
/// - 名字必须**紧跟** `<` / `</`，`比较 a < b 与 c > d` 里的 `<` 不算标签；
/// - 纯小写 + 下划线 + 数字的「包装名」（`<user_query>` / `<status>`）**不计数**：真人确实
///   真打过 `前一句 <user_query>不算包装</user_query> 后一句`，有回归用例钉着这个反例。
fn is_tool_payload(text: &str) -> bool {
    // 残片：文本以闭合标签开头（`</task-id>` 这种被截断的记录）。
    if text.starts_with("</") {
        return true;
    }
    let mut tag_like = 0usize;
    let mut pos = 0usize;
    while let Some(offset) = text[pos..].find('<') {
        let start = pos + offset;
        match tag_marker_at(text, start) {
            Some((name, end)) => {
                // 固定载荷标签：出现一个就够定案。
                if TOOL_PAYLOAD_TAGS
                    .iter()
                    .any(|tag| name.eq_ignore_ascii_case(tag))
                {
                    return true;
                }
                if !is_wrapper_tag_name(name) {
                    tag_like += 1;
                    if tag_like >= 2 {
                        return true;
                    }
                }
                pos = end;
            }
            // 不是标签（正文里的比较符）：只跳过这个 `<`，继续往后扫。
            None => pos = start + 1,
        }
    }
    false
}

/// 解析 `text[start..]` 处的「标签形状」标记（`<name …>` / `</name>`）。
///
/// 返回（标签名，`>` 之后的字节下标）；不是标签时返回 `None`。判据：
///
/// 1. `start` 必须是 `<`，名字必须**紧跟** `<` / `</`（`a < b` 中间有空白 → 不是标签）；
/// 2. 名字后只允许 `>` / `/` / 空白（属性），`<a=1` 这种不算；
/// 3. 必须以 `>` 收尾，且到 `>` 之前不能再出现 `<` 或换行（否则那是正文而不是一个标签）。
fn tag_marker_at(text: &str, start: usize) -> Option<(&str, usize)> {
    let inner = text[start..].strip_prefix('<')?;
    let head = inner.trim_start_matches('/');
    let name_len = head
        .chars()
        .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '-' || *ch == '_')
        .count();
    if name_len == 0 {
        return None;
    }
    let name = &head[..name_len];
    let tail = &head[name_len..];
    if let Some(first) = tail.chars().next() {
        if !(first == '>' || first == '/' || first.is_whitespace()) {
            return None;
        }
    }
    let gt = tail.find('>')?;
    if tail[..gt].contains('<') || tail[..gt].contains('\n') {
        return None;
    }
    // 绝对结束位置 = `<` + 被剪掉的前导 `/` + 名字 + 属性 + `>`。
    let slashes = inner.len() - head.len();
    Some((name, start + 1 + slashes + name_len + gt + 1))
}

/// 摘要文本的截断（按字符而不是字节，避免把多字节汉字截成半个）。
fn excerpt_text(text: &str) -> String {
    text.chars().take(EXCERPT_CHARS).collect()
}

#[cfg(test)]
mod list_cache_tests {
    use super::*;
    use crate::modules::session_link::{ContentSnapshot, ListFacts, NormalizedContent};
    use std::sync::Arc;

    /// 缓存命中时 `text` 为空：摘要、预览、记录数、同步判定必须仍然可用。
    /// 这条用例钉住「缓存不丢信息」，防止以后有人给摘要改用 text 而绕过 facts。
    #[test]
    fn cached_snapshot_without_text_still_serves_list() {
        let normalized = NormalizedContent {
            record_count: 2,
            line_digests: vec!["a".into(), "b".into()],
            comparable_line_digests: vec!["a".into(), "b".into()],
            total_digest: "total".into(),
        };
        let facts = ListFacts {
            excerpt: Some("缓存里的摘要".into()),
            preview: vec![json!({"speaker": "用户", "text": "缓存里的预览"})],
            record_count: Some(2),
        };
        let state = ContentState::Ready(ContentSnapshot {
            text: String::new(),
            full_digest: "digest".into(),
            normalized,
            latest_at: Some(1234),
            list_facts: Some(Arc::new(facts)),
        });
        assert_eq!(excerpt_of(&state), Some("缓存里的摘要".to_string()));
        assert_eq!(content_preview(&state)[0]["text"], "缓存里的预览");
        assert_eq!(content_count_view(&state), Some(2));
        assert_eq!(activity_at_of(&state, 999), 1234);
    }
}

#[cfg(test)]
mod excerpt_body_tests {
    use super::*;

    /// 现场残留（真实库 13/102 组）：用户消息由协议拼成「正文 + 附件块」两段，
    /// 摘要在切掉开标签后还留着闭合标签与本地附件路径。
    #[test]
    fn cuts_residue_after_first_closing_tag() {
        let text = "<user_query>@image#1:这里再帮我调整一下</user_query> <image_local_path>C:/tmp/a.png</image_local_path>";
        assert_eq!(excerpt_body(text), "@image#1:这里再帮我调整一下");
    }

    /// 整段就是一个包装对：由 `unwrap_wrapper_tags` 处理，末尾剪切不应把正文吃掉。
    #[test]
    fn keeps_body_of_single_wrapper() {
        assert_eq!(
            excerpt_body("<user_query>总结一下项目规模</user_query>"),
            "总结一下项目规模"
        );
    }

    /// 不以标签开头：真人手打标签也一个字都不动（这条是防回归的红线）。
    #[test]
    fn leaves_user_typed_tags_alone() {
        let text = "前一句 <user_query>不算包装</user_query> 后一句";
        assert_eq!(excerpt_body(text), text);
    }

    /// 普通正文里的比较符不能被当成标签。
    #[test]
    fn leaves_comparisons_alone() {
        let text = "比较 a < b 与 c > d 的差异";
        assert_eq!(excerpt_body(text), text);
    }
}
