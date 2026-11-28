import { SessionCopyReport, SessionRecoveryReport, SessionSyncReport } from "./session";

/**
 * WorkBuddy 客户端档位：国内版（cn）/ 国际版（ai）。
 * 后端以字符串返回，历史数据与旧响应可能缺省该字段，读取时统一按国内版处理。
 */
export type WbVariant = "cn" | "ai";

/**
 * 账号卡片显示名取哪个字段（每账号独立的本地偏好）。
 *
 * R5-07 起显示名统一为「备注优先」（见 `src/lib/account-display.ts` 的 `displayName`）：
 * - `nickname`（缺省/异常）：账号名 = 无备注时的回退，有备注时仍显示备注；
 * - `phone`：手机号**始终压过备注**（唯一例外）；
 * - `note`：与新默认等价，仅为兼容存量数据保留，弹窗里映射到「账号名」。
 */
export type DisplayField = "nickname" | "phone" | "note";

/**
 * 账号标记的语义色（R5-04）。与后端 `MARK_COLORS` 一一对应，顺序也保持一致。
 *
 * - `warning`：需要留意（如「即将限额」）；
 * - `danger`：明确不可用 / 已废弃（浅底 + 同色文字，不做实心 destructive）；
 * - `info`：中性提示；
 * - `success`：正向状态；
 * - `neutral`：纯备注性质。
 */
export type AccountMarkColor = "warning" | "danger" | "info" | "success" | "neutral";

/**
 * 账号自定义标记（R5-04）：单账号最多一条，重复保存即覆盖。
 *
 * 只保存在本机账号库（`accounts.json` 的同级字段 `mark`），随账号备份导出/导入，
 * 不同步官方；到期后卡片上不再显示图标，但**记录保留**，编辑时提示「已过期」可续期。
 */
export interface AccountMark {
  /** 标记文本（1–16 字，纯文本，不含换行）。 */
  text: string;
  color: AccountMarkColor;
  /** 创建时刻（绝对毫秒），由后端写入，客户端传值会被忽略。 */
  createdAt: number;
  /** 到期时刻（绝对毫秒）；`null` = 不自动过期（只能手动解除）。 */
  expiresAt: number | null;
}

/**
 * 写入用的标记载荷：`createdAt` 由后端生成，客户端不传（传了也会被忽略）。
 *
 * `expiresAt` 必须是 `null`（不自动过期）或**晚于当前时刻**的毫秒数，
 * 过去时间会被后端拒绝（「到期时间必须晚于当前时间」）。
 */
export type UpdateAccountMark = Pick<AccountMark, "text" | "color" | "expiresAt">;

/**
 * 「最后使用」来源端标签（issue #184）：账号最后一次被切到哪个客户端。
 *
 * 与后端 `LAST_USED_CLIENTS` 一一对应；后端遇到不认识的标签会下发 `null`，
 * 前端另外给未知字符串留了「其它客户端」兜底文案，所以这里保持宽松的 `string`。
 */
export type LastUsedClient =
  | "workbuddy"
  | "codebuddy-ide"
  | "codebuddy-cn-ide"
  | "vscode"
  | "jetbrains"
  | "cli"
  | "rotate";

/**
 * 凭据形态（R6-02）：只描述**类型**，永远不含凭据值。
 *
 * - `plain`：账号库里是明文 access / refresh token，本工具可直接使用；
 * - `envelope`：账号库里是 WorkBuddy 的加密信封，本工具读不到明文（可用本机客户端凭据读取）；
 * - `missing`：没有任何可用凭据（需要重新登录）。
 */
export type CredentialKind = "plain" | "envelope" | "missing";

export interface AccountMeta {
  id: string;
  uid: string | null;
  email: string | null;
  nickname: string | null;
  /** 官方手机号（实时读 profile_raw，仅国内版账号可能有）。 */
  phoneNumber?: string | null;
  /** 本地备注（用户自填，不来自官方数据）。 */
  note?: string | null;
  /** 本地显示字段偏好。 */
  displayField?: DisplayField | null;
  /**
   * 本地自定义标记（R5-04）：`null` / 缺省（旧账号库）= 无标记。
   * 后端下发的 `mark` 已做过类型校验，任何一项不合法都会整体折叠成 `null`。
   */
  mark?: AccountMark | null;
  enterpriseName: string | null;
  expiresAt: number | null;
  refreshExpiresAt: number | null;
  refreshedAt: number | null;
  createdAt: number | null;
  needsRelogin: boolean;
  needsReloginReason: string | null;
  /**
   * 凭据形态（R6-02）：见 `CredentialKind`。**可选**——旧后端与历史快照都没有该字段，
   * 缺省（`undefined`）按「形态未知」处理，卡片行为与改造前完全一致（不报错、不空白）。
   *
   * 信封账号（`envelope`）不再显示「需重新登录」：它没坏，只是本工具读不到明文。
   */
  credentialKind?: CredentialKind;
  /**
   * 本次刷新「被跳过」的机器可读标记（issue #202）；只有真的跳过时才出现。
   *
   * 当前唯一取值 `credential_lock_busy`：抢不到 `locks/credentials.lock` 时 core 直接放弃，
   * **一个字都没写**（既不是成功，也不是账号失效）。只在「刷新令牌」入口的响应里下发，
   * 账号库与列表接口都不带这个字段，所以是可选字段。
   */
  refreshSkipped?: string;
  /**
   * 上面那次跳过的用户可读原因，可直接展示给用户（已包含「未写入任何凭据」的说明）。
   * 与 `refreshSkipped` 成对出现；未跳过时两者都缺省。
   */
  refreshSkippedReason?: string;
  /** 账号所属档位；缺省（旧后端/历史账号）按国内版处理。 */
  variant?: WbVariant;
  /**
   * 企业版账号（有 enterpriseId，或 type 为 ultimate/exclusive）；
   * 缺省（旧后端/历史账号）按非企业处理。
   */
  isEnterprise?: boolean;
  /**
   * 本机最后一次把该账号切到某个客户端的时刻（绝对毫秒，issue #184）。
   *
   * 只在切换**已生效之后**由后端写入；缺省 / `null`（旧账号库、从未切过）= 卡片不显示
   * 「最近切换」徽标，排序时恒排最后。属于本地字段，官方资料重采不会覆盖它。
   */
  lastUsedAt?: number | null;
  /**
   * 上面那次切换的来源端标签；`null` = 记录里没有可识别的来源端。
   *
   * 后端只接受 `LastUsedClient` 里的取值，未知取值折叠成 `null`。
   */
  lastUsedClient?: LastUsedClient | null;
}

