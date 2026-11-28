//! 会话复制与切换复制入口：复制选项、目标构建、逐会话复制与报告。

#[allow(unused_imports)]
use super::index_merge::*;
#[allow(unused_imports)]
use super::list::*;
#[allow(unused_imports)]
use super::paths::*;
#[allow(unused_imports)]
use super::remap::*;
use super::*;

// ---------------------------------------------------------------------------
// 会话复制
// ---------------------------------------------------------------------------

/// 把勾选的会话复制到目标账号（新 id，加法）。返回复制报告。
///
/// 前置校验（任一不满足即拒绝，且不写任何文件）：
/// VS Code 必须未运行；目标 uid 合法；源 uid 由当前扩展登录态推导且源 ≠ 目标。
pub fn copy_vscode_sessions(target_uid: &str, items: &[CopyItem]) -> Result<Value, String> {
    if items.is_empty() {
        return Ok(json!({ "copied": [], "errors": [] }));
    }
    if !is_safe_uid(target_uid) {
        return Err("目标账号 uid 非法，拒绝写入".to_string());
    }
    // 写入前必须先完全退出 VS Code，否则会被运行中的扩展覆盖。
    if vscode_ext::is_vscode_running() {
        return Err(
            "检测到 VS Code 正在运行，请先完全退出后再复制会话，否则写入会被 VS Code 覆盖。"
                .to_string(),
        );
    }
    let source_uid = vscode_ext::active_ext_uid()
        .filter(|uid| is_safe_uid(uid))
        .ok_or_else(|| {
            "未检测到 VS Code CodeBuddy 插件当前登录账号，无法定位源会话。请先在 VS Code 中登录该插件后重试。"
                .to_string()
        })?;
    if source_uid == target_uid {
        return Err("源账号与目标账号相同，无需复制会话".to_string());
    }
    let root = ext_data_root()
        .ok_or_else(|| "未找到 VS Code CodeBuddy 插件数据目录，无法复制会话".to_string())?;
    let backup_root = backup_dir().join(VSCODE_STORE.backup_kind).join(utc_iso());
    copy_sessions_in(&root, &backup_root, &source_uid, target_uid, items)
}

/// [`copy_vscode_sessions`] 的可测实现：显式传入数据根与备份根（VS Code 插件策略）。
pub fn copy_sessions_in(
    root: &Path,
    backup_root: &Path,
    source_uid: &str,
    target_uid: &str,
    items: &[CopyItem],
) -> Result<Value, String> {
    copy_sessions_in_with(
        VSCODE_STORE,
        VSCODE_COPY,
        root,
        backup_root,
        source_uid,
        target_uid,
        items,
    )
}

