import { useEffect, useState } from "react";
import { ChevronDown, Loader2 } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { CardContent } from "@/components/ui/card";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import { Switch } from "@/components/ui/switch";
import * as api from "@/lib/api";
import type {
  RateLimitConfig,
  RateLimitHookStatus,
} from "@/lib/types";

import { RestoreDefaultsRow, stripDefaults } from "./restore-defaults";
import { SettingsFieldRow, SettingsGroup } from "./rows";

/** 限额监听：总开关 + hook 接入状态（CLI / WorkBuddy 实时上报，桌面 IDE 与 VS Code 插件走日志扫描）。 */
export function RateLimitCard() {
  const [config, setConfig] = useState<RateLimitConfig | null>(null);
  const [status, setStatus] = useState<RateLimitHookStatus | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    let cancelled = false;
    void Promise.all([api.getRateLimitConfig(), api.getRateLimitHookStatus()])
      .then(([cfg, hook]) => {
        if (cancelled) return;
        setConfig(cfg);
        setStatus(hook);
      })
      .catch((e) => {
        if (!cancelled) toast.error("限额监听配置加载失败", { description: api.asError(e) });
      });
    return () => {
      cancelled = true;
    };
  }, []);

  async function onToggle(enabled: boolean) {
    if (!config || busy) return;
    const previous = config;
    setConfig({ ...config, enabled });
    setBusy(true);
    try {
      // 整个配置一起提交：只带 enabled 会把「卸载过」标记冲掉，重启后 hook 又被自动装回。
      setConfig(await api.saveRateLimitConfig({ ...config, enabled }));
      toast.success(enabled ? "限额监听已开启" : "限额监听已关闭");
    } catch (e) {
      setConfig(previous);
      toast.error("限额监听设置保存失败", { description: api.asError(e) });
    } finally {
      setBusy(false);
    }
  }

  /**
   * 「扫描 CodeBuddy IDE 日志」独立开关：关掉两个桌面 IDE 与 VS Code 插件宿主的日志来源
   * （这两类的 429 不触发任何事件，日志是它们唯一的数据源），CLI / WorkBuddy 的 hook 通路不受影响。
   */
  /** 跨客户端兜底（13-B）：只影响账号卡是否提示「其它客户端限流至 …」。 */
  async function onToggleCrossClient(crossClientInfer: boolean) {
    if (!config || busy) return;
    const previous = config;
    setConfig({ ...config, crossClientInfer });
    setBusy(true);
    try {
      setConfig(await api.saveRateLimitConfig({ ...config, crossClientInfer }));
      toast.success(
        crossClientInfer ? "已开启跨客户端限额提示" : "已关闭跨客户端限额提示",
      );
    } catch (e) {
      setConfig(previous);
      toast.error("限额监听设置保存失败", { description: api.asError(e) });
    } finally {
      setBusy(false);
    }
  }

  async function onToggleIdeLogs(scanIdeLogs: boolean) {
    if (!config || busy) return;
    const previous = config;
    setConfig({ ...config, scanIdeLogs });
    setBusy(true);
    try {
      // 与总开关一样整份提交：只带 scanIdeLogs 会把 enabled / hookOptOut 冲成默认值。
      setConfig(await api.saveRateLimitConfig({ ...config, scanIdeLogs }));
      toast.success(
        scanIdeLogs
          ? "已开启 IDE 日志扫描"
          : "已关闭日志扫描：CodeBuddy 桌面 IDE 与 VS Code 插件的限额不再显示",
      );
    } catch (e) {
      setConfig(previous);
      toast.error("限额监听设置保存失败", { description: api.asError(e) });
    } finally {
      setBusy(false);
    }
  }

  /**
   * 回读限额配置：装 / 卸 hook 无论成败都会写「接入 / 卸载」意图（部分目标失败也算），
   * 本地标记不能只靠乐观更新，否则随后拨总开关会把过期值写回磁盘。
   */
  function refreshHookConfig() {
    return api
      .getRateLimitConfig()
      .then(setConfig)
      .catch(() => {
        /* 读不到就保持本地值，下次进设置页会重新拉 */
      });
  }

  async function onInstall() {
    if (busy) return;
    setBusy(true);
    try {
      setStatus(await api.installRateLimitHook());
      toast.success("已接入限额监听", {
        description: "CodeBuddy CLI / WorkBuddy 的 429 会实时上报（原配置已备份，可随时卸载还原）",
      });
    } catch (e) {
      toast.error("接入 hook 失败", { description: api.asError(e) });
    } finally {
      setBusy(false);
      void refreshHookConfig();
    }
  }

  async function onUninstall() {
    if (busy) return;
    setBusy(true);
    try {
      setStatus(await api.uninstallRateLimitHook());
      toast.success("已卸载 hook", {
        description: "客户端配置恢复原状，之后不会再自动接入（限额改由日志扫描发现，可随时点「接入 hook」恢复）",
      });
    } catch (e) {
      toast.error("卸载 hook 失败", { description: api.asError(e) });
    } finally {
      setBusy(false);
      void refreshHookConfig();
    }
  }

  /**
   * 「恢复默认」：参数取自后端回包里的 `defaults`，前端不硬编码任何默认值。
   *
   * `hookOptOut` 例外——它是「用户主动点过卸载 hook」的意图记录，跟着默认值恢复成
   * `false` 等于把 hook 又自动装回去，正是用户明确拒绝过的行为；这里保留原值。
   */
  async function restoreDefaults() {
    if (!config?.defaults || busy) return;
    const defaults = stripDefaults(config.defaults);
    setBusy(true);
    try {
      setConfig(await api.saveRateLimitConfig({ ...defaults, hookOptOut: config.hookOptOut }));
      toast.success("已恢复默认参数", {
        description: "限额监听开关、IDE 日志扫描与跨客户端提示已回到出厂默认；hook 接入状态保持不变",
      });
    } catch (e) {
      toast.error("恢复默认失败", { description: api.asError(e) });
    } finally {
      setBusy(false);
    }
  }

  const probe = status?.vscodeExt;
  const existingTargets = status?.targets.filter((target) => target.exists) ?? [];
  const installedCount = existingTargets.filter((target) => target.installed).length;
  // IDE 的限额只有日志一条来源：扫描开关关闭时文案不能再说「仍按日志扫描」。
  const ideNote =
    config?.scanIdeLogs === false
      ? "CodeBuddy 桌面 IDE 与 VS Code 插件的日志扫描已关闭"
      : "CodeBuddy 桌面 IDE 与 VS Code 插件无事件，仍按日志扫描";
  const hookDescription = status
    ? existingTargets.length === 0
      ? `未检测到 CodeBuddy CLI / WorkBuddy 客户端：没有可接入的配置（${ideNote}）`
      : status.installed
        ? `${installedCount} / ${existingTargets.length} 个已安装客户端已接入：429 当轮实时上报（秒级）；${ideNote}`
        : config?.hookOptOut
          ? "已卸载：不会再自动接入，限额改由日志扫描发现；点「接入 hook」可恢复实时上报"
          : "未接入：限额仅靠定期扫描日志发现（最多滞后数分钟）"
    : "加载中…";

  return (
    <SettingsGroup id="settings-rate-limit" title="限额监听">
      <CardContent className="space-y-0 p-0">
        <SettingsFieldRow
          label="启用限额监听"
          description="关闭后不扫描日志、账号卡片不显示限额标记；重新开启后恢复"
          htmlFor="rl-enabled"
          operational
        >
          <Switch
            id="rl-enabled"
            checked={config?.enabled ?? true}
            disabled={busy || !config}
            onCheckedChange={(v) => void onToggle(v)}
            aria-label="启用限额监听"
          />
        </SettingsFieldRow>

        <SettingsFieldRow
          label="扫描 CodeBuddy IDE 日志"
          description="CodeBuddy 桌面 IDE 与 VS Code 插件的限额只有日志一条来源，关掉后不再显示；CodeBuddy CLI / WorkBuddy 的实时上报不受影响。"
          htmlFor="rl-ide-scan"
          operational
        >
          <Switch
            id="rl-ide-scan"
            checked={config?.scanIdeLogs ?? true}
            disabled={busy || !config}
            onCheckedChange={(v) => void onToggleIdeLogs(v)}
            aria-label="扫描 CodeBuddy IDE 日志"
          />
        </SettingsFieldRow>

        <SettingsFieldRow
          label="跨客户端限额提示"
          description="当前账号没有该模型的限额记录时，用同账号其它客户端（WorkBuddy / CLI / IDE）的限额提示「其它客户端限流至 …」；只作提示，不改任何客户端自己的数字"
          htmlFor="rl-cross-client"
          operational
        >
          <Switch
            id="rl-cross-client"
            checked={config?.crossClientInfer ?? true}
            disabled={busy || !config}
            onCheckedChange={(v) => void onToggleCrossClient(v)}
            aria-label="跨客户端限额提示"
          />
        </SettingsFieldRow>

        <SettingsFieldRow
          label="接入客户端 hook"
          description={hookDescription}
          operational
        >
          {status?.installed ? (
            <Button
              id="rl-hook"
              size="sm"
              variant="outline"
              disabled={busy}
              onClick={() => void onUninstall()}
            >
              {busy ? <Loader2 className="animate-spin" /> : null}卸载 hook
            </Button>
          ) : (
            <Button
              id="rl-hook"
              size="sm"
              disabled={busy || !status || existingTargets.length === 0}
              onClick={() => void onInstall()}
            >
              {busy ? <Loader2 className="animate-spin" /> : null}接入 hook
            </Button>
          )}
        </SettingsFieldRow>

        <div className="px-4 py-1 sm:px-5">
          <RestoreDefaultsRow
            description="把限额监听开关、IDE 日志扫描与跨客户端提示恢复为出厂默认；已接入/已卸载的 hook 状态保持不动。"
            disabled={busy || !config?.defaults}
            onRestore={() => void restoreDefaults()}
          />
        </div>

        {/*
          VS Code 插件的限额走**日志来源**：插件宿主的 429 记录落在 `exthost/<插件>/` 下，
          两个桌面 IDE 的目录里没有，因此单独作为一路日志来源扫描。
          识别是尽力而为的：只读本机日志、不注入插件；没有实时上报通道，最多滞后一个扫描周期。
        */}
        <div className="border-t border-border/50 px-4 py-3 text-xs leading-5 text-muted-foreground sm:px-5">
          <p className="font-medium text-foreground">VS Code 插件：按本地日志识别（尽力而为）</p>
          <p className="mt-1">
            插件宿主的日志里出现限额记录时，账号卡片会显示对应模型的限额标记与官方给出的恢复时刻；只读取本机日志，不向插件写入任何内容。插件没有实时上报通道，识别最多滞后一个扫描周期（约 5 分钟）；日志里没有记录时不会显示——看不到标记不代表没有被限流。
          </p>
          {probe ? (
            /*
              探针收进折叠区（T05 第 5 项）：它是排障数据（监听文件数、命中样本路径），
              对普通用户是噪声，默认收起；需要取证时再展开，避免每次都占掉半屏。
            */
            <Collapsible className="mt-2">
              <CollapsibleTrigger asChild>
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  className="h-7 gap-1 rounded-md px-2 text-[11px] text-muted-foreground [&[data-state=open]>svg]:rotate-180"
                >
                  <ChevronDown className="size-3.5 transition-transform" />
                  诊断信息（日志探针）
                </Button>
              </CollapsibleTrigger>
              <CollapsibleContent>
                <div className="mt-1 rounded-lg border border-border/60 bg-muted/30 px-3 py-2 text-[11px] leading-5">
                  <p className="font-medium text-foreground">日志探针（只采样，不判定限流）</p>
                  <p className="mt-1 text-muted-foreground">
                    已监听 {probe.files} 个文件 · 累计命中 {probe.hits} 条强信号 · 本次新读{" "}
                    {probe.scannedFiles} 个文件
                    {probe.lastScanAt ? ` · 最近扫描 ${new Date(probe.lastScanAt).toLocaleString("zh-CN")}` : ""}
                  </p>
                  <p className="mt-1 text-muted-foreground">{probe.reason}</p>
                  {probe.samples.length > 0 ? (
                    <ul className="mt-1 space-y-0.5 font-mono text-[10px] text-muted-foreground/80">
                      {probe.samples.map((sample, index) => (
                        <li key={`${index}-${sample.slice(0, 12)}`} className="truncate" title={sample}>
                          {sample}
                        </li>
                      ))}
                    </ul>
                  ) : null}
                  {probe.samplesPath ? (
                    <p className="mt-1 break-all font-mono text-[10px] text-muted-foreground/70">
                      样本：{probe.samplesPath}
                    </p>
                  ) : null}
                </div>
              </CollapsibleContent>
            </Collapsible>
          ) : null}
        </div>
      </CardContent>
    </SettingsGroup>
  );
}
