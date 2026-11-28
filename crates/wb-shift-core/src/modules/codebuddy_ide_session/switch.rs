//! 切换复制编排：CN/国际版切换入口、关闭重开与报告计数。

use super::conversations;
use super::copy::*;
use super::paths::*;
use super::*;

/// 切换**国内版** CodeBuddy IDE 账号，可选「先复制会话」与「把关联会话的新增内容同步过去」。
///
/// 语义见 [`switch_ide_with_copy`]。
pub fn switch_codebuddy_cn_ide_with_copy(
    account_id: &str,
    restart: bool,
    items: &[CopyItem],
    sync_selections: &[SyncSelection],
) -> Result<Value, String> {
    switch_ide_with_copy(IdeFlavor::Cn, account_id, restart, items, sync_selections)
}

/// 切换**国际版** CodeBuddy IDE（`CodeBuddy.app`）账号；语义与国内版完全一致
/// （同一套会话存储与关联命名空间，组内 `variant = ai` 区分），见 [`switch_ide_with_copy`]。
pub fn switch_codebuddy_intl_ide_with_copy(
    account_id: &str,
    restart: bool,
    items: &[CopyItem],
    sync_selections: &[SyncSelection],
) -> Result<Value, String> {
    switch_ide_with_copy(IdeFlavor::Intl, account_id, restart, items, sync_selections)
}

