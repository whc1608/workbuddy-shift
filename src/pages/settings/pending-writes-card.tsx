import { useCallback, useEffect, useState } from "react";
import { AlertTriangle, Loader2, ShieldCheck } from "lucide-react";
import { toast } from "sonner";

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
import type { PendingSessionWrite } from "@/lib/types";
import { SettingsGroup } from "./rows";

/**
 * 未完成的写入：同步写到一半被打断时，工具会保留现场，并**挡住这条会话之后的同步**。
 *
 * 这里的动作只有一个语义：**结束这条记录**——不碰正文、不碰目标会话行，只是不再尝试
 * 这次同步。为什么按钮只做这一件事：
 * - 自动恢复已经尽力过了（启动/切号时会先跑一遍），能补完的不会出现在这里；
 * - 「用备份覆盖目标」是另一件更危险的事，不能和「别再挡着我」混在同一个按钮里。
 *
 * 因此确认文案必须把后果说清：目标内容可能不是同步后的状态，备份可能保留。
 */
export function PendingWritesCard() {
  const [items, setItems] = useState<PendingSessionWrite[] | null>(null);
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(false);
  const [confirming, setConfirming] = useState<PendingSessionWrite | null>(null);
  const [busyId, setBusyId] = useState("");

  const load = useCallback(async () => {
    setLoading(true);
    setError("");
    try {
      setItems(await api.pendingSessionWrites());
    } catch (e) {
      setError(api.asError(e));
      setItems([]);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  async function discard(item: PendingSessionWrite) {
    setBusyId(item.operationId);
    try {
      const result = await api.discardSessionWrite(item.operationId);
      toast.success("已结束这次写入", { description: result.note });
      setConfirming(null);
      await load();
    } catch (e) {
      // 后端会说明为什么不允许（例如「正文本就是本次写入的内容，下次切号会自动补完」）。
      toast.error("没有结束", { description: api.asError(e) });
    } finally {
      setBusyId("");
    }
  }

  const count = items?.length ?? 0;

  return (
    <SettingsGroup id="settings-pending-writes" title="未完成的写入">
      <CardContent className="space-y-0 p-0">
        <div className="border-b border-border/50 px-4 py-3 text-xs leading-5 text-muted-foreground sm:px-5">
          切号过程中被中断的同步会留下一份记录，并挡住这条会话之后的同步。结束记录不会修改
          任何会话内容，只是不再尝试这次同步。
        </div>

        {loading && items === null ? (
          <div className="flex items-center gap-2 px-4 py-4 text-xs text-muted-foreground sm:px-5">
            <Loader2 className="size-3.5 animate-spin" aria-hidden />
            正在读取…
          </div>
        ) : error ? (
          <div className="px-4 py-4 text-xs leading-5 text-amber-600 sm:px-5">
            读取未完成的写入失败：{error}
          </div>
        ) : count === 0 ? (
          <div className="flex items-center gap-2 px-4 py-4 text-xs text-muted-foreground sm:px-5">
            <ShieldCheck className="size-3.5" aria-hidden />
            没有未完成的写入。
          </div>
        ) : (
          <ul className="divide-y divide-border/50">
            {items?.map((item) => (
              <li key={item.operationId} className="px-4 py-3 sm:px-5">
                <div className="flex flex-wrap items-start justify-between gap-3">
                  <div className="min-w-0 space-y-1">
                    <div className="flex items-center gap-2 text-[13px] font-medium">
                      <AlertTriangle className="size-3.5 shrink-0 text-amber-500" aria-hidden />
                      <span className="truncate">
                        {item.title ?? "（会话已不在列表里）"}
                      </span>
                      <span className="shrink-0 rounded bg-foreground/5 px-1.5 py-0.5 text-[10px] font-normal text-muted-foreground">
                        {item.kind === "sync" ? "同步" : "复制"}
                      </span>
                    </div>
                    <div className="text-[11px] leading-5 text-muted-foreground">
                      {new Date(item.updatedAt).toLocaleString()} · 目标会话{" "}
                      {item.targetSessionId.slice(0, 8)}…
                    </div>
                    {item.lastError ? (
                      <div className="max-w-2xl text-[11px] leading-5 text-muted-foreground">
                        {item.lastError}
                      </div>
                    ) : null}
                    {!item.discardable ? (
                      <div className="text-[11px] leading-5 text-muted-foreground">
                        复制的中断记录牵涉新建的会话行，请按提示恢复，不要在这里结束。
                      </div>
                    ) : null}
                  </div>
                  <Button
                    variant="outline"
                    size="sm"
                    disabled={!item.discardable || busyId === item.operationId}
                    onClick={() => setConfirming(item)}
                  >
                    {busyId === item.operationId ? (
                      <Loader2 className="size-3.5 animate-spin" aria-hidden />
                    ) : null}
                    结束这次写入
                  </Button>
                </div>
              </li>
            ))}
          </ul>
        )}
      </CardContent>

      <Dialog open={confirming !== null} onOpenChange={(open) => !open && setConfirming(null)}>
        <DialogContent className="max-w-md">
          <DialogHeader>
            <DialogTitle>结束这次写入？</DialogTitle>
            <DialogDescription className="space-y-2 text-xs leading-5">
              <span className="block">
                只会删除这条未完成记录，<strong>不修改任何会话内容</strong>，之后这条会话可以
                重新同步。
              </span>
              <span className="block">
                如果目标内容在中断时可能被改写过，工具会保留备份（不会自动覆盖或删除），
                你可以稍后按备份手动还原。
              </span>
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button variant="ghost" size="sm" onClick={() => setConfirming(null)}>
              取消
            </Button>
            <Button
              size="sm"
              disabled={busyId !== ""}
              onClick={() => confirming && void discard(confirming)}
            >
              {busyId ? <Loader2 className="size-3.5 animate-spin" aria-hidden /> : null}
              结束
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </SettingsGroup>
  );
}
