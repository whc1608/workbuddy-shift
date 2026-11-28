

import { WbVariant } from "./account";
import { SessionSyncMode, SessionSyncVerdict } from "./session";

export interface CodeBuddyCliStatus {
  configured: boolean;
  authMode?: "settings-env" | "api-key-helper";
  environmentOverride?: boolean;
  settingsPresent: boolean;
  helperPresent: boolean;
  helperSupportsAccountIds: boolean;
  helperCurrent?: boolean;
  migrationRequired?: boolean;
  /**
   * 需要用户介入的脱节：settings.json 里的 Token 已长期匹配不上账号库。
   *
   * 与 `syncInProgress` 互斥。保活刷新造成的短暂不一致不算脱节。
   */
  syncPending?: boolean;
  /**
   * 保活刷新刚换完账号库的 Token、settings.json 尚未同步的正常中间态。
   *
   * 仅在账号库 mtime 落在同步宽限窗口内时为真；此时不该提示用户点「更新认证」。
   */
  syncInProgress?: boolean;
  activeIndex: number | null;
  activeAccountId: string | null;
  activeAccountName: string | null;
  /** 当前 CLI 账号所属档位；尚未接入时缺省。 */
  activeAccountVariant?: WbVariant | null;
  accountCount: number;
  statePath: string;
}

export interface CodeBuddyCliSwitchResult {
  ok: boolean;
  configured: boolean;
  synced: boolean;
  verified?: boolean;
  authMode?: "settings-env" | "api-key-helper";
  activeIndex?: number;
  activeAccountId?: string;
  source?: string;
  skipped?: boolean;
  regionChanged?: boolean;
  cliClosed?: boolean;
  closedProcessCount?: number;
  message?: string;
  error?: string;
}

export interface CodeBuddyCliInstallResult {
  ok: boolean;
  configured: boolean;
  helperPresent: boolean;
  helperSupportsAccountIds: boolean;
  verified?: boolean;
  authMode?: "settings-env" | "api-key-helper";
  message?: string;
  error?: string;
}

/** CodeBuddy CN IDE（桌面客户端）状态；与 CodeBuddy CLI 独立。 */
export interface CodeBuddyCnIdeStatus {
  installed: boolean;
  running: boolean;
  /**
   * 是否存在 IDE 登录态（`state.vscdb` 里有会话 secret 行）。
   *
   * 只读查询、不解密；查询失败或文件不存在时为 false。仅用于文案与入口判定，
   * 不参与切换判定。国内版与国际版 IDE 状态都返回该字段（旧后端可能缺省 `undefined`）。
   */
  loggedIn?: boolean;
  dataDir: string | null;
  dbPath: string | null;
  dbExists: boolean;
  appPath: string | null;
  activeAccountId: string | null;
  activeAccountName: string | null;
  detectedFrom?: string;
  statePath?: string;
}

export interface CodeBuddyCnIdeSwitchResult {
  ok: boolean;
  account: string;
  accountId: string;
  dbPath?: string;
  restarted?: boolean;
  message?: string;
  /** 切换时复制会话的结果（未勾选复制时不返回）。 */
  sessionCopy?: VscodeSessionCopyResult;
  /** 切换时同步关联会话的结果（未勾选同步时不返回）。 */
  sessionSync?: VscodeSessionSyncReport;
}

/** VS Code 内 CodeBuddy 扩展（tencent-cloud.coding-copilot）状态；与 CN IDE / CLI 独立。 */
export interface VscodeExtStatus {
  /** VS Code 用户数据目录是否存在。 */
  installed: boolean;
  /** CodeBuddy 扩展是否已安装（globalStorage/<extensionId> 存在）。 */
  extensionInstalled: boolean;
  running: boolean;
  /**
   * 是否存在扩展登录态（`state.vscdb` 里有会话 secret 行）。
   *
   * 只读查询、不解密；查询失败或文件不存在时为 false。仅用于提示文案，
   * 不参与切换判定（false 时切换按新会话写入，同样可用）。
   */
  loggedIn: boolean;
  dataDir: string | null;
  dbPath: string | null;
  dbExists: boolean;
  activeAccountId: string | null;
  activeAccountName: string | null;
  detectedFrom?: string;
  statePath?: string;
}

