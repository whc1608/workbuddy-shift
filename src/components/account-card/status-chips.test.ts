import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import type { AccountMeta } from "@/lib/types";

import {
  AccountStatusChips,
  ENCRYPTED_CREDENTIAL_HINT,
  ENCRYPTED_CREDENTIAL_LABEL,
  RELOGIN_RECOVERY_HINT,
} from "./status-chips";

/**
 * 账号卡片状态 chip 的凭据相关文案（R6-05 + R6-02）。
 *
 * 走 `renderToStaticMarkup`（与 `account-card.test.ts` 一致，不引入 jsdom / testing-library）。
 * 钉住三件最容易回退的事：
 * 1. 失效提示不再指向早已不存在的「从本机导入」入口，恢复路径是 OAuth 重新扫码；
 * 2. 信封（加密）凭据账号不显示「需重新登录」，改显示「凭据已加密」；
 * 3. `credentialKind` 缺省（旧后端 / 旧快照）与 `plain` 账号的行为与改造前完全一致。
 */

const NEEDS_RELOGIN_LABEL = "需重新登录";
const LEGACY_HINT_KEYWORD = "从本机导入";

function account(overrides: Partial<AccountMeta> = {}): AccountMeta {
  return {
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
    ...overrides,
  };
}

/** 渲染卡片头部的状态区（其余 chip 都不触发，只看凭据相关的那个）。 */
function render(accountMeta: AccountMeta): string {
  return renderToStaticMarkup(
    createElement(AccountStatusChips, {
      account: accountMeta,
      activeProductCount: 0,
      compact: false,
      now: Date.now(),
    }),
  );
}

describe("失效提示的恢复路径（R6-05）", () => {
  it("hint 不再提「从本机导入」，改为「删除该账号后重新 OAuth 扫码」", () => {
    const html = render(account({ needsRelogin: true, needsReloginReason: "缺少 refresh token" }));

    expect(html).toContain(NEEDS_RELOGIN_LABEL);
    expect(html).not.toContain(LEGACY_HINT_KEYWORD);
    expect(html).toContain("OAuth 扫码");
    expect(html).toContain(RELOGIN_RECOVERY_HINT);
  });

  it("Token 过期（未标 needsRelogin）用同一句恢复说明", () => {
    const html = render(account({ expiresAt: Date.now() - 60_000 }));

    expect(html).toContain("Token 已过期");
    expect(html).not.toContain(LEGACY_HINT_KEYWORD);
    expect(html).toContain(RELOGIN_RECOVERY_HINT);
  });
});

describe("信封凭据账号的 chip（R6-02）", () => {
  it("envelope：渲染「凭据已加密」，不渲染「需重新登录」", () => {
    const html = render(
      account({
        credentialKind: "envelope",
        // 历史误标：R6-02 之前后端会给信封账号写 needsRelogin，前端不能再照着显示。
        needsRelogin: true,
        needsReloginReason: "缺少 refresh token，无法刷新，需重新登录",
      }),
    );

    expect(html).toContain(ENCRYPTED_CREDENTIAL_LABEL);
    expect(html).toContain(ENCRYPTED_CREDENTIAL_HINT);
    expect(html).not.toContain(NEEDS_RELOGIN_LABEL);
    expect(html).not.toContain(LEGACY_HINT_KEYWORD);
  });

  it("envelope 且 Token 过期：也只显示「凭据已加密」（形态说明优先于过期推断）", () => {
    const html = render(account({ credentialKind: "envelope", expiresAt: Date.now() - 60_000 }));

    expect(html).toContain(ENCRYPTED_CREDENTIAL_LABEL);
    expect(html).not.toContain("Token 已过期");
  });

  it("plain：行为与改造前一致（有标记时才出「需重新登录」）", () => {
    const flagged = render(account({ credentialKind: "plain", needsRelogin: true }));
    expect(flagged).toContain(NEEDS_RELOGIN_LABEL);
    expect(flagged).not.toContain(ENCRYPTED_CREDENTIAL_LABEL);

    const healthy = render(account({ credentialKind: "plain" }));
    expect(healthy).not.toContain(NEEDS_RELOGIN_LABEL);
    expect(healthy).not.toContain(ENCRYPTED_CREDENTIAL_LABEL);
  });

  it("credentialKind 缺失（旧后端 / 旧快照）：不报错、不空白，按原逻辑显示", () => {
    const flagged = render(account({ needsRelogin: true }));
    expect(flagged).toContain(NEEDS_RELOGIN_LABEL);
    expect(flagged).not.toContain(ENCRYPTED_CREDENTIAL_LABEL);
  });
});
