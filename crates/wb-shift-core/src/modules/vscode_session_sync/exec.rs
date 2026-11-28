//! 同步执行：快进/覆盖、索引与基线提交、目录合并与文件复制。

use crate::modules::usage_origin;

use super::*;

/// 按模式执行写入。
///
/// R6-07 写入点 4/4：`fast_forward` / `overwrite` 成功后在这里统一记一次归属交接。
/// `codebuddy_ide_session_sync` 委托到 `sync_selected_in_for_variant`，最终也走本函数，
/// 因此**只埋一次**，避免双写。
pub(super) fn execute_sync_item(
    paths: &SessionPaths,
    plan: &SyncItemPlan,
) -> Result<Value, String> {
    let mode = plan.mode;
    let value = match mode {
        SyncMode::FastForward => fast_forward(paths, plan)?,
        SyncMode::Overwrite | SyncMode::UnifyOverwrite => overwrite(paths, plan)?,
        // 插件侧没有 sessions 表、不感知归档状态，永不产生状态同步计划。
        SyncMode::StatusOnly => return Err("当前客户端不支持仅同步归档，已拒绝".to_string()),
    };
    // 走到这里说明正文与基线都已落盘。失败只记日志：索引写不进去最多让归属回退到
    // 文件自身 uid，绝不能把一次已经成功的同步报告成失败。
    if let Some(namespace) = plan.ns {
        usage_origin::record_sync_quiet(
            &paths.store_root,
            namespace,
            &plan.target_conversation_id,
            namespace,
            &plan.source_conversation_id,
            now_ms(),
            &plan.source_member.uid,
        );
    }
    Ok(value)
}

/// 一条待写入的副本消息文件。
pub(super) struct PlannedFile {
    /// 派生后的消息 id（写进索引的条目 id）。
    pub(super) id: String,
    pub(super) path: PathBuf,
    pub(super) content: String,
    /// 目标已有同名同内容文件：本次不写、回滚也不删（重跑幂等）。
    pub(super) already_applied: bool,
}

