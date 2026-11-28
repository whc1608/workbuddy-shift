import { afterEach, beforeEach, describe, expect, it } from "vitest";

import {
  DEFAULT_CREDIT_BAR_PALETTE,
  CreditBarPalettePreference,
  creditBarTones,
  getCreditBarPalette,
  setCreditBarPalette,
} from "@/lib/credit-bar-palette";

describe("积分条配色偏好", () => {
  const store = new Map<string, string>();
  const originalLocalStorage = globalThis.localStorage;
  const originalWindow = globalThis.window;

  const fakeLocalStorage = {
    getItem: (key: string) => store.get(key) ?? null,
    setItem: (key: string, val: string) => {
      store.set(key, String(val));
    },
    removeItem: (key: string) => {
      store.delete(key);
    },
    clear: () => store.clear(),
    get length() {
      return store.size;
    },
    key: (index: number) => Array.from(store.keys())[index] ?? null,
  };

  beforeEach(() => {
    store.clear();
    Object.defineProperty(globalThis, "localStorage", { value: fakeLocalStorage, writable: true, configurable: true });
    Object.defineProperty(globalThis, "window", { value: new EventTarget(), writable: true, configurable: true });
  });

  afterEach(() => {
    store.clear();
    Object.defineProperty(globalThis, "localStorage", { value: originalLocalStorage, writable: true, configurable: true });
    Object.defineProperty(globalThis, "window", { value: originalWindow, writable: true, configurable: true });
  });

  it("缺省时默认是 traffic（本 fork 沿用绿 / 橙 / 红观感）", () => {
    expect(DEFAULT_CREDIT_BAR_PALETTE).toBe("traffic");
    expect(CreditBarPalettePreference.fallback).toBe("traffic");
    expect(getCreditBarPalette()).toBe("traffic");
  });

  it("非法值、大小写变体与空字符串一律回退默认配色", () => {
    for (const raw of ["rainbow", "", "THEME", "Traffic", " theme", "theme ", "null", "{}"]) {
      store.set(CreditBarPalettePreference.key, raw);
      expect(getCreditBarPalette()).toBe(DEFAULT_CREDIT_BAR_PALETTE);
    }
  });

  it("写入 traffic 后能原样读回，再写回 theme 同样生效", () => {
    setCreditBarPalette("traffic");
    expect(store.get(CreditBarPalettePreference.key)).toBe("traffic");
    expect(getCreditBarPalette()).toBe("traffic");
    setCreditBarPalette("theme");
    expect(getCreditBarPalette()).toBe("theme");
  });

  it("key 使用 workbuddy-shift. 前缀，eventName 使用 workbuddy-shift: 前缀", () => {
    expect(CreditBarPalettePreference.key).toBe("workbuddy-shift.credit.palette");
    expect(CreditBarPalettePreference.eventName).toBe("workbuddy-shift:credit-palette-changed");
  });

  it("theme 的正常色是主题蓝，traffic 的正常色是绿；两套的临期橙与过期红相同", () => {
    expect(creditBarTones("theme").normal).toBe("bg-[var(--data-series-blue)]");
    expect(creditBarTones("traffic").normal).toBe("bg-emerald-500/75");
    expect(creditBarTones("theme").expiringSoon).toBe("bg-orange-500/80");
    expect(creditBarTones("traffic").expiringSoon).toBe(creditBarTones("theme").expiringSoon);
    expect(creditBarTones("theme").expired).toBe("bg-red-600/60 dark:bg-red-400/60");
    expect(creditBarTones("traffic").expired).toBe(creditBarTones("theme").expired);
  });

  it("三态类名都是字面量（不含模板拼接痕迹，Tailwind 才能扫描到）", () => {
    for (const palette of ["theme", "traffic"] as const) {
      const tones = creditBarTones(palette);
      for (const tone of [tones.normal, tones.expiringSoon, tones.expired]) {
        expect(tone.startsWith("bg-")).toBe(true);
        expect(tone).not.toContain("${");
      }
    }
  });
});
