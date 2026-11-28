import { displayName } from "../account-display";
import type { CreditExpiry, CreditOfficialUsageModel, CreditStatistics } from "../types";
import { accountVariant, variantSupportsCheckin } from "../variant";
import type { DemoContext } from "./accounts";
import { atLocalTime, accounts, currentAccountIds, futureAt, hydratedAccounts, localDate } from "./accounts";

/**
 * 积分演示数据：积分包 fixture、到期明细与统计页聚合。
 *
 * 与账号列表严格同索引（usageSeeds / creditPackages / accounts），新增账号需同步追加。
 */

export const CREDIT_DEMO_COMMANDS = ["get_credit_expiry", "get_credit_statistics"] as const;

/** 演示数据里的模型名（只作为类型来源；新增 fixture 时同步这里）。 */
type ModelName = "deepseek-v4-flash" | "kimi-k3-1" | "deepseek-v4-pro" | "glm-5.2" | "hy3";

interface ModelSeed {
  model: ModelName;
  requestCount: number;
  credit: number;
}

interface AccountUsageSeed {
  requestCount: number;
  models: ModelSeed[];
}

// Counts and relative model roles follow anonymous aggregates from the sanitized local cache.
// No upstream request row or identifier is copied into this fixture.
// 与 `accounts` 严格同索引：`buildStatistics()` 按账号索引取这里的 seed。
const usageSeeds: AccountUsageSeed[] = [
  {
    requestCount: 2243,
    models: [
      { model: "deepseek-v4-flash", requestCount: 2133, credit: 1794.39 },
      { model: "kimi-k3-1", requestCount: 24, credit: 2497.16 },
      { model: "deepseek-v4-pro", requestCount: 23, credit: 3.63 },
      { model: "glm-5.2", requestCount: 1, credit: 33.63 },
      { model: "hy3", requestCount: 62, credit: 0 },
    ],
  },
  {
    requestCount: 679,
    models: [
      { model: "deepseek-v4-flash", requestCount: 659, credit: 1270.62 },
      { model: "hy3", requestCount: 20, credit: 0 },
    ],
  },
  {
    requestCount: 318,
    models: [
      { model: "deepseek-v4-flash", requestCount: 309, credit: 595.08 },
      { model: "hy3", requestCount: 9, credit: 0 },
    ],
  },
  // 国际版 A：用量集中在主力模型，国际版演示视图因此有可读的曲线与模型占比。
  {
    requestCount: 1420,
    models: [
      { model: "deepseek-v4-flash", requestCount: 1338, credit: 1120.5 },
      { model: "kimi-k3-1", requestCount: 68, credit: 642.7 },
      { model: "glm-5.2", requestCount: 14, credit: 33.63 },
    ],
  },
  // 国际版 B：轻量账号，用于演示同一档位内的用量差异。
  {
    requestCount: 268,
    models: [
      { model: "deepseek-v4-flash", requestCount: 240, credit: 482.9 },
      { model: "hy3", requestCount: 28, credit: 0 },
    ],
  },
  // 企业版 D：轻量用量，让统计页的企业版账号也有可读数据。
  {
    requestCount: 156,
    models: [
      { model: "deepseek-v4-flash", requestCount: 148, credit: 268.4 },
      { model: "hy3", requestCount: 8, credit: 0 },
    ],
  },
  // 资源包矩阵：用量与它的 18 个积分包无关（包只用于明细取证），给一份中等规模、
  // 与其它账号不成比例的稳定数字，避免账号消耗构成里出现并列。
  {
    requestCount: 412,
    models: [
      { model: "deepseek-v4-flash", requestCount: 380, credit: 742.35 },
      { model: "kimi-k3-1", requestCount: 22, credit: 318.6 },
      { model: "hy3", requestCount: 10, credit: 0 },
    ],
  },
];

/**
 * 演示积分包种子：
 * `[名称, 总量, 剩余, N 天后到期, 获取时间 = N 天前（可选）]`；与 `accounts` 严格同索引。
 *
 * `expireDays` 允许为负：负数表示**已过期**（`expired` 由 `expireDays < 0` 推导，
 * 到期时间落在过去），用于演示积分明细的「过期红」。
 * `startDaysAgo` 缺省沿用 `30 - expireDays`；过期包必须显式给值——否则获取时间会被推
 * 到 30 天以前，「近 30 天」筛选下整行看不到（明细是按 `startAt` 过滤的）。
 */
type CreditPackageSeed = readonly [
  packageName: string,
  total: number,
  remaining: number,
  expireDays: number,
  startDaysAgo?: number,
];

