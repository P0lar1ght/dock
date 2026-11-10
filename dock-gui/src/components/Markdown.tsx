// 助手回复的 Markdown。字号行高照用户给的参考截图：15px 正文、段距宽松。
// 代码块走 Shiki 高亮（markdown/CodeBlock）、```mermaid 画成图、$…$ / $$…$$ 用 KaTeX 排公式。
// 回复切成顶层块分别 memo（markdown/blocks）：流式时只重渲染最后一块。

import { memo, useContext, useMemo, type CSSProperties, type ReactNode } from "react";
import ReactMarkdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import remarkMath from "remark-math";
import rehypeKatex from "rehype-katex";
import type { Element, ElementContent } from "hast";
import { Check } from "lucide-react";
import "katex/dist/katex.min.css";
import { CodeBlock, type CodeSize } from "./markdown/CodeBlock";
import { Mermaid } from "./markdown/Mermaid";
import { normalizeMath, splitBlocks } from "./markdown/blocks";
import { PathLinkContext } from "./markdown/pathLink";

const REMARK = [remarkGfm, remarkMath];
const REHYPE = [[rehypeKatex, { throwOnError: false, strict: "ignore" }]] as never;

function textOf(node: ElementContent): string {
  if (node.type === "text") return node.value;
  if (node.type === "element") return node.children.map(textOf).join("");
  return "";
}

/** `<pre><code class="language-x">` → 代码块 / Mermaid 图。 */
function pre(size: CodeSize) {
  return function Pre({ node, children }: { node?: Element; children?: ReactNode }) {
    const code = node?.children[0];
    if (code?.type !== "element" || code.tagName !== "code") return <pre>{children}</pre>;
    const classes = code.properties.className;
    const lang =
      (Array.isArray(classes) ? classes : [])
        .map(String)
        .find((c) => c.startsWith("language-"))
        ?.slice("language-".length)
        .toLowerCase() ?? "";
    const text = textOf(code).replace(/\n$/, "");
    if (lang === "mermaid") return <Mermaid code={text} size={size} />;
    return <CodeBlock code={text} lang={lang} size={size} />;
  };
}

const INLINE_CODE = "md-inline-code rounded-[5px] bg-[#efeff2] px-[0.35em] py-[0.12em] font-mono text-[0.86em] text-ink-2 wrap-anywhere";

/** 行内代码；写的是会话目录里的文件路径就能点，在文件面板里打开（对话流给了 PathLinkContext 才有）。 */
function InlineCode({ children }: { children?: ReactNode }) {
  const linker = useContext(PathLinkContext);
  const target = linker && typeof children === "string" ? linker.resolve(children) : null;
  if (!linker || !target) return <code className={INLINE_CODE}>{children}</code>;
  return (
    <code
      role="link"
      tabIndex={0}
      title={`在文件面板中打开${target.line ? `（第 ${target.line} 行）` : ""}`}
      onClick={() => linker.open(target)}
      onKeyDown={(e) => {
        if (e.key === "Enter") linker.open(target);
      }}
      className={`${INLINE_CODE} motion-colors cursor-pointer !text-accent underline decoration-accent/30 underline-offset-[3px] hover:bg-[#e4eefb] hover:decoration-accent`}
    >
      {children}
    </code>
  );
}

const align = (style?: CSSProperties) => (style?.textAlign ? { textAlign: style.textAlign } : undefined);

