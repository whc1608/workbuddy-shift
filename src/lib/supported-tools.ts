import { useEffect, useState } from "react";

import {
  readPreference,
  subscribePreferenceChanges,
  writePreference,
  type PersistedPreferenceOptions,
} from "@/lib/persisted-preference";

/**
 * 「支持工具」开关：控制各客户端入口是否在界面上出现。
 *
 * 语义（与设置页文案一致）：
 * - 关闭某工具 = 该工具的入口（账号卡片按钮、页顶状态徽标）不渲染，相关状态
 *   轮询也跳过；**不影响账号库与其它工具**，重新打开即恢复。
 * - 默认值：现有的四个端默认开启；JetBrains 端默认关闭（作者要求的灰度策略）。
 *
 * 持久化在 localStorage（与 `workbuddy-shift.compact` / `workbuddy-shift.theme` 同风格），
 * 键名 `workbuddy-shift.tools.<id>`，值 `"1"` / `"0"`；缺省时取 `defaultEnabled`。
 */
export type ToolId = "workbuddy" | "workbuddyAi" | "codebuddyIde" | "codebuddyCli" | "vscodeExt" | "jetbrains";

export interface ToolDef {
  id: ToolId;
  /** 设置页开关标题。 */
  label: string;
  /** 设置页说明文案。 */
  description: string;
  /** 未显式配置时的默认状态。 */
  defaultEnabled: boolean;
}

export const SUPPORTED_TOOLS: ToolDef[] = [
  {
    id: "workbuddy",
    label: "WorkBuddy 国内版",
    description: "WorkBuddy 桌面客户端（国内版档位）账号切换与会话复制",
    defaultEnabled: true,
  },
  {
    id: "workbuddyAi",
    label: "WorkBuddy 国际版",
    description: "WorkBuddy 桌面客户端（国际版档位）账号切换与会话复制",
    defaultEnabled: true,
  },
  {
    id: "codebuddyIde",
    label: "CodeBuddy IDE",
    description: "CodeBuddy IDE 桌面客户端（国内版 / 国际版）账号切换",
    defaultEnabled: true,
  },
  {
    id: "codebuddyCli",
    label: "CodeBuddy CLI",
    description: "CodeBuddy CLI 默认账号与自动轮换",
    defaultEnabled: true,
  },
  {
    id: "vscodeExt",
    label: "VS Code CodeBuddy 插件",
    description: "VS Code 内 CodeBuddy 插件账号切换与会话复制",
    defaultEnabled: true,
  },
  {
    id: "jetbrains",
    label: "JetBrains IDE 插件（IDE 全家桶 / Android Studio）",
    description:
      "IntelliJ IDEA / PyCharm / GoLand / WebStorm / CLion 等 JetBrains 系 IDE 与 Android Studio 内 CodeBuddy 插件账号切换",
    defaultEnabled: false,
  },
];

/** 同页面内的变更通知（storage 事件只在其它标签页触发）。 */
const TOOLS_CHANGED_EVENT = "workbuddy-shift:tools-changed";

function toolPreference(id: ToolId): PersistedPreferenceOptions<boolean> {
  const def = SUPPORTED_TOOLS.find((tool) => tool.id === id);
  return {
    key: `workbuddy-shift.tools.${id}`,
    fallback: def?.defaultEnabled ?? true,
    // 与拆分前的读取语义一致：键缺失才用默认值，其余非 "1" 文本一律视为关闭。
    parse: (raw: string) => raw === "1",
    serialize: (enabled: boolean) => (enabled ? "1" : "0"),
    eventName: TOOLS_CHANGED_EVENT,
  };
}

/**
 * 工具入口是否可见：缺省 / 缺失键视为开启，与 `SUPPORTED_TOOLS` 的默认值一致。
 *
 * 与 `isToolEnabled` 的区别：输入是调用方已订阅的 `enabledTools` 快照，不读 localStorage。
 * 账号卡片的头部与操作区共用这一判定，避免各写一份「!== false」。
 */
export function toolEntryEnabled(
  enabledTools: Partial<Record<ToolId, boolean>> | undefined,
  id: ToolId,
): boolean {
  return enabledTools?.[id] !== false;
}

export function isToolEnabled(id: ToolId): boolean {
  return readPreference(toolPreference(id));
}

/**
 * WorkBuddy 入口开关：国内版与国际版是两个独立开关（`workbuddy` / `workbuddyAi`）。
 *
 * 账号卡片、页顶状态徽标必须按**账号档位**取对应的那个，否则关掉国际版会把国内版入口
 * 一起关掉（或反过来）。关联会话页是例外：它按客户端分组、不按档位，那里任一开关打开
 * 就显示 WorkBuddy 标签（见 `pages/sessions/constants.ts`）。
 */
export function workbuddyEntryEnabled(
  enabledTools: Partial<Record<ToolId, boolean>> | undefined,
  variant: "cn" | "ai" | undefined,
): boolean {
  return toolEntryEnabled(enabledTools, variant === "ai" ? "workbuddyAi" : "workbuddy");
}

export function setToolEnabled(id: ToolId, enabled: boolean): void {
  writePreference(toolPreference(id), enabled);
}

function readAll(): Record<ToolId, boolean> {
  return SUPPORTED_TOOLS.reduce(
    (acc, tool) => {
      acc[tool.id] = isToolEnabled(tool.id);
      return acc;
    },
    {} as Record<ToolId, boolean>,
  );
}

/** 订阅「支持工具」开关；设置页改动后（同一标签页）其余页面立即同步。 */
export function useSupportedTools(): Record<ToolId, boolean> {
  const [state, setState] = useState<Record<ToolId, boolean>>(readAll);
  useEffect(() => {
    const sync = () => setState(readAll());
    return subscribePreferenceChanges(TOOLS_CHANGED_EVENT, sync);
  }, []);
  return state;
}