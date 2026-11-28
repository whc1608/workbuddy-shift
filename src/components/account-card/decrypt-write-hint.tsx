import { useId } from "react";

import { Button } from "@/components/ui/button";
import type { AccountMeta, CredentialConfig } from "@/lib/types";
import { cn } from "@/lib/utils";

/** 提示标题（`aria-labelledby` 与视觉标题共用同一段文案）。 */
export const DECRYPT_WRITE_HINT_TITLE = "签到 / 旅行需要打开写解密";

/** 提示正文：说明只影响本机内存，不写回账号库 / 客户端登录态。 */
export const DECRYPT_WRITE_HINT_DESCRIPTION =
  "该账号的凭据是客户端加密信封。开启后 Shift 可在内存中解密，用于签到、派猫猫旅行等写操作；不会写回账号库或客户端登录态。";

/**
 * 是否该在**这张卡**上提示打开写解密。
 *
 * 只认信封账号：明文账号的写操作本来就能用，缺失凭据的账号先要重登；
 * `firstPromptShown` 与 `decryptWrite` 任一已置位都不再打扰用户。
 * 配置尚未加载（`null`）时返回 `false`——提示是增强，绝不能挡住账号列表。
 */
export function shouldShowDecryptWriteHint(
  account: Pick<AccountMeta, "credentialKind"> | null | undefined,
  config: CredentialConfig | null | undefined,
): boolean {
  if (!account || account.credentialKind !== "envelope") return false;
  if (!config) return false;
  return !config.firstPromptShown && !config.decryptWrite;
}

export interface DecryptWriteHintProps {
  /** 该提示所属账号的显示名（帮助用户对上具体是哪张卡）。 */
  accountName: string;
  /** 紧凑模式：字号与内边距收紧，但不隐藏任何按钮。 */
  compact?: boolean;
  /** 保存中：两个按钮都禁用，避免重复提交。 */
  busy?: boolean;
  onEnable: () => void;
  onDismiss: () => void;
}

/**
 * 信封账号的「打开写解密」首次引导（R6-06 收尾）。
 *
 * 语义：外层是 `role="note"` 的分区，标题用 `useId()` 关联 `aria-labelledby`；
 * 两个按钮都是原生 `<button>`（Button 透传），键盘可达，焦点样式沿用既有 `focus-visible`。
 * 视觉只使用主题 token（`border` / `bg-muted/30` / `text-muted-foreground`），不写字面色值。
 */
export function DecryptWriteHint({
  accountName,
  compact = false,
  busy = false,
  onEnable,
  onDismiss,
}: DecryptWriteHintProps) {
  const titleId = useId();
  return (
    <section
      role="note"
      aria-labelledby={titleId}
      className={cn(
        "min-w-0 rounded-[14px] border border-border bg-muted/30",
        compact ? "px-3 py-2.5 text-[11px]" : "px-4 py-3 text-xs",
      )}
    >
      <h3 id={titleId} className="break-words text-sm font-medium text-foreground">
        {DECRYPT_WRITE_HINT_TITLE}
      </h3>
      <p className="mt-1 break-words leading-5 text-muted-foreground">
        <span className="text-foreground">{accountName}</span>：{DECRYPT_WRITE_HINT_DESCRIPTION}
      </p>
      <div className="mt-2 flex flex-wrap items-center gap-2">
        <Button type="button" size="sm" disabled={busy} onClick={onEnable}>
          一键开启
        </Button>
        <Button type="button" size="sm" variant="ghost" disabled={busy} onClick={onDismiss}>
          不再提示
        </Button>
      </div>
    </section>
  );
}