export interface VscodeExtSwitchResult {
  ok: boolean;
  account: string;
  accountId: string;
  dbPath?: string;
  /** 本次是否真的执行了「关闭并重新打开 VS Code」（切换前未运行时为 false）。 */
  restarted?: boolean;
  /** 本次切换是否由 workbuddy-shift 关闭了 VS Code（为 false 时表示编辑器本来没运行）。 */
  closedByUs?: boolean;
  /** 切换前是否读到了既有会话；false = 扩展未登录，按新会话载荷写入。 */
  existingSession?: boolean;
  /**
   * 后端生成的生效说明：区分「已重新打开 VS Code」「自动重开失败（含原因）」
   * 「本来未运行」三种情形，不再出现「重载窗口生效」。
   */
  message?: string;
  /** 切换时复制会话的结果（未勾选复制时不返回）。 */
  sessionCopy?: VscodeSessionCopyResult;
  /** 切换时同步关联会话的结果（未勾选同步时不返回）。 */
  sessionSync?: VscodeSessionSyncReport;
}

/** JetBrains IDE 的一条配置目录状态（IDEA / PyCharm / GoLand / Android Studio 等各自独立）。 */
export interface JetbrainsTargetStatus {
  /** 配置目录名（如 "PyCharm2026.2"）。 */
  configDir: string;
  /** 是否安装了 CodeBuddy 插件（plugins/coding-copilot* 目录存在）。 */
  pluginInstalled: boolean;
  running: boolean;
  /** 是否存在插件登录态（secret-storage.xml 里有会话 secret）。 */
  loggedIn: boolean;
  secretPath: string;
}

/** 运行中实例的配置目录映射来源；`unmapped` = 映射失败，切换会被拒绝。 */
export type JetbrainsMappingSource = "product-info" | "fallback" | "unmapped";

/** 一个运行中的 JetBrains IDE 实例的映射诊断（不含任何 secret）。 */
export interface JetbrainsRunningProcess {
  pid: number;
  /** 产品名（product-info 的 name 或 exe 名）。 */
  product: string | null;
  version: string | null;
  /** 映射到的配置目录名；null = 映射失败。 */
  configDir: string | null;
  mappingSource: JetbrainsMappingSource;
  /** 降级 / 失败原因。 */
  mappingNote: string | null;
  launchTarget: string | null;
}

/** 配置根下一个子目录的扫描结论（用于区分「没装插件」与「没识别到目录」）。 */
export interface JetbrainsScannedDir {
  name: string;
  /** 是否通过配置目录名判定（false 的目录不会出现在切换列表）。 */
  accepted: boolean;
  pluginDirs: string[];
  pluginInstalled: boolean;
  hasSecretFile: boolean;
}

/** 一个 JetBrains 配置根的存在性（两个根并列，Android Studio 在 Google 根）。 */
export interface JetbrainsConfigRootState {
  /** 配置根的绝对路径。 */
  path: string;
  /** 该根当前是否存在（不存在不算错误：只装了 IDEA 的机器没有 Google 根）。 */
  exists: boolean;
}

/** 「未检测到 IDE」时折叠展示的诊断信息。 */
export interface JetbrainsDiagnostics {
  /** 全部候选配置根（`.../JetBrains` 与 `.../Google`），只读不写。 */
  configRoots: string[];
  /** 与 `configRoots` 一一对应的存在性结论。 */
  rootStates: JetbrainsConfigRootState[];
  scannedDirs: JetbrainsScannedDir[];
  acceptedDirs: string[];
  pluginInstalledDirs: string[];
  running: JetbrainsRunningProcess[];
  unmappedRunningCount: number;
}

/** 运行中但映射不到配置目录的实例（切换前必须让用户先退出）。 */
export interface JetbrainsUnmappedProcess {
  pid: number;
  product: string | null;
  version: string | null;
  path: string | null;
  reason: string | null;
}

export interface JetbrainsStatus {
  /** 是否存在任一受支持的 JetBrains 配置目录。 */
  installed: boolean;
  /** 是否至少一个配置目录安装了 CodeBuddy 插件。 */
  pluginInstalled: boolean;
  /** 是否有「装了插件」的 IDE 正在运行。 */
  running: boolean;
  /** 是否任一装了插件的目标存在登录态。 */
  loggedIn: boolean;
  /** 全部候选配置根路径（`.../JetBrains`、`.../Google`）。 */
  configRoots: string[];
  targets: JetbrainsTargetStatus[];
  activeAccountId: string | null;
  activeAccountName: string | null;
  detectedFrom?: string;
  statePath?: string;
  /** 运行中但映射不到配置目录的实例；非空时后端拒绝写入。 */
  unmappedRunning?: JetbrainsUnmappedProcess[];
  unmappedRunningHint?: string | null;
  diagnostics?: JetbrainsDiagnostics | null;
}

