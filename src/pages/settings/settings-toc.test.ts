import { describe, expect, it } from "vitest";

import {
  landingDelta,
  pickActiveSection,
  TOC_ACTIVE_OFFSET,
  TOC_LANDING_TOP,
  type TocSectionBox,
} from "./settings-toc";

/**
 * 目录高亮的判定规则（`pickActiveSection`）单测。
 *
 * 这些用例对应 T05 报告 §4 的缺陷 A1：点目录「凭据」后滚动位置与 hash 都对，
 * 但 `aria-current` 漂到下一节「未完成的写入」。旧实现用 IntersectionObserver
 * 按「最大交叉比 / 回调批次里最靠上的可见项」选 active，短卡必输给下一节；
 * 新实现只看节标题是否越过判定线（`top <= viewportTop + offset`）。
 */

const bo = (id: string, top: number): TocSectionBox => ({ id, top });

/** 1440x900 视口、`main` 贴顶时的度量（`getBoundingClientRect().top`）。 */
const VIEW_TOP = 0;
const VIEW_HEIGHT = 900;

describe("pickActiveSection", () => {
  it("在顶部（第一节标题还没越线）时选第一节", () => {
    const sections = [bo("settings-appearance", 230), bo("settings-tools", 700), bo("settings-sync", 1200)];
    expect(pickActiveSection(sections, VIEW_TOP, VIEW_HEIGHT, TOC_ACTIVE_OFFSET)).toBe(
      "settings-appearance",
    );
  });

  it("取最后一个越线的节，而不是交叉比最大的节", () => {
    // 第二节是张很短的卡（标题 top=110，下一节 320 就来了）：旧实现里它占视口比例小，
    // 会被下一节抢走；按位置判定则应归它。
    const sections = [bo("settings-credential", 110), bo("settings-pending-writes", 320)];
    expect(pickActiveSection(sections, VIEW_TOP, VIEW_HEIGHT, TOC_ACTIVE_OFFSET)).toBe(
      "settings-credential",
    );
  });

  it("点击/深链的落点（标题停在 scroll-mt-24 的 96px）稳定选中目标节，再等也不会漂到下一节", () => {
    // 点「凭据」后 `scrollIntoView` 把标题停在 96px（`scroll-mt-24`），下一节在 640px。
    // 1.5s / 4s 各重算一次：滚动已停，矩形不变，结果必须每次都一样。
    const sections = [
      bo("settings-diagnostics", -900),
      bo("settings-credential", 96),
      bo("settings-pending-writes", 640),
      bo("settings-bundle", 1100),
    ];
    const at15 = pickActiveSection(sections, VIEW_TOP, VIEW_HEIGHT, TOC_ACTIVE_OFFSET);
    const at4s = pickActiveSection(sections, VIEW_TOP, VIEW_HEIGHT, TOC_ACTIVE_OFFSET);
    expect(at15).toBe("settings-credential");
    expect(at4s).toBe("settings-credential");
    // 判定线必须高于 scroll-mt（96px），否则落点会被判成「还没到」而退回上一节。
    expect(TOC_ACTIVE_OFFSET).toBeGreaterThan(96);
  });

  it("滚到底时选最后一节（最后一节标题越不过判定线也一样）", () => {
    const sections = [bo("settings-bundle", -1200), bo("settings-about", 500)];
    expect(pickActiveSection(sections, VIEW_TOP, VIEW_HEIGHT, TOC_ACTIVE_OFFSET, true)).toBe(
      "settings-about",
    );
    // 没到底时不硬选最后一节：标题还在下面，应归上一节。
    expect(pickActiveSection(sections, VIEW_TOP, VIEW_HEIGHT, TOC_ACTIVE_OFFSET, false)).toBe(
      "settings-bundle",
    );
  });

  it("滚动容器不是贴顶时按容器的视口顶计算", () => {
    // 桌面壳顶栏 72px 时，main 的可视顶从 72px 开始，判定线也随之平移：
    // 容器贴顶（line=120）时 tools 在 180 还没越线；容器下移 72px（line=192）时越线。
    const sections = [
      bo("settings-appearance", 60),
      bo("settings-tools", 180),
      bo("settings-sync", 700),
    ];
    expect(pickActiveSection(sections, 0, VIEW_HEIGHT, TOC_ACTIVE_OFFSET)).toBe("settings-appearance");
    expect(pickActiveSection(sections, 72, VIEW_HEIGHT, TOC_ACTIVE_OFFSET)).toBe("settings-tools");
  });

  it("判定线不会越过视口下沿", () => {
    // 窗口被压到 60px 高时 offset 会失真：判定线取视口高度，第二节（100px）不算越线。
    expect(pickActiveSection([bo("a", 50), bo("b", 100)], 0, 60, TOC_ACTIVE_OFFSET)).toBe("a");
  });

  it("空列表返回空串（搜索把目录清空时不写回 undefined）", () => {
    expect(pickActiveSection([], VIEW_TOP, VIEW_HEIGHT, TOC_ACTIVE_OFFSET)).toBe("");
  });
});

/**
 * 落点校正量（`landingDelta`）单测，对应 T05 已知偶发问题 K1：
 * 点目录后 `scrollIntoView({ behavior: "smooth" })` 只算一次目标偏移，跳转期间上方卡片
 * 长高 89px，标题从 96px 漂到 185px → 越过判定线 → 高亮落到上一节。
 * 平滑滚动停稳后按这个差值做一次非平滑校正，把它拉回 96px。
 */
describe("landingDelta", () => {
  it("正好落在目标线上返回 0", () => {
    expect(landingDelta(TOC_LANDING_TOP, VIEW_TOP, TOC_LANDING_TOP)).toBe(0);
  });

  it("落点偏短（标题漂到 185，差 89px）返回 +89", () => {
    // 首轮 verify 的失败几何：scrollTop 3321（应为 3410），标题 top 185。
    expect(landingDelta(185, VIEW_TOP, TOC_LANDING_TOP)).toBe(89);
  });

  it("落点过头（多滚了 37px，标题在 59）返回 -37", () => {
    expect(landingDelta(TOC_LANDING_TOP - 37, VIEW_TOP, TOC_LANDING_TOP)).toBe(-37);
  });

  it("±2px 内返回 0（平滑收尾的像素抖动不触发校正）", () => {
    expect(landingDelta(TOC_LANDING_TOP + 2, VIEW_TOP, TOC_LANDING_TOP)).toBe(0);
    expect(landingDelta(TOC_LANDING_TOP - 2, VIEW_TOP, TOC_LANDING_TOP)).toBe(0);
    expect(landingDelta(TOC_LANDING_TOP + 1.9, VIEW_TOP, TOC_LANDING_TOP)).toBe(0);
    expect(landingDelta(TOC_LANDING_TOP - 2.5, VIEW_TOP, TOC_LANDING_TOP)).toBe(-2.5);
  });

  it("滚动容器不贴顶时按容器可视顶度量", () => {
    // main 的可视顶在 72px（桌面壳顶栏）时，标题停在 72+96=168 才算命中。
    expect(landingDelta(168, 72, TOC_LANDING_TOP)).toBe(0);
    expect(landingDelta(240, 72, TOC_LANDING_TOP)).toBe(72);
  });

  it("落点线必须落在判定线以内：校正后落地即命中目标节", () => {
    // 校正目标 96px < 判定线 120px，否则校正完高亮还会掉回上一节。
    expect(TOC_LANDING_TOP).toBeLessThan(TOC_ACTIVE_OFFSET);
  });
});
