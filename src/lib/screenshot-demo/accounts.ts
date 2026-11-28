import { displayName } from "../account-display";
import { normalizeVariant } from "../variant";
import type {
  AccountMark,
  AccountMeta,
  AppStatus,
  AutoRotateConfig,
  CodeBuddyCliStatus,
  CodeBuddyCliSwitchResult,
  CodeBuddyCnIdeStatus,
  DisplayField,
  GithubConfig,
  JetbrainsStatus,
  RotateStatus,
  SessionGroupClient,
  SessionGroupDetail,
  VscodeExtStatus,
} from "../types";

/**
 * 账号与客户端演示数据：账号库 fixture、时间辅助函数、页面上下文类型，以及账号/客户端状态分发。
 *
 * 该文件同时是演示数据的「基底」：credits / sessions / settings 都从这里取账号 fixture 与时间函数，
 * DemoContext 由 index 组装，避免各域重复拼装上下文。
 */

/** 演示响应分发上下文：index 每次请求组装一次，各域只读。 */
export interface DemoContext {
  demoAccounts: AccountMeta[];
  appStatus: AppStatus;
  activeIndex: number;
  cliStatus: CodeBuddyCliStatus;
  config: AutoRotateConfig;
  rotateStatus: RotateStatus;
  githubConfig: GithubConfig;
  groupClient: SessionGroupClient | undefined;
  groupScope: "cn" | "ai" | null;
  demoGroups: SessionGroupDetail[];
}

export const ACCOUNT_DEMO_COMMANDS = [
  "get_status",
  "get_accounts",
  "get_codebuddy_cli_status",
  "get_codebuddy_cn_ide_status",
  "get_codebuddy_ide_status",
  "get_vscode_ext_status",
  "get_jetbrains_status",
  "switch_codebuddy_cli_account",
  "update_account_display",
] as const;

// 演示数据按档位分组：前三行是国内版 fixture，后两行是国际版；D 是企业版（国内）演示账号，
// 只覆盖账号页卡片形态（「企业版」标注 + 企业配额 / 不限量）；最后一条是「资源包矩阵」，
// 只用于积分明细的取证（18 个资源包 / 四态），形态与个人版一致。
// 国际版只覆盖账号页（及数据同源的积分统计视图）；Token 统计的 workbuddy-ai 源、
// 限额台账、签到、旅行、轮换保持原有空态，见 `emptyTokenSource` 与各档位能力函数。
export const intlAccountA: AccountMeta = { id: "demo-account-ai-a", uid: "demo-intl-001", email: "intl-a@example.com", nickname: "国际版 A", enterpriseName: "Demo Workspace", expiresAt: 0, refreshExpiresAt: 0, refreshedAt: 0, createdAt: 0, needsRelogin: false, needsReloginReason: null, variant: "ai" };
const intlAccountB: AccountMeta = { id: "demo-account-ai-b", uid: "demo-intl-002", email: "intl-b@example.com", nickname: "国际版 B", enterpriseName: "Demo Workspace", expiresAt: 0, refreshExpiresAt: 0, refreshedAt: 0, createdAt: 0, needsRelogin: false, needsReloginReason: null, variant: "ai" };

