import { useEffect, useRef, useState } from "react";
import { Check, Copy, Download, Loader2 } from "lucide-react";
import { toast } from "sonner";

import { DemoAction } from "@/components/demo-action";
import { Button } from "@/components/ui/button";
import { CardContent } from "@/components/ui/card";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import * as api from "@/lib/api";
import { diagnosticsFileName, previewPhase } from "@/lib/diagnostics-format";
import { downloadTextFile } from "@/lib/export-download";
import type { DiagnosticsReport } from "@/lib/types";
import { SettingsGroup } from "./rows";

/** 导出文件名：本地时间戳（与报告头部一致），用户连导两次不会互相覆盖。 */
function fileNameFor(report: DiagnosticsReport): string {
  return diagnosticsFileName(report.generatedAt);
}

/** 体积提示：报告是纯文本，几百 KB 已经算很大（日志最多 200 行）。 */
function sizeLabel(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  return `${(bytes / 1024).toFixed(1)} KB`;
}

/**
 * 诊断包：一键得到「能直接贴进 issue 的脱敏信息」。
 *
 * R6-10 体验优化：
 * - 卡片挂载后在空闲时**预热一次**（宿主侧还有 30s 探测缓存），点预览几乎即时；
 * - 「预览」**立即打开弹层**并显示「生成中」，完成后填充，不再「点了等半天才弹」；
 * - 「重新生成」传 `force=true` 跳过缓存。
 */
