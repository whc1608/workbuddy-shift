//! WorkBuddy 会话列表、复制与跨账号同步预览。
//!
//! 唯一 Tauri IPC 边界的一部分；只做参数解析、边界校验与错误字符串化，业务逻辑在 wb-shift-core。

use serde_json::json;
use serde_json::Value;
use wb_shift_core::modules::variant::WbVariant;
use wb_shift_core::modules::{account, session, session_groups};

/// GET /api/sessions —— 当前账号的会话列表（`variant` 缺省国内版）。
#[tauri::command]
pub fn list_sessions(variant: Option<String>) -> Value {
    let variant = WbVariant::parse(variant.as_deref());
    match session::current_user_uid(variant) {
        Some(uid) => json!({
            "sessions": session::list_sessions_for_user(variant, &uid),
            "current": uid,
        }),
        None => json!({"sessions": [], "current": Value::Null}),
    }
}

/// GET /api/sessions/account —— 指定账号名下的会话列表（会话管理页源账号视角）。
///
/// `client` 缺省 workbuddy（读 WorkBuddy 数据库）；`vscodeExt` / `jetbrains` 改读各自插件的
/// 会话数据仓（两棵树同构，只有客户端段目录名不同）。
#[tauri::command(rename_all = "camelCase")]
pub fn list_account_sessions(account_id: String, client: Option<String>) -> Result<Value, String> {
    if account_id.trim().is_empty() {
        return Err("缺少 accountId".to_string());
    }
    let account = account::find_account(&account_id).ok_or("账号不存在")?;
    let client = session_groups::SessionClient::parse(client.as_deref().unwrap_or("workbuddy"))?;
    session_groups::list_account_sessions(client, &account)
}