export interface JetbrainsSwitchResult {
  ok: boolean;
  account: string;
  accountId: string;
  /** 本次写入的配置目录名列表（一次切换覆盖所有装了插件的 IDE）。 */
  written?: string[];
  /** 本次是否真的执行了「关闭并重新打开 IDE」。 */
  restarted?: boolean;
  /** 本次切换是否由 workbuddy-shift 关闭了 IDE。 */
  closedByUs?: boolean;
  message?: string;
}

/** VS Code 扩展的一条可复制会话。 */
export interface VscodeSession {
  /** 会话 id（32 位小写 hex）。 */
  id: string;
  /** 工作区目录名 md5(工作区)（32 位小写 hex，不可反解为路径）。 */
  workspaceHash: string;
  /** 会话标题；无标题时为 "(无标题)"。 */
  title: string;
  /** 最近消息时间（epoch 毫秒，0 表示未知）。 */
  updatedAt: number;
  /** 会话类型（如 "craft"）。 */
  type: string;
  /** 是否包含正文消息。 */
  hasHistory: boolean;
  /**
   * 会话所在存储：缺省 = 旧目录树（`history/<工作区>/index.json`）；
   * `"db"` = 新版 IDE 写进 CLI 会话库（`~/.codebuddy/codebuddy.db`）的会话。
   *
   * 新版会话的正文是与终端 CLI 共用的 JSONL，复制时只改会话库里的账号归属，不搬文件。
   */
  store?: "legacy" | "db";
}

/** 复制某项会话的引用（工作区 hash + 会话 id）。 */
export interface VscodeSessionRef {
  workspaceHash: string;
  conversationId: string;
  /** 与 [`VscodeSession.store`] 同义；缺省按旧目录树处理（旧后端不认识该字段也不受影响）。 */
  store?: "legacy" | "db";
}

/** VS Code 扩展会话复制结果。 */
export interface VscodeSessionCopyResult {
  sourceUid: string | null;
  targetUid: string;
  copied: { workspaceHash: string; oldId: string; newId: string; messages: number }[];
  errors?: { workspaceHash: string; conversationId: string; error: string }[];
  /** 索引备份根目录（便于用户找回）。 */
  backup?: string;
  /**
   * 已复制但未能建立关联的条目（复制成功、登记失败；登记失败不回滚复制）。
   * 前端据此提示「已复制但未建立关联」，不得静默当成成功。
   */
  linkErrors?: { workspaceHash?: string; conversationId?: string; error: string }[];
}

/** VS Code 插件侧「关联会话」的同步结果项（与 core 报告同形）。 */
export interface VscodeSessionSyncResultItem {
  groupId: string;
  status: "synced";
  verdict: SessionSyncVerdict;
  mode: SessionSyncMode;
  /** 记录数（条）：source 为来源当前条数，targetBefore/target 为副本写入前后条数。 */
  recordCount: { source: number; targetBefore: number; target: number };
  /** 附件合并结果（快进模式补入的附件）。 */
  assets: { copied: number; overwritten: number };
  /** 本次备份目录（快进为索引备份，覆盖为整目录备份）。 */
  backup: string;
  message: string;
}

/** 被跳过的同步项（`reasonCode` 为 previewStale 时说明预览已过期，不得显示为成功）。 */
export interface VscodeSessionSyncSkippedItem {
  groupId: string;
  status: "skipped";
  reasonCode: string;
  message: string;
  verdict: SessionSyncVerdict | null;
}

/** VS Code 插件侧「关联会话」同步报告；`errors` 里可能是整批被拒（无 groupId）。 */
export interface VscodeSessionSyncReport {
  synced: VscodeSessionSyncResultItem[];
  skipped: VscodeSessionSyncSkippedItem[];
  errors: { groupId?: string; error: string }[];
}

/** 可复制会话列表。 */
export interface VscodeSessionList {
  sourceUid: string | null;
  sessions: VscodeSession[];
  /** 无法解析（损坏）的工作区索引数量。 */
  skipped?: number;
  /**
   * 扩展数据根目录；`null` 表示未找到（与「有目录但没有会话」区分）。
   * 可选：旧后端不返回该字段时为 `undefined`，前端按旧文案处理。
   */
  dataRoot?: string | null;
}
