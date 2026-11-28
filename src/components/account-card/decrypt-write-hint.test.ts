import { createElement, type ReactNode } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import type { AccountMeta, CredentialConfig } from "@/lib/types";

import { AccountCard } from "../account-card";
import {
  DECRYPT_WRITE_HINT_DESCRIPTION,
  DECRYPT_WRITE_HINT_TITLE,
  DecryptWriteHint,
  shouldShowDecryptWriteHint,
} from "./decrypt-write-hint";

/**
 * 信封账号「打开写解密」首次引导（R6-06 收尾）。
 *
 * 走 `renderToStaticMarkup`（与 `status-chips.test.ts` 一致，不引入 jsdom / testing-library）。
 * 覆盖两件最容易回退的事：可见条件（信封 + 写关 + 未提示）与语义（标题关联、两个原生按钮）。
 */

const CONFIG: CredentialConfig = {
  decryptReadOnly: true,
  decryptWrite: false,
  firstPromptShown: false,
  platformProbe: "ok",
  platformProbeVersion: "",
};

function render(props: { accountName?: string; compact?: boolean; busy?: boolean } = {}): string {
  return renderToStaticMarkup(
    createElement(DecryptWriteHint, {
      accountName: props.accountName ?? "测试账号",
      compact: props.compact,
      busy: props.busy,
      onEnable: () => {},
      onDismiss: () => {},
    }),
  );
}

describe("可见条件：只在信封账号、写解密关闭且未提示过时出现", () => {
  it("信封 + 写关 + 未提示 → true", () => {
    expect(shouldShowDecryptWriteHint({ credentialKind: "envelope" }, CONFIG)).toBe(true);
  });

  it("firstPromptShown=true → false（用户已经看过一次）", () => {
    expect(
      shouldShowDecryptWriteHint({ credentialKind: "envelope" }, { ...CONFIG, firstPromptShown: true }),
    ).toBe(false);
  });

  it("decryptWrite 已开启 → false（没什么可引导的）", () => {
    expect(
      shouldShowDecryptWriteHint({ credentialKind: "envelope" }, { ...CONFIG, decryptWrite: true }),
    ).toBe(false);
  });

  it("plain / 缺省 credentialKind → false（不打扰明文与旧快照账号）", () => {
    expect(shouldShowDecryptWriteHint({ credentialKind: "plain" }, CONFIG)).toBe(false);
    expect(shouldShowDecryptWriteHint({}, CONFIG)).toBe(false);
    expect(shouldShowDecryptWriteHint(null, CONFIG)).toBe(false);
  });

  it("配置未加载（null / undefined）→ false（列表先出，引导是增强）", () => {
    expect(shouldShowDecryptWriteHint({ credentialKind: "envelope" }, null)).toBe(false);
    expect(shouldShowDecryptWriteHint({ credentialKind: "envelope" }, undefined)).toBe(false);
  });
});

describe("渲染：文案、语义与两个原生按钮", () => {
  it("标题 / 说明 / 两个按钮齐全，账号名写进正文", () => {
    const html = render({ accountName: "我的账号" });

    expect(html).toContain(DECRYPT_WRITE_HINT_TITLE);
    expect(html).toContain(DECRYPT_WRITE_HINT_DESCRIPTION);
    expect(html).toContain("我的账号");
    expect(html).toContain("一键开启");
    expect(html).toContain("不再提示");
    expect((html.match(/<button/g) ?? []).length).toBe(2);
  });

  it("外层是 role=note，标题通过 aria-labelledby 关联（id 非空）", () => {
    const html = render();

    expect(html).toContain('role="note"');
    const labelledBy = /aria-labelledby="([^"]+)"/.exec(html)?.[1];
    expect(labelledBy).toBeTruthy();
    expect(html).toContain(`id="${labelledBy}"`);
  });

  it("busy：两个按钮都禁用，避免重复提交", () => {
    const html = render({ busy: true });
    expect((html.match(/<button[^>]*disabled/g) ?? []).length).toBe(2);
  });

  it("compact 只收紧间距，不隐藏按钮", () => {
    const html = render({ compact: true });
    expect(html).toContain("一键开启");
    expect(html).toContain("不再提示");
  });
});

describe("卡片插槽：提示渲染在该卡片内部", () => {
  const ACCOUNT: AccountMeta = {
    id: "acc-1",
    uid: "u-1",
    email: "user@example.com",
    nickname: "测试账号",
    enterpriseName: null,
    expiresAt: null,
    refreshExpiresAt: null,
    refreshedAt: null,
    createdAt: null,
    needsRelogin: false,
    needsReloginReason: null,
  };

  function renderCard(hint?: ReactNode): string {
    return renderToStaticMarkup(
      createElement(AccountCard, {
        account: ACCOUNT,
        onDelete: () => {},
        featuresDisabled: false,
        decryptWriteHint: hint,
      }),
    );
  }

  it("传槽时出现在 <article> 内部（不是网格级横幅）", () => {
    const html = renderCard(
      createElement(DecryptWriteHint, {
        accountName: "测试账号",
        onEnable: () => {},
        onDismiss: () => {},
      }),
    );
    const articleStart = html.indexOf("<article");
    const articleEnd = html.indexOf("</article>");
    const hintAt = html.indexOf(DECRYPT_WRITE_HINT_TITLE);

    expect(articleStart).toBeGreaterThanOrEqual(0);
    expect(hintAt).toBeGreaterThan(articleStart);
    expect(hintAt).toBeLessThan(articleEnd);
  });

  it("不传槽时整张卡不出现提示（其余账号行为不变）", () => {
    expect(renderCard()).not.toContain(DECRYPT_WRITE_HINT_TITLE);
  });
});
