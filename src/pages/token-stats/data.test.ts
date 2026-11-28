import { describe, expect, it } from "vitest";

import { dateDaysAgo, dateKey } from "@/lib/format";
import type { TokenStatsGroup, TokenStatsSource } from "@/lib/types";

import { fillRangePoints, overviewTotals, rangePoints, rangeTotals, staleLogDays } from "./data";

const today = dateKey(new Date());
const daysAgo = (days: number) => dateDaysAgo(days);

function group(key: string, overrides: Partial<TokenStatsGroup> = {}): TokenStatsGroup {
  return {
    key,
    total: 0,
    input: 0,
    output: 0,
    cacheRead: 0,
    cacheWrite: 0,
    uncachedInput: 0,
    records: 0,
    cacheHitRate: null,
    ...overrides,
  };
}

const zero = (key: string) => group(key);

describe("token-stats rangePoints", () => {
  const daily = [
    group(daysAgo(1)),
    group(daysAgo(40)),
    group(today),
    group(daysAgo(6)),
    group(daysAgo(29)),
    group(daysAgo(30)),
  ];

  it("today / 7d / 30d 按日期窗口过滤并按日期升序", () => {
    expect(rangePoints(daily, "today").map((point) => point.key)).toEqual([today]);
    expect(rangePoints(daily, "7d").map((point) => point.key)).toEqual([
      daysAgo(6),
      daysAgo(1),
      today,
    ]);
    expect(rangePoints(daily, "30d").map((point) => point.key)).toEqual([
      daysAgo(29),
      daysAgo(6),
      daysAgo(1),
      today,
    ]);
  });

  it("month 只保留当月日期", () => {
    const points = rangePoints(daily, "month");
    expect(points.every((point) => point.key.startsWith(`${today.slice(0, 7)}-`))).toBe(true);
  });

  it("空源返回空数组", () => {
    expect(rangePoints([], "30d")).toEqual([]);
  });
});

describe("token-stats fillRangePoints", () => {
  it("空输入直接返回空数组（不生成整段空日期轴）", () => {
    expect(fillRangePoints([], "30d")).toEqual([]);
  });

  it("today 区间只有一天，且保留已有点对象", () => {
    const point = group(today, { total: 9 });
    const filled = fillRangePoints([point], "today");
    expect(filled).toHaveLength(1);
    expect(filled[0]).toBe(point);
  });

  it("7d / 30d 补齐缺失日期为零值，并保留有值点", () => {
    const existing = group(daysAgo(3), { total: 5, input: 5 });
    const filled = fillRangePoints([group(today), existing], "7d");
    expect(filled).toHaveLength(7);
    expect(filled[0].key).toBe(daysAgo(6));
    const kept = filled.find((point) => point.key === daysAgo(3));
    expect(kept).toBe(existing);
    const missing = filled.find((point) => point.key === daysAgo(1));
    expect(missing).toEqual(zero(daysAgo(1)));
    expect(fillRangePoints([existing], "30d")).toHaveLength(30);
  });
});

describe("token-stats rangeTotals", () => {
  it("汇总各 token 字段，total 不重复计 cacheRead", () => {
    const totals = rangeTotals([
      group("a", { input: 10, output: 5, cacheWrite: 2, cacheRead: 4, records: 2 }),
      group("b", { input: 0, output: 1, cacheWrite: 0, cacheRead: 0, records: 1 }),
    ]);
    expect(totals.input).toBe(10);
    expect(totals.output).toBe(6);
    expect(totals.cacheWrite).toBe(2);
    expect(totals.cacheRead).toBe(4);
    expect(totals.records).toBe(3);
    // total = input + output + cacheWrite（input 已包含 cacheRead）
    expect(totals.total).toBe(18);
    expect(totals.cacheHitRate).toBeCloseTo(0.4);
  });

  it("空集合返回全零且命中率为 null", () => {
    expect(rangeTotals([])).toEqual({
      total: 0,
      input: 0,
      output: 0,
      cacheRead: 0,
      cacheWrite: 0,
      uncachedInput: 0,
      records: 0,
      cacheHitRate: null,
    });
  });
});

describe("token-stats overviewTotals", () => {
  const totals = rangeTotals([group(today, { input: 3, output: 1, records: 1 })]);
  const source = {
    source: "workbuddy",
    summary: rangeTotals([group(today, { input: 99 })]),
    models: [],
    projects: [],
    sessions: [],
    daily: [group(today, { input: 3, output: 1, records: 1 })],
    hours: [],
    filesScanned: 0,
    parseErrors: 0,
  } as TokenStatsSource;

  it("all 直接采用后端汇总（与积分页「全部 = 本地账本」语义对齐）", () => {
    expect(overviewTotals(source, "all")).toBe(source.summary);
  });

  it("按区间汇总时使用对应日期窗口", () => {
    expect(overviewTotals(source, "today")).toEqual(totals);
    expect(overviewTotals(source, "7d")).toEqual(totals);
  });
});

describe("staleLogDays", () => {
  // 用本地时间构造，和函数内部「按本地整日比较」一致（避免时区把断言变成偶发失败）。
  const now = new Date(2026, 10, 28, 10, 0, 0).getTime();

  it("今天有日志时不提示", () => {
    expect(staleLogDays(new Date(2026, 10, 28, 8, 30).getTime(), now)).toBeNull();
  });

  it("按整日差算：昨天是 1 天，前天的 23:22 是 2 天", () => {
    expect(staleLogDays(new Date(2026, 10, 27, 23, 30).getTime(), now)).toBe(1);
    expect(staleLogDays(new Date(2026, 10, 26, 23, 22).getTime(), now)).toBe(2);
  });

  it("没有覆盖信息、非法值、将来时间都不提示", () => {
    expect(staleLogDays(null, now)).toBeNull();
    expect(staleLogDays(undefined, now)).toBeNull();
    expect(staleLogDays(Number.NaN, now)).toBeNull();
    expect(staleLogDays(now + 86_400_000, now)).toBeNull();
  });
});
