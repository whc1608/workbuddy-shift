import { invoke } from "@tauri-apps/api/core";
import type { WbVariant } from "../types";
import { DEMO_UNAVAILABLE_MESSAGE, demoModeEnabled } from "../demo-mode";
import { screenshotDemoResponse } from "../screenshot-demo/index";
import { failureFromResponse, structuredFailureOf } from "./failure";

/**
 * 双通道适配层：
 * - 桌面 App（Tauri）：`invoke` 调用 Rust commands
 * - webui（浏览器）：HTTP fetch 调用同源服务（服务端口 / SSH 转发端口均可）
 */

export const DEMO_READ_COMMANDS = new Set([
  "get_status", "get_accounts", "get_codebuddy_cli_status", "get_codebuddy_cn_ide_status", "get_codebuddy_ide_status", "get_vscode_ext_status", "get_jetbrains_status", "list_vscode_sessions", "list_codebuddy_ide_sessions", "list_codebuddy_intl_ide_sessions", "vscode_session_links_preview", "codebuddy_ide_session_links_preview", "codebuddy_intl_ide_session_links_preview", "list_account_sessions", "session_links_preview_cross", "list_session_groups", "get_session_group", "preview_session_group_pair", "get_checkin_status",
  "get_credit_expiry", "get_credit_statistics", "get_auto_checkin_config",
  "get_token_statistics",
  "get_checkin_logs", "get_auto_rotate_config", "rotate_status", "get_rotate_logs",
  "get_github_config", "check_update", "get_launch_at_login_enabled", "switch_progress",
  "get_travel_status", "get_auto_travel_config", "get_rate_limits",
  "get_rate_limit_hook_status", "get_rate_limit_config",
  "storage_scan", "storage_sessions", "storage_trash",
]);

export function isDemoMode(): boolean {
  return demoModeEnabled;
}

export function isWebui(): boolean {
  return typeof window !== "undefined" && !("__TAURI_INTERNALS__" in window);
}

/** Tauri mobile 也注入内部 API；用现有平台 UA 约定把桌面宿主与移动宿主区分开。 */
function isMobilePlatform(): boolean {
  if (typeof navigator === "undefined") return false;
  const ua = navigator.userAgent;
  return (
    /Android|iPhone|iPad|iPod/i.test(ua) ||
    (ua.includes("Macintosh") && navigator.maxTouchPoints > 1)
  );
}

/** 是否为提供桌面专属能力的 Tauri 宿主。 */
export function isDesktop(): boolean {
  return !isWebui() && !isMobilePlatform();
}

/** 会话悬浮窗只由桌面宿主管理，不能经 WebUI 或演示模式访问。 */
export function requireCompanionDesktop(): void {
  if (demoModeEnabled) throw new Error(DEMO_UNAVAILABLE_MESSAGE);
  if (!isDesktop()) throw new Error("会话悬浮窗仅在桌面版中可用");
}

type Route = { method: "GET" | "POST"; path: string };

