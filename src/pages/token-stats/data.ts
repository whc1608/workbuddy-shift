import { readPreference, writePreference } from "@/lib/persisted-preference";
import { dateDaysAgo, dateKey, pad2 } from "@/lib/format";
import {
  statsDateInRange,
  statsRangeLabel,
  type StatsRangeKey,
} from "@/lib/stats-range";
import type {
  TokenStatsGroup,
  TokenStatsSource,
  TokenStatsTotals,
} from "@/lib/types";

export type SourceKey = TokenStatsSource["source"];

/**
 * 逐日切片口径统一用 `StatsRangeKey`（见 `@/lib/stats-range`）：
 * 范围选项只在那一处定义，本文件只负责「按范围过滤 / 补齐日期轴 / 求和」。
 */
export type RangeKey = StatsRangeKey;

export type DistributionKey = "projects" | "models";

const TOKEN_SOURCE_STORAGE_KEY = "workbuddy-shift:token-stats:source";

export const RANKING_LIMIT = 10;

export const REQUEST_PAGE_SIZE = 50;

export function isSourceKey(value: unknown): value is SourceKey {
  return (
    value === "workbuddy" ||
    value === "workbuddy-ai" ||
    value === "codebuddy-cli" ||
    value === "vscode-ext" ||
    value === "jetbrains" ||
    value === "codebuddy-ide"
  );
}

const TOKEN_SOURCE_PREFERENCE = {
  key: TOKEN_SOURCE_STORAGE_KEY,
  fallback: "workbuddy" as SourceKey,
  parse: (raw: string): SourceKey | null => (isSourceKey(raw) ? raw : null),
  serialize: (source: SourceKey) => source,
  eventName: "workbuddy-shift:token-stats-source-changed",
} as const;

export function readPreferredTokenSource(): SourceKey {
  return readPreference(TOKEN_SOURCE_PREFERENCE);
}

export function persistPreferredTokenSource(source: SourceKey): void {
  writePreference(TOKEN_SOURCE_PREFERENCE, source);
}

const compactTokenFormatter = new Intl.NumberFormat("en-US", {
  notation: "compact",
  compactDisplay: "short",
  maximumFractionDigits: 1,
});

export const exact = new Intl.NumberFormat("zh-CN");

const exactTokenFormatter = new Intl.NumberFormat("en-US");

export function formatTokenCompact(value: number): string {
  return compactTokenFormatter
    .format(value)
    .replace(/([kmb])$/i, (unit) => unit.toUpperCase());
}

export function formatTokenExact(value: number): string {
  return exactTokenFormatter.format(value);
}

/** 展示总量：input 已包含 cacheRead，因此不能再次加上 cacheRead。 */
export const tokenTotal = (value: TokenStatsTotals) =>
  value.input + value.output + value.cacheWrite;

export const percentage = (value: number, sum: number) =>
  sum > 0 ? `${((value / sum) * 100).toFixed(1)}%` : "—";

export function formatChartDate(date: string): string {
  return date.slice(5).replace("-", "/");
}

/** 请求明细的精确时间：本地时区 `YYYY-MM-DD HH:mm:ss`。 */
export function formatRequestTime(timestamp: number): string {
  const date = new Date(timestamp);
  return `${date.getFullYear()}-${pad2(date.getMonth() + 1)}-${pad2(date.getDate())} ${pad2(date.getHours())}:${pad2(date.getMinutes())}:${pad2(date.getSeconds())}`;
}

/**
 * 行级账号短标签：`账号 …abc123`（uid 后 6 位）；空 uid 是「未归属」。
 *
 * 排行榜/请求明细里同名会话可能属于不同账号，必须能区分；这里不查账号库，避免把
 * 账号昵称（可能含隐私）渲染到图表行里，也便于纯逻辑单测。
 */
export function shortAccountLabel(uid?: string): string | null {
  if (uid === undefined) return null;
  if (!uid) return "未归属";
  return `账号 …${uid.slice(-6)}`;
}

export function formatHeatmapDate(date: Date): string {
  return date.toLocaleDateString("zh-CN", {
    month: "long",
    day: "numeric",
  });
}

export function rangeLabel(range: RangeKey): string {
  return statsRangeLabel(range);
}

export function rangePoints(daily: TokenStatsGroup[], range: RangeKey): TokenStatsGroup[] {
  return daily
    .filter((point) => statsDateInRange(point.key, range))
    .sort((left, right) => left.key.localeCompare(right.key));
}

/**
 * 范围的首尾日期；`all` 返回 null（日期轴就是数据本身的跨度，不需要补齐）。
 *
 * `month` 从本月 1 日到今天，`90d` 从 89 天前开始——与 `statsDateInRange` 完全一致。
 */
function rangeBounds(range: RangeKey): { start: string; end: string } | null {
  if (range === "all") return null;
  const today = dateKey(new Date());
  if (range === "today") return { start: today, end: today };
  if (range === "7d") return { start: dateDaysAgo(6), end: today };
  if (range === "30d") return { start: dateDaysAgo(29), end: today };
  if (range === "90d") return { start: dateDaysAgo(89), end: today };
  const monthStart = new Date();
  monthStart.setHours(12, 0, 0, 0);
  monthStart.setDate(1);
  return { start: dateKey(monthStart), end: today };
}

