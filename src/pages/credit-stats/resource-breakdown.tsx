import { CircleAlert, Loader2 } from "lucide-react";
import { creditResourceName } from "@/lib/credit-package-names";
import { formatAmount, formatFullDateTime } from "@/lib/format";
import type {
  CreditExpiry,
} from "@/lib/types";

export function ResourceBreakdown({ credit, loading }: { credit?: CreditExpiry; loading?: boolean }) {
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
  const resources = allResources.filter((resource) => resource.remaining > 0);
  if (allResources.length === 0) {
    return <div className="px-4 py-8 text-center text-sm text-muted-foreground sm:px-5">当前没有可展示的资源包。</div>;
  }
  if (resources.length === 0) {
    return <div className="px-4 py-8 text-center text-sm text-muted-foreground sm:px-5">当前没有可用积分，已用完的资源包不展示。</div>;
  }

  return (
    <div className="divide-y divide-border/60">
      {resources.map((resource, index) => {
        const ratio = resource.total > 0 ? Math.min(100, Math.max(0, (resource.remaining / resource.total) * 100)) : 0;
        return (
          <div key={`${resource.packageCode || resource.packageName || "resource"}-${index}`} className="min-w-0 px-4 py-1.5 sm:px-5">
            <div className="flex min-w-0 items-center justify-between gap-2">
              <div className="min-w-0 truncate text-[13px] font-medium">{creditResourceName(resource, "未命名资源包")}</div>
              <div className="flex shrink-0 items-center gap-2.5">
                <span className="text-[11px] text-muted-foreground">
                  {resource.expired ? "已到期" : `到期 ${formatFullDateTime(resource.expireAt)}`}
                  {resource.used > 0 ? ` · 已用 ${formatAmount(resource.used)}` : ""}
                </span>
                <span className="text-xs font-medium">{formatAmount(resource.remaining)} / {formatAmount(resource.total)}</span>
              </div>
            </div>
            <div className="mt-1 h-1 overflow-hidden rounded-full bg-muted" aria-hidden="true">
              <div className="h-full rounded-full bg-emerald-500/75" style={{ width: `${ratio}%` }} />
            </div>
          </div>
        );
      })}
    </div>
  );
}