const components: Components = {
  p: ({ children }) => <p className="my-3 first:mt-0 last:mb-0">{children}</p>,
  ul: ({ children, className }) =>
    className?.includes("contains-task-list") ? (
      <ul className="my-3 list-none pl-1 first:mt-0 last:mb-0 [&_ul]:my-1">{children}</ul>
    ) : (
      <ul className="my-3 list-disc pl-6 marker:text-ink-4 first:mt-0 last:mb-0 [&_ol]:my-1 [&_ul]:my-1">{children}</ul>
    ),
  ol: ({ children, start }) => (
    <ol start={start} className="my-3 list-decimal pl-6 marker:text-ink-4 first:mt-0 last:mb-0 [&_ol]:my-1 [&_ul]:my-1">
      {children}
    </ol>
  ),
  li: ({ children, className }) =>
    className?.includes("task-list-item") ? (
      // 勾掉的项淡一档（勾选框是直接子节点；松散列表里在 p 下面）。
      <li className="relative my-1 list-none pl-6 has-[>[data-checked=true]]:text-ink-4 has-[>p>[data-checked=true]]:text-ink-4">
        {children}
      </li>
    ) : (
      <li className="my-1 pl-1">{children}</li>
    ),
  input: ({ checked, type }) =>
    type === "checkbox" ? (
      <span
        data-checked={!!checked}
        className={`absolute left-0 top-[0.36em] flex size-[15px] items-center justify-center rounded-[4px] border ${
          checked ? "border-accent bg-accent text-white" : "border-[#c7c7cc] bg-white"
        }`}
      >
        {checked && <Check size={11} strokeWidth={3} />}
      </span>
    ) : null,
  a: ({ children, href }) => (
    <a
      href={href}
      target="_blank"
      rel="noreferrer"
      className="text-accent underline decoration-accent/30 underline-offset-[3px] hover:decoration-accent"
    >
      {children}
    </a>
  ),
  strong: ({ children }) => <strong className="font-semibold text-ink">{children}</strong>,
  h1: ({ children }) => <h2 className="mb-3 mt-7 text-[20px] font-semibold leading-snug text-ink first:mt-0">{children}</h2>,
  h2: ({ children }) => <h3 className="mb-2.5 mt-6 text-[17.5px] font-semibold leading-snug text-ink first:mt-0">{children}</h3>,
  h3: ({ children }) => <h4 className="mb-2 mt-5 text-[15.5px] font-semibold leading-snug text-ink first:mt-0">{children}</h4>,
  h4: ({ children }) => <h4 className="mb-1.5 mt-4 font-semibold leading-snug text-ink first:mt-0">{children}</h4>,
  blockquote: ({ children }) => (
    <blockquote className="md-quote my-4 rounded-r-lg border-l-[3px] border-[#d1d1d6] bg-[#f5f5f7] px-4 py-2.5 text-ink-3 first:mt-0 last:mb-0">
      {children}
    </blockquote>
  ),
  table: ({ children }) => (
    <div className="md-surface thin-scroll my-4 overflow-x-auto rounded-[10px] border border-line first:mt-0 last:mb-0">
      <table className="w-full border-collapse text-[13.5px] leading-normal tabular-nums [&_tbody_tr:first-child_td]:border-t-0">
        {children}
      </table>
    </div>
  ),
  thead: ({ children }) => <thead className="md-surface-head bg-[#f5f5f7]">{children}</thead>,
  th: ({ children, style }) => (
    <th style={align(style)} className="border-b border-line px-3 py-2 text-left font-semibold text-ink-2">
      {children}
    </th>
  ),
  td: ({ children, style }) => (
    <td style={align(style)} className="border-t border-line-soft px-3 py-2 align-top">
      {children}
    </td>
  ),
  hr: () => <hr className="my-6 border-0 border-t border-line" />,
  img: ({ src, alt }) => <img src={typeof src === "string" ? src : undefined} alt={alt ?? ""} className="my-2 max-w-full rounded-lg border border-line" />,
  pre: pre("body"),
  code: ({ children, className }) =>
    className ? <code className={className}>{children}</code> : <InlineCode>{children}</InlineCode>,
};

