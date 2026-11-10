// 回复切成顶层块（段落、列表、代码块、表格…）分别渲染：流式时只有最后一块在变，
// 前面的块 memo 住，不再重跑 Markdown / KaTeX / 高亮。

import { unified } from "unified";
import remarkParse from "remark-parse";
import remarkGfm from "remark-gfm";
import remarkMath from "remark-math";

const parser = unified().use(remarkParse).use(remarkGfm).use(remarkMath);

/**
 * 按顶层节点的源码位置切块。有链接定义 / 脚注时整段不切（它们跨块引用，切开就断了）。
 */
export function splitBlocks(text: string): string[] {
  const tree = parser.parse(text);
  if (tree.children.some((n) => n.type === "definition" || n.type === "footnoteDefinition")) return [text];
  const out: string[] = [];
  for (const node of tree.children) {
    const start = node.position?.start.offset;
    const end = node.position?.end.offset;
    if (start === undefined || end === undefined) return [text];
    out.push(text.slice(start, end));
  }
  return out;
}

/**
 * 模型常用 `\[ … \]` / `\( … \)` 写公式，remark-math 只认 `$$` / `$`：换成后者。
 * 代码块、行内代码里的不动。
 */
export function normalizeMath(text: string): string {
  if (!text.includes("\\[") && !text.includes("\\(")) return text;
  return text
    .split(/(^[ \t]*(?:```|~~~)[^\n]*\n[\s\S]*?(?:^[ \t]*(?:```|~~~)[ \t]*$|(?![\s\S])))/m)
    .map((part, i) =>
      i % 2 === 1
        ? part
        : part.replace(/(`+)[\s\S]*?\1|\\\[([\s\S]+?)\\\]|\\\(([\s\S]+?)\\\)/g, (m, tick, display, inline) => {
            if (tick) return m;
            if (display !== undefined) return `\n$$\n${display.trim()}\n$$\n`;
            return `$${inline.trim()}$`;
          }),
    )
    .join("");
}
