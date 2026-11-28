import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Download, FileSpreadsheet } from "lucide-react";
import {
  CalendarDays,
  CalendarRange,
  CircleAlert,
  CircleCheck,
  Loader2,
  Sparkles,
  RefreshCw,
  TrendingDown,
} from "lucide-react";

import {
  Accordion,
  AccordionContent,
  AccordionItem,
  AccordionTrigger,
} from "@/components/ui/accordion";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { FailureActions } from "@/components/failure-actions";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { DemoAction } from "@/components/demo-action";
import { ExportMenu } from "@/components/export-menu";
import { OAuthLoginDialog } from "@/components/oauth-login-dialog";
import { StatsRangeControl } from "@/components/stats-range";
import * as api from "@/lib/api";
import { formatAmount, formatDateTime } from "@/lib/format";
import { saveCsvFile, saveXlsxFile } from "@/lib/export-download";
import {
  usePersistedPreferenceState,
  type PersistedPreferenceOptions,
} from "@/lib/persisted-preference";
import {
  invalidateCreditStatisticsCache,
  loadCreditStatistics,
  readCreditStatisticsCache,
} from "@/lib/stats-cache";
import {
  creditDataRange,
  monthSliceNote,
  STATS_RANGE_PREFERENCE,
  statsRangeDayCount,
  statsRangeLabel,
  statsRangeSupports90d,
  type StatsRangeKey,
} from "@/lib/stats-range";
import { toast } from "sonner";
import type {
  CreditRange,
  CreditStatistics,
  WbVariant,
} from "@/lib/types";

import { accountVariant } from "@/lib/variant";
import { toolEntryEnabled, useSupportedTools, type ToolId } from "@/lib/supported-tools";
import { useAccountsStore } from "@/stores/accounts";
import { SelectedAccountDetails } from "./account-details";
import {
  creditFailureActionsProps,
  findCreditFailure,
  reloginAccountOf,
  reloginVariantOf,
  type CreditFailureHit,
} from "./credit-failure";
import { AccountUsageShare } from "./account-usage-share";
import { ClientFilterMenu } from "./client-filter-menu";
import {
  CLIENT_VIEW_LABELS,
  VARIANT_VIEW_OPTIONS,
  accountUsageRows,
  clientViewOptions,
  filterOfficialByClient,
  formatPercent,
  isOfficialUsageAvailable,
  ledgerClientViewOptions,
  ledgerToOfficialUsage,
  rangeModels,
  rangePeakUsage,
  recomputeLocalStats,
  recomputeOfficialUsage,
  resolveAccountVariant,
  statsVariantViewLabel,
  topAccountShare,
  type CreditClientKey,
  type StatsVariantView,
} from "./data";
import {
  creditSourceBadge,
  localCoverageText,
  statsRangeConsumption,
} from "./coverage";
import { ModelBreakdown } from "./model-breakdown";
import { useProgressiveRefresh } from "./progressive-refresh";
import { buildCreditStatsCsv, creditStatsFileName } from "./export";
import { buildCreditStatsXlsx, creditStatsXlsxFileName } from "./export-xlsx";
import { StatMetric, TrendChart } from "./trend-chart";

/** 毫秒展示：未完成显示「…」，避免把 0 当成「瞬间完成」。 */
function formatMs(value: number | null): string {
  return value === null ? "…" : `${(value / 1000).toFixed(1)}s`;
}

/** 进入统计页时距上次采集超过此时长（ms）则自动重新采集一次 */
const STATISTICS_AUTO_REFRESH_MS = 30 * 60 * 1000;

/** 文件名片段的紧凑范围标签（`近 30 天` → `近30天`，避免文件名里出现空格）。 */
function rangeFileLabel(range: StatsRangeKey): string {
  return statsRangeLabel(range).replace(/\s+/g, "");
}

/**
 * 客户端筛选键 → 设置页「支持工具」开关。
 *
 * 「全部 / 未知」不属于任何客户端，永远保留：未知是数据里 `client` 缺失的行，
 * 跟开关无关，去掉会让「全部」与各客户端条目之和对不上。
 *
 * WorkBuddy 只映射到国内版开关：官方明细的 `client` 字段只有 `WorkBuddy` 一个值，
 * 分不出国内版 / 国际版（档位由账号决定，另有「档位」筛选），所以国际版开关
 * 管不到这个菜单。
 */
/**
 * 档位与客户端筛选：记住上次看的维度（与统计范围同一套 localStorage 偏好）。
 *
 * 只持久化「看的角度」；取数窗口 `dataRange` 由范围偏好派生，不单独存。
 * `parse` 严格校验取值：旧版本存过的键名可能已改名，认不出就回默认，而不是带着
 * 非法值渲染出一个没有任何选中项的工具栏。
 */
const VARIANT_VIEW_PREFERENCE: PersistedPreferenceOptions<StatsVariantView> = {
  key: "workbuddy-shift.credit-stats.variant",
  fallback: "all",
  parse: (raw) =>
    VARIANT_VIEW_OPTIONS.some((option) => option.key === raw) ? (raw as StatsVariantView) : null,
  serialize: (value) => value,
  eventName: "workbuddy-shift:credit-stats-variant-changed",
};

