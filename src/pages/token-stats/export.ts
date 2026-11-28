import { dateKey } from "@/lib/format";
import { statsDateInRange, statsRangeLabel, type StatsRangeKey } from "@/lib/stats-range";
import type { TokenStatsGroup, TokenStatsSource } from "@/lib/types";
import { buildCsv, csvFileName, type CsvSection } from "@/lib/csv";
import { buildXlsx, type XlsxCell, type XlsxSheet } from "@/lib/xlsx";

import { formatRequestTime, modelTotalsInRange, overviewTotals, rangePoints } from "./data";

/**
 * Token 统计导出（清单第 17 条）。
 *
 * 五类数据**不再共用一套表头**：旧实现的 `键 | 名称 | …` 让模型/项目/日/小时的「名称」
 * 永远为空，WPS 里看起来像整体右移。现在每个区块只带自己的列：
 *
 * | 区块 | 表头 |
 * | --- | --- |
 * | 汇总 | 指标 · 数值 |
 * | 按日 | 日期 · 总量… |
 * | 按模型 | 模型 · 总量… |
 * | 按项目 | 项目 · 总量… |
 * | 按会话 | 项目 · 会话 · 标题 · 账号 · 总量… |
 * | 按小时 | 小时 · 总量… |
 * | 请求明细 | 时间 · 模型 · 项目 · 会话 · 标题 · 账号 · 输入 … |
 *
 * CSV 与 xlsx 共用同一份中间结构（`ExportSection`），保证两种格式逐段数字一致。
 *
 * **范围口径（R7-T04）**：导出跟随页面顶部唯一的「统计范围」，不再固定导全量。
 * 切片规则见 `sliceTokenStatsSource()`：能按日期切回来的维度（汇总 / 按日 / 按模型 /
 * 请求明细）与页面同一份函数算；后端只给全量聚合的维度（按项目 / 按会话 / 按小时 /
 * 按账号）保持全量——页面本身也标注「不受统计范围影响」，硬切只会得到假数字。
 */

/**
 * 数值列（CSV 里保持纯数字，可求和；xlsx 里是数值类型）。
 *
 * **规则（R5-18）：导出列必须与图表系列一一对得上。**
 * 图表 `./trend-chart.tsx` 的 `TOKEN_SERIES`（缓存读取 / 新增输入 / 输出 / 缓存写入）
 * 与这里的分类是同一套口径：图上有哪个分类，导出就有哪一列，绝不出现「界面有、导出没有」
 * 或「导出有、界面没有」。注意映射不是字面同名——`输入` 列 = `新增输入 + 缓存读取`
 * （`data.ts` 里 `total = input + output + cacheWrite`，`input` 已含 cacheRead），
 * `总 Token` 与 `请求数` 是合计/计数列，不对应单个系列。
 * 因此任一侧增删分类（例如将来出现 cacheWrite > 0 的新系列）都必须同时改另一侧。
 *
 * 「缓存写」列在 WorkBuddy（DeepSeek 系）数据里恒为 0，属上游不填，是**预期值**，
 * 不是导出缺陷，因此保留该列（隐藏系列 / 删列都已明确否决）。
 */
const TOTALS_HEADERS = ["总 Token", "输入", "输出", "缓存读", "缓存写", "请求数"];

interface ExportSection {
  /** 区块名 = xlsx 的 sheet 名；CSV 用它做段标题（加 `# ` 前缀）。 */
  name: string;
  header: string[];
  rows: XlsxCell[][];
}

function totalsCells(group: TokenStatsGroup): XlsxCell[] {
  return [
    group.total,
    group.input,
    group.output,
    group.cacheRead,
    group.cacheWrite,
    group.records,
  ];
}

/** 逐日/逐模型/逐项目/逐小时共用的「键 + 数值」区块。 */
function keyedSection(name: string, keyLabel: string, groups: TokenStatsGroup[]): ExportSection {
  return {
    name,
    header: [keyLabel, ...TOTALS_HEADERS],
    rows: groups.map((group) => [group.key, ...totalsCells(group)]),
  };
}

/** 账号列：空 uid 是「未归属」，其余显示 uid 后 6 位（与页面行标签一致）。 */
function accountCell(uid?: string): string {
  if (uid === undefined) return "";
  if (!uid) return "未归属";
  return `账号 …${uid.slice(-6)}`;
}

function summarySection(source: TokenStatsSource, range: StatsRangeKey): ExportSection {
  const summary = source.summary;
  const rows: XlsxCell[][] = [
    // 文件里写明这份数字属于哪个范围：文件名有标签，但表内也要能自证。
    ["统计范围", statsRangeLabel(range)],
    ["总 Token", summary.total],
    ["输入", summary.input],
    ["输出", summary.output],
    ["缓存读", summary.cacheRead],
    ["缓存写", summary.cacheWrite],
    ["未命中缓存输入", summary.uncachedInput],
    ["请求数", summary.records],
    [
      "缓存命中率",
      summary.cacheHitRate === null ? "" : `${(summary.cacheHitRate * 100).toFixed(1)}%`,
    ],
    ["会话文件数", source.filesScanned],
    ["解析错误数", source.parseErrors],
  ];
  if (source.coverageStartAt) {
    rows.push(["覆盖起算", formatRequestTime(source.coverageStartAt)]);
  }
  if (source.coverageEndAt) {
    rows.push(["覆盖终点", formatRequestTime(source.coverageEndAt)]);
  }
  return { name: "汇总", header: ["指标", "数值"], rows };
}

