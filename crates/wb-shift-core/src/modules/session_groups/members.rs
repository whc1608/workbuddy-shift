//! 成员与分组增删：复制登记、移除成员、删除分组与兼容账号筛选。

use super::*;

/// 插件侧「复制 + 登记」共享段：复制给定会话到目标账号，并登记关联（无现成组则新建关联组）。
///
/// 供 `add_member`（往已有组加成员）与 `copy_linked_sessions`（从零复制并建组）共用；
/// 只做复制与登记，不做运行门禁与收尾——生命周期窗口由调用方负责。
///
/// `spec` 决定复制与登记落在哪棵同构会话树上（VS Code 插件 / JetBrains 插件），
/// 两处都必须用它，否则会「复制到 JetBrains 目录、登记到 VS Code 表」。
///
/// 复制策略沿用 [`vscode_session::VSCODE_COPY`]（一律新 id + 同步写工作区索引备份）：
/// 用户决策「复制一律新 id」对两个插件型客户端一致——复制体必须与源会话区分。
pub(super) fn plugin_copy_and_register(
    spec: SessionStoreSpec,
    root: &Path,
    paths: &SessionPaths,
    variant: WbVariant,
    source_uid: &str,
    target_uid: &str,
    items: &[CopyItem],
) -> Result<(Value, Vec<Value>), String> {
    let backup_root = paths
        .backup_root()
        .join(spec.backup_kind)
        .join(config::utc_iso());
    let report = vscode_session::copy_sessions_in_with(
        spec,
        vscode_session::VSCODE_COPY,
        root,
        &backup_root,
        source_uid,
        target_uid,
        items,
    )?;
    let link_errors =
        vscode_session_sync::register_copied_sessions_in(spec, root, paths, variant, &report);
    Ok((report, link_errors))
}

