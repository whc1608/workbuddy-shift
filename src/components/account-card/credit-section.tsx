import { useState } from "react";
import { ArrowRight, Clock3, Coins, Loader2, PackageOpen, Sparkles } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { chipClass } from "./status-chips";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { cn } from "@/lib/utils";
import { creditResourceName } from "@/lib/credit-package-names";
import { useCreditBarTones } from "@/lib/credit-bar-palette";
import { formatAmount, formatFullDateTime } from "@/lib/format";
import type { AccountMeta, CreditExpiry, CreditResource } from "@/lib/types";

import { expiryClass, formatCreditExpiry, formatCreditUpdatedAt, creditResources } from "./formatters";
interface AccountCreditSectionProps {
  account: AccountMeta;
  compact: boolean;
  credit?: CreditExpiry;
  creditLoading?: boolean;
  creditUpdatedAt?: number;
  name: string;
}

export function AccountCreditSection({ account, compact, credit, creditLoading, creditUpdatedAt, name }: AccountCreditSectionProps) {
  const [resourcesOpen, setResourcesOpen] = useState(false);
  // 三态配色的唯一来源：默认主题蓝，可在「设置 → 外观」切成绿 / 橙 / 红。
  const tones = useCreditBarTones();
  const resources = creditResources(credit);
  const visibleResources = resources.slice(0, 2);
  const expiringAmount = credit?.ok ? credit.expiringSoonRemaining ?? 0 : 0;
  return (
    <>
      <section className={cn("flex min-w-0 flex-1 flex-col", compact ? "px-3.5 pb-4 pt-3" : "px-5 pb-5 pt-4")}>
        {creditLoading ? (
          <div className="flex items-center gap-2 py-3 text-sm text-muted-foreground"><Loader2 className="size-4 animate-spin" />积分查询中…</div>
        ) : !credit ? (
          <div className="py-3 text-sm text-muted-foreground">等待积分数据…</div>
        ) : !credit.ok ? (
          <div className="flex min-w-0 items-center gap-2 py-3 text-sm text-danger" title={credit.error}>
            <Coins className="size-4 shrink-0" />
            <span className="min-w-0 truncate">{credit.error || "积分查询失败"}</span>
          </div>
        ) : (
          <>
            <div className="flex items-baseline gap-x-3 gap-y-1">
              <span className="flex items-center gap-1.5">
                <Sparkles className="size-4 shrink-0 stroke-[1.75] text-muted-foreground" aria-hidden="true" />
                <strong className={cn("font-semibold leading-none tabular-nums tracking-[-0.025em]", compact ? "text-[20px]" : "text-[22px]")} style={{ fontFamily: '"Bricolage Grotesque Variable", "SF Pro Display", ui-sans-serif, sans-serif' }}>{credit.unlimited ? "不限量" : formatAmount(credit.totalRemaining ?? 0)}</strong>
                {/* 紧凑卡片的头部单行放不下企业版徽标（会挤掉账号名），改在大数字旁标注。 */}
                {compact && account.isEnterprise && <Badge variant="secondary" className={chipClass}>企业版</Badge>}
              </span>
              <span className={cn("text-muted-foreground", compact ? "text-[11px]" : "text-xs")}>{credit.unlimited || account.isEnterprise ? "企业配额" : `${resources.length} 个积分包`}</span>
              <div className={cn("ml-auto flex items-center gap-1.5 text-muted-foreground", compact ? "text-[11px]" : "text-xs")} title={expiringAmount > 0 ? `${formatAmount(expiringAmount)} 积分将在 7 天内到期` : resources[0]?.expireAt ? `最近到期 ${formatCreditExpiry(resources[0].expireAt)}` : "当前积分长期有效"}>
                <Clock3 className="size-3.5 shrink-0" />
                <span className="whitespace-nowrap tabular-nums">{creditUpdatedAt ? `${formatCreditUpdatedAt(creditUpdatedAt)} 更新` : "—"}</span>
              </div>
            </div>

            <div className={cn("flex items-center justify-between gap-2", compact ? "mt-3" : "mt-4")}>
              <span className="text-[11px] font-medium text-muted-foreground">近期到期</span>
              {resources.length > 2 && (
                <button
                  type="button"
                  className="inline-flex shrink-0 cursor-pointer items-center gap-1 text-[11px] font-medium text-primary transition-colors hover:text-primary/80 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary/30"
                  onClick={() => setResourcesOpen(true)}
                >
                  查看全部积分包
                  <ArrowRight className="size-3.5" />
                </button>
              )}
            </div>
            {/* 上下间距统一：列表上方与内容区底部 padding 同值（紧凑 12px / 宽松 16px）。
                否则「标签→首条」比「末条→卡片底」窄，垂直节奏不对称。 */}
            <div className={cn(compact ? "mt-3 space-y-2" : "mt-4 space-y-2.5")}>
              {credit.unlimited ? (
                // 不限量没有积分包可列：用一条「企业配额 · 不限量」行表达（复用占位行样式，不引入新组件）。
                <CreditResourceRow compact={compact} placeholderLabel="企业配额 · 不限量" />
              ) : (
                [0, 1].map((index) => {
                  const resource = visibleResources[index];
                  // 一条积分都没有时，第一槽位显示空态提示，第二槽位仍是纯占位（不再重复"暂无其他积分包"）
                  if (!resource && index === 0 && visibleResources.length === 0) {
                    return <div key="credit-empty" className="py-1 text-[11px] text-muted-foreground">暂无可用积分</div>;
                  }
                  return (
                    <CreditResourceRow
                      key={resource ? `${resource.packageCode ?? "resource"}-${resource.expireAt ?? "none"}-${index}` : `credit-slot-${index}`}
                      resource={resource}
                      compact={compact}
                      placeholderLabel={visibleResources.length > 0 ? "暂无其他积分包" : undefined}
                    />
                  );
                })
              )}
            </div>
          </>
        )}
      </section>

      <Dialog open={resourcesOpen} onOpenChange={setResourcesOpen}>
        <DialogContent className="sm:max-w-md">
          <DialogHeader>
            <DialogTitle>全部积分包</DialogTitle>
            <DialogDescription>{name} · 共 {resources.length} 个积分包</DialogDescription>
          </DialogHeader>
          {resources.length === 0 ? (
            <div className="px-1 py-6 text-center text-sm text-muted-foreground">当前没有可展示的资源包。</div>
          ) : (
            <div className="max-h-[60vh] min-w-0 overflow-y-auto divide-y divide-border/60">
              {resources.map((resource, index) => {
                const ratio = resource.total > 0 ? Math.min(100, Math.max(0, (resource.remaining / resource.total) * 100)) : 0;
                return (
                  <div key={`${resource.packageCode || resource.packageName || "resource"}-${index}`} className="min-w-0 py-3 first:pt-0 last:pb-0">
                    <div className="flex min-w-0 items-start justify-between gap-3">
                      <div className="min-w-0">
                        <div className="truncate text-sm font-medium">{creditResourceName(resource, "未命名资源包")}</div>
                        <div className="mt-1 text-[11px] text-muted-foreground">
                          {resource.expired ? "已到期" : resource.expireAt ? `到期 ${formatFullDateTime(resource.expireAt)}` : "长期有效"}
                        </div>
                      </div>
                      <div className="shrink-0 text-right text-xs">
                        <div className="font-medium">{formatAmount(resource.remaining)} / {formatAmount(resource.total)}</div>
                        <div className="mt-1 text-[11px] text-muted-foreground">已用 {formatAmount(resource.used)}</div>
                      </div>
                    </div>
                    <div className="mt-2 h-1.5 overflow-hidden rounded-full bg-muted" aria-hidden="true">
                      <div className={cn("h-full rounded-full", resource.expired ? tones.expired : resource.expiringSoon ? tones.expiringSoon : tones.normal)} style={{ width: `${ratio}%` }} />
                    </div>
                  </div>
                );
              })}
            </div>
          )}
        </DialogContent>
      </Dialog>
    </>
  );
}
export function CreditResourceRow({ resource, compact, placeholderLabel }: { resource?: CreditResource; compact: boolean; placeholderLabel?: string }) {
  const placeholder = !resource;
  // hook 取在提前 return 之前：占位行同样走同一条渲染路径，不能破坏 hooks 顺序。
  const tones = useCreditBarTones();

  if (placeholder && placeholderLabel) {
    return (
      <div className="min-w-0">
        <div className={cn("grid min-w-0 grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-3", compact ? "text-[11px]" : "text-xs")}>
          {/* 放在第一列：与真实行的徽标同起点，即贴列表左缘 */}
          <span className={cn("flex items-center gap-1.5 text-muted-foreground", compact ? "py-0.5" : "py-1")}>
            <PackageOpen className="size-3.5 shrink-0" aria-hidden="true" />
            {placeholderLabel}
          </span>
          <span className="invisible truncate">{"\u00a0"}</span>
          <span className="invisible">{"\u00a0"}</span>
        </div>
        {/* 与真实行的进度条等高，但不画轨道 —— 占位不该看起来像"剩余为 0" */}
        <div className={cn("h-1", compact ? "mt-1" : "mt-1.5")} aria-hidden="true" />
      </div>
    );
  }

  const name = resource ? creditResourceName(resource, "积分包") : "\u00a0";
  const remainingText = resource ? `${formatAmount(resource.remaining)} 积分` : "\u00a0";
  const expiryText = resource ? formatCreditExpiry(resource.expireAt) : "\u00a0";
  const expiryTitle = resource?.expireAt ? `${expiryText} 到期` : expiryText;
  const ratio = resource && resource.total > 0 ? Math.min(100, Math.max(0, (resource.remaining / resource.total) * 100)) : 0;
  // 三态与「全部积分包」弹窗、积分明细页保持一致：已过期红 / 即将过期橙 / 正常可用色
  // （正常可用默认主题蓝，可在「设置 → 外观 → 积分条状态色」切成绿 / 橙 / 红）。
  const barTone = resource?.expired
    ? tones.expired
    : resource?.expiringSoon
      ? tones.expiringSoon
      : tones.normal;
  const title = resource ? `${name} · 剩余 ${formatAmount(resource.remaining)} / ${formatAmount(resource.total)} · ${expiryTitle}` : undefined;
  return (
    <div className={cn("min-w-0", placeholder && "invisible")} aria-hidden={placeholder || undefined} title={title}>
      <div className={cn("grid min-w-0 grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-3", compact ? "text-[11px]" : "text-xs")}>
        <span className={cn("rounded-lg bg-muted/80 font-medium tabular-nums text-foreground", compact ? "px-1.5 py-0.5" : "px-2 py-1")}>{remainingText}</span>
        <span className="truncate text-muted-foreground">{name}</span>
        <span className={cn("whitespace-nowrap tabular-nums", resource && expiryClass(resource.expired, resource.expiringSoon))}>{expiryText}</span>
      </div>
      <div className={cn("h-1 overflow-hidden rounded-full bg-muted", compact ? "mt-1" : "mt-1.5")} aria-hidden="true">
        <div className={cn("h-full rounded-full", barTone)} style={{ width: `${ratio}%` }} />
      </div>
    </div>
  );
}
