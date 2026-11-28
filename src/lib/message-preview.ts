/**
 * 关联会话详情的「最近内容预览」解析（纯逻辑，无 React / DOM 依赖）。
 *
 * 为什么需要它：预览文本直接来自模型回复的原始 Markdown（后端只做注入块清理 + 截断），
 * 里面常见的表格 `| 批次 | 内容 | … |` 与分隔行 `|---|---|` 如果当纯文本渲染，
 * 列宽会完全塌陷、整段挤成一坨，用户看不出这是一张表。这里把常见的几种块
 * （表格 / 代码 / 标题 / 列表 / 段落）切出来，交给 `@/components/message-preview` 渲染。
 *
 * 三条硬约束：
 * 1. **不丢内容**：未识别的行一律进段落块，原文的每一行都必须出现在某个块里；
 * 2. **截断容错**：预览在 240 / 480 字符处被硬切，表格经常缺失分隔行甚至只剩半行，
 *    这时按「无表头表格」兜底，而不是退回纯文本；
 * 3. **列数对齐**：模型输出的表格行列数常常不齐，一律以表头列数为准补空串 / 截断。
 */

/** 预览块（判别联合；`kind` 决定渲染方式）。 */
export type PreviewBlock =
  /** 段落：`lines` 保留原始换行与缩进，渲染时用 `whitespace-pre-wrap`。 */
  | { kind: "paragraph"; lines: string[] }
  /** 标题：`#` / `##` / `###`，级别只影响字号。 */
  | { kind: "heading"; level: 1 | 2 | 3; text: string }
  /** 列表：`- ` / `* ` 与 `1. `。 */
  | { kind: "list"; ordered: boolean; items: string[] }
  /** 围栏代码块（含未闭合围栏）。 */
  | { kind: "code"; lang: string; text: string }
  /**
   * 表格：`head` 是表头（无表头兜底时是首行数据），`rows` 是数据行。
   * `hasHeader === false` 表示没找到分隔行（预览被截断），渲染时不出 `<thead>`。
   */
  | { kind: "table"; head: string[]; rows: string[][]; hasHeader: boolean };

/** 标题：1~3 个 `#` + 至少一个空白（4 个 `#` 不算标题，按段落保留）。 */
const HEADING_RE = /^(#{1,3})\s+(.+)$/;

/** 无序列表项：`- ` / `* ` / `+ `（`**粗体**` 开头不会命中：第二个字符不是空白）。 */
const BULLET_RE = /^[-*+]\s+(.*)$/;

/** 有序列表项：`1. ` / `1) `。 */
const ORDERED_RE = /^\d{1,3}[.)]\s+(.*)$/;

/**
 * Markdown 表格分隔行：`|---|---|`、`| --- | :--- |`、`--- | ---` 等。
 * 至少两列（只有一列时不算表格），首尾竖线可选。
 */
const TABLE_SEPARATOR_RE = /^\|?\s*:?-{3,}:?\s*(?:\|\s*:?-{3,}:?\s*)+\|?$/;

/** 该行是否是围栏（```）开头。 */
function isFence(line: string): boolean {
  return line.trim().startsWith("```");
}

/** 围栏语言标记（```ts → "ts"；无语言返回空串）。 */
function fenceLang(line: string): string {
  const rest = line.trim().slice(3).trim();
  return rest.split(/\s+/)[0] ?? "";
}

/** 标题行 → `{ level, text }`；不是标题返回 `null`。 */
function matchHeading(line: string): { level: 1 | 2 | 3; text: string } | null {
  const matched = HEADING_RE.exec(line.trim());
  if (!matched) return null;
  const level = Math.min(matched[1].length, 3) as 1 | 2 | 3;
  return { level, text: matched[2].trim() };
}

/** 列表项 → `{ ordered, text }`；不是列表项返回 `null`。 */
function matchListItem(line: string): { ordered: boolean; text: string } | null {
  const trimmed = line.trim();
  const ordered = ORDERED_RE.exec(trimmed);
  if (ordered) return { ordered: true, text: ordered[1].trim() };
  const bullet = BULLET_RE.exec(trimmed);
  if (bullet) return { ordered: false, text: bullet[1].trim() };
  return null;
}

/**
 * 按未转义的竖线切单元格，并剥掉首尾的可选竖线。
 *
 * `\|` 是单元格内的字面竖线（模型输出里表示「a 或 b」时常见），不能当列分隔。
 */
function splitRow(line: string): string[] {
  const trimmed = line.trim();
  const start = trimmed.startsWith("|") ? 1 : 0;
  let end = trimmed.length;
  // 结尾的 `|` 是表格边框；`\|` 结尾时它是内容，不能剥。
  if (end > start && trimmed.endsWith("|") && !trimmed.endsWith("\\|")) end -= 1;
  const body = trimmed.slice(start, end);
  const cells: string[] = [];
  let current = "";
  for (let index = 0; index < body.length; index += 1) {
    const char = body[index];
    if (char === "\\" && body[index + 1] === "|") {
      current += "|";
      index += 1;
      continue;
    }
    if (char === "|") {
      cells.push(current.trim());
      current = "";
      continue;
    }
    current += char;
  }
  cells.push(current.trim());
  return cells;
}

