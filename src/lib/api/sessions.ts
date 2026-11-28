import type {
  DiscardedSessionWrite,
  PendingSessionWrite,
  Session,
  SessionCopyReport,
  SessionLinksPreview,
  SessionSyncMode,
  SessionGroupAllSafeReport,
  SessionGroupClient,
  SessionGroupList,
  SessionGroupDetail,
  SessionGroupPairPreview,
  SessionGroupActionReport,
  SessionGroupPruneReport,
  WbVariant,
} from "../types";
import { DEMO_UNAVAILABLE_MESSAGE, demoModeEnabled } from "../demo-mode";
import { call, variantArgs } from "./core";

/** 当前登录态的会话列表；`variant` 缺省为国内版。 */
export function listSessions(variant?: WbVariant): Promise<{
  sessions: Session[];
  current: string | null;
}> {
  return call("list_sessions", variantArgs(variant));
}

/** 指定账号名下的会话列表（会话管理页的源账号视角）；账号不存在时后端返回明确错误。 */
export function listAccountSessions(accountId: string, client?: SessionGroupClient): Promise<{
  /** 插件侧会话没有 `cwd`（只有 `workspaceHash`）：调用方补齐默认值后再交给会话树。 */
  sessions: (Omit<Session, "cwd"> & { cwd?: string })[];
  current: string | null;
  variant?: WbVariant;
  /** 插件侧：插件数据仓根目录（`null` = 未找到目录，与「该账号无会话」区分）。 */
  dataRoot?: string | null;
  sourceUid?: string;
}> {
  return call("list_account_sessions", { accountId, ...(client ? { client } : {}) });
}

/**
 * 跨档把会话从**显式源账号**复制到**显式目标账号**（会话管理页用）。
 *
 * 与 `copySessions` 同形，只多一个 `sourceAccountId`：源可为国内版或国际版账号，
 * 不再要求源是当前登录账号；报告另带 `sourceVariant` / `targetVariant`。
 *
 * `copyMemory` 为复制弹窗里**默认不勾**的「同时复制工作记忆（MEMORY.md）」：
 * 不勾时请求体不带该字段（后端缺省 false），后端连源文件都不读。
 */
export function copySessionsCross(
  sourceAccountId: string,
  targetAccountId: string,
  sessionIds: string[],
  ignoreAuxData?: boolean,
  copyMemory?: boolean,
): Promise<SessionCopyReport & { variant?: WbVariant }> {
  return call("copy_sessions_cross", {
    sourceAccountId,
    targetAccountId,
    sessionIds,
    // 只由「仍然复制（不带用量与中断记录）」传 true：忽略后不会后台补写账本。
    ...(ignoreAuxData ? { ignoreAuxData: true } : {}),
    // 只由用户勾选「同时复制工作记忆」时传 true：不勾就不产生任何读文件动作。
    ...(copyMemory ? { copyMemory: true } : {}),
  });
}

/**
 * 预览「当前账号 → 目标账号」可同步的关联会话（只读）。
 *
 * 默认勾选与可选模式都来自后端：前端只按 `defaultChecked` / `availableModes` 渲染，
 * 不自行扩大权限。`variant` 缺省由后端取目标账号自身档位。
 */
export function sessionLinksPreview(
  targetAccountId: string,
  variant?: WbVariant,
): Promise<SessionLinksPreview> {
  const args: Record<string, unknown> = { targetAccountId };
  if (variant === "ai") args.variant = variant;
  return call("session_links_preview", args);
}

export function listSessionGroups(client: SessionGroupClient, variantScope?: WbVariant): Promise<SessionGroupList> {
  return call("list_session_groups", { client, ...(variantScope ? { variantScope } : {}) });
}

export function getSessionGroup(client: SessionGroupClient, groupId: string, variantScope?: WbVariant): Promise<SessionGroupDetail> {
  return call("get_session_group", { client, groupId, ...(variantScope ? { variantScope } : {}) });
}

