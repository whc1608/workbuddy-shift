//! sessions 域的 HTTP handler。

use axum::extract::rejection::JsonRejection;
use axum::extract::RawQuery;
use axum::http::StatusCode;
use axum::response::Response;
use axum::Json;

use crate::api::*;
use serde_json::json;
use wb_shift_core::modules::variant::WbVariant;
use wb_shift_core::modules::*;

/// GET /api/sessions —— 当前账号的会话列表（query 可选 `variant`，缺省国内版）。
pub(crate) async fn api_sessions(RawQuery(query): RawQuery) -> Response {
    let variant = query_variant(query.as_deref());
    match session::current_user_uid(variant) {
        Some(uid) => json_ok(json!({
            "sessions": session::list_sessions_for_user(variant, &uid),
            "current": uid,
            "variant": variant.as_str(),
        })),
        None => json_ok(json!({ "sessions": [], "current": null, "variant": variant.as_str() })),
    }
}

/// GET /api/sessions/account —— 指定账号名下的会话列表（会话管理页的源账号视角）。
///
/// 与 `GET /api/sessions` 同形，但来源是显式账号（`accountId` query）：账号不存在
/// 返回 400；账号缺 uid 时返回空列表 + `current: null`（与现有容错一致）。
pub(crate) async fn api_account_sessions(RawQuery(query): RawQuery) -> Response {
    let account_id = query_param(query.as_deref(), "accountId").unwrap_or_default();
    let client = query_param(query.as_deref(), "client").unwrap_or_else(|| "workbuddy".to_string());
    let Some(account) = account::find_account(&account_id) else {
        return json_err("账号不存在".to_string(), StatusCode::BAD_REQUEST);
    };
    let client = match session_groups::SessionClient::parse(&client) {
        Ok(client) => client,
        Err(error) => return json_err(error, StatusCode::BAD_REQUEST),
    };
    match session_groups::list_account_sessions(client, &account) {
        Ok(value) => json_ok(value),
        Err(error) => json_err(error, StatusCode::BAD_REQUEST),
    }
}

pub(crate) async fn api_copy_sessions(
    body: Result<Json<CopySessionsRequest>, JsonRejection>,
) -> Response {
    let request = match parse_body(body) {
        Ok(request) => request,
        Err(rejection) => return invalid_body(rejection),
    };
    let Some(target) = account::find_account(&request.target_account_id) else {
        return json_err("目标账号不存在".to_string(), StatusCode::BAD_REQUEST);
    };
    // 档位取目标账号自身（源 uid 也从该档位的登录态读）。
    let variant = account::variant_of(&target);
    // 与桌面端同形：直接返回 core 的复制报告（copied / alreadyLinked / errors / needsRecovery）。
    let mut report = match session::copy_sessions_for_switch_opts(
        &target,
        &request.session_ids,
        session::CopyOptions::new(request.ignore_aux_data, request.copy_memory),
    ) {
        Ok(report) => report,
        Err(error) => {
            return json_err(error, StatusCode::BAD_REQUEST);
        }
    };
    report["variant"] = json!(variant.as_str());
    json_ok(report)
}

/// POST /api/sessions/copy-cross —— 跨档复制：源与目标账号都显式给出（会话管理页用）。
///
/// 与 `POST /api/sessions/copy` 同形，多一个 `sourceAccountId`：源 uid 取自该账号，
/// 不再从目标档登录态读取，支持国内版 ↔ 国际版。报告与桌面端 `copy_sessions_cross`
/// 同形（含 `sourceVariant` / `targetVariant`）。
pub(crate) async fn api_copy_sessions_cross(
    body: Result<Json<CopySessionsCrossRequest>, JsonRejection>,
) -> Response {
    let request = match parse_body(body) {
        Ok(request) => request,
        Err(rejection) => return invalid_body(rejection),
    };
    let Some(source) = account::find_account(&request.source_account_id) else {
        return json_err("源账号不存在".to_string(), StatusCode::BAD_REQUEST);
    };
    let Some(target) = account::find_account(&request.target_account_id) else {
        return json_err("目标账号不存在".to_string(), StatusCode::BAD_REQUEST);
    };
    let target_variant = account::variant_of(&target);
    let mut report = match session::copy_sessions_cross_opts(
        &source,
        &target,
        &request.session_ids,
        session::CopyOptions::new(request.ignore_aux_data, request.copy_memory),
    ) {
        Ok(report) => report,
        Err(error) => {
            return json_err(error, StatusCode::BAD_REQUEST);
        }
    };
    // 与同档端点同形：`variant` 恒为目标档（前端按目标账号渲染）。
    report["variant"] = json!(target_variant.as_str());
    json_ok(report)
}

