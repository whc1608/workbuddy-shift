

import { AccountMeta, WbVariant } from "./account";

export interface Session {
  id: string;
  title: string;
  cwd: string;
  updatedAt: number;
  hasHistory: boolean;
  /** WorkBuddy playground（侧栏「任务」）；缺省视为空间会话。 */
  isPlayground?: boolean;
  /**
   * 空间 id（WorkBuddy 5.x `sessions.space_id`）；老库不返回该字段。
   *
   * 分组键：有它按空间合组，没有则回退 `cwd` 最后一段（老库行为不变）。
   */
  spaceId?: string | null;
  /**
   * 空间真名（`spaces.name`）；空间行缺失或已删除时后端给 null，前端回退文件夹名。
   */
  spaceName?: string | null;
  /**
   * JetBrains 插件会话的**项目段**（`<uid>/JetBrains/<projectKey>/history/<bucket>/`）。
   *
   * 只有 JetBrains 会返回（VS Code / CodeBuddy IDE 的那一级恒为账号 uid，后端不返回，
   * 既有载荷逐字不变）：项目 key 是项目路径的 base64url，未打开项目时退化成账号 uid。
   * 同一会话可能同时出现在「无项目」与「项目」两个桶里，靠它区分。
   */
  projectKey?: string;
  /** 项目路径（由 `projectKey` 反解，解码不出来为空串）；仅 JetBrains 返回。 */
  projectPath?: string;
}

/** 临时备份的清理状态：cleaned 已回收；pending 已保留待下次维护重试；legacyRetained 旧操作无生命周期记录。 */
export type SessionBackupCleanupState = "cleaned" | "pending" | "legacyRetained";

/**
 * 临时备份残留（待清理 / 待恢复）：复制、同步、恢复报告共用同一结构。
 * `cleanupPending` 表示已完成但本轮没清理成功（下次切号重试）；`needsRecovery`
 * 表示必须保留材料、需要恢复流程或人工确认。
 */
export interface TemporaryFileInfo {
  operationId: string;
  sessionId?: string;
  title?: string;
  state: "cleanupPending" | "needsRecovery";
  reason: string;
}

/** 本次新建的副本（目标 UUID 由后端预分配）。 */
export interface CopyResult {
  id: string;
  newId: string;
  groupId: string;
  /** 待清理位置（已清理为 null）；仅表示待清理，不是可撤销备份。 */
  backup: string | null;
  /** 成功后立即清理：cleaned 已回收 / pending 待下次维护重试；旧后端可能缺字段。 */
  cleanupState?: SessionBackupCleanupState;
  /** 清理失败原因（`cleanupState` 为 pending 时有值）。 */
  cleanupError?: string;
  /**
   * 源会话正文有坏行、复制时做过容错处理时的人话说明（如「源会话末尾有 1 条不完整记录
   * （第 1000 行），已跳过」）。缺字段表示源正文一次校验通过，副本与原会话逐字同源。
   */
  sourceRepaired?: string;
  /**
   * 目标客户端缺账本表/列，副本不含会话用量。不算失败，只说明原因。
   *
   * R6-08：复制**默认不再搬历史消耗**（历史消耗归来源账号，副本只记录自己的新增），
   * 所以副本的「共消耗」显示 0 或偏少都属于预期；账号级消耗请看「积分统计」。
   */
  auxNote?: string;
  /**
   * 空间归属诊断（同一字段承载两种语义，文案由后端给出，前端只展示）：
   * 1) 目标账号有多个空间绑定同一目录，本次按目录新建了分组（不是串组）；
   * 2) 源会话本来有项目归属，但目标账号里既没能定位、也没能新建项目分组，
   *    副本未归入任何项目（源会话本身没有项目时不发这条说明）。
   * 缺字段表示副本的项目归属没有需要解释的异常。
   */
  spaceNote?: string;
}

/** 目标账号上已有真实有效的副本：复用而不是重复复制。 */
export interface LinkedCopyResult {
  id: string;
  sessionId: string;
  groupId: string;
}

