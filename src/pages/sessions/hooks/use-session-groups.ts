import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import * as api from "@/lib/api";
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
  reloadGroups: () => Promise<void>;
}

/**
 * 关联会话列表：加载、缓存快照、状态筛选、搜索、排序、分组与分页。
 *
 * 竞态守卫沿用原来的两道：listRequestId 自增 + 当前 client:scope 上下文键，
 * 切换客户端/档位后旧请求的结果直接丢弃，避免把上一个客户端的数据写入当前列表。
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
  const contextRef = useRef({ client, scope });
  contextRef.current = { client, scope };
  const listContextKey = `${client}:${scope ?? "all"}`;
  const currentContextKey = () => `${contextRef.current.client}:${contextRef.current.scope ?? "all"}`;

  const reloadGroups = useCallback(async () => {
    const key = `${client}:${scope ?? "all"}`;
    const requestId = ++listRequestId.current;
    // 已有快照时不再退回骨架屏：刷新是「替换数据」，不是「重新加载页面」。
    if (!groupListCache.has(key)) setLoading(true);
    // 刷新标记**立刻**置上（不等响应）：这是「点了刷新马上有反馈」的唯一来源。
    setRefreshing(true);
    setLoadError(null);
    setStoreError(null);
    try {
      const result = await api.listSessionGroups(client, scope);
      if (currentContextKey() !== key || listRequestId.current !== requestId) return;
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
      if (currentContextKey() !== key || listRequestId.current !== requestId) return;
      setGroups([]);
      setLoadError(api.asError(error));
      groupListCache.delete(key);
    } finally {
      if (currentContextKey() === key && listRequestId.current === requestId) {
        setLoading(false);
        setRefreshing(false);
      }
    }
  }, [client, scope]);

  useEffect(() => {
    let live = true;
    const requestId = ++listRequestId.current;
    const key = listContextKey;
    const cached = groupListCache.get(key);
    setLoading(!cached);
    // 挂载/切客户端后也有一个在途请求：即使有快照也要顶住「刷新」按钮，避免重复请求。
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
