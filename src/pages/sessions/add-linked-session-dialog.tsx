import { useEffect, useRef, useState } from "react";
import { Link2, Loader2 } from "lucide-react";
import { toast } from "sonner";
import { DemoAction } from "@/components/demo-action";
import { SessionTreeList } from "@/components/session-tree";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle, DialogTrigger } from "@/components/ui/dialog";
import { displayName } from "@/lib/account-display";
import * as api from "@/lib/api";
import { toggleGroupSelection, toggleSessionSelection, toggleSetKey } from "@/lib/session-selection";
import {
  SESSION_USAGE_ATTRIBUTION,
  SESSION_USAGE_ATTRIBUTION_DETAIL,
  SESSION_USAGE_ATTRIBUTION_TOAST,
} from "@/lib/session-usage-copy";
import type {
  AccountMeta,
  Session,
  SessionGroupClient,
  WbVariant,
} from "@/lib/types";
import { accountVariant, variantLabel } from "@/lib/variant";
import { useAccountsStore } from "@/stores/accounts";
import { LINKED_TREE_MIN_H } from "./constants";
import {
  linkedSessionAccountLabel,
  linkedSessionCopyFailureToast,
  linkedSessionCopyKind,
  linkedSessionEditorLabel,
  linkedSessionEmptySessionsHint,
  linkedSessionNeedsRestartPrecheck,
  linkedSessionScopesTargets,
  linkedSessionTargetHint,
  linkedSessionTargetOptions,
} from "./linked-session";

/**
 * 右上角「新增关联会话」弹窗（与切号弹窗同构）：来源账号 → 会话树勾选 → 目标账号，
 * 一次提交全部勾选。会话树复用 `SessionTreeList`（任务平铺 / 空间按文件夹分组 / 组头三态勾选）。
 *
 * 四个客户端都开放这个入口，但后端入口分三类（见 `./linked-session`）：
 * - WorkBuddy 走 `copySessionsCross`（按账号档位选库，源与目标都可跨档）；
 * - CodeBuddy IDE 走 `copyLinkedSessions` + `variantScope`（两个 IDE 共用一棵会话树，
 *   档位决定关联命名空间；IDE 运行中直接拒绝，不参与「关闭并重开」）；
 * - VS Code / JetBrains 插件走 `copyLinkedSessions`，运行中需「关闭并重开宿主」授权。
 *
 * 三个入口都会在复制后建立关联：不做「只复制不建关联」的降级——关联断了用户会以为同步成功。
 */