/**
 * 工作记忆（`MEMORY.md`）跨账号同步结果（issue #209）。
 *
 * 只由复制弹窗里**默认不勾**的「同时复制工作记忆」触发；未勾选时报告里不出现 `memory`
 * 字段（后端连源文件都不读）。同步口径是**只追加不覆盖**：按行去重后把目标缺失的行
 * 追加到末尾，目标已有内容逐字保留。
 */
export interface SessionMemoryReport {
  /** 本次追加到目标账号的行数（`0` 表示无需同步或已是最新）。 */
  appendedLines: number;
  /** 非致命失败原因；缺字段表示正常（工作记忆不影响复制结果）。 */
  skippedReason?: string;
}

/** 切换时的会话复制报告；复制失败时后端只回 `error`（切换本身仍继续）。 */
export interface SessionCopyReport {
  sourceUid?: string;
  targetUid?: string;
  /** 跨档复制时带两侧档位；同档复制不出现这两个字段。 */
  sourceVariant?: WbVariant;
  targetVariant?: WbVariant;
  copied?: CopyResult[];
  alreadyLinked?: LinkedCopyResult[];
  errors?: { id: string; error: string }[];
  /** 仍有未完成的会话写入时为 true（失败项可重试，不会产生第二个副本）。 */
  needsRecovery?: boolean;
  /**
   * 有会话因「账本（用量 + 中断记录）写入失败」而整次失败时为 true：
   * 前端据此给「仍然复制（不带用量与中断记录）」的显式忽略入口。
   *
   * R6-08 起正常复制路径就不再搬 `session_usage`，这个入口只剩**失败兜底**一种用途
   * （让用户先拿到正文）；旧后端仍可能下发该字段，前端保持兼容。
   */
  canIgnoreAux?: boolean;
  /** 可忽略失败的会话 id（与 `errors` 一一对应，仅供「仍然复制」按钮使用）。 */
  auxFailed?: string[];
  /**
   * 本次请求带了显式忽略：副本不含历史消耗，不会后台补写。
   *
   * R6-08：正常复制的默认行为已经是「不搬历史消耗」（历史消耗归来源账号，
   * 副本只记录自己的新增），副本的「共消耗」显示 0 或偏少都属于预期。
   */
  auxSkipped?: boolean;
  /** 本次请求由恢复重放补完的会话 id（重试「仍然复制」时它们不会出现在 copied 里）。 */
  recovered?: string[];
  /** 临时备份残留（待清理/待恢复）；无异常时为空数组。 */
  temporaryFiles?: TemporaryFileInfo[];
  /** 工作记忆同步结果：只在勾选「同时复制工作记忆」时出现。 */
  memory?: SessionMemoryReport;
  error?: string;
}

/** 切换前对未完成会话写入的恢复结果。 */
export interface SessionRecoveryReport {
  recovered: number;
  abandoned: number;
  needsRecovery: { operationId: string; reason: string; retryable: boolean }[];
  /** 临时备份残留（待清理/待恢复）；无异常时为空数组。 */
  temporaryFiles?: TemporaryFileInfo[];
}

// ---------------------------------------------------------------------------
// 会话同步（关联组）：预览与执行契约，与 core / Tauri / HTTP 三端同形
// ---------------------------------------------------------------------------

/**
 * 同步判定结果（design §3.2 优先级表）：
 * `identical` 两边一致、`fastForward` 有新增可同步、`ahead` 仅目标账号有更新、
 * `diverge` 两边都改过需显式覆盖、`unknown` 无法确认。
 */
export type SessionSyncVerdict = "identical" | "fastForward" | "ahead" | "diverge" | "unknown";

/** 同步写入模式：只有后端 `availableModes` 里给出的模式才允许提交。 */
export type SessionSyncMode = "fastForward" | "overwrite" | "unifyOverwrite" | "statusOnly";

