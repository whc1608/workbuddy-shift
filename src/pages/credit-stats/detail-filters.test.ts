import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { CreditOfficialUsageRequest, CreditResource } from "@/lib/types";
import type { StatsRangeKey } from "@/lib/stats-range";

import {
  UNKNOWN_START_AT_NOTICE,
  extractRequestDateKey,
  filterRequestsByAccountAndRange,
  filterResourcesByRange,
  isKnownStartAt,
  isRequestInRange,
  sortResourcesByExpiry,
} from "./detail-filters";

const NOW = new Date(2026, 10, 26, 15, 30);

function localDateKeyOf(date: Date): string {
  const pad = (value: number) => String(value).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
}

function request(accountId: string, requestTime: string): CreditOfficialUsageRequest {
  return {
    accountId,
    accountName: accountId === "a" ? "账号 A" : "账号 B",
    requestId: `${accountId}-${requestTime}`,
    credit: 1,
    model: "model",
    client: "cli",
    requestTime,
  };
}

function resource(packageCode: string, startAt: number | null | undefined): CreditResource {
  return {
    packageCode,
    packageName: packageCode,
    total: 100,
    remaining: 50,
    used: 50,
    status: 0,
    startAt,
    expireAt: null,
    expired: false,
    expiringSoon: false,
  };
}

/** 本地时间 `2026-{month}-{day} 10:00` 的毫秒时间戳。 */
const at = (month: number, day: number, year = 2026) => new Date(year, month - 1, day, 10).getTime();

/**
 * `statsDateInRange` 的「近 N 天」下界按系统时钟计算，`now` 只是参考日；
 * 这里把系统时间一并固定，两个口径才指向同一天。
 */
beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(NOW);
});
afterEach(() => {
  vi.useRealTimers();
});

describe("请求日期范围（复用 statsDateInRange）", () => {
  const rows = [
    request("a", "2026-11-26 10:00:00"),
    request("b", "2026-11-26 09:00:00"),
    request("a", "2026-11-20 10:00:00"),
    request("a", "2026-11-19 10:00:00"),
    request("a", "2026-11-01 10:00:00"),
    request("a", "2026-10-28 10:00:00"),
    request("b", "2026-10-27 10:00:00"),
    request("a", "2026-08-29 10:00:00"),
    request("b", "2026-08-28 10:00:00"),
  ];

  it("覆盖 today / 7d / 30d / 90d / month / all 的首尾边界", () => {
    const count = (range: StatsRangeKey) => filterRequestsByAccountAndRange(rows, null, range, NOW).length;
    expect(count("all")).toBe(9);
    expect(count("today")).toBe(2);
    expect(count("7d")).toBe(3);
    expect(count("30d")).toBe(6);
    expect(count("90d")).toBe(8);
    expect(count("month")).toBe(5);
  });

  it("不传 now 时以当前时间为参考日", () => {
    expect(filterRequestsByAccountAndRange(rows, null, "today")).toHaveLength(2);
    expect(filterRequestsByAccountAndRange(rows, null, "90d")).toHaveLength(8);
  });

  it("跨月与跨年仍按自然日计算", () => {
    const crossMonth = new Date(2026, 11, 3, 8, 0);
    vi.setSystemTime(crossMonth);
    expect(isRequestInRange(request("a", "2026-11-27 00:00:00"), "7d", crossMonth)).toBe(true);
    expect(isRequestInRange(request("a", "2026-11-26 23:59:59"), "7d", crossMonth)).toBe(false);
    expect(isRequestInRange(request("a", "2026-11-04 00:00:00"), "30d", crossMonth)).toBe(true);
    expect(isRequestInRange(request("a", "2026-11-03 23:59:59"), "30d", crossMonth)).toBe(false);
    expect(isRequestInRange(request("a", "2026-11-30 12:00:00"), "month", crossMonth)).toBe(false);
    expect(isRequestInRange(request("a", "2026-12-01 00:00:00"), "month", crossMonth)).toBe(true);

    const crossYear = new Date(2027, 0, 3, 8, 0);
    vi.setSystemTime(crossYear);
    expect(isRequestInRange(request("a", "2026-12-28 00:00:00"), "7d", crossYear)).toBe(true);
    expect(isRequestInRange(request("a", "2026-12-27 23:59:59"), "7d", crossYear)).toBe(false);
    expect(isRequestInRange(request("a", "2026-12-05 00:00:00"), "30d", crossYear)).toBe(true);
    expect(isRequestInRange(request("a", "2026-12-04 23:59:59"), "30d", crossYear)).toBe(false);
    expect(isRequestInRange(request("a", "2027-01-01 00:00:00"), "month", crossYear)).toBe(true);
    expect(isRequestInRange(request("a", "2026-12-31 23:59:59"), "month", crossYear)).toBe(false);
  });

  it("具体范围与账号取交集，all 保留所有已加载明细", () => {
    expect(filterRequestsByAccountAndRange(rows, "a", "30d", NOW).map((row) => row.requestTime)).toEqual([
      "2026-11-26 10:00:00",
      "2026-11-20 10:00:00",
      "2026-11-19 10:00:00",
      "2026-11-01 10:00:00",
      "2026-10-28 10:00:00",
    ]);
    expect(filterRequestsByAccountAndRange(rows, "b", "month", NOW).map((row) => row.requestTime)).toEqual([
      "2026-11-26 09:00:00",
    ]);
    expect(filterRequestsByAccountAndRange(rows, "a", "all", NOW)).toHaveLength(6);
    expect(filterRequestsByAccountAndRange(rows, "missing", "all", NOW)).toEqual([]);
  });

  it("非法时间在 all 保留，在具体范围排除", () => {
    const invalid = [request("a", "invalid")];
    expect(filterRequestsByAccountAndRange(invalid, null, "all", NOW)).toHaveLength(1);
    for (const range of ["today", "7d", "30d", "90d", "month"] as const) {
      expect(filterRequestsByAccountAndRange(invalid, null, range, NOW)).toEqual([]);
    }
  });

  it("非法参考日下具体范围一律不命中", () => {
    const invalidNow = new Date(Number.NaN);
    expect(filterRequestsByAccountAndRange(rows, null, "today", invalidNow)).toEqual([]);
    expect(filterRequestsByAccountAndRange(rows, null, "all", invalidNow)).toHaveLength(9);
  });

  it("不修改输入数组，返回新数组", () => {
    const original = rows.map((row) => ({ ...row }));
    const result = filterRequestsByAccountAndRange(rows, "a", "7d", NOW);
    expect(rows).toEqual(original);
    expect(result).not.toBe(rows);
    expect(result[0]).toBe(rows[0]);
  });
});

