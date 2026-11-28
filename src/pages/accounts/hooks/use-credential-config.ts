import { useCallback, useEffect, useState } from "react";
import { toast } from "sonner";

import * as api from "@/lib/api";
import type { CredentialConfig } from "@/lib/types";

/**
 * 账号页对「凭据解密设置」的最小读取 / 写回封装（R6-06 收尾）。
 *
 * 三件事定死在这里：
 * 1. **整页只读一次**：`getCredentialConfig()` 只在挂载后异步取一次，失败静默（账号列表
 *    不依赖它，读不到就不提示），不要每张卡各发一次请求；
 * 2. **以回读值为准**：保存后一律用 `saveCredentialConfig` 的返回值覆盖本地状态，
 *    后端会把「只读关闭 → 写开关一并关闭」，拿提交对象当权威状态会显示错；
 * 3. **失败保留提示**：保存失败弹 `toast.error`，本地状态不动，用户可以重试。
 */
export interface CredentialConfigController {
  /** 配置本体；`null` = 尚未加载 / 读取失败（此时不渲染任何引导）。 */
  config: CredentialConfig | null;
  /** 保存进行中（两个按钮同时禁用）。 */
  saving: boolean;
  /** 「一键开启」：打开只读 + 写解密，并置位首次提示。 */
  enableWriteDecrypt: () => void;
  /** 「不再提示」：只置位首次提示，不改任何开关。 */
  dismissFirstPrompt: () => void;
}

export function useCredentialConfig(): CredentialConfigController {
  const [config, setConfig] = useState<CredentialConfig | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    let cancelled = false;
    void api.getCredentialConfig().then(
      (value) => {
        if (!cancelled) setConfig(value);
      },
      () => {
        // 读不到就保持 null：引导是增强，不能阻塞账号列表。
      },
    );
    return () => {
      cancelled = true;
    };
  }, []);

  const persist = useCallback(
    async (patch: Partial<CredentialConfig>, successText: string) => {
      if (!config || saving) return;
      setSaving(true);
      try {
        const saved = await api.saveCredentialConfig({ ...config, ...patch });
        setConfig(saved);
        if (successText) toast.success(successText);
      } catch (error) {
        toast.error("凭据设置保存失败", { description: api.asError(error) });
      } finally {
        setSaving(false);
      }
    },
    [config, saving],
  );

  const enableWriteDecrypt = useCallback(() => {
    void persist(
      { decryptReadOnly: true, decryptWrite: true, firstPromptShown: true },
      "已开启写解密",
    );
  }, [persist]);

  const dismissFirstPrompt = useCallback(() => {
    void persist({ firstPromptShown: true }, "");
  }, [persist]);

  return { config, saving, enableWriteDecrypt, dismissFirstPrompt };
}
