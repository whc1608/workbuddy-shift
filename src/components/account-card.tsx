import { useEffect, useState } from "react";

import { TooltipProvider } from "@/components/ui/tooltip";
import { displayName } from "@/lib/account-display";
import { avatarTone } from "@/lib/avatar-tone";

import { AccountCardActions } from "./account-card/tool-actions";
import { AccountCardHeader } from "./account-card/header";
import { AccountCreditSection } from "./account-card/credit-section";
import type { AccountCardProps } from "./account-card/types";
import { AccountStatusChips } from "./account-card/status-chips";

export function AccountCard(props: AccountCardProps) {
  const {
    account,
    autoCheckinAllowed,
    codebuddyCliActive,
    codebuddyCnIdeActive,
    compact = false,
    creditPriority,
    jetbrainsActive,
    rateLimits,
    rateLimitBorrowed,
    todayCheckedIn,
    travelStatus,
    vscodeExtActive,
    workbuddyActive,
  } = props;
  const [now, setNow] = useState(() => Date.now());
  /**
   * 限额图标与标记图标都要随时间自动消失，所以本地每秒走一次时钟：
   * - 限额：官方恢复时刻到了就消失（AC3）；
   * - 标记（R5-04）：`expiresAt` 到了就隐藏，「剩 32 分钟」也要自己走。
   * 只在确实需要倒计时时才开定时器，普通卡片不引入额外开销。
   */
  const markExpiresAt = account.mark?.expiresAt ?? null;
  useEffect(() => {
    if (!rateLimits?.length && markExpiresAt === null) return;
    const timer = window.setInterval(() => setNow(Date.now()), 1_000);
    return () => window.clearInterval(timer);
  }, [rateLimits, markExpiresAt]);
  /**
   * 「最近切换」徽标（issue #184）按分钟粒度展示（`3 小时前`），单独一个 60s 时钟：
   * 复用上面那个 1s 倒计时会让所有切过的账号每秒重渲染，没必要。
   */
  const lastUsedAt = account.lastUsedAt ?? null;
  useEffect(() => {
    if (lastUsedAt === null) return;
    const timer = window.setInterval(() => setNow(Date.now()), 60_000);
    return () => window.clearInterval(timer);
  }, [lastUsedAt]);
  const name = displayName(account);
  const avatarClass = avatarTone(name);
  const activeProductCount = [workbuddyActive, codebuddyCliActive, codebuddyCnIdeActive, vscodeExtActive, jetbrainsActive].filter(Boolean).length;

  return (
    <TooltipProvider>
      <article className="flex min-w-0 flex-col overflow-hidden rounded-[14px] border border-border bg-card shadow-[var(--shadow-card),inset_0_1px_0_var(--card-highlight)] transition-shadow hover:shadow-[var(--shadow-card)]">
        <AccountCardHeader
          {...props}
          name={name}
          avatarClass={avatarClass}
          statusChips={
            <AccountStatusChips
              account={account}
              activeProductCount={activeProductCount}
              autoCheckinAllowed={autoCheckinAllowed}
              compact={compact}
              creditPriority={creditPriority}
              now={now}
              onEditMark={props.onShowMark ? () => props.onShowMark?.(props.account) : undefined}
              rateLimits={rateLimits}
              rateLimitBorrowed={rateLimitBorrowed}
              todayCheckedIn={todayCheckedIn}
              travelStatus={travelStatus}
            />
          }
        />
        {props.decryptWriteHint ? (
          <div className={compact ? "px-3.5 pb-3" : "px-5 pb-3"}>{props.decryptWriteHint}</div>
        ) : null}
        <AccountCreditSection
          account={account}
          compact={compact}
          credit={props.credit}
          creditLoading={props.creditLoading}
          creditUpdatedAt={props.creditUpdatedAt}
          name={name}
        />
        {!compact && <AccountCardActions {...props} />}
      </article>
    </TooltipProvider>
  );
}