/** Tauri command → HTTP 路由映射（webui 模式）。 */
const ROUTES: Record<string, Route> = {
  get_status: { method: "GET", path: "/api/status" },
  get_accounts: { method: "GET", path: "/api/accounts" },
  get_codebuddy_cli_status: { method: "GET", path: "/api/codebuddy-cli/status" },
  install_codebuddy_cli_helper: { method: "POST", path: "/api/codebuddy-cli/install-helper" },
  switch_codebuddy_cli_account: { method: "POST", path: "/api/codebuddy-cli/switch" },
  get_codebuddy_cn_ide_status: { method: "GET", path: "/api/codebuddy-cn-ide/status" },
  switch_codebuddy_cn_ide_account: { method: "POST", path: "/api/codebuddy-cn-ide/switch" },
  detect_codebuddy_cn_ide_account: { method: "POST", path: "/api/codebuddy-cn-ide/detect" },
  list_codebuddy_ide_sessions: { method: "GET", path: "/api/codebuddy-cn-ide/sessions" },
  codebuddy_ide_session_links_preview: {
    method: "POST",
    path: "/api/codebuddy-cn-ide/session-links",
  },
  get_vscode_ext_status: { method: "GET", path: "/api/vscode-ext/status" },
  get_jetbrains_status: { method: "GET", path: "/api/jetbrains/status" },
  switch_jetbrains_account: { method: "POST", path: "/api/jetbrains/switch" },
  detect_jetbrains_account: { method: "POST", path: "/api/jetbrains/detect" },
  list_vscode_sessions: { method: "GET", path: "/api/vscode-ext/sessions" },
  switch_vscode_ext_account: { method: "POST", path: "/api/vscode-ext/switch" },
  vscode_session_links_preview: { method: "POST", path: "/api/vscode-ext/session-links" },
  detect_vscode_ext_account: { method: "POST", path: "/api/vscode-ext/detect" },
  get_codebuddy_ide_status: { method: "GET", path: "/api/codebuddy-ide/status" },
  switch_codebuddy_ide_account: { method: "POST", path: "/api/codebuddy-ide/switch" },
  detect_codebuddy_ide_account: { method: "POST", path: "/api/codebuddy-ide/detect" },
  list_codebuddy_intl_ide_sessions: { method: "GET", path: "/api/codebuddy-ide/sessions" },
  codebuddy_intl_ide_session_links_preview: {
    method: "POST",
    path: "/api/codebuddy-ide/session-links",
  },
  delete_account: { method: "POST", path: "/api/delete" },
  update_account_display: { method: "POST", path: "/api/update-account-display" },
  oauth_start: { method: "POST", path: "/api/oauth/start" },
  oauth_status: { method: "POST", path: "/api/oauth/status" },
  import_local: { method: "POST", path: "/api/import-local" },
  export_accounts: { method: "POST", path: "/api/export-accounts" },
  export_accounts_to_path: { method: "POST", path: "/api/export-accounts-to-path" },
  preview_import_accounts: { method: "POST", path: "/api/import/preview" },
  import_accounts: { method: "POST", path: "/api/import" },
  export_accounts_encrypted: { method: "POST", path: "/api/accounts/export-encrypted" },
  import_accounts_encrypted: { method: "POST", path: "/api/accounts/import-encrypted" },
  switch_account: { method: "POST", path: "/api/switch" },
  list_sessions: { method: "GET", path: "/api/sessions" },
  list_account_sessions: { method: "GET", path: "/api/sessions/account" },
  copy_sessions: { method: "POST", path: "/api/sessions/copy" },
  copy_sessions_cross: { method: "POST", path: "/api/sessions/copy-cross" },
  session_links_preview: { method: "POST", path: "/api/session-links/preview" },
  session_links_preview_cross: { method: "POST", path: "/api/session-links/preview-cross" },
  session_pending_writes: { method: "GET", path: "/api/session-writes/pending" },
  session_discard_write: { method: "POST", path: "/api/session-writes/discard" },
  session_sync_cross: { method: "POST", path: "/api/session-sync/cross" },
  list_session_groups: { method: "POST", path: "/api/session-groups/list" },
  get_session_group: { method: "POST", path: "/api/session-groups/detail" },
  preview_session_group_pair: { method: "POST", path: "/api/session-groups/preview" },
  sync_session_group_pair: { method: "POST", path: "/api/session-groups/sync" },
  sync_session_group_unify: { method: "POST", path: "/api/session-groups/unify" },
  sync_session_group_safe_batch: { method: "POST", path: "/api/session-groups/sync-safe" },
  sync_all_session_groups_safe: { method: "POST", path: "/api/session-groups/sync-safe-all" },
  add_session_group_member: { method: "POST", path: "/api/session-groups/add" },
  copy_linked_sessions: { method: "POST", path: "/api/session-groups/copy-linked" },
  vscode_restart_precheck: { method: "POST", path: "/api/vscode-ext/restart-precheck" },
  jetbrains_restart_precheck: { method: "POST", path: "/api/jetbrains/restart-precheck" },
  unlink_session_group_member: { method: "POST", path: "/api/session-groups/unlink" },
  delete_session_group: { method: "POST", path: "/api/session-groups/delete" },
  prune_session_groups: { method: "POST", path: "/api/session-groups/prune-dead" },
  get_checkin_status: { method: "GET", path: "/api/checkin/status" },
  get_credit_expiry: { method: "POST", path: "/api/credits" },
  get_credit_statistics: { method: "GET", path: "/api/credits/stats" },
  get_token_statistics: { method: "GET", path: "/api/token-stats" },
  get_rate_limits: { method: "GET", path: "/api/rate-limits" },
  get_rate_limit_hook_status: { method: "GET", path: "/api/rate-limits/hook-status" },
  install_rate_limit_hook: { method: "POST", path: "/api/rate-limits/install-hook" },
  uninstall_rate_limit_hook: { method: "POST", path: "/api/rate-limits/uninstall-hook" },
  get_rate_limit_config: { method: "GET", path: "/api/rate-limits/config" },
  save_rate_limit_config: { method: "POST", path: "/api/rate-limits/config" },
  checkin: { method: "POST", path: "/api/checkin" },
  checkin_all: { method: "POST", path: "/api/checkin/all" },
  // 国际版每日活跃会话（issue #214）：只有手动触发，没有定时/启动钩子。
  run_daily_activity: { method: "POST", path: "/api/daily-activity/run" },
  get_auto_checkin_config: { method: "GET", path: "/api/checkin/config" },
  save_auto_checkin_config: { method: "POST", path: "/api/checkin/config" },
  get_checkin_logs: { method: "GET", path: "/api/checkin/logs" },
  list_notifications: { method: "GET", path: "/api/notifications" },
  record_notification: { method: "POST", path: "/api/notifications/record" },
  clear_notifications: { method: "POST", path: "/api/notifications/clear" },
  get_travel_status: { method: "GET", path: "/api/travel/status" },
  get_auto_travel_config: { method: "GET", path: "/api/travel/config" },
  save_auto_travel_config: { method: "POST", path: "/api/travel/config" },
  get_auto_rotate_config: { method: "GET", path: "/api/rotate/config" },
  save_auto_rotate_config: { method: "POST", path: "/api/rotate/config" },
  rotate_status: { method: "GET", path: "/api/rotate/status" },
  run_rotate: { method: "POST", path: "/api/rotate/run" },
  get_rotate_logs: { method: "GET", path: "/api/rotate/logs" },
  refresh_account_token: { method: "POST", path: "/api/refresh-token" },
  get_github_config: { method: "GET", path: "/api/update/config" },
  save_github_config: { method: "POST", path: "/api/update/config" },
  check_update: { method: "GET", path: "/api/update/check" },
  switch_progress: { method: "GET", path: "/api/switch/progress" },
  storage_scan: { method: "GET", path: "/api/storage/scan" },
  storage_sessions: { method: "POST", path: "/api/storage/sessions" },
  storage_clean: { method: "POST", path: "/api/storage/clean" },
  storage_trash: { method: "GET", path: "/api/storage/trash" },
  storage_trash_restore: { method: "POST", path: "/api/storage/trash/restore" },
  storage_trash_purge: { method: "POST", path: "/api/storage/trash/purge" },
  storage_reveal: { method: "POST", path: "/api/storage/reveal" },
  storage_revalidate: { method: "POST", path: "/api/storage/revalidate" },
  refresh_official_usage: { method: "POST", path: "/api/refresh-official-usage" },
  save_text_file: { method: "POST", path: "/api/save-text-file" },
  get_settings_sync_status: { method: "GET", path: "/api/settings-sync" },
  save_settings_sync_config: { method: "POST", path: "/api/settings-sync/config" },
  preview_settings_sync: { method: "POST", path: "/api/settings-sync/preview" },
  sync_settings: { method: "POST", path: "/api/settings-sync/apply" },
  list_settings_sync_backups: { method: "GET", path: "/api/settings-sync/backups" },
  restore_settings_sync_backup: { method: "POST", path: "/api/settings-sync/restore" },
  // 设置页「关于 / 设置包」：数据目录只读；导出与导入是同一份 core 白名单校验的包装
  // （导入任一键非法即整体拒绝、零写入，见 core 的 config/configs/bundle.rs）。
  get_data_dir: { method: "GET", path: "/api/data-dir" },
  export_settings_bundle: { method: "GET", path: "/api/settings/export" },
  import_settings_bundle: { method: "POST", path: "/api/settings/import" },
  // 排障：脱敏诊断包全文（只读）。落盘导出是桌面专属（`export_diagnostics`），
  // 网页版拿到文本后自己复制 —— 服务端不往用户机器上写任意路径。
  get_diagnostics_report: { method: "GET", path: "/api/diagnostics" },
  // R6-06：按需解密开关。只读写三个布尔位，不触发解密、不回传 token。
  get_credential_config: { method: "GET", path: "/api/credential-config" },
  save_credential_config: { method: "POST", path: "/api/credential-config" },
};

