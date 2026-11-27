import { CircleAlert, Loader2 } from "lucide-react";
import { creditResourceName } from "@/lib/credit-package-names";
import { formatAmount, formatDateTime, formatFullDateTime, pad2 } from "@/lib/format";
import { statsRangeLabel, type StatsRangeKey } from "@/lib/stats-range";
import { cn } from "@/lib/utils";
import type {
  CreditExpiry,
  CreditResource,
} from "@/lib/types";

import { expiryClass } from "@/components/account-card/formatters";
import { UNKNOWN_START_AT_NOTICE, filterResourcesByRange, isKnownStartAt } from "./detail-filters";

/** `MM/DD`（完整时刻放 `title`）：紧凑表格里年份没有信息量，月份日期足够排序判断。 */
function shortMonthDay(value: number | null | undefined): string | null {
  if (!value) return null;
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return null;
  return `${pad2(date.getMonth() + 1)}/${pad2(date.getDate())}`;
}

/** 到期日的 `MM/DD`；没有或无法解析的到期时间即长期。 */
function shortExpiry(expireAt: number | null): string {
  return shortMonthDay(expireAt) ?? "长期";
}

/** 未使用（剩余 === 总量）时的中性色：灰 = 还没动过，不是「已用完」。 */
const UNUSED_BAR = "bg-muted-foreground/35";

/** 迷你条的配色：过期红 / 临期橙（已登记的例外）> 未使用灰 > 用过的主题蓝。 */
function barTone(resource: CreditResource): string {
  if (resource.expired) return "bg-red-600/60 dark:bg-red-400/60";
  if (resource.expiringSoon) return "bg-orange-500/80";
  if (resource.used <= 0) return UNUSED_BAR;
  return "bg-[var(--data-series-blue)]";
}

/** 列宽固定：名称自适应，其余五列定宽（资源包 / 获取 / 到期 / 已用 / 剩余 / 占比）。 */
const GRID_COLUMNS = "grid grid-cols-[minmax(0,1fr)_52px_52px_64px_104px_64px]";