/// 幂等删除文件：`NotFound` 视为成功（文件本就不在，目标状态已达成）。
pub(super) fn remove_file_if_exists(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// 写目标会话索引（`index.json` 与 `.index_bak.json` 同内容，与复制路径一致）。
pub(super) fn write_session_index(target_dir: &Path, index: &Value) -> Result<(), String> {
    let text = index.to_string();
    session_backup::durable_write_str(&target_dir.join("index.json"), &text)
        .map_err(|error| format!("写入副本索引失败：{error}"))?;
    session_backup::durable_write_str(&target_dir.join(".index_bak.json"), &text)
        .map_err(|error| format!("写入副本索引备份失败：{error}"))?;
    Ok(())
}

/// 快进：把来源基线之后的新增记录追加到副本（design §6.1）。
pub(super) fn fast_forward(paths: &SessionPaths, plan: &SyncItemPlan) -> Result<Value, String> {
    let source_index = vscode_session_link::read_session_index(&plan.source_dir)
        .ok_or_else(|| "源会话索引缺失或损坏，已停止同步".to_string())?;
    let target_index = vscode_session_link::read_session_index(&plan.target_dir)
        .ok_or_else(|| "目标会话索引缺失或损坏，已停止同步".to_string())?;
    let source_message_ids = vscode_session_link::message_ids_of(&source_index);
    let target_message_ids = vscode_session_link::message_ids_of(&target_index);
    let baseline_count = plan.target_content.normalized.record_count;
    if target_message_ids.len() != baseline_count {
        return Err("目标内容与索引不一致，已停止同步".to_string());
    }
    if source_message_ids.len() < baseline_count
        || plan.source_content.normalized.line_digests.len() != source_message_ids.len()
    {
        return Err("来源内容与索引不一致，已停止同步".to_string());
    }
    let source_messages = source_index
        .get("messages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let source_requests = source_index
        .get("requests")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    // 翻译表：前缀按位置对齐（判定保证「目标内容是来源内容的严格有序前缀」），
    // 新增消息 / 请求用确定性派生 id（design §6.1：重跑覆盖同一批文件）。
    let mut translation =
        vscode_session_link::positional_translation(&source_message_ids, &target_message_ids);
    let target_request_ids = request_ids_of(&target_index);
    let source_request_ids = request_ids_of(&source_index);
    let request_prefix = target_request_ids.len().min(source_request_ids.len());
    translation.extend(vscode_session_link::positional_translation(
        &source_request_ids[..request_prefix],
        &target_request_ids[..request_prefix],
    ));
    let request_digests =
        vscode_session_link::request_digests(&source_index, &plan.source_conversation_id);

    // 新增请求的确定性 id 必须先落进翻译表：消息文件里的 `extra.requestId` 要一起翻译，
    // 否则副本里会残留源请求 id，归一化摘要也就对不上源（判定地基被破坏）。
    let mut used_request_ids: BTreeSet<String> = target_request_ids.iter().cloned().collect();
    for (index, source_id) in source_request_ids.iter().enumerate().skip(request_prefix) {
        let digest = request_digests.get(index).cloned().unwrap_or_default();
        let mut derived = None;
        for salt in 0..SALT_LIMIT {
            let candidate = vscode_session_link::derive_request_id(
                &plan.target_uid,
                &plan.target_conversation_id,
                index,
                &digest,
                salt,
            );
            if !used_request_ids.contains(&candidate) {
                derived = Some(candidate);
                break;
            }
        }
        let derived = derived.ok_or_else(|| "目标账号已存在同名请求，已停止同步".to_string())?;
        used_request_ids.insert(derived.clone());
        translation.insert(source_id.clone(), derived);
    }

    let mut files: Vec<PlannedFile> = Vec::new();
    for (index, source_id) in source_message_ids.iter().enumerate().skip(baseline_count) {
        let digest = &plan.source_content.normalized.line_digests[index];
        let source_file = plan
            .source_dir
            .join("messages")
            .join(format!("{source_id}.json"));
        let text = std::fs::read_to_string(&source_file)
            .map_err(|error| format!("读取来源消息失败（{source_id}）：{error}"))?;
        let value: Value = serde_json::from_str(&text)
            .map_err(|_| format!("来源消息 {source_id} 无法解析，已停止同步"))?;
        let planned = plan_message_file(plan, index, digest, source_id, &value, &mut translation)?;
        translation.insert(source_id.clone(), planned.id.clone());
        files.push(planned);
    }

    // 目标索引 = 旧索引 + 翻译后的新增消息 / 请求。
    let mut out_index = target_index.clone();
    if let Some(object) = out_index.as_object_mut() {
        if let Some(array) = object
            .entry("messages".to_string())
            .or_insert_with(|| json!([]))
            .as_array_mut()
        {
            for entry in source_messages.iter().skip(baseline_count) {
                array.push(vscode_session_link::translate_message_entry(
                    entry,
                    &translation,
                ));
            }
        }
        if let Some(array) = object
            .entry("requests".to_string())
            .or_insert_with(|| json!([]))
            .as_array_mut()
        {
            for entry in source_requests.iter().skip(request_prefix) {
                array.push(vscode_session_link::translate_request_entry(
                    entry,
                    &translation,
                ));
            }
        }
    }

    let backup_dir = begin_index_backup(paths, plan)?;
    let mut created: Vec<PathBuf> = Vec::new();
    let assets = match write_fast_forward(plan, &out_index, &files, &mut created) {
        Ok(assets) => assets,
        Err(error) => {
            return Err(rollback_message(
                rollback_index(plan, &backup_dir, &created),
                &error,
            ));
        }
    };
    // 基线提交失败 → 恢复索引 + 删除本次新增文件（design §6.1 步 6）。
    if let Err(error) = commit_baseline(paths, plan) {
        return Err(rollback_message(
            rollback_index(plan, &backup_dir, &created),
            &error,
        ));
    }

    let source_count = plan.source_content.normalized.record_count;
    let target_before = plan.target_content.normalized.record_count;
    Ok(json!({
        "groupId": plan.group_id,
        "status": "synced",
        "verdict": plan.verdict.as_str(),
        "mode": plan.mode.as_str(),
        "recordCount": {"source": source_count, "targetBefore": target_before, "target": source_count},
        "assets": {"copied": assets.0, "overwritten": assets.1},
        "backup": backup_dir.to_string_lossy(),
        "message": format!(
            "已把当前账号新增的 {} 条内容同步到副本（追加，不改动目标已有内容）",
            source_count.saturating_sub(target_before)
        ),
    }))
}

/// 写入计划：消息文件 → 会话索引 → 附件。任一步失败由调用方回滚。
pub(super) fn write_fast_forward(
    plan: &SyncItemPlan,
    index: &Value,
    files: &[PlannedFile],
    created: &mut Vec<PathBuf>,
) -> Result<(usize, usize), String> {
    for file in files {
        if file.already_applied {
            continue;
        }
        session_backup::durable_write_str(&file.path, &file.content)
            .map_err(|error| format!("写入副本消息失败：{error}"))?;
        created.push(file.path.clone());
    }
    write_session_index(&plan.target_dir, index)?;
    merge_children(&plan.source_dir, &plan.target_dir)
}

/// 提交配对基线：写来源当前记录摘要 + 刷新两侧成员的同步时间（定向更新，不动其它配对）。
pub(super) fn commit_baseline(paths: &SessionPaths, plan: &SyncItemPlan) -> Result<(), String> {
    let group_id = plan.group_id.clone();
    let source_member_id = plan.source_member.member_id.clone();
    let target_member_id = plan.target_member.member_id.clone();
    let normalized = plan.source_content.normalized.clone();
    let baseline_ref = uuid::Uuid::new_v4().to_string();
    session_link::with_link_store_write(paths, move |store| {
        let Some(group) = store.groups.iter_mut().find(|group| group.id == group_id) else {
            return Err("会话的关联关系已不存在，未提交同步结果".to_string());
        };
        if !group
            .members
            .iter()
            .any(|member| member.member_id == source_member_id)
        {
            return Err("当前账号的会话已不存在，未提交同步结果".to_string());
        }
        let now = now_ms();
        for member_id in [&source_member_id, &target_member_id] {
            if let Some(member) = group
                .members
                .iter_mut()
                .find(|member| member.member_id == *member_id)
            {
                member.last_synced_at = Some(now);
            }
        }
        session_link::save_baseline(paths, &baseline_ref, &normalized)?;
        session_link::set_pair_base(
            group,
            &source_member_id,
            &target_member_id,
            &baseline_ref,
            NORMALIZATION_VERSION,
        );
        Ok(())
    })
}

// --- 覆盖（design §6.2） ---------------------------------------------------

/// 覆盖：整目录备份 → 按确定性翻译表全量重建（保留目标会话 id）→ 失败整目录恢复 → 提交基线。
pub(super) fn overwrite(paths: &SessionPaths, plan: &SyncItemPlan) -> Result<Value, String> {
    let source_index = vscode_session_link::read_session_index(&plan.source_dir)
        .ok_or_else(|| "源会话索引缺失或损坏，已停止同步".to_string())?;
    let source_message_ids = vscode_session_link::message_ids_of(&source_index);
    let digests = &plan.source_content.normalized.line_digests;
    if digests.len() != source_message_ids.len() {
        return Err("来源内容与索引不一致，已停止同步".to_string());
    }

    // 1) 整目录备份（先备份再动任何东西）。
    let backup_dir = backup_root(paths, plan.backup_kind)
        .join(&plan.workspace_hash)
        .join(format!("{}{OVERWRITE_SUFFIX}", plan.target_conversation_id));
    vscode_session::remove_dir_all_if_exists(&backup_dir);
    vscode_session::copy_dir_recursive(&plan.target_dir, &backup_dir)
        .map_err(|error| format!("备份目标会话目录失败：{error}"))?;

    // 2) 翻译表：全量重建，消息 / 请求 id 用确定性派生（保留目标会话 id → 工作区索引无需改动）。
    let mut message_ids: BTreeMap<String, String> = BTreeMap::new();
    for (index, source_id) in source_message_ids.iter().enumerate() {
        message_ids.insert(
            source_id.clone(),
            vscode_session_link::derive_message_id(
                &plan.target_uid,
                &plan.target_conversation_id,
                index,
                &digests[index],
                0,
            ),
        );
    }
    let source_request_ids = request_ids_of(&source_index);
    let request_digests =
        vscode_session_link::request_digests(&source_index, &plan.source_conversation_id);
    let mut request_ids: BTreeMap<String, String> = BTreeMap::new();
    for (index, source_id) in source_request_ids.iter().enumerate() {
        request_ids.insert(
            source_id.clone(),
            vscode_session_link::derive_request_id(
                &plan.target_uid,
                &plan.target_conversation_id,
                index,
                &request_digests.get(index).cloned().unwrap_or_default(),
                0,
            ),
        );
    }
    let remap = vscode_session::remap_plan_from_maps(message_ids, request_ids);

    // 3) 先写临时目录，再原子替换（保留目标会话 id，工作区索引与 `current` 语义天然不变）。
    let tmp_dir = plan.target_dir.with_file_name(format!(
        ".tmp-{}{OVERWRITE_SUFFIX}",
        plan.target_conversation_id
    ));
    vscode_session::remove_dir_all_if_exists(&tmp_dir);
    if let Err(error) =
        vscode_session::write_conversation(&plan.source_dir, &tmp_dir, &source_index, &remap)
    {
        vscode_session::remove_dir_all_if_exists(&tmp_dir);
        return Err(format!("重建副本会话目录失败：{error}"));
    }
    vscode_session::remove_dir_all_if_exists(&plan.target_dir);
    if let Err(error) = std::fs::rename(&tmp_dir, &plan.target_dir) {
        vscode_session::remove_dir_all_if_exists(&tmp_dir);
        return Err(rollback_message(
            restore_dir(&backup_dir, &plan.target_dir),
            &format!("提交副本会话目录失败：{error}"),
        ));
    }

    // 4) 复算核验：重建后的记录摘要必须与来源一致，否则整目录回滚。
    let rebuilt =
        vscode_session_link::read_session_content(&plan.target_dir, &plan.target_conversation_id);
    let verified = matches!(
        &rebuilt,
        ContentState::Ready(snapshot)
            if snapshot.normalized.line_digests == plan.source_content.normalized.line_digests
    );
    if !verified {
        return Err(rollback_message(
            restore_dir(&backup_dir, &plan.target_dir),
            "重建后的内容与来源不一致，已整目录回滚",
        ));
    }

    // 5) 提交基线；失败 → 整目录从备份恢复。
    if let Err(error) = commit_baseline(paths, plan) {
        return Err(rollback_message(
            restore_dir(&backup_dir, &plan.target_dir),
            &error,
        ));
    }

    let source_count = plan.source_content.normalized.record_count;
    Ok(json!({
        "groupId": plan.group_id,
        "status": "synced",
        "verdict": plan.verdict.as_str(),
        "mode": plan.mode.as_str(),
        "recordCount": {
            "source": source_count,
            "targetBefore": plan.target_content.normalized.record_count,
            "target": source_count,
        },
        "assets": {"copied": 0, "overwritten": 0},
        "backup": backup_dir.to_string_lossy(),
        "message": format!("已用当前账号的 {source_count} 条内容覆盖副本（覆盖前已整目录备份）"),
    }))
}

// --- 附件合并 --------------------------------------------------------------

/// 把源会话里目标缺失的附件 / 子目录补进目标，返回（新增数, 覆盖数）。
///
/// 只处理非索引顶层条目：`index.json` / `.index_bak.json` / `messages/` 由写入流程单独处理。
/// 同名不同内容以源为准覆盖（源是权威），覆盖数在报告里标注。
pub(super) fn merge_children(
    source_dir: &Path,
    target_dir: &Path,
) -> Result<(usize, usize), String> {
    let mut copied = 0usize;
    let mut overwritten = 0usize;
    let entries = std::fs::read_dir(source_dir).map_err(|error| error.to_string())?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name == "index.json" || name == ".index_bak.json" || name == "messages" {
            continue;
        }
        let from = entry.path();
        let to = target_dir.join(&name);
        if from.is_dir() {
            let (nested_copied, nested_overwritten) = merge_dir(&from, &to)?;
            copied += nested_copied;
            overwritten += nested_overwritten;
        } else if from.is_file() {
            match copy_if_needed(&from, &to)? {
                Some(true) => copied += 1,
                Some(false) => overwritten += 1,
                None => {}
            }
        }
    }
    Ok((copied, overwritten))
}

/// [`merge_children`] 的递归部分。
pub(super) fn merge_dir(source_dir: &Path, target_dir: &Path) -> Result<(usize, usize), String> {
    let mut copied = 0usize;
    let mut overwritten = 0usize;
    let entries = std::fs::read_dir(source_dir).map_err(|error| error.to_string())?;
    for entry in entries.flatten() {
        let from = entry.path();
        let to = target_dir.join(entry.file_name());
        if from.is_dir() {
            let (nested_copied, nested_overwritten) = merge_dir(&from, &to)?;
            copied += nested_copied;
            overwritten += nested_overwritten;
        } else if from.is_file() {
            match copy_if_needed(&from, &to)? {
                Some(true) => copied += 1,
                Some(false) => overwritten += 1,
                None => {}
            }
        }
    }
    Ok((copied, overwritten))
}

/// 目标缺失 → 复制（`Some(true)`）；同名不同内容 → 以源为准覆盖（`Some(false)`）；相同 → `None`。
pub(super) fn copy_if_needed(from: &Path, to: &Path) -> Result<Option<bool>, String> {
    let same = match (std::fs::read(to), std::fs::read(from)) {
        (Ok(existing), Ok(source)) => existing == source,
        _ => false,
    };
    if same {
        return Ok(None);
    }
    let existed = to.exists();
    if let Some(parent) = to.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
    }
    std::fs::copy(from, to).map_err(|error| format!("复制附件失败：{error}"))?;
    Ok(Some(!existed))
}
