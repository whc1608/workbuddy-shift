import { useEffect, useMemo, useState, type ReactNode } from "react";
import { Link } from "react-router-dom";
import { CircleAlert, Download, FileSpreadsheet, Info, ListTree, Loader2, RefreshCw } from "lucide-react";
import { toast } from "sonner";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { DemoAction } from "@/components/demo-action";
import { ExportMenu } from "@/components/export-menu";
import { StatsRangeControl } from "@/components/stats-range";
import { CodeBuddyCnIdeMark, CodeBuddyMark, JetbrainsMark, VscodeExtMark, WorkBuddyAiMark, WorkBuddyMark } from "@/components/product-marks";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Skeleton } from "@/components/ui/skeleton";
import * as api from "@/lib/api";
import { formatDate, formatDateTime } from "@/lib/format";
import { usePersistedPreferenceState } from "@/lib/persisted-preference";
import { toolEntryEnabled, useSupportedTools, type ToolId } from "@/lib/supported-tools";
import {
  statsDateSpanDays,
  STATS_RANGE_PREFERENCE,
} from "@/lib/stats-range";
import {
  isTokenStatisticsCacheFresh,
  loadTokenStatistics,
  readTokenStatisticsCache,
} from "@/lib/stats-cache";
import type {
  TokenStatistics,
} from "@/lib/types";
import { useAccountsStore } from "@/stores/accounts";

import { saveCsvFile, saveXlsxFile } from "@/lib/export-download";
import {
  type AccountSelection,
  accountOptions,
  cliAccountsUnsupported,
  filterSourceByAccount,
  hasAccountDimension,
  parseSelection,
  persistAccountSelection,
  readAccountSelection,
  selectionLabel,
  serializeSelection,
} from "./accounts";
import { TokenAccountSelect } from "./account-select";
import { Dashboard } from "./dashboard";
import { buildTokenStatsCsv, buildTokenStatsXlsx, tokenStatsFileName, tokenStatsXlsxFileName } from "./export";
import { SourceKey, isSourceKey, persistPreferredTokenSource, readPreferredTokenSource, staleLogDays } from "./data";
import { RequestDetailDialog } from "./request-detail";
import { TokenStatsLoadingSkeleton } from "./skeleton";

/**
 * 「最后一条日志距今几天」达到这个值才提示。
 *
 * 1 天不提示：早上打开时「昨天有日志」是正常状态，天天提示等于噪音；
 * 连续两天没有新日志才符合「最近怎么没数据了」的直觉。
 */
const STALE_HINT_DAYS = 2;

const SOURCE_KEYS: SourceKey[] = [
  "workbuddy",
  "workbuddy-ai",
  "codebuddy-cli",
  "vscode-ext",
  "jetbrains",
  "codebuddy-ide",
];

/**
 * 来源 → 设置页「支持工具」开关：关掉的客户端不再出现在来源标签里。
 *
 * WorkBuddy 国内版 / 国际版是两个独立开关（`workbuddy` / `workbuddyAi`），
 * 与来源一一对应；其余来源与客户端一一对应。
 */
const SOURCE_TOOL: Record<SourceKey, ToolId> = {
  workbuddy: "workbuddy",
  "workbuddy-ai": "workbuddyAi",
  "codebuddy-cli": "codebuddyCli",
  "vscode-ext": "vscodeExt",
  jetbrains: "jetbrains",
  "codebuddy-ide": "codebuddyIde",
};

/** 来源标签的图标与文案：顺序即标签顺序，渲染前按上面的开关过滤。 */
const SOURCE_TABS: { key: SourceKey; label: string; mark: ReactNode }[] = [
  { key: "workbuddy", label: "WorkBuddy", mark: <WorkBuddyMark size={16} /> },
  { key: "workbuddy-ai", label: "WorkBuddy 国际版", mark: <WorkBuddyAiMark size={16} /> },
  { key: "codebuddy-cli", label: "CodeBuddy CLI", mark: <CodeBuddyMark size={16} /> },
  { key: "vscode-ext", label: "VS Code 插件", mark: <VscodeExtMark size={16} /> },
  { key: "jetbrains", label: "JetBrains 插件", mark: <JetbrainsMark size={16} /> },
  { key: "codebuddy-ide", label: "CodeBuddy IDE", mark: <CodeBuddyCnIdeMark size={16} /> },
];

/** 每个来源记住上次选择的账号（第 7 条：筛选层级按来源独立）。 */
function readAllSelections(): Record<SourceKey, AccountSelection> {
  return SOURCE_KEYS.reduce(
    (all, key) => {
      all[key] = readAccountSelection(key);
      return all;
    },
    {} as Record<SourceKey, AccountSelection>,
  );
}