export const accounts: AccountMeta[] = [
  // 演示账号 A 带一条 2 小时后到期的标记（R5-04）：截图上能看到状态区首位的标签图标，
  // 悬停显示「即将限额 / 至 … · 剩 2 小时 / 创建于 …」。
  { id: "demo-account-a", uid: "demo-user-001", email: "test-a@example.com", nickname: "测试 A", phoneNumber: "138 0013 8001", note: "主力账号", enterpriseName: "Demo Workspace", expiresAt: 0, refreshExpiresAt: 0, refreshedAt: 0, createdAt: 0, needsRelogin: false, needsReloginReason: null, variant: "cn", mark: { text: "即将限额", color: "warning", createdAt: Date.now(), expiresAt: Date.now() + 2 * 3_600_000 } },
  { id: "demo-account-b", uid: "demo-user-002", email: "test-b@example.com", nickname: "测试 B", phoneNumber: "138 0013 8002", note: "备用号", displayField: "note", enterpriseName: "Demo Workspace", expiresAt: 0, refreshExpiresAt: 0, refreshedAt: 0, createdAt: 0, needsRelogin: false, needsReloginReason: null, variant: "cn" },
  // 企业版有限配额：卡片应显示「企业版」徽标 + 单条「企业配额」。
  { id: "demo-account-c", uid: "demo-user-003", email: "test-c@example.com", nickname: "测试 C", enterpriseName: "Demo Workspace", expiresAt: 0, refreshExpiresAt: 0, refreshedAt: 0, createdAt: 0, needsRelogin: false, needsReloginReason: null, variant: "cn", isEnterprise: true },
  intlAccountA,
  intlAccountB,
  // 企业版不限量（limitNum=-1）：卡片应显示「不限量」+「企业配额」，无积分包。
  { id: "demo-account-d", uid: "demo-user-004", email: "test-d@example.com", nickname: "测试 D", enterpriseName: "Demo Workspace", expiresAt: 0, refreshExpiresAt: 0, refreshedAt: 0, createdAt: 0, needsRelogin: false, needsReloginReason: null, variant: "cn", isEnterprise: true },
  // 资源包矩阵（R7-T09）：只承载「积分明细」的取证数据（18 个资源包 + 四态）。
  // 独立成账号而不是塞进 A/B：A 的「1 行卡片」、B 的「2 个积分包」是既有演示形态，
  // 改它们会让账号卡截图基线整体失效。昵称/邮箱都是演示占位，不含真实账号信息。
  { id: "demo-account-matrix", uid: "demo-user-005", email: "test-e@example.com", nickname: "演示账号 · 资源包矩阵", enterpriseName: "Demo Workspace", expiresAt: 0, refreshExpiresAt: 0, refreshedAt: 0, createdAt: 0, needsRelogin: false, needsReloginReason: null, variant: "cn" },
];

/** 演示模式中的临时 CLI 当前账号，仅存在于本次页面会话。 */
let demoActiveCliAccountId = accounts[0].id;

/**
 * 积分统计里的「当前账号」：后端按账号列表构造 `current_account_ids`（列表内即当前），
 * 前端档位视图只把 `isCurrent` 账号计入「当前剩余」。演示里各档位取一个代表账号，
 * 使国际版视图也有非零的当前剩余；国内版仍只取账号 A，保持既有数值不变。
 */
export const currentAccountIds = new Set([accounts[0].id, intlAccountA.id]);

export function startOfToday(): Date {
  const date = new Date();
  date.setHours(0, 0, 0, 0);
  return date;
}

export function localDate(daysAgo: number): string {
  const date = startOfToday();
  date.setDate(date.getDate() - daysAgo);
  return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`;
}

export function atLocalTime(daysAgo: number, hour: number, minute: number): number {
  const date = startOfToday();
  date.setDate(date.getDate() - daysAgo);
  date.setHours(hour, minute, 0, 0);
  return date.getTime();
}

export function futureAt(daysAhead: number, hour = 23, minute = 59): number {
  const date = startOfToday();
  date.setDate(date.getDate() + daysAhead);
  date.setHours(hour, minute, 0, 0);
  return date.getTime();
}

export function hydratedAccounts(): AccountMeta[] {
  return accounts.map((account, index) => ({
    ...account,
    expiresAt: futureAt(12 + index * 5, 18, 30),
    refreshExpiresAt: futureAt(40 + index * 7),
    refreshedAt: atLocalTime(0, 9, 12 + index * 7),
    createdAt: atLocalTime(45 + index * 19, 10, 0),
  }));
}

/** 演示模式中的临时 CLI 当前账号，仅存在于本次页面会话。 */
export function getDemoActiveCliAccountId(): string {
  return demoActiveCliAccountId;
}

export function setDemoActiveCliAccountId(id: string): void {
  demoActiveCliAccountId = id;
}

export function accountsDemoResponse(
  command: string,
  ctx: DemoContext,
  args?: Record<string, unknown>,
): unknown | undefined {
  const { demoAccounts, appStatus, cliStatus } = ctx;
  switch (command) {
    // 档位随请求回显：两个档位各有一套演示账号，国际版同样回显「运行中 + 当前账号」。
    case "get_status": {
      const variant = normalizeVariant(args?.variant);
      if (variant === "ai") {
        return {
          ...appStatus,
          running: true,
          current: { uid: intlAccountA.uid, nickname: intlAccountA.nickname, email: intlAccountA.email },
          authFile: "/demo/workbuddy-ai/auth.json",
          appPath: "/demo/WorkBuddy AI.app",
          variant,
        };
      }
      return { ...appStatus, variant };
    }
    case "get_accounts": return { accounts: demoAccounts };

    case "get_codebuddy_cli_status": return cliStatus;
    // 让演示里存在一个「CodeBuddy IDE 当前账号」：否则 IDE 标记与选中态染色（淡紫）在演示里永远不可见。
    // 取第二个账号，使三张卡各自演示一种形态（A 占位行 / B IDE 选中 / C 查看全部）。
    case "get_codebuddy_cn_ide_status": return {
      installed: true,
      running: true,
      loggedIn: true,
      dataDir: "/demo/codebuddy-cn-ide",
      dbPath: "/demo/codebuddy-cn-ide/state.vscdb",
      dbExists: true,
      appPath: "/demo/CodeBuddy CN.app",
      activeAccountId: demoAccounts[1].id,
      activeAccountName: displayName(demoAccounts[1]),
    } satisfies CodeBuddyCnIdeStatus;
    // 国际版同理：不 mock 会让演示切到「国际版」时落到 default → throw（被 AccountsPage 的
    // try/catch 吞掉），IDE 标记退化成「未接入」——而这正是本轮国际版 IDE 功能在演示页的展示面。
    case "get_codebuddy_ide_status": return {
      installed: true,
      running: true,
      loggedIn: true,
      dataDir: "/demo/codebuddy-ide",
      dbPath: "/demo/codebuddy-ide/state.vscdb",
      dbExists: true,
      appPath: "/demo/CodeBuddy IDE.app",
      activeAccountId: intlAccountA.id,
      activeAccountName: displayName(intlAccountA),
    } satisfies CodeBuddyCnIdeStatus;

    case "get_vscode_ext_status": return {
      installed: true,
      extensionInstalled: true,
      running: false,
      loggedIn: true,
      dataDir: "/demo/Code",
      dbPath: "/demo/Code/User/globalStorage/state.vscdb",
      dbExists: true,
      activeAccountId: demoAccounts[0].id,
      activeAccountName: displayName(demoAccounts[0]),
      detectedFrom: "state",
      statePath: "/demo/vscode_ext.json",
    } satisfies VscodeExtStatus;
    // JetBrains 端默认关闭，但设置页开启后演示页仍会探测；缺少该分支会落到 default → throw，
    // ToolStatusMarks 会静默退化成「未接入」。给一份与其它客户端同形的只读状态。
    case "get_jetbrains_status": return {
      installed: true,
      pluginInstalled: true,
      running: false,
      loggedIn: true,
      // 两个配置根：演示数据给全，避免演示页把「只有 JetBrains 根」当成唯一口径。
      configRoots: ["/demo/JetBrains", "/demo/Google"],
      targets: [
        {
          configDir: "PyCharm2026.2",
          pluginInstalled: true,
          running: false,
          loggedIn: true,
          secretPath: "/demo/JetBrains/PyCharm2026.2/options/secret-storage.xml",
        },
      ],
      activeAccountId: demoAccounts[2]?.id ?? null,
      activeAccountName: demoAccounts[2] ? displayName(demoAccounts[2]) : null,
      detectedFrom: "secret-storage",
      statePath: "/demo/jetbrains_ext.json",
    } satisfies JetbrainsStatus;


    case "switch_codebuddy_cli_account": {
      const target = demoAccounts.find((account) => account.id === args?.accountId);
      if (!target) throw new Error("账号不存在");
      setDemoActiveCliAccountId(target.id);
      return { ok: true, configured: true, synced: true, verified: true, activeIndex: demoAccounts.indexOf(target), activeAccountId: target.id, regionChanged: false, cliClosed: false, closedProcessCount: 0, message: "演示切换已完成" } satisfies CodeBuddyCliSwitchResult;
    }
    // 写操作：演示模式下直接改内存里的演示账号，保存后卡片/弹框立即可见。
    case "update_account_display": {
      const target = accounts.find((account) => account.id === args?.accountId);
      if (!target) throw new Error("账号不存在");
      const patch = (args?.patch ?? {}) as {
        note?: string | null;
        displayField?: DisplayField;
        mark?: AccountMark | null;
      };
      if ("note" in patch) {
        const trimmed = typeof patch.note === "string" ? patch.note.trim() : "";
        target.note = trimmed || null;
      }
      if (patch.displayField) target.displayField = patch.displayField;
      // R5-04：标记（对象 = 覆盖保存，null = 解除）；createdAt 与真实后端一样由服务端注入。
      if ("mark" in patch) {
        const incoming = patch.mark;
        target.mark = incoming
          ? { ...incoming, createdAt: Date.now() }
          : null;
      }
      return { ok: true, account: { ...target } };
    }
    default: return undefined;
  }
}