/**
 * 演示积分包按账号索引与 `accounts` 严格对应，覆盖卡片内容区与积分明细的多种形态：
 * A（1 条）/ B（2 条）为个人版，覆盖 1 行、2 行两种高度；C 为企业版单条「企业配额」
 * （企业接口只返回一条配额资源）；D 为企业版不限量，无积分包；末位「资源包矩阵」
 * 有 18 条，覆盖未使用灰 / 部分使用（正常色）/ 临期橙 / 过期红四态（R7-T09）。
 */
const creditPackages: readonly (readonly CreditPackageSeed[])[] = [
  // 账号 A：1 条 → 1 行，无「查看全部积分包」
  [
    ["CodeBuddy 新用户体验包", 800, 386.4, 5],
  ],
  // 账号 B：2 条 → 2 行，无「查看全部积分包」（链接在 resources.length > 2 时才出现）
  [
    ["CodeBuddy 个人版积分包", 1800, 905.5, 42],
    ["CodeBuddy 签到赠送积分", 240, 174.35, 15],
  ],
  // 账号 C（企业版有限配额）：单条「企业配额」，总量 2000 / 剩余 1500 / 30 天重置
  [
    ["企业配额", 2000, 1500, 30],
  ],
  // 国际版账号：合成码不会命中官方商品码映射，因此展示的就是这里自造的名字；
  // 沿用官方命名口径，覆盖「试用 / 赠送 / 购买」三类国际版语义。
  [
    ["试用版基础用量", 2000, 1340.8, 30],
    ["版本赠送用量", 600, 402.35, 75],
  ],
  [
    ["购买积分", 3000, 2180.6, 12],
    ["试用版基础用量", 500, 318.75, 60],
    ["平台奖励积分", 800, 512.4, 90],
  ],
  // 账号 D（企业版不限量）：无积分包，creditExpiry 返回 unlimited 形态
  [],
  // 「演示账号 · 资源包矩阵」：18 条 → 一屏 18 行 + 四态齐全（R7-T09 证据）。
  // 获取时间（第 5 列）铺在 2–28 天前，默认「近 30 天」即可见齐全；过期包的获取时间
  // 必须显式给值，否则按 `30 - expireDays` 推到 33 天前，会被日期范围筛掉。
  [
    // —— 未使用：remaining === total（明细里「已用」显示 —，条子灰） ——
    ["CodeBuddy 个人版积分包", 1800, 1800, 46, 6],
    ["CodeBuddy 签到赠送积分", 240, 240, 21, 12],
    ["CodeBuddy 新用户体验包", 800, 800, 33, 18],
    ["平台奖励积分", 600, 600, 58, 24],
    // —— 部分使用：0 < 已用 < 总量（正常色） ——
    ["CodeBuddy 个人版积分包", 1800, 905.5, 42, 2],
    ["CodeBuddy 签到赠送积分", 240, 174.35, 15, 9],
    ["活动赠送积分", 500, 320.6, 27, 4],
    ["团队共享积分", 1200, 640.25, 36, 14],
    ["CodeBuddy 个人版积分包", 2000, 1520.4, 61, 20],
    ["补充包 · 月度", 1000, 880.75, 74, 26],
    ["试用版基础用量", 400, 96.5, 19, 28],
    // —— 临期：7 天内到期且有剩余（橙） ——
    ["CodeBuddy 新用户体验包", 800, 386.4, 5, 25],
    ["签到赠送积分", 300, 289.2, 3, 27],
    // —— 已过期：到期时间在过去、仍有剩余（红） ——
    ["增量购买积分", 1500, 412.8, -3, 20],
    ["活动赠送积分", 600, 88.6, -10, 15],
    // —— 其余：把 18 行铺满，获取时间分散在近 30 天内 ——
    ["团队共享积分", 900, 733.15, 11, 8],
    ["补充包 · 周度", 250, 41.3, 8, 17],
    ["平台奖励积分", 700, 700, 52, 22],
  ],
];

