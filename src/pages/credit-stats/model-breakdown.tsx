import { useState } from "react";
import { Badge } from "@/components/ui/badge";
import { Card, CardContent, CardHeader } from "@/components/ui/card";
import { formatAmount } from "@/lib/format";
import type { StatsRangeKey } from "@/lib/stats-range";
import type {
  CreditOfficialUsage,
  CreditOfficialUsageModel,
} from "@/lib/types";

import { AccountFilterMenu } from "./account-filter-menu";
import { formatPercent, rangeModels } from "./data";

function ModelBreakdownRows({ models }: { models: CreditOfficialUsageModel[] }) {
  const totalCredit = models.reduce((sum, model) => sum + model.credit, 0);
  const totalRequests = models.reduce((sum, model) => sum + model.requestCount, 0);

  return (
    <div className="space-y-3">
      {models.map((model) => {
        const ratio = totalCredit > 0 ? model.credit / totalCredit : totalRequests > 0 ? model.requestCount / totalRequests : 0;
        const label = model.model === "—" ? "未知模型" : model.model;
        return (
          <div key={model.model} className="min-w-0">
            <div className="flex min-w-0 items-center justify-between gap-3 text-xs">
              <span className="min-w-0 truncate font-medium" title={label}>
                {label}
              </span>
              <span className="shrink-0 text-muted-foreground">
                {formatAmount(model.credit)} 积分 · {formatAmount(model.requestCount)} 次
                <span className="ml-1.5 font-medium text-foreground">{formatPercent(ratio)}</span>
              </span>
            </div>
            <div className="mt-1.5 h-1.5 overflow-hidden rounded-full bg-muted" aria-hidden="true">
              <div className="h-full rounded-full bg-emerald-500/75" style={{ width: `${Math.min(100, Math.max(0, ratio * 100))}%` }} />
            </div>
          </div>
        );
      })}
    </div>
  );
}

export function ModelBreakdown({
  officialUsage,
  range,
  heading = true,
}: {
  officialUsage: CreditOfficialUsage;
  /** 全局统计范围：本卡片不再单独持有范围状态。 */
  range: StatsRangeKey;
  /** 折叠标题已承担 `h2` 角色时不再重复渲染标题。 */
  heading?: boolean;
}) {
  /** null = 所有账号汇总；本卡片独立，不影响其他卡片（与时间范围是两个维度） */
  const [accountFilter, setAccountFilter] = useState<string | null>(null);
  const filterAccounts = officialUsage.accounts;
  const activeFilterAccount =
    accountFilter && filterAccounts.some((account) => account.accountId === accountFilter)
      ? filterAccounts.find((account) => account.accountId === accountFilter)
      : undefined;
  const effectiveFilter = activeFilterAccount?.accountId ?? null;
  // 按选中账号 + 全局范围，从逐日模型聚合求和（全量，不受明细条数上限影响）
  const models = rangeModels(officialUsage, range, effectiveFilter);
  const totalCredit = models.reduce((sum, model) => sum + model.credit, 0);
  const totalRequests = models.reduce((sum, model) => sum + model.requestCount, 0);

  const card = (
    <Card className="min-w-0 gap-0 overflow-hidden rounded-xl py-0 shadow-none">
      <CardHeader className="gap-0 px-4 pt-3 pb-0 sm:px-5">
        <div className="flex min-w-0 flex-wrap items-center justify-between gap-2">
          <Badge variant="outline" className="shrink-0">
            {models.length} 个模型
          </Badge>
          <div className="flex flex-wrap items-center justify-end gap-1.5">
            <AccountFilterMenu
              accounts={filterAccounts}
              accountFilter={effectiveFilter}
              onAccountFilterChange={setAccountFilter}
              ariaLabel="按账号筛选模型分类"
            />
          </div>
        </div>
      </CardHeader>
      {models.length === 0 ? (
        <CardContent className="px-4 py-8 text-center text-sm text-muted-foreground sm:px-5">
          {activeFilterAccount && !activeFilterAccount.ok ? "该账号官方用量暂不可用。" : "官方暂无可用的模型消耗明细。"}
        </CardContent>
      ) : (
        <CardContent className="px-4 pt-3 pb-4 sm:px-5">
          <div className="mb-4 flex flex-wrap items-center justify-between gap-2 text-xs text-muted-foreground">
            <span>共 {formatAmount(totalRequests)} 次请求</span>
            <span className="font-medium text-foreground">合计 {formatAmount(totalCredit)} 积分</span>
          </div>
          <ModelBreakdownRows models={models} />
        </CardContent>
      )}
    </Card>
  );

  if (!heading) return <div className="min-w-0">{card}</div>;

  return (
    <section className="min-w-0 space-y-2.5" aria-labelledby="model-breakdown-title">
      <div className="px-1">
        <h2 id="model-breakdown-title" className="text-[13px] font-medium leading-5">按模型分类</h2>
      </div>
      {card}
    </section>
  );
}