/** 关联组成员（不含正文）：`state` 为 active 时才算该账号的有效成员。 */
export interface SessionLinkMember {
  memberId: string;
  uid: string;
  accountId: string | null;
  sessionId: string;
  state: "active" | "stale" | "superseded";
}

/** 关联组的预览项；`defaultChecked` 与 `availableModes` 是勾选权限的唯一来源。 */
export interface SessionLinkPreviewGroup {
  groupId: string;
  title: string;
  cwd: string;
  verdict: SessionSyncVerdict;
  /** 来源独有记录数（多重集差集，仅用于向用户解释）。 */
  extraA: number;
  /** 目标独有记录数（多重集差集，仅用于向用户解释）。 */
  extraB: number;
  common: number;
  defaultChecked: boolean;
  /**
   * 正文写入模式；为空且没有 `archiveAction` 时才不可勾选
   * （identical / ahead / unknown / 预览凭据不可用，但仅同步归档仍可勾选）。
   */
  availableModes: SessionSyncMode[];
  reason: string;
  /**
   * 仅同步生命周期状态（把目标对齐为已归档），正文零写入。
   * 由后端在生成预览时给出，不随用户勾选变化。
   */
  archiveAction?: "statusOnly";
  /** 记录数（不是消息数）：不可验证时 source/target 为 0、baseline 为 null。 */
  recordCount: { source: number; target: number; baseline: number | null };
  source: SessionLinkMember | null;
  target: SessionLinkMember | null;
  /** 勾选时必须原样回传的预览凭据；缺失即不可勾选。 */
  previewToken?: string;
}

/** 关联会话预览：`supported` 为 false（或 storeStatus 为 unsupported）时不展示同步区块。 */
export interface SessionLinksPreview {
  supported: boolean;
  storeStatus: "ready" | "missing" | "unavailable" | "unsupported";
  storeError?: string;
  sourceUid: string;
  targetUid: string;
  /** 跨档预览时带两侧档位；同档预览不出现这两个字段。 */
  sourceVariant?: WbVariant;
  targetVariant?: WbVariant;
  groups: SessionLinkPreviewGroup[];
}

/** 一条同步选择：与预览凭据绑定，执行时后端会重新校验。 */
export interface SessionSyncSelection {
  groupId: string;
  previewToken: string;
  mode: SessionSyncMode;
}

/** 已同步的关联组（保留目标 sessionId 与标题）。 */
export interface SessionSyncResultItem {
  groupId: string;
  status: "synced";
  verdict: SessionSyncVerdict;
  mode: SessionSyncMode;
  sourceSessionId: string;
  targetSessionId: string;
  recordCount: { source: number; targetBefore: number; target: number };
  updatedAt: number;
  /** 待清理位置（已清理为 null）；旧操作可能仍返回目录路径。 */
  backup: string | null;
  backupManifest: string | null;
  /** 成功后立即清理：cleaned 表示临时备份已回收；pending 表示待下次维护重试。 */
  cleanupState?: SessionBackupCleanupState;
  cleanupError?: string;
  message: string;
}

/** 被跳过的关联组：`reasonCode` 为 previewStale 时说明预览已过期，不得显示为成功。 */
export interface SessionSyncSkippedItem {
  groupId: string;
  status: "skipped";
  reasonCode: string;
  message: string;
  verdict: SessionSyncVerdict | null;
}

/** 同步执行报告；`errors` 里可能是整批被拒（无 groupId）。 */
export interface SessionSyncReport {
  synced: SessionSyncResultItem[];
  skipped: SessionSyncSkippedItem[];
  errors: { groupId?: string; error: string }[];
  /** 仍有未完成/无法安全恢复的会话写入时为 true。 */
  needsRecovery?: boolean;
  /** 临时备份残留（待清理/待恢复）；无异常时为空数组。 */
  temporaryFiles?: TemporaryFileInfo[];
}

// ---------------------------------------------------------------------------
// Client-scoped session group directory
// ---------------------------------------------------------------------------

export type SessionGroupClient = "workbuddy" | "codebuddyIde" | "vscodeExt" | "jetbrains";