function sessionSection(source: TokenStatsSource): ExportSection {
  return {
    name: "按会话",
    header: ["项目 · 会话", "标题", "账号", ...TOTALS_HEADERS],
    rows: source.sessions.map((group) => [
      group.key,
      group.title ?? "",
      accountCell(group.uid),
      ...totalsCells(group),
    ]),
  };
}

function requestSection(source: TokenStatsSource): ExportSection {
  const rows = source.requests ?? [];
  if (source.source !== "codebuddy-cli" || rows.length === 0) {
    // 只有 CLI 提供逐请求明细；其它来源明说，避免用户以为数据丢了。
    return {
      name: "请求明细",
      header: ["说明"],
      rows: [["当前数据源不提供逐条请求明细（仅 CodeBuddy CLI 支持）"]],
    };
  }
  return {
    name: "请求明细",
    header: [
      "时间",
      "模型",
      "项目",
      "会话",
      "标题",
      "账号",
      "输入",
      "输出",
      "缓存读",
      "缓存写",
      "思考",
      "总 Token",
    ],
    rows: rows.map((row) => [
      formatRequestTime(row.timestamp),
      row.model,
      row.project,
      row.sessionId,
      row.title ?? "",
      accountCell(row.uid),
      row.input,
      row.output,
      row.cacheRead,
      row.cacheWrite,
      row.thinking,
      row.total,
    ]),
  };
}

/** 一次导出用到的全部区块（CSV 与 xlsx 共用）；`source` 应已按范围切片。 */
export function tokenStatsSections(source: TokenStatsSource, range: StatsRangeKey): ExportSection[] {
  return [
    summarySection(source, range),
    keyedSection("按日", "日期", source.daily),
    keyedSection("按模型", "模型", source.models),
    keyedSection("按项目", "项目", source.projects),
    sessionSection(source),
    keyedSection("按小时", "小时", source.hours),
    requestSection(source),
  ];
}

/** CSV：每段自己的表头；数值保持纯数字（不做强制文本）；段标题加 `# ` 前缀。 */
export function buildTokenStatsCsv(source: TokenStatsSource, range: StatsRangeKey): string {
  const sections: CsvSection[] = tokenStatsSections(source, range).map((section) => ({
    title: `# ${section.name}`,
    headers: section.header,
    rows: section.rows.map((row) => row.map((cell) => (cell === null ? "" : cell))),
  }));
  return buildCsv(sections);
}

/** xlsx：每个区块一个工作表，首行冻结 + 表头样式 + 数值类型。 */
export function tokenStatsSheets(source: TokenStatsSource, range: StatsRangeKey): XlsxSheet[] {
  return tokenStatsSections(source, range).map((section) => ({
    name: section.name,
    header: section.header,
    rows: section.rows,
  }));
}

/** 生成 xlsx 字节（前端生成，走 Blob 下载）。 */
export function buildTokenStatsXlsx(source: TokenStatsSource, range: StatsRangeKey): Uint8Array {
  return buildXlsx(tokenStatsSheets(source, range));
}

/**
 * 把导出数据切片到当前统计范围（纯函数，页面与单测共用）。
 *
 * 切片口径与页面**同一份实现**，不另抄一套：
 * - 汇总 → `overviewTotals`（首屏四卡与结论行的分子）；
 * - 按日 → `rangePoints`（趋势图）；
 * - 按模型 → `modelTotalsInRange`（用量分布·模型；旧后端无 `dailyByModel` 时与页面同样退回全量聚合）；
 * - 请求明细 → 按 `timestamp` 落在范围内的行（仅 CodeBuddy CLI 提供）。
 *
 * 不切片：projects / sessions / hours / accounts。后端只下发全量聚合，页面也明标
 * 「不受统计范围影响」；按范围硬切会把它们算成 0，是假数字。
 */
export function sliceTokenStatsSource(
  source: TokenStatsSource,
  range: StatsRangeKey,
): TokenStatsSource {
  const requests = source.requests?.filter((row) =>
    statsDateInRange(dateKey(new Date(row.timestamp)), range),
  );
  return {
    ...source,
    summary: overviewTotals(source, range),
    daily: rangePoints(source.daily, range),
    models: modelTotalsInRange(source, range),
    ...(requests ? { requests } : {}),
  };
}

/** 文件名里的紧凑范围标签（`近 30 天` → `近30天`）；文案唯一来源是 `statsRangeLabel`。 */
export function rangeFileLabel(range: StatsRangeKey): string {
  return statsRangeLabel(range).replace(/\s+/g, "");
}

/** 导出作用域：数据源 + 账号筛选（账号名会被清洗成安全文件名片段）+ 范围标签。 */
function exportScope(
  source: TokenStatsSource["source"],
  range: StatsRangeKey,
  accountLabel?: string,
): string {
  return [source, accountLabel, rangeFileLabel(range)].filter(Boolean).join("-");
}

export function tokenStatsFileName(
  source: TokenStatsSource["source"],
  range: StatsRangeKey,
  accountLabel?: string,
): string {
  return csvFileName("token-stats", exportScope(source, range, accountLabel));
}

export function tokenStatsXlsxFileName(
  source: TokenStatsSource["source"],
  range: StatsRangeKey,
  accountLabel?: string,
): string {
  return tokenStatsFileName(source, range, accountLabel).replace(/\.csv$/, ".xlsx");
}