export interface AppStatus {
  running: boolean;
  authFile: string;
  current: {
    uid: string | null;
    nickname: string | null;
    email: string | null;
  } | null;
  appPath: string;
  version: string;
  /** 上述字段所属档位；缺省按国内版处理。 */
  variant?: WbVariant;
}

export interface OAuthStartResult {
  loginId: string;
  verificationUri: string;
  expiresIn: number;
}

export interface OAuthPollResult {
  done: boolean;
  result?: AccountMeta;
  error?: string;
}

/** 导出文件中的完整账号记录（含 token，仅导出命令返回；字段与账号库原始记录一致）。 */
export interface AccountRecord {
  id?: string;
  uid?: string | null;
  nickname?: string | null;
  email?: string | null;
  access_token?: string | null;
  refresh_token?: string | null;
  token_type?: string | null;
  domain?: string | null;
  expiresAt?: number | null;
  refreshExpiresAt?: number | null;
  auth_raw?: unknown;
  profile_raw?: unknown;
  createdAt?: number | null;
  [key: string]: unknown;
}

/** 导入文件账号的脱敏预览（不含 token）。 */
export interface ImportPreviewAccount {
  index: number;
  uid: string | null;
  nickname: string | null;
  email: string | null;
  hasToken: boolean;
  /** access_token 为 WorkBuddy 加密信封：可导入，但仅切换可用（签到/积分不可用）。 */
  encrypted: boolean;
  /**
   * access_token 为第三方号池的加密密文（`enc:v1:` 前缀）：本机解不开，导入会整体跳过。
   *
   * 老后端没有该字段：缺省按「不是第三方密文」处理。
   */
  foreignEncrypted?: boolean;
}

/** 导入结果计数。 */
export interface ImportResult {
  ok: boolean;
  imported: number;
  skipped: number;
  overwritten: number;
}

/**
 * 加密导出（issue #186）的结果。
 *
 * `path` 是实际写入的文件路径（保存对话框选的那一个），`count` 是写进文件的账号数。
 * 口令一概不回传：后端只报"写到了哪里、写了几个"。
 */
export interface EncryptedExportResult {
  ok: boolean;
  path: string;
  count: number;
}

/**
 * 启用中的「自动化」定时任务摘要（切号提示用）。
 *
 * 任务按 `owner_user_id` 绑账号：切号后原账号的任务不再执行，客户端界面却不给任何提示，
 * 因此切号弹窗要用 `count` 主动提醒；`names` 最多 5 条，超出由前端显示「等 N 个」。
 */
export interface AutomationTasksSummary {
  count: number;
  names: string[];
  /** 同步开关（默认关闭）：开启后切号会把简单任务复制到目标账号并暂停源任务。 */
  enabled: boolean;
}

/** 切号时自动化任务的同步报告；未开启同步或未重启时不出现在结果里。 */
export interface AutomationSyncReport {
  ok: boolean;
  backup?: string;
  copied: { id: string; name: string }[];
  paused: { id: string; name: string }[];
  skipped: { name: string; reason: string }[];
  error?: string;
}

export interface SwitchResult {
  ok: boolean;
  account: string;
  /** 目标账号自身档位；缺省按国内版处理。 */
  variant?: WbVariant;
  backup: string | null;
  sessionCopy?: SessionCopyReport;
  /** 本次的会话同步报告（未勾选同步时不返回）；含跳过与失败原因，不只是成功数。 */
  sessionSync?: SessionSyncReport;
  sessionRecovery?: SessionRecoveryReport;
  /** 自动化定时任务的同步报告（开关关闭或未勾选重启时不返回）。 */
  automationSync?: AutomationSyncReport;
}