export function ResourceBreakdown({
  credit,
  loading,
  range,
}: {
  credit?: CreditExpiry;
  loading?: boolean;
  /** 页面统一的统计范围：按资源包的获取时间 `startAt` 过滤。 */
  range: StatsRangeKey;
}) {
  if (loading) {
    return (
      <div className="flex items-center gap-2 px-4 py-8 text-sm text-muted-foreground sm:px-5">
        <Loader2 className="size-4 animate-spin" />
        正在加载资源包…
      </div>
    );
  }
  if (!credit) {
    return <div className="px-4 py-8 text-center text-sm text-muted-foreground sm:px-5">尚未采集当前资源包。</div>;
  }
  if (!credit.ok) {
    return (
      <div className="flex items-start gap-2 px-4 py-8 text-sm text-destructive sm:px-5">
        <CircleAlert className="mt-0.5 size-4 shrink-0" />
        <span>{credit.error || "积分资源查询失败"}</span>
      </div>
    );
  }
  const allResources = credit.resources ?? [];
  // 已用完的资源包即使还没到期也不再占用明细列表
  const activeResources = allResources.filter((resource) => resource.remaining > 0);
  if (allResources.length === 0) {
    return <div className="px-4 py-8 text-center text-sm text-muted-foreground sm:px-5">当前没有可展示的资源包。</div>;
  }
  if (activeResources.length === 0) {
    return <div className="px-4 py-8 text-center text-sm text-muted-foreground sm:px-5">当前没有可用积分，已用完的资源包不展示。</div>;
  }
  // 日期范围按获取时间二次过滤；获取时间未知的资源包始终保留并在行内标注，
  // 所以表头的个数与总剩余始终等于下方可见列表。
  const resources = filterResourcesByRange(activeResources, range);
  const rangeFiltered = range !== "all";
  if (resources.length === 0) {
    return (
      <div className="px-4 py-8 text-center text-sm text-muted-foreground sm:px-5">
        {`「${statsRangeLabel(range)}」内没有获取的资源包，可切换为「全部」查看。`}
      </div>
    );
  }
  const totalRemaining = resources.reduce((sum, resource) => sum + resource.remaining, 0);
  const unknownStartAtCount = resources.filter((resource) => !isKnownStartAt(resource.startAt)).length;

  return (
    <div className="min-w-0">
      {rangeFiltered && unknownStartAtCount > 0 && (
        <div className="flex items-start gap-2 border-b bg-warning-soft px-4 py-2 text-[11px] text-warning-foreground sm:px-5">
          <CircleAlert className="mt-0.5 size-3.5 shrink-0" />
          <span>
            {UNKNOWN_START_AT_NOTICE}
            {`（${unknownStartAtCount} 个）`}
          </span>
        </div>
      )}
      {/* 表头即结论：按什么筛选、共几个包、还剩多少、什么时候采的 */}
      <div className="flex min-w-0 flex-wrap items-center justify-between gap-x-3 gap-y-1 border-b border-border/60 px-4 py-2 text-[11px] leading-5 text-muted-foreground sm:px-5">
        <span className="min-w-0">
          {rangeFiltered && `按获取时间「${statsRangeLabel(range)}」筛选 · `}
          共 {resources.length} 个资源包 · 合计剩余{" "}
          <span className="font-medium tabular-nums text-foreground">{formatAmount(totalRemaining)}</span>
        </span>
        <span className="min-w-0 tabular-nums">
          {credit.updatedAt ? `最近采集 ${formatDateTime(credit.updatedAt)}` : "最近采集 —"}
        </span>
      </div>
      {/* 行高 34px 换一屏能看 18 行以上 */}
      <div
        className={cn(
          GRID_COLUMNS,
          "items-center gap-3 border-b border-border/40 px-4 py-1 text-[10px] font-medium uppercase tracking-wide text-muted-foreground sm:px-5",
        )}
      >
        <span>资源包</span>
        <span className={cn("text-right", rangeFiltered && "text-foreground")}>获取</span>
        <span className="text-right">到期</span>
        <span className="text-right">已用</span>
        <span className="text-right">剩余 / 总量</span>
        <span className="text-right">占比</span>
      </div>
      <div className="divide-y divide-border/60">
        {resources.map((resource, index) => {
          const ratio =
            resource.total > 0
              ? Math.min(100, Math.max(0, (resource.remaining / resource.total) * 100))
              : 0;
          const unused = resource.used <= 0;
          const name = creditResourceName(resource, "未命名资源包");
          const knownStartAt = isKnownStartAt(resource.startAt);
          return (
            <div
              key={`${resource.packageCode || resource.packageName || "resource"}-${index}`}
              className={cn(GRID_COLUMNS, "h-[34px] min-w-0 items-center gap-3 px-4 text-xs sm:px-5")}
            >
              <span className="min-w-0 truncate font-medium" title={name}>
                {name}
              </span>
              <span
                className={cn("text-right tabular-nums", !knownStartAt && "text-warning-foreground")}
                title={knownStartAt ? formatFullDateTime(resource.startAt) : "获取时间未知，不参与日期范围筛选"}
              >
                {knownStartAt ? shortMonthDay(resource.startAt) : "未知"}
              </span>
              <span
                className={cn(
                  "text-right tabular-nums",
                  expiryClass(resource.expired, resource.expiringSoon),
                )}
                title={resource.expired ? "已到期" : formatFullDateTime(resource.expireAt)}
              >
                {shortExpiry(resource.expireAt)}
              </span>
              <span className="text-right tabular-nums text-muted-foreground">
                {unused ? "—" : formatAmount(resource.used)}
              </span>
              <span className="min-w-0 text-right tabular-nums">
                <span
                  className={cn(
                    "text-[15px] font-semibold leading-none",
                    unused ? "text-muted-foreground" : "text-foreground",
                  )}
                >
                  {formatAmount(resource.remaining)}
                </span>
                <span className="ml-1 text-[11px] font-normal text-muted-foreground">
                  / {formatAmount(resource.total)}
                </span>
              </span>
              <span
                className="ml-auto h-1.5 w-16 overflow-hidden rounded-full bg-muted"
                aria-hidden="true"
                title={`${ratio.toFixed(0)}%`}
              >
                <span
                  className={cn("block h-full rounded-full", barTone(resource))}
                  style={{ width: `${ratio}%` }}
                />
              </span>
            </div>
          );
        })}
      </div>
    </div>
  );
}
