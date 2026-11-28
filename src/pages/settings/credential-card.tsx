import { useEffect, useState } from "react";
import { ShieldCheck } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { CardContent } from "@/components/ui/card";
import { Switch } from "@/components/ui/switch";
import { asError } from "@/lib/api/core";
import {
  getCredentialConfig,
  saveCredentialConfig,
} from "@/lib/api/credential";
import type { CredentialConfig } from "@/lib/types/credential";

import { cn } from "@/lib/utils";
import { RestoreDefaultsRow, stripDefaults } from "./restore-defaults";
import { SettingsFieldRow, SettingsGroup } from "./rows";

/**
 * R6-06：用本机 WorkBuddy 客户端凭据填补「加密凭据账号」的空档。
 *
 * # 为什么需要这张卡片
 *
 * WorkBuddy 5.6 起把认证文件里的 token 存成加密信封，账号库里导入的也可能是信封。
 * 这类账号过去完全不可用（积分 / 签到 / 旅行全部只拿到「凭据已加密」）。Shift 现在
 * 可以在**本机内存里**解出 access token 用于本次请求 —— 但这属于动用本机凭据，
 * 必须由用户显式打开，且只读与写操作分成两个开关。
 *
 * # 前端只做三件事
 *
 * 1. 展示/切换两个开关（`decryptReadOnly` / `decryptWrite`）；
 * 2. 展示平台探测结论（**只展示**，后端不接受提交）；
 * 3. 首次说明置位 `firstPromptShown`。
 *
 * 界面上任何地方都不显示、也不请求令牌本身：解密能力没有对外接口。
 */
