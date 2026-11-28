/**
 * 积分明细（资源包 / 请求用量）的账号与日期范围过滤。
 *
 * 日期切片口径只有一份：`statsDateInRange`（`@/lib/stats-range`），与总览、趋势、
 * 模型统计同一来源，这里不另写一套。本文件只保留两类有独立价值的逻辑：
 *
 * - **请求时间解析**：官方 `requestTime` 形态不固定（无时区文本 / RFC3339 / 秒或毫秒
 *   时间戳），要先换算成本地日历日才能交给 `statsDateInRange`；
 * - **资源包 `startAt`**：获取时间未知时无法判定是否命中，因此保留展示并由 UI 标注，
 *   绝不静默丢弃。
 */
import { dateKey } from "@/lib/format";
import { statsDateInRange, type StatsRangeKey } from "@/lib/stats-range";
import type { CreditOfficialUsageRequest, CreditResource } from "@/lib/types";

export const UNKNOWN_START_AT_NOTICE = "获取时间未知的资源包仍保留展示，不参与日期范围筛选。";

const DATE_KEY = /^(\d{4})-(\d{2})-(\d{2})$/;
const LOCAL_DATETIME = /^(\d{4}-\d{2}-\d{2})[ T](\d{2}:\d{2})(?::(\d{2})(?:\.\d+)?)?$/;
const ZONED_DATETIME = /^(\d{4}-\d{2}-\d{2})[ T](\d{2}:\d{2})(?::(\d{2})(\.\d+)?)?(Z|[+-]\d{2}:?\d{2})$/i;
const NUMERIC_TIMESTAMP = /^-?\d+$/;

function validDateKey(value: string): string | null {
  const match = DATE_KEY.exec(value);
  if (!match) return null;
  const year = Number(match[1]);
  const month = Number(match[2]);
  const day = Number(match[3]);
  const date = new Date(0);
  date.setHours(0, 0, 0, 0);
  date.setFullYear(year, month - 1, day);
  return date.getFullYear() === year && date.getMonth() === month - 1 && date.getDate() === day
    ? value
    : null;
}

function validTime(hourMinute: string, second: string): boolean {
  const [hour, minute] = hourMinute.split(":").map(Number);
  const seconds = Number(second);
  return hour >= 0 && hour <= 23 && minute >= 0 && minute <= 59 && seconds >= 0 && seconds <= 59;
}

function localDateKeyOf(date: Date): string | null {
  return Number.isNaN(date.getTime()) ? null : dateKey(date);
}

/**
 * 请求时间 → 本地日期键。无时区的官方时间按本地日历日处理；带时区时间先换算为本地日期。
 * 纯数字按秒/毫秒时间戳解析；非法或含义不明的输入返回 `null`，不猜测日期。
 */
export function extractRequestDateKey(requestTime: string): string | null {
  const text = requestTime.trim();
  if (!text) return null;
  if (NUMERIC_TIMESTAMP.test(text)) {
    const value = Number(text);
    if (!Number.isSafeInteger(value)) return null;
    return localDateKeyOf(new Date(Math.abs(value) < 10_000_000_000 ? value * 1000 : value));
  }

  const zoned = ZONED_DATETIME.exec(text);
  if (zoned) {
    const [, day, hourMinute, second = "00", fraction = "", rawZone] = zoned;
    if (validDateKey(day) === null || !validTime(hourMinute, second)) return null;
    const zone = rawZone.toUpperCase();
    const offset = zone.length === 5 ? `${zone.slice(0, 3)}:${zone.slice(3)}` : zone;
    const millis = fraction.slice(0, 4);
    return localDateKeyOf(new Date(`${day}T${hourMinute}:${second}${millis}${offset}`));
  }

  const local = LOCAL_DATETIME.exec(text);
  if (local) {
    const [, day, hourMinute, second = "00"] = local;
    return validDateKey(day) !== null && validTime(hourMinute, second) ? day : null;
  }
  return validDateKey(text);
}

/**
 * 单条请求是否落在范围内；`all` 恒为真，具体范围排除解析不出日期的请求。
 *
 * `now` 只是参考日（今天 / 本月 / 上界），交给 `statsDateInRange`；近 7 / 30 / 90 天的
 * 下界由它按系统时钟计算，口径与页面其它区块完全一致。
 */
export function isRequestInRange(
  request: Pick<CreditOfficialUsageRequest, "requestTime">,
  range: StatsRangeKey,
  now: Date = new Date(),
): boolean {
  if (range === "all") return true;
  const requestDate = extractRequestDateKey(request.requestTime ?? "");
  return requestDate !== null && statsDateInRange(requestDate, range, dateKey(now));
}

/** 按账号（`null` = 所有账号）与日期范围过滤请求；`all` 保留全部已加载明细。 */
export function filterRequestsByAccountAndRange(
  requests: readonly CreditOfficialUsageRequest[],
  accountId: string | null,
  range: StatsRangeKey,
  now: Date = new Date(),
): CreditOfficialUsageRequest[] {
  return requests.filter(
    (request) =>
      (accountId === null || request.accountId === accountId) && isRequestInRange(request, range, now),
  );
}

/** 与展示层一致：非正数、非有限数与无效 Date 都视为未知获取时间。 */
export function isKnownStartAt(value: number | null | undefined): value is number {
  return typeof value === "number" && Number.isFinite(value) && value > 0 && !Number.isNaN(new Date(value).getTime());
}

/**
 * 已知 `startAt` 的资源按日期范围过滤，保持原顺序。
 * 未知时间无法判定是否命中，因此在所有范围内都保留，由 UI 明确标注，绝不静默丢弃。
 */
export function filterResourcesByRange(
  resources: readonly CreditResource[],
  range: StatsRangeKey,
  now: Date = new Date(),
): CreditResource[] {
  return resources.filter((resource) => {
    if (range === "all" || !isKnownStartAt(resource.startAt)) return true;
    const startDate = localDateKeyOf(new Date(resource.startAt));
    return startDate !== null && statsDateInRange(startDate, range, dateKey(now));
  });
}

/** 到期时间的可比较值：缺失 / 无效（长期包）返回 `null`。 */
function comparableExpiry(resource: CreditResource): number | null {
  const value = resource.expireAt;
  if (typeof value !== "number" || !Number.isFinite(value) || Number.isNaN(new Date(value).getTime())) {
    return null;
  }
  return value;
}

/**
 * 明细列表按「最早到期」在前排序，临期 / 已到期的包优先露出。
 *
 * 后端返回顺序没有时间语义（企业配额与个人资源包混在同一份列表里），直接渲染会出现
 * 「11/30 到期的包排在最后」这种与直觉相反的排布。到期时间缺失（长期包）视为最大，
 * 统一落到末尾；到期时间相同时保持后端原顺序（`Array#sort` 稳定），方便对照同一份数据。
 */
export function sortResourcesByExpiry(resources: readonly CreditResource[]): CreditResource[] {
  return resources
    .map((resource, index) => ({ resource, index, expiry: comparableExpiry(resource) }))
    .sort((a, b) => {
      if (a.expiry !== null && b.expiry !== null) {
        if (a.expiry !== b.expiry) return a.expiry - b.expiry;
      } else if (a.expiry !== b.expiry) {
        return a.expiry === null ? 1 : -1;
      }
      return a.index - b.index;
    })
    .map((entry) => entry.resource);
}