/// Copy one member into a compatible saved account and register it in the same namespace.
pub fn add_member(
    client: SessionClient,
    scope: Option<WbVariant>,
    group_id: &str,
    source_member_id: &str,
    target_account_id: &str,
    restart: bool,
) -> Result<Value, String> {
    client.validate_scope(scope)?;
    let paths = client.store_paths();
    let accounts = account::load_accounts_at(&account::accounts_file_in(&paths.store_root));
    let target = accounts
        .iter()
        .find(|account| account.get("id").and_then(Value::as_str) == Some(target_account_id))
        .cloned()
        .ok_or_else(|| "目标账号不存在".to_string())?;
    let target_uid = account::get_str(&target, "uid")
        .map(|uid| uid.trim().to_string())
        .filter(|uid| !uid.is_empty())
        .ok_or_else(|| "目标账号缺少 uid".to_string())?;
    validate_target_compatibility(client, scope, &target)?;

    let store = match session_link::load_store(&paths) {
        StoreState::Ready(store) => store,
        StoreState::Missing => return Err("会话关联组不存在".to_string()),
        // 与列表载荷的 `pruneBlocked`、批量清理的 `Err` 共用同一句（单一来源）。
        StoreState::Unavailable(reason) => {
            return Err(super::maintenance::store_unavailable_reason(&reason));
        }
    };
    let group = store
        .groups
        .iter()
        .find(|group| group.id == group_id && group_is_in_scope(client, scope, group))
        .cloned()
        .ok_or_else(|| "会话关联组不存在或不属于当前客户端".to_string())?;
    let source = group
        .members
        .iter()
        .find(|member| member.member_id == source_member_id && member.state == MemberState::Active)
        .cloned()
        .ok_or_else(|| "来源成员已失效，请刷新会话组后重试".to_string())?;
    if source.uid == target_uid {
        return Err("目标账号已是该会话组成员".to_string());
    }
    if group
        .members
        .iter()
        .any(|member| member.uid == target_uid && member.state == MemberState::Active)
    {
        return Ok(
            json!({ "status": "alreadyLinked", "client": client.as_str(), "groupId": group_id, "targetAccountId": target_account_id }),
        );
    }

    match client {
        SessionClient::Workbuddy => {
            let source_acc = account_by_uid(&accounts, &source.uid)
                .ok_or_else(|| "来源账号已不在本机账号库中".to_string())?;
            let report = session::copy_sessions_cross(
                &source_acc,
                &target,
                std::slice::from_ref(&source.session_id),
                false,
            )?;
            let copied = report
                .get("copied")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            let linked = report
                .get("alreadyLinked")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            Ok(json!({
                "status": if copied > 0 { "linked" } else if linked > 0 { "alreadyLinked" } else { "failed" },
                "client": client.as_str(),
                "groupId": group_id,
                "report": report,
            }))
        }
        SessionClient::CodebuddyIde | SessionClient::VscodeExt | SessionClient::Jetbrains => {
            let variant = scope.unwrap_or_else(|| account::variant_of(&target));
            let _operation_lock = session_link::try_acquire_client_ops_lock(&paths)?;
            // Recheck under the operation lock: the underlying IDE/plugin copy kernels are not idempotent.
            let current = match session_link::load_store(&paths) {
                StoreState::Ready(store) => store,
                StoreState::Missing => return Err("会话关联组不存在".to_string()),
                StoreState::Unavailable(reason) => {
                    return Err(super::maintenance::store_unavailable_reason(&reason))
                }
            };
            let current_group = current
                .groups
                .iter()
                .find(|candidate| {
                    candidate.id == group_id && group_is_in_scope(client, scope, candidate)
                })
                .ok_or_else(|| "会话关联组已变化，请刷新后重试".to_string())?;
            if current_group
                .members
                .iter()
                .any(|member| member.uid == target_uid && member.state == MemberState::Active)
            {
                return Ok(
                    json!({ "status": "alreadyLinked", "client": client.as_str(), "groupId": group_id, "targetAccountId": target_account_id }),
                );
            }
            if client == SessionClient::CodebuddyIde {
                let flavor = ide_flavor(variant);
                if flavor.is_running() {
                    return Err(
                        "检测到 CodeBuddy IDE 正在运行，请先完全退出后再添加关联账号。".to_string(),
                    );
                }
            }
            let root = client
                .data_root()
                .ok_or_else(|| "未找到客户端会话数据目录".to_string())?;
            let spec = client
                .store_spec()
                .ok_or_else(|| "客户端数据仓不可用".to_string())?;
            let location = vscode_session_sync::session_location_and_content(
                spec,
                &root,
                &source.uid,
                &source.session_id,
            )
            .ok_or_else(|| "来源会话索引已不存在，请刷新后重试".to_string())?;
            if !matches!(location.content, ContentState::Ready(_)) {
                return Err("来源会话内容缺失或无法确认，不能复制".to_string());
            }
            let item = CopyItem {
                workspace_hash: location.workspace_hash,
                conversation_id: source.session_id.clone(),
                project_key: Some(location.project_key),
                store: None,
            };
            // 校验（含来源内容）全部完成后，插件侧才打开编辑器生命周期窗口：为注定失败的请求关编辑器是无谓的。
            let guard = prepare_editor_window(client, restart, true, &[target_uid.as_str()])?;
            let result = (|| -> Result<Value, String> {
                let (report, link_errors) = match client {
                    SessionClient::CodebuddyIde => {
                        let backup_root = paths
                            .backup_root()
                            .join(spec.backup_kind)
                            .join(config::utc_iso());
                        let report = codebuddy_ide_session::copy_codebuddy_ide_sessions_in(
                            &root,
                            &backup_root,
                            &source.uid,
                            &target_uid,
                            &[item],
                        )?;
                        let link_errors = codebuddy_ide_session_sync::register_copied_sessions(
                            &root, &paths, variant, &report,
                        );
                        (report, link_errors)
                    }
                    SessionClient::VscodeExt | SessionClient::Jetbrains => {
                        plugin_copy_and_register(
                            spec,
                            &root,
                            &paths,
                            variant,
                            &source.uid,
                            &target_uid,
                            &[item],
                        )?
                    }
                    SessionClient::Workbuddy => unreachable!(),
                };
                let copied = report
                    .get("copied")
                    .and_then(Value::as_array)
                    .map_or(0, Vec::len);
                if copied == 0 {
                    return Ok(
                        json!({ "status": "failed", "client": client.as_str(), "groupId": group_id, "report": report }),
                    );
                }
                if !link_errors.is_empty() {
                    return Ok(json!({
                        "status": "copiedUnlinked",
                        "client": client.as_str(),
                        "groupId": group_id,
                        "report": report,
                        "linkErrors": link_errors,
                    }));
                }
                Ok(
                    json!({ "status": "linked", "client": client.as_str(), "groupId": group_id, "report": report, "linkErrors": [] }),
                )
            })();
            settle_editor_window(guard, result)
        }
    }
}

/// 编辑器 / IDE 客户端：把来源账号的勾选会话复制到目标账号并登记关联（无现成组则新建关联组）。
///
/// 三个客户端（VS Code 插件、JetBrains 插件、CodeBuddy IDE）共用「复制 + 登记」内核，差别只有
/// 数据仓规格与关联命名空间；WorkBuddy 走 `session::copy_sessions_cross`（会话管理页的来源 /
/// 目标账号视图），本入口对它一律拒绝。
///
/// `scope` 只有 CodeBuddy IDE 需要：档位决定运行态判定（关哪个 IDE）与关联组的 `variant`
/// （两个 IDE 共用同一棵会话树，但关联组按档位隔离）。插件侧的存储与地区无关，`scope` 必须为空
/// （见 [`SessionClient::validate_scope`]），`variant` 只作关联组标记，取目标账号自身。
///
/// 运行门禁与「确认后关闭重开」沿用插件侧统一生命周期：全部校验通过后才关闭宿主，失败路径尽力开回。
pub fn copy_linked_sessions(
    client: SessionClient,
    scope: Option<WbVariant>,
    source_account_id: &str,
    target_account_id: &str,
    session_ids: &[String],
    restart: bool,
) -> Result<Value, String> {
    if client == SessionClient::Workbuddy {
        return Err("当前客户端暂不支持新增关联会话".to_string());
    }
    // CodeBuddy IDE 缺档位在此拒绝（`cn` / `ai` 决定复制进哪个关联命名空间）。
    client.validate_scope(scope)?;
    if session_ids.is_empty() {
        return Err("缺少要复制的会话".to_string());
    }
    let paths = client.store_paths();
    let accounts = account::load_accounts_at(&account::accounts_file_in(&paths.store_root));
    let source = accounts
        .iter()
        .find(|item| account::get_str(item, "id").as_deref() == Some(source_account_id))
        .cloned()
        .ok_or_else(|| "来源账号不存在".to_string())?;
    let target = accounts
        .iter()
        .find(|item| account::get_str(item, "id").as_deref() == Some(target_account_id))
        .cloned()
        .ok_or_else(|| "目标账号不存在".to_string())?;
    let source_uid = account::get_str(&source, "uid")
        .map(|uid| uid.trim().to_string())
        .filter(|uid| !uid.is_empty())
        .ok_or_else(|| "来源账号缺少 uid".to_string())?;
    let target_uid = account::get_str(&target, "uid")
        .map(|uid| uid.trim().to_string())
        .filter(|uid| !uid.is_empty())
        .ok_or_else(|| "目标账号缺少 uid".to_string())?;
    if !vscode_session::is_safe_uid(&target_uid) {
        return Err("目标账号 uid 非法，拒绝写入".to_string());
    }
    if source_uid == target_uid {
        return Err("来源账号与目标账号相同，无需复制会话".to_string());
    }
    // CodeBuddy IDE 的档位由调用方显式给出（`validate_scope` 已保证非空）；插件侧取目标账号自身。
    let variant = scope.unwrap_or_else(|| account::variant_of(&target));
    if client == SessionClient::CodebuddyIde {
        // 目标账号必须与当前 IDE 档位一致：登记到别的档位会让新组只在另一个档位的列表里出现。
        // 位置在数据根解析之前：档位不匹配是纯参数错误，不该先被「未找到数据目录」盖住。
        validate_target_compatibility(client, scope, &target)?;
    }
    let root = client
        .data_root()
        .ok_or_else(|| missing_data_root_reason(client))?;
    let spec = client
        .store_spec()
        .ok_or_else(|| "客户端数据仓不可用".to_string())?;
    // 逐条校验来源内容：无内容 / 索引缺失的跳过并如实报告；全部无效才拒绝
    // （不为注定失败的请求关编辑器）。
    let mut items = Vec::with_capacity(session_ids.len());
    let mut skipped: Vec<Value> = Vec::new();
    for session_id in session_ids {
        match vscode_session_sync::session_location_and_content(
            spec,
            &root,
            &source_uid,
            session_id,
        ) {
            Some(location) if matches!(location.content, ContentState::Ready(_)) => {
                items.push(CopyItem {
                    workspace_hash: location.workspace_hash,
                    conversation_id: session_id.clone(),
                    project_key: Some(location.project_key),
                    store: None,
                })
            }
            Some(_) => skipped.push(json!({
                "id": session_id,
                "error": "来源会话内容缺失或无法确认",
            })),
            None => skipped.push(json!({
                "id": session_id,
                "error": "来源会话索引已不存在",
            })),
        }
    }
    if items.is_empty() {
        return Err("勾选的会话都没有可复制的内容".to_string());
    }
    let _operation_lock = session_link::try_acquire_client_ops_lock(&paths)?;
    // CodeBuddy IDE 没有「关闭并重开」生命周期（`prepare_editor_window` 对它返回 `None`）：
    // 运行中的 IDE 会把内存态写回磁盘，覆盖刚复制的会话，因此直接拒绝并说明原因，
    // 不做「假装写入」（与 `add_member` / `run_pair_sync` 同一口径）。
    if client == SessionClient::CodebuddyIde && ide_flavor(variant).is_running() {
        return Err(CODEBUDDY_IDE_ADD_LINKED_RUNNING_HINT.to_string());
    }
    let guard = prepare_editor_window(client, restart, true, &[target_uid.as_str()])?;
    let result = (|| -> Result<Value, String> {
        let (report, link_errors) = match client {
            SessionClient::CodebuddyIde => {
                // 与 `add_member` 同一条链路：复制内核 + 关联命名空间固定 `for_codebuddy_ide`。
                let backup_root = paths
                    .backup_root()
                    .join(spec.backup_kind)
                    .join(config::utc_iso());
                let report = codebuddy_ide_session::copy_codebuddy_ide_sessions_in(
                    &root,
                    &backup_root,
                    &source_uid,
                    &target_uid,
                    &items,
                )?;
                let link_errors = codebuddy_ide_session_sync::register_copied_sessions(
                    &root, &paths, variant, &report,
                );
                (report, link_errors)
            }
            SessionClient::VscodeExt | SessionClient::Jetbrains => plugin_copy_and_register(
                spec,
                &root,
                &paths,
                variant,
                &source_uid,
                &target_uid,
                &items,
            )?,
            SessionClient::Workbuddy => unreachable!("WorkBuddy 已在入口处拒绝"),
        };
        let copied = report
            .get("copied")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        if copied == 0 {
            return Ok(json!({
                "status": "failed",
                "client": client.as_str(),
                "report": report,
                "skipped": skipped,
            }));
        }
        if !link_errors.is_empty() {
            return Ok(json!({
                "status": "copiedUnlinked",
                "client": client.as_str(),
                "report": report,
                "linkErrors": link_errors,
                "skipped": skipped,
            }));
        }
        Ok(json!({
            "status": "linked",
            "client": client.as_str(),
            "report": report,
            "linkErrors": [],
            "skipped": skipped,
        }))
    })();
    settle_editor_window(guard, result)
}

