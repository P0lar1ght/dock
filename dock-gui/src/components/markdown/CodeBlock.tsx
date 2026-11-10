// 回复里的代码块：头栏（语言 · 换行 · 复制）+ Shiki 高亮的正文。
// 高亮没好时先画原文，排版一样，好了只换颜色，不跳。

import { useState, type CSSProperties, type ReactNode } from "react";
import { Check, Copy, WrapText } from "lucide-react";
import type { ThemedToken } from "shiki";
import { usePrefs } from "../../dock/prefs";
import { useHighlight } from "./highlight";

export type CodeSize = "body" | "small";

/** 高亮主题的底色是不是深色（#rgb / #rrggbb；认不出当浅色）。 */
export function isDark(hex: string): boolean {
  const m = /^#([0-9a-f]{3}|[0-9a-f]{6})/i.exec(hex);
  if (!m) return false;
  const h = m[1].length === 3 ? [...m[1]].map((c) => c + c).join("") : m[1];
  const [r, g, b] = [0, 2, 4].map((i) => parseInt(h.slice(i, i + 2), 16) / 255);
  return 0.2126 * r + 0.7152 * g + 0.0722 * b < 0.5;
}

const PRE: Record<CodeSize, string> = {
  body: "px-4 py-3 text-[13px] leading-[1.6]",
  small: "px-3 py-2 text-[11px] leading-[1.55]",
};

export function CodeBlock({
  code,
  lang,
  size = "body",
  actions,
  children,
}: {
  code: string;
  lang: string;
  size?: CodeSize;
  /** 头栏右侧额外的按钮（Mermaid 的「图 / 源码」）。 */
  actions?: ReactNode;
  /** 代替代码正文画的内容（Mermaid 的图）。 */
  children?: ReactNode;
}) {
  const { codeTheme } = usePrefs();
  const [wrap, setWrap] = useState(false);
  const shown = children === undefined;
  const hl = useHighlight(shown ? code : "", lang, codeTheme);
  return (
    <div className="md-surface my-3 overflow-hidden rounded-xl border border-line first:mt-0 last:mb-0">
      <div className="md-surface-head flex h-8 items-center gap-1 border-b border-line bg-[#f5f5f7] pl-3 pr-1.5 text-[11.5px] text-ink-4 select-none">
        <span className="min-w-0 flex-1 truncate font-mono">{lang || "text"}</span>
        {actions}
        {shown && (
          <HeaderButton title={wrap ? "不换行" : "自动换行"} active={wrap} onClick={() => setWrap((w) => !w)}>
            <WrapText size={13} />
          </HeaderButton>
        )}
        <CopyButton text={code} />
      </div>
      {shown ? (
        <pre
          className={`md-code thin-scroll overflow-x-auto font-mono text-ink-2 ${PRE[size]} ${wrap ? "whitespace-pre-wrap break-words" : ""}`}
          // 底色走变量：玻璃材质下 styles.css 把它调半透（深色主题留得厚，字才读得清）。
          style={{ "--code-bg": hl?.bg || "#fbfbfc" } as CSSProperties}
          data-dark={isDark(hl?.bg || "") || undefined}
        >
          <code>{hl && code.startsWith(hl.code) ? <Tokens lines={hl.lines} tail={code.slice(hl.code.length)} /> : code}</code>
        </pre>
      ) : (
        children
      )}
    </div>
  );
}

/** 高亮好的行；`tail` 是流式时高亮之后新到的字，先按原文接上。 */
export function Tokens({ lines, tail }: { lines: ThemedToken[][]; tail: string }) {
  return (
    <>
      {lines.map((line, i) => (
        <span key={i}>
          {line.map((t, j) => (
            <span key={j} style={tokenStyle(t)}>
              {t.content}
            </span>
          ))}
          {i < lines.length - 1 && "\n"}
        </span>
      ))}
      {tail}
    </>
  );
}

function tokenStyle(t: ThemedToken): React.CSSProperties {
  const style: React.CSSProperties = { color: t.color };
  const f = t.fontStyle ?? 0;
  if (f & 1) style.fontStyle = "italic";
  if (f & 2) style.fontWeight = 600;
  if (f & 4) style.textDecoration = "underline";
  return style;
}

export function HeaderButton({
  title,
  active = false,
  onClick,
  children,
}: {
  title: string;
  active?: boolean;
  onClick: () => void;
  children: ReactNode;
}) {
  return (
    <button
      type="button"
      title={title}
      aria-label={title}
      aria-pressed={active}
      onClick={onClick}
      className={`motion-colors flex h-6 items-center gap-1 rounded-md px-1.5 hover:bg-hover hover:text-ink-2 ${
        active ? "text-accent" : ""
      }`}
    >
      {children}
    </button>
  );
}

function CopyButton({ text }: { text: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <HeaderButton
      title="复制"
      onClick={() =>
        void navigator.clipboard.writeText(text).then(() => {
          setCopied(true);
          window.setTimeout(() => setCopied(false), 1200);
        })
      }
    >
      {copied ? <Check size={13} className="pop-in text-[#34c759]" /> : <Copy size={13} />}
      <span>{copied ? "已复制" : "复制"}</span>
    </HeaderButton>
  );
}
