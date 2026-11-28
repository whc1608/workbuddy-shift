import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import * as api from "@/lib/api";
import { createInFlightGuard } from "@/lib/in-flight";
import type { InFlightGuard } from "@/lib/in-flight";
import {
  usePersistedPreferenceState,
  type PersistedPreferenceOptions,
} from "@/lib/persisted-preference";
import type { SessionGroupClient, SessionGroupSummary, WbVariant } from "@/lib/types";

import { groupListCache } from "../constants";
import {
  filterAndSortGroups,
  groupByProject,
  paginateList,
  type SessionGroupMode,
  type SessionGroupSection,
} from "../list-view";

// 类型原来定义在这里，现挪到纯函数模块 `../list-view` 以便单测；这里转出以保持既有 import 路径可用。
export type { SessionGroupMode, SessionGroupSection };

/** 状态筛选的合法取值（与页面的 `statusOptions` 一致）。 */
const STATUS_FILTER_VALUES = ["all", "behind", "diverge", "latest", "missing", "unknown"];

/**
 * 列表视图偏好：状态筛选、排序、分组模式跟随用户上次的选择。
 *
 * 只持久化「看的方式」：搜索词不存（属一次性意图），页码也不存（数据变化后会落到
 * 一个超出范围的空页）。`parse` 严格校验取值，认不出的旧值回默认。
 */
const STATUS_FILTER_PREFERENCE: PersistedPreferenceOptions<string> = {
  key: "workbuddy-shift.sessions.status-filter",
  fallback: "all",
  parse: (raw) => (STATUS_FILTER_VALUES.includes(raw) ? raw : null),
  serialize: (value) => value,
  eventName: "workbuddy-shift:sessions-status-filter-changed",
};

const SORT_ORDER_PREFERENCE: PersistedPreferenceOptions<string> = {
  key: "workbuddy-shift.sessions.sort",
  fallback: "recent",
  parse: (raw) => (raw === "recent" || raw === "oldest" ? raw : null),
  serialize: (value) => value,
  eventName: "workbuddy-shift:sessions-sort-changed",
};

const GROUP_MODE_PREFERENCE: PersistedPreferenceOptions<SessionGroupMode> = {
  key: "workbuddy-shift.sessions.group-mode",
  fallback: "none",
  parse: (raw) => (raw === "none" || raw === "project" ? raw : null),
  serialize: (value) => value,
  eventName: "workbuddy-shift:sessions-group-mode-changed",
};

/** 列表缓存的键：与「当前上下文」比较，用来丢弃切换客户端 / 档位后的过期回包。 */
function contextKeyOf(context: { client: SessionGroupClient; scope: WbVariant | undefined }): string {
  return `${context.client}:${context.scope ?? "all"}`;
}

export interface ReloadGroupsOptions {
  /**
   * 动作完成后强制重拉（默认 `false`）。
   *
   * `force: true` 时若有在途请求，**等它结束再发一次**，不复用旧结果 —— 复制 / 同步 / 清理
   * 之后的 onDone 必须看到动作之后的数据。默认语义是「同一时刻最多一个列表请求」：
   * 重复调用直接返回，不再发一次（按钮 `disabled` 只是 UI 层的第二道保险）。
   */
  force?: boolean;
}

export interface SessionGroupsController {
  groups: SessionGroupSummary[];
  /** 被列表隐藏的失效组数量：> 0 时仍要显示「清理失效关联组」入口。 */
  deadGroupCount: number;
  /**
   * 清理被阻塞的原因（`null` = 可清理）。
   *
   * 后端给的「点了必失败」的原因：入口要禁用并展示它，`requestPrune` 也要在发请求前拦下。
   */
  pruneBlocked: string | null;
  storeStatus: "missing" | "ready" | "unavailable";
  storeError: string | null;
  /** 首次加载（无快照）：页面显示骨架屏。 */
  loading: boolean;
  /**
   * 刷新中（含「已有快照、后台重新拉取」这一路）。
   *
   * 与 `loading` 分开：有快照时刷新不能退回骨架屏，但按钮必须立刻变成「刷新中…」并禁用，
   * 否则用户点了没有任何反馈、还会连点出多个请求（首屏一次 `list` 在大库上要几十秒）。
   */
  refreshing: boolean;
  /** 最近一次成功拉到列表的本地时刻（`null` = 还没有成功过）。页头据此显示「更新于 HH:MM」。 */
  updatedAt: number | null;
  loadError: string | null;
  statusFilter: string;
  setStatusFilter: (value: string) => void;
  sortOrder: string;
  setSortOrder: (value: string) => void;
  /** 搜索词：同时匹配标题、项目名与卡片摘要；与状态筛选、排序并存。 */
  query: string;
  setQuery: (value: string) => void;
  groupMode: SessionGroupMode;
  setGroupMode: (value: SessionGroupMode) => void;
  page: number;
  setPage: (value: number) => void;
  /** 状态筛选 + 搜索之后、分页之前的全部会话组（计数与排序口径都以它为准）。 */
  filteredGroups: SessionGroupSummary[];
  /** 按项目分组的全部节（折叠节的水位与节内条目都在这里）。 */
  sections: SessionGroupSection[];
  pageCount: number;
  currentPage: number;
  /** 当前页要渲染的会话组（分组模式下是当前页各节的组按节顺序展平）。 */
  visibleGroups: SessionGroupSummary[];
  /** 当前页要渲染的节（仅分组模式有内容）。 */
  visibleSections: SessionGroupSection[];
  /** 是否处于分组模式（决定页面渲染节还是渲染卡片网格）。 */
  grouped: boolean;
  /** 是否显示底部分页条：条目太少时一页装得下，去掉更清爽。 */
  showPagination: boolean;
  /**
   * 重新拉取列表。
   *
   * 有在途请求时默认直接返回（不重发）；动作链用 `{ force: true }` 排队重拉一次。
   */
  reloadGroups: (options?: ReloadGroupsOptions) => Promise<void>;
}