export type SessionGroupStatus = "latest" | "behind" | "diverge" | "missing" | "unknown";

export type SessionMemberVersionStatus = SessionGroupStatus | "stale" | "superseded";

export interface SessionGroupSummary {
  key: string;
  client: SessionGroupClient;
  variantScope: WbVariant | null;
  groupId: string;
  groupVariant: WbVariant;
  title: string;
  projectLabel: string;
  latestActivityAt: number;
  /**
   * 卡片摘要：最近一条用户消息（没有用户消息时取助手消息），约 120 字。
   *
   * 只下发 1 条以控制列表载荷；`null` / 缺省 = 正文里没有可读消息（前端不占位）。
   * 老后端没有该字段：按「没有摘要」处理，卡片不渲染摘要行。
   */
  excerpt?: string | null;
  memberCount: number;
  activeMemberCount: number;
  /**
   * 组内 `versionStatus === "behind"` 的成员数（「跨组安全批量同步」的候选副本数）。
   *
   * 老后端没有该字段：消费方按 0 处理（`selectSyncAllCandidates` 已用 `?? 0`），
   * 只影响确认框里的副本数，不影响能否同步。
   */
  behindMemberCount?: number;
  accountNames: string[];
  summaryStatus: SessionGroupStatus;
  summaryText: string;
  safeSourceMemberId: string | null;
  hasSafeSource: boolean;
}

export interface SessionGroupMemberDetail {
  memberId: string;
  accountId: string | null;
  uid: string;
  sessionId: string;
  accountName: string;
  variant: WbVariant;
  linkState: "active" | "stale" | "superseded";
  versionStatus: SessionMemberVersionStatus;
  title: string;
  projectLabel: string;
  updatedAt: number;
  recordCount: number | null;
  contentPreview?: { speaker: string; text: string }[];
  contentState: "ready" | "missing" | "unavailable";
  reason: string;
  canBeSource: boolean;
}

export interface SessionGroupList {
  client: SessionGroupClient;
  variantScope: WbVariant | null;
  storeStatus: "missing" | "ready" | "unavailable";
  storeError?: string;
  /**
   * 被列表隐藏的失效组数量（组内所有成员的会话行都已失效）。
   *
   * 老后端没有该字段：缺省按 0 处理。列表为空时仍要靠它触达「清理失效关联组」。
   */
  deadGroupCount?: number;
  /**
   * 清理会被拒绝的原因（`null` = 可以清理）。
   *
   * 后端用**同一次判定**产出：只要非空，「清理失效关联组」点下去必然整次失败（例如某个
   * 账号的会话索引读不全）。前端据此禁用入口并展示原因，而不是让用户白点一次。
   * 老后端没有该字段：缺省按「没有阻塞」处理。
   */
  pruneBlocked?: string | null;
  groups: SessionGroupSummary[];
}

/**
 * 清理失效关联组的回报。
 *
 * `dryRun: true` 表示「将要删除」（不写盘），`false` 表示已经写盘；
 * `removedGroups` / `removedMembers` 只统计受影响的关联关系，不涉及任何会话内容。
 */
export interface SessionGroupPruneReport {
  dryRun: boolean;
  removedGroups: number;
  removedMembers: number;
  groupIds: string[];
}

export interface SessionGroupDetail extends SessionGroupSummary {
  members: SessionGroupMemberDetail[];
  addTargets: AccountMeta[];
  divergence?: {
    commonMemberIds: string[];
    branches: string[][];
  };
}

export interface SessionGroupPairPreview {
  client: SessionGroupClient;
  variantScope: WbVariant | null;
  groupId: string;
  sourceMemberId: string;
  targetMemberId: string;
  verdict: SessionSyncVerdict;
  availableModes: SessionSyncMode[];
  previewToken: string | null;
  reason: string;
  recordCount: { source: number; target: number; baseline: number | null } | null;
  extraTargetCount: number;
}