describe("请求时间解析", () => {
  it("识别本地日期与 YYYY-MM-DD HH:mm:ss", () => {
    expect(extractRequestDateKey("2026-11-26 00:17:44")).toBe("2026-11-26");
    expect(extractRequestDateKey(" 2026-11-26T23:59:59.123 ")).toBe("2026-11-26");
    expect(extractRequestDateKey("2026-11-26 10:20")).toBe("2026-11-26");
    expect(extractRequestDateKey("2026-11-26")).toBe("2026-11-26");
  });

  it("RFC3339 与带时区字符串换算成本地日期", () => {
    const instant = Date.parse("2026-11-26T22:30:00.000Z");
    const expected = localDateKeyOf(new Date(instant));
    expect(extractRequestDateKey("2026-11-26T22:30:00Z")).toBe(expected);
    expect(extractRequestDateKey("2026-11-27T06:30:00+08:00")).toBe(expected);
    expect(extractRequestDateKey("2026-11-27T06:30:00+0800")).toBe(expected);
    expect(extractRequestDateKey("2026-11-26 22:30:00.123456Z")).toBe(expected);
    expect(extractRequestDateKey("2026-11-26T17:30:00-05:00")).toBe(expected);
  });

  it("带时区的请求按本地日期参与范围过滤", () => {
    const lateYesterday = new Date(2026, 10, 25, 23, 30).toISOString();
    const earlyToday = new Date(2026, 10, 26, 0, 30).toISOString();
    const zoned = [request("a", lateYesterday), request("a", earlyToday)];
    expect(filterRequestsByAccountAndRange(zoned, null, "today", NOW).map((row) => row.requestTime)).toEqual([
      earlyToday,
    ]);
  });

  it("识别秒与毫秒时间戳", () => {
    const instant = new Date(2026, 10, 26, 12, 0).getTime();
    expect(extractRequestDateKey(String(instant))).toBe("2026-11-26");
    expect(extractRequestDateKey(String(Math.floor(instant / 1000)))).toBe("2026-11-26");
    const stamped = [
      request("a", String(Math.floor(new Date(2026, 10, 26, 12, 0).getTime() / 1000))),
      request("a", String(new Date(2026, 10, 25, 12, 0).getTime())),
    ];
    expect(filterRequestsByAccountAndRange(stamped, null, "today", NOW)).toHaveLength(1);
  });

  it("非法日期、时间和格式返回 null", () => {
    const invalid = ["", "not-a-date", "2026/11/26", "2026-02-30", "2026-11-26 24:00:00", "1764134400000.5"];
    expect(invalid.map(extractRequestDateKey)).toEqual(invalid.map(() => null));
  });
});