/// 复制内核（两个数据仓共用）：显式传入数据仓、复制策略、数据根与备份根。
///
/// 逐条隔离：单条失败只回退该条（删临时/成品目录 + 回退该工作区索引），
/// 记入 `errors[]` 后继续处理其余条目，不整体中止。
///
/// **目标位置解析**（本函数的第一个阶段）：
/// - [`ScopeSegment::Uid`]（VS Code / CodeBuddy IDE）：项目段恒为目标账号 uid、桶取源桶，
///   目标目录按需创建——行为与历史完全一致；
/// - [`ScopeSegment::ProjectKey`]（JetBrains）：项目段沿用源侧项目 key（项目路径决定，
///   跨账号同值），桶只能取**目标该项目下已存在的值**（安装级常量）。目标账号在该项目下
///   一个会话都没有时**不凭空创建**目录，直接整次拒绝并提示用户先去 IDE 里打开该项目——
///   凭空造一个桶只会得到一个客户端永远打不开的目录。
pub(crate) fn copy_sessions_in_with(
    spec: SessionStoreSpec,
    options: CopyOptions,
    root: &Path,
    backup_root: &Path,
    source_uid: &str,
    target_uid: &str,
    items: &[CopyItem],
) -> Result<Value, String> {
    if !is_safe_uid(source_uid) {
        return Err("源账号 uid 非法".to_string());
    }
    if !is_safe_uid(target_uid) {
        return Err("目标账号 uid 非法".to_string());
    }
    if source_uid == target_uid {
        return Err("源账号与目标账号相同，无需复制会话".to_string());
    }

    let mut errors: Vec<Value> = Vec::new();

    // R6-07 写入点 3/4：插件/IDE **复制**的归属交接。boundary 在函数开头取一次，
    // 整批条目共用同一个时刻——它们都是「本次批量复制」的产物，同一批里先后差几毫秒
    // 没有语义（段的边界只用来区分「复制前的历史」与「复制后的新增」）。
    let origin_boundary_ms = crate::modules::config::now_ms();
    // 命名空间与读侧共用同一份映射（`VSCode → ide:vscode` 等）；未知客户端不记索引。
    let origin_namespace =
        crate::modules::usage_origin::IdeClientSource::from_client_dir(spec.client_dir)
            .map(|client| client.namespace());
    // 索引落在工具存储根下：由备份根（`<store>/backups/<kind>/<iso>`）反推。
    // 反推不出来（测试注入的临时备份目录）就整块跳过——宁可不写，也不把索引写到
    // 不相干的目录里去污染用户真实存储。
    let origin_store_root = crate::modules::usage_origin::store_root_from_backup_root(backup_root);

    // 阶段 1：逐条校验并解析目标位置（不写盘）。
    let mut prepared: Vec<PreparedCopy> = Vec::with_capacity(items.len());
    let mut missing_projects: Vec<String> = Vec::new();
    for item in items {
        let workspace_hash = item.workspace_hash.trim();
        let conversation_id = item.conversation_id.trim();
        if !is_hex32(workspace_hash) || !is_hex32(conversation_id) {
            errors.push(json!({
                "workspaceHash": item.workspace_hash,
                "conversationId": item.conversation_id,
                "error": "工作区或会话 id 非法",
            }));
            continue;
        }
        let source_project_key = source_project_key_of(spec, item, source_uid);
        let source_ws_dir =
            scope_history_root_in(spec, root, source_uid, &source_project_key).join(workspace_hash);
        if !source_ws_dir.join(conversation_id).is_dir() {
            errors.push(json!({
                "workspaceHash": workspace_hash,
                "conversationId": conversation_id,
                "error": "源会话目录不存在",
            }));
            continue;
        }
        let (target_project_key, target_bucket) = match spec.scope {
            // 目标那一级恒为目标账号 uid：源桶原样沿用。
            ScopeSegment::Uid => (target_uid.to_string(), workspace_hash.to_string()),
            ScopeSegment::ProjectKey => {
                // 无项目桶的项目段是**账号 uid**（插件在「没有打开项目」时的退化写法），
                // 跨账号不同值：必须换成目标账号自己的 uid，否则会写进
                // `<目标>/JetBrains/<源 uid>/`——客户端永远不会去读的目录。
                // 真实项目 key 由项目路径决定、跨账号同值，原样沿用。
                let target_project_key = if source_project_key == source_uid {
                    target_uid.to_string()
                } else {
                    source_project_key.clone()
                };
                let target_history =
                    scope_history_root_in(spec, root, target_uid, &target_project_key);
                match existing_bucket(&target_history, workspace_hash) {
                    Some(bucket) => (target_project_key, bucket),
                    None => {
                        if !missing_projects.contains(&target_project_key) {
                            missing_projects.push(target_project_key);
                        }
                        continue;
                    }
                }
            }
        };
        prepared.push(PreparedCopy {
            source_ws_dir,
            target_project_key,
            target_bucket,
            conversation_id: conversation_id.to_string(),
        });
    }

    // 目标账号缺项目：整次拒绝（不写任何文件），并指名是哪个项目。
    if !missing_projects.is_empty() {
        return Err(missing_target_project_error(&missing_projects));
    }

    // 阶段 2：为每个目标工作区建立「磁盘备份 + 内存工作副本」，保证逐条隔离与可回退。
    let mut states: BTreeMap<String, WorkspaceState> = BTreeMap::new();
    for item in &prepared {
        let key = item.state_key();
        if states.contains_key(&key) {
            continue;
        }
        let ws_dir = item.target_ws_dir(spec, root, target_uid);
        let state = WorkspaceState::load(&ws_dir, &item.backup_dir(spec, backup_root))
            .map_err(|error| format!("备份目标工作区索引失败（{key}）：{error}"))?;
        states.insert(key, state);
    }

    let mut copied: Vec<Value> = Vec::new();
    for item in &prepared {
        let key = item.state_key();
        let Some(state) = states.get_mut(&key) else {
            errors.push(json!({
                "workspaceHash": item.target_bucket,
                "conversationId": item.conversation_id,
                "error": "工作区状态缺失",
            }));
            continue;
        };
        let target_dir = item.target_ws_dir(spec, root, target_uid);
        match copy_one_conversation(
            options,
            &item.source_ws_dir.join(&item.conversation_id),
            &target_dir,
            &item.target_bucket,
            &item.conversation_id,
            state,
        ) {
            Ok(mut outcome) => {
                // R6-07：副本继承了来源的整份历史，必须把「边界之前归来源账号」写进
                // 归属索引；否则副本一旦被继续使用，mtime 反超来源，历史就翻到副本账号。
                // 失败只记日志（`record_copy_quiet` 签名里没有 `Result`），绝不影响复制结果。
                if let (Some(namespace), Some(store_root)) = (origin_namespace, &origin_store_root)
                {
                    if let Some(new_id) = outcome.get("newId").and_then(Value::as_str) {
                        crate::modules::usage_origin::record_copy_quiet(
                            store_root,
                            namespace,
                            new_id,
                            namespace,
                            &item.conversation_id,
                            origin_boundary_ms,
                            source_uid,
                        );
                    }
                }
                // 项目 key 只在多项目数据仓里回给调用方（VS Code 系报告形状保持不变）。
                if spec.scope == ScopeSegment::ProjectKey {
                    outcome["projectKey"] = json!(item.target_project_key);
                }
                copied.push(outcome);
            }
            Err(error) => errors.push(json!({
                "workspaceHash": item.target_bucket,
                "conversationId": item.conversation_id,
                "error": error,
            })),
        }
    }

    let mut report = json!({
        "sourceUid": source_uid,
        "targetUid": target_uid,
        "copied": copied,
        "backup": backup_root.to_string_lossy(),
    });
    if !errors.is_empty() {
        report["errors"] = json!(errors);
    }
    Ok(report)
}

