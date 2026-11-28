import type { ReactNode } from "react";

/**
 * 卡片状态区（R5-05）：状态 chip 的数据化描述 + 紧凑卡「3 槽 + N」选择逻辑。
 *
 * 拆成纯函数放在独立模块里，便于单测（`status-selection.test.ts` 走 node 环境，
 * 不渲染 React）。渲染仍在 `status-chips.tsx`。
 */

/**
 * 状态 chip 的稳定标识。
 *
 * 新增一种 chip 时**必须**同时给出 `priority` 与 `detail`：前者决定紧凑卡是否被收进
 * `+N`，后者是 `+N` tooltip 与 `aria-label` 的纯文本文案（没有 detail 就会出现空行）。
 */
export type StatusChipKey =
  | "mark"
  | "relogin"
  | "rateLimit"
  | "checkinPending"
  | "autoCheckinOff"
  | "creditPriority"
  | "checkinDone"
  | "travel"
  | "credential"
  | "lastUsed"
  | "enterprise"
  | "multiTool";

/**
 * 固定优先级（唯一来源）：数字越小越重要，紧凑卡优先保留。
 *
 * 前 9 项与清单 R5-05 的决策表逐项一致；`creditPriority`（「建议优先使用」徽标）
 * 清单表里未列出，插在 `autoCheckinOff` 与 `checkinDone` 之间——它表达的是
 * 「这个账号有即将到期的积分」，比「今日已签到」这种纯信息更重要。
 */
export const STATUS_PRIORITY: Record<StatusChipKey, number> = {
  /** 用户自己设的，**永不进 +N**（另见 `pinned`）。 */
  mark: 1,
  /** 需重新登录 / Token 过期，阻断使用。 */
  relogin: 2,
  /** 模型限额，直接影响当前任务。 */
  rateLimit: 3,
  /** 今日未签到，可操作提醒。 */
  checkinPending: 4,
  /** 自动签到被关闭，配置状态。 */
  autoCheckinOff: 5,
  /** 建议优先使用（有即将到期积分）。 */
  creditPriority: 6,
  /** 今日已签到，纯信息。 */
  checkinDone: 7,
  /** 旅行状态，纯信息。 */
  travel: 8,
  /**
   * 凭据形态（R6-02）：「凭据已加密」只是形态说明，不阻断使用（信封账号照常能切换），
   * 所以排在纯信息档、比「最近切换」更靠前——它解释的是「为什么没有『需重新登录』」。
   */
  credential: 9,
  /**
   * 「最近切换」（issue #184）：纯信息，但比身份属性更贴近「我刚做了什么」，
   * 所以排在 `enterprise` 之前——紧凑卡只剩一个空槽时优先留下它。
   */
  lastUsed: 10,
  /** 身份属性。 */
  enterprise: 11,
  /** 多端在用汇总。 */
  multiTool: 12,
};

/** 紧凑卡固定 3 个状态槽（含标记）。 */
export const COMPACT_STATUS_SLOT_COUNT = 3;

/** 一个状态 chip：渲染节点 + 选择所需元数据。 */
export interface StatusChipDescriptor {
  key: StatusChipKey;
  /** 越小越重要（见 `STATUS_PRIORITY`）。 */
  priority: number;
  /** 渲染节点（紧凑与非紧凑同一份）。 */
  node: ReactNode;
  /** 纯文本详情，供 `+N` 的 tooltip 与 `aria-label` 使用。 */
  detail: string;
  /**
   * 钉住：无论优先级与槽数如何都渲染（标记属于用户显式设置，不允许被折叠）。
   */
  pinned?: boolean;
}

/**
 * 按「钉住优先 + 优先级升序」挑出紧凑卡要渲染的 chip，其余进 `+N`。
 *
 * - 入参数组的顺序即**视觉顺序**（与非紧凑卡一致），返回值也保持该顺序；
 *   也就是说「选谁」由 `priority` 决定，「怎么排」仍按视觉顺序。
 * - 同级按原索引稳定排序，避免每次渲染跳位。
 * - `cap <= 0` 时全部收起（仅为完整性，调用方固定用 3）。
 */
export function selectCompactStatusChips<T extends { priority: number; pinned?: boolean }>(
  chips: readonly T[],
  cap: number = COMPACT_STATUS_SLOT_COUNT,
): { visible: T[]; hidden: T[] } {
  if (chips.length === 0) return { visible: [], hidden: [] };
  if (cap <= 0) return { visible: [], hidden: [...chips] };

  const pinnedIndexes = chips
    .map((chip, index) => ({ chip, index }))
    .filter(({ chip }) => chip.pinned === true)
    .map(({ index }) => index)
    // 钉住项也可能超过槽数（理论上不会），此时按优先级截断，保证不超宽。
    .sort((left, right) => chips[left].priority - chips[right].priority || left - right)
    .slice(0, cap);

  const keep = new Set<number>(pinnedIndexes);
  const remaining = chips
    .map((chip, index) => ({ chip, index }))
    .filter(({ index }) => !keep.has(index))
    .sort((left, right) => left.chip.priority - right.chip.priority || left.index - right.index);

  for (const { index } of remaining) {
    if (keep.size >= cap) break;
    keep.add(index);
  }

  const visible: T[] = [];
  const hidden: T[] = [];
  chips.forEach((chip, index) => {
    if (keep.has(index)) visible.push(chip);
    else hidden.push(chip);
  });
  return { visible, hidden };
}

/** `+N` 徽标的读屏文案：`还有 2 个状态：今日已签到、正在旅行`。 */
export function hiddenChipsAriaLabel(hidden: readonly { detail: string }[]): string {
  if (hidden.length === 0) return "";
  return `还有 ${hidden.length} 个状态：${hidden.map((chip) => chip.detail).join("、")}`;
}