export function CredentialCard() {
  const [cfg, setCfg] = useState<CredentialConfig | null>(null);
  const [saving, setSaving] = useState(false);
  const [loadError, setLoadError] = useState("");

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const loaded = await getCredentialConfig();
        if (!cancelled) setCfg(loaded);
      } catch (error) {
        if (!cancelled) setLoadError(asError(error));
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  /**
   * 提交开关位并采用回读值。
   *
   * 必须采用回读值而不是本地拼的那一份：后端会把「只读关闭」时的写开关一并归一成
   * 关闭，也会忽略提交里的探测结论。拿本地对象当权威状态会让界面与磁盘不一致。
   */
  async function submit(patch: Partial<CredentialConfig>, successTitle: string) {
    if (!cfg) return;
    setSaving(true);
    try {
      const saved = await saveCredentialConfig({ ...cfg, ...patch });
      setCfg(saved);
      toast.success(successTitle);
    } catch (error) {
      toast.error("凭据设置保存失败", { description: asError(error) });
    } finally {
      setSaving(false);
    }
  }

  function onToggleReadOnly(next: boolean) {
    // 只读关闭时写开关一并失效：本地先跟着变，避免出现「只读已关、写还亮着」的错觉；
    // 权威结果仍以后端回读值为准。
    void submit(
      { decryptReadOnly: next, decryptWrite: next ? cfg?.decryptWrite === true : false },
      next ? "已允许本机凭据用于只读查询" : "已关闭本机凭据解密（只读）",
    );
  }

  function onToggleWrite(next: boolean) {
    void submit({ decryptWrite: next }, next ? "已允许本机凭据用于写操作" : "已关闭本机凭据解密（写操作）");
  }

  function onAcknowledgeNotice() {
    void submit({ firstPromptShown: true }, "不再提示");
  }

  function onRestoreDefaults() {
    const defaults = cfg?.defaults;
    if (!defaults) return;
    // 恢复默认只回填三个开关位：探测结论由闸门自己写，前端不碰。
    const patch = stripDefaults(defaults);
    void submit(
      {
        decryptReadOnly: patch.decryptReadOnly,
        decryptWrite: patch.decryptWrite,
        firstPromptShown: patch.firstPromptShown,
      },
      "已恢复默认开关",
    );
  }

  const readOnlyOn = cfg?.decryptReadOnly === true;
  const writeOn = cfg?.decryptWrite === true;

  return (
    <SettingsGroup id="settings-credential" title="本机客户端凭据">
      <CardContent className="space-y-0 p-0">
        {loadError ? (
          <p className="px-4 py-3 text-xs text-danger sm:px-5">凭据设置读取失败：{loadError}</p>
        ) : !cfg ? (
          <p className="px-4 py-3 text-sm text-muted-foreground sm:px-5">加载配置中…</p>
        ) : (
          <>
            {!cfg.firstPromptShown && (
              <div className="border-b border-border/60 bg-muted/25 px-4 py-3 sm:px-5">
                <div className="flex items-start gap-2">
                  <ShieldCheck className="mt-0.5 size-4 shrink-0 text-muted-foreground" />
                  <div className="min-w-0 space-y-1.5 text-xs leading-5 text-muted-foreground">
                    <p>
                      开启后，加密凭据的账号也能正常查积分、签到与派猫猫：Shift 会用本机
                      WorkBuddy 客户端当前的登录凭据，在内存里解出访问令牌用于本次请求。
                    </p>
                    <p>
                      不会写回账号库、不会改动客户端登录态、不会进入导出备份；解出的令牌只保留
                      最多 5 分钟，refresh token 从不解密。客户端没有登录同一账号时，这些账号
                      仍按原来的「凭据已加密」处理。
                    </p>
                    <Button size="sm" variant="outline" onClick={onAcknowledgeNotice} disabled={saving}>
                      知道了
                    </Button>
                  </div>
                </div>
              </div>
            )}

            <SettingsFieldRow
              label="只读查询使用本机凭据"
              description="积分、官方用量查询。关闭后写操作开关一并失效，也不再拉起客户端进程。"
              operational
            >
              <Switch
                aria-label="只读查询使用本机凭据"
                checked={readOnlyOn}
                disabled={saving}
                onCheckedChange={onToggleReadOnly}
              />
            </SettingsFieldRow>

            <SettingsFieldRow
              label="写操作使用本机凭据"
              description="签到、派猫猫旅行、每日活跃。默认关闭；只在只读开关打开时可开启。"
              operational
            >
              <Switch
                aria-label="写操作使用本机凭据"
                checked={writeOn}
                disabled={saving || !readOnlyOn}
                onCheckedChange={onToggleWrite}
              />
            </SettingsFieldRow>

            <SettingsFieldRow
              label="本机客户端解密能力"
              description={probeDescription(cfg)}
              className="border-b-0"
              operational
            >
              <span
                className={cn(
                  "text-xs",
                  cfg.platformProbe === "ok"
                    ? "text-success"
                    : cfg.platformProbe === "failed"
                      ? "text-warning"
                      : "text-muted-foreground",
                )}
              >
                {probeLabel(cfg)}
              </span>
            </SettingsFieldRow>

            <div className="mx-4 sm:mx-5">
              <RestoreDefaultsRow
                description="把两个开关恢复为出厂值（只读开、写关），不改动探测结论。"
                disabled={saving || !cfg.defaults}
                onRestore={onRestoreDefaults}
              />
            </div>
          </>
        )}
      </CardContent>
    </SettingsGroup>
  );
}

/** 探测结论的短标签（只读展示：后端不接受前端提交探测状态）。 */
function probeLabel(cfg: CredentialConfig): string {
  switch (cfg.platformProbe) {
    case "ok":
      return "可用";
    case "failed":
      return "不可用";
    default:
      return "尚未探测";
  }
}

/** 探测结论的说明：把「什么时候会重试」讲清楚，避免用户反复点开关试图触发。 */
function probeDescription(cfg: CredentialConfig): string {
  const version = cfg.platformProbeVersion ? `（客户端 ${cfg.platformProbeVersion}）` : "";
  switch (cfg.platformProbe) {
    case "ok":
      return `已在本机验证通过${version}。客户端升级后会重新验证一次。`;
    case "failed":
      return `本机当前验证未通过${version}，不会反复重试；客户端升级后会自动再验一次。`;
    default:
      return "首次用到时会自动验证一次；验证通过后才真正启用解密。";
  }
}