describe("资源包 startAt 过滤", () => {
  const rows = [
    resource("today", at(11, 26)),
    resource("seven-days", at(11, 20)),
    resource("outside-seven", at(11, 19)),
    resource("month-start", at(11, 1)),
    resource("thirty-days", at(10, 28)),
    resource("outside-thirty", at(10, 27)),
    resource("ninety-days", at(8, 29)),
    resource("outside-ninety", at(8, 28)),
    resource("future", at(11, 27)),
    resource("unknown-null", null),
    resource("unknown-undefined", undefined),
    resource("unknown-invalid", Number.NaN),
  ];
  const codes = (range: StatsRangeKey) => filterResourcesByRange(rows, range, NOW).map((row) => row.packageCode);
  const unknown = ["unknown-null", "unknown-undefined", "unknown-invalid"];

  it("all 保留全部资源且保持顺序", () => {
    expect(codes("all")).toEqual(rows.map((row) => row.packageCode));
  });

  it("已知时间按 today / 7d / 30d / 90d / month 过滤", () => {
    expect(codes("today")).toEqual(["today", ...unknown]);
    expect(codes("7d")).toEqual(["today", "seven-days", ...unknown]);
    expect(codes("30d")).toEqual(["today", "seven-days", "outside-seven", "month-start", "thirty-days", ...unknown]);
    expect(codes("90d")).toEqual([
      "today",
      "seven-days",
      "outside-seven",
      "month-start",
      "thirty-days",
      "outside-thirty",
      "ninety-days",
      ...unknown,
    ]);
    // 「本月」沿用 statsDateInRange 的口径：只按自然月匹配，不设上界。
    expect(codes("month")).toEqual(["today", "seven-days", "outside-seven", "month-start", "future", ...unknown]);
  });

  it("跨月与跨年按自然日回溯", () => {
    const crossYear = new Date(2027, 0, 3, 8, 0);
    vi.setSystemTime(crossYear);
    const cross = [
      resource("last-year", at(12, 28, 2026)),
      resource("too-early", at(12, 27, 2026)),
      resource("this-month", at(1, 1, 2027)),
    ];
    expect(filterResourcesByRange(cross, "7d", crossYear).map((row) => row.packageCode)).toEqual([
      "last-year",
      "this-month",
    ]);
    expect(filterResourcesByRange(cross, "month", crossYear).map((row) => row.packageCode)).toEqual([
      "this-month",
    ]);
  });

  it("未知 startAt 在具体范围保留并有明确 UI 提示", () => {
    for (const range of ["today", "7d", "30d", "90d", "month"] as const) {
      expect(codes(range)).toEqual(expect.arrayContaining(unknown));
    }
    expect(UNKNOWN_START_AT_NOTICE).toContain("保留展示");
    expect(UNKNOWN_START_AT_NOTICE).toContain("不参与日期范围筛选");
  });

  it("isKnownStartAt 只接受可用的正数时间戳", () => {
    for (const value of [null, undefined, Number.NaN, Number.POSITIVE_INFINITY, 0, -1, "1700000000000"]) {
      expect(isKnownStartAt(value as unknown as number)).toBe(false);
    }
    expect(isKnownStartAt(at(11, 26))).toBe(true);
  });

  it("不修改输入数组，返回新数组", () => {
    const original = rows.map((row) => ({ ...row }));
    const result = filterResourcesByRange(rows, "30d", NOW);
    expect(rows).toEqual(original);
    expect(result).not.toBe(rows);
    expect(result[0]).toBe(rows[0]);
  });
});

describe("资源包排序（按到期时间升序）", () => {
  function expiring(packageCode: string, expireAt: number | null, expired = false): CreditResource {
    return { ...resource(packageCode, at(11, 20)), expireAt, expired };
  }

  it("最早到期的排最前，长期包（无到期）落末尾", () => {
    const rows = [
      expiring("长期包", null),
      expiring("12 月底到期", at(12, 31)),
      expiring("11/30 到期", at(11, 30)),
    ];
    expect(sortResourcesByExpiry(rows).map((row) => row.packageCode)).toEqual([
      "11/30 到期",
      "12 月底到期",
      "长期包",
    ]);
  });

  it("已过期的包（到期时间最早）仍排最前，由颜色区分风险", () => {
    const rows = [expiring("未到期", at(12, 31)), expiring("已过期", at(11, 26), true)];
    expect(sortResourcesByExpiry(rows).map((row) => row.packageCode)).toEqual(["已过期", "未到期"]);
  });

  it("到期时间相同保持后端原顺序（稳定排序）", () => {
    const rows = [expiring("B", at(12, 31)), expiring("A", at(12, 31)), expiring("C", at(12, 31))];
    expect(sortResourcesByExpiry(rows).map((row) => row.packageCode)).toEqual(["B", "A", "C"]);
  });

  it("无效到期值按长期处理，且不修改输入数组", () => {
    const rows = [
      expiring("无效值", Number.NaN),
      expiring("有效值", at(11, 30)),
      expiring("缺失", null),
    ];
    const original = rows.map((row) => ({ ...row }));
    const result = sortResourcesByExpiry(rows);
    expect(result.map((row) => row.packageCode)).toEqual(["有效值", "无效值", "缺失"]);
    expect(rows).toEqual(original);
    expect(result).not.toBe(rows);
  });
});