export function DiagnosticsCard() {
  const [report, setReport] = useState<DiagnosticsReport | null>(null);
  const [loading, setLoading] = useState(false);
  const [exporting, setExporting] = useState(false);
  const [previewOpen, setPreviewOpen] = useState(false);
  const [copied, setCopied] = useState(false);
  const [error, setError] = useState("");
  // 用 ref 防重复：预热与用户点击可能并发触发，不能各发一次。
  const inflight = useRef(false);

  /** 取一次报告（已有则复用，force 时重新生成）。 */
  async function load(force = false): Promise<DiagnosticsReport | null> {
    if (inflight.current) return report;
    if (!force && report) return report;
    inflight.current = true;
    setLoading(true);
    setError("");
    try {
      const next = await api.getDiagnosticsReport(force);
      setReport(next);
      return next;
    } catch (e) {
      setError(api.asError(e));
      return null;
    } finally {
      inflight.current = false;
      setLoading(false);
    }
  }

  // 挂载预热：空闲时拉一次，用户点预览前报告通常已就绪。
  useEffect(() => {
    let cancelled = false;
    const run = () => {
      if (!cancelled) void load(false);
    };
    const idle = typeof window !== "undefined" ? window.requestIdleCallback : undefined;
    let handle: number;
    if (typeof idle === "function") {
      handle = idle(run, { timeout: 2000 });
    } else {
      handle = window.setTimeout(run, 300);
    }
    return () => {
      cancelled = true;
      if (typeof idle === "function" && typeof window.cancelIdleCallback === "function") {
        window.cancelIdleCallback(handle);
      } else {
        window.clearTimeout(handle);
      }
    };
    // 只在挂载时预热一次。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  /** 立即打开弹层（先显示「生成中」），再在后台取报告。 */
  function openPreview() {
    setPreviewOpen(true);
    void load(false);
  }

  /** 复制全文：剪贴板失败必须如实提示，不能静默当成成功。 */
  async function copyAll() {
    if (!report) return;
    try {
      await navigator.clipboard.writeText(report.text);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 2000);
      toast.success("诊断包已复制");
    } catch {
      toast.error("复制失败", {
        description: "浏览器未授予剪贴板权限，请手动选中预览内容后复制。",
      });
    }
  }

  async function exportReport() {
    // 预览正在生成时不要静默取消：能复用已有报告就复用，没有再等一次。
    const loaded = report ?? (await load(false));
    if (!loaded) return;
    // 网页版：浏览器下载（不落服务端磁盘）。
    if (api.isWebui()) {
      downloadTextFile(fileNameFor(loaded), loaded.text);
      toast.success("已开始下载诊断包");
      return;
    }
    setExporting(true);
    try {
      const target = await api.saveFileDialog({
        defaultPath: fileNameFor(loaded),
        filters: [{ name: "文本", extensions: ["txt"] }],
      });
      if (!target) return;
      // 导出复用宿主侧 30s 缓存探测（不强制重新探测）。
      await api.exportDiagnostics(target, false);
      toast.success("诊断包已导出", { description: target });
    } catch (e) {
      toast.error("导出失败", { description: api.asError(e) });
    } finally {
      setExporting(false);
    }
  }

  const busy = loading || exporting;
  const phase = previewPhase(previewOpen, loading, Boolean(report?.text));

  return (
    <SettingsGroup id="settings-diagnostics" title="诊断包">
      <CardContent className="space-y-0 p-0">
        <div className="border-b border-border/50 px-4 py-3 text-xs leading-5 text-muted-foreground sm:px-5">
          反馈问题前先导出一份诊断包：含版本、宿主、客户端安装情况与最近错误日志，
          路径 / 令牌 / 邮箱等已自动脱敏，可直接粘贴到 issue。
        </div>
        <div className="flex flex-wrap gap-2 px-4 py-3 sm:px-5">
          <DemoAction>
            <Button size="sm" variant="outline" disabled={busy} onClick={() => openPreview()}>
              {loading ? <Loader2 className="animate-spin" /> : null}
              预览诊断包
            </Button>
          </DemoAction>
          <DemoAction>
            <Button size="sm" disabled={busy} onClick={() => void exportReport()}>
              {exporting ? <Loader2 className="animate-spin" /> : <Download />}
              导出诊断包
            </Button>
          </DemoAction>
        </div>
        {report && (
          <div className="border-t border-border/50 px-4 py-2 text-[11px] leading-5 text-muted-foreground sm:px-5">
            上次生成：{new Date(report.generatedAt).toLocaleString()} · {sizeLabel(report.bytes)}
            {typeof report.elapsedMs === "number" ? ` · 耗时 ${report.elapsedMs}ms` : ""}
          </div>
        )}
        {error && (
          <div className="border-t border-border/50 px-4 py-2 text-[11px] leading-5 text-destructive sm:px-5">
            {error}
          </div>
        )}
      </CardContent>

      <Dialog open={previewOpen} onOpenChange={setPreviewOpen}>
        <DialogContent className="max-w-3xl">
          <DialogHeader>
            <DialogTitle>诊断包预览</DialogTitle>
            <DialogDescription>
              以下文本就是将要导出的全部内容（已脱敏）。可滚动查看，确认后再复制或导出。
            </DialogDescription>
          </DialogHeader>
          {phase === "loading" || phase === "empty" ? (
            <div className="flex min-h-[160px] items-center justify-center rounded-lg border border-border/60 bg-foreground/[0.04] p-3 text-[11px] text-muted-foreground">
              {phase === "loading" ? (
                <span className="flex items-center gap-2">
                  <Loader2 className="size-3.5 animate-spin" />
                  生成中…
                </span>
              ) : (
                <span>{error || "暂时没有可预览的内容，请点「重新生成」。"}</span>
              )}
            </div>
          ) : (
            <pre className="max-h-[60vh] overflow-auto rounded-lg border border-border/60 bg-foreground/[0.04] p-3 font-mono text-[11px] leading-5 whitespace-pre-wrap break-all">
              {report?.text ?? ""}
            </pre>
          )}
          <DialogFooter className="sm:justify-between">
            <Button variant="outline" size="sm" onClick={() => void load(true)} disabled={loading}>
              {loading ? <Loader2 className="animate-spin" /> : null}
              重新生成
            </Button>
            <Button size="sm" onClick={() => void copyAll()} disabled={!report}>
              {copied ? <Check /> : <Copy />}
              {copied ? "已复制全文" : "复制全文"}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </SettingsGroup>
  );
}