export function previewSessionGroupPair(args: {
  client: SessionGroupClient;
  groupId: string;
  sourceMemberId: string;
  targetMemberId: string;
  variantScope?: WbVariant;
}): Promise<SessionGroupPairPreview> {
  return call("preview_session_group_pair", args as unknown as Record<string, unknown>);
}

export function syncSessionGroupPair(args: {
  client: SessionGroupClient;
  groupId: string;
  sourceMemberId: string;
  targetMemberId: string;
  previewToken: string;
  mode: SessionSyncMode;
  variantScope?: WbVariant;
  /** 已获用户授权（确认框）时传 `true`：插件侧运行中允许关闭并重开 VS Code。 */
  restart?: boolean;
}): Promise<SessionGroupActionReport> {
  if (demoModeEnabled) return Promise.reject(new Error(DEMO_UNAVAILABLE_MESSAGE));
  return call("sync_session_group_pair", args as unknown as Record<string, unknown>);
}

export function syncSessionGroupUnify(args: {
  client: SessionGroupClient;
  groupId: string;
  sourceMemberId: string;
  targets: { targetMemberId: string; previewToken: string; mode: SessionSyncMode }[];
  /** 已获用户授权（确认框）时传 `true`：插件侧运行中允许关闭并重开宿主（VS Code / JetBrains IDE）。 */
  restart?: boolean;
}): Promise<SessionGroupActionReport> {
  if (demoModeEnabled) return Promise.reject(new Error(DEMO_UNAVAILABLE_MESSAGE));
  return call("sync_session_group_unify", args as unknown as Record<string, unknown>);
}

export function syncSessionGroupSafeBatch(client: SessionGroupClient, groupId: string, variantScope?: WbVariant, restart?: boolean): Promise<SessionGroupActionReport> {
  if (demoModeEnabled) return Promise.reject(new Error(DEMO_UNAVAILABLE_MESSAGE));
  return call("sync_session_group_safe_batch", { client, groupId, ...(variantScope ? { variantScope } : {}), ...(restart ? { restart: true } : {}) });
}

/**
 * 跨组安全批量同步：一次把多个关联组的「可安全快进副本」同步完。
 *
 * `groupIds` 由页面用 `selectSyncAllCandidates` 从列表载荷筛出（`summaryStatus === "behind"`）；
 * 后端逐组重算安全来源并复核，分歧组整组跳过（原因在报告的 `skippedGroups` 里），
 * 编辑器生命周期全程只关 / 开一次。`restart` 只在确认框授权后传 `true`。
 */
export function syncAllSessionGroupsSafe(
  client: SessionGroupClient,
  groupIds: string[],
  variantScope?: WbVariant,
  restart?: boolean,
): Promise<SessionGroupAllSafeReport> {
  if (demoModeEnabled) return Promise.reject(new Error(DEMO_UNAVAILABLE_MESSAGE));
  return call("sync_all_session_groups_safe", {
    client,
    groupIds,
    ...(variantScope ? { variantScope } : {}),
    restart: restart === true,
  });
}

export function addSessionGroupMember(args: {
  client: SessionGroupClient;
  groupId: string;
  sourceMemberId: string;
  targetAccountId: string;
  variantScope?: WbVariant;
  /** 已获用户授权（确认框）时传 `true`：插件侧运行中允许关闭并重开 VS Code。 */
  restart?: boolean;
}): Promise<{ status: "linked" | "alreadyLinked" | "copiedUnlinked" | "failed"; editorError?: string; restartedEditor?: boolean; /** 内容缺失 / 索引丢失而被跳过的会话。 */ skipped?: { id: string; error: string }[]; [key: string]: unknown }> {
  if (demoModeEnabled) return Promise.reject(new Error(DEMO_UNAVAILABLE_MESSAGE));
  return call("add_session_group_member", args as unknown as Record<string, unknown>);
}

/**
 * 编辑器 / IDE：把来源账号的勾选会话复制到目标账号并登记关联（无现成组则新建关联组）。
 *
 * `variantScope` 只有 CodeBuddy IDE 需要（`cn` / `ai` 决定运行态判定与关联组的档位）；
 * **其余客户端不传该字段**（与 `listAccountSessions` 的 `client` 同一写法）：桌面端 invoke
 * 与 HTTP 请求体的「缺省」必须逐字一致，值一律不靠序列化阶段的 `undefined` 丢弃来保证。
 */
