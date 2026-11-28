import { type ReactNode } from "react";
import { CardContent } from "@/components/ui/card";
import { Switch } from "@/components/ui/switch";
import { SUPPORTED_TOOLS, setToolEnabled, useSupportedTools, type ToolId } from "@/lib/supported-tools";
import { variantUsesIntlCodebuddyIde } from "@/lib/variant";
import { CodeBuddyAiIdeMark, CodeBuddyCnIdeMark, CodeBuddyMark, JetbrainsMark, VscodeExtMark, WorkBuddyAiMark, WorkBuddyMark } from "@/components/product-marks";
import { useAccountsStore } from "@/stores/accounts";
import { SettingsFieldRow, SettingsGroup } from "./rows";

/**
 * 支持工具：控制各客户端入口是否在界面上出现。
 *
 * 关闭只隐藏入口（账号卡片按钮、页顶状态徽标）并跳过该端的状态轮询，
 * 不动账号库、不影响其它工具，重新打开即恢复。JetBrains 端默认关闭
 * （新增端先灰度），打开后才出现对应入口。
 */
export function SupportedToolsCard() {
  const enabled = useSupportedTools();
  const variant = useAccountsStore((s) => s.variant);
  /** 行内产品图标：WorkBuddy 两行按档位各用自己的字块，其余按客户端固定。 */
  const marks: Record<ToolId, (size: number) => ReactNode> = {
    workbuddy: (size) => <WorkBuddyMark size={size} />,
    workbuddyAi: (size) => <WorkBuddyAiMark size={size} />,
    codebuddyIde: (size) =>
      variantUsesIntlCodebuddyIde(variant) ? <CodeBuddyAiIdeMark size={size} /> : <CodeBuddyCnIdeMark size={size} />,
    codebuddyCli: (size) => <CodeBuddyMark size={size} />,
    vscodeExt: (size) => <VscodeExtMark size={size} />,
    jetbrains: (size) => <JetbrainsMark size={size} />,
  };

  return (
    <SettingsGroup id="settings-tools" title="支持工具">
      <CardContent className="space-y-0 p-0">
        {SUPPORTED_TOOLS.map((tool, index) => (
          <SettingsFieldRow
            key={tool.id}
            className={index === SUPPORTED_TOOLS.length - 1 ? "border-b-0" : undefined}
            label={
              <span className="flex items-center gap-2.5">
                {marks[tool.id](20)}
                <span>{tool.label}</span>
              </span>
            }
            description={tool.description}
            htmlFor={`tools-${tool.id}`}
          >
            <Switch
              id={`tools-${tool.id}`}
              checked={enabled[tool.id]}
              onCheckedChange={(on) => setToolEnabled(tool.id, on)}
              aria-label={tool.label}
            />
          </SettingsFieldRow>
        ))}
      </CardContent>
    </SettingsGroup>
  );
}