/**
 * 从 `start` 开始、连续「含竖线的非空行」到哪结束（不含该下标）。
 *
 * 预览被截断时表格常以半行收尾，所以这里不做语法校验，只按「连续且含 `|`」收口。
 */
function tableRunEnd(lines: string[], start: number): number {
  let end = start;
  while (end < lines.length) {
    const trimmed = lines[end].trim();
    if (!trimmed) break;
    if (isFence(trimmed) || HEADING_RE.test(trimmed)) break;
    if (!trimmed.includes("|")) break;
    end += 1;
  }
  return end;
}

/** 数据行按表头列数对齐：短行补空串，长行截断（模型输出常见）。 */
function alignRows(head: string[], rows: string[][]): string[][] {
  const width = Math.max(1, head.length);
  return rows.map((row) => {
    if (row.length === width) return row;
    if (row.length > width) return row.slice(0, width);
    return [...row, ...Array.from({ length: width - row.length }, () => "")];
  });
}

/** 从 `start` 尝试解析一张表格；不是表格返回 `null`（调用方按段落处理）。 */
function matchTable(lines: string[], start: number): { block: PreviewBlock; next: number } | null {
  const end = tableRunEnd(lines, start);
  // 只有一行含 `|`：不够成表，按段落保留（否则一句「a | b」会被渲染成单行表格）。
  if (end - start < 2) return null;
  const rows = lines.slice(start, end).map((line) => line.trim());
  const head = splitRow(rows[0]);
  if (head.length < 2) return null;

  // 标准表格：第二行是分隔行，分隔行不进 rows。
  if (TABLE_SEPARATOR_RE.test(rows[1])) {
    const dataRows = rows
      .slice(2)
      .filter((row) => !TABLE_SEPARATOR_RE.test(row))
      .map(splitRow);
    return {
      block: { kind: "table", head, rows: alignRows(head, dataRows), hasHeader: true },
      next: end,
    };
  }

  // 截断兜底：连续 ≥2 行、每行都以 `|` 开头且 ≥2 列 —— 表头当首行数据用，不出 <thead>。
  const looksLikeTable = rows.every((row) => row.startsWith("|") && splitRow(row).length >= 2);
  if (!looksLikeTable) return null;
  return {
    block: { kind: "table", head, rows: alignRows(head, rows.slice(1).map(splitRow)), hasHeader: false },
    next: end,
  };
}

/** 从 `start` 尝试解析一个列表；不是列表返回 `null`。 */
function matchList(lines: string[], start: number): { block: PreviewBlock; next: number } | null {
  const first = matchListItem(lines[start]);
  if (!first) return null;
  const items: string[] = [];
  let index = start;
  while (index < lines.length) {
    const item = matchListItem(lines[index]);
    // 有序与无序混排时切开成两个列表，避免 `1.` 混进 `-` 的块里被渲染成圆点。
    if (!item || item.ordered !== first.ordered) break;
    items.push(item.text);
    index += 1;
  }
  if (items.length === 0) return null;
  return { block: { kind: "list", ordered: first.ordered, items }, next: index };
}

/** 该下标处是否开始了一个非段落块（段落收集时用来收口）。 */
function isBlockStart(lines: string[], index: number): boolean {
  const trimmed = lines[index].trim();
  if (!trimmed) return true;
  if (isFence(trimmed)) return true;
  if (HEADING_RE.test(trimmed)) return true;
  if (matchListItem(lines[index])) return true;
  return matchTable(lines, index) !== null;
}

/**
 * 把预览文本切成块序列。
 *
 * 空行只用来分块，不产生块；空文本返回空数组（调用方据此不渲染容器）。
 */
export function parseMessagePreview(text: string): PreviewBlock[] {
  if (!text) return [];
  const lines = text.split(/\r?\n/);
  const blocks: PreviewBlock[] = [];
  let index = 0;

  while (index < lines.length) {
    const trimmed = lines[index].trim();
    if (!trimmed) {
      index += 1;
      continue;
    }

    if (isFence(trimmed)) {
      const lang = fenceLang(trimmed);
      const body: string[] = [];
      index += 1;
      while (index < lines.length && !isFence(lines[index])) {
        body.push(lines[index]);
        index += 1;
      }
      // 未闭合围栏（预览被截断）：吃到文末同样收口，内容原样保留。
      if (index < lines.length) index += 1;
      blocks.push({ kind: "code", lang, text: body.join("\n") });
      continue;
    }

    const heading = matchHeading(trimmed);
    if (heading) {
      blocks.push({ kind: "heading", level: heading.level, text: heading.text });
      index += 1;
      continue;
    }

    const table = matchTable(lines, index);
    if (table) {
      blocks.push(table.block);
      index = table.next;
      continue;
    }

    const list = matchList(lines, index);
    if (list) {
      blocks.push(list.block);
      index = list.next;
      continue;
    }

    const paragraph: string[] = [];
    while (index < lines.length) {
      const current = lines[index].trim();
      if (!current) break;
      // 首行一定进段落（上面已排除其它块），之后遇到别的块就收口。
      if (paragraph.length > 0 && isBlockStart(lines, index)) break;
      paragraph.push(lines[index]);
      index += 1;
    }
    blocks.push({ kind: "paragraph", lines: paragraph });
  }

  return blocks;
}
