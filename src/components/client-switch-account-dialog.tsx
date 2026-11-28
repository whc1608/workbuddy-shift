import { useEffect, useMemo, useState } from "react";
import { Loader2 } from "lucide-react";

import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Switch } from "@/components/ui/switch";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { busyCopy, emptyStateHint, notLoggedInHint, statusNoticeFor } from "@/components/client-switch-account-copy";
import { reportClientSwitchResult, type ClientSwitchResult } from "@/components/client-switch-account-result";
import type { ClientSwitchClient, ClientSwitchStatus } from "@/components/client-switch-account-types";
import { buildGroups, SessionCopyTab } from "@/components/session-copy-tab";
import type { SessionLinksMeta } from "@/components/session-link-shared";
import { SessionSyncSection } from "@/components/session-sync-section";
import * as api from "@/lib/api";
import { displayName } from "@/lib/account-display";
import { toggleGroupSelection, toggleSessionSelection, toggleSetKey } from "@/lib/session-selection";
import { switchAccountSummary } from "@/lib/switch-account-summary";
import type {
  AccountMeta,
  SessionLinkPreviewGroup,
  SessionSyncSelection,
  VscodeSession,
  VscodeSessionRef,
  WbVariant,
} from "@/lib/types";
import { cn } from "@/lib/utils";
import { TAB_TRIGGER_CLASS, tabCount } from "@/components/switch-account-tabs";
import { booleanPreference, readPreference, writePreference } from "@/lib/persisted-preference";
import { variantUsesIntlCodebuddyIde } from "@/lib/variant";

export type { ClientSwitchClient, ClientSwitchStatus } from "@/components/client-switch-account-types";

interface Props {
  client: ClientSwitchClient;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** 目标账号 */
  account: AccountMeta | null;
  /** 目标客户端状态（用于渲染空态与运行中提示）。 */
  status?: ClientSwitchStatus | null;
  /**
   * 当前档位：仅 CodeBuddy IDE 通道使用。
   *
   * 国内版（`CodeBuddy CN.app`）与国际版（`CodeBuddy.app`）共用同一套会话存储与后端语义，
   * 因此组件只做这一处分流，两个 tab、摘要与空态保持一份。
   */
  variant?: WbVariant;
  /** 切换完成后刷新列表 */
  onDone?: () => void;
}

/** 自动关闭并重开 VS Code 的开关（缺省开启，与 `workbuddy-shift.compact` 同风格）。 */
const AUTO_RESTART_PREFERENCE = booleanPreference(
  "workbuddy-shift.vscodeExt.autoRestart",
  "workbuddy-shift:vscode-auto-restart-changed",
  true,
);

/** 关闭并重开开关的持久化读取；localStorage 不可用时按默认开启处理。 */
function readAutoRestart(): boolean {
  return readPreference(AUTO_RESTART_PREFERENCE);
}

/**
 * 客户端切号弹窗（VS Code 插件 / CodeBuddy IDE 共用的参数化实现）。
 *
 * 两个客户端同形：关联会话 / 复制会话两个 tab、底部摘要与结果反馈一致，差异只有三处：
 * 1. 状态三态与空态文案（VS Code 多「未装扩展」，且多了自动关闭并重开开关）——见
 *    `client-switch-account-copy.tsx`；
 * 2. 数据源与切换命令（VS Code 插件 / CodeBuddy IDE 国内版 / 国际版）；
 * 3. 生效说明与结果 toast（VS Code 区分「已重开 / 重开失败 / 本来没运行」；IDE 固定关闭并重开）
 *    ——见 `client-switch-account-result.ts`。
 *
 * 对外 props 由两个薄适配层（`vscode-switch-account-dialog.tsx` /
 * `codebuddy-ide-switch-account-dialog.tsx`）保持原样。
 */
