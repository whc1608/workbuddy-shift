import { describe, expect, it } from "vitest";

import { dateDaysAgo, dateKey } from "@/lib/format";
import type { TokenStatsSource } from "@/lib/types";

import {
  buildTokenStatsCsv,
  buildTokenStatsXlsx,
  rangeFileLabel,
  sliceTokenStatsSource,
  tokenStatsFileName,
  tokenStatsSheets,
  tokenStatsSections,
  tokenStatsXlsxFileName,
} from "./export";


/** 解析 store 模式 ZIP 的条目（与 xlsx.ts 的写实现对应），返回 name → UTF-8 文本。 */
function readStoredZip(bytes: Uint8Array): Map<string, string> {
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const decoder = new TextDecoder();
  const entries = new Map<string, string>();
  let cursor = 0;
  while (cursor + 30 <= bytes.length) {
    if (view.getUint32(cursor, true) !== 0x04034b50) break;
    const method = view.getUint16(cursor + 8, true);
    const size = view.getUint32(cursor + 18, true);
    const nameLength = view.getUint16(cursor + 26, true);
    const extraLength = view.getUint16(cursor + 28, true);
    const nameStart = cursor + 30;
    const name = decoder.decode(bytes.subarray(nameStart, nameStart + nameLength));
    const dataStart = nameStart + nameLength + extraLength;
    expect(method).toBe(0);
    entries.set(name, decoder.decode(bytes.subarray(dataStart, dataStart + size)));
    cursor = dataStart + size;
  }
  return entries;
}

function totals(total: number) {
  return {
    total,
    input: total,
    output: 0,
    cacheRead: 0,
    cacheWrite: 0,
    uncachedInput: total,
    records: 1,
    cacheHitRate: null,
  };
}

function source(overrides: Partial<TokenStatsSource> = {}): TokenStatsSource {
  return {
    source: "workbuddy",
    summary: totals(1_064_789_123),
    models: [{ key: "model-a", ...totals(30) }],
    projects: [{ key: "project-a", ...totals(20) }],
    sessions: [
      { key: "project-a · session-1", title: "会话标题", project: "project-a", sessionId: "session-1", uid: "uid-123456789", ...totals(10) },
    ],
    daily: [{ key: "2026-11-19", ...totals(5) }],
    hours: [{ key: "0-9", ...totals(4) }],
    filesScanned: 3,
    parseErrors: 1,
    coverageStartAt: 1_700_000_000_000,
    coverageEndAt: 1_700_100_000_000,
    ...overrides,
  };
}

/** 一条 CLI 逐请求明细（仅范围切片的测试需要，字段与后端下发一致）。 */
function requestRow(timestamp: number) {
  return {
    timestamp,
    model: "model-a",
    project: "project-a",
    sessionId: "session-1",
    title: null,
    uid: "",
    input: 10,
    output: 2,
    cacheRead: 0,
    cacheWrite: 0,
    uncachedInput: 10,
    thinking: 0,
    total: 12,
  };
}

