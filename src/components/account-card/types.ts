import type { ReactNode } from "react";

import type { ToolId } from "@/lib/supported-tools";
import type { AccountMeta, CreditExpiry, RateLimitBorrowed, RateLimitEntry, TravelStatus } from "@/lib/types";

export interface AccountCardProps {
  account: AccountMeta;
  onDelete: (a: AccountMeta) => void;
  onCheckin?: (a: AccountMeta) => void;
  onRefresh?: (a: AccountMeta) => void;
  /** 打开「账号信息」弹框（查看信息、编辑备注、选择显示字段）。 */
  onShowInfo?: (a: AccountMeta) => void;
  /**
   * 打开「账号标记」弹框（R5-04）：卡片上的标记图标与「…」菜单的标记项共用它。
   * 缺省时不渲染菜单项，状态区的标记图标也不可点击。
   */
  onShowMark?: (a: AccountMeta) => void;
  onSwitch?: (a: AccountMeta) => void;
  todayCheckedIn?: boolean;
  /** 单账号参与许可，独立于全局开关；undefined 表示配置尚未加载。 */
  autoCheckinAllowed?: boolean;
  /** 今日旅行状态（undefined=查询中/未知，不渲染标签） */
  travelStatus?: TravelStatus;
  /** 该账号当前受限的模型（来自本机日志台账）；空/缺失=无受限，不渲染图标。 */
  rateLimits?: RateLimitEntry[];
  /** 跨客户端兜底（13-B）：VS Code 无自有记录时提示「其它客户端限流至 …」。 */
  rateLimitBorrowed?: RateLimitBorrowed | null;
  credit?: CreditExpiry;
  creditLoading?: boolean;
  /** 该账号积分最近一次查询完成时间（时间戳） */
  creditUpdatedAt?: number;
  creditPriority?: boolean;
  workbuddyActive?: boolean;
  codebuddyCliConfigured?: boolean;
  codebuddyCliActive?: boolean;
  /** 任一 CodeBuddy CLI 账号切换正在进行，用于阻止并发切换。 */
  codebuddyCliBusy?: boolean;
  onSwitchCodebuddyCli?: (a: AccountMeta) => void;
  /** 当前卡片是否为正在切换的目标账号。 */
  codebuddyCliLoading?: boolean;
  /** CodeBuddy CN IDE 是否已安装（可切换）。 */
  codebuddyCnIdeAvailable?: boolean;
  codebuddyCnIdeActive?: boolean;
  codebuddyCnIdeBusy?: boolean;
  codebuddyCnIdeLoading?: boolean;
  onSwitchCodebuddyCnIde?: (a: AccountMeta) => void;
  /** VS Code 是否已安装（用户数据目录存在）。 */
  vscodeExtInstalled?: boolean;
  /** CodeBuddy 扩展是否已安装。 */
  vscodeExtExtensionInstalled?: boolean;
  /** VS Code 与扩展均已就绪，可执行切换。 */
  vscodeExtAvailable?: boolean;
  vscodeExtActive?: boolean;
  /** 任一 VS Code 扩展账号切换正在进行，用于阻止并发切换。 */
  vscodeExtBusy?: boolean;
  /** 当前卡片是否为正在切换的目标账号。 */
  vscodeExtLoading?: boolean;
  onSwitchVscodeExt?: (a: AccountMeta) => void;
  /** JetBrains 配置目录是否存在（IDEA / PyCharm 任一）。 */
  jetbrainsInstalled?: boolean;
  /** 是否任一配置目录安装了 CodeBuddy 插件。 */
  jetbrainsPluginInstalled?: boolean;
  /** JetBrains 与插件均已就绪，可执行切换。 */
  jetbrainsAvailable?: boolean;
  jetbrainsActive?: boolean;
  /** 任一 JetBrains 插件账号切换正在进行，用于阻止并发切换。 */
  jetbrainsBusy?: boolean;
  /** 当前卡片是否为正在切换的目标账号。 */
  jetbrainsLoading?: boolean;
  onSwitchJetbrains?: (a: AccountMeta) => void;
  /**
   * 支持工具开关（设置页「支持工具」）：关闭的端不渲染入口（按钮与激活徽标）。
   * 缺省 / 缺失键视为开启，与 `src/lib/supported-tools.ts` 的默认值一致。
   */
  enabledTools?: Partial<Record<ToolId, boolean>>;
  /**
   * 全局写操作开关：为 true 时卡片上的所有写操作按钮禁用（如运行时不支持写操作）。
   * 显式必填，避免子组件各自默认为 true 造成静默禁用；当前唯一调用方传 false。
   */
  featuresDisabled: boolean;
  /** 紧凑模式：头部缩成一条、按钮图标化、无 footer */
  compact?: boolean;
  /**
   * 卡片内的补充说明槽（当前只用于信封账号的「打开写解密」首次引导）。
   *
   * 渲染在头部之后、积分区之前：提示属于**这张卡**，不该被提到网格上方做成全局横幅；
   * 缺省不传即不渲染，卡片其余行为不变。调用方负责只在需要提示的那张卡上传。
   */
  decryptWriteHint?: ReactNode;
}
