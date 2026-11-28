/**
 * 诊断包前端的纯函数（无 DOM / 无 Tauri 依赖，便于 vitest 直接断言）。
 *
 * 关键口径（R6-04）：文件名与报告头部的「本地时间」必须一致，因此**不用**
 * `toISOString()`（那是 UTC），而是按显式时区偏移手工 pad。
 */

function pad2(value: number): string {
  return value.toString().padStart(2, "0");
}

/** 本机当前相对 UTC 的偏移（秒，东八区为 +28800）。 */
export function localOffsetSeconds(at: number = Date.now()): number {
  return -new Date(at).getTimezoneOffset() * 60;
}

/**
 * 生成导出文件名：`workbuddy-shift-diagnostics-YYYY-MM-DD-HH-mm.txt`。
 *
 * 时间按 `offsetSeconds` 本地化后再取（与报告头部同口径）。单测传固定 `generatedAt` +
 * 固定 offset 断言，不依赖运行机器时区。
 */
export function diagnosticsFileName(
  generatedAt: number,
  offsetSeconds: number = localOffsetSeconds(generatedAt),
): string {
  const shifted = new Date(generatedAt + offsetSeconds * 1000);
  const year = shifted.getUTCFullYear();
  const month = pad2(shifted.getUTCMonth() + 1);
  const day = pad2(shifted.getUTCDate());
  const hour = pad2(shifted.getUTCHours());
  const minute = pad2(shifted.getUTCMinutes());
  return `workbuddy-shift-diagnostics-${year}-${month}-${day}-${hour}-${minute}.txt`;
}

/** 预览弹层当前应显示什么。 */
export type PreviewPhase = "closed" | "loading" | "ready" | "empty";

/**
 * 预览弹层的展示阶段：
 * - 未打开 → `closed`；
 * - 已有文本 → `ready`（先显示旧文本，重新生成时也不闪白）；
 * - 正在生成且没文本 → `loading`（R6-10：弹层立即打开并显示「生成中」）；
 * - 打开但既没文本也没在生成 → `empty`（生成失败且从未成功过）。
 */
export function previewPhase(
  open: boolean,
  loading: boolean,
  hasText: boolean,
): PreviewPhase {
  if (!open) return "closed";
  if (hasText) return "ready";
  if (loading) return "loading";
  return "empty";
}

/** 是否需要发起一次请求：`force` 或本地还没有报告时。 */
export function shouldFetch(
  hasReport: boolean,
  loading: boolean,
  force: boolean,
): boolean {
  if (loading) return false;
  return force || !hasReport;
}