/** One read-only plan for making every active copy match a chosen group member. */
export interface SessionGroupUnifyPlan {
  client: SessionGroupClient;
  groupId: string;
  sourceMemberId: string;
  sourceName: string;
  targets: {
    memberId: string;
    accountName: string;
    preview: SessionGroupPairPreview | null;
    error: string | null;
  }[];
}

/** A client-reported current login, matched by stable UID or saved account ID. */
export interface SessionGroupCurrentAccount {
  variant?: WbVariant;
  uid?: string | null;
  accountId?: string | null;
  running?: boolean;
}

/**
 * 「跨组安全批量同步」的跳过项（`skipped` / `skippedGroups` 共用一套判定口径）。
 *
 * 跨组汇总后每条都带 `groupId`；`targetMemberId` 只在「单个副本复核后不再快进」时出现
 * （整组跳过用 `reasonCode` 说明原因，没有目标成员）。
 */
export interface SessionGroupAllSafeSkippedItem {
  groupId: string;
  targetMemberId?: string;
  status?: "skipped";
  verdict?: SessionSyncVerdict | null;
  /** `detailFailed` / `noSafeSource` / `noBehind`（整组）/ `previewFailed` / `recheckNotFastForward`（单目标）。 */
  reasonCode: string;
  message: string;
}

/**
 * 跨组安全批量同步报告（issue #231）。
 *
 * `groups` 为本次涉及的组与目标数，`skippedGroups` 为整组跳过的原因；`synced` / `skipped` /
 * `errors` 是跨组汇总，条目内一律带 `groupId` 便于回溯。编辑器生命周期由后端统一收口：
 * 全程只关 / 开一次宿主，`restartedEditor` 表示本次确实由工具关闭并重开成功。
 */
export interface SessionGroupAllSafeReport {
  client: SessionGroupClient;
  variantScope: WbVariant | null;
  groups: { groupId: string; title: string; targetCount: number }[];
  skippedGroups: { groupId: string; title: string; reasonCode: string; message: string }[];
  synced: SessionSyncResultItem[];
  skipped: SessionGroupAllSafeSkippedItem[];
  errors: { groupId?: string; error: string }[];
  needsRecovery: boolean;
  temporaryFiles: TemporaryFileInfo[];
  restartedEditor?: boolean;
  /** 写入已完成但宿主未能自动重新打开（无 `errors` 数组时用它兜底）。 */
  editorError?: string;
}

export interface SessionGroupActionReport {
  client: SessionGroupClient;
  groupId: string;
  sourceMemberId?: string;
  targetMemberId?: string;
  synced: SessionSyncResultItem[];
  skipped: SessionSyncSkippedItem[];
  errors: { groupId?: string; error: string }[];
  needsRecovery?: boolean;
  temporaryFiles?: TemporaryFileInfo[];
  restartedVariants?: WbVariant[];
  /** 插件侧：本次由 workbuddy-shift 关闭并成功重开了 VS Code。 */
  restartedEditor?: boolean;
  /** 插件侧：写入已完成但 VS Code 未能自动重新打开（无 `errors` 数组的入口用它兜底）。 */
  editorError?: string;
}

/** 一条未完成的会话写入（设置页「未完成的写入」）。 */
export interface PendingSessionWrite {
  operationId: string;
  /** `sync`（同步）或 `copy`（复制）。 */
  kind: string;
  targetSessionId: string;
  /** 目标会话标题；会话行已不存在时为 `null`。 */
  title: string | null;
  variant: WbVariant;
  updatedAt: number;
  lastError: string | null;
  /** 是否可「结束这次写入」：复制的中断记录牵涉新建的会话行，不提供。 */
  discardable: boolean;
}

/** 「结束这次写入」的结果。 */
export interface DiscardedSessionWrite {
  operationId: string;
  /** 是否保留了备份（`true` 时维护入口会继续把这条备份列为待确认）。 */
  backupKept: boolean;
  /** 结束后的状态说明，可直接展示。 */
  note: string;
}
