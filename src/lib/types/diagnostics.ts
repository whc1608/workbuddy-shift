/**
 * 脱敏诊断包的类型（与 `wb-shift-core` 的 `diagnostics` 模块、Tauri command
 * `get_diagnostics_report` / `export_diagnostics` 以及 `GET /api/diagnostics` 对齐）。
 */

/** `GET /api/diagnostics` / `get_diagnostics_report` 的响应。 */
export interface DiagnosticsReport {
  /** 报告全文（纯文本，可直接粘贴进 issue）。 */
  text: string;
  /** 服务端生成时间的毫秒时间戳（存储口径，展示/文件名再转本地时间）。 */
  generatedAt: number;
  /** 文本字节数（UTF-8），用于展示体量。 */
  bytes: number;
  /** 宿主侧生成耗时毫秒（R6-10）。 */
  elapsedMs?: number;
}

/** `export_diagnostics` 的响应（桌面专属；网页版不落盘）。 */
export interface DiagnosticsExportResult {
  ok: boolean;
  /** 实际写入的路径。 */
  path: string;
  /** 写入字节数。 */
  bytes: number;
  /** 写入完成时间的毫秒时间戳。 */
  savedAt: number;
}
