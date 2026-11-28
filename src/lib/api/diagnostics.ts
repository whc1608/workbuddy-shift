/**
 * 诊断包：生成脱敏报告，桌面端另可落盘。
 *
 * 报告全部由后端装配（`wb_shift_core::modules::diagnostics`），前端只负责展示与复制：
 * 脱敏逻辑绝不能有第二份实现，否则两份规则的差异就是一次泄漏。
 */
import type { DiagnosticsExportResult, DiagnosticsReport } from "@/lib/types";
import { call } from "./core";

/** 界面语言标签；只用于报告里「界面语言」一行，取不到时传空串（后端写 `unknown`）。 */
function interfaceLocale(): string {
  if (typeof navigator === "undefined") return "";
  return typeof navigator.language === "string" ? navigator.language : "";
}

/**
 * 生成脱敏诊断包（只读）。
 *
 * `force=true` 跳过宿主侧 30s 探测缓存（「重新生成」用）。
 */
export function getDiagnosticsReport(force = false): Promise<DiagnosticsReport> {
  return call<DiagnosticsReport>("get_diagnostics_report", {
    locale: interfaceLocale(),
    force,
  });
}

/**
 * 把诊断包写到用户选定的路径（**桌面专属**；网页版请改用剪贴板复制）。
 *
 * 内容由宿主重新生成，不接受前端回传文本，因此这条命令无法被当成通用写文件通道。
 */
export function exportDiagnostics(path: string, force = false): Promise<DiagnosticsExportResult> {
  return call<DiagnosticsExportResult>("export_diagnostics", {
    path,
    locale: interfaceLocale(),
    force,
  });
}
