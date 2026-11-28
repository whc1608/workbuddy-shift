import { useEffect, useRef, useState } from "react";
import { Link } from "react-router-dom";
import {
  ChevronLeft,
  ChevronRight,
  CheckCircle2,
  Eraser,
  FolderTree,
  Layers3,
  LayoutList,
  Loader2,
  RefreshCw,
  Search,
  SlidersHorizontal,
  X,
} from "lucide-react";
import { DemoAction } from "@/components/demo-action";
import { GroupDetailPanel } from "@/components/session-group-detail";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog";
import { AlertDialog, AlertDialogCancel, AlertDialogContent, AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle } from "@/components/ui/alert-dialog";
import { DialogAutoHeight } from "@/components/ui/dialog-auto-height";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import type { SessionGroupClient, WbVariant } from "@/lib/types";
import * as api from "@/lib/api";
import { useAccountsStore } from "@/stores/accounts";
import { AddLinkedSessionDialog } from "./add-linked-session-dialog";
import { PAGE_SIZE, visibleClients, type SessionClientEntry } from "./constants";
import { useSupportedTools } from "@/lib/supported-tools";
import { ScanProgressHint } from "@/components/scan-progress-hint";

import { ClientMark, GroupSkeleton, ProjectSection, SessionGroupCard } from "./group-list";
import type { RebuildLinkPrefill, RebuildLinkRequest } from "./blocked-target";
import { useSessionActions } from "./hooks/use-session-actions";
import { useSessionDetail } from "./hooks/use-session-detail";
import { useSessionGroups, type SessionGroupMode } from "./hooks/use-session-groups";
import { canPruneGroups, pruneConfirmDescription, showPruneEntry } from "./prune";
import { restartDialogCopy } from "./restart";
import { selectSyncAllCandidates, syncAllDialogCopy } from "./sync-all";

/** 页头的「更新于 HH:MM」：只到分钟，和卡片里的相对时间不重复。 */
function timeOfDay(timestamp: number): string {
  return new Date(timestamp).toLocaleTimeString("zh-CN", { hour: "2-digit", minute: "2-digit", hour12: false });
}

export default function SessionsPage() {
  const enabledTools = useSupportedTools();
  const clients = visibleClients(enabledTools);
  // 全部关闭时不挂载页面主体：主体一挂载就会拉会话组（大库几十秒），
  // 这些工具都被关掉了，没必要为一个看不见的列表付这份代价。
  if (clients.length === 0) return <NoEnabledClients />;
  return <SessionsPageContent clients={clients} />;
}

function NoEnabledClients() {
  return (
    <div className="mx-auto w-full max-w-[1180px] min-w-0 px-6 py-8 sm:px-8 sm:py-9">
      <header className="mb-6">
        <h1 className="text-[28px] font-semibold tracking-tight">关联会话</h1>
        <p className="mt-2 max-w-2xl text-sm leading-6 text-muted-foreground">统一管理各账号下的会话副本，按需复制或同步。</p>
      </header>
      <div className="rounded-xl border border-dashed border-border px-6 py-12 text-center">
        <p className="text-sm font-medium">未启用任何客户端</p>
        <p className="mt-2 text-sm text-muted-foreground">在「设置 → 支持工具」里开启至少一个客户端后，这里会显示它对应的关联会话。</p>
        <Button asChild variant="outline" size="sm" className="mt-4"><Link to="/settings">去设置</Link></Button>
      </div>
    </div>
  );
}

