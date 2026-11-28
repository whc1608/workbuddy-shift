import type { CredentialConfig } from "../types/credential";
import { call } from "./core";

/**
 * 读取 R6-06 的凭据设置（只读 / 写解密开关 + 平台探测结论 + 出厂默认值）。
 *
 * 回包里没有任何令牌或路径：解密能力**不经**这两个命令暴露，见后端模块注释。
 */
export function getCredentialConfig(): Promise<CredentialConfig> {
  return call("get_credential_config");
}

/**
 * 保存凭据设置。
 *
 * 只提交开关位：后端会忽略提交里的 `platformProbe`（探测结论只能由闸门写），
 * 只读关闭时也会把写开关一并归一成关闭 —— 因此调用方应当用回读值刷新界面，
 * 不要拿自己提交的对象当权威状态。
 */
export function saveCredentialConfig(config: CredentialConfig): Promise<CredentialConfig> {
  return call("save_credential_config", { config });
}