export function fillRangePoints(points: TokenStatsGroup[], range: RangeKey): TokenStatsGroup[] {
  if (points.length === 0) return [];
  const bounds = rangeBounds(range);
  // 「全部」不补零：账本/日志的跨度由数据自己决定，补到某个猜测的起点反而会造假。
  if (!bounds) return [...points].sort((left, right) => left.key.localeCompare(right.key));
  const { start, end } = bounds;
  const byDate = new Map(points.map((point) => [point.key, point]));
  const cursor = new Date(`${start}T12:00:00`);
  const endDate = new Date(`${end}T12:00:00`);
  const filled: TokenStatsGroup[] = [];
  while (cursor <= endDate) {
    const key = dateKey(cursor);
    const point = byDate.get(key);
    filled.push(
      point ?? {
        key,
        total: 0,
        input: 0,
        output: 0,
        cacheRead: 0,
        cacheWrite: 0,
        uncachedInput: 0,
        records: 0,
        cacheHitRate: null,
      },
    );
    cursor.setDate(cursor.getDate() + 1);
  }
  return filled;
}

export function rangeTotals(points: TokenStatsGroup[]): TokenStatsTotals {
  const totals = points.reduce(
    (sum, point) => ({
      total: sum.total + tokenTotal(point),
      input: sum.input + point.input,
      output: sum.output + point.output,
      cacheRead: sum.cacheRead + point.cacheRead,
      cacheWrite: sum.cacheWrite + point.cacheWrite,
      uncachedInput: sum.uncachedInput + point.uncachedInput,
      records: sum.records + point.records,
      cacheHitRate: null,
    }),
    {
      total: 0,
      input: 0,
      output: 0,
      cacheRead: 0,
      cacheWrite: 0,
      uncachedInput: 0,
      records: 0,
      cacheHitRate: null,
    } satisfies TokenStatsTotals,
  );

  return {
    ...totals,
    cacheHitRate: totals.input > 0 ? totals.cacheRead / totals.input : null,
  };
}

export function overviewTotals(source: TokenStatsSource, range: RangeKey): TokenStatsTotals {
  // 「全部」直接用后端汇总（本地日志全量），与积分页「全部 = 本地账本」语义对齐：
  // 两页的最后一个范围项都表示「不受窗口限制的全量累计」。
  if (range === "all") return source.summary;
  return rangeTotals(rangePoints(source.daily, range));
}

/**
 * 指定范围内各模型的用量。
 *
 * 逐模型日序列（`dailyByModel`）是模型维度唯一能按范围切的数据；旧后端不下发时退回
 * `models`（全量聚合），此时范围只影响第一屏四卡与趋势，模型占比仍是全量口径。
 */
export function modelTotalsInRange(source: TokenStatsSource, range: RangeKey): TokenStatsGroup[] {
  const byModel = source.dailyByModel;
  if (range === "all" || !byModel) return source.models;
  return Object.entries(byModel)
    .map(([model, points]) => ({ ...rangeTotals(rangePoints(points, range)), key: model }))
    .filter((group) => tokenTotal(group) > 0)
    .sort((left, right) => tokenTotal(right) - tokenTotal(left));
}

/** 头部占比结论：第一屏那一行文字与折叠区标题共用。 */
export interface TopGroupShare {
  key: string;
  amount: number;
  share: number;
  total: number;
  count: number;
}

export function topGroupShare(groups: TokenStatsGroup[]): TopGroupShare | null {
  const rows = groups
    .map((group) => ({ key: group.key, amount: tokenTotal(group) }))
    .filter((row) => row.amount > 0)
    .sort((left, right) => right.amount - left.amount);
  const total = rows.reduce((sum, row) => sum + row.amount, 0);
  const top = rows[0];
  if (!top || total <= 0) return null;
  return { key: top.key, amount: top.amount, share: top.amount / total, total, count: rows.length };
}

/**
 * 热力图 53 周窗口内的活跃天数。
 *
 * 与 `heatmap.tsx` 的网格起点保持同一算法（周日对齐、回退 52 周），标题里的「N 个
 * 活跃日」才不会和图表 `aria-label` 里的数字打架。
 */
export function heatmapActiveDays(groups: TokenStatsGroup[], today = new Date()): number {
  const anchor = new Date(today);
  anchor.setHours(12, 0, 0, 0);
  const start = new Date(anchor);
  start.setDate(start.getDate() - start.getDay() - 52 * 7);
  const startKey = dateKey(start);
  const todayKey = dateKey(anchor);
  return groups.filter(
    (group) => group.key >= startKey && group.key <= todayKey && tokenTotal(group) > 0,
  ).length;
}

/**
 * 某个来源「最后一条日志距今几天」（按整日差，不是 24 小时差）。
 *
 * Token 统计只吃**本机日志**：客户端升级后如果不再写日志，趋势图就会凭空少掉最近
 * 几天的柱子，看起来像统计坏了。实测 CodeBuddy IDE 在 2026-11-27 夜里升级后就不再
 * 更新 `history/<id>/index.json`，而页面此前没有任何提示。
 *
 * 返回 `null` = 没有覆盖信息（旧后端）、无法解析，或最后一条落在将来（时钟偏差）；
 * 这三种情况都不该提示，由调用方决定从第几天开始提醒。
 */
export function staleLogDays(
  coverageEndAt: number | null | undefined,
  now: number = Date.now(),
): number | null {
  if (!coverageEndAt) return null;
  const end = new Date(coverageEndAt);
  if (Number.isNaN(end.getTime())) return null;
  const today = new Date(now);
  const endDay = new Date(end.getFullYear(), end.getMonth(), end.getDate()).getTime();
  const todayDay = new Date(today.getFullYear(), today.getMonth(), today.getDate()).getTime();
  const days = Math.round((todayDay - endDay) / 86_400_000);
  return days > 0 ? days : null;
}