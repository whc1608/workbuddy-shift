import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import SettingsPage, { settingsBodyClass } from "./index";
import { visibleSettingsSections } from "./sections";

/**
 * 设置页最小渲染冒烟（与账号页同一个套路，`renderToStaticMarkup`，不引 jsdom）。
 *
 * 两件事必须在改动结构时被守住：
 * 1. 首屏不因新增/重排卡片崩溃（effects 不执行，所以不会发起真实请求）；
 * 2. **注册表里的每一节都真的渲染出了自己的锚点**——目录与深链都靠 `id` 定位，
 *    漏挂一张卡不会报错，只会让 `/settings#settings-xxx` 跳到一个不存在的节点。
 */
describe("设置页", () => {
  it("渲染全部可见节，并保留它们的锚点 id", () => {
    const html = renderToStaticMarkup(createElement(SettingsPage));

    expect(html).toContain("设置");
    for (const section of visibleSettingsSections()) {
      expect(html, `缺少锚点 ${section.id}`).toContain(`id="${section.id}"`);
      expect(html, `缺少标题 ${section.title}`).toContain(section.title);
    }
  });

  it("四组标题按固定顺序出现在首屏", () => {
    const html = renderToStaticMarkup(createElement(SettingsPage));
    const order = ["通用", "账号与同步", "自动化", "桌面与维护"].map((title) =>
      html.indexOf(`>${title}<`),
    );
    for (const index of order) expect(index).toBeGreaterThan(-1);
    expect([...order].sort((a, b) => a - b)).toEqual(order);
  });
});

/**
 * 搜索空态布局（T05 缺陷 A2）：无命中时目录不渲染，若仍留着 200px 的第一列，
 * 正文会被自动塞进那 200px 里（`after-search-kaiji.png`：提示折成三行）。
 */
describe("设置页主体列定义", () => {
  it("有节时是 200px 目录 + 正文的两栏", () => {
    const className = settingsBodyClass(true);
    expect(className).toContain("xl:grid-cols-[200px_minmax(0,1fr)]");
  });

  it("搜索无命中时退回单列，不留 200px 的第一列", () => {
    const className = settingsBodyClass(false);
    expect(className).not.toContain("200px");
    expect(className).not.toContain("grid-cols");
    expect(className).not.toContain("xl:grid");
  });
});
