

import { toolEntryEnabled, type ToolId } from "@/lib/supported-tools";
import type {
  SessionGroupClient,
  SessionGroupSummary,
} from "@/lib/types";

export const PAGE_SIZE = 8;

/** 列表不超过该数量时隐藏底部分页条：一页装得下，去掉更清爽。 */
export const PAGINATION_HIDE_MAX = 4;

/**
 * 按项目分组时每页的**节**数（不是会话数）。
 *
 * 分组后的分页语义选的是「按节分页，节内条目全给」，理由：
 * - 节头文案是 `项目 X · N 个会话`，N 必须是该节**全部**会话数。若改成「按条目分页」，
 *   同一个项目会被切到两页上，节头只能显示「本页 N 个」，用户会以为这个项目只有几条；
 * - 一页 4 节在 1440 宽的 2 列网格里正好是 8 张卡（≈ 原 `PAGE_SIZE`），首屏高度不失控；
 * - 折叠起来以后一屏能看到多个项目，比「一个项目占满整页」更接近「按项目收拢」的目的。
 */
export const PROJECT_SECTIONS_PER_PAGE = 4;

export type SessionClientEntry = { id: SessionGroupClient; title: string; description: string };

export const CLIENTS: SessionClientEntry[] = [
  { id: "workbuddy", title: "WorkBuddy", description: "含国内版与国际版会话" },
  { id: "vscodeExt", title: "CodeBuddy 插件", description: "VS Code 扩展会话" },
  { id: "codebuddyIde", title: "CodeBuddy IDE", description: "国内版 / 国际版独立分组" },
  { id: "jetbrains", title: "JetBrains 插件", description: "IDEA / PyCharm 共用" },
];

/**
 * 页头可切换的客户端：与设置页「支持工具」开关同步。
 *
 * 关掉某工具就等于「它不存在」——这里不能再列出它的标签页，否则用户会看到一个
 * 已声明的关闭项，还会为它拉一遍会话组（大库上一次要几十秒）。全部关闭时返回空数组，
 * 由页面显示「未启用任何客户端」的空态，而不是硬塞一个默认客户端。
 */
export function visibleClients(
  enabledTools: Partial<Record<ToolId, boolean>> | undefined,
): SessionClientEntry[] {
  return CLIENTS.filter((item) => toolEntryEnabled(enabledTools, item.id));
}

type GroupListSnapshot = {
  groups: SessionGroupSummary[];
  storeStatus: "missing" | "ready" | "unavailable";
  storeError: string | null;
  /** 被隐藏的失效组数量（列表为空时前端仍要能显示清理入口）。 */
  deadGroupCount: number;
  /** 清理被阻塞的原因（`null` = 可清理）：非空时入口禁用并展示原因。 */
  pruneBlocked: string | null;
};

/**
 * 列表页的进程内缓存，键为 `client:scope`。
 *
 * 首屏要读数百份会话正文并逐行摘要（后端已并行，仍要数秒）。切到别的页面再切回来
 * 时不该重新等一遍骨架屏：先用上次结果渲染，同时后台刷新替换。
 */
export const groupListCache = new Map<string, GroupListSnapshot>();

/** 会话树区最小高度：加载态、空态与列表共用同一下沿，避免弹窗高度跳变。 */
export const LINKED_TREE_MIN_H = "min-h-[min(10rem,30vh)]";
