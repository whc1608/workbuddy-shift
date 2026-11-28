import { Check, Loader2 } from "lucide-react";

import { Button } from "@/components/ui/button";
import { DemoAction } from "@/components/demo-action";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { CodeBuddyCnIdeMark, CodeBuddyMark, JetbrainsMark, VscodeExtMark, WorkBuddyMark } from "@/components/product-marks";
import { demoModeEnabled } from "@/lib/demo-mode";
import { toolEntryEnabled, workbuddyEntryEnabled } from "@/lib/supported-tools";
import { accountVariant } from "@/lib/variant";
import { cn } from "@/lib/utils";

import type { AccountCardProps } from "./types";
import { jetbrainsTooltip, vscodeExtTooltip } from "./status-chips";

/**
 * 产品的「当前账号」标记：footer 用胶囊形态，紧凑卡片头部用图标角标形态。
 *
 * 两种形态共用同一份产品 → 标题 / 标记映射，避免头部与 footer 各写一遍当前账号角标。
 */
export function ProductCurrentState({
  product,
  compact = false,
  iconOnly = false,
}: {
  product: "workbuddy" | "codebuddy" | "codebuddy-cn" | "vscode-ext" | "jetbrains";
  compact?: boolean;
  /** 图标角标形态（紧凑卡片头部用）：与操作按钮同尺寸，只保留勾选角标与悬停文案。 */
  iconOnly?: boolean;
}) {
  const title =
    product === "workbuddy"
      ? "WorkBuddy 当前账号"
      : product === "codebuddy-cn"
        ? "CodeBuddy IDE 当前账号"
        : product === "vscode-ext"
          ? "VS Code CodeBuddy 插件当前账号"
          : product === "jetbrains"
            ? "JetBrains IDE CodeBuddy 插件当前账号"
            : "CodeBuddy CLI 当前账号";
  const mark = (size: number) =>
    product === "workbuddy" ? (
      <WorkBuddyMark size={size} />
    ) : product === "codebuddy-cn" ? (
      <CodeBuddyCnIdeMark size={size} />
    ) : product === "vscode-ext" ? (
      <VscodeExtMark size={size} />
    ) : product === "jetbrains" ? (
      <JetbrainsMark size={size} />
    ) : (
      <CodeBuddyMark size={size} />
    );

  if (iconOnly) {
    return (
      <Tooltip>
        <TooltipTrigger asChild>
          <span
            role="status"
            aria-label={title}
            className="relative inline-flex size-7 items-center justify-center rounded-lg border border-primary/25 bg-primary/10 text-primary"
          >
            {mark(15)}
            <span className="absolute -right-1 -top-1 flex size-3.5 items-center justify-center rounded-full bg-primary text-primary-foreground">
              <Check className="size-2.5" strokeWidth={3} />
            </span>
          </span>
        </TooltipTrigger>
        <TooltipContent side="top">{title}</TooltipContent>
      </Tooltip>
    );
  }

  return (
    <span
      role="status"
      aria-label={title}
      title={title}
      className={cn(
        "inline-flex items-center gap-2 rounded-full border border-primary/25 bg-primary/10 px-2.5 text-primary shadow-[inset_0_1px_0_rgba(255,255,255,.8)]",
        compact ? "h-7 text-xs" : "h-9",
      )}
    >
      {mark(compact ? 18 : 22)}
      <Check className={compact ? "size-3.5" : "size-4"} strokeWidth={2.25} />
    </span>
  );
}

