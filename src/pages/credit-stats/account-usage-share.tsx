import { useMemo, useState } from "react";
import { Badge } from "@/components/ui/badge";
import { Card, CardContent, CardDescription, CardHeader } from "@/components/ui/card";
import { formatAmount } from "@/lib/format";
import type {
  CreditOfficialUsage,
  CreditStatistics,
  WbVariant,
} from "@/lib/types";
import { DEFAULT_VARIANT, variantLabel } from "@/lib/variant";
import { RangeKey, USAGE_SHARE_RANGE_OPTIONS, VARIANT_GROUP_ORDER, accountLabel, isOfficialUsageAvailable, usageShareValue } from "./data";

export function AccountUsageShare({
  stats,
  officialUsage,
  variantByAccountId,
  ledger = false,
}: {
  stats: CreditStatistics;
  officialUsage?: CreditOfficialUsage;
  variantByAccountId: Map<string, WbVariant>;
  /** 数据是否来自「全部」的本地账本（只影响口径文案）。 */
  ledger?: boolean;
}) {
  /** 本卡片独立的时间范围，不影响其他卡片 */
  const [range, setRange] = useState<RangeKey>("today");
  const official = isOfficialUsageAvailable(officialUsage) ? officialUsage : undefined;

  // 官方用量优先，不可用时回退本地观察口径（与总览、趋势图一致）
  const rows = useMemo(() => {
    const source = official
      ? official.accounts.map((account) => ({
          accountId: account.accountId,
          accountName: account.accountName,
          usage: account.ok ? usageShareValue(account, account.daily, range) : null,
        }))
      : stats.accounts.map((account) => ({
          accountId: account.accountId,
          accountName: account.accountName,
          usage: usageShareValue(account, account.daily, range),
        }));
    return source
      .map((row) => ({ ...row, variant: variantByAccountId.get(row.accountId) ?? DEFAULT_VARIANT }))
      .sort(
        (a, b) =>
          (b.usage ?? -1) - (a.usage ?? -1) || accountLabel(a).localeCompare(accountLabel(b)),
      );
  }, [official, stats.accounts, range, variantByAccountId]);

  // 国内版与国际版积分体系不同，占比按档位分组、组内各算 100%；单档位视图下自然只有一组
  const groups = useMemo(() => {
    const grouped = new Map<WbVariant, typeof rows>();
    for (const row of rows) {
      const list = grouped.get(row.variant);
      if (list) list.push(row);
      else grouped.set(row.variant, [row]);
    }
    return [...grouped.entries()]
      .sort((a, b) => VARIANT_GROUP_ORDER.indexOf(a[0]) - VARIANT_GROUP_ORDER.indexOf(b[0]))
      .map(([variant, items]) => ({
        variant,
        items,
        total: items.reduce((sum, item) => sum + (item.usage ?? 0), 0),
      }));
  }, [rows]);

  // 官方账号失败时为 null，不能当 0 计入分母，否则占比会失真
  const measurable = rows.filter((row) => row.usage !== null);
  const grandTotal = groups.reduce((sum, group) => sum + group.total, 0);
  const showGroupLabel = groups.length > 1;

  return (
    <section className="min-w-0 space-y-2.5" aria-labelledby="account-usage-share-title">
      <div className="px-1">
        <h2 id="account-usage-share-title" className="text-[13px] font-medium leading-5">
          账号消耗构成
        </h2>
      </div>
      <Card className="min-w-0 gap-0 overflow-hidden rounded-xl py-0 shadow-none">
        <CardHeader className="gap-0 px-4 pt-3 pb-0 sm:px-5">
          <div className="flex min-w-0 flex-wrap items-center justify-between gap-3">
            <CardDescription className="min-w-0 text-xs">
              {official
                ? `${
                    ledger ? "来自本地账本（自工具首次抓取起累计）" : "来自 WorkBuddy 官方请求用量"
                  } · ${official.rangeStart} 至 ${official.rangeEnd}`
                : "按本地观察口径；官方用量恢复后刷新即可切换。"}
            </CardDescription>
            <div
              className="flex max-w-full flex-wrap gap-1 rounded-lg bg-muted p-1"
              aria-label="账号消耗构成范围"
            >
              {USAGE_SHARE_RANGE_OPTIONS.map((option) => (
                <button
                  key={option.key}
                  type="button"
                  className={`cursor-pointer rounded-md px-2.5 py-1.5 text-xs transition-colors ${
                    range === option.key
                      ? "bg-background font-medium text-foreground shadow-sm"
                      : "text-muted-foreground hover:text-foreground"
                  }`}
                  onClick={() => setRange(option.key)}
                  aria-pressed={range === option.key}
                >
                  {option.label}
                </button>
              ))}
            </div>
          </div>
        </CardHeader>
        {measurable.length === 0 ? (
          <CardContent className="px-4 py-8 text-center text-sm text-muted-foreground sm:px-5">
            {official ? "当前账号的官方用量暂不可用，刷新后重试。" : "暂无账号消耗数据。"}
          </CardContent>
        ) : grandTotal <= 0 ? (
          <CardContent className="px-4 py-8 text-center text-sm text-muted-foreground sm:px-5">
            当前范围暂未观察到积分消耗。
          </CardContent>
        ) : (
          <CardContent className="px-4 pt-3 pb-4 sm:px-5">
            <div className="mb-4 flex flex-wrap items-center justify-between gap-2 text-xs text-muted-foreground">
              <span>共 {rows.length} 个账号</span>
              <span className="font-medium text-foreground">合计 {formatAmount(grandTotal)} 积分</span>
            </div>
            <div className="space-y-4">
              {groups.map((group, index) => (
                <div
                  key={group.variant}
                  className={`min-w-0 space-y-3 ${index > 0 ? "border-t border-border/60 pt-4" : ""}`}
                >
                  {showGroupLabel && (
                    <div className="flex items-center justify-between gap-2">
                      <Badge variant="outline">{variantLabel(group.variant)}</Badge>
                      <span className="text-xs text-muted-foreground">
                        合计 {formatAmount(group.total)} 积分
                      </span>
                    </div>
                  )}
                  {group.items.map((row) => {
                    const ratio = row.usage !== null && group.total > 0 ? row.usage / group.total : 0;
                    const percent = ratio * 100;
                    return (
                      <div key={row.accountId} className="min-w-0">
                        <div className="flex min-w-0 items-center justify-between gap-3 text-xs">
                          <span className="min-w-0 truncate font-medium" title={accountLabel(row)}>
                            {accountLabel(row)}
                          </span>
                          <span className="shrink-0 text-muted-foreground">
                            {row.usage === null ? "官方用量不可用" : `${formatAmount(row.usage)} 积分`}
                            {row.usage !== null && (
                              <span className="ml-1.5 font-medium text-foreground">
                                {group.total > 0
                                  ? percent < 0.05
                                    ? "<0.1%"
                                    : `${percent.toFixed(1)}%`
                                  : "—"}
                              </span>
                            )}
                          </span>
                        </div>
                        <div className="mt-1.5 h-1.5 overflow-hidden rounded-full bg-muted" aria-hidden="true">
                          <div
                            className="h-full rounded-full bg-emerald-500/75"
                            style={{ width: `${Math.min(100, Math.max(0, ratio * 100))}%` }}
                          />
                        </div>
                      </div>
                    );
                  })}
                </div>
              ))}
            </div>
          </CardContent>
        )}
      </Card>
    </section>
  );
}
