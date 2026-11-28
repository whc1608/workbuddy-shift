import { useMemo, type ReactNode } from "react";

import { parseMessagePreview, type PreviewBlock } from "@/lib/message-preview";
import { cn } from "@/lib/utils";

/**
 * 关联会话详情里的消息预览渲染。
 *
 * 容器很窄（11px 字号、卡片内、外层 `max-h-56 overflow-y-auto`），所以：
 * - 表格一律 `w-max min-w-full` + 外层横向滚动，宁可滚动也不挤成一坨；
 * - 只用 token（`border-border/60`、`bg-background/40`、`text-foreground`），
 *   浅色与深色都跟主题走，不写死颜色；
 * - 段落用 `whitespace-pre-wrap`，模型回复里的换行不再被折叠成空格。
 */

/**
 * 行内标记：`**粗体**` 与 `` `代码` ``。
 *
 * `String.prototype.split` 带捕获组时会把命中项一并放进结果，正好可以直接映射成节点；
 * 其它 Markdown 标记（链接 / 斜体 / 图片）原样保留——预览只求「能读」，不做完整渲染。
 */
const INLINE_PATTERN = /(\*\*[^*\n]+\*\*|`[^`\n]+`)/g;

/** 把一行文本渲染成节点序列（粗体 / 行内代码 / 纯文本）。 */
function renderInline(text: string, keyPrefix: string): ReactNode[] {
  if (!text) return [];
  return text.split(INLINE_PATTERN).map((part, index) => {
    const key = `${keyPrefix}-${index}`;
    if (part.length > 4 && part.startsWith("**") && part.endsWith("**")) {
      return (
        <strong key={key} className="font-medium text-foreground">
          {part.slice(2, -2)}
        </strong>
      );
    }
    if (part.length > 2 && part.startsWith("`") && part.endsWith("`")) {
      return (
        <code key={key} className="rounded bg-background/60 px-1 font-mono text-[10px]">
          {part.slice(1, -1)}
        </code>
      );
    }
    return part;
  });
}

/** 标题字号：级别只影响视觉重量，`##` 本身不会再出现在文本里。 */
const HEADING_CLASS: Record<1 | 2 | 3, string> = {
  1: "text-xs font-semibold",
  2: "text-[11px] font-medium",
  3: "text-[11px] font-medium",
};

/** 单元格通用样式：不折行 + 细边（表格本身可横向滚动）。 */
const CELL_CLASS = "whitespace-nowrap border border-border/60 px-1.5 py-0.5 align-top";

function TableBlockView({ block }: { block: Extract<PreviewBlock, { kind: "table" }> }) {
  // 无表头兜底时，`head` 是第一行数据，要并进 tbody。
  const bodyRows = block.hasHeader ? block.rows : [block.head, ...block.rows];
  return (
    <div className="max-w-full overflow-x-auto">
      <table className="w-max min-w-full border-collapse text-[11px]">
        {block.hasHeader && (
          <thead>
            <tr className="bg-background/40">
              {block.head.map((cell, cellIndex) => (
                <th key={cellIndex} className={cn(CELL_CLASS, "text-left font-medium text-foreground")}>
                  {renderInline(cell, `th-${cellIndex}`)}
                </th>
              ))}
            </tr>
          </thead>
        )}
        <tbody>
          {bodyRows.map((row, rowIndex) => (
            <tr key={rowIndex}>
              {row.map((cell, cellIndex) => (
                <td key={cellIndex} className={CELL_CLASS}>
                  {renderInline(cell, `td-${rowIndex}-${cellIndex}`)}
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

/** 单个预览块的渲染。 */
function PreviewBlockView({ block }: { block: PreviewBlock }) {
  switch (block.kind) {
    case "table":
      return <TableBlockView block={block} />;
    case "code":
      return (
        <pre className="overflow-x-auto rounded bg-background/60 p-1.5 font-mono text-[10px] whitespace-pre">
          {block.text}
        </pre>
      );
    case "heading":
      return <p className={cn("leading-4 text-foreground", HEADING_CLASS[block.level])}>{renderInline(block.text, "h")}</p>;
    case "list": {
      const items = block.items.map((item, index) => (
        <li key={index} className="break-words">
          {renderInline(item, `li-${index}`)}
        </li>
      ));
      return block.ordered ? (
        <ol className="list-decimal space-y-0.5 pl-4">{items}</ol>
      ) : (
        <ul className="list-disc space-y-0.5 pl-4">{items}</ul>
      );
    }
    case "paragraph":
      return <p className="whitespace-pre-wrap break-words">{block.lines.join("\n")}</p>;
  }
}

/**
 * 消息预览：把一段原始 Markdown 文本渲染成块（表格 / 代码 / 标题 / 列表 / 段落）。
 *
 * 空文本渲染 `null`（不产出空 `<p>`，否则窄卡片里会多出一道空白）。
 */
export function MessagePreview({ text }: { text: string }) {
  const blocks = useMemo(() => parseMessagePreview(text), [text]);
  if (blocks.length === 0) return null;
  return (
    <div className="min-w-0 space-y-1.5">
      {blocks.map((block, index) => (
        <PreviewBlockView key={index} block={block} />
      ))}
    </div>
  );
}
