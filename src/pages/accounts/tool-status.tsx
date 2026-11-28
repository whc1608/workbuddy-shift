import {
  CodeBuddyAiIdeMark,
  CodeBuddyCnIdeMark,
  CodeBuddyMark,
  JetbrainsMark,
  VscodeExtMark,
  WorkBuddyAiMark,
  WorkBuddyMark,
} from "@/components/product-marks";
import { useSupportedTools, workbuddyEntryEnabled } from "@/lib/supported-tools";
import type { AppStatus, WbVariant } from "@/lib/types";
import { variantAppName, variantCodebuddyIdeName, variantUsesIntlCodebuddyIde } from "@/lib/variant";
import type { ClientStatusSnapshot } from "@/stores/accounts";

interface ToolStatusMarksProps {
  variant: WbVariant;
  enabledTools: ReturnType<typeof useSupportedTools>;
  status: AppStatus | null;
  clientStatus: ClientStatusSnapshot;
  workbuddyCurrentName: string;
}

/**
 * 页头右侧的五个工具状态标记：图标 + hover 卡片（是否运行 / 当前账号）。
 *
 * 展示名全部在这里派生，页面只负责把状态对象与当前账号名传进来——
 * 否则页面要在 JSX 前维护四个 `xxxCurrentName`，且只有一个消费点。
 */
export function ToolStatusMarks({
  variant,
  enabledTools,
  status,
  clientStatus,
  workbuddyCurrentName,
}: ToolStatusMarksProps) {
  const { codebuddyCli, codebuddyCnIde, vscodeExt, jetbrains } = clientStatus;
  const appName = variantAppName(variant);
  const codebuddyCurrentName = codebuddyCli?.configured
    ? codebuddyCli.activeAccountName || "未检测到"
    : "尚未接入";
  const cnIdeCurrentName = codebuddyCnIde?.installed
    ? codebuddyCnIde.activeAccountName || "未检测到"
    : "未安装";
  const vscodeExtCurrentName = vscodeExt?.installed
    ? vscodeExt.activeAccountName || "未检测到"
    : "未接入";
  const jetbrainsCurrentName = jetbrains?.installed
    ? jetbrains.activeAccountName || "未检测到"
    : "未接入";

  return (
    <div className="flex shrink-0 items-center gap-4 pt-1">
      <div className="flex items-center gap-2.5">
{workbuddyEntryEnabled(enabledTools, variant) && (
        <span className="group relative inline-flex cursor-default">
          <span
            className={
              status?.running
                ? "inline-flex rounded-[22%] bg-primary/70 p-[2px] shadow-sm shadow-primary/15"
                : "inline-flex rounded-[22%] bg-muted-foreground/30 p-[2px]"
            }
          >
            {variant === "ai" ? <WorkBuddyAiMark size={28} /> : <WorkBuddyMark size={28} />}
          </span>
          <span className="pointer-events-none absolute right-0 top-full z-50 mt-2 hidden whitespace-nowrap rounded-md bg-popover px-2.5 py-1.5 text-xs text-popover-foreground shadow-lg ring-1 ring-black/5 group-hover:block">
            {appName}：{status?.running ? "运行中" : "未运行"} · 当前账号：{workbuddyCurrentName}
          </span>
        </span>
      )}
{enabledTools.codebuddyIde && (
        <span className="group relative inline-flex cursor-default">
          <span
            className={
              codebuddyCnIde?.installed
                ? "inline-flex rounded-[22%] bg-primary/70 p-[2px] shadow-sm shadow-primary/15"
                : "inline-flex rounded-[22%] bg-muted-foreground/30 p-[2px]"
            }
          >
            {variantUsesIntlCodebuddyIde(variant) ? (
              <CodeBuddyAiIdeMark size={28} />
            ) : (
              <CodeBuddyCnIdeMark size={28} />
            )}
          </span>
          <span className="pointer-events-none absolute right-0 top-full z-50 mt-2 hidden whitespace-nowrap rounded-md bg-popover px-2.5 py-1.5 text-xs text-popover-foreground shadow-lg ring-1 ring-black/5 group-hover:block">
            {variantCodebuddyIdeName(variant)}：{codebuddyCnIde?.installed ? (codebuddyCnIde.running ? "运行中" : "已接入") : "未接入"} · 当前账号：{cnIdeCurrentName}
          </span>
        </span>
      )}
{enabledTools.vscodeExt && (
        <span className="group relative inline-flex cursor-default">
          <span
            className={
              vscodeExt?.installed && vscodeExt?.extensionInstalled
                ? "inline-flex rounded-[22%] bg-primary/70 p-[2px] shadow-sm shadow-primary/15"
                : "inline-flex rounded-[22%] bg-muted-foreground/30 p-[2px]"
            }
          >
            <VscodeExtMark size={28} />
          </span>
          <span className="pointer-events-none absolute right-0 top-full z-50 mt-2 hidden whitespace-nowrap rounded-md bg-popover px-2.5 py-1.5 text-xs text-popover-foreground shadow-lg ring-1 ring-black/5 group-hover:block">
            VS Code CodeBuddy 插件：{!vscodeExt?.installed ? "未检测到 VS Code" : !vscodeExt.extensionInstalled ? "未安装插件" : vscodeExt.running ? "运行中" : "已接入"} · 当前账号：{vscodeExtCurrentName}
          </span>
        </span>
      )}
{enabledTools.jetbrains && (
        <span className="group relative inline-flex cursor-default">
          <span
            className={
              jetbrains?.installed && jetbrains?.pluginInstalled
                ? "inline-flex rounded-[22%] bg-primary/70 p-[2px] shadow-sm shadow-primary/15"
                : "inline-flex rounded-[22%] bg-muted-foreground/30 p-[2px]"
            }
          >
            <JetbrainsMark size={28} />
          </span>
          <span className="pointer-events-none absolute right-0 top-full z-50 mt-2 hidden whitespace-nowrap rounded-md bg-popover px-2.5 py-1.5 text-xs text-popover-foreground shadow-lg ring-1 ring-black/5 group-hover:block">
            JetBrains IDE 插件：{!jetbrains?.installed ? "未检测到 JetBrains IDE" : !jetbrains.pluginInstalled ? "未安装插件" : jetbrains.running ? "运行中" : "已接入"} · 当前账号：{jetbrainsCurrentName}
          </span>
        </span>
      )}
{enabledTools.codebuddyCli && (
        <span className="group relative inline-flex cursor-default">
          <span
            className={
              codebuddyCli?.configured
                ? "inline-flex rounded-[22%] bg-primary/70 p-[2px] shadow-sm shadow-primary/15"
                : "inline-flex rounded-[22%] bg-muted-foreground/30 p-[2px]"
            }
          >
            <CodeBuddyMark size={28} />
          </span>
          <span className="pointer-events-none absolute right-0 top-full z-50 mt-2 hidden whitespace-nowrap rounded-md bg-popover px-2.5 py-1.5 text-xs text-popover-foreground shadow-lg ring-1 ring-black/5 group-hover:block">
            CodeBuddy CLI：{codebuddyCli?.migrationRequired ? "需升级" : codebuddyCli?.configured ? "已接入" : "未接入"} · 当前账号：{codebuddyCurrentName}
          </span>
        </span>
      )}
      </div>
    </div>
  );
}
