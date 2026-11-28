//! 路由表（唯一装配点）。

use super::handlers::*;
use super::host_guard::{local_host_only, local_origin_only, MAX_REQUEST_BODY_BYTES};
use super::state::SharedState;
use super::static_assets::static_handler;
use axum::extract::DefaultBodyLimit;
use axum::middleware;
use axum::routing::{get, post};
use axum::Router;

/// 唯一装配点。`state` 由调用方持有：`serve()` 一份，测试每个用例一份。
pub fn router(state: SharedState) -> Router {
    Router::new()
        .route("/api/status", get(api_status))
        .route("/api/accounts", get(api_accounts))
        .route("/api/codebuddy-cli/status", get(api_codebuddy_cli_status))
        .route(
            "/api/codebuddy-cli/install-helper",
            post(api_codebuddy_cli_install_helper),
        )
        .route("/api/codebuddy-cli/switch", post(api_codebuddy_cli_switch))
        .route(
            "/api/codebuddy-cn-ide/status",
            get(api_codebuddy_cn_ide_status),
        )
        .route(
            "/api/codebuddy-cn-ide/switch",
            post(api_codebuddy_cn_ide_switch),
        )
        .route(
            "/api/codebuddy-cn-ide/detect",
            post(api_codebuddy_cn_ide_detect),
        )
        .route(
            "/api/codebuddy-cn-ide/sessions",
            get(api_codebuddy_cn_ide_sessions),
        )
        .route(
            "/api/codebuddy-cn-ide/session-links",
            post(api_codebuddy_cn_ide_session_links_preview),
        )
        .route("/api/codebuddy-ide/status", get(api_codebuddy_ide_status))
        .route("/api/codebuddy-ide/switch", post(api_codebuddy_ide_switch))
        .route("/api/codebuddy-ide/detect", post(api_codebuddy_ide_detect))
        .route(
            "/api/codebuddy-ide/sessions",
            get(api_codebuddy_intl_ide_sessions),
        )
        .route(
            "/api/codebuddy-ide/session-links",
            post(api_codebuddy_intl_ide_session_links_preview),
        )
        .route("/api/jetbrains/status", get(api_jetbrains_status))
        .route("/api/jetbrains/switch", post(api_jetbrains_switch))
        .route("/api/jetbrains/detect", post(api_jetbrains_detect))
        .route("/api/vscode-ext/status", get(api_vscode_ext_status))
        .route("/api/vscode-ext/sessions", get(api_vscode_ext_sessions))
        .route("/api/vscode-ext/switch", post(api_vscode_ext_switch))
        .route("/api/vscode-ext/detect", post(api_vscode_ext_detect))
        .route(
            "/api/vscode-ext/restart-precheck",
            post(api_vscode_restart_precheck),
        )
        .route(
            "/api/jetbrains/restart-precheck",
            post(api_jetbrains_restart_precheck),
        )
        .route(
            "/api/vscode-ext/session-links",
            post(api_vscode_ext_session_links_preview),
        )
        .route("/api/delete", post(api_delete))
        .route(
            "/api/update-account-display",
            post(api_update_account_display),
        )
        .route("/api/oauth/start", post(api_oauth_start))
        .route("/api/oauth/status", post(api_oauth_status))
        .route("/api/import-local", post(api_import_local))
        .route("/api/export-accounts", post(api_export_accounts))
        .route(
            "/api/export-accounts-to-path",
            post(api_export_accounts_to_path),
        )
        .route("/api/import/preview", post(api_preview_import))
        .route("/api/import", post(api_import))
        .route(
            "/api/accounts/export-encrypted",
            post(api_export_accounts_encrypted),
        )
        .route(
            "/api/accounts/import-encrypted",
            post(api_import_accounts_encrypted),
        )
        .route("/api/switch", post(api_switch))
        .route("/api/switch/progress", get(api_switch_progress))
        .route("/api/sessions", get(api_sessions))
        .route("/api/sessions/account", get(api_account_sessions))
        .route("/api/sessions/copy", post(api_copy_sessions))
        .route("/api/sessions/copy-cross", post(api_copy_sessions_cross))
        .route(
            "/api/session-links/preview",
            post(api_session_links_preview),
        )
        .route(
            "/api/session-links/preview-cross",
            post(api_session_links_preview_cross),
        )
        .route(
            "/api/session-writes/pending",
            get(api_pending_session_writes),
        )
        .route(
            "/api/session-writes/discard",
            post(api_discard_session_write),
        )
        .route("/api/session-sync/cross", post(api_session_sync_cross))
        .route("/api/session-groups/list", post(api_list_session_groups))
        .route("/api/session-groups/detail", post(api_get_session_group))
        .route(
            "/api/session-groups/preview",
            post(api_preview_session_group_pair),
        )
        .route(
            "/api/session-groups/sync",
            post(api_sync_session_group_pair),
        )
        .route(
            "/api/session-groups/unify",
            post(api_sync_session_group_unify),
        )
        .route(
            "/api/session-groups/sync-safe",
            post(api_sync_session_group_safe_batch),
        )
        .route(
            "/api/session-groups/sync-safe-all",
            post(api_sync_all_session_groups_safe),
        )
        .route(
            "/api/session-groups/add",
            post(api_add_session_group_member),
        )
        .route(
            "/api/session-groups/unlink",
            post(api_unlink_session_group_member),
        )
        .route("/api/session-groups/delete", post(api_delete_session_group))
        .route(
            "/api/session-groups/prune-dead",
            post(api_prune_session_groups),
        )
        .route(
            "/api/session-groups/copy-linked",
            post(api_copy_linked_sessions),
        )
        .route("/api/checkin/status", get(api_checkin_status))
        .route("/api/credits", post(api_credits))
        .route("/api/credits/stats", get(api_credit_statistics))
        .route("/api/token-stats", get(api_token_statistics))
        .route("/api/save-text-file", post(api_save_text_file))
        .route(
            "/api/refresh-official-usage",
            post(api_refresh_official_usage),
        )
        .route("/api/rate-limits", get(api_rate_limits))
        .route(
            "/api/rate-limits/hook-status",
            get(api_rate_limit_hook_status),
        )
        .route(
            "/api/rate-limits/install-hook",
            post(api_install_rate_limit_hook),
        )
        .route(
            "/api/rate-limits/uninstall-hook",
            post(api_uninstall_rate_limit_hook),
        )
        .route(
            "/api/rate-limits/config",
            get(api_rate_limit_config).post(api_save_rate_limit_config),
        )
        .route("/api/checkin", post(api_checkin))
        .route("/api/checkin/all", post(api_checkin_all))
        // 国际版每日活跃会话（issue #214）：只有手动触发，没有定时/启动钩子。
        .route("/api/daily-activity/run", post(api_run_daily_activity))
        .route(
            "/api/checkin/config",
            get(api_checkin_config).post(api_save_checkin_config),
        )
        .route("/api/checkin/logs", get(api_checkin_logs))
        .route("/api/notifications", get(api_notifications))
        .route("/api/notifications/record", post(api_record_notification))
        .route("/api/notifications/clear", post(api_clear_notifications))
        .route("/api/travel/status", get(api_travel_status))
        .route(
            "/api/travel/config",
            get(api_travel_config).post(api_save_travel_config),
        )
        .route(
            "/api/rotate/config",
            get(api_rotate_config).post(api_save_rotate_config),
        )
        .route("/api/rotate/status", get(api_rotate_status))
        .route("/api/rotate/run", post(api_rotate_run))
        .route("/api/rotate/logs", get(api_rotate_logs))
        .route("/api/storage/scan", get(api_storage_scan))
        .route("/api/storage/sessions", post(api_storage_sessions))
        .route("/api/storage/clean", post(api_storage_clean))
        .route("/api/storage/trash", get(api_storage_trash))
        .route(
            "/api/storage/trash/restore",
            post(api_storage_trash_restore),
        )
        .route("/api/storage/trash/purge", post(api_storage_trash_purge))
        .route("/api/storage/reveal", post(api_storage_reveal))
        .route("/api/storage/revalidate", post(api_storage_revalidate))
        .route("/api/settings-sync", get(api_settings_sync_status))
        .route(
            "/api/settings-sync/config",
            post(api_save_settings_sync_config),
        )
        .route(
            "/api/settings-sync/preview",
            post(api_preview_settings_sync),
        )
        .route("/api/settings-sync/apply", post(api_sync_settings))
        .route(
            "/api/settings-sync/backups",
            get(api_list_settings_sync_backups),
        )
        .route(
            "/api/settings-sync/restore",
            post(api_restore_settings_sync_backup),
        )
        .route("/api/refresh-token", post(api_refresh_token))
        // 设置页「关于 / 设置包」：数据目录只读回路径；导出与导入走同一份 core 白名单校验
        // （导入任一键非法即整体拒绝、零写入，见 config/configs/bundle.rs）。
        .route("/api/data-dir", get(api_data_dir))
        .route("/api/settings/export", get(api_export_settings_bundle))
        .route("/api/settings/import", post(api_import_settings_bundle))
        // 排障：脱敏诊断包全文（只读）。网页版只在响应里给文本，由前端复制，
        // 没有落盘路由——服务端不写用户机器上的任意路径。
        .route("/api/diagnostics", get(api_diagnostics_report))
        // R6-06：只读写「按需解密」的三个开关位；不触发解密、不返回 token。
        .route(
            "/api/credential-config",
            get(api_credential_config).post(api_save_credential_config),
        )
        .route("/api/update/check", get(api_update_check))
        .route(
            "/api/update/config",
            get(api_update_config).post(api_save_update_config),
        )
        .fallback(static_handler)
        .with_state(state)
        // 请求体上限显式化：与 axum 默认值一致（2 MiB），避免上游默认值变化时
        // 不知不觉放宽；导入/导出走 `fileText` JSON，正常体量远低于此。
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BODY_BYTES))
        // DNS rebinding 防护：只接受本机 Host（Docker 形态可额外配置 WB_SHIFT_ALLOWED_HOSTS，
        // 见 host_guard.rs）。这层挡的是「恶意网页把域名解析到 127.0.0.1 后发起的同源请求」。
        .layer(middleware::from_fn(local_host_only))
        // CSRF：浏览器跨源请求一定带 Origin；只接受与 Host 同源的页面发起的请求。
        .layer(middleware::from_fn(local_origin_only))
}
