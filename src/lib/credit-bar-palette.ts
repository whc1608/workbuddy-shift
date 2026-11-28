import {
  readPreference,
  usePersistedPreference,
  usePersistedPreferenceState,
  writePreference,
  type PersistedPreferenceOptions,
} from "@/lib/persisted-preference";

/**
 * 积分进度条「正常可用」状态的配色方案（设置 → 外观里可选）。
 *
 * 语义：
 * - `theme`（**默认**）：正常可用沿用主题数据蓝 `--data-series-blue`，与 0.5.0 线上观感一致；
 * - `traffic`：正常可用用绿（绿 / 橙 / 红三态，PR #30 的观感），适合把「还剩多少」当红绿灯看。
 *
 * 临期橙与已过期红两套完全相同：这两态表达的是**风险**，不该被用户偏好改掉，
 * 否则「临期」可能在两套配色里长得不一样，反而看不出紧迫程度。
 *
 * 类名字符串必须是字面量：Tailwind 只静态扫描源码里的完整类名，运行时拼接（`bg-${x}`）
 * 会被裁掉，生产构建里条子直接没有颜色。所以两套色调写成常量表，不做字符串运算。
 */

/** 用户可选的积分条配色。 */
export type CreditBarPalette = "theme" | "traffic";

/** 三态配色：正常可用 / 即将过期 / 已过期。 */
export interface CreditBarTones {
  normal: string;
  expiringSoon: string;
  expired: string;
}

/**
 * 默认配色：绿 / 橙 / 红。
 *
 * 本 fork 的自定义（上游默认 `theme`）——沿用改动前「正常绿、临期橙、过期红」的观感；
 * 想要上游的主题蓝可在「设置 → 外观 → 积分条状态色」里切回 `theme`。
 */
export const DEFAULT_CREDIT_BAR_PALETTE: CreditBarPalette = "traffic";

/** 已过期：两套配色共用（风险色不随偏好变化）。 */
const EXPIRED_TONE = "bg-red-600/60 dark:bg-red-400/60";
/** 即将过期：两套配色共用。 */
const EXPIRING_SOON_TONE = "bg-orange-500/80";
/** 正常可用 · 主题蓝。 */
const THEME_NORMAL_TONE = "bg-[var(--data-series-blue)]";
/** 正常可用 · 绿橙红里的绿。 */
const TRAFFIC_NORMAL_TONE = "bg-emerald-500/75";

/** 两套色调的唯一来源；调用方只读，不要就地改写。 */
const CREDIT_BAR_TONES: Record<CreditBarPalette, CreditBarTones> = {
  theme: {
    normal: THEME_NORMAL_TONE,
    expiringSoon: EXPIRING_SOON_TONE,
    expired: EXPIRED_TONE,
  },
  traffic: {
    normal: TRAFFIC_NORMAL_TONE,
    expiringSoon: EXPIRING_SOON_TONE,
    expired: EXPIRED_TONE,
  },
};

/** 取某个配色的三态类名（纯函数，测试与渲染共用）。 */
export function creditBarTones(palette: CreditBarPalette): CreditBarTones {
  return CREDIT_BAR_TONES[palette];
}

/**
 * localStorage 偏好：键 `workbuddy-shift.credit.palette`，值 `"theme"` / `"traffic"`。
 *
 * `parse` 只认这两个值，其它一律回退 `"theme"`：曾经存过别的方案名或手改坏了，
 * 都该看到默认观感，而不是条子变成透明。
 */
export const CreditBarPalettePreference: PersistedPreferenceOptions<CreditBarPalette> = {
  key: "workbuddy-shift.credit.palette",
  fallback: DEFAULT_CREDIT_BAR_PALETTE,
  parse: (raw) => (raw === "theme" || raw === "traffic" ? raw : null),
  serialize: (value) => value,
  eventName: "workbuddy-shift:credit-palette-changed",
};

/** 读取当前配色（存储不可用 / 损坏时回退默认）。 */
export function getCreditBarPalette(): CreditBarPalette {
  return readPreference(CreditBarPalettePreference);
}

/** 写入配色并通知同页面订阅者（跨标签页走原生 storage 事件）。 */
export function setCreditBarPalette(palette: CreditBarPalette): void {
  writePreference(CreditBarPalettePreference, palette);
}

/** 订阅配色偏好，返回当前值；设置页改动后所有积分条即时同步。 */
export function useCreditBarPalette(): CreditBarPalette {
  return usePersistedPreference(CreditBarPalettePreference);
}

/**
 * 订阅配色偏好并返回 `[当前值, 写入函数]`（设置页的下拉用）。
 *
 * 写入函数先更新订阅者再落盘，存储不可用时本次会话内仍然生效。
 */
export function useCreditBarPaletteState(): [CreditBarPalette, (palette: CreditBarPalette) => void] {
  return usePersistedPreferenceState(CreditBarPalettePreference);
}

/** 当前配色对应的三态类名（组件内取一次，避免每个条形重复读偏好）。 */
export function useCreditBarTones(): CreditBarTones {
  return creditBarTones(useCreditBarPalette());
}
