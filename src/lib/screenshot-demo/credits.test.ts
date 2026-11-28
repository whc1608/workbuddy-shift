import { describe, expect, it } from "vitest";

import type { CreditExpiry } from "../types";
import { screenshotDemoResponse } from "./index";

/**
 * 演示积分包 fixture 的不变量（R7-T09）。
 *
 * 这两条断言是「积分明细四态 + 一屏 18 行」取证的前提：截图能不能拍到四态、能不能铺满
 * 18 行，完全取决于 fixture 里有没有这些种子，而不是 UI 能不能画。改了种子就要在这里
 * 同步，避免下次又出现「结构性拍不到」。
 */

function creditExpiry(accountId: string): CreditExpiry {
  return screenshotDemoResponse("get_credit_expiry", { accountId }) as CreditExpiry;
}

describe("演示积分包 fixture（演示账号 · 资源包矩阵）", () => {
  it("≥18 个资源包，且未使用 / 部分使用 / 临期 / 过期四态齐全", () => {
    const credit = creditExpiry("demo-account-matrix");
    const resources = credit.resources ?? [];
    expect(resources.length).toBeGreaterThanOrEqual(18);

    // 未使用灰：remaining === total（明细里「已用」显示 —）
    expect(resources.some((row) => row.remaining === row.total && row.used === 0)).toBe(true);
    // 部分使用：0 < 已用 < 总量
    expect(resources.some((row) => row.used > 0 && row.remaining > 0 && row.remaining < row.total && !row.expired && !row.expiringSoon)).toBe(true);
    // 临期橙：7 天内到期、仍未用完、尚未过期
    expect(resources.some((row) => row.expiringSoon && !row.expired && row.remaining > 0)).toBe(true);
    // 过期红：到期时间在过去
    const expired = resources.filter((row) => row.expired);
    expect(expired.length).toBeGreaterThan(0);
    expect(expired.every((row) => (row.expireAt ?? 0) < Date.now())).toBe(true);
    // 已过期的不再计为临期（否则橙色指标会把红色行重复统计一次）
    expect(expired.some((row) => row.expiringSoon)).toBe(false);
  });

  it("顶层指标与包列表自洽：capacity / remaining / expiredRemaining 都能对账", () => {
    const credit = creditExpiry("demo-account-matrix");
    const resources = credit.resources ?? [];
    const sum = (rows: typeof resources) => Number(rows.reduce((total, row) => total + row.remaining, 0).toFixed(2));

    expect(credit.totalCapacity).toBe(resources.reduce((total, row) => total + row.total, 0));
    // 剩余 = 未过期包剩余；已过期剩余单独计，两者相加 = 包列表 remaining 总和。
    expect(credit.totalRemaining).toBe(sum(resources.filter((row) => !row.expired)));
    expect(credit.expiredRemaining).toBe(sum(resources.filter((row) => row.expired)));
    expect(Number(((credit.totalRemaining ?? 0) + (credit.expiredRemaining ?? 0)).toFixed(2))).toBe(sum(resources));
    expect(credit.expired).toBe(true);
    expect(credit.expiringSoon).toBe(true);
  });
});