export function AddLinkedSessionDialog({ client, variantScope, disabled, onDone, requestRestartConfirm, prefill, onPrefillConsumed }: {
  client: SessionGroupClient;
  /** 仅 CodeBuddy IDE 使用：当前页面选中的档位（其余客户端忽略）。 */
  variantScope?: WbVariant;
  disabled: boolean;
  onDone: () => void;
  /** 插件侧运行中：请求页面统一的「关闭并重开」确认框；用户确认后执行 `run`。 */
  requestRestartConfirm: (actionLabel: string, run: () => Promise<void>) => void;
  /**
   * 外部预填（issue #3 的「重新复制并重建关联」用）：非空时自动打开弹窗并预选来源 / 目标账号；
   * `null` / 省略 = 走原有的手动路径（点页头按钮打开）。
   *
   * 只预选账号，**不预选会话**：复制哪几个会话必须由用户自己勾选，这里不替他决定。
   */
  prefill?: { sourceAccountId: string; targetAccountId: string } | null;
  /** 弹窗关闭时通知调用方清空预填，避免下一次手动打开还带着上一次的账号。 */
  onPrefillConsumed?: () => void;
}) {
  const accounts = useAccountsStore((state) => state.accounts);
  const [open, setOpen] = useState(false);
  const [sourceAccountId, setSourceAccountId] = useState("");
  const [targetAccountId, setTargetAccountId] = useState("");
  const [sessions, setSessions] = useState<Session[]>([]);
  const [sessionsLoading, setSessionsLoading] = useState(false);
  const [sessionsError, setSessionsError] = useState("");
  /** 插件侧数据目录：`null` = 未找到（与「该账号无会话」区分）；`undefined` = 返回未携带该字段。 */
  const [dataRoot, setDataRoot] = useState<string | null | undefined>(undefined);
  /** 勾选 / 展开集合由调用方持有（会话树契约），执行时一次提交全部勾选。 */
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [busy, setBusy] = useState(false);
  /**
   * 账本写入失败（可显式忽略）：`count` 是可忽略的会话数。
   * 忽略是**不可逆**选择——副本永久不含用量与中断记录，不做后台补写，所以必须点一次确认。
   */
  const [auxBlocked, setAuxBlocked] = useState<{ count: number } | null>(null);
  /**
   * 「同时复制工作记忆（MEMORY.md）」（issue #209）：**按操作勾选、默认不勾**。
   * 不勾时后端连源文件都不读，报告里也不会出现 `memory` 字段。
   */
  const [copyMemory, setCopyMemory] = useState(false);
  const requestId = useRef(0);
  /** 复制请求代号。关窗换代后，晚到的结果不得再关闭或改写当前这次弹窗。 */
  const copyRequestId = useRef(0);
  // 账号库就是本机 WorkBuddy 账号，国内版 / 国际版都可以作为来源或目标。
  const workbuddyAccounts = accounts;
  const sourceAccount = workbuddyAccounts.find((account) => account.id === sourceAccountId) ?? null;
  const targetAccount = workbuddyAccounts.find((account) => account.id === targetAccountId) ?? null;
  // 目标候选：排除来源账号；CodeBuddy IDE 下再按当前档位收窄（跨档登记会建到另一个档位的组里，
  // 用户在刚刷新的列表里将看不到这个组）。其余客户端不按档位过滤——跨档复制是有意支持的能力。
  const targetOptions = linkedSessionTargetOptions(workbuddyAccounts, {
    client,
    sourceAccountId,
    variantScope,
  });
  /** 目标账号区的说明：没有可用目标账号时必须给原因（选择器此时是禁用的）。 */
  const targetHint = linkedSessionTargetHint({
    client,
    sourceAccountId,
    targetCount: targetOptions.length,
    variantScope,
  });
  /** 宿主名（成功提示 / 重开失败提示里要指名是哪个客户端）。 */
  const editorLabel = linkedSessionEditorLabel(client);
  /** 弹窗文案用：来源账号的客户端称呼。 */
  const clientAccountLabel = linkedSessionAccountLabel(client);
  /** 本次复制请求要带上的档位：只有 CodeBuddy IDE 需要，其余客户端不传（后端会拒绝多余档位）。 */
  const copyVariantScope = linkedSessionScopesTargets(client) ? variantScope : undefined;

  async function loadSessions(account: AccountMeta) {
    const id = ++requestId.current;
    setSessionsLoading(true);
    setSessionsError("");
    try {
      const result = await api.listAccountSessions(account.id, client);
      if (requestId.current !== id) return;
      // 插件会话没有工作目录概念（只有 workspaceHash）：补空 cwd，交给会话树平铺展示。
      setSessions(result.sessions.map((session) => ({ ...session, cwd: session.cwd ?? "" })));
      setDataRoot(result.dataRoot);
    } catch (error) {
      if (requestId.current !== id) return;
      setSessions([]);
      setSessionsError(api.asError(error));
    } finally {
      if (requestId.current === id) setSessionsLoading(false);
    }
  }
  // 换来源账号：作废在途请求，勾选 / 展开 / 目标账号全部重来，源会话按新账号重新加载。
  function changeSource(id: string) {
    setSourceAccountId(id);
    setTargetAccountId("");
    setSessions([]);
    setSessionsError("");
    setSelected(new Set());
    setExpanded(new Set());
    requestId.current += 1;
    setSessionsLoading(false);
    const account = workbuddyAccounts.find((item) => item.id === id);
    if (account) void loadSessions(account);
  }
  // 预填消费：账号库与读取函数每次渲染都会变，用 ref 取最新值——把它们放进依赖会让
  // 「复制后刷新账号库」把用户已经改过的来源 / 目标覆盖回去。依赖只留 `prefill`
  // （它变成非空 = 有一次新的预填请求）。
  const accountsRef = useRef(workbuddyAccounts);
  accountsRef.current = workbuddyAccounts;
  const loadSessionsRef = useRef(loadSessions);
  loadSessionsRef.current = loadSessions;
  useEffect(() => {
    if (!prefill) return;
    // 与手动打开同一套复位：作废在途请求、清空勾选，再按预填项落座。
    requestId.current += 1;
    copyRequestId.current += 1;
    setSessionsLoading(false);
    setBusy(false);
    setSessions([]);
    setSessionsError("");
    setSelected(new Set());
    setExpanded(new Set());
    setCopyMemory(false);
    setSourceAccountId(prefill.sourceAccountId);
    setTargetAccountId(prefill.targetAccountId);
    setOpen(true);
    const account = accountsRef.current.find((item) => item.id === prefill.sourceAccountId);
    if (account) void loadSessionsRef.current(account);
  }, [prefill]);
  function blockDismissWhileBusy(event: { preventDefault: () => void }) {
    // 遮罩写明执行中不可关闭；只藏关闭按钮挡不住 Esc 和点遮罩。
    if (busy) event.preventDefault();
  }
  /** 关闭弹窗并消费预填：预填只对「这一次」打开有效，下次手动打开回到干净状态。 */
  function closeAndConsume() {
    requestId.current += 1;
    setSessionsLoading(false);
    setOpen(false);
    onPrefillConsumed?.();
  }
  function openChange(next: boolean) {
    if (!next) {
      if (busy) return;
      // 关闭时作废在途的会话读取和复制回写：晚到结果不得写回下一次打开。
      copyRequestId.current += 1;
      closeAndConsume();
      return;
    }
    // 重新打开回到干净状态：来源 / 目标 / 勾选都重选，不复用上一次的会话列表。
    requestId.current += 1;
    copyRequestId.current += 1;
    setSessionsLoading(false);
    setBusy(false);
    setSourceAccountId("");
    setTargetAccountId("");
    setSessions([]);
    setSessionsError("");
    setSelected(new Set());
    setExpanded(new Set());
    // 换来源账号 = 换一份工作记忆：勾选状态回到默认（不勾），避免误带上一份记忆。
    setCopyMemory(false);
    setOpen(true);
  }
  function toggleSession(id: string) {
    setSelected((prev) => toggleSessionSelection(prev, id));
  }
  function toggleGroup(ids: string[]) {
    setSelected((prev) => toggleGroupSelection(prev, ids));
  }
  function toggleExpanded(key: string) {
    setExpanded((prev) => toggleSetKey(prev, key));
  }
  /** 预检：按客户端各自的判定决定是否要「关闭并重开」宿主（失败按不需要处理，后端仍有实时判定兜底）。 */
  async function pluginNeedsRestart(): Promise<boolean> {
    try {
      if (client === "jetbrains") {
        return (await api.jetbrainsRestartPrecheck([targetAccountId])).required === true;
      }
      return (await api.vscodeRestartPrecheck([targetAccountId])).required === true;
    } catch {
      return false;
    }
  }
  async function copyAndLink() {
    if (!sourceAccount || !targetAccountId || sourceAccount.id === targetAccountId || selected.size === 0 || busy) return;
    const kind = linkedSessionCopyKind(client);
    if (kind === "workbuddy") {
      await copyAndLinkWorkbuddy();
      return;
    }
    if (kind === "codebuddyIde") {
      // IDE 不参与「关闭并重开」：运行中时后端直接拒绝并给原因，前端不加确认框。
      await copyAndLinkEditor(false);
      return;
    }
    // 插件侧运行中的写入会被宿主覆盖：先走页面统一的确认框授权「关闭并重开」。
    if (linkedSessionNeedsRestartPrecheck(client) && (await pluginNeedsRestart())) {
      requestRestartConfirm("关闭并复制", () => copyAndLinkEditor(true));
      return;
    }
    await copyAndLinkEditor(false);
  }
  /**
   * 编辑器 / IDE 侧的「复制 + 建立关联」（`copyLinkedSessions`）。
   *
   * `restart` 只有插件侧会传 true（已获用户授权）；CodeBuddy IDE 忽略该参数。
   */
  async function copyAndLinkEditor(restart: boolean) {
    if (!sourceAccount || !targetAccountId || sourceAccount.id === targetAccountId || selected.size === 0 || busy) return;
    const copyId = ++copyRequestId.current;
    setBusy(true);
    try {
      const report = await api.copyLinkedSessions({
        client,
        variantScope: copyVariantScope,
        sourceAccountId: sourceAccount.id,
        targetAccountId,
        sessionIds: [...selected],
        restart,
      });
      const targetName = targetAccount ? displayName(targetAccount) : "目标账号";
      const status = String(report.status);
      const skipped = Array.isArray(report.skipped) ? report.skipped : [];
      const copiedCount = (report.report as { copied?: unknown[] } | undefined)?.copied?.length ?? 0;
      if (status === "linked") {
        toast.success(`已复制 ${copiedCount} 个会话到「${targetName}」并建立关联`, report.restartedEditor ? { description: `已重新打开 ${editorLabel}` } : undefined);
      } else if (status === "copiedUnlinked") {
        // 只复制成功、关联没建成：必须说清「要手动处理」，否则用户会以为同步已经生效。
        toast.warning("会话已复制，但没有建立关联", { description: JSON.stringify(report.linkErrors ?? []) });
      } else {
        const failure = linkedSessionCopyFailureToast(typeof report.error === "string" ? report.error : "");
        toast.error(failure.title, failure.description ? { description: failure.description } : undefined);
      }
      if (skipped.length > 0) toast.info(`有 ${skipped.length} 个会话因内容缺失被跳过`, { description: skipped.map((item) => item.error).join("；") });
      if (report.editorError) toast.warning(`${editorLabel} 未能自动重新打开`, { description: report.editorError });
      // 关窗换代后，结果已经提示过；不要把新打开的弹窗关掉，也不要清掉新一次执行的 busy。
      if (copyRequestId.current !== copyId) {
        if (status === "linked") onDone();
        return;
      }
      if (status === "linked") {
        closeAndConsume();
        onDone();
      }
    } catch (error) {
      // 换代后也要报失败：请求已经发出，静默会让用户以为没执行。
      const failure = linkedSessionCopyFailureToast(api.asError(error));
      toast.error(failure.title, failure.description ? { description: failure.description } : undefined);
    } finally {
      if (copyRequestId.current === copyId) setBusy(false);
    }
  }
  async function copyAndLinkWorkbuddy(ignoreAux = false) {
    if (!sourceAccount || !targetAccountId || sourceAccount.id === targetAccountId || selected.size === 0 || busy) return;
    const copyId = ++copyRequestId.current;
    setBusy(true);
    setAuxBlocked(null);
    try {
      const report = await api.copySessionsCross(sourceAccount.id, targetAccountId, [...selected], ignoreAux, copyMemory);
      const copied = report.copied?.length ?? 0;
      const alreadyLinked = report.alreadyLinked?.length ?? 0;
      const errors = report.errors ?? [];
      const targetName = targetAccount ? displayName(targetAccount) : "目标账号";
      // 同一份报告可能同时含已复制、已存在与失败项：各分支各自提示，互不隐藏。
      if (copied > 0) toast.success(`已复制 ${copied} 个会话到「${targetName}」并建立关联`);
      if (alreadyLinked > 0) toast.info(`「${targetName}」已存在 ${alreadyLinked} 个会话的副本，未重复复制`);
      if (errors.length > 0) {
        const allFailed = copied === 0 && alreadyLinked === 0;
        toast.error(allFailed ? "复制失败" : "部分会话复制失败", {
          description: errors.map((item) => `${sessions.find((session) => session.id === item.id)?.title || item.id}：${item.error}`).join("；"),
        });
      }
      // 恢复重放补完的会话不会出现在 copied 里：不说明的话用户会以为「仍然复制」没生效。
      const recovered = report.recovered?.length ?? 0;
      if (recovered > 0) toast.success(`已补完 ${recovered} 个会话的复制`);
      if (ignoreAux && copied + recovered > 0) {
        toast.info("本次复制未带用量与中断记录", { description: SESSION_USAGE_ATTRIBUTION_TOAST });
      }
      // 账本写入失败是唯一带「显式忽略」出口的失败：不在报告层吞掉，摆在弹窗里让用户选。
      if (report.canIgnoreAux) setAuxBlocked({ count: report.auxFailed?.length ?? errors.length });
      if (report.needsRecovery) toast.error("会话操作待恢复", { description: "已保留恢复所需材料。" });
      // 工作记忆是增强数据：只在勾选时才出现在报告里，失败也只提示，不影响复制结果。
      if (report.memory) {
        if (report.memory.skippedReason) {
          toast.warning("工作记忆没有同步", { description: report.memory.skippedReason });
        } else if (report.memory.appendedLines > 0) {
          toast.info(`已向「${targetName}」追加 ${report.memory.appendedLines} 行工作记忆`);
        }
      }
      if (copied === 0 && alreadyLinked === 0 && recovered === 0 && errors.length === 0 && !report.needsRecovery) {
        toast.error("复制并关联失败", report.error ? { description: report.error } : undefined);
      }
      // 关窗换代后，结果已经提示过；不要把新打开的弹窗关掉，也不要清掉新一次执行的 busy。
      if (copyRequestId.current !== copyId) {
        if (copied > 0 || alreadyLinked > 0) onDone();
        return;
      }
      // 有成功复制，或没有 errors 的已存在副本：关闭并刷新。纯失败留在弹窗。
      if (copied > 0 || recovered > 0 || (alreadyLinked > 0 && errors.length === 0 && !report.canIgnoreAux)) {
        closeAndConsume();
        onDone();
      }
    } catch (error) {
      // 换代后也要报失败：请求已经发出，静默会让用户以为没执行。
      toast.error("复制并关联失败", { description: api.asError(error) });
    } finally {
      if (copyRequestId.current === copyId) setBusy(false);
    }
  }
  // 页脚摘要固定带目标账号名：摘要随勾选数与目标变化，执行前就能确认去向。
  const summarySub = !sourceAccount
    ? "先选择来源账号"
    : !targetAccount
      ? "再选择目标账号"
      : selected.size > 0
        ? `复制并关联到「${displayName(targetAccount)}」`
        : `勾选要复制到「${displayName(targetAccount)}」的会话`;
  return <Dialog open={open} onOpenChange={openChange}>
    <span className="inline-flex">
      <DialogTrigger asChild>
        <Button size="sm" className="shrink-0 bg-brand text-brand-foreground hover:bg-brand/90" disabled={disabled} title="新增关联会话"><Link2 />新增关联会话</Button>
      </DialogTrigger>
    </span>
    <DialogContent showCloseButton={!busy} onEscapeKeyDown={blockDismissWhileBusy} onPointerDownOutside={blockDismissWhileBusy} onInteractOutside={blockDismissWhileBusy} className="flex max-h-[min(90vh,calc(100vh-2rem))] min-w-0 flex-col overflow-hidden">
      <DialogHeader className="shrink-0">
        <DialogTitle>新增关联会话</DialogTitle>
        <DialogDescription>选择一个来源账号的会话，复制到目标账号并建立关联。{client === "codebuddyIde" ? "复制前请先完全退出 CodeBuddy IDE。" : ""}</DialogDescription>
      </DialogHeader>
      {busy && <div className="absolute inset-0 z-50 flex flex-col items-center justify-center gap-3 rounded-lg bg-background/85 backdrop-blur-sm">
        <Loader2 className="size-8 animate-spin text-primary" />
        <p className="text-sm font-medium">正在复制并关联…</p>
        <p className="max-w-xs text-center text-xs text-muted-foreground">正在处理中，请勿关闭窗口</p>
      </div>}
      <div className="flex min-h-0 flex-col gap-3 overflow-x-hidden overflow-y-auto">
        <div className="min-w-0 space-y-1.5"><p className="text-xs font-medium">来源账号</p>
          <Select value={sourceAccountId} onValueChange={changeSource} disabled={busy}>
            <SelectTrigger size="sm" className="w-full min-w-0 bg-background text-xs" aria-label="选择来源账号"><SelectValue placeholder={`选择${clientAccountLabel}`} /></SelectTrigger>
            <SelectContent>{workbuddyAccounts.map((account) => <SelectItem key={account.id} value={account.id} className="text-xs">{displayName(account)} · {variantLabel(accountVariant(account))}</SelectItem>)}</SelectContent>
          </Select>
          {workbuddyAccounts.length === 0 && <p className="text-xs text-muted-foreground">账号库里还没有{clientAccountLabel}。</p>}
        </div>
        <div className="min-w-0 space-y-1.5"><p className="text-xs font-medium">来源会话</p>
          {!sourceAccountId
            ? <p className={`flex items-center justify-center px-3 text-center text-sm text-muted-foreground ${LINKED_TREE_MIN_H}`}>先选择来源账号，再从会话树里勾选要复制的会话。</p>
            : sessionsLoading
              ? <div className={`flex items-center justify-center gap-2 text-sm text-muted-foreground ${LINKED_TREE_MIN_H}`}><Loader2 className="animate-spin" />正在读取会话…</div>
              : sessionsError
                ? <div role="alert" className={`flex flex-col items-center justify-center gap-2 px-3 text-center ${LINKED_TREE_MIN_H}`}><p className="text-sm text-danger">{sessionsError}</p><Button variant="outline" size="sm" onClick={() => { if (sourceAccount) void loadSessions(sourceAccount); }}>重试</Button></div>
                : sessions.length === 0
                  ? <p className={`flex items-center justify-center px-3 text-center text-sm text-muted-foreground ${LINKED_TREE_MIN_H}`}>{linkedSessionEmptySessionsHint({ client, dataRoot })}</p>
                  : <SessionTreeList
                    sessions={sessions}
                    selected={selected}
                    expanded={expanded}
                    onToggleSession={toggleSession}
                    onToggleGroup={toggleGroup}
                    onToggleExpanded={toggleExpanded}
                    className={`max-h-[min(22rem,45vh)] overflow-y-auto pr-1 ${LINKED_TREE_MIN_H}`}
                  />}
        </div>
        {auxBlocked && <div role="alert" className="rounded-md border border-danger/40 bg-danger/5 p-2.5 text-xs">
          <p className="font-medium text-danger">有 {auxBlocked.count} 个会话的用量记录没能写入</p>
          <p className="mt-1 text-muted-foreground">可以仍然复制，但副本不带用量与中断记录：{SESSION_USAGE_ATTRIBUTION}，<span className="font-medium text-foreground">之后不会自动补上</span>。</p>
          <Button variant="outline" size="sm" className="mt-2" onClick={() => void copyAndLinkWorkbuddy(true)} disabled={busy}>仍然复制（不带用量与中断记录）</Button>
        </div>}
        <div className="min-w-0 space-y-1.5"><p className="text-xs font-medium">目标账号</p>
          <Select value={targetAccountId} onValueChange={setTargetAccountId} disabled={busy || !sourceAccountId || targetOptions.length === 0}>
            <SelectTrigger size="sm" className="w-full min-w-0 bg-background text-xs" aria-label="选择目标账号"><SelectValue placeholder={sourceAccountId ? "选择目标账号" : "先选择来源账号"} /></SelectTrigger>
            <SelectContent>{targetOptions.map((account) => <SelectItem key={account.id} value={account.id} className="text-xs">{displayName(account)} · {variantLabel(accountVariant(account))}</SelectItem>)}</SelectContent>
          </Select>
          {/* 没有可用目标账号时选择器是禁用的，必须在这里说清原因（否则用户只看到灰掉的控件）。 */}
          {targetHint && <p className="text-xs text-muted-foreground">{targetHint}</p>}
        </div>
        <div className="min-w-0 space-y-1.5"><p className="text-xs font-medium">附加内容</p>
          <label className="flex min-w-0 items-start gap-2 text-xs">
            <Checkbox
              className="mt-0.5"
              checked={copyMemory}
              disabled={busy || client !== "workbuddy" || !sourceAccountId || !targetAccountId}
              onCheckedChange={(state) => setCopyMemory(state === true)}
              aria-label="同时复制工作记忆（MEMORY.md）"
            />
            <span className="min-w-0">
              <span className="font-medium">同时复制工作记忆（MEMORY.md）</span>
              <span className="mt-0.5 block text-muted-foreground">
                把来源账号积累的长期偏好搬到目标账号：<span className="font-medium text-foreground">只追加目标里没有的行，不覆盖目标已有内容</span>。默认不勾。
              </span>
            </span>
          </label>
        </div>
        {/* R6-08：用量归属是**静态说明**，不再给「连账本一起复制 / 不带用量」这类选项——
            后端复制路径已默认不搬 `session_usage`，让用户选只会造出与后端不一致的预期。 */}
        <div className="min-w-0 space-y-1.5">
          <p className="text-xs font-medium">用量与消耗</p>
          <SessionUsageAttributionNote />
        </div>
      </div>
      <DialogFooter className="shrink-0 sm:justify-between">
        <div className="min-w-0 space-y-0.5">
          <div className="text-sm font-medium">已选 {selected.size} 个会话</div>
          <div className="text-xs text-muted-foreground">{summarySub}</div>
        </div>
        <div className="flex shrink-0 gap-2">
          <Button variant="outline" onClick={() => openChange(false)} disabled={busy}>取消</Button>
          <DemoAction><Button onClick={() => void copyAndLink()} disabled={busy || !sourceAccount || !targetAccountId || sourceAccount.id === targetAccountId || selected.size === 0}>{busy ? <Loader2 className="animate-spin" /> : <Link2 />}{busy ? "处理中…" : "复制并关联"}</Button></DemoAction>
        </div>
      </DialogFooter>
    </DialogContent>
  </Dialog>;
}

/**
 * 「共消耗」归属的静态说明（R6-08）：复制前就把口径说清，复制后用户看到副本的
 * 「共消耗」是 0 或偏少时才不会以为丢了数据。
 *
 * 单独导出是为了让单测能直接渲染这段文案（弹窗关闭状态下 `DialogContent` 不挂载，
 * 无法通过渲染整个弹窗来断言）。文案本体来自 `@/lib/session-usage-copy`。
 */
export function SessionUsageAttributionNote() {
  return <p className="text-xs text-muted-foreground">{SESSION_USAGE_ATTRIBUTION_DETAIL}</p>;
}