/**
 * 档位参数只在国际版时下发：缺省（国内版）保持改造前的请求体逐字一致，
 * Tauri 走 `invoke(cmd, undefined)`，HTTP 走无 query 的路径。
 */
export function variantArgs(variant?: WbVariant): Record<string, unknown> | undefined {
  return variant === "ai" ? { variant } : undefined;
}

function queryString(args?: Record<string, unknown>): string {
  if (!args) return "";
  const params = new URLSearchParams();
  for (const [key, value] of Object.entries(args)) {
    if (value === undefined || value === null) continue;
    params.set(key, String(value));
  }
  const text = params.toString();
  return text ? `?${text}` : "";
}

/** 页面当前来源，仅用于错误提示；缺失时返回空串，调用方据此省略括号。 */
function currentOrigin(): string {
  if (typeof window === "undefined") return "";
  const origin = window.location?.origin;
  return typeof origin === "string" ? origin : "";
}

export async function httpCall<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const route = ROUTES[cmd];
  if (!route) throw new Error(`webui 模式暂不支持该操作: ${cmd}`);
  let res: Response;
  try {
    // 相对路径以页面自身来源为基准（服务端口 / SSH 转发端口 / localhost 与
    // 127.0.0.1 混用都自动跟随），不能写死 127.0.0.1:57910。
    const url =
      route.method === "GET"
        ? `${route.path}${queryString(args)}`
        : route.path;
    res = await fetch(url, {
      method: route.method,
      headers: { "Content-Type": "application/json" },
      body: route.method === "POST" ? JSON.stringify(args ?? {}) : undefined,
    });
  } catch {
    const origin = currentOrigin();
    throw new Error(
      origin
        ? `无法连接 workbuddy-shift 服务（${origin}），请先运行 \`workbuddy-shift\``
        : `无法连接 workbuddy-shift 服务，请先运行 \`workbuddy-shift\``,
    );
  }
  const data = await res.json().catch(() => ({}));
  if (!res.ok) {
    // 结构化失败：`message` 与改造前逐字一致，额外带上 `failureKind` / `httpStatus` /
    // `recoveryAction`（见 `./failure`）。`asError()` 读 `message`，因此既有调用点不变。
    throw failureFromResponse(res.status, data);
  }
  return data as T;
}

export async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (demoModeEnabled) {
    if (cmd === "get_credit_statistics" && args?.refresh === true) {
      throw new Error(DEMO_UNAVAILABLE_MESSAGE);
    }
    if (!DEMO_READ_COMMANDS.has(cmd)) throw new Error(DEMO_UNAVAILABLE_MESSAGE);
    return screenshotDemoResponse(cmd, args) as T;
  }
  if (!isWebui()) {
    try {
      return await invoke<T>(cmd, args);
    } catch (e) {
      // Tauri 的 `Err(String)` 可能是 core 结构化失败对象的 JSON 串（如 `failureKind`）。
      // 只在**真的解析出结构**时替换抛出值；普通字符串 / 对象原样放行，
      // 既有 `catch` 分支拿到的形状与改造前完全一致。
      const structured = structuredFailureOf(e);
      throw structured ?? e;
    }
  }
  return httpCall<T>(cmd, args);
}

/** 把 Tauri command / HTTP 抛出的错误统一为 Error。 */
export function asError(e: unknown): string {
  if (typeof e === "string") return e;
  if (e instanceof Error) return e.message;
  return JSON.stringify(e ?? "未知错误");
}