/** 卡片里的小号排版（计划卡）：照 wonder-ui/chat/PlanCard（jeel / khat）。 */
const compact: Components = {
  ...components,
  p: ({ children }) => <p className="my-2 first:mt-0 last:mb-0">{children}</p>,
  ul: ({ children, className }) =>
    className?.includes("contains-task-list") ? (
      <ul className="my-2 list-none pl-0.5 first:mt-0 last:mb-0">{children}</ul>
    ) : (
      <ul className="my-2 list-disc pl-5 marker:text-ink-4 first:mt-0 last:mb-0">{children}</ul>
    ),
  ol: ({ children, start }) => (
    <ol start={start} className="my-2 list-decimal pl-5 marker:text-ink-4 first:mt-0 last:mb-0">
      {children}
    </ol>
  ),
  li: ({ children, className }) =>
    className?.includes("task-list-item") ? (
      <li className="relative my-1 list-none pl-5 [&>span[data-checked]]:top-[0.15em] [&>span[data-checked]]:size-3">{children}</li>
    ) : (
      <li className="my-1 pl-0.5">{children}</li>
    ),
  h1: ({ children }) => <h3 className="mb-2 mt-3 text-[15px] font-bold leading-snug text-ink first:mt-0">{children}</h3>,
  h2: ({ children }) => <h4 className="mb-1.5 mt-3 text-[13.5px] font-bold leading-snug text-ink first:mt-0">{children}</h4>,
  h3: ({ children }) => <h4 className="mb-1.5 mt-3 text-[13.5px] font-bold leading-snug text-ink first:mt-0">{children}</h4>,
  h4: ({ children }) => <h4 className="mb-1.5 mt-3 font-bold leading-snug text-ink first:mt-0">{children}</h4>,
  blockquote: ({ children }) => (
    <blockquote className="my-2 border-l-2 border-line pl-3 text-ink-3 first:mt-0 last:mb-0">{children}</blockquote>
  ),
  hr: () => <hr className="my-3 border-0 border-t border-line" />,
  pre: pre("small"),
  code: ({ children, className }) =>
    className ? (
      <code className={className}>{children}</code>
    ) : (
      <code className="md-inline-code rounded-sm border border-line-strong bg-fill px-1 font-mono text-[11px] text-ink-3 wrap-anywhere">{children}</code>
    ),
};

/** 插件视图树里的 markdown 节点：照 wonder-ui/plugins/PluginViewGallery（hum）。 */
const view: Components = {
  ...compact,
  h1: ({ children }) => <h3 className="mb-1.5 mt-3 text-sm font-semibold text-ink first:mt-0">{children}</h3>,
  h2: ({ children }) => <h4 className="mb-1.5 mt-3 text-sm font-semibold text-ink first:mt-0">{children}</h4>,
  h3: ({ children }) => <h4 className="mb-1.5 mt-3 text-[13px] font-semibold text-ink first:mt-0">{children}</h4>,
  code: ({ children, className }) =>
    className ? (
      <code className={className}>{children}</code>
    ) : (
      <code className="md-inline-code rounded-sm bg-[#f2f2f7] px-1 font-mono text-xs text-ink wrap-anywhere">{children}</code>
    ),
};

const SIZES = {
  // 正文字号读 --chat-text：侧边聊天这种窄面板调小一号（Transcript 的 compact）。
  body: { className: "font-chat text-[length:var(--chat-text,15px)] leading-[1.75] text-ink", components },
  card: { className: "text-xs leading-normal text-ink-2", components: compact },
  view: { className: "text-[13px] leading-[1.55] text-ink", components: view },
};

type Size = keyof typeof SIZES;

const Block = memo(function Block({ text, size }: { text: string; size: Size }) {
  return (
    <ReactMarkdown remarkPlugins={REMARK} rehypePlugins={REHYPE} components={SIZES[size].components}>
      {text}
    </ReactMarkdown>
  );
});

export function Markdown({
  text,
  size = "body",
  live = false,
}: {
  text: string;
  size?: Size;
  /** 还在流：末尾挂一根闪烁的光标。 */
  live?: boolean;
}) {
  const blocks = useMemo(() => splitBlocks(normalizeMath(text)), [text]);
  return (
    <div className={`${SIZES[size].className} ${live ? "md-live" : ""}`}>
      {blocks.map((block, i) => (
        <Block key={i} text={block} size={size} />
      ))}
    </div>
  );
}