describe("Token 导出（CSV / xlsx）", () => {
  it("每段表头列数 == 数据列数（不再有空的「名称」列）", () => {
    const sections = tokenStatsSections(source(), "all");
    expect(sections.map((section) => section.name)).toEqual([
      "汇总",
      "按日",
      "按模型",
      "按项目",
      "按会话",
      "按小时",
      "请求明细",
    ]);
    for (const section of sections) {
      for (const row of section.rows) {
        expect(row.length, `${section.name} 的行列数`).toBe(section.header.length);
      }
    }
    const models = sections.find((section) => section.name === "按模型");
    expect(models?.header).toEqual(["模型", "总 Token", "输入", "输出", "缓存读", "缓存写", "请求数"]);
    expect(models?.header).not.toContain("名称");
    // 按会话段必须有标题列；键列只放「项目 · 会话」。
    const sessions = sections.find((section) => section.name === "按会话");
    expect(sessions?.header).toContain("标题");
    expect(sessions?.header[0]).toBe("项目 · 会话");
  });

  it("CSV：每段独立表头、段标题带 # 前缀、数字不加引号", () => {
    const csv = buildTokenStatsCsv(source(), "all");
    const lines = csv.replace(/^\uFEFF/, "").split("\r\n").filter((line) => line.length > 0);

    expect(lines).toContain("# 按模型");
    expect(lines).toContain("# 按会话");
    expect(lines).toContain("模型,总 Token,输入,输出,缓存读,缓存写,请求数");
    // 大数字保持纯数字（可求和），不加引号、不写 ="…"。
    expect(csv).toContain("1064789123");
    expect(csv).not.toContain('"1064789123"');
    expect(csv).not.toContain('="1064789123"');
    // 会话段能看到标题。
    expect(csv).toContain("project-a · session-1,会话标题,账号 …456789,10");
  });

  it("CSV 与 xlsx 的数值逐段一致（抽样三段）", () => {
    const data = source();
    const sheets = tokenStatsSheets(data, "all");
    const csv = buildTokenStatsCsv(data, "all");
    for (const name of ["按日", "按模型", "按项目"]) {
      const sheet = sheets.find((item) => item.name === name);
      expect(sheet).toBeDefined();
      for (const row of sheet?.rows ?? []) {
        const line = row.map((cell) => String(cell)).join(",");
        expect(csv).toContain(line);
      }
    }
  });

  it("xlsx：7 个 sheet、首行冻结、数字是数值类型", () => {
    const bytes = buildTokenStatsXlsx(source(), "all");
    const entries = readStoredZip(bytes);
    expect(entries.get("xl/workbook.xml")).toContain('name="请求明细"');
    const summary = entries.get("xl/worksheets/sheet1.xml") ?? "";
    expect(summary).toContain('state="frozen"');
    // 总 Token 的值写进 <v>（数值），可求和。
    expect(summary).toContain("<v>1064789123</v>");
    // 非 CLI 来源的请求明细段只有一行说明，不是空表头。
    const detail = entries.get("xl/worksheets/sheet7.xml") ?? "";
    expect(detail).toContain("不提供逐条请求明细");
  });

  it("CLI 来源导出逐请求明细，含账号列与本地时间", () => {
    const data = source({
      source: "codebuddy-cli",
      requests: [
        {
          timestamp: 1_700_000_000_000,
          model: "model-a",
          project: "project-a",
          sessionId: "session-1",
          title: null,
          uid: "",
          input: 10,
          output: 2,
          cacheRead: 0,
          cacheWrite: 0,
          uncachedInput: 10,
          thinking: 0,
          total: 12,
        },
      ],
    });
    const detail = tokenStatsSections(data, "all").find((section) => section.name === "请求明细");
    expect(detail?.header[0]).toBe("时间");
    expect(detail?.header).toContain("账号");
    expect(detail?.rows[0]?.[5]).toBe("未归属");
    // 本地时间字符串（不是 UTC ISO），避免时区误读。
    expect(String(detail?.rows[0]?.[0])).toMatch(/^\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}$/);
  });

  it("文件名带账号标签与范围标签，且非法字符被清洗", () => {
    const plain = tokenStatsFileName("workbuddy", "30d");
    expect(plain).toMatch(/^token-stats-workbuddy-近30天-\d{8}-\d{4}\.csv$/);
    // `@` 与中文在 Windows 文件名里合法，保留；`/`、空白等非法字符被替换成 `-`。
    const scoped = tokenStatsFileName("workbuddy", "today", "a@example.com/国内");
    expect(scoped).toMatch(/^token-stats-workbuddy-a@example\.com-国内-今天-\d{8}-\d{4}\.csv$/);
    expect(tokenStatsXlsxFileName("workbuddy", "month", "账号 A")).toMatch(
      /^token-stats-workbuddy-账号-A-本月-\d{8}-\d{4}\.xlsx$/,
    );
  });

  it("范围标签只有一份文案来源：`近 30 天` 在文件名里压成 `近30天`", () => {
    expect(rangeFileLabel("7d")).toBe("近7天");
    expect(rangeFileLabel("all")).toBe("全部");
  });

  it("导出切片跟随统计范围：汇总/按日/按模型/请求明细只保留范围内数据", () => {
    const today = dateKey(new Date());
    const data = source({
      summary: totals(100),
      daily: [
        { key: dateDaysAgo(40), ...totals(60) },
        { key: dateDaysAgo(2), ...totals(30) },
        { key: today, ...totals(10) },
      ],
      models: [{ key: "model-a", ...totals(100) }],
      dailyByModel: {
        "model-a": [
          { key: dateDaysAgo(40), ...totals(60) },
          { key: dateDaysAgo(2), ...totals(30) },
          { key: today, ...totals(10) },
        ],
      },
      requests: [
        requestRow(Date.parse(`${dateDaysAgo(40)}T10:00:00`)),
        requestRow(Date.parse(`${dateDaysAgo(2)}T10:00:00`)),
        requestRow(Date.parse(`${today}T10:00:00`)),
      ],
      source: "codebuddy-cli",
    });

    const week = sliceTokenStatsSource(data, "7d");
    // 按日：40 天前那一天被切掉。
    expect(week.daily.map((point) => point.key)).toEqual([dateDaysAgo(2), today]);
    // 汇总：与首屏四卡同一份 `overviewTotals`（=范围内逐日合计）。
    expect(week.summary.total).toBe(40);
    // 按模型：有 dailyByModel 时按范围重算。
    expect(week.models.map((group) => group.total)).toEqual([40]);
    // 请求明细：同样按 timestamp 切片。
    expect(week.requests).toHaveLength(2);
    // 按项目 / 按会话 / 按小时：全量聚合不动（页面也标注「不受范围影响」）。
    expect(week.projects).toEqual(data.projects);
    expect(week.sessions).toEqual(data.sessions);
    expect(week.hours).toEqual(data.hours);
    // 「全部」= 不切片。
    const all = sliceTokenStatsSource(data, "all");
    expect(all.daily).toHaveLength(3);
    expect(all.summary.total).toBe(100);
    expect(all.requests).toHaveLength(3);
  });

  it("汇总段首行写明统计范围，且按日段落行数与切片一致", () => {
    const today = dateKey(new Date());
    const data = source({
      summary: totals(30),
      daily: [
        { key: dateDaysAgo(10), ...totals(30) },
        { key: today, ...totals(5) },
      ],
    });
    const sliced = sliceTokenStatsSource(data, "today");
    const sections = tokenStatsSections(sliced, "today");
    const summary = sections.find((section) => section.name === "汇总");
    expect(summary?.rows[0]).toEqual(["统计范围", "今天"]);
    const daily = sections.find((section) => section.name === "按日");
    expect(daily?.rows.map((row) => row[0])).toEqual([today]);
    // CSV 里也能读到范围，避免文件离开应用后失去口径。
    expect(buildTokenStatsCsv(sliced, "today")).toContain("统计范围,今天");
  });
});