/// 一条校验通过、目标位置已确定的复制项（[`copy_sessions_in_with`] 阶段 1 的产物）。
struct PreparedCopy {
    /// 源会话所在的工作区目录：`<source>/<client>/<project>/history/<bucket>`。
    source_ws_dir: PathBuf,
    /// 目标项目段（`Uid` 语义 = 目标账号 uid；`ProjectKey` 语义 = 源侧项目 key）。
    target_project_key: String,
    /// 目标会话桶目录名（`Uid` 语义 = 源桶；`ProjectKey` 语义 = 目标该项目下已存在的桶）。
    target_bucket: String,
    conversation_id: String,
}

impl PreparedCopy {
    /// 目标工作区目录：`<target>/<client>/<project>/history/<bucket>`。
    fn target_ws_dir(&self, spec: SessionStoreSpec, root: &Path, target_uid: &str) -> PathBuf {
        scope_history_root_in(spec, root, target_uid, &self.target_project_key)
            .join(&self.target_bucket)
    }

    /// 索引备份目录。
    ///
    /// - `Uid` 语义：`<backup>/<bucket>`（与历史布局逐字一致）；
    /// - `ProjectKey` 语义：`<backup>/<project>/<bucket>`（多项目下同名桶不互相覆盖）。
    fn backup_dir(&self, spec: SessionStoreSpec, backup_root: &Path) -> PathBuf {
        match spec.scope {
            ScopeSegment::Uid => backup_root.join(&self.target_bucket),
            ScopeSegment::ProjectKey => backup_root
                .join(&self.target_project_key)
                .join(&self.target_bucket),
        }
    }

    /// 状态表 / 错误文案里的工作区标识（项目 + 桶）。
    fn state_key(&self) -> String {
        format!("{}/{}", self.target_project_key, self.target_bucket)
    }
}