/// 切换 CodeBuddy IDE 账号（两个 IDE 共用的编排内核），可选「先复制会话」与「同步关联会话」。
///
/// 时序：空参数直通对应档位的 `switch_account`（行为与现网一致）→ 校验目标
/// （账号 / 档位 / `access_token` / 数据目录；复制则再校验数据根，能读到 uid 时挡源=目标）
/// → 关闭（仅 `restart = true` **且当时在运行**）→ 再读源 uid 并复制、登记关联
/// → 执行勾选的同步 → 注入凭证 → 重启（`restart = true` 时）。
///
/// 复制与同步都逐条隔离：单条失败不影响其余条目与后续切换，失败原因写在报告里
/// （`errors` / `linkErrors`）。复制本身致命失败（如数据根缺失、目标不可写）时：IDE 是本次
/// 由我们关闭的就 best-effort 开回来，再返回错误，避免「IDE 关了、会话也没复制成」的双输。
pub(super) fn switch_ide_with_copy(
    flavor: IdeFlavor,
    account_id: &str,
    restart: bool,
    items: &[CopyItem],
    sync_selections: &[SyncSelection],
) -> Result<Value, String> {
    if items.is_empty() && sync_selections.is_empty() {
        return flavor.switch_account(account_id, restart);
    }
    let acc =
        account::find_account(account_id).ok_or_else(|| format!("账号不存在: {account_id}"))?;
    let target_uid = get_str(&acc, "uid")
        .ok_or_else(|| "账号缺少 uid，无法定位 CodeBuddy IDE 会话目录".to_string())?;

    // 与对应档位的 `switch_account` 同序：先把「注定失败」的目标挡在关闭之前。
    let (_, data_dir) = flavor.validate_switch_target(account_id)?;

    // This store is shared by domestic and international IDE variants. Serialize the
    // complete link/copy/sync sequence namespace-wide, in addition to the legacy variant gate.
    let _group_ops_lock = crate::modules::session_link::try_acquire_client_ops_lock(
        &SessionPaths::for_codebuddy_ide(),
    )?;

    // 复制与同步都拒绝「IDE 正在运行」：手动模式（`restart = false`）直接报错，不假装写入。
    let ide_running = flavor.is_running();
    if !restart && ide_running {
        return Err(
            "检测到 CodeBuddy IDE 正在运行，请先完全退出后再操作，否则写入会被 IDE 覆盖。"
                .to_string(),
        );
    }

    // 新版 IDE 的会话存在 CLI 会话库里（`conversations.user_id`），旧目录树不再更新；
    // 两类条目分开处理：旧格式搬目录，新格式改归属。
    let (db_items, legacy_items): (Vec<CopyItem>, Vec<CopyItem>) = items
        .iter()
        .cloned()
        .partition(|item| item.store.as_deref() == Some(conversations::STORE_DB));

    // 关闭前只做确定失败的检查。源 uid 的权威读取放在关闭之后：IDE 运行时
    // `state.vscdb` 可能被占用，此时回退状态文件会误判成「上次用本工具切过的账号」。
    if !legacy_items.is_empty() {
        if ide_data_root().is_none() {
            return Err(
                "未找到 CodeBuddy IDE 数据目录，无法复制会话。请先打开 CodeBuddy IDE 并登录一次。"
                    .to_string(),
            );
        }
        match flavor.active_uid() {
            Some(uid) if uid == target_uid => {
                return Err("源账号与目标账号相同，无需复制会话".to_string());
            }
            None if !ide_running => {
                return Err(
                    "未检测到 CodeBuddy IDE 当前登录账号，无法定位源会话。请先在 CodeBuddy IDE 中登录后重试。"
                        .to_string(),
                );
            }
            _ => {}
        }
    }
    if !db_items.is_empty() && !conversations::available() {
        return Err(
            "未找到新版 CodeBuddy 会话库（~/.codebuddy/codebuddy.db），无法复制升级后的会话。请先打开 CodeBuddy IDE 或 CLI 一次。"
                .to_string(),
        );
    }

    // 仅当 IDE 当时在跑且本次要重启时才关。`closed` 表示「是我们关掉的」：
    // 失败才负责开回来；`restart = true` 但本来没运行，失败路径不得把没开过的 IDE 拉起来。
    let closed = restart && ide_running;
    if closed {
        eprintln!("[codebuddy-ide-session] closing {}…", flavor.label());
        flavor.close(20)?;
    }

    let copy_report = if legacy_items.is_empty() && db_items.is_empty() {
        None
    } else {
        let mut merged: Option<Value> = None;
        if !legacy_items.is_empty() {
            let (root, source_uid) = match copy_source(flavor) {
                Ok(source) => source,
                Err(error) => {
                    relaunch_if_closed(flavor, closed);
                    return Err(error);
                }
            };
            if source_uid == target_uid {
                relaunch_if_closed(flavor, closed);
                return Err("源账号与目标账号相同，无需复制会话".to_string());
            }
            match copy_codebuddy_ide_sessions(&root, &source_uid, &target_uid, &legacy_items) {
                Ok(mut report) => {
                    // 复制成功即登记「源 ↔ 副本」关联；登记失败不回滚复制，只写进报告。
                    let link_errors = codebuddy_ide_session_sync::register_copied_sessions(
                        &root,
                        &SessionPaths::for_codebuddy_ide(),
                        account::variant_of(&acc),
                        &report,
                    );
                    if !link_errors.is_empty() {
                        report["linkErrors"] = json!(link_errors);
                    }
                    merged = Some(report);
                }
                Err(error) => {
                    relaunch_if_closed(flavor, closed);
                    return Err(error);
                }
            }
        }
        if !db_items.is_empty() {
            // 源 uid 同样在关闭 IDE 之后读：运行时 secret 可能被占用。
            let source_uid = match flavor.active_uid() {
                Some(uid) => uid,
                None => {
                    relaunch_if_closed(flavor, closed);
                    return Err(
                        "未检测到 CodeBuddy IDE 当前登录账号，无法复制升级后的会话。"
                            .to_string(),
                    );
                }
            };
            if source_uid == target_uid {
                relaunch_if_closed(flavor, closed);
                return Err("源账号与目标账号相同，无需复制会话".to_string());
            }
            let ids: Vec<String> = db_items
                .iter()
                .map(|item| item.conversation_id.clone())
                .collect();
            match conversations::migrate_sessions(&source_uid, &target_uid, &ids) {
                Ok(report) => match merged.as_mut() {
                    Some(base) => merge_copy_report(base, report),
                    None => merged = Some(report),
                },
                Err(error) => {
                    relaunch_if_closed(flavor, closed);
                    return Err(error);
                }
            }
        }
        merged
    };

    let sync_report = if sync_selections.is_empty() {
        None
    } else {
        // 同步失败不阻断切换：报告形状与成功路径一致，错误挂在 `errors` 里。
        Some(
            match codebuddy_ide_session_sync::sync_selected_for(flavor, &acc, sync_selections) {
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

    match flavor.inject_and_finish(account_id, &acc, &data_dir, restart) {
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
            let error = if done.is_empty() {
                error
            } else {
                format!(
                    "{error}（注意：{}，但账号切换未完成，请重试切换）",
                    done.join("、")
                )
            };
            // 注入失败且 IDE 是本次我们关闭的：best-effort 开回来再报错，别让用户两头落空。
            if closed {
                match flavor.launch() {
                    Ok(()) => Err(error),
                    Err(launch_error) => Err(format!("{error}\n\n{launch_error}")),
                }
            } else {
                Err(error)
            }
        }
    }
}

pub(super) use crate::modules::config::count_items;

/// 把新版会话库的迁移报告并入旧库复制报告。
///
/// 两份报告形状一致（`copied` / `skipped` / `errors` / `linkErrors`），只是旧库那份可能没有
/// 某些键；这里按「有则追加、无则补上」合并，`sourceUid` / `targetUid` 保留旧库那份。
pub(super) fn merge_copy_report(base: &mut Value, extra: Value) {
    let Value::Object(extra_map) = extra else {
        return;
    };
    for key in ["copied", "skipped", "errors", "linkErrors"] {
        let Some(extra_items) = extra_map.get(key).and_then(Value::as_array) else {
            continue;
        };
        if extra_items.is_empty() {
            continue;
        }
        match base.get_mut(key).and_then(Value::as_array_mut) {
            Some(list) => list.extend(extra_items.iter().cloned()),
            None => {
                base[key] = Value::Array(extra_items.clone());
            }
        }
    }
}

/// 失败兜底：只在本次确实关掉 IDE 时才 best-effort 开回来。
pub(super) fn relaunch_if_closed(flavor: IdeFlavor, closed: bool) {
    if closed {
        let _ = flavor.launch();
    }
}