/// POST /api/sessions/copy —— 把勾选会话复制到指定账号（路径 B）。
#[tauri::command(rename_all = "camelCase")]
pub async fn copy_sessions(
    target_account_id: String,
    session_ids: Vec<String>,
    ignore_aux_data: Option<bool>,
    copy_memory: Option<bool>,
) -> Result<Value, String> {
    if target_account_id.trim().is_empty() {
        return Err("缺少 targetAccountId".to_string());
    }
    if session_ids.is_empty() {
        return Err("缺少 sessionIds".to_string());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let target = account::find_account(&target_account_id).ok_or("目标账号不存在")?;
        // 档位取目标账号自身（copy_sessions_for_switch 内部判定）。
        // `copy_memory` 缺省 false：未勾选「同时复制工作记忆」时完全不读源 `MEMORY.md`。
        session::copy_sessions_for_switch_opts(
            &target,
            &session_ids,
            session::CopyOptions::new(
                ignore_aux_data.unwrap_or(false),
                copy_memory.unwrap_or(false),
            ),
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

/// POST /api/sessions/copy-cross —— 跨档复制：源与目标账号都显式给出（会话管理页用）。
///
/// 与 `copy_sessions` 同形，只多一个 `sourceAccountId`：源 uid 取自该账号，
/// 不再从目标档登录态读取，支持国内版 ↔ 国际版。写入侧仍以目标账号为准。
#[tauri::command(rename_all = "camelCase")]
pub async fn copy_sessions_cross(
    source_account_id: String,
    target_account_id: String,
    session_ids: Vec<String>,
    ignore_aux_data: Option<bool>,
    copy_memory: Option<bool>,
) -> Result<Value, String> {
    if source_account_id.trim().is_empty() {
        return Err("缺少 sourceAccountId".to_string());
    }
    if target_account_id.trim().is_empty() {
        return Err("缺少 targetAccountId".to_string());
    }
    if session_ids.is_empty() {
        return Err("缺少 sessionIds".to_string());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let source = account::find_account(&source_account_id).ok_or("源账号不存在")?;
        let target = account::find_account(&target_account_id).ok_or("目标账号不存在")?;
        session::copy_sessions_cross_opts(
            &source,
            &target,
            &session_ids,
            session::CopyOptions::new(
                ignore_aux_data.unwrap_or(false),
                copy_memory.unwrap_or(false),
            ),
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 预览「当前账号 → 目标账号」的关联会话同步项（桌面端 command）。
///
/// 只读：返回 `supported / storeStatus / groups`，其中每组的 `defaultChecked` 与
/// `availableModes` 是前端勾选权限的唯一来源，前端不得自行扩大。
/// `variant` 缺省取目标账号自身档位：来源 uid 从该档位的登录态读取，目标与来源必须在
/// 同一档位内比较（与 `copy_sessions` 的档位约定一致）。与 `POST /api/session-links/preview` 同形。
#[tauri::command(rename_all = "camelCase")]
pub async fn session_links_preview(
    target_account_id: String,
    variant: Option<String>,
) -> Result<Value, String> {
    if target_account_id.trim().is_empty() {
        return Err("缺少 targetAccountId".to_string());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let target = account::find_account(&target_account_id).ok_or("目标账号不存在")?;
        let variant = variant
            .as_deref()
            .map(|raw| WbVariant::parse(Some(raw)))
            .unwrap_or_else(|| account::variant_of(&target));
        session::session_links_preview(variant, &target)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// POST /api/session-links/preview-cross —— 预览「显式来源账号 → 显式目标账号」（跨档支持）。
///
/// 与 `session_links_preview` 同形，多一个 `sourceAccountId`；成员内容按成员自身档位读取。
#[tauri::command(rename_all = "camelCase")]
pub async fn session_links_preview_cross(
    source_account_id: String,
    target_account_id: String,
) -> Result<Value, String> {
    if source_account_id.trim().is_empty() {
        return Err("缺少 sourceAccountId".to_string());
    }
    if target_account_id.trim().is_empty() {
        return Err("缺少 targetAccountId".to_string());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let source = account::find_account(&source_account_id).ok_or("源账号不存在")?;
        let target = account::find_account(&target_account_id).ok_or("目标账号不存在")?;
        session::session_links_preview_cross(&source, &target)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// POST /api/session-sync/cross —— 把显式来源账号的新增同步到显式目标账号（会话管理页）。
///
/// `syncSelections` 与切号弹窗同形（core 校验缺 groupId / previewToken / mode）；
/// 跨档支持：源正文按源档读取，写入侧仍以目标账号为准。
#[tauri::command(rename_all = "camelCase")]
pub async fn session_sync_cross(
    source_account_id: String,
    target_account_id: String,
    sync_selections: Value,
) -> Result<Value, String> {
    if source_account_id.trim().is_empty() {
        return Err("缺少 sourceAccountId".to_string());
    }
    if target_account_id.trim().is_empty() {
        return Err("缺少 targetAccountId".to_string());
    }
    // 入参形状由 core 校验：缺字段/未知模式一律拒绝，这里只做透传。
    let selections = session::parse_sync_selections(Some(&sync_selections))?;
    tauri::async_runtime::spawn_blocking(move || {
        let source = account::find_account(&source_account_id).ok_or("源账号不存在")?;
        let target = account::find_account(&target_account_id).ok_or("目标账号不存在")?;
        session::sync_sessions_cross(&source, &target, &selections)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 列出未完成的会话写入（设置页「未完成的写入」；只读）。
///
/// 操作目录跨档共用（`~/.workbuddy-shift/session-links`），档位只影响标题解析，由 core
/// 按操作自身的 `variant` 处理，因此这里固定用任一档位构造存储路径。
#[tauri::command]
pub async fn session_pending_writes() -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let paths = session::SessionPaths::for_variant(WbVariant::Cn);
        session::pending_session_operations(&paths).map(|items| json!({ "items": items }))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 结束一次未完成的同步写入（只改操作记录，不碰正文与会话行）。
#[tauri::command(rename_all = "camelCase")]
pub async fn session_discard_write(operation_id: String) -> Result<Value, String> {
    if operation_id.trim().is_empty() {
        return Err("缺少 operationId".to_string());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let paths = session::SessionPaths::for_variant(WbVariant::Cn);
        session::discard_unfinished_sync(&paths, &operation_id)
            .and_then(|result| serde_json::to_value(result).map_err(|e| e.to_string()))
    })
    .await
    .map_err(|e| e.to_string())?
}