/// 源侧项目段：`Uid` 语义恒为源账号 uid；`ProjectKey` 语义取条目里的项目 key，缺省回退 uid。
fn source_project_key_of(spec: SessionStoreSpec, item: &CopyItem, source_uid: &str) -> String {
    match spec.scope {
        ScopeSegment::Uid => source_uid.to_string(),
        ScopeSegment::ProjectKey => item
            .project_key
            .as_deref()
            .map(str::trim)
            .filter(|key| is_safe_segment(key))
            .unwrap_or(source_uid)
            .to_string(),
    }
}

/// 目标项目下**已存在的**会话桶。
///
/// 桶是安装级常量（本机所有项目 / 账号共用一个值），因此不能新建：优先沿用源桶，否则取
/// 该项目下按名排序的第一个桶目录。项目不存在 / 没有任何桶时返回 `None`。
fn existing_bucket(history_root: &Path, source_bucket: &str) -> Option<String> {
    let entries = std::fs::read_dir(history_root).ok()?;
    let mut buckets: Vec<String> = entries
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .filter(|name| is_hex32(name))
        .collect();
    buckets.sort();
    if buckets.iter().any(|name| name == source_bucket) {
        return Some(source_bucket.to_string());
    }
    buckets.into_iter().next()
}

/// 「目标账号在这个项目下还没有会话」的统一文案（项目路径可解码时一并给出）。
///
/// 项目 key 解不出路径时只可能是「无项目」桶（项目段退化成了账号 uid）：此时说「未打开项目」
/// 比把 uid 甩给用户更有用。
fn missing_target_project_error(project_keys: &[String]) -> String {
    let labels: Vec<String> = project_keys
        .iter()
        .map(|key| {
            let path = decode_project_key(key);
            if path.is_empty() {
                "未打开项目".to_string()
            } else {
                path
            }
        })
        .collect();
    format!(
        "目标账号在这个项目下还没有会话，请先在 PyCharm 里打开该项目并发一条消息（项目：{}）。",
        labels.join("、")
    )
}

