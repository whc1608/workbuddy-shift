import { useState } from "react";
import { Card, CardDescription, CardHeader } from "@/components/ui/card";
import { formatDateTime } from "@/lib/format";
import {
  readPreference,
  usePersistedPreferenceState,
  writePreference,
  type PersistedPreferenceOptions,
} from "@/lib/persisted-preference";
import type { StatsRangeKey } from "@/lib/stats-range";
import type {
  CreditExpiry,
  CreditOfficialUsage,
  CreditStatistics,
} from "@/lib/types";

import { AccountFilterMenu } from "./account-filter-menu";
import { isOfficialUsageAvailable } from "./data";
import { OfficialUsageBreakdown } from "./official-usage";
import { ResourcesByAccount } from "./resources-by-account";

/**
 * 「所有账号」只来自用户主动选择。已选账号失效（账号列表刷新）时回退首账号，
 * 不能被当成选了全部，否则明细会在用户没操作的情况下突然变成全量。
 */
type DetailAccountSelection =
  | { kind: "all" }
  | { kind: "account"; accountId: string | null };

/**
 * 记住上次看的账号：切到别的页面再回来不该跳回第一个账号。
 *
 * 存 `all` 或 `account:<accountId>`。账号被删 / 换机器导致存的值对不上时，沿用下面
 * `effectiveFilter` 的既有规则回退首个账号；这里**不**顺手改写存储，否则「账号列表暂时
 * 没加载完」会被误判成「账号已删除」，把用户的选择覆盖掉。
 */
const ACCOUNT_SELECTION_PREFERENCE: PersistedPreferenceOptions<string> = {
  key: "workbuddy-shift.credit-detail-account",
  fallback: "",
  parse: (raw) => raw,
  serialize: (value) => value,
  eventName: "workbuddy-shift:credit-detail-account-changed",
};

const ACCOUNT_PREFIX = "account:";

/** 「积分明细 / 请求用量」子 Tab：记住上次看的那一栏。 */
const DETAIL_TAB_PREFERENCE: PersistedPreferenceOptions<"credits" | "requests"> = {
  key: "workbuddy-shift.credit-detail-tab",
  fallback: "credits",
  parse: (raw) => (raw === "credits" || raw === "requests" ? raw : null),
  serialize: (value) => value,
  eventName: "workbuddy-shift:credit-detail-tab-changed",
};

function readDetailAccountSelection(): DetailAccountSelection {
  const raw = readPreference(ACCOUNT_SELECTION_PREFERENCE);
  if (raw === "all") return { kind: "all" };
  if (raw.startsWith(ACCOUNT_PREFIX)) {
    return { kind: "account", accountId: raw.slice(ACCOUNT_PREFIX.length) || null };
  }
  return { kind: "account", accountId: null };
}

function persistDetailAccountSelection(selection: DetailAccountSelection): void {
  writePreference(
    ACCOUNT_SELECTION_PREFERENCE,
    selection.kind === "all" ? "all" : `${ACCOUNT_PREFIX}${selection.accountId ?? ""}`,
  );
}

