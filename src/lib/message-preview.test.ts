import { describe, expect, it } from "vitest";

import { parseMessagePreview, type PreviewBlock } from "@/lib/message-preview";

/**
 * 内容归一化：去掉 Markdown 记号（`#` `|` `*` `` ` `` `>` `-` `:` 反斜杠）与全部空白，
 * 只比较「真正要展示的字符」。分隔行 `|---|---|` 归一化后是空串，天然被跳过。
 */
function normalize(text: string): string {
  return text.replace(/[\\#|`*>\-:\s]/g, "");
}

/** 去行首块级标记（`## ` / `- ` / `1. `）：标记本身不进解析结果，属于语法而非内容。 */
function stripLeadingMarker(line: string): string {
  return line.replace(/^\s*(?:#{1,6}\s+|[-*+]\s+|\d{1,3}[.)]\s+)/, "");
}

/** 所有块里的文本拼起来（表格按单元格、代码块带上语言标记）。 */
function blockTexts(blocks: PreviewBlock[]): string {
  return blocks
    .map((block) => {
      switch (block.kind) {
        case "paragraph":
          return block.lines.join("\n");
        case "heading":
          return block.text;
        case "code":
          return `${block.lang}\n${block.text}`;
        case "list":
          return block.items.join("\n");
        case "table":
          return [block.head, ...block.rows].map((row) => row.join(" ")).join("\n");
      }
    })
    .join("\n");
}

/** 取出表格块（顺带做类型收窄）。 */
function tableOf(block: PreviewBlock | undefined): Extract<PreviewBlock, { kind: "table" }> {
  if (!block || block.kind !== "table") {
    throw new Error(`期望表格块，实际是 ${block ? block.kind : "空"}`);
  }
  return block;
}

/** 用户截图里的真实形态（末尾 `| **合计** | *` 就是 240 字截断处）。 */
const REAL_SHAPE = [
  "清理完成。",
  "",
  "## 执行汇总",
  "",
  "| 批次 | 内容 | 文件数 | 释放 |",
  "|---|---|---|---|",
  "| 1 | 推免/各大院校 下 16 个院校目录 | 约 300 | 302 MB |",
  "| **合计** | *",
].join("\n");

describe("parseMessagePreview", () => {
  it("空文本返回空数组（调用方据此不渲染容器）", () => {
    expect(parseMessagePreview("")).toEqual([]);
    expect(parseMessagePreview("\n\n   \n")).toEqual([]);
  });

  it("真实形态：段落 + 二级标题 + 4 列表格（分隔行不进 rows，单元格不残留竖线）", () => {
    const blocks = parseMessagePreview(REAL_SHAPE);
    expect(blocks.map((block) => block.kind)).toEqual(["paragraph", "heading", "table"]);
    expect(blocks[0]).toEqual({ kind: "paragraph", lines: ["清理完成。"] });
    expect(blocks[1]).toEqual({ kind: "heading", level: 2, text: "执行汇总" });

    const table = tableOf(blocks[2]);
    expect(table.hasHeader).toBe(true);
    expect(table.head).toEqual(["批次", "内容", "文件数", "释放"]);
    expect(table.rows).toHaveLength(2);
    expect(table.rows[0]).toEqual(["1", "推免/各大院校 下 16 个院校目录", "约 300", "302 MB"]);
    // 截断的半行被补到 4 列，不丢已有单元格
    expect(table.rows[1]).toEqual(["**合计**", "*", "", ""]);
    for (const row of [table.head, ...table.rows]) {
      for (const cell of row) expect(cell).not.toContain("|");
    }
  });

  it("转义竖线 \\| 仍是同一个单元格", () => {
    const blocks = parseMessagePreview(["| 名称 | 备注 |", "|---|---|", "| a | b\\|c |"].join("\n"));
    const table = tableOf(blocks[0]);
    expect(table.head).toEqual(["名称", "备注"]);
    expect(table.rows).toEqual([["a", "b|c"]]);
  });

  it("列数不齐：以表头列数为准，短行补空串、长行截断", () => {
    const blocks = parseMessagePreview(
      ["| a | b | c |", "|---|---|---|", "| 1 | 2 |", "| 1 | 2 | 3 | 4 |"].join("\n"),
    );
    const table = tableOf(blocks[0]);
    expect(table.rows).toEqual([
      ["1", "2", ""],
      ["1", "2", "3"],
    ]);
  });

  it("缺分隔行但连续 ≥2 行都以 | 开头：按无表头表格兜底", () => {
    const blocks = parseMessagePreview(["| a | b |", "| 1 | 2 |"].join("\n"));
    const table = tableOf(blocks[0]);
    expect(table.hasHeader).toBe(false);
    expect(table.head).toEqual(["a", "b"]);
    expect(table.rows).toEqual([["1", "2"]]);
  });

  it("只有一行含 |：仍按段落处理", () => {
    const blocks = parseMessagePreview("| a | b |");
    expect(blocks).toHaveLength(1);
    expect(blocks[0]).toEqual({ kind: "paragraph", lines: ["| a | b |"] });
  });

  it("围栏代码块（含语言标记与未闭合围栏）", () => {
    const closed = parseMessagePreview(["```ts", "const a = 1;", "```"].join("\n"));
    expect(closed).toEqual([{ kind: "code", lang: "ts", text: "const a = 1;" }]);

    const unclosed = parseMessagePreview(["```", "line1", "line2"].join("\n"));
    expect(unclosed).toEqual([{ kind: "code", lang: "", text: "line1\nline2" }]);
  });

  it("有序 / 无序列表，混排时切开成两个列表", () => {
    expect(parseMessagePreview(["- a", "- b"].join("\n"))).toEqual([
      { kind: "list", ordered: false, items: ["a", "b"] },
    ]);
    expect(parseMessagePreview(["1. x", "2. y"].join("\n"))).toEqual([
      { kind: "list", ordered: true, items: ["x", "y"] },
    ]);
    expect(parseMessagePreview(["- a", "1. b"].join("\n")).map((block) => block.kind)).toEqual([
      "list",
      "list",
    ]);
  });

  it("标题级别只取 1~3，4 个 # 当普通段落", () => {
    const blocks = parseMessagePreview(["# 一级", "### 三级", "#### 不是标题"].join("\n"));
    expect(blocks[0]).toEqual({ kind: "heading", level: 1, text: "一级" });
    expect(blocks[1]).toEqual({ kind: "heading", level: 3, text: "三级" });
    expect(blocks[2]).toEqual({ kind: "paragraph", lines: ["#### 不是标题"] });
  });

  it("段落保留原始换行（不再被折叠成一行）", () => {
    const blocks = parseMessagePreview(["第一行", "第二行", "", "另一段"].join("\n"));
    expect(blocks).toEqual([
      { kind: "paragraph", lines: ["第一行", "第二行"] },
      { kind: "paragraph", lines: ["另一段"] },
    ]);
  });

  it("完整性：原文每个非空行都能在解析结果里找到（不丢内容）", () => {
    const samples = [
      REAL_SHAPE,
      ["普通段落", "- 列表项", "1. 有序项", "", "```js", "const x = 1;", "```", "收尾"].join("\n"),
      ["| a | b\\|c |", "|---|---|", "| 1 | 2 |", "文字尾巴"].join("\n"),
      ["| 无表头 | 表格 |", "| 1 | 2 |", "| 3 | 4 |"].join("\r\n"),
    ];
    for (const sample of samples) {
      const joined = normalize(blockTexts(parseMessagePreview(sample)));
      for (const line of sample.split(/\r?\n/)) {
        const expected = normalize(stripLeadingMarker(line));
        if (!expected) continue;
        expect(joined, `丢了这一行的内容：${line}`).toContain(expected);
      }
    }
  });
});