export function copyLinkedSessions(args: {
  client: SessionGroupClient;
  sourceAccountId: string;
  targetAccountId: string;
  sessionIds: string[];
  /** 仅 CodeBuddy IDE：当前页面选中的档位。 */
  variantScope?: WbVariant;
  /** 已获用户授权（确认框）时传 `true`：运行中允许关闭并重开 VS Code / JetBrains IDE。 */
  restart?: boolean;
}): Promise<{ status: "linked" | "alreadyLinked" | "copiedUnlinked" | "failed"; editorError?: string; restartedEditor?: boolean; [key: string]: unknown }> {
  if (demoModeEnabled) return Promise.reject(new Error(DEMO_UNAVAILABLE_MESSAGE));
  const { client, sourceAccountId, targetAccountId, sessionIds, variantScope, restart } = args;
  return call("copy_linked_sessions", {
    client,
    sourceAccountId,
    targetAccountId,
    sessionIds,
    ...(variantScope ? { variantScope } : {}),
    restart: restart === true,
  });
}

/**
 * 取消关联：把成员从会话组移除（只解除管理关系，不删除账号内的会话内容）。
 *
 * `groupRemoved` 表示移除后组内已无成员，该组已被删除。
 */
export function unlinkSessionGroupMember(args: {
  client: SessionGroupClient;
  groupId: string;
  memberId: string;
  variantScope?: WbVariant;
}): Promise<{ status: "removed" | "groupRemoved"; client: SessionGroupClient; groupId: string; memberId: string; remaining: number }> {
  if (demoModeEnabled) return Promise.reject(new Error(DEMO_UNAVAILABLE_MESSAGE));
  return call("unlink_session_group_member", args as unknown as Record<string, unknown>);
}

/**
 * 删除会话组：组内所有成员一起解除关联（只解除管理关系，不删除账号内的会话内容）。
 */
export function deleteSessionGroup(args: {
  client: SessionGroupClient;
  groupId: string;
  variantScope?: WbVariant;
}): Promise<{ status: "groupRemoved"; client: SessionGroupClient; groupId: string; removed: number }> {
  if (demoModeEnabled) return Promise.reject(new Error(DEMO_UNAVAILABLE_MESSAGE));
  return call("delete_session_group", args as unknown as Record<string, unknown>);
}

/**
 * 批量清理失效关联组：组内**所有**成员指向的会话都已不存活时才删整组。
 *
 * `dryRun: true` 只统计不写盘（页面先用它取数量再弹确认框）；`dryRun: false` 会先备份
 * 关联记录再写盘。只删除关联关系，不删除任何账号内的会话内容。
 */
export function pruneSessionGroups(args: {
  client: SessionGroupClient;
  variantScope?: WbVariant;
  dryRun: boolean;
}): Promise<SessionGroupPruneReport> {
  if (demoModeEnabled) return Promise.reject(new Error(DEMO_UNAVAILABLE_MESSAGE));
  return call("prune_session_groups", args as unknown as Record<string, unknown>);
}

/**
 * 未完成的会话写入（只读）。
 *
 * 操作目录跨档共用：返回的每项自带 `variant`，不按当前档位过滤。
 */
export async function pendingSessionWrites(): Promise<PendingSessionWrite[]> {
  const result = await call<{ items: PendingSessionWrite[] }>("session_pending_writes", {});
  return result.items;
}

/**
 * 结束一次未完成的同步写入。
 *
 * 只改操作记录：不碰正文、不碰目标会话行；后端会拒绝不该由用户放弃的现场
 * （例如「正文本就是本次写入的内容，下次切号会自动补完」），错误文案直接展示。
 */
export function discardSessionWrite(operationId: string): Promise<DiscardedSessionWrite> {
  return call("session_discard_write", { operationId });
}