export function ClientSwitchAccountDialog({
  client,
  open,
  onOpenChange,
  account,
  status,
  variant,
  onDone,
}: Props) {
  const isVscode = client === "vscode";
  /** CodeBuddy IDE 国际版档位：会话列表 / 关联预览 / 切换接口都走 `codebuddy-ide` 通道。 */
  const intl = client === "codebuddy-ide" && variant !== undefined && variantUsesIntlCodebuddyIde(variant);
  const [sessions, setSessions] = useState<VscodeSession[]>([]);
  const [sourceUid, setSourceUid] = useState<string | null>(null);
  /** 客户端数据根目录：`null` = 未找到（与「有目录但无会话」区分）；`undefined` = 后端未返回该字段。 */
  const [dataRoot, setDataRoot] = useState<string | null | undefined>(undefined);
  const [loadingSessions, setLoadingSessions] = useState(false);
  const [copyEnabled, setCopyEnabled] = useState(false);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  /** 展开的工作区分组（默认全部展开，会话较少）。 */
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set());
  /** 自动关闭并重开 VS Code：默认开启，持久化到 localStorage。 */
  const [autoRestart, setAutoRestart] = useState<boolean>(() => (isVscode ? readAutoRestart() : true));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  /** 当前 tab：与 WorkBuddy 切换弹窗同一顺序与默认值（关联会话在前）。 */
  const [tab, setTab] = useState<"links" | "copy">("links");
  /** 关联会话区块上报的状态（tab 徽标 / 未登录时隐藏 tab）。 */
  const [linksMeta, setLinksMeta] = useState<SessionLinksMeta | null>(null);
  /** 勾选提交给后端的同步选择（绑定预览凭据，执行前后端会重新校验）。 */
  const [syncSelections, setSyncSelections] = useState<SessionSyncSelection[]>([]);
  /** 已勾选的关联会话（结果反馈里回显会话名）。 */
  const [syncGroups, setSyncGroups] = useState<SessionLinkPreviewGroup[]>([]);

  function toggleAutoRestart(next: boolean) {
    setAutoRestart(next);
    writePreference(AUTO_RESTART_PREFERENCE, next);
  }

  // 打开时加载「当前客户端账号」可复制的会话。
  useEffect(() => {
    if (!open || !account) return;
    setCopyEnabled(false);
    setSelected(new Set());
    setCollapsed(new Set());
    setDataRoot(undefined);
    setError("");
    setLoadingSessions(true);
    const fetchSessions = isVscode
      ? api.listVscodeSessions
      : intl
        ? api.listCodebuddyIntlIdeSessions
        : api.listCodebuddyIdeSessions;
    fetchSessions()
      .then((res) => {
        setSessions(res.sessions);
        setSourceUid(res.sourceUid);
        setDataRoot(res.dataRoot);
      })
      .catch(() => {
        setSessions([]);
        setSourceUid(null);
        setDataRoot(undefined);
      })
      .finally(() => setLoadingSessions(false));
  }, [open, account, isVscode, intl]);

  const groups = useMemo(() => buildGroups(sessions), [sessions]);
  const sessionById = useMemo(() => {
    const map = new Map<string, VscodeSession>();
    for (const session of sessions) map.set(session.id, session);
    return map;
  }, [sessions]);

  function toggleSession(id: string) {
    setSelected((prev) => toggleSessionSelection(prev, id));
  }

  function toggleGroup(ids: string[]) {
    setSelected((prev) => toggleGroupSelection(prev, ids));
  }

  function toggleCollapsed(key: string) {
    setCollapsed((prev) => toggleSetKey(prev, key));
  }

  async function doSwitch() {
    if (!account) return;
    setBusy(true);
    setError("");
    try {
      const refs: VscodeSessionRef[] | undefined = copyEnabled
        ? [...selected]
            .map((id) => {
              const session = sessionById.get(id);
              return session
                ? {
                    workspaceHash: session.workspaceHash,
                    conversationId: session.id,
                    ...(session.store ? { store: session.store } : {}),
                  }
                : null;
            })
            .filter((ref): ref is VscodeSessionRef => ref !== null)
        : undefined;

      // 勾选绑定预览凭据；执行前后端会重新校验，版本变化则跳过该项。
      const syncPayload = syncSelections.length > 0 ? syncSelections : undefined;
      // IDE 切换固定「关闭 + 写入 + 重开」；VS Code 插件按自动关闭开关决定是否由后端关闭。
      const res: ClientSwitchResult = isVscode
        ? await api.switchVscodeExtAccount(account.id, autoRestart, refs, syncPayload)
        : intl
          ? await api.switchCodebuddyIdeAccount(account.id, true, refs, syncPayload)
          : await api.switchCodebuddyCnIdeAccount(account.id, true, refs, syncPayload);

      reportClientSwitchResult({
        client,
        nickname: displayName(account),
        result: res,
        syncGroups,
        // syncSelections 在本次请求里是否非空（闭包内取本次渲染的值）。
        requestedSync: syncSelections.length > 0,
      });
      onOpenChange(false);
      onDone?.();
    } catch (e) {
      setError(api.asError(e));
    } finally {
      setBusy(false);
    }
  }

  const copyCount = copyEnabled ? selected.size : 0;
  const hasCopyable = groups.length > 0;
  const syncCount = syncSelections.length;
  /** 覆盖项数量：底部摘要据此提示风险。 */
  const overwriteCount = syncSelections.filter((item) => item.mode === "overwrite").length;
  /** 关联会话 tab 是否可用：区块不可用（能力判定不通过）时回落到仅复制会话。 */
  const linksAvailable = linksMeta?.available ?? true;
  const running = status?.running === true;
  // CodeBuddy IDE 国际版不再有独立确认框，不勾选时由底部摘要承担「将重启 IDE」的说明；
  // 国内版与 VS Code 文案保持通用句。
  const { main: summaryMain, sub: summarySub } = switchAccountSummary({
    copyCount,
    syncCount,
    overwriteCount,
    idleHint:
      client === "codebuddy-ide" && intl
        ? running
          ? "未选择复制或同步会话，确认后将关闭并重启 IDE"
          : "未选择复制或同步会话，确认后将打开 IDE"
        : undefined,
  });
  /** 客户端未登录（`loggedIn === false`）：按新会话 / 新登录写入，仅影响提示文案。 */
  const notLoggedIn = status?.loggedIn === false;
  /** 本次会由后端关闭并重开 VS Code（仅在编辑器正在运行且开关打开时）。 */
  const autoClose = isVscode && running && autoRestart;
  const notLoggedInText = notLoggedIn ? notLoggedInHint(client) : undefined;
  const emptyHint = emptyStateHint(client, status, loadingSessions, sourceUid, dataRoot, hasCopyable);
  const statusNotice = statusNoticeFor(client, running, autoRestart);
  const busyText = busyCopy(client, { autoClose, copyCount, syncCount });

  // 关联 tab 不可用（能力判定不通过）时回落到复制会话，避免停在空 tab。
  useEffect(() => {
    if (!linksAvailable && tab === "links") setTab("copy");
  }, [linksAvailable, tab]);

  // 「复制会话」tab 内容：勾选即意图，提交结果由底部摘要兜底确认。
  const copyTabContent = (
    <SessionCopyTab
      loading={loadingSessions}
      hint={emptyHint}
      hasCopyable={hasCopyable}
      enabled={copyEnabled}
      onEnabledChange={setCopyEnabled}
      groups={groups}
      selected={selected}
      onToggleSession={toggleSession}
      onToggleGroup={toggleGroup}
      collapsed={collapsed}
      onToggleCollapsed={toggleCollapsed}
    />
  );

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        showCloseButton={!busy}
        className="flex max-h-[min(90vh,calc(100vh-2rem))] min-w-0 flex-col overflow-hidden"
        tabIndex={-1}
        onOpenAutoFocus={(event) => {
          // Radix 默认把焦点交给第一个可聚焦元素（状态图标 / 自动重开开关），
          // 其 Tooltip 会因 focus 常驻在弹窗上；改为聚焦弹窗容器本身。
          event.preventDefault();
          const container = event.target as HTMLElement | null;
          window.requestAnimationFrame(() => container?.focus());
        }}
      >
        <DialogHeader className="shrink-0">
          <div className="flex items-center gap-3">
            <DialogTitle className="min-w-0 flex-1">
              切换到「{account ? displayName(account) : "该账号"}」
            </DialogTitle>
            {/* 提示收进图标 tooltip、开关与标题同行；mr-6 为右上角关闭按钮留空位。 */}
            <div className="mr-6 flex shrink-0 items-center gap-2">
              <Tooltip>
                <TooltipTrigger asChild>
                  {/* 仅悬停出提示：不加 tabIndex，避免点击/自动聚焦后 Tooltip 常驻不消失。 */}
                  <span
                    className={cn(
                      "flex size-5 cursor-help items-center justify-center",
                      statusNotice.warning
                        ? "text-warning"
                        : "text-muted-foreground",
                    )}
                    aria-label={statusNotice.title}
                  >
                    {statusNotice.icon}
                  </span>
                </TooltipTrigger>
                <TooltipContent side="bottom" align="end" className="max-w-xs space-y-1">
                  <div className="font-medium">{statusNotice.title}</div>
                  <p className="text-muted-foreground">{statusNotice.description}</p>
                  {notLoggedInText && <p className="text-muted-foreground">{notLoggedInText}</p>}
                </TooltipContent>
              </Tooltip>
              {isVscode && (
                <Tooltip>
                  <TooltipTrigger asChild>
                    <span className="inline-flex">
                      <Switch
                        checked={autoRestart}
                        onCheckedChange={toggleAutoRestart}
                        disabled={busy}
                        aria-label="自动关闭并重开 VS Code"
                      />
                    </span>
                  </TooltipTrigger>
                  <TooltipContent side="bottom" align="end" className="max-w-xs space-y-1">
                    <div className="font-medium">自动关闭并重开 VS Code</div>
                    <p className="text-muted-foreground">
                      VS Code 运行时先自动关闭编辑器，写入凭证后再重新打开
                    </p>
                  </TooltipContent>
                </Tooltip>
              )}
            </div>
          </div>
          <DialogDescription>
            {isVscode
              ? "将把所选账号写入 VS Code CodeBuddy 插件；可选把当前账号的会话复制过去，并把关联会话的新内容同步过去。"
              : "将把所选账号写入 CodeBuddy IDE；可选把当前账号的会话复制过去，并把关联会话的新内容同步过去。"}
          </DialogDescription>
        </DialogHeader>

        {busy && (
          <div className="absolute inset-0 z-50 flex flex-col items-center justify-center gap-3 rounded-lg bg-background/85 backdrop-blur-sm">
            <Loader2 className="size-8 animate-spin text-primary" />
            <p className="text-sm font-medium">{busyText.title}</p>
            <p className="max-w-xs text-center text-xs text-muted-foreground">{busyText.hint}</p>
          </div>
        )}

        <div className="min-h-0 space-y-3 overflow-x-hidden overflow-y-auto">
          {linksAvailable ? (
            <Tabs
              value={tab}
              onValueChange={(value) => setTab(value as "links" | "copy")}
              className="flex min-h-0 flex-col gap-3 overflow-hidden"
            >
              <TabsList className="h-auto w-full shrink-0 justify-start gap-5 rounded-none border-b border-border bg-transparent p-0">
                <TabsTrigger value="links" className={TAB_TRIGGER_CLASS}>
                  关联会话
                  {tabCount(linksMeta?.groupCount ?? 0)}
                </TabsTrigger>
                <TabsTrigger value="copy" className={TAB_TRIGGER_CLASS}>
                  复制会话
                  {tabCount(copyCount)}
                </TabsTrigger>
              </TabsList>
              {/* forceMount：切换 tab 不得卸载另一侧，否则关联勾选会被预览重拉重置。 */}
              <TabsContent
                value="copy"
                forceMount
                className="min-h-0 space-y-3 overflow-y-auto data-[state=inactive]:hidden data-[state=active]:animate-in data-[state=active]:fade-in-0 data-[state=active]:duration-150"
              >
                {copyTabContent}
              </TabsContent>
              <TabsContent
                value="links"
                forceMount
                className="min-h-0 overflow-y-auto data-[state=inactive]:hidden data-[state=active]:animate-in data-[state=active]:fade-in-0 data-[state=active]:duration-150"
              >
                <SessionSyncSection
                  open={open}
                  account={account}
                  disabled={busy}
                  loggedIn={!notLoggedIn}
                  // 两个客户端共用展示件，只有预览通道按 client 分流（稳定引用：模块级函数）。
                  fetchPreview={
                    isVscode
                      ? api.vscodeSessionLinksPreview
                      : intl
                        ? api.codebuddyIntlIdeSessionLinksPreview
                        : api.codebuddyIdeSessionLinksPreview
                  }
                  loggedOutHint={
                    isVscode
                      ? "未检测到 VS Code CodeBuddy 插件当前登录账号，请先在 VS Code 中登录该插件后再切换。"
                      : "未检测到 CodeBuddy IDE 当前登录账号，请先在 CodeBuddy IDE 中登录后再切换。"
                  }
                  onChange={(state) => {
                    setSyncSelections(state.selections);
                    setSyncGroups(state.groups);
                  }}
                  onMetaChange={setLinksMeta}
                />
              </TabsContent>
            </Tabs>
          ) : (
            <div className="min-h-0 space-y-3 overflow-y-auto">{copyTabContent}</div>
          )}

          {error && (
            <Alert variant="destructive" className="min-w-0 break-all">
              <AlertDescription className="min-w-0 break-all">{error}</AlertDescription>
            </Alert>
          )}
        </div>

        <DialogFooter className="shrink-0 sm:justify-between">
          <div className="min-w-0 space-y-0.5">
            <div className="text-sm font-medium">{summaryMain}</div>
            {summarySub && <div className="text-xs text-muted-foreground">{summarySub}</div>}
          </div>
          <div className="flex shrink-0 gap-2">
            <Button variant="outline" onClick={() => onOpenChange(false)} disabled={busy}>
              取消
            </Button>
            {/* 勾了复制却没选会话时仍然拦住；只要还有同步项可执行就允许确认。 */}
            <Button
              onClick={doSwitch}
              disabled={busy || (copyEnabled && copyCount === 0 && syncCount === 0)}
            >
              {busy ? "切换中…" : "确认切换"}
            </Button>
          </div>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}