function creditExpiry(accountId: string): CreditExpiry {
  const index = Math.max(0, accounts.findIndex((account) => account.id === accountId));
  const account = accounts[index] ?? accounts[0];
  const packages = creditPackages[index] ?? [];
  const resources = packages.map(([packageName, total, remaining, expireDays, startDaysAgo], packageIndex) => {
    // 过期由到期天数推导：负数 = 到期时间已在过去（演示「过期红」的唯一来源）。
    const expired = expireDays < 0;
    return {
      packageCode: `demo-package-${index + 1}-${packageIndex + 1}`,
      packageName,
      total,
      remaining,
      used: Number((total - remaining).toFixed(2)),
      status: 1,
      // `futureAt` 的 daysAhead 允许为负，等价于「N 天前」：过期包的到期时间必须落在过去。
      expireAt: futureAt(expireDays),
      startAt: atLocalTime(startDaysAgo ?? Math.max(0, 30 - expireDays), 0, 17),
      expired,
      // 已过期的包不再算「临期」：否则红色行会同时被橙色的「7 天内到期」指标统计一次。
      expiringSoon: !expired && expireDays <= 7,
    };
  });
  /**
   * 顶层指标与包列表自洽：
   * - `totalCapacity` = 全部包总量合计；
   * - `totalRemaining` = **未过期**包剩余合计（过期积分已不可用，不算「还剩」）；
   * - `expiredRemaining` = 已过期包剩余合计；后两者相加 = 包列表 remaining 总和。
   */
  const totalCapacity = resources.reduce((sum, resource) => sum + resource.total, 0);
  const totalRemaining = resources
    .filter((resource) => !resource.expired)
    .reduce((sum, resource) => sum + resource.remaining, 0);
  const expiredRemaining = resources
    .filter((resource) => resource.expired)
    .reduce((sum, resource) => sum + resource.remaining, 0);
  const expiringSoonRemaining = resources.filter((resource) => resource.expiringSoon).reduce((sum, resource) => sum + resource.remaining, 0);
  // 「最近到期」只看仍可用的包；已过期的到期时间在过去，混进来会显示成「三天前到期」。
  const usableExpireAt = resources.filter((resource) => !resource.expired).map((resource) => resource.expireAt);
  const updatedAt = Date.now() - (index + 1) * 4 * 60 * 1000;

  // 企业版（issue #167）：企业配额不在个人版接口里，卡片按企业形态渲染。
  // 无积分包 = 不限量（limitNum=-1）；有积分包 = 单条「企业配额」有限配额。
  if (account.isEnterprise && resources.length === 0) {
    return {
      ok: true,
      accountId: account.id,
      accountName: displayName(account),
      updatedAt,
      unlimited: true,
      totalCapacity: null,
      totalRemaining: null,
      expiringSoonRemaining: 0,
      expiredRemaining: 0,
      soonestExpireAt: null,
      expiringSoon: false,
      expired: false,
      resources: [],
    };
  }

  return {
    ok: true,
    accountId: account.id,
    accountName: displayName(account),
    updatedAt,
    // 企业版有限配额显式标记 unlimited=false；个人版不新增字段（保持原有 mock 形态）。
    ...(account.isEnterprise ? { unlimited: false } : {}),
    totalCapacity,
    totalRemaining: Number(totalRemaining.toFixed(2)),
    expiringSoonRemaining: Number(expiringSoonRemaining.toFixed(2)),
    expiredRemaining: Number(expiredRemaining.toFixed(2)),
    // 无可用包时退回到全部包，避免 `Math.min()` 空数组得到 Infinity。
    soonestExpireAt: Math.min(...(usableExpireAt.length > 0 ? usableExpireAt : resources.map((resource) => resource.expireAt))),
    expiringSoon: expiringSoonRemaining > 0,
    expired: expiredRemaining > 0,
    resources,
  };
}

function dailyWeight(accountIndex: number, dayIndex: number): number {
  const weekdayWave = [0.72, 1.08, 0.93, 1.22, 0.84, 1.16, 1.01][dayIndex % 7];
  const quiet = (dayIndex + accountIndex * 4) % 13 === 0 ? 0.16 : 1;
  return weekdayWave * quiet * (1 + accountIndex * 0.035);
}

function distributeModels(seed: AccountUsageSeed, accountIndex: number) {
  const weights = Array.from({ length: 30 }, (_, dayIndex) => dailyWeight(accountIndex, dayIndex));
  const weightTotal = weights.reduce((sum, weight) => sum + weight, 0);
  const countSeries = seed.models.map((model) => {
    const raw = weights.map((weight) => (model.requestCount * weight) / weightTotal);
    const values = raw.map(Math.floor);
    const remaining = model.requestCount - values.reduce((sum, value) => sum + value, 0);
    const byFraction = raw.map((value, index) => ({ index, fraction: value - Math.floor(value) })).sort((left, right) => right.fraction - left.fraction);
    for (let index = 0; index < remaining; index += 1) values[byFraction[index].index] += 1;
    return values;
  });
  const creditSeries = seed.models.map((model) => {
    const values = weights.map((weight) => Number(((model.credit * weight) / weightTotal).toFixed(2)));
    const drift = Number((model.credit - values.reduce((sum, value) => sum + value, 0)).toFixed(2));
    values[values.length - 1] = Number((values[values.length - 1] + drift).toFixed(2));
    return values;
  });
  return Array.from({ length: 30 }, (_, dayIndex) => {
    const models = seed.models.map((model, modelIndex) => ({
      model: model.model,
      requestCount: countSeries[modelIndex][dayIndex],
      credit: creditSeries[modelIndex][dayIndex],
    }));
    return {
      date: localDate(29 - dayIndex),
      usage: Number(models.reduce((sum, model) => sum + model.credit, 0).toFixed(2)),
      models,
    };
  });
}

function sumModels(rows: { models: CreditOfficialUsageModel[] }[]): CreditOfficialUsageModel[] {
  const totals = new Map<string, CreditOfficialUsageModel>();
  for (const row of rows) {
    for (const model of row.models) {
      const current = totals.get(model.model) ?? { model: model.model, requestCount: 0, credit: 0 };
      current.requestCount += model.requestCount;
      current.credit = Number((current.credit + model.credit).toFixed(2));
      totals.set(model.model, current);
    }
  }
  return [...totals.values()].sort((left, right) => right.credit - left.credit);
}

function visibleRequests(accountIndex: number) {
  const account = accounts[accountIndex];
  const seed = usageSeeds[accountIndex];
  const hours = [16, 15, 17, 14, 1, 0, 3];
  const flashCredits = [0.13, 0.04, 0.2, 1, 0.08, 3.99, 0.45, 8.5, 24.56];
  const kimiCredits = [86.4, 103.2, 112.8, 128.4, 74.6];
  const proCredits = [0.04, 0.13, 0.2, 0.45];
  const weightedModels = seed.models.flatMap((model) =>
    Array.from({ length: Math.max(1, Math.round((model.requestCount / seed.requestCount) * 100)) }, () => model.model),
  );

  return Array.from({ length: 100 }, (_, rowIndex) => {
    const daysAgo = Math.floor(rowIndex / 8);
    const hour = hours[(rowIndex + accountIndex * 2) % hours.length];
    const minute = (rowIndex * 7 + accountIndex * 11) % 60;
    const ts = new Date(atLocalTime(daysAgo, hour, minute));
    const model = rowIndex < seed.models.length
      ? seed.models[rowIndex].model
      : weightedModels[rowIndex % weightedModels.length];
    const credit = model === "hy3"
      ? 0
      : model === "kimi-k3-1"
        ? kimiCredits[(rowIndex + accountIndex) % kimiCredits.length]
        : model === "glm-5.2"
          ? 33.63
          : model === "deepseek-v4-pro"
            ? proCredits[(rowIndex + accountIndex) % proCredits.length]
            : flashCredits[(rowIndex + accountIndex * 3) % flashCredits.length];
    return {
      accountId: account.id,
      accountName: displayName(account),
      requestId: `demo-request-${String(accountIndex + 1).padStart(2, "0")}-${String(rowIndex + 1).padStart(4, "0")}`,
      credit,
      model,
      client: rowIndex % 50 === 0 ? "CodeBuddyIDE" : "CLI",
      requestTime: `${localDate(daysAgo)} ${String(ts.getHours()).padStart(2, "0")}:${String(ts.getMinutes()).padStart(2, "0")}:00`,
    };
  });
}

