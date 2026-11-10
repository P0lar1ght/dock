// 角色提示词编辑框（设计 moyo）：行号栏 + 标题着色的 Markdown 编辑（透明 textarea 叠在
// 着色层上），「编辑 Markdown / 渲染预览」切换，字符数 / 行数，复制。

import { useRef, useState, type UIEvent } from "react";
import { Check, Copy } from "lucide-react";
import { Markdown } from "../Markdown";

const LINE = "leading-[1.65]";

export function PromptEditor({
  value,
  onChange,
  placeholder,
  footer,
  invalid = false,
  actions,
}: {
  value: string;
  onChange: (v: string) => void;
  placeholder: string;
  footer: React.ReactNode;
  invalid?: boolean;
  /** 工具栏右侧额外的按钮（AI 润色 / 扩写）。 */
  actions?: React.ReactNode;
}) {
  const [tab, setTab] = useState<"edit" | "preview">("edit");
  const [copied, setCopied] = useState(false);
  const layer = useRef<HTMLDivElement>(null);
  const lines = value.split("\n");
  const chars = [...value].length;

  // 着色层（带行号）跟着 textarea 滚。
  const onScroll = (e: UIEvent<HTMLTextAreaElement>) => {
    const { scrollTop, scrollLeft } = e.currentTarget;
    if (layer.current) {
      layer.current.scrollTop = scrollTop;
      layer.current.scrollLeft = scrollLeft;
    }
  };
  const copy = () => {
    navigator.clipboard.writeText(value).then(() => {
      setCopied(true);
      setTimeout(() => setCopied(false), 1200);
    });
  };

  return (
    <div
      className={`flex min-h-[360px] flex-1 flex-col overflow-hidden rounded-xl border bg-white shadow-sm ${
        invalid ? "border-[#ff3b30]" : "border-[#d1d1d6]"
      }`}
    >
      <div className="flex h-9 shrink-0 items-center justify-between border-b border-line-strong bg-[#f7f7f8] px-3.5">
        <div className="flex items-center gap-3">
          <div className="flex gap-0.5 rounded-md bg-line-strong p-0.5">
            {(["edit", "preview"] as const).map((t) => (
              <button
                key={t}
                type="button"
                onClick={() => setTab(t)}
                className={`motion-colors rounded-sm px-2.5 py-0.5 text-[11.5px] ${
                  tab === t ? "bg-white font-semibold text-ink shadow-sm" : "font-medium text-ink-3 hover:text-ink"
                }`}
              >
                {t === "edit" ? "编辑 Markdown" : "渲染预览"}
              </button>
            ))}
          </div>
          <span className="font-mono text-[11.5px] text-ink-4">Mono / SF Pro Text</span>
        </div>
        <div className="flex items-center gap-3 text-[11.5px] text-ink-4">
          {actions}
          <span className="font-mono">
            {chars} 字符 · {lines.length} 行
          </span>
          <button type="button" onClick={copy} disabled={!value} className="flex items-center gap-1 text-accent disabled:opacity-40">
            {copied ? <Check size={12} className="pop-in" /> : <Copy size={12} />}
            {copied ? "已复制" : "复制"}
          </button>
        </div>
      </div>

      {tab === "edit" ? (
        <div className="relative min-h-0 flex-1 p-3.5 font-mono text-[12.5px]">
          {/* 行号跟着各自那一行（折行时停在首行），所以放在着色层里、和正文同一个格子。 */}
          <div className="relative h-full">
            <div
              ref={layer}
              aria-hidden
              className={`pointer-events-none absolute inset-0 overflow-hidden text-ink ${LINE}`}
            >
              {lines.map((l, i) => (
                <div key={i} className="flex">
                  <span className="w-8 shrink-0 border-r border-[#f2f2f7] pr-2 text-right text-xs text-ink-5">
                    {String(i + 1).padStart(2, "0")}
                  </span>
                  <span className={`min-h-[1.65em] min-w-0 flex-1 whitespace-pre-wrap break-words pl-3 ${lineStyle(l)}`}>
                    {l || "\u200b"}
                  </span>
                </div>
              ))}
            </div>
            <textarea
              value={value}
              onChange={(e) => onChange(e.target.value)}
              onScroll={onScroll}
              spellCheck={false}
              placeholder={placeholder}
              className={`absolute inset-0 h-full w-full resize-none overflow-y-auto bg-transparent pl-[44px] text-transparent caret-ink outline-none select-text placeholder:text-ink-5 ${LINE}`}
            />
          </div>
        </div>
      ) : (
        <div className="thin-scroll enter-fade min-h-0 flex-1 overflow-y-auto px-5 py-4">
          {value.trim() ? <Markdown text={value} /> : <span className="text-[13px] text-ink-4">还没有内容。</span>}
        </div>
      )}

      <div className="flex shrink-0 items-center justify-between border-t border-line-soft bg-[#fbfbfb] px-3.5 py-1.5 text-[11.5px] text-ink-4">
        {footer}
      </div>
    </div>
  );
}

/** 标题行着色（设计：一级蓝、二级以下橙）。 */
function lineStyle(line: string): string {
  if (/^#\s/.test(line)) return "font-bold text-accent";
  if (/^#{2,6}\s/.test(line)) return "font-semibold text-[#d97706]";
  return "";
}