/// Remove one member from a group, keeping the account's session content untouched.
///
/// Group-management data only: no client kernel runs and no session file is written. Pair
/// baselines that reference the member are dropped together with it (`validate_store` requires
/// pair references to stay inside the group); a group left without members is deleted.
pub fn remove_member(
    client: SessionClient,
    scope: Option<WbVariant>,
    group_id: &str,
    member_id: &str,
) -> Result<Value, String> {
    client.validate_scope(scope)?;
    let paths = client.store_paths();
    // 单次存储锁内完成读改写：失败不写盘、不推进 revision（`with_link_store_write` 契约）。
    let (group_removed, remaining) = session_link::with_link_store_write(&paths, |store| {
        remove_member_in(store, client, scope, group_id, member_id)
    })?;
    Ok(json!({
        "status": if group_removed { "groupRemoved" } else { "removed" },
        "client": client.as_str(),
        "groupId": group_id,
        "memberId": member_id,
        "remaining": remaining,
    }))
}

/// [`remove_member`] 的纯变更部分：与存储 IO 分离，便于用固定夹具覆盖不变量。
pub(super) fn remove_member_in(
    store: &mut crate::modules::session_link::LinkStore,
    client: SessionClient,
    scope: Option<WbVariant>,
    group_id: &str,
    member_id: &str,
) -> Result<(bool, usize), String> {
    let group = store
        .groups
        .iter_mut()
        .find(|group| group.id == group_id && group_is_in_scope(client, scope, group))
        .ok_or_else(|| "会话关联组不存在或不属于当前客户端".to_string())?;
    if !group
        .members
        .iter()
        .any(|member| member.member_id == member_id)
    {
        return Err("成员不存在，请刷新后重试".to_string());
    }
    group.members.retain(|member| member.member_id != member_id);
    group
        .pair_bases
        .retain(|pair| !pair.member_ids.iter().any(|id| id == member_id));
    let remaining = group.members.len();
    let group_removed = remaining == 0;
    if group_removed {
        store.groups.retain(|group| group.id != group_id);
    }
    Ok((group_removed, remaining))
}