/// 切换 VS Code CodeBuddy 扩展账号，可选「先复制会话」与「把关联会话的新增内容同步过去」。
///
/// 时序（D11 / design §7）：校验目标（账号 / `access_token` / 目录 / db，注定失败的先挡掉、
/// 不关编辑器）→ 关闭（`restart = true` 时；失败即返回，此时编辑器未被触碰）
/// → 复制（本体自带「编辑器未运行」前置，所以关闭必须在它之前）并登记关联
/// → 执行勾选的同步 → 注入 token → 重开。
///
/// 复制与同步都按逐条隔离执行：任一条失败不影响其余条目与后续的账号切换，失败原因写在
/// 各自报告里（`errors` / `linkErrors`），不静默吞掉。若注入失败但已有会话被处理过，
/// 返回明确文案说明残留状态，不做隐式回滚；失败且编辑器是本次我们关闭的 →
/// best-effort 开回来再报错，避免「编辑器关了、会话也没处理成 / 账号也没切成」的双输。
pub fn switch_vscode_ext_with_copy(
    account_id: &str,
    restart: bool,
    items: &[CopyItem],
    sync_selections: &[SyncSelection],
) -> Result<Value, String> {
    if items.is_empty() && sync_selections.is_empty() {
        return vscode_ext::switch_account(account_id, restart);
    }
    let acc =
        account::find_account(account_id).ok_or_else(|| format!("账号不存在: {account_id}"))?;
    let target_uid = get_str(&acc, "uid")
        .ok_or_else(|| "账号缺少 uid，无法定位 VS Code CodeBuddy 插件数据目录".to_string())?;

    // 与 [`vscode_ext::switch_account`] 同序：先把「注定失败」的目标挡在关闭之前，
    // 账号存在但 `access_token` 为空 / 数据目录或 state.vscdb 缺失时不该关掉用户的编辑器。
    vscode_ext::validate_switch_target(account_id)?;

    // Plugin groups span account variants but share one link store; do not let concurrent
    // cn/ai switch flows race their duplicate checks and link registration.
    let _group_ops_lock =
        crate::modules::session_link::try_acquire_client_ops_lock(&SessionPaths::for_vscode_ext())?;

    // 关闭必须在复制与同步之前：两者都会拒绝「VS Code 正在运行」。
    let closed = vscode_ext::close_vscode_for_switch(restart)?;

    let copy_report = if items.is_empty() {
        None
    } else {
        match copy_vscode_sessions(&target_uid, items) {
            Ok(mut report) => {
                // 复制成功即登记「源 ↔ 副本」关联；登记失败不回滚复制，只写进报告（design §5）。
                let link_errors = match ext_data_root() {
                    Some(root) => vscode_session_sync::register_copied_sessions(
                        &root,
                        &SessionPaths::for_vscode_ext(),
                        account::variant_of(&acc),
                        &report,
                    ),
                    None => vec![json!({
                        "error": "未找到 VS Code CodeBuddy 插件数据目录，未能建立会话关联",
                    })],
                };
                if !link_errors.is_empty() {
                    report["linkErrors"] = json!(link_errors);
                }
                Some(report)
            }
            Err(error) => {
                if let Some(closed) = &closed {
                    let _ = vscode_ext::relaunch_closed_editor(closed);
                }
                return Err(error);
            }
        }
    };

    let sync_report = if sync_selections.is_empty() {
        None
    } else {
        // 同步失败不阻断切换：报告形状与成功路径一致，错误挂在 `errors` 里（与 WorkBuddy 同口径）。
        Some(
            match vscode_session_sync::sync_selected(&acc, sync_selections) {
                Ok(report) => report,
                Err(error) => json!({
                    "synced": [],
                    "skipped": [],
                    "errors": [{ "error": error }],
                }),
            },
        )
    };

    let copied = copy_report
        .as_ref()
        .map(|report| count_items(report, "copied"))
        .unwrap_or(0);
    let synced = sync_report
        .as_ref()
        .map(|report| count_items(report, "synced"))
        .unwrap_or(0);

    match vscode_ext::switch_account_after_close(account_id, closed) {
        Ok(mut result) => {
            if let Some(report) = copy_report {
                result["sessionCopy"] = report;
            }
            if let Some(report) = sync_report {
                result["sessionSync"] = report;
            }
            Ok(result)
        }
        Err(error) => {
            let mut done: Vec<String> = Vec::new();
            if copied > 0 {
                done.push(format!("已成功复制 {copied} 个会话到目标账号"));
            }
            if synced > 0 {
                done.push(format!("已成功同步 {synced} 个会话"));
            }
            if done.is_empty() {
                Err(error)
            } else {
                Err(format!(
                    "{error}（注意：{}，但账号切换未完成，请重试切换）",
                    done.join("、")
                ))
            }
        }
    }
}

pub(super) use crate::modules::config::count_items;