/// POST /api/session-links/preview —— 预览当前账号 → 目标账号的关联会话同步项。
///
/// 与桌面端 `session_links_preview` 同形：直接返回 core 的只读预览（`supported` /
/// `storeStatus` / `groups`），每组的 `defaultChecked` 与 `availableModes` 是前端的
/// 勾选权限来源。`variant` 缺省取目标账号自身档位。
pub(crate) async fn api_session_links_preview(
    body: Result<Json<SessionLinksPreviewRequest>, JsonRejection>,
) -> Response {
    let request = match parse_body(body) {
        Ok(request) => request,
        Err(rejection) => return invalid_body(rejection),
    };
    if request.target_account_id.trim().is_empty() {
        return json_err("缺少 targetAccountId".to_string(), StatusCode::BAD_REQUEST);
    }
    let Some(target) = account::find_account(&request.target_account_id) else {
        return json_err("目标账号不存在".to_string(), StatusCode::BAD_REQUEST);
    };
    let variant = match request.variant.as_deref() {
        Some(raw) => WbVariant::parse(Some(raw)),
        None => account::variant_of(&target),
    };
    match session::session_links_preview(variant, &target) {
        Ok(report) => json_ok(report),
        Err(error) => json_err(error, StatusCode::BAD_REQUEST),
    }
}

/// POST /api/session-links/preview-cross —— 预览「显式来源账号 → 显式目标账号」（跨档支持）。
///
/// 与 `POST /api/session-links/preview` 同形，多一个 `sourceAccountId`；
/// 成员内容按成员自身档位读取（跨档组的源读源档、目标读目标档）。
pub(crate) async fn api_session_links_preview_cross(
    body: Result<Json<SessionLinksPreviewCrossRequest>, JsonRejection>,
) -> Response {
    let request = match parse_body(body) {
        Ok(request) => request,
        Err(rejection) => return invalid_body(rejection),
    };
    if request.source_account_id.trim().is_empty() {
        return json_err("缺少 sourceAccountId".to_string(), StatusCode::BAD_REQUEST);
    }
    let Some(source) = account::find_account(&request.source_account_id) else {
        return json_err("源账号不存在".to_string(), StatusCode::BAD_REQUEST);
    };
    let Some(target) = account::find_account(&request.target_account_id) else {
        return json_err("目标账号不存在".to_string(), StatusCode::BAD_REQUEST);
    };
    match session::session_links_preview_cross(&source, &target) {
        Ok(report) => json_ok(report),
        Err(error) => json_err(error, StatusCode::BAD_REQUEST),
    }
}

/// POST /api/session-sync/cross —— 把显式来源账号的新增同步到显式目标账号（跨档支持）。
///
/// `syncSelections` 与切号弹窗同形（core 校验缺 groupId / previewToken / mode）。
pub(crate) async fn api_session_sync_cross(
    body: Result<Json<SessionSyncCrossRequest>, JsonRejection>,
) -> Response {
    let request = match parse_body(body) {
        Ok(request) => request,
        Err(rejection) => return invalid_body(rejection),
    };
    let Some(source) = account::find_account(&request.source_account_id) else {
        return json_err("源账号不存在".to_string(), StatusCode::BAD_REQUEST);
    };
    let Some(target) = account::find_account(&request.target_account_id) else {
        return json_err("目标账号不存在".to_string(), StatusCode::BAD_REQUEST);
    };
    let selections = match session::parse_sync_selections(request.sync_selections.as_ref()) {
        Ok(selections) => selections,
        Err(error) => return json_err(error, StatusCode::BAD_REQUEST),
    };
    match session::sync_sessions_cross(&source, &target, &selections) {
        Ok(report) => json_ok(report),
        Err(error) => json_err(error, StatusCode::BAD_REQUEST),
    }
}

/// GET /api/session-writes/pending —— 未完成的会话写入（只读）。
///
/// 操作目录跨档共用，档位只影响标题解析（core 内按操作自身的 `variant` 处理），
/// 因此这里固定用任一档位构造存储路径。
pub(crate) async fn api_pending_session_writes() -> Response {
    let paths = session::SessionPaths::for_variant(WbVariant::Cn);
    match session::pending_session_operations(&paths) {
        Ok(items) => json_ok(json!({ "items": items })),
        Err(error) => json_err(error, StatusCode::BAD_REQUEST),
    }
}

/// POST /api/session-writes/discard —— 结束一次未完成的同步写入。
///
/// 只改操作记录：不碰正文、不碰目标会话行；目标内容可能被改写时保留备份并如实返回。
pub(crate) async fn api_discard_session_write(
    body: Result<Json<SessionWriteDiscardRequest>, JsonRejection>,
) -> Response {
    let request = match parse_body(body) {
        Ok(request) => request,
        Err(rejection) => return invalid_body(rejection),
    };
    if request.operation_id.trim().is_empty() {
        return json_err("缺少 operationId".to_string(), StatusCode::BAD_REQUEST);
    }
    let paths = session::SessionPaths::for_variant(WbVariant::Cn);
    match session::discard_unfinished_sync(&paths, &request.operation_id) {
        Ok(result) => json_ok(json!(result)),
        Err(error) => json_err(error, StatusCode::BAD_REQUEST),
    }
}