function buildStatistics(): CreditStatistics {
  const demoAccounts = hydratedAccounts();
  const accountDaily = usageSeeds.map((seed, index) => distributeModels(seed, index));
  const daily = accountDaily[0].map((_, dayIndex) => {
    const models = new Map<string, CreditOfficialUsageModel>();
    for (const rows of accountDaily) {
      for (const model of rows[dayIndex].models) {
        const current = models.get(model.model) ?? { model: model.model, requestCount: 0, credit: 0 };
        current.requestCount += model.requestCount;
        current.credit = Number((current.credit + model.credit).toFixed(2));
        models.set(model.model, current);
      }
    }
    const modelRows = [...models.values()];
    return { date: accountDaily[0][dayIndex].date, usage: Number(modelRows.reduce((sum, model) => sum + model.credit, 0).toFixed(2)), models: modelRows };
  });
  const sumRecent = (rows: { usage: number }[], count: number) => Number(rows.slice(-count).reduce((sum, row) => sum + row.usage, 0).toFixed(2));
  const monthPrefix = localDate(0).slice(0, 7);
  const sumMonth = (rows: { date: string; usage: number }[]) => Number(rows.filter((row) => row.date.startsWith(monthPrefix)).reduce((sum, row) => sum + row.usage, 0).toFixed(2));
  const generatedAt = Date.now() - 3 * 60 * 1000;
  const creditRows = accounts.map((account) => creditExpiry(account.id));
  const officialAccounts = demoAccounts.map((account, index) => ({
    accountId: account.id,
    accountName: displayName(account),
    ok: true,
    requestCount: usageSeeds[index].requestCount,
    detailTruncated: true,
    usageToday: accountDaily[index][accountDaily[index].length - 1]?.usage ?? 0,
    usage7Days: sumRecent(accountDaily[index], 7),
    usageThisMonth: sumMonth(accountDaily[index]),
    reportedTotal: usageSeeds[index].requestCount,
    fetchedCount: usageSeeds[index].requestCount,
    models: sumModels(accountDaily[index]),
    daily: accountDaily[index],
  }));
  const usageToday = daily[daily.length - 1]?.usage ?? 0;
  const usage7Days = sumRecent(daily, 7);
  const usageThisMonth = sumMonth(daily);
  const totalRemaining = creditRows.reduce((sum, credit) => sum + (credit.totalRemaining ?? 0), 0);
  const totalCapacity = creditRows.reduce((sum, credit) => sum + (credit.totalCapacity ?? 0), 0);

  return {
    generatedAt,
    retentionDays: 90,
    coverageStartAt: atLocalTime(29, 0, 0),
    // 签到计数与 `checkinLogs()` / `events` 的构造规则保持一致：
    // 支持签到的账号共 5 个（国内版 A/B/C/D + 资源包矩阵），其中 B 为 already、其余为 success。
    summary: { currentRemaining: Number(totalRemaining.toFixed(2)), currentCapacity: totalCapacity, usageToday, usage7Days, usageThisMonth, todayCheckedInAccounts: 5, todaySuccess: 4, todayAlready: 1, todayFailed: 0 },
    daily,
    accounts: demoAccounts.map((account, index) => {
      // 国际版没有签到接口：签到态一律留空，与 `variantSupportsCheckin` 一致。
      const checkinSupported = variantSupportsCheckin(accountVariant(account));
      const checkinResult = index === 1 ? "already" : "success";
      return {
        accountId: account.id,
        accountName: displayName(account),
        isCurrent: currentAccountIds.has(account.id),
        currentRemaining: creditRows[index].totalRemaining ?? null,
        totalCapacity: creditRows[index].totalCapacity ?? null,
        lastSnapshotAt: generatedAt - index * 120_000,
        usageToday: officialAccounts[index].usageToday ?? 0,
        usage7Days: officialAccounts[index].usage7Days ?? 0,
        usageThisMonth: officialAccounts[index].usageThisMonth ?? 0,
        checkedInToday: checkinSupported ? true : null,
        checkinStatusToday: checkinSupported ? checkinResult : null,
        lastCheckinAt: checkinSupported ? atLocalTime(0, 8, 6 + index * 9) : null,
        lastCheckinResult: checkinSupported ? checkinResult : null,
        daily: accountDaily[index],
      };
    }),
    // 签到事件按档位能力生成：国际版没有签到接口，不构造事件（国内版索引与取值不变）。
    events: demoAccounts.flatMap((account, index) => variantSupportsCheckin(accountVariant(account))
      ? [{
          kind: "checkin" as const,
          ts: atLocalTime(0, 8, 6 + index * 9),
          date: localDate(0),
          accountId: account.id,
          accountName: displayName(account),
          result: index === 1 ? "already" : "success",
        }]
      : []),
    officialUsage: {
      status: "complete",
      rangeStart: localDate(29),
      rangeEnd: localDate(0),
      collectedAt: generatedAt,
      summary: { usageToday, usage7Days, usageThisMonth },
      daily,
      accounts: officialAccounts,
      requests: accounts.flatMap((_, index) => visibleRequests(index)),
      models: sumModels(daily),
      detailLimitPerAccount: 100,
      errors: [],
    },
  };
}

export function creditsDemoResponse(
  command: string,
  _ctx: DemoContext,
  args?: Record<string, unknown>,
): unknown | undefined {
  switch (command) {
    case "get_credit_expiry": return creditExpiry(String(args?.accountId ?? ""));
    case "get_credit_statistics": return buildStatistics();
    default: return undefined;
  }
}