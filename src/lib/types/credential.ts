/**
 * R6-06「按需解密本机凭据」的设置结构（`<store_root>/credential.json`）。
 *
 * 语义（跨轨道契约）：
 * - `decryptReadOnly`：只读用途（积分 / 官方用量）的解密开关，默认**开启**；
 * - `decryptWrite`：写操作用途（签到 / 旅行 / 每日活跃）的解密开关，默认**关闭**；
 *   **只读关闭时写开关一并失效**（后端读取时归一，前端只负责把它显示成关闭）；
 * - `firstPromptShown`：首次说明是否已展示（前端置位，后端只做持久化）；
 * - `platformProbe` / `platformProbeVersion`：本机客户端能否解密的探测结论。
 *   这两个字段**只写不读**——后端绝不接受外部提交，前端也就只做展示。
 *
 * 回包里的 `defaults` 是后端下发的出厂默认值（纯增量字段），不是配置本体。
 */
export interface CredentialConfig {
  decryptReadOnly: boolean;
  decryptWrite: boolean;
  firstPromptShown: boolean;
  platformProbe: CredentialProbeState;
  platformProbeVersion: string;
  defaults?: CredentialConfig;
}

/** 平台探测结论：`unknown` = 尚未探测（下次使用时探测一次）。 */
export type CredentialProbeState = "unknown" | "ok" | "failed";

/** 设置页可提交的开关位（与后端白名单 `CREDENTIAL_SWITCH_KEYS` 一致）。 */
export type CredentialSwitchKey = "decryptReadOnly" | "decryptWrite" | "firstPromptShown";