/**
 * 关联会话列表：加载、缓存快照、状态筛选、搜索、排序、分组与分页。
 *
 * 竞态守卫是两道，各管一件事：
 * - `in-flight` 守卫（`reloadGroups` 入口）防**重发**：同一时刻最多一个列表请求，绕过按钮
 *   连续调用也不会真的发两次；
 * - `listRequestId` 自增 + 当前 client:scope 上下文键防**脏写**：切换客户端/档位后旧请求的
 *   结果直接丢弃，避免把上一个客户端的数据写入当前列表。
 */
export function useSessionGroups(
  client: SessionGroupClient,
  scope: WbVariant | undefined,
): SessionGroupsController {
  const [groups, setGroups] = useState<SessionGroupSummary[]>([]);
  const [deadGroupCount, setDeadGroupCount] = useState(0);
  const [pruneBlocked, setPruneBlocked] = useState<string | null>(null);
  const [storeStatus, setStoreStatus] = useState<"missing" | "ready" | "unavailable">("missing");
  const [storeError, setStoreError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [refreshing, setRefreshing] = useState(false);
  const [updatedAt, setUpdatedAt] = useState<number | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [statusFilter, setStatusFilter] = usePersistedPreferenceState(STATUS_FILTER_PREFERENCE);
  const [sortOrder, setSortOrder] = usePersistedPreferenceState(SORT_ORDER_PREFERENCE);
  const [query, setQuery] = useState("");
  const [groupMode, setGroupMode] = usePersistedPreferenceState(GROUP_MODE_PREFERENCE);
  const [page, setPage] = useState(1);

  const listRequestId = useRef(0);
  /** 「是否允许发起」的唯一来源：防重发（`requestId` 防的是过期回包，两件事）。 */
  const guardRef = useRef<InFlightGuard | null>(null);
  /** 每次真正发起请求时自增：只让最后一轮复位 `refreshing`，排队重跑期间不能提前复位。 */
  const reloadRunId = useRef(0);
  const contextRef = useRef({ client, scope });
  contextRef.current = { client, scope };
  const listContextKey = contextKeyOf({ client, scope });
  const currentContextKey = () => contextKeyOf(contextRef.current);

  const reloadGroups = useCallback((options: ReloadGroupsOptions = {}): Promise<void> => {
    const guard = (guardRef.current ??= createInFlightGuard());
    /** 这次调用是否真的发起了请求（含它自己触发的排队重跑）。 */
    let started = false;
    let activeKey = "";
    let runId = 0;

    const task = async (): Promise<void> => {
      // 上下文在**执行时**读：排队重跑要拉的是「当前」客户端，而不是排队那一刻的。
      const context = contextRef.current;
      const key = contextKeyOf(context);
      const requestId = ++listRequestId.current;
      started = true;
      activeKey = key;
      runId = ++reloadRunId.current;
      // 已有快照时不再退回骨架屏：刷新是「替换数据」，不是「重新加载页面」。
      if (!groupListCache.has(key)) setLoading(true);
      // 刷新标记**立刻**置上（不等响应）：这是「点了刷新马上有反馈」的唯一来源。
      setRefreshing(true);
      setLoadError(null);
      setStoreError(null);
      try {
        const result = await api.listSessionGroups(context.client, context.scope);
        if (contextKeyOf(contextRef.current) !== key || listRequestId.current !== requestId) return;
        groupListCache.set(key, {
          groups: result.groups,
          storeStatus: result.storeStatus,
          storeError: result.storeError ?? null,
          deadGroupCount: result.deadGroupCount ?? 0,
          pruneBlocked: result.pruneBlocked ?? null,
        });
        setGroups(result.groups);
        setDeadGroupCount(result.deadGroupCount ?? 0);
        setPruneBlocked(result.pruneBlocked ?? null);
        setStoreStatus(result.storeStatus);
        setStoreError(result.storeError ?? null);
        setUpdatedAt(Date.now());
      } catch (error) {
        if (contextKeyOf(contextRef.current) !== key || listRequestId.current !== requestId) return;
        setGroups([]);
        setLoadError(api.asError(error));
        groupListCache.delete(key);
      }
    };

    // 空闲时立刻执行；在途时默认复用（不重发），`force` 排队等它结束再执行一次。
    return guard.run(task, { force: options.force === true }).finally(() => {
      // 只有真正发起过请求、且仍是最新一轮的调用才复位：
      // ① 复用在途请求的调用不能把别人的刷新态关掉；② 排队重跑期间 `refreshing` 必须保持 true。
      if (!started || runId !== reloadRunId.current) return;
      if (contextKeyOf(contextRef.current) !== activeKey) return;
      setLoading(false);
      setRefreshing(false);
    });
  }, []);

  useEffect(() => {
    let live = true;
    const requestId = ++listRequestId.current;
    const key = listContextKey;
    const cached = groupListCache.get(key);
    setLoading(!cached);
    // 挂载/切客户端后也有一个在途请求：即使有快照也要顶住「刷新」按钮，避免重复请求。
    // 这一个请求不并入 `reloadGroups` 的 in-flight 守卫：它的错误口径不同（有快照时保留画面，
    // 不清空列表、不删缓存）。两者意外并发时由 `listRequestId` 保证只有最后一次的结果会写进状态。
    setRefreshing(true);
    setLoadError(null);
    if (cached) {
      setGroups(cached.groups);
      setDeadGroupCount(cached.deadGroupCount);
      setPruneBlocked(cached.pruneBlocked);
      setStoreStatus(cached.storeStatus);
      setStoreError(cached.storeError);
    }
    api.listSessionGroups(client, scope).then((result) => {
      if (!live || listRequestId.current !== requestId || currentContextKey() !== key) return;
      groupListCache.set(key, {
        groups: result.groups,
        storeStatus: result.storeStatus,
        storeError: result.storeError ?? null,
        deadGroupCount: result.deadGroupCount ?? 0,
        pruneBlocked: result.pruneBlocked ?? null,
      });
      setGroups(result.groups);
      setDeadGroupCount(result.deadGroupCount ?? 0);
      setPruneBlocked(result.pruneBlocked ?? null);
      setStoreStatus(result.storeStatus);
      setStoreError(result.storeError ?? null);
      setUpdatedAt(Date.now());
    }).catch((error) => {
      if (!live || listRequestId.current !== requestId || currentContextKey() !== key) return;
      // 有旧快照时保留画面，把失败留给下一次刷新/操作提示，不把已有内容清空。
      if (cached) return;
      setGroups([]);
      setLoadError(api.asError(error));
    }).finally(() => {
      if (live && listRequestId.current === requestId && currentContextKey() === key) {
        setLoading(false);
        setRefreshing(false);
      }
    });
    return () => { live = false; };
  }, [client, scope, listContextKey]);

  // 状态筛选 + 搜索 + 排序：三个维度正交，纯函数见 `../list-view`。
  const filteredGroups = useMemo(
    () => filterAndSortGroups(groups, { statusFilter, sortOrder, query }),
    [groups, statusFilter, sortOrder, query],
  );

  const sections = useMemo(() => groupByProject(filteredGroups), [filteredGroups]);

  const grouped = groupMode === "project";
  // 分页口径见 `paginateList`：平铺按条目、分组按节（节内条目全给）。
  const { pageCount, currentPage, visibleGroups, visibleSections, showPagination } = paginateList(
    filteredGroups,
    sections,
    grouped,
    page,
  );

  // 筛选/搜索/分组/排序/客户端变化都回到第一页：留在第 3 页看空列表是最常见的困惑。
  useEffect(() => { setPage(1); }, [statusFilter, sortOrder, query, groupMode, client, scope]);

  return {
    groups,
    deadGroupCount,
    pruneBlocked,
    storeStatus,
    storeError,
    loading,
    refreshing,
    updatedAt,
    loadError,
    statusFilter,
    setStatusFilter,
    sortOrder,
    setSortOrder,
    query,
    setQuery,
    groupMode,
    setGroupMode,
    page,
    setPage,
    filteredGroups,
    sections,
    pageCount,
    currentPage,
    visibleGroups,
    visibleSections,
    grouped,
    showPagination,
    reloadGroups,
  };
}