export function SelectedAccountDetails({
  stats,
  officialUsage,
  creditMap,
  creditLoadingMap,
  range,
  heading = true,
}: {
  stats: CreditStatistics;
  officialUsage?: CreditOfficialUsage;
  creditMap: Record<string, CreditExpiry>;
  creditLoadingMap: Record<string, boolean>;
  /** 折叠标题已承担 `h2` 角色时不再重复渲染标题。 */
  heading?: boolean;
  /** 页面统一的统计范围；卡片内不再另设范围按钮。 */
  range: StatsRangeKey;
}) {
  const [detailTab, setDetailTab] = usePersistedPreferenceState(DETAIL_TAB_PREFERENCE);
  const official = isOfficialUsageAvailable(officialUsage) ? officialUsage : undefined;
  const filterAccounts = official ? official.accounts : stats.accounts;
  /** 默认读上次的选择；没有记录时 `accountId` 为 null，再由 `effectiveFilter` 落到首账号 */
  const [accountSelection, setAccountSelection] = useState<DetailAccountSelection>(readDetailAccountSelection);
  const activeFilterAccount =
    accountSelection.kind === "account" && accountSelection.accountId !== null
      ? filterAccounts.find((account) => account.accountId === accountSelection.accountId)
      : undefined;
  const effectiveFilter =
    accountSelection.kind === "all" ? null : (activeFilterAccount?.accountId ?? filterAccounts[0]?.accountId ?? null);
  const visibleAccounts = effectiveFilter
    ? stats.accounts.filter((account) => account.accountId === effectiveFilter)
    : stats.accounts;
  // 最近事件卡片已隐藏，events 不再使用。恢复时取消本注释。
  // const events = (effectiveFilter
  //   ? stats.events.filter((event) => event.accountId === effectiveFilter)
  //   : stats.events
  // ).slice(0, 50);
  const latestSnapshotAt = visibleAccounts.reduce<number | null>((latest, account) => {
    if (account.lastSnapshotAt == null) return latest;
    if (latest == null || account.lastSnapshotAt > latest) return account.lastSnapshotAt;
    return latest;
  }, null);

  // R5-15：不随账号切换重置 detailTab。切号后保持当前子 Tab，
  // 无数据 / 查询失败由 OfficialUsageBreakdown 自身的空态与错误态覆盖，
  // 分页仍由该组件在 accountId 或 range 变化时回到第 1 页。
  return (
    <div className="flex min-w-0 flex-col gap-12">
      <section
        className="min-w-0 space-y-2.5"
        aria-labelledby={heading ? "credit-detail-title" : undefined}
      >
        {heading ? (
          <div className="px-1">
            <h2 id="credit-detail-title" className="text-[13px] font-medium leading-5">积分明细</h2>
          </div>
        ) : null}
        <Card className="min-w-0 gap-0 overflow-hidden rounded-xl py-0 shadow-none">
          <CardHeader className="gap-0 border-b px-4 pt-3 pb-3 sm:px-5">
            <div className="flex min-w-0 flex-wrap items-center justify-between gap-3">
              <CardDescription className="min-w-0 truncate text-xs">
                {latestSnapshotAt ? `最近采集 ${formatDateTime(latestSnapshotAt)}` : "暂无账号资源包。"}
              </CardDescription>
              <AccountFilterMenu
                accounts={filterAccounts}
                accountFilter={effectiveFilter}
                onAccountFilterChange={(accountId) => {
                  const next: DetailAccountSelection =
                    accountId === null ? { kind: "all" } : { kind: "account", accountId };
                  persistDetailAccountSelection(next);
                  setAccountSelection(next);
                }}
                ariaLabel="按账号筛选积分明细"
                allowAll={true}
              />
            </div>
            <div className="mt-3 flex max-w-full gap-1 rounded-lg bg-muted p-1" role="tablist" aria-label="积分详情类型">
              {(
                [
                  ["credits", "积分明细"],
                  ["requests", "请求用量"],
                ] as const
              ).map(([value, label]) => (
                <button
                  key={value}
                  type="button"
                  role="tab"
                  aria-selected={detailTab === value}
                  className={`min-w-0 flex-1 rounded-md px-2.5 py-1.5 text-xs transition-colors ${
                    detailTab === value
                      ? "bg-background font-medium text-foreground shadow-sm"
                      : "text-muted-foreground hover:text-foreground"
                  }`}
                  onClick={() => setDetailTab(value)}
                >
                  {label}
                </button>
              ))}
            </div>
          </CardHeader>
          {detailTab === "credits" ? (
            <div className="min-w-0">
              <ResourcesByAccount
                accounts={visibleAccounts}
                creditMap={creditMap}
                creditLoadingMap={creditLoadingMap}
              />
            </div>
          ) : (
            <OfficialUsageBreakdown officialUsage={officialUsage} accountId={effectiveFilter} range={range} />
          )}
        </Card>
      </section>
    </div>
  );
}