/// 删除整个会话组：组内所有成员一起解除关联，只解除管理关系，不删除账号内的会话内容。
pub fn delete_group(
    client: SessionClient,
    scope: Option<WbVariant>,
    group_id: &str,
) -> Result<Value, String> {
    client.validate_scope(scope)?;
    let paths = client.store_paths();
    // 单次存储锁内完成读改写：失败不写盘、不推进 revision（`with_link_store_write` 契约）。
    let removed = session_link::with_link_store_write(&paths, |store| {
        delete_group_in(store, client, scope, group_id)
    })?;
    Ok(json!({
        "status": "groupRemoved",
        "client": client.as_str(),
        "groupId": group_id,
        "removed": removed,
    }))
}

/// [`delete_group`] 的纯变更部分：与存储 IO 分离，便于用固定夹具覆盖不变量。
pub(super) fn delete_group_in(
    store: &mut crate::modules::session_link::LinkStore,
    client: SessionClient,
    scope: Option<WbVariant>,
    group_id: &str,
) -> Result<usize, String> {
    let removed = store
        .groups
        .iter()
        .find(|group| group.id == group_id && group_is_in_scope(client, scope, group))
        .map(|group| group.members.len())
        .ok_or_else(|| "会话关联组不存在或不属于当前客户端".to_string())?;
    store.groups.retain(|group| group.id != group_id);
    Ok(removed)
}

/// 列出某账号在指定客户端下的可复制会话（四个宿主共用同一份实现）。
///
/// 四个客户端的数据来源完全不同（数据库 / VS Code 插件仓 / CodeBuddy IDE 仓 / JetBrains
/// 插件仓），但形状一致：`{ sourceUid, sessions: [...], skipped, dataRoot }`。三个同构客户端
/// 的会话文件都按数据根解析，`dataRoot` 为 `null` 表示未找到数据目录（据此区分
/// 「未找到数据目录」与「该账号无会话」）。
pub fn list_account_sessions(client: SessionClient, account: &Value) -> Result<Value, String> {
    let uid = account::get_str(account, "uid")
        .map(|uid| uid.trim().to_string())
        .filter(|uid| !uid.is_empty())
        .ok_or_else(|| "账号缺少 uid".to_string())?;
    match client {
        SessionClient::Workbuddy => Ok(session::list_sessions_for_account(account)),
        // 两个 IDE 共用同一棵会话树，按 uid 定位即可，不需要档位。
        SessionClient::CodebuddyIde => Ok(codebuddy_ide_session::list_codebuddy_ide_sessions(&uid)),
        SessionClient::VscodeExt => Ok(vscode_session::list_vscode_sessions(&uid)),
        SessionClient::Jetbrains => {
            let root = client
                .data_root()
                .ok_or_else(|| missing_data_root_reason(client))?;
            Ok(vscode_session::list_sessions_in_store(
                JETBRAINS_STORE,
                &root,
                &uid,
            ))
        }
    }
}

/// 会话数据根解析失败时的文案：指名「去打开 / 登录哪个客户端」，否则用户只会看到一句无解的话。
pub(super) fn missing_data_root_reason(client: SessionClient) -> String {
    match client {
        SessionClient::Workbuddy => "未找到 WorkBuddy 数据库目录".to_string(),
        SessionClient::CodebuddyIde => {
            "未找到 CodeBuddy IDE 的会话数据目录：请先打开 CodeBuddy IDE 并登录一次后重试。"
                .to_string()
        }
        SessionClient::VscodeExt => "未找到 VS Code CodeBuddy 插件数据目录".to_string(),
        SessionClient::Jetbrains => "未找到 JetBrains CodeBuddy 插件的会话数据目录".to_string(),
    }
}

pub(super) fn compatible_accounts(
    client: SessionClient,
    scope: Option<WbVariant>,
    group: &LinkGroup,
    accounts: &[Value],
) -> Vec<Value> {
    accounts
        .iter()
        .filter(|target| {
            let Some(uid) = account::get_str(target, "uid") else {
                return false;
            };
            if group
                .members
                .iter()
                .any(|member| member.uid == uid && member.state == MemberState::Active)
            {
                return false;
            }
            validate_target_compatibility(client, scope, target).is_ok()
                && (client == SessionClient::Workbuddy || vscode_session::is_safe_uid(&uid))
        })
        .map(account::account_meta)
        .collect()
}