export default function TokenStatsPage() {
  // 首帧直接用共享缓存（启动预热可能已经扫完盘），避免把整页推回骨架屏再等一次 IPC。
  const [stats, setStats] = useState<TokenStatistics | null>(() => readTokenStatisticsCache());
  const [active, setActive] = useState<SourceKey>(readPreferredTokenSource);
  const [loading, setLoading] = useState(() => readTokenStatisticsCache() === null);
  const [error, setError] = useState<string | null>(null);
  /** 0 = 首次进入（缓存新鲜时不发请求）；>0 = 用户点了刷新/重试，强制重新扫描。 */
  const [reload, setReload] = useState(0);
  const [detailOpen, setDetailOpen] = useState(false);
  /**
   * 全局唯一的展示范围：四指标、结论、趋势图、用量分布都按它切片。
   * 持久化到 localStorage，与积分统计页共用同一份。
   */
  const [range, setRange] = usePersistedPreferenceState(STATS_RANGE_PREFERENCE);
  const enabledTools = useSupportedTools();
  /** 允许展示的来源：设置页关掉的客户端整条去掉（统计页也不该给它留一个入口）。 */
  const visibleSources = useMemo(
    () => SOURCE_TABS.filter((item) => toolEntryEnabled(enabledTools, SOURCE_TOOL[item.key])),
    [enabledTools],
  );

  useEffect(() => {
    const force = reload > 0;
    // 10 分钟窗口内直接渲染缓存：与启动预热共用同一份判定（都看响应自带的 generatedAt），
    // 因此「预热刚扫过 → 进入本页」不会触发第二次全量扫描。
    if (!force && isTokenStatisticsCacheFresh()) {
      setStats(readTokenStatisticsCache());
      setLoading(false);
      setError(null);
      return;
    }
    let disposed = false;
    setLoading(true);
    setError(null);
    // 统一走共享模块：已有在途请求会被 join，不会与预热并发扫两遍。
    loadTokenStatistics({ force })
      .then((result) => {
        if (disposed) return;
        setStats(result);
      })
      .catch((cause: unknown) => {
        // 失败时保留旧数据继续展示，由下面的错误条提示重试（不弹 toast）。
        if (!disposed) setError(api.asError(cause));
      })
      .finally(() => {
        if (!disposed) setLoading(false);
      });
    return () => {
      disposed = true;
    };
  }, [reload]);

  useEffect(() => {
    if (!stats || stats.sources.length === 0) return;
    // 只考虑「有数据且没被设置页关掉」的来源：否则会切到一个不存在的标签上。
    const available = stats.sources
      .map((item) => item.source)
      .filter((key) => visibleSources.some((item) => item.key === key));
    if (available.length === 0) return;
    const next = available.includes(active)
      ? active
      : available.includes("workbuddy")
        ? "workbuddy"
        : available[0];
    if (!next) return;
    if (next !== active) setActive(next);
    persistPreferredTokenSource(next);
  }, [active, stats, visibleSources]);

  const accounts = useAccountsStore((state) => state.accounts);
  const fetchAccounts = useAccountsStore((state) => state.fetchAll);
  const [selections, setSelections] = useState<Record<SourceKey, AccountSelection>>(readAllSelections);

  useEffect(() => {
    // 账号下拉的选项来自账号库：没有账号时补拉一次（与积分页同一数据源）。
    if (accounts.length === 0) void fetchAccounts();
  }, [accounts.length, fetchAccounts]);

  const source = stats?.sources.find((item) => item.source === active);
  /** 最后一条日志距今几天（`null` = 无覆盖信息）：达到 `STALE_HINT_DAYS` 才在页头提示。 */
  const logStaleDays = staleLogDays(source?.coverageEndAt);
  const options = useMemo(
    () => accountOptions(active, accounts, source?.accounts),
    [active, accounts, source],
  );
  // 保存的 uid 不在当前列表（账号被删、换机器、旧后端没有账号维度）时回退「所有账号」。
  const selection = useMemo<AccountSelection>(() => {
    const current = selections[active];
    if (!current || current.kind === "all") return { kind: "all" };
    const value = serializeSelection(current);
    return options.some((option) => option.value === value) ? current : { kind: "all" };
  }, [selections, active, options]);
  const selectionValue = serializeSelection(selection);
  const filteredSource = useMemo(
    () => (source ? filterSourceByAccount(source, selection) : undefined),
    [source, selection],
  );
  const scopeLabel = selectionLabel(selection, accounts);
  const accountNotice = cliAccountsUnsupported(active)
    ? "CLI 日志不记录账号，只能按「未归属」统计；IDE 来源按数据目录 uid 归因。"
    : null;

  /**
   * 本地日志的实际跨度（首末日期之差，含首尾）。
   *
   * 「近 90 天」只在跨度够 90 天时渲染：本地日志不足 90 天时这个选项必然显示一个
   * 与「全部」相同的数字，属于假选项。已经在选中 90 天（例如换数据源）时保留按钮，
   * 避免选中项凭空消失。
   */
  const rangeSpanDays = useMemo(() => {
    const daily = filteredSource?.daily ?? [];
    if (daily.length === 0) return 0;
    let first = daily[0].key;
    let last = daily[0].key;
    for (const point of daily) {
      if (point.key < first) first = point.key;
      if (point.key > last) last = point.key;
    }
    return statsDateSpanDays(first, last);
  }, [filteredSource]);
  const include90d = rangeSpanDays >= 90 || range === "90d";

  /** 导出当前来源 + 当前账号：范围是全量日期，图表时间范围不参与。 */
  async function exportCsv() {
    if (!filteredSource) {
      toast.error("当前数据源没有可导出的数据");
      return;
    }
    try {
      const content = buildTokenStatsCsv(filteredSource);
      const outcome = await saveCsvFile(
        tokenStatsFileName(filteredSource.source, selection.kind === "all" ? undefined : scopeLabel),
        content,
      );
      if (outcome === "saved") {
        toast.success("已导出 Token 统计 CSV", {
          description: `${filteredSource.source} · ${scopeLabel} · 全量日期 · ${(content.length / 1024).toFixed(0)} KB`,
        });
      }
    } catch (cause) {
      toast.error("导出失败", { description: api.asError(cause) });
    }
  }



  /** 导出 Excel（xlsx）：同样按「当前来源 + 当前账号」，7 个 sheet 各一段。 */
  async function exportXlsx() {
    if (!filteredSource) {
      toast.error("当前数据源没有可导出的数据");
      return;
    }
    try {
      const bytes = buildTokenStatsXlsx(filteredSource);
      const outcome = await saveXlsxFile(
        tokenStatsXlsxFileName(filteredSource.source, selection.kind === "all" ? undefined : scopeLabel),
        bytes,
      );
      if (outcome === "saved") {
        toast.success("已导出 Token 统计 Excel", {
          description: `${filteredSource.source} · ${scopeLabel} · 全量日期 · ${(bytes.length / 1024).toFixed(0)} KB`,
        });
      }
    } catch (cause) {
      toast.error("导出失败", { description: api.asError(cause) });
    }
  }

  return (
    <div className="mx-auto w-full max-w-[1180px] min-w-0 px-4 py-6 sm:px-8 sm:py-9">
      <header className="mb-6 flex min-w-0 flex-wrap items-start justify-between gap-4">
        <div className="min-w-0">
          {loading && !stats ? (
            <div aria-hidden="true">
              <Skeleton className="h-8 w-40" />
              <Skeleton className="mt-2 h-5 w-64 max-w-full" />
            </div>
          ) : (
            <>
              <h1 className="text-[28px] font-semibold tracking-tight">Token 统计</h1>
              <p className="mt-2 max-w-2xl text-sm leading-6 text-muted-foreground">
                当前数据更新于 {stats ? formatDateTime(stats.generatedAt) : "—"}
              </p>
              {source ? (
                <p className="mt-1 max-w-2xl text-[11px] leading-5 text-muted-foreground">
                  本地日志口径（不是官方账单）：覆盖 {formatDate(source.coverageStartAt)} ~{" "}
                  {formatDate(source.coverageEndAt)} · 账号：{scopeLabel} · 逐请求明细仅 CodeBuddy CLI 提供
                </p>
              ) : null}
              {source && logStaleDays !== null && logStaleDays >= STALE_HINT_DAYS ? (
                <p className="mt-1 flex max-w-2xl items-start gap-1.5 text-[11px] leading-5 text-muted-foreground">
                  <Info className="mt-0.5 size-3.5 shrink-0 text-amber-600 dark:text-amber-500" aria-hidden="true" />
                  <span>
                    最近一次日志是 {formatDate(source.coverageEndAt)}（{logStaleDays} 天前）。客户端升级后可能不再写本机日志，
                    本页只能显示历史数据；近期消耗可切到「CodeBuddy CLI」来源，或看积分页的官方用量。
                  </span>
                </p>
              ) : null}
            </>
          )}
        </div>
        <div className="flex max-w-full flex-wrap items-center justify-end gap-2">
          <StatsRangeControl
            value={range}
            include90d={include90d}
            onChange={setRange}
            ariaLabel="统计范围"
          />
          {active === "codebuddy-cli" && (
            <Button
              className="shrink-0"
              variant="outline"
              size="sm"
              onClick={() => setDetailOpen(true)}
            >
              <ListTree />
              查看请求明细
            </Button>
          )}
          <ExportMenu
            disabled={!filteredSource}
            title={
              filteredSource
                ? "导出当前数据源 + 当前账号（全量日期，不受展示范围限制）"
                : "没有可导出的数据"
            }
            items={[
              {
                key: "csv",
                label: "CSV",
                description: "7 段明细 · UTF-8 BOM · Excel 可直接打开",
                icon: <Download className="size-3.5" />,
                onSelect: () => void exportCsv(),
              },
              {
                key: "xlsx",
                label: "Excel（xlsx）",
                description: "7 个 sheet · 数值可求和",
                icon: <FileSpreadsheet className="size-3.5" />,
                onSelect: () => void exportXlsx(),
              },
            ]}
          />
          <DemoAction>
            <Button
              className="shrink-0"
              variant="outline"
              size="sm"
              onClick={() => setReload((value) => value + 1)}
              disabled={loading}
            >
              {loading ? <Loader2 className="animate-spin" /> : <RefreshCw />}
              刷新统计
            </Button>
          </DemoAction>
        </div>
      </header>

      {/* 数据源 Tab 独占一行：数据源变多时不与右上角操作挤在同一排 */}
      {loading && !stats ? (
        <Skeleton className="mb-8 h-9 w-full max-w-md rounded-lg" aria-hidden="true" />
      ) : visibleSources.length === 0 ? (
        <div className="mb-8 rounded-xl border border-dashed border-border px-6 py-10 text-center">
          <p className="text-sm font-medium">未启用任何客户端</p>
          <p className="mt-2 text-sm text-muted-foreground">在「设置 → 支持工具」里开启客户端后，这里会显示它对应的 Token 统计。</p>
          <Button asChild variant="outline" size="sm" className="mt-4"><Link to="/settings">去设置</Link></Button>
        </div>
      ) : (
        <div className="mb-8 flex min-w-0 flex-wrap items-center justify-between gap-3">
          <Tabs
            className="min-w-0 gap-0"
            value={active}
            onValueChange={(value) => {
              if (!isSourceKey(value)) return;
              if (stats && !stats.sources.some((item) => item.source === value)) return;
              setActive(value);
            }}
          >
            <TabsList className="h-auto max-w-full flex-wrap" aria-label="Token 数据来源">
              {visibleSources.map((item) => (
                <TabsTrigger
                  key={item.key}
                  className="max-w-full gap-2 whitespace-normal"
                  value={item.key}
                  disabled={Boolean(stats && !stats.sources.some((entry) => entry.source === item.key))}
                >
                  {item.mark}
                  {item.label}
                </TabsTrigger>
              ))}
            </TabsList>
          </Tabs>
          {hasAccountDimension(source) ? (
            <TokenAccountSelect
              options={options}
              value={selectionValue}
              notice={accountNotice}
              onChange={(value) => {
                const next = parseSelection(value);
                if (!next) return;
                setSelections((previous) => ({ ...previous, [active]: next }));
                persistAccountSelection(active, next);
              }}
            />
          ) : null}
        </div>
      )}

      {error && (
        <Alert variant="destructive" className="mb-5">
          <CircleAlert />
          <AlertTitle>统计加载失败</AlertTitle>
          <AlertDescription className="flex flex-wrap items-center gap-3">
            <span>{error}</span>
            <Button size="sm" variant="outline" onClick={() => setReload((value) => value + 1)}>
              重试
            </Button>
          </AlertDescription>
        </Alert>
      )}

      {loading && !stats ? (
        <TokenStatsLoadingSkeleton />
      ) : filteredSource ? (
        <Dashboard source={filteredSource} scopeLabel={scopeLabel} range={range} />
      ) : (
        !error && (
          <div className="rounded-xl border border-dashed px-4 py-16 text-center text-sm text-muted-foreground">
            该来源暂无可用统计数据，请点击刷新重试。
          </div>
        )
      )}

      {filteredSource && (
        <RequestDetailDialog
          open={detailOpen}
          onOpenChange={setDetailOpen}
          source={filteredSource}
        />
      )}
    </div>
  );
}