export function AccountCardActions({ account, codebuddyCliActive, codebuddyCliBusy, codebuddyCliConfigured, codebuddyCliLoading, codebuddyCnIdeActive, codebuddyCnIdeAvailable, codebuddyCnIdeBusy, codebuddyCnIdeLoading, enabledTools, featuresDisabled, jetbrainsActive, jetbrainsAvailable, jetbrainsBusy, jetbrainsInstalled, jetbrainsLoading, jetbrainsPluginInstalled, onSwitch, onSwitchCodebuddyCli, onSwitchCodebuddyCnIde, onSwitchJetbrains, onSwitchVscodeExt, vscodeExtActive, vscodeExtAvailable, vscodeExtBusy, vscodeExtExtensionInstalled, vscodeExtInstalled, vscodeExtLoading, workbuddyActive }: AccountCardProps) {
  return (
        <footer className="flex flex-wrap items-center gap-2.5 border-t px-5 py-2.5">
          {workbuddyEntryEnabled(enabledTools, accountVariant(account)) && (workbuddyActive ? <ProductCurrentState product="workbuddy" compact /> : demoModeEnabled ? (
            <DemoAction>
              <Button variant="outline" size="sm" className="h-7 rounded-full px-2.5 pr-3.5 text-xs" aria-label="设为 WorkBuddy 当前账号">
                <WorkBuddyMark size={18} /><span>设为当前</span>
              </Button>
            </DemoAction>
          ) : (
            <Tooltip>
              <TooltipTrigger asChild>
                <Button variant="outline" size="sm" className="h-7 rounded-full px-2.5 pr-3.5 text-xs" disabled={featuresDisabled || !onSwitch} onClick={() => onSwitch?.(account)} aria-label="设为 WorkBuddy 当前账号">
                  <WorkBuddyMark size={18} /><span>设为当前</span>
                </Button>
              </TooltipTrigger>
              <TooltipContent side="top">设为 WorkBuddy 当前账号（会重启 WorkBuddy）</TooltipContent>
            </Tooltip>
          ))}
          {toolEntryEnabled(enabledTools, "codebuddyIde") && (codebuddyCnIdeActive ? <ProductCurrentState product="codebuddy-cn" compact /> : demoModeEnabled ? (
            <DemoAction>
              <Button variant="outline" size="sm" className="h-7 rounded-full px-2.5 pr-3.5 text-xs" aria-label="切换到 CodeBuddy IDE">
                <CodeBuddyCnIdeMark size={18} /><span>IDE</span>
              </Button>
            </DemoAction>
          ) : (
            <Tooltip>
              <TooltipTrigger asChild>
                <Button variant="outline" size="sm" className="h-7 rounded-full px-2.5 pr-3.5 text-xs" disabled={featuresDisabled || !codebuddyCnIdeAvailable || !onSwitchCodebuddyCnIde || codebuddyCnIdeBusy} onClick={() => onSwitchCodebuddyCnIde?.(account)} aria-label={codebuddyCnIdeLoading ? "正在切换 CodeBuddy IDE" : "切换到 CodeBuddy IDE"} aria-busy={codebuddyCnIdeLoading}>
                  {codebuddyCnIdeLoading ? <Loader2 className="size-4 animate-spin" /> : <CodeBuddyCnIdeMark size={18} />}<span>{codebuddyCnIdeLoading ? "切换中…" : "IDE"}</span>
                </Button>
              </TooltipTrigger>
              <TooltipContent side="top">{codebuddyCnIdeAvailable ? "切换到 CodeBuddy IDE（会重启 IDE）" : "未检测到 CodeBuddy IDE"}</TooltipContent>
            </Tooltip>
          ))}
          {toolEntryEnabled(enabledTools, "vscodeExt") && (vscodeExtActive ? <ProductCurrentState product="vscode-ext" compact /> : demoModeEnabled ? (
            <DemoAction>
              <Button variant="outline" size="sm" className="h-7 rounded-full px-2.5 pr-3.5 text-xs" aria-label="切换到 VS Code CodeBuddy 插件（可复制会话）">
                <VscodeExtMark size={18} /><span>VS Code</span>
              </Button>
            </DemoAction>
          ) : (
            <Tooltip>
              <TooltipTrigger asChild>
                <Button variant="outline" size="sm" className="h-7 rounded-full px-2.5 pr-3.5 text-xs" disabled={featuresDisabled || !vscodeExtAvailable || !onSwitchVscodeExt || vscodeExtBusy} onClick={() => onSwitchVscodeExt?.(account)} aria-label="切换到 VS Code CodeBuddy 插件（可复制会话）" aria-busy={vscodeExtLoading}>
                  {vscodeExtLoading ? <Loader2 className="size-4 animate-spin" /> : <VscodeExtMark size={18} />}<span>{vscodeExtLoading ? "切换中…" : "VS Code"}</span>
                </Button>
              </TooltipTrigger>
              <TooltipContent side="top">{vscodeExtTooltip(vscodeExtInstalled, vscodeExtExtensionInstalled)}</TooltipContent>
            </Tooltip>
          ))}
          {toolEntryEnabled(enabledTools, "jetbrains") && (jetbrainsActive ? <ProductCurrentState product="jetbrains" compact /> : demoModeEnabled ? (
            <DemoAction>
              <Button variant="outline" size="sm" className="h-7 rounded-full px-2.5 pr-3.5 text-xs" aria-label="切换到 JetBrains IDE CodeBuddy 插件">
                <JetbrainsMark size={18} /><span>JetBrains</span>
              </Button>
            </DemoAction>
          ) : (
            <Tooltip>
              <TooltipTrigger asChild>
                <Button variant="outline" size="sm" className="h-7 rounded-full px-2.5 pr-3.5 text-xs" disabled={featuresDisabled || !jetbrainsAvailable || !onSwitchJetbrains || jetbrainsBusy} onClick={() => onSwitchJetbrains?.(account)} aria-label="切换到 JetBrains IDE CodeBuddy 插件" aria-busy={jetbrainsLoading}>
                  {jetbrainsLoading ? <Loader2 className="size-4 animate-spin" /> : <JetbrainsMark size={18} />}<span>{jetbrainsLoading ? "切换中…" : "JetBrains"}</span>
                </Button>
              </TooltipTrigger>
              <TooltipContent side="top">{jetbrainsTooltip(jetbrainsInstalled, jetbrainsPluginInstalled)}</TooltipContent>
            </Tooltip>
          ))}
          {toolEntryEnabled(enabledTools, "codebuddyCli") && (codebuddyCliActive ? <ProductCurrentState product="codebuddy" compact /> : demoModeEnabled ? (
            <DemoAction>
              <Button variant="outline" size="sm" className="h-7 rounded-full px-2.5 pr-3.5 text-xs" aria-label="设为 CodeBuddy CLI 当前账号">
                <CodeBuddyMark size={18} /><span>CLI 当前</span>
              </Button>
            </DemoAction>
          ) : (
            <Tooltip>
              <TooltipTrigger asChild>
                <Button variant="outline" size="sm" className="h-7 rounded-full px-2.5 pr-3.5 text-xs" disabled={featuresDisabled || !codebuddyCliConfigured || !onSwitchCodebuddyCli || codebuddyCliBusy} onClick={() => onSwitchCodebuddyCli?.(account)} aria-label={codebuddyCliLoading ? "正在切换 CodeBuddy CLI 当前账号" : "设为 CodeBuddy CLI 当前账号"} aria-busy={codebuddyCliLoading}>
                  {codebuddyCliLoading ? <Loader2 className="size-4 animate-spin" /> : <CodeBuddyMark size={18} />}<span>{codebuddyCliLoading ? "切换中…" : "CLI 当前"}</span>
                </Button>
              </TooltipTrigger>
              <TooltipContent side="top">{codebuddyCliConfigured ? "设为 CodeBuddy CLI 当前账号" : "请先接入 CodeBuddy CLI"}</TooltipContent>
            </Tooltip>
          ))}
        </footer>
  );
}