const CLIENT_VIEW_PREFERENCE: PersistedPreferenceOptions<CreditClientKey> = {
  key: "workbuddy-shift.credit-stats.client",
  fallback: "all",
  parse: (raw) =>
    Object.prototype.hasOwnProperty.call(CLIENT_VIEW_LABELS, raw) ? (raw as CreditClientKey) : null,
  serialize: (value) => value,
  eventName: "workbuddy-shift:credit-stats-client-changed",
};

const CLIENT_TOOL: Partial<Record<CreditClientKey, ToolId>> = {
  workbuddy: "workbuddy",
  vscode: "vscodeExt",
  jetbrains: "jetbrains",
  ide: "codebuddyIde",
  cli: "codebuddyCli",
};


export default function CreditStatsPage() {
  const {
    accounts,
    creditMap,
    creditLoadingMap,
    fetchAll,
    refreshCredits,
  } = useAccountsStore();
  /**
   * 全局唯一的展示范围（今天 / 7 天 / 30 天 / 本月 / 90 天 / 全部）。
   * 持久化到 localStorage，与 Token 统计页共用同一份。
   */
  const [range, setRange] = usePersistedPreferenceState(STATS_RANGE_PREFERENCE);
  /** 实际取数窗口：由展示范围映射而来，不新增后端 `range` 取值。 */
  const [dataRange, setDataRange] = useState<CreditRange>(() => {
    const cached = readCreditStatisticsCache();
    return creditDataRange(range, statsRangeSupports90d(cached?.windowProbe?.maxDays));
  });
  // 首帧仅在 30 天窗口时直接渲染共享缓存，避免恢复为 90 天 / 全部时拿 30 天数字冒充目标窗口。
  const [stats, setStats] = useState<CreditStatistics | null>(() =>
    dataRange === "30d" ? readCreditStatisticsCache() : null,
  );
  const [loading, setLoading] = useState(() =>
    dataRange === "30d" ? readCreditStatisticsCache() === null : true,
  );
  const [error, setError] = useState<string | null>(null);
  /**
   * 错误条的结构化失败（**只服务抛异常的分支**：`fetchAll` / `loadCreditStatistics`，
   * 例如 webui 的 `!res.ok`）。这些分支没有账号上下文，因此不给「重新登录」handler。
   *
   * 积分取数失败**不走这里**：`get_credit_expiry` 失败也是正常 resolve
   * （`{ok:false, error, failureKind?}`），桌面端 `catch` 根本不执行。那条链路的动作行
   * 由 `officialFailure`（`officialUsage.errors[]`，见下方派生处）负责。
   *
   * 这里**不读全局 store**：读 `creditMap` 会在「本轮没发起查询」时把上一次的失败重新
   * 显示出来（`loadCredits` 只补未缓存的 id，旧失败条目不会被重查）。
   */
  const [loadFailure, setLoadFailure] = useState<CreditFailureHit | null>(null);
  /** 「重新登录」复用现有的 OAuth 登录弹窗（不新造流程）。 */
  const [oauthOpen, setOauthOpen] = useState(false);
  const [viewVariant, setViewVariant] = usePersistedPreferenceState(VARIANT_VIEW_PREFERENCE);
  /** 客户端筛选（与档位正交）：只在明细上做，选具体值时四卡改「据明细汇总」。 */
  const [clientView, setClientView] = usePersistedPreferenceState(CLIENT_VIEW_PREFERENCE);
  const [rangeLoading, setRangeLoading] = useState(false);
  const initialRangeRef = useRef<CreditRange | null>(dataRange === "30d" ? null : dataRange);
  const syncedWindowKeyRef = useRef<string | null>(null);
  /** 单次窗口取数的序号：后发的请求作废先发的，避免慢响应把新窗口的数据改回去。 */
  const loadSeqRef = useRef(0);
  const unmountedRef = useRef(false);
  useEffect(() => {
    unmountedRef.current = false;
    return () => {
      unmountedRef.current = true;
    };
  }, []);

  // 官方窗口是否支持 90 天；探测结果没回来（null）时按不支持处理，不给必然降级的按钮。
  const supports90d = statsRangeSupports90d(stats?.windowProbe?.maxDays);
  // 已经选着 90 天时即使探测结论变了也保留该按钮，避免选中项突然消失。
  const include90d = supports90d || range === "90d";

  // 渐进式刷新：本地数字 → 积分 / 官方用量并行；按钮显示「更新中 N/2 项」。
  const progressive = useProgressiveRefresh({
    commit: (next) => {
      // `progressive-refresh` 直接调 api（三段编排要拿到各自的中间结果），绕过了共享缓存，
      // 这里让缓存失效，避免「刚刷新完、切页回来又看到旧数字」。
      invalidateCreditStatisticsCache();
      setStats(next);
      setError(null);
      setLoadFailure(null);
    },
    refreshCredits,
    range: dataRange,
    loadAccountIds: async () => {
      await fetchAll();
      return useAccountsStore.getState().accounts.map((account) => account.id);
    },
  });

  const load = useCallback(
    async (refresh = false, options?: { background?: boolean }) => {
      const background = options?.background === true;
      const cached = readCreditStatisticsCache();
      if (!refresh && cached) {
        setStats(cached);
        setLoading(false);
        setError(null);
        setLoadFailure(null);
        return;
      }
      // 后台刷新时保留已渲染的旧数据：采集 12 个账号的官方用量要几秒，
      // 把整页推回骨架屏等于让用户重新等一次，而这期间旧数据仍然可用。
      if (!background) setLoading(true);
      setError(null);
      setLoadFailure(null);
      try {
        await fetchAll();
        const accountState = useAccountsStore.getState();
        if (accountState.error) {
          throw new Error(accountState.error);
        }
        const currentAccounts = accountState.accounts;
        const ids = currentAccounts.map((account) => account.id);
        if (refresh && ids.length > 0) {
          await refreshCredits(ids);
        }
        // 共享模块负责 single-flight 与成功缓存：与启动预热共用同一份数据来源。
        setStats(await loadCreditStatistics(refresh));
      } catch (cause) {
        if (!background) {
          setError(api.asError(cause));
          setLoadFailure({ failure: api.asFailure(cause), accountId: null, accountName: null });
        }
      } finally {
        if (!background) setLoading(false);
      }
    },
    [fetchAll, refreshCredits],
  );

  /**
   * 统一取数入口：用户点选、首屏恢复、跨标签同步三条路径共用，避免各写一份
   * 「置 loading → 切窗口 → 30 天走缓存 / 其它窗口走接口」。
   *
   * 失败直接抛给调用方：点选要回滚选择，自动同步只能交给错误条（见各自的 catch）。
   *
   * 非 30 天窗口默认不刷账号列表（点选 / 跨标签同步时账号已由首屏载入）；
   * 首屏恢复与错误条重试传 `withAccounts`，先 `fetchAll` 再取数。
   */
  const loadWindow = useCallback(
    async (nextDataRange: CreditRange, options?: { withAccounts?: boolean }) => {
      const seq = ++loadSeqRef.current;
      setRangeLoading(true);
      setDataRange(nextDataRange);
      try {
        if (nextDataRange === "30d") {
          await load(false);
          return;
        }
        if (options?.withAccounts) {
          await fetchAll();
          const accountState = useAccountsStore.getState();
          if (accountState.error) throw new Error(accountState.error);
        }
        const nextStats = await api.getCreditStatistics(false, nextDataRange);
        // 期间又切了范围（seq 变化）或已卸载：本次结果作废，别覆盖新窗口的数字。
        if (loadSeqRef.current !== seq || unmountedRef.current) return;
        setStats(nextStats);
        setError(null);
        setLoadFailure(null);
      } finally {
        if (loadSeqRef.current === seq && !unmountedRef.current) {
          setRangeLoading(false);
          setLoading(false);
        }
      }
    },
    [fetchAll, load],
  );

  /**
   * 切范围：展示范围立即变；只有取数窗口也跟着变时才重新取一次数。
   * 今天 / 7 天 / 30 天共用 30 天窗口，所以它们之间切换是纯本地的（无请求、不闪烁）。
   */
  async function selectRange(next: StatsRangeKey) {
    if (next === range || rangeLoading) return;
    const previousRange = range;
    const previousDataRange = dataRange;
    const nextDataRange = creditDataRange(next, supports90d);
    setRange(next);
    if (nextDataRange === previousDataRange) return;
    try {
      await loadWindow(nextDataRange);
    } catch (cause) {
      // 失败就退回上一个范围与窗口：让「选中的范围」与「页面数字」始终对应，不留半截状态。
      setRange(previousRange);
      setDataRange(previousDataRange);
      setRangeLoading(false);
      toast.error("切换展示范围失败", { description: api.asError(cause) });
    }
  }

  /** 错误条上的「重试」：跟随当前取数窗口，不会把 90 天/账本悄悄换回 30 天。 */
  async function reloadCurrentRange() {
    try {
      // 重试可能是在接首屏账号拉取失败，所以连账号一起刷。
      await loadWindow(dataRange, { withAccounts: true });
    } catch (cause) {
      setError(api.asError(cause));
      setLoadFailure({ failure: api.asFailure(cause), accountId: null, accountName: null });
    }
  }

  useEffect(() => {
    // 自动补采只覆盖 30 天窗口（`loadCreditStatistics` 的缓存口径）：其它窗口由用户主动刷新，
    // 否则会用 30 天结果覆盖 90 天/账本的页面数字。
    if (dataRange !== "30d") return;
    void (async () => {
      // 先渲染本地缓存（后端只读磁盘，不用等网络；预热命中时这里不发请求）
      await load(false);
      if (api.isDemoMode()) return;
      // 过期只看后端记录的采集时刻：会话内变量每次启动都归零，判断不出
      // 「缓存其实是几小时前采的」，所以刚打开应用时不会自动刷新。
      // 启动预热已在同一门控下补采过并回填缓存，这里读到的是新鲜时刻，不会二次采集。
      const collectedAt = readCreditStatisticsCache()?.officialUsage?.collectedAt ?? 0;
      if (collectedAt > 0 && Date.now() - collectedAt < STATISTICS_AUTO_REFRESH_MS) return;
      if (useAccountsStore.getState().accounts.length === 0) return;
      // 后台补采：不清空、不挡交互，采完再替换数字。
      await load(true, { background: true });
    })();
  }, [load, dataRange]);

  // 首次进入页面时若从 localStorage 恢复为非 30 天窗口（90d / all），主动读对应窗口数据。
  useEffect(() => {
    const target = initialRangeRef.current;
    if (!target) return;
    // 先置空再取数：StrictMode 的二次挂载 / effect 重跑都不会重复请求。
    initialRangeRef.current = null;
    void (async () => {
      try {
        // 非 30 天窗口不经过 load()，首屏需要在这里先载入账号列表。
        await loadWindow(target, { withAccounts: true });
      } catch (cause) {
        if (!unmountedRef.current) {
          setError(api.asError(cause));
          setLoadFailure({ failure: api.asFailure(cause), accountId: null, accountName: null });
        }
      }
    })();
  }, [loadWindow]);

  // 跨标签页同步 range 或探测返回 supports90d=true 导致所需窗口变化时，补齐取数窗口。
  useEffect(() => {
    const expected = creditDataRange(range, supports90d);
    if (expected === dataRange || rangeLoading || loading) return;
    const syncKey = `${range}:${expected}`;
    if (syncedWindowKeyRef.current === syncKey) return;
    syncedWindowKeyRef.current = syncKey;
    void (async () => {
      try {
        await loadWindow(expected);
      } catch (cause) {
        if (unmountedRef.current) return;
        // 这里不回滚 dataRange：range 是被另一处（另一个标签页 / 窗口探测）改掉的，
        // 回滚会留下「按钮高亮 90 天、数字还是 30 天、再点同一个按钮又被 next === range 挡住」
        // 的死状态。改为交给页面错误条 + 「重试」（reloadCurrentRange 跟随当前 dataRange）。
        setError(api.asError(cause));
        setLoadFailure({ failure: api.asFailure(cause), accountId: null, accountName: null });
        // 清掉去重键，后续切范围或探测更新时允许重试同一个窗口。
        syncedWindowKeyRef.current = null;
      }
    })();
  }, [range, supports90d, dataRange, rangeLoading, loading, loadWindow]);

  const variantByAccountId = useMemo(() => {
    const map = new Map<string, WbVariant>();
    for (const account of accounts) {
      map.set(account.id, accountVariant(account));
    }
    return map;
  }, [accounts]);

  const variantAccountIds = useMemo(() => {
    if (!stats || viewVariant === "all") return null;
    return new Set(
      stats.accounts
        .filter((account) => resolveAccountVariant(account, variantByAccountId) === viewVariant)
        .map((account) => account.accountId),
    );
  }, [stats, viewVariant, variantByAccountId]);

  const filteredStats = useMemo(() => {
    // D1: viewVariant === "all" 时直接沿用后端聚合值（stats.summary / stats.daily），
    // 不得走过滤重算路径，避免把国内版与国际版不可比积分加回一起，破坏改动前回归基线。
    if (viewVariant === "all" || !stats || !variantAccountIds) return stats;
    return recomputeLocalStats(stats, variantAccountIds, viewVariant, variantByAccountId);
  }, [stats, viewVariant, variantAccountIds, variantByAccountId]);

  const filteredOfficial = useMemo(() => {
    const officialUsage = stats?.officialUsage;
    // D1: 「全部」原样使用 officialUsage.summary / daily / models，不走过滤重算。
    if (viewVariant === "all" || !officialUsage || !variantAccountIds) return officialUsage;
    return recomputeOfficialUsage(officialUsage, variantAccountIds);
  }, [stats, viewVariant, variantAccountIds]);

  // 客户端维度只能在明细上做（官方没有按客户端聚合）。
  const enabledTools = useSupportedTools();
  const clientOptions = useMemo(() => {
    const options =
      dataRange === "all"
        ? ledgerClientViewOptions(stats?.ledger)
        : clientViewOptions(stats?.officialUsage?.requests);
    // 设置页关掉的客户端不再列出：与关联会话 / Token 统计同一条规则（未知项始终保留）。
    return options.filter((option) => {
      const tool = CLIENT_TOOL[option.key];
      return tool === undefined || toolEntryEnabled(enabledTools, tool);
    });
  }, [dataRange, stats, enabledTools]);
  useEffect(() => {
    // 明细里没有该客户端（换账号、后端换了明细范围）→ 回到「全部」，不留一个空筛选项。
    if (clientView === "all") return;
    if (!clientOptions.some((option) => option.key === clientView)) setClientView("all");
  }, [clientOptions, clientView, setClientView]);
  const clientDetail = useMemo(() => {
    if (clientView === "all" || !filteredOfficial) return null;
    return filterOfficialByClient(filteredOfficial, clientView, variantAccountIds);
  }, [clientView, filteredOfficial, variantAccountIds]);

  // 选「全部」走账本；账本不可用/为空时降级回官方 30 天并在页面上说明。
  const ledgerOfficial = useMemo(
    () => (dataRange === "all" ? ledgerToOfficialUsage(stats?.ledger, clientView) : null),
    [dataRange, stats, clientView],
  );
  const ledgerFallback = dataRange === "all" && !ledgerOfficial;
  /** 当前口径的官方数据：全部=账本 / 具体客户端=据明细汇总 / 否则=官方聚合。 */
  const officialUsage = ledgerFallback
    ? filteredOfficial
    : ledgerOfficial ?? clientDetail?.official ?? filteredOfficial;
  const official = isOfficialUsageAvailable(officialUsage) ? officialUsage : undefined;

  /** 「全部」时的账号级累计（账本按账号只给 credit，没有逐日）。 */
  const ledgerAccountTotals = useMemo(() => {
    const entries = stats?.ledger?.accounts ?? [];
    if (entries.length === 0) return undefined;
    return new Map(entries.map((account) => [account.accountId, account.credit]));
  }, [stats]);

  /** 账号消耗构成的行：第一屏结论与折叠区条形图共用同一份。 */
  const usageRows = useMemo(
    () =>
      filteredStats
        ? accountUsageRows(
            filteredStats,
            officialUsage,
            range,
            variantByAccountId,
            ledgerAccountTotals,
          )
        : [],
    [filteredStats, officialUsage, range, variantByAccountId, ledgerAccountTotals],
  );
  const headline = useMemo(() => topAccountShare(usageRows), [usageRows]);

  /** 按模型分类的范围聚合（折叠区标题与卡片内容共用）。 */
  const rangeModelRows = useMemo(
    () => (official ? rangeModels(official, range, null) : []),
    [official, range],
  );
  const modelHeadline = useMemo(() => {
    const total = rangeModelRows.reduce((sum, model) => sum + model.credit, 0);
    const top = rangeModelRows[0];
    return {
      count: rangeModelRows.length,
      label: top ? (top.model === "—" ? "未知模型" : top.model) : null,
      share: top && total > 0 ? top.credit / total : null,
    };
  }, [rangeModelRows]);

  /** 四卡：剩余（账号级）+ 本范围消耗 / 日均 / 单日峰值（同一份逐日数据）。 */
  const coverage = filteredStats?.coverage;
  const consumption = filteredStats
    ? statsRangeConsumption(range, official, filteredStats, coverage)
    : null;
  const rangeSeries = official?.daily ?? filteredStats?.daily ?? [];
  const rangeDays = statsRangeDayCount(range, rangeSeries.length);
  const rangePeak = filteredStats ? rangePeakUsage(rangeSeries, range) : null;
  const dailyAverage =
    consumption?.value == null ? null : Math.round(consumption.value / rangeDays);

  /** 导出：当前范围 + 档位 + 客户端筛选，文件名与提示都带范围标签。 */
  async function exportCsv() {
    if (!stats) {
      toast.error("还没有可导出的统计数据");
      return;
    }
    try {
      const content = buildCreditStatsCsv(stats, officialUsage ?? undefined);
      const outcome = await saveCsvFile(
        creditStatsFileName(`${viewVariant}-${rangeFileLabel(range)}`),
        content,
      );
      if (outcome === "saved") {
        toast.success("已导出积分统计 CSV", {
          description: `${viewVariant} · ${statsRangeLabel(range)} · ${(content.length / 1024).toFixed(0)} KB`,
        });
      }
    } catch (cause) {
      toast.error("导出失败", { description: api.asError(cause) });
    }
  }

  /** 导出 Excel：与 CSV 同一份口径，多 sheet、数字可求和、每个 sheet 末行写说明。 */
  async function exportXlsx() {
    if (!stats) {
      toast.error("还没有可导出的统计数据");
      return;
    }
    try {
      const bytes = buildCreditStatsXlsx(stats, officialUsage ?? undefined, {
        range: dataRange,
        ledger: stats.ledger,
        clientLabel: clientView === "all" ? undefined : CLIENT_VIEW_LABELS[clientView],
        caliberNote: ledgerCaliber ?? undefined,
        windowLabel: statsRangeLabel(range),
      });
      const outcome = await saveXlsxFile(
        creditStatsXlsxFileName(`${viewVariant}-${rangeFileLabel(range)}`),
        bytes,
      );
      if (outcome === "saved") {
        toast.success("已导出积分统计 Excel", {
          description: `${viewVariant} · ${statsRangeLabel(range)} · ${(bytes.length / 1024).toFixed(0)} KB · 7 个 sheet`,
        });
      }
    } catch (cause) {
      toast.error("导出失败", { description: api.asError(cause) });
    }
  }

  /** 顶部口径说明：账本优先，其次明细汇总，最后官方/本地。 */
  const ledgerCaliber =
    dataRange === "all"
      ? ledgerOfficial
        ? `本地账本：${stats?.ledger?.startAt ?? "—"} ~ ${stats?.ledger?.endAt ?? "—"} · ${
            stats?.ledger?.rows ?? 0
          } 条（自本工具首次抓取起累计）`
        : "本地账本暂无数据，暂用官方近 30 天口径"
      : null;
  const rangeFallbackText =
    stats?.rangeFallback === true
      ? `官方接口只接受近 ${stats?.windowProbe?.maxDays ?? 30} 天窗口，已按近 30 天统计`
      : null;
  // 「本月」在只支持 30 天窗口时退化成 30 天切片，必须如实标注。
  const monthNote = monthSliceNote(range, supports90d);
  const variantEmpty =
    viewVariant !== "all" && variantAccountIds !== null && variantAccountIds.size === 0;

  const coverageHint = consumption?.absenceReason ?? null;
  const officialRangeText = official ? `${official.rangeStart} ~ ${official.rangeEnd}` : "";
  // 失败原因取**未过滤**的官方响应：档位过滤会按 accountId 收紧 `errors`，
  // 而归一化出来的「整页不可用」没有任何账号行，过滤后会把原因一起丢掉。
  const failureSource = stats?.officialUsage ?? officialUsage;
  const officialFailureText = (failureSource?.errors ?? [])
    .map((item) => `${item.accountName}: ${item.error}`)
    .join("；");
  /**
   * 失败动作走**数据通道**：`errors[]` 里的档位。桌面端 `get_credit_expiry` /
   * `get_credit_statistics` 失败时也是正常 resolve（`catch` 不执行），所以动作不能
   * 挂在 catch 上。没有档位时 `null` → 动作行整块不渲染，这个 Alert 与改造前逐像素相同。
   */
  const officialFailure = useMemo(() => findCreditFailure(failureSource?.errors), [failureSource]);

  // 明细已取到但总览仍回退本地时额外提示，避免「明细有数字、总览是另一个数」的自相矛盾。
  const detailHasData =
    !official &&
    (officialUsage?.requests.length ?? 0) > 0 &&
    (officialUsage?.accounts ?? []).some((account) => account.ok && account.requestCount > 0);

  const rangeText = statsRangeLabel(range);
  const accountUsageTitle = headline
    ? `账号消耗构成 · 头部 ${headline.label} 占 ${formatPercent(headline.share)}（${formatAmount(headline.usage)} 积分）`
    : "账号消耗构成 · 当前范围没有观察到积分消耗";
  const modelTitle =
    official && modelHeadline.count > 0
      ? `按模型分类 · ${modelHeadline.count} 个模型 · 头部 ${modelHeadline.label ?? "—"} 占 ${formatPercent(modelHeadline.share)}`
      : "按模型分类 · 官方暂无模型消耗明细";
  const resourceTitle = `积分明细 · 剩余 ${formatAmount(filteredStats?.summary.currentRemaining ?? null)} 积分 · ${
    filteredStats?.accounts.length ?? 0
  } 个账号`;

  return (
    <div className="mx-auto w-full max-w-[1180px] min-w-0 px-4 py-6 sm:px-8 sm:py-9">
      <header className="mb-6 flex min-w-0 flex-wrap items-start justify-between gap-4">
        <div className="min-w-0">
          <h1 className="text-[28px] font-semibold tracking-tight">积分统计</h1>
          <p className="mt-2 max-w-2xl text-sm leading-6 text-muted-foreground">
            当前数据更新于 {stats ? formatDateTime(official?.collectedAt ?? stats.generatedAt) : "—"}
          </p>
          <p className="mt-1 max-w-2xl text-[11px] leading-5 text-muted-foreground">
            积分按账号汇总（国内版与国际版分开计算）；「客户端」筛选在官方明细上做，
            选中具体客户端时总览改「据明细汇总」。官方用量接口只提供近 30 天，
            逐请求明细每账号上限 1000 条（聚合不受此限制）。
          </p>
          {stats ? (
            <p className="mt-0.5 max-w-2xl text-[11px] leading-5 text-muted-foreground">
            {ledgerCaliber ??
              (clientDetail
                ? `据明细汇总（${CLIENT_VIEW_LABELS[clientView]}）· ${clientDetail.records} 条 · ${
                    clientDetail.truncated ? "部分截断（数字为下限）" : "未截断"
                  }${rangeFallbackText ? ` · ${rangeFallbackText}` : ""}`
                : official
                  ? `官方口径：${officialRangeText} · 明细上限 ${official.detailLimitPerAccount} 条/账号${
                      rangeFallbackText ? ` · ${rangeFallbackText}` : ""
                    }`
                  : `本地观察口径：${localCoverageText(coverage)}`)}
            </p>
          ) : null}
          {monthNote ? (
            <p className="mt-0.5 max-w-2xl text-[11px] leading-5 text-muted-foreground">{monthNote}</p>
          ) : null}

          {progressive.running || progressive.timing.officialMs !== null ? (
            <p className="mt-1 flex flex-wrap gap-x-3 text-[11px] text-muted-foreground">
              <span>本地 {formatMs(progressive.timing.localMs)}</span>
              <span>积分 {formatMs(progressive.timing.creditsMs)}</span>
              <span>官方 {formatMs(progressive.timing.officialMs)}</span>
              {progressive.phase.local ? <span className="text-success">本地已更新</span> : null}
            </p>
          ) : null}
        </div>
        <div className="flex min-w-0 max-w-full flex-wrap items-center justify-end gap-2">
          <StatsRangeControl
            value={range}
            include90d={include90d}
            onChange={(next) => void selectRange(next)}
            disabled={rangeLoading}
            ariaLabel="统计范围"
            hints={{
              all: "本地账本：从本工具首次抓取官方明细起累计",
              month: supports90d ? undefined : "官方窗口只支持 30 天，本月按近 30 天数据统计",
            }}
          />
          <div
            className="flex max-w-full flex-wrap gap-1 rounded-lg bg-muted p-1"
            role="tablist"
            aria-label="档位筛选"
          >
            {VARIANT_VIEW_OPTIONS.map((option) => (
              <button
                key={option.key}
                type="button"
                role="tab"
                aria-selected={viewVariant === option.key}
                className={`cursor-pointer rounded-md px-2.5 py-1.5 text-xs transition-colors ${
                  viewVariant === option.key
                    ? "bg-background font-medium text-foreground shadow-sm"
                    : "text-muted-foreground hover:text-foreground"
                }`}
                onClick={() => setViewVariant(option.key)}
              >
                {option.label}
              </button>
            ))}
          </div>
          <ClientFilterMenu
            options={clientOptions}
            value={clientView}
            onChange={setClientView}
            counts={stats?.officialUsage?.requests.length ?? 0}
          />
          <ExportMenu
            disabled={!stats}
            title={
              stats
                ? `导出当前范围（${rangeText}）+ 档位 + 客户端筛选`
                : "没有可导出的数据"
            }
            items={[
              {
                key: "csv",
                label: "CSV",
                description: "6 段明细 · UTF-8 BOM · Excel 可直接打开",
                icon: <Download className="size-3.5" />,
                onSelect: () => void exportCsv(),
              },
              {
                key: "xlsx",
                label: "Excel（xlsx）",
                description: "7 个 sheet · 数值可求和 · 末行写口径说明",
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
              onClick={() => void progressive.run()}
              disabled={loading || progressive.running}
            >
              {progressive.running ? <Loader2 className="animate-spin" /> : <RefreshCw />}
              {progressive.running ? `更新中 ${progressive.done}/2 项` : "刷新统计"}
            </Button>
          </DemoAction>
        </div>
      </header>

      {error && (
        <Alert variant="destructive" className="mb-5">
          <CircleAlert />
          <AlertTitle>统计加载失败</AlertTitle>
          <AlertDescription className="flex flex-wrap items-center gap-3">
            <span>{error}</span>
            {/* 有档位、且动作有落点才渲染（否则组件返回 null，不留空位）。 */}
            <FailureActions
              {...creditFailureActionsProps(loadFailure, accounts, () => setOauthOpen(true))}
            />
            <Button size="sm" variant="outline" onClick={() => void reloadCurrentRange()}>
              重试
            </Button>
          </AlertDescription>
        </Alert>
      )}

      {loading && !stats ? (
        <div className="flex items-center gap-2 py-20 text-sm text-muted-foreground">
          <Loader2 className="animate-spin" />
          正在采集账号积分并加载统计…
        </div>
      ) : stats && filteredStats ? (
        <div className="min-w-0 space-y-4">
          {viewVariant === "all" && accounts.length === 0 && (
            <Alert>
              <CircleAlert />
              <AlertTitle>暂无当前账号</AlertTitle>
              <AlertDescription>可以先去账号管理导入或登录账号；历史事件仍会保留。</AlertDescription>
            </Alert>
          )}

          {variantEmpty ? (
            <div className="rounded-xl border border-dashed px-4 py-16 text-center text-sm text-muted-foreground">
              <p>暂无{statsVariantViewLabel(viewVariant)}账号的积分数据。</p>
              <p className="mt-2 text-xs leading-5">国内版与国际版积分体系不同，不会合并计算。</p>
            </div>
          ) : (
            <>
              {!official && (
                <Alert variant="warning">
                  <CircleAlert />
                  <AlertTitle>官方用量暂不可用，当前为本地观察数据</AlertTitle>
                  <AlertDescription className="flex flex-col gap-1">
                    <span>
                      本地观察覆盖：{localCoverageText(coverage)}。今日值为本地快照差值；
                      近 7 天/本月在本地覆盖不足时显示「—」，不用今日值代替。
                      失败原因已写入 error.log，官方接口恢复后点「刷新统计」即可重新同步。
                    </span>
                    {detailHasData && (
                      <span className="text-xs">
                        明细已取到部分官方请求，但总览暂用本地口径；明细与总览的差异来自这里。
                      </span>
                    )}
                    {officialFailureText && (
                      <span className="text-xs text-warning-foreground">{officialFailureText}</span>
                    )}
                    {/* 有档位、且动作有落点才渲染（否则组件返回 null，不留空位）。 */}
                    <FailureActions
                      {...creditFailureActionsProps(officialFailure, accounts, () =>
                        setOauthOpen(true),
                      )}
                    />
                  </AlertDescription>
                </Alert>
              )}

              {official && official.status === "partial" && (
                <Alert variant="warning">
                  <CircleAlert />
                  <AlertTitle>部分账号官方用量未同步（partial）</AlertTitle>
                  <AlertDescription className="flex flex-col gap-1">
                    <span>
                      已同步 {official.accounts.filter((account) => account.ok).length}/
                      {official.accounts.length} 个当前账号；失败账号的官方数值显示为「—」，
                      总览为已同步账号之和，不代表全部账号。
                    </span>
                    {officialFailureText && (
                      <span className="text-xs text-warning-foreground">{officialFailureText}</span>
                    )}
                    {/*
                      partial（部分账号失败）才是最常见的现场：动作行同样挂在数据通道上。
                      `error` 非空时**不渲染动作行**：刷新抛错后红条「统计加载失败」与这条
                      黄条会同屏（stats 保留旧值），两条各带一行动作按钮会让用户不知道点哪个。
                      只掐动作行，黄条本身（「部分账号未同步」的信息）仍然保留。
                    */}
                    {error
                      ? null
                      : (
                          <FailureActions
                            {...creditFailureActionsProps(officialFailure, accounts, () =>
                              setOauthOpen(true),
                            )}
                          />
                        )}
                  </AlertDescription>
                </Alert>
              )}

              <Card className="min-w-0 gap-0 overflow-hidden rounded-2xl bg-card/70 py-0 shadow-none" aria-label="积分总览">
                <div className="flex min-w-0 flex-wrap items-center justify-between gap-x-3 gap-y-1 border-b border-border/60 px-4 py-2 text-[11px] leading-5 text-muted-foreground sm:px-5">
                  <span className="min-w-0">
                    {dataRange === "all" && ledgerOfficial
                      ? `本地账本 · ${stats?.ledger?.rows ?? 0} 条 · ${
                          ledgerFallback ? "降级" : "累计"
                        }（剩余积分为账号级）`
                      : clientDetail
                        ? `据明细汇总 · ${clientDetail.records} 条 · ${
                            clientDetail.truncated ? "部分截断" : "未截断"
                          }（剩余积分为账号级）`
                        : `${creditSourceBadge(official)}：${
                            official
                              ? `${officialRangeText}（官方仅提供近 30 天）`
                              : localCoverageText(coverage)
                          }`}
                  </span>
                  <span className="min-w-0">
                    {coverageHint ?? "明细每账号上限 1000 条，聚合与趋势不受限"}
                  </span>
                </div>
                <CardContent className="grid min-w-0 grid-cols-1 divide-y divide-border/60 p-0 sm:grid-cols-4 sm:divide-y-0 sm:py-5">
                  <StatMetric
                    icon={Sparkles}
                    label="剩余积分"
                    value={formatAmount(filteredStats.summary.currentRemaining)}
                  />
                  <StatMetric
                    icon={TrendingDown}
                    label={`${rangeText}消耗`}
                    value={consumption?.text ?? "—"}
                    divided
                  />
                  <StatMetric
                    icon={CalendarRange}
                    label="日均消耗"
                    value={dailyAverage === null ? "—" : formatAmount(dailyAverage)}
                    divided
                  />
                  <StatMetric
                    icon={CalendarDays}
                    label="单日峰值"
                    value={rangePeak === null ? "—" : formatAmount(rangePeak)}
                    divided
                  />
                </CardContent>
              </Card>

              {/* 结论先行：一行说清「谁最吃积分」，与下方折叠区同一份数字 */}
              <p className="min-w-0 px-1 text-sm leading-6 text-muted-foreground">
                {headline ? (
                  <>
                    {rangeText}消耗最多的是{" "}
                    <span className="font-medium text-foreground">{headline.label}</span>，占{" "}
                    <span className="font-medium tabular-nums text-foreground">
                      {formatPercent(headline.share)}
                    </span>
                    （{formatAmount(headline.usage)} / {formatAmount(headline.total)} 积分，
                    共 {headline.counted} 个账号有消耗）
                  </>
                ) : (
                  <>{rangeText}内没有观察到积分消耗。</>
                )}
              </p>

              {!official && !filteredStats.coverageStartAt && filteredStats.events.some((event) => event.kind === "checkin") && (
                <Alert>
                  <CircleCheck />
                  <AlertTitle>目前只有签到记录</AlertTitle>
                  <AlertDescription>签到不会被计入积分消耗。首次成功采集积分资源后，趋势统计才会开始累计。</AlertDescription>
                </Alert>
              )}

              <TrendChart
                stats={filteredStats}
                officialUsage={officialUsage}
                range={range}
              />

              {/* 次要从属折叠：默认全部展开，保持与旧版一致的完整信息密度 */}
              <Accordion
                type="multiple"
                defaultValue={["account-usage", "models", "resources"]}
                className="min-w-0"
              >
                <AccordionItem value="account-usage" className="min-w-0 border-t border-border/60">
                  <AccordionTrigger className="px-1 py-2.5 text-[13px] font-medium leading-5 text-foreground">
                    <span className="min-w-0 truncate text-left" title={accountUsageTitle}>
                      {accountUsageTitle}
                    </span>
                  </AccordionTrigger>
                  <AccordionContent className="pb-3 pt-1">
                    <AccountUsageShare
                      stats={filteredStats}
                      officialUsage={officialUsage}
                      variantByAccountId={variantByAccountId}
                      ledger={Boolean(ledgerOfficial)}
                      range={range}
                      allTimeByAccountId={ledgerAccountTotals}
                      heading={false}
                    />
                  </AccordionContent>
                </AccordionItem>

                {official && (
                  <AccordionItem value="models" className="min-w-0 border-t border-border/60">
                    <AccordionTrigger className="px-1 py-2.5 text-[13px] font-medium leading-5 text-foreground">
                      <span className="min-w-0 truncate text-left" title={modelTitle}>
                        {modelTitle}
                      </span>
                    </AccordionTrigger>
                    <AccordionContent className="pb-3 pt-1">
                      <ModelBreakdown officialUsage={official} range={range} heading={false} />
                    </AccordionContent>
                  </AccordionItem>
                )}

                <AccordionItem value="resources" className="min-w-0 border-t border-border/60">
                  <AccordionTrigger className="px-1 py-2.5 text-[13px] font-medium leading-5 text-foreground">
                    <span className="min-w-0 truncate text-left" title={resourceTitle}>
                      {resourceTitle}
                    </span>
                  </AccordionTrigger>
                  <AccordionContent className="pb-3 pt-1">
                    <SelectedAccountDetails
                      stats={filteredStats}
                      officialUsage={officialUsage}
                      creditMap={creditMap}
                      creditLoadingMap={creditLoadingMap}
                      heading={false}
                      range={range}
                    />
                  </AccordionContent>
                </AccordionItem>
              </Accordion>
            </>
          )}
        </div>
      ) : (
        <div className="rounded-xl border border-dashed px-4 py-16 text-center text-sm text-muted-foreground">
          暂无统计数据，请点击刷新重试。
        </div>
      )}

      {/* 失败动作里的「重新登录」复用现有 OAuth 弹窗；拿不到账号上下文时按钮本就不渲染。 */}
      <OAuthLoginDialog
        open={oauthOpen}
        onOpenChange={setOauthOpen}
        variant={reloginVariantOf(reloginAccountOf(loadFailure, accounts))}
      />
    </div>
  );
}
