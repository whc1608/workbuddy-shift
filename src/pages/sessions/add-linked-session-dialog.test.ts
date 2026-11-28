import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import { SESSION_USAGE_ATTRIBUTION } from "@/lib/session-usage-copy";

import { SessionUsageAttributionNote } from "./add-linked-session-dialog";

/**
 * 复制对话框里的「共消耗」归属说明（R6-08）。
 *
 * 弹窗关闭时 `DialogContent` 不挂载，无法通过渲染整个弹窗断言，因此这里直接渲染
 * 弹窗内使用的那段静态说明组件——同一段 JSX，同一份文案来源。
 */

function renderNote(): string {
  return renderToStaticMarkup(createElement(SessionUsageAttributionNote));
}

describe("复制对话框的用量归属说明（R6-08）", () => {
  it("写明「历史消耗归来源账号，副本只记录自己的新增」", () => {
    expect(renderNote()).toContain(SESSION_USAGE_ATTRIBUTION);
  });

  it("点明同步也不搬、且「共消耗」不可按账号相加（账号级看「积分统计」）", () => {
    const html = renderNote();
    expect(html).toContain("同步也不会搬已有消耗");
    expect(html).toContain("不能按账号相加");
    expect(html).toContain("积分统计");
  });

  it("不再出现已失效的「从本机导入」，也不断言副本一定显示 0", () => {
    const html = renderNote();
    expect(html).not.toContain("从本机导入");
    // 客户端可能显示 0，也可能显示偏少：文案不能把其中一种说死。
    expect(html).not.toContain("一定显示 0");
  });
});