function SessionsPageContent({ clients }: { clients: SessionClientEntry[] }) {
  const { accounts, fetchAll } = useAccountsStore();
  const [clientChoice, setClientChoice] = useState<SessionGroupClient>(clients[0].id);
  // 选中的客户端被关掉时回退到第一个仍启用的，避免留下一个不在标签列表里的选中项。
  const client = clients.some((item) => item.id === clientChoice) ? clientChoice : clients[0].id;
  const [variantScope, setVariantScope] = useState<WbVariant>("cn");
  const [selectedGroupId, setSelectedGroupId] = useState<string | null>(null);
  const [detailOpen, setDetailOpen] = useState(false);
  const [addOpen, setAddOpen] = useState(false);
  /** 「重新复制并重建关联」的预填：非空即自动打开「新增关联会话」并预选来源 / 目标账号。 */
  const [rebuildPrefill, setRebuildPrefill] = useState<RebuildLinkPrefill | null>(null);
  const [contentWidth, setContentWidth] = useState(0);
  const rootRef = useRef<HTMLDivElement>(null);
  const openerRef = useRef<HTMLElement | null>(null);
  const detailMode = "modal";

  useEffect(() => {
    const root = rootRef.current;
    if (!root) return;
    const observer = new ResizeObserver(([entry]) => setContentWidth(entry.contentRect.width));
    observer.observe(root);
    return () => observer.disconnect();
  }, []);

  useEffect(() => {
    if (accounts.length === 0) void fetchAll();
  }, [accounts.length, fetchAll]);

  // 进入页面先自动识别一次 JetBrains 当前登录：用户在 PyCharm / IDEA 里换了账号后，
  // 不必先点「检测账号」才能看到新账号。**失败静默**——没装 IDE、插件没登录都是正常情况，
  // 不该弹错。命中账号时后端会回写「当前账号」，账号库显示名可能随之变化，故再拉一次。
  useEffect(() => {
    if (api.isDemoMode()) return;
    let live = true;
    void api
      .detectJetbrainsAccount()
      .then((result) => {
        if (live && result.matched) void fetchAll();
      })
      .catch(() => {
        /* JetBrains 未安装 / 未登录 / 读 secret-storage 失败：静默 */
      });
    return () => {
      live = false;
    };
  }, [fetchAll]);

  const scope = client === "codebuddyIde" ? variantScope : undefined;
  const list = useSessionGroups(client, scope);
  const detail = useSessionDetail({
    client,
    scope,
    selectedGroupId,
    detailOpen,
    reloadGroups: list.reloadGroups,
  });
  const actions = useSessionActions({
    client,
    scope,
    pruneBlocked: list.pruneBlocked,
    selectedGroupId,
    setSelectedGroupId,
    closeDetail,
    detail,
    reloadGroups: list.reloadGroups,
  });

  function closeDetail() {
    setDetailOpen(false);
    requestAnimationFrame(() => openerRef.current?.focus({ preventScroll: true }));
  }

  function openDetail(id: string) {
    openerRef.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    detail.resetForOpen();
    setSelectedGroupId(id);
    setAddOpen(false);
    // 上一次没走完的重建不带进新的会话组。
    setRebuildPrefill(null);
    setDetailOpen(true);
  }

  function changeClient(value: string) {
    const next = value as SessionGroupClient;
    setClientChoice(next);
    setDetailOpen(false);
    setSelectedGroupId(null);
    setRebuildPrefill(null);
  }

  /**
   * 被阻塞目标的出路（issue #3）：先解除该账号与当前会话组的关联（账号里的会话内容不动），
   * 再把这个账号预填成「新增关联会话」的目标账号，由用户勾选会话后走既有的复制 + 关联流程。
   *
   * 不碰任何写入许可：这里不解锁 `unknown` 的覆盖，只是把「组里这份坏掉的关联」换成一份新的副本。
   * 返回 `false` = 解除关联失败（`actions.unlinkMember` 已 toast），二次确认框留着可重试。
   */
  async function rebuildLink(request: RebuildLinkRequest): Promise<boolean> {
    const member = detail.detail?.members.find((item) => item.memberId === request.memberId) ?? null;
    if (!member) return false;
    try {
      await actions.unlinkMember(member);
    } catch {
      return false;
    }
    setRebuildPrefill(request.prefill);
    return true;
  }

  const detailPanel = (
    <GroupDetailPanel
      client={client} detail={detail.detail} sourceMemberId={detail.sourceMemberId}
      currentAccounts={detail.currentAccounts}
      setSourceMemberId={detail.setSourceMemberId}
      addTargetId={detail.addTargetId} setAddTargetId={detail.setAddTargetId}
      unifyPlan={detail.unifyPlan} unifyLoading={detail.unifyLoading} setUnifyPlan={detail.setUnifyPlan} busy={actions.actionBusy || detail.detailLoading || !!detail.detailError}
      error={detail.detailError} loading={detail.detailLoading} onClose={closeDetail}
      onRetry={() => { detail.setDetailError(null); void detail.reloadSelectedGroup(); }}
      onPrepareUnify={actions.prepareUnify} onConfirmUnify={actions.confirmUnify} onBatchSync={actions.syncSafeBatch} onAdd={actions.addMember}
      onUnlinkMember={actions.unlinkMember}
      onRebuildLink={rebuildLink}
      addOpen={addOpen} setAddOpen={setAddOpen} fullPage={false}
    />
  );
  /** 运行中确认框文案：按客户端分派（JetBrains 会关闭 IDEA / PyCharm 并逐个重开）。 */
  const restartCopy = restartDialogCopy(client);
  // 跨组批量同步的候选（只含 `behind` 组）：N>0 才显示入口，文案按客户端说明是否关宿主。
  const syncAllCandidates = selectSyncAllCandidates(list.groups);
  const syncAllCopy = syncAllDialogCopy(
    client,
    actions.syncAllTarget?.groupCount ?? syncAllCandidates.groupCount,
    actions.syncAllTarget?.memberCount ?? syncAllCandidates.memberCount,
  );
  const statusOptions = [    ["all", "全部"], ["behind", "待同步"], ["diverge", "有分歧"], ["latest", "内容一致"],
    ["missing", "内容缺失"], ["unknown", "无法确认"],
  ];
  const pages = Array.from({ length: list.pageCount }, (_, index) => index + 1)
    .filter((value) => value === 1 || value === list.pageCount || Math.abs(value - list.currentPage) <= 1);
  /**
   * 刷新按钮的「忙」判定：三处来源都算——首次加载、后台刷新、详情在途。
   *
   * 按钮禁用是「连点只发一次请求」的唯一保证（不另加节流层）：从点击到 `refreshing` 置位
   * 之间没有任何 await，React 的同一次提交就把它变成禁用态。
   */
  const refreshBusy = list.loading || list.refreshing || detail.detailLoading;
  /** 卡片网格列数：分组模式与平铺模式共用同一套断点。 */
  const gridColumns = contentWidth >= 600 ? "grid-cols-2" : "grid-cols-1";

  return (
    <div data-detail-mode={detailMode} className="mx-auto w-full max-w-[1180px] min-w-0 px-6 py-8 sm:px-8 sm:py-9">
      <div ref={rootRef} className="flex min-w-0 flex-col">
        <header className="mb-6">
          <div className="flex min-w-0 flex-wrap items-start justify-between gap-4">
            <div className="min-w-0">
              <h1 className="text-[28px] font-semibold tracking-tight">关联会话</h1>
              <p className="mt-2 max-w-2xl text-sm leading-6 text-muted-foreground">统一管理各账号下的会话副本，按需复制或同步。
                {list.updatedAt != null && <span role="status" className="ml-2 whitespace-nowrap text-xs tabular-nums">更新于 {timeOfDay(list.updatedAt)}</span>}
              </p>
            </div>
            <div className="flex max-w-full flex-wrap items-center justify-end gap-2">
              <Button variant="outline" size="sm" className="shrink-0" aria-busy={refreshBusy} onClick={() => void (detailOpen && selectedGroupId ? detail.reloadSelectedGroup() : list.reloadGroups())} disabled={refreshBusy}><RefreshCw className={refreshBusy ? "animate-spin" : undefined} />{refreshBusy ? "刷新中…" : "刷新"}</Button>
              {syncAllCandidates.groupCount > 0 && <Button variant="outline" size="sm" className="shrink-0" onClick={() => actions.requestSyncAll(list.groups)} disabled={actions.actionBusy || list.loading}><RefreshCw className={actions.actionBusy ? "animate-spin" : undefined} />同步 {syncAllCandidates.groupCount} 个待同步组</Button>}
              {showPruneEntry({ client, groupCount: list.groups.length, deadGroupCount: list.deadGroupCount }) && <span className="shrink-0" title={list.pruneBlocked ?? undefined}><Button variant="outline" size="sm" onClick={() => void actions.requestPrune()} disabled={!canPruneGroups({ client, groupCount: list.groups.length, deadGroupCount: list.deadGroupCount, busy: actions.actionBusy || list.loading, blockedReason: list.pruneBlocked })}><Eraser />清理失效关联组</Button></span>}
              <AddLinkedSessionDialog client={client} variantScope={scope} disabled={list.loading} prefill={rebuildPrefill} onPrefillConsumed={() => setRebuildPrefill(null)} onDone={() => void (selectedGroupId ? detail.reloadSelectedGroup() : list.reloadGroups())} requestRestartConfirm={(actionLabel, run) => actions.setRestartPrompt({ actionLabel, run })} />
            </div>
          </div>
          <Tabs value={client} onValueChange={changeClient} className="mt-4">
            <TabsList aria-label="会话客户端" className="grid h-auto w-full grid-cols-[repeat(auto-fit,minmax(160px,1fr))] gap-2 bg-transparent p-0">
              {clients.map((item) => <TabsTrigger key={item.id} value={item.id} className="relative h-auto min-w-0 justify-start gap-3 rounded-lg border border-border bg-background px-3 py-4 text-left hover:border-brand/50 data-[state=active]:border-brand data-[state=active]:bg-brand/5 data-[state=active]:shadow-sm">
                <ClientMark client={item.id} />
                <span className="min-w-0"><span className="block truncate text-sm font-semibold text-foreground">{item.title}</span><span className="mt-1 block truncate text-xs font-normal text-muted-foreground">{item.id === "workbuddy" ? "国内版 · 国际版" : item.id === "codebuddyIde" ? "国内版 / 国际版" : item.id === "jetbrains" ? "IDEA / PyCharm" : "VS Code"}</span></span>
                {client === item.id && <CheckCircle2 className="absolute right-2 top-2 size-3.5 text-brand" />}
              </TabsTrigger>)}
            </TabsList>
          </Tabs>
        </header>
        <section aria-labelledby="session-groups-title" className="min-w-0">
          <div className="mb-4 flex flex-wrap items-center gap-2">
            <h2 id="session-groups-title" className="text-base font-semibold tracking-tight">{clients.find((item) => item.id === client)?.title} 会话</h2>
            <Badge variant="secondary" className="h-6 min-w-6 rounded-full border-0 px-1.5 text-[11px] tabular-nums text-muted-foreground shadow-none" aria-label={`${list.groups.length} 个会话组`}>{list.groups.length}</Badge>
          </div>
          <div className="my-4 flex flex-wrap items-center justify-between gap-2">
            <div className="flex min-w-0 max-w-full flex-wrap items-center gap-2">
              {client === "codebuddyIde" && <Select value={variantScope} onValueChange={(value) => { setVariantScope(value as WbVariant); setSelectedGroupId(null); setDetailOpen(false); }}><SelectTrigger size="sm" className="w-32 bg-background text-xs" aria-label="选择 CodeBuddy IDE 档位"><SelectValue /></SelectTrigger><SelectContent><SelectItem value="cn">国内版</SelectItem><SelectItem value="ai">国际版</SelectItem></SelectContent></Select>}
              {/* 搜索放在筛选之前：列表最主要的入口是「我记得那个项目 / 那句话」。
                  它是**受控 + 即时**过滤（本地过滤 `filteredGroups`，不再请求后端），
                  与状态筛选、排序、分组四个维度正交，互不覆盖。 */}
              <div className="relative w-full min-w-0 sm:w-60">
                <Search className="pointer-events-none absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground" aria-hidden="true" />
                <Input
                  value={list.query}
                  onChange={(event) => list.setQuery(event.target.value)}
                  onKeyDown={(event) => {
                    // Esc 清空搜索：键盘用户不必去够那个小叉。
                    if (event.key === "Escape" && list.query) list.setQuery("");
                  }}
                  placeholder="搜索标题、项目或摘要"
                  aria-label="搜索会话"
                  className="h-8 pl-8 pr-8 text-xs"
                />
                {list.query && <Button variant="ghost" size="icon" className="absolute right-1 top-1/2 size-6 -translate-y-1/2 text-muted-foreground hover:text-foreground" aria-label="清空搜索" title="清空搜索" onClick={() => list.setQuery("")}><X className="size-3.5" /></Button>}
              </div>
              <Tabs value={list.statusFilter} onValueChange={list.setStatusFilter} className="min-w-0 max-w-full gap-0">
                <TabsList className="h-auto max-w-full flex-wrap justify-start gap-0.5" aria-label="按状态筛选">
                  {statusOptions.filter(([value]) => ["all", "behind", "diverge", "latest"].includes(value) || value === list.statusFilter || list.groups.some((group) => group.summaryStatus === value)).map(([value, label]) => <TabsTrigger key={value} value={value} className="h-8 gap-1.5 px-2.5 hover:bg-background/60">
                    {label}<span className={`rounded-full px-1.5 text-[11px] tabular-nums ${list.statusFilter === value ? "bg-muted" : "bg-background/70"}`}>{value === "all" ? list.groups.length : list.groups.filter((group) => group.summaryStatus === value).length}</span>
                  </TabsTrigger>)}
                </TabsList>
              </Tabs>
              {/* 「不分组 / 按项目」只改**呈现方式**：搜索、状态筛选、排序都照旧先生效，
                  分组是在结果集之上再收拢一层，因此不会覆盖筛选条件。 */}
              <Tabs value={list.groupMode} onValueChange={(value) => list.setGroupMode(value as SessionGroupMode)} className="min-w-0 gap-0">
                <TabsList className="h-auto gap-0.5" aria-label="分组方式">
                  <TabsTrigger value="none" className="h-8 gap-1.5 px-2.5 hover:bg-background/60"><LayoutList className="size-3.5" aria-hidden="true" />不分组</TabsTrigger>
                  <TabsTrigger value="project" className="h-8 gap-1.5 px-2.5 hover:bg-background/60"><FolderTree className="size-3.5" aria-hidden="true" />按项目</TabsTrigger>
                </TabsList>
              </Tabs>
            </div>
            <Select value={list.sortOrder} onValueChange={list.setSortOrder}><SelectTrigger size="sm" className="w-auto min-w-28 gap-1.5 border-0 bg-transparent px-2 text-xs text-muted-foreground shadow-none hover:bg-accent hover:text-foreground" aria-label="排序方式"><SlidersHorizontal className="size-4" aria-hidden="true" /><SelectValue /></SelectTrigger><SelectContent><SelectItem value="recent">最近更新</SelectItem><SelectItem value="oldest">最早更新</SelectItem></SelectContent></Select>
          </div>
          {(list.storeStatus === "unavailable" || list.storeError) && <p role="status" className="mb-3 rounded-lg border border-warning-border p-3 text-sm text-muted-foreground">{list.storeError ?? "关联组存储暂不可用，当前只展示可读取的数据。"}</p>}
          {list.loadError && <div role="alert" className="mb-3 rounded-lg border border-danger-border p-3 text-sm text-danger">{list.loadError}<Button variant="outline" size="sm" className="ml-2" onClick={() => void list.reloadGroups()}>重试</Button></div>}
          {list.loading ? (
            <div className="space-y-3"><ScanProgressHint /><GroupSkeleton /></div>
          ) : list.filteredGroups.length === 0 ? (
            <div className="flex min-h-56 flex-col items-center justify-center rounded-xl border border-dashed p-6 text-center">
              <Layers3 className="size-7 text-muted-foreground" />
              <h3 className="mt-3 text-sm font-medium">{list.groups.length === 0 ? (list.deadGroupCount > 0 ? "关联组都已失效" : "还没有关联会话组") : list.query.trim() ? `没有匹配「${list.query.trim()}」的会话` : "没有符合条件的会话"}</h3>
              <p className="mt-2 max-w-sm text-sm text-muted-foreground">{list.groups.length === 0 ? (list.deadGroupCount > 0 ? `有 ${list.deadGroupCount} 个组的会话已被删除，可用上方「清理失效关联组」移除关联关系。` : "从账号切换时复制会话后，关联组会显示在这里。") : list.query.trim() ? "搜索会匹配标题、项目名与卡片摘要；换个词或清空再看。" : "试试其它筛选条件。"}</p>
              {list.groups.length > 0 && (list.query.trim() !== "" || list.statusFilter !== "all") && <Button variant="outline" size="sm" className="mt-3" onClick={() => { list.setQuery(""); list.setStatusFilter("all"); }}>清空搜索与筛选</Button>}
            </div>
          ) : list.grouped ? (
            /* 分组模式：每节一个 `项目 X · N 个会话` 折叠节头（默认展开，见 `ProjectSection`）。
               分页口径是「按节分页、节内条目全给」，所以同一项目的会话永远落在同一页的同一节里，
               节头里的 N 也是该项目的**全部**会话数。 */
            <div className="min-w-0 space-y-4">
              {list.visibleSections.map((section) => <ProjectSection key={section.project || "__none__"} section={section} columns={gridColumns} selectedGroupId={selectedGroupId} onSelectGroup={openDetail} onRequestDelete={(group) => actions.setDeleteTarget(group)} />)}
            </div>
          ) : (
            <div className={`grid min-w-0 gap-3 ${gridColumns}`}>
              {list.visibleGroups.map((group) => <SessionGroupCard key={group.key} group={group} selected={group.groupId === selectedGroupId} onSelect={() => openDetail(group.groupId)} onRequestDelete={() => actions.setDeleteTarget(group)} />)}
            </div>
          )}
          {/* 分页条：显隐规则交给 `list.showPagination`（平铺看条目数，分组看节数），
              页面不再自己算一遍阈值，避免两处规则漂移。 */}
          {!list.loading && list.showPagination && (
            <div className="mt-6 flex flex-wrap items-center justify-between gap-3 text-xs text-muted-foreground">
              <span>{list.grouped
                ? `显示 ${list.visibleSections.length} / 共 ${list.sections.length} 个项目 · 本页 ${list.visibleGroups.length} 个会话`
                : `显示 ${(list.currentPage - 1) * PAGE_SIZE + 1}–${Math.min(list.currentPage * PAGE_SIZE, list.filteredGroups.length)}`}，共 {list.filteredGroups.length} 个会话</span>
              <nav aria-label="会话分页" className="flex flex-wrap items-center gap-1.5">
                <Button variant="outline" size="icon" className="size-8" aria-label="上一页" disabled={list.currentPage <= 1} onClick={() => list.setPage(list.currentPage - 1)}><ChevronLeft /></Button>
                {pages.map((value, index) => <span key={value} className="flex items-center gap-1.5">{index > 0 && value - pages[index - 1] > 1 && <span>…</span>}<Button variant={value === list.currentPage ? "default" : "outline"} className={`size-8 p-0 ${value === list.currentPage ? "bg-brand text-brand-foreground hover:bg-brand/90" : ""}`} aria-label={`第 ${value} 页`} aria-current={value === list.currentPage ? "page" : undefined} onClick={() => list.setPage(value)}>{value}</Button></span>)}
                <Button variant="outline" size="icon" className="size-8" aria-label="下一页" disabled={list.currentPage >= list.pageCount} onClick={() => list.setPage(list.currentPage + 1)}><ChevronRight /></Button>
              </nav>
            </div>
          )}
        </section>
      </div>
      <Dialog open={detailOpen} onOpenChange={(open) => { if (!open) closeDetail(); }}>
        <DialogContent showCloseButton={false} aria-describedby={undefined} onCloseAutoFocus={(event) => { event.preventDefault(); if (!detailOpen) openerRef.current?.focus({ preventScroll: true }); }} className="session-detail-modal flex flex-col gap-0 overflow-hidden p-0">
          <DialogTitle className="sr-only">会话详情</DialogTitle>
          <DialogAutoHeight>{detailPanel}</DialogAutoHeight>
        </DialogContent>
      </Dialog>
      {/* 插件侧写操作的运行中确认框：写入会被运行中的宿主覆盖，需要先关闭并在结束后重开。 */}
      <AlertDialog open={!!actions.restartPrompt} onOpenChange={(open) => { if (!open && !actions.actionBusy) actions.setRestartPrompt(null); }}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{restartCopy?.title ?? "同步前需要重启宿主"}</AlertDialogTitle>
            <AlertDialogDescription>{restartCopy?.description ?? ""}</AlertDialogDescription>
          </AlertDialogHeader>
          {actions.actionBusy && <p role="status" className="flex items-center gap-2 text-sm text-muted-foreground"><Loader2 className="size-4 animate-spin" />{restartCopy?.busyText ?? "正在处理，请勿关闭本窗口…"}</p>}
          <AlertDialogFooter>
            <AlertDialogCancel disabled={actions.actionBusy}>取消</AlertDialogCancel>
            <DemoAction><Button disabled={actions.actionBusy} onClick={() => void actions.confirmRestart()}>{actions.actionBusy ? "处理中…" : actions.restartPrompt?.actionLabel ?? "关闭并继续"}</Button></DemoAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
      <AlertDialog open={!!actions.deleteTarget} onOpenChange={(open) => { if (!open && !actions.actionBusy) actions.setDeleteTarget(null); }}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>删除「{actions.deleteTarget?.title}」的关联？</AlertDialogTitle>
            <AlertDialogDescription>
              组内 {actions.deleteTarget?.memberCount} 个账号会一起解除关联，不再参与同步；账号里的会话内容不会被删除。
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={actions.actionBusy}>取消</AlertDialogCancel>
            <DemoAction><Button variant="destructive" disabled={actions.actionBusy} onClick={() => void actions.deleteGroup()}>{actions.actionBusy ? "处理中…" : "确认删除关联"}</Button></DemoAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
      {/* 跨组批量同步：一次确认把所有可安全快进的副本同步完，分歧组跳过并说明原因。 */}
      <AlertDialog open={!!actions.syncAllTarget} onOpenChange={(open) => { if (!open && !actions.actionBusy) actions.setSyncAllTarget(null); }}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{syncAllCopy.title}</AlertDialogTitle>
            <AlertDialogDescription>{syncAllCopy.description}</AlertDialogDescription>
          </AlertDialogHeader>
          {actions.actionBusy && <p role="status" className="flex items-center gap-2 text-sm text-muted-foreground"><Loader2 className="size-4 animate-spin" />{syncAllCopy.busyText}</p>}
          <AlertDialogFooter>
            <AlertDialogCancel disabled={actions.actionBusy}>取消</AlertDialogCancel>
            <DemoAction><Button disabled={actions.actionBusy} onClick={() => void actions.confirmSyncAll()}>{actions.actionBusy ? "同步中…" : "开始同步"}</Button></DemoAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
      {/* 批量清理：先 dry-run 取数量，再二次确认；只删关联关系，不删会话内容。 */}
      <AlertDialog open={!!actions.pruneTarget} onOpenChange={(open) => { if (!open && !actions.actionBusy) actions.setPruneTarget(null); }}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>清理失效的关联组？</AlertDialogTitle>
            <AlertDialogDescription>
              {actions.pruneTarget ? pruneConfirmDescription(actions.pruneTarget) : ""}
            </AlertDialogDescription>
          </AlertDialogHeader>
          {actions.actionBusy && <p role="status" className="flex items-center gap-2 text-sm text-muted-foreground"><Loader2 className="size-4 animate-spin" />正在清理，请勿关闭本窗口…</p>}
          <AlertDialogFooter>
            <AlertDialogCancel disabled={actions.actionBusy}>取消</AlertDialogCancel>
            <DemoAction><Button variant="destructive" disabled={actions.actionBusy} onClick={() => void actions.confirmPrune()}>{actions.actionBusy ? "处理中…" : "确认清理"}</Button></DemoAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}
