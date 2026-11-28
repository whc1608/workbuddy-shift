import { describe, expect, it } from "vitest";

import type { TokenStatsGroup, TokenStatsSource, TokenStatsTotals } from "@/lib/types";

import { mergeTokenSources } from "./merge-sources";

function totals(overrides: Partial<TokenStatsTotals> = {}): TokenStatsTotals {
  const input = overrides.input ?? 0;
  const output = overrides.output ?? 0;
  const cacheRead = overrides.cacheRead ?? 0;
  const cacheWrite = overrides.cacheWrite ?? 0;
  return {
    total: input + output + cacheWrite,
    input,
    output,
    cacheRead,
    cacheWrite,
    uncachedInput: Math.max(0, input - cacheRead),
    records: overrides.records ?? 0,
    cacheHitRate: input > 0 ? cacheRead / input : null,
  };
}

function group(key: string, overrides: Partial<TokenStatsGroup> = {}): TokenStatsGroup {
  return { key, ...totals(overrides) };
}

function source(overrides: Partial<TokenStatsSource> = {}): TokenStatsSource {
  return {
    source: "codebuddy-ide",
    summary: totals(),
    models: [],
    projects: [],
    sessions: [],
    daily: [],
    hours: [],
    filesScanned: 0,
    parseErrors: 0,
    ...overrides,
  };
}

describe("mergeTokenSources", () => {
  it("保留左侧来源身份，汇总数字与 records 相加", () => {
    const merged = mergeTokenSources(
      source({ summary: totals({ input: 100, output: 20, cacheRead: 40, records: 2 }) }),
      source({ source: "codebuddy-cli", summary: totals({ input: 300, output: 80, cacheWrite: 10, records: 5 }) }),
    );
    expect(merged.source).toBe("codebuddy-ide");
    expect(merged.summary).toEqual(totals({ input: 400, output: 100, cacheRead: 40, cacheWrite: 10, records: 7 }));
    // 命中率按合并后的 input 重算，不是两份比率的平均。
    expect(merged.summary.cacheHitRate).toBeCloseTo(40 / 400);
  });

  it("同 key 的日/模型/项目/会话行相加，不同 key 追加", () => {
    const merged = mergeTokenSources(
      source({
        daily: [group("2026-11-26", { input: 10, records: 1 })],
        models: [{ ...group("claude"), title: "Claude" }],
      }),
      source({
        daily: [
          group("2026-11-26", { input: 30, records: 2 }),
          group("2026-11-27", { input: 50, records: 3 }),
        ],
        models: [group("claude")],
      }),
    );
    expect(merged.daily.map((row) => [row.key, row.input])).toEqual([
      ["2026-11-26", 40],
      ["2026-11-27", 50],
    ]);
    expect(merged.models).toHaveLength(1);
    // 标题只在左侧提供时保留，不会被右侧的空值覆盖。
    expect(merged.models[0].title).toBe("Claude");
  });

  it("账号按 uid 合并，嵌套的 summary 与 daily 一并相加", () => {
    const merged = mergeTokenSources(
      source({
        accounts: [
          {
            uid: "u1",
            summary: totals({ input: 10, records: 1 }),
            models: [],
            projects: [],
            daily: [group("2026-11-26", { input: 10, records: 1 })],
            hours: [],
          },
        ],
      }),
      source({
        accounts: [
          {
            uid: "u1",
            summary: totals({ input: 20, records: 2 }),
            models: [],
            projects: [],
            daily: [group("2026-11-26", { input: 20, records: 2 })],
            hours: [],
          },
          {
            uid: "u2",
            summary: totals({ input: 5, records: 1 }),
            models: [],
            projects: [],
            daily: [],
            hours: [],
          },
        ],
      }),
    );
    expect(merged.accounts?.map((account) => account.uid)).toEqual(["u1", "u2"]);
    expect(merged.accounts?.[0].summary.input).toBe(30);
    expect(merged.accounts?.[0].daily[0].input).toBe(30);
  });

  it("覆盖区间取并集（最早起点 + 最晚终点），文件计数与解析错误相加", () => {
    const merged = mergeTokenSources(
      source({ coverageStartAt: 1_000, coverageEndAt: 5_000, filesScanned: 3, parseErrors: 1 }),
      source({ coverageStartAt: 2_000, coverageEndAt: 9_000, filesScanned: 4, parseErrors: 2 }),
    );
    expect(merged.coverageStartAt).toBe(1_000);
    expect(merged.coverageEndAt).toBe(9_000);
    expect(merged.filesScanned).toBe(7);
    expect(merged.parseErrors).toBe(3);
  });

  it("缺失覆盖区间时取另一侧；两侧都缺保持 null", () => {
    expect(mergeTokenSources(source({ coverageStartAt: 10 }), source()).coverageStartAt).toBe(10);
    expect(mergeTokenSources(source(), source({ coverageEndAt: 20 })).coverageEndAt).toBe(20);
    expect(mergeTokenSources(source(), source()).coverageEndAt).toBeNull();
  });

  it("逐请求明细按时间倒序合并", () => {
    const row = (timestamp: number) => ({
      timestamp,
      model: "claude",
      project: "p",
      sessionId: "s",
      input: 1,
      output: 1,
      cacheRead: 0,
      cacheWrite: 0,
      uncachedInput: 1,
      thinking: 0,
      total: 2,
    });
    const merged = mergeTokenSources(
      source({ requests: [row(100), row(300)] }),
      source({ requests: [row(200)] }),
    );
    expect(merged.requests?.map((item) => item.timestamp)).toEqual([300, 200, 100]);
  });
});
