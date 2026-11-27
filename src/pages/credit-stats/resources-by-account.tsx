import type {
  CreditExpiry,
  CreditStatsAccount,
} from "@/lib/types";

import { accountLabel } from "./data";
import { ResourceBreakdown } from "./resource-breakdown";

export function ResourcesByAccount({
  accounts,
  creditMap,
  creditLoadingMap,
}: {
  accounts: CreditStatsAccount[];
  creditMap: Record<string, CreditExpiry>;
  creditLoadingMap: Record<string, boolean>;
}) {
  if (accounts.length === 0) {
    return <div className="px-4 py-8 text-center text-sm text-muted-foreground sm:px-5">暂无账号统计。</div>;
  }
  if (accounts.length === 1) {
    const account = accounts[0];
    return <ResourceBreakdown credit={creditMap[account.accountId]} loading={creditLoadingMap[account.accountId]} />;
  }
  return (
    <div className="divide-y divide-border/60">
      {accounts.map((account) => (
        <div key={account.accountId} className="min-w-0">
          <div className="px-4 py-2.5 text-xs font-medium sm:px-5">{accountLabel(account)}</div>
          <ResourceBreakdown credit={creditMap[account.accountId]} loading={creditLoadingMap[account.accountId]} />
        </div>
      ))}
    </div>
  );
}