/// 复制单个会话：写临时目录 → 原子提交 → 合并目标工作区索引。失败时回退。
///
/// 两条路径由 [`CopyOptions::id_policy`] 决定：
/// - [`CopyIdPolicy::AlwaysNew`]：恒定取新会话 id，并按 [`RemapPlan`] 重写消息 / 请求 id
///   （VS Code 插件侧历史行为）；
/// - [`CopyIdPolicy::KeepUnlessConflict`]：整目录按字节复制、沿用源 id，仅当目标工作区
///   已存在同 id（索引条目或磁盘目录）时重随机会话 id 并改写副本内的旧 id 引用（IDE 侧）。
pub(super) fn copy_one_conversation(
    options: CopyOptions,
    source_dir: &Path,
    target_ws_dir: &Path,
    workspace_hash: &str,
    conversation_id: &str,
    state: &mut WorkspaceState,
) -> Result<Value, String> {
    let source_index = read_json(&source_dir.join("index.json"))
        .ok_or_else(|| "源会话 index.json 缺失或损坏".to_string())?;

    let plan = match options.id_policy {
        CopyIdPolicy::AlwaysNew => Some(RemapPlan::build(&source_index, source_dir)),
        CopyIdPolicy::KeepUnlessConflict => None,
    };
    let new_conversation_id = match &plan {
        Some(_) => gen_hex32(),
        // 冲突判定把「索引条目」与「磁盘目录」都算上：既不覆盖目标既有会话，也不产生重复条目。
        None if conversation_exists(target_ws_dir, &state.current, conversation_id) => {
            unique_hex32(&mut used_conversation_ids(target_ws_dir, &state.current))
        }
        None => conversation_id.to_string(),
    };

    // 1) 先写临时目录，避免出现「半个会话」。
    let tmp_dir = target_ws_dir.join(format!(".tmp-{new_conversation_id}"));
    remove_dir_all_if_exists(&tmp_dir);
    let write_result = match &plan {
        Some(plan) => write_conversation(source_dir, &tmp_dir, &source_index, plan),
        None => copy_conversation_keep(source_dir, &tmp_dir, conversation_id, &new_conversation_id),
    };
    if let Err(error) = write_result {
        remove_dir_all_if_exists(&tmp_dir);
        return Err(format!("写入临时会话目录失败：{error}"));
    }

    // 2) 原子提交：rename tmp → <newId>。
    let final_dir = target_ws_dir.join(&new_conversation_id);
    remove_dir_all_if_exists(&final_dir);
    if let Err(error) = std::fs::rename(&tmp_dir, &final_dir) {
        remove_dir_all_if_exists(&tmp_dir);
        return Err(format!("提交会话目录失败：{error}"));
    }

    // 3) 合并目标工作区索引；失败则回退目录与索引（仅本条）。
    let source_ws_index = read_json(&source_dir.parent().unwrap_or(source_dir).join("index.json"));
    let source_entry = source_ws_index
        .as_ref()
        .and_then(|index| find_conversation(index, conversation_id));
    let entry = conversation_entry(source_entry.as_ref(), &new_conversation_id);

    let before = state.current.clone();
    merge_workspace_index(&mut state.current, entry, &new_conversation_id);
    if let Err(error) = state.persist(options.write_workspace_index_backup) {
        // 回退本条对索引的改动（保留同工作区其它已成功条目的合并结果）。
        state.current = before;
        if state.persist(options.write_workspace_index_backup).is_err() {
            // 二次写入仍失败：磁盘一致性已受损，从备份整体恢复该工作区索引（最后手段）。
            state.restore();
        }
        remove_dir_all_if_exists(&final_dir);
        return Err(format!("合并工作区索引失败：{error}"));
    }

    let messages = match &plan {
        Some(plan) => plan.message_total,
        None => source_index
            .get("messages")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0),
    };
    Ok(json!({
        "workspaceHash": workspace_hash,
        "oldId": conversation_id,
        "newId": new_conversation_id,
        "messages": messages,
    }))
}

/// 「沿用 id」路径的写入器：整目录按字节复制；仅当会话 id 变更（冲突重随机）时，
/// 改写副本内真正引用旧会话 id 的 JSON 文件，其余文件保持字节不变。
pub(super) fn copy_conversation_keep(
    source_dir: &Path,
    dest_dir: &Path,
    old_conversation_id: &str,
    new_conversation_id: &str,
) -> std::io::Result<()> {
    copy_dir_recursive(source_dir, dest_dir)?;
    if old_conversation_id != new_conversation_id {
        rewrite_conversation_id_references(dest_dir, old_conversation_id, new_conversation_id);
    }
    Ok(())
}

/// 目标工作区是否已存在该会话：索引里有条目，或磁盘上有同名会话目录。
pub(super) fn conversation_exists(
    target_ws_dir: &Path,
    index: &Value,
    conversation_id: &str,
) -> bool {
    target_ws_dir.join(conversation_id).is_dir()
        || find_conversation(index, conversation_id).is_some()
}

/// 目标工作区已占用的 id 集合（索引条目 id + 磁盘目录名），供冲突重随机避开。
pub(super) fn used_conversation_ids(target_ws_dir: &Path, index: &Value) -> BTreeSet<String> {
    let mut used: BTreeSet<String> = BTreeSet::new();
    if let Some(conversations) = index.get("conversations").and_then(Value::as_array) {
        for entry in conversations {
            if let Some(id) = entry.get("id").and_then(Value::as_str) {
                used.insert(id.trim().to_string());
            }
        }
    }
    if let Ok(entries) = std::fs::read_dir(target_ws_dir) {
        for entry in entries.flatten() {
            used.insert(entry.file_name().to_string_lossy().to_string());
        }
    }
    used
}
