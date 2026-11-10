// 主会话里「来自侧边聊天的笔记」：侧边聊天写进主线的结论（模型看得到）。
// 设计：wonder-ui/chat/SideChat/SideNoteCard{Collapsed,Expanded}（aal / two）。
// 默认收起：标题 + 第一行摘要；点开看整段（markdown），底部写入时间。

import { useState } from "react";
import { ChevronDown, MessageSquare } from "lucide-react";
import { Markdown } from "./Markdown";

export function SideNoteCard({ text, at }: { text: string; at: number }) {
  const [open, setOpen] = useState(false);
  const summary = text
    .split("\n")
    // 摘要是一行纯文本：去掉列表符号和 markdown 记号（反引号、粗体星号）。
    .map((l) => l.replace(/^\s*(?:[-*•]|\d+[.)])\s+/, "").replace(/[`*]/g, "").trim())
    .find(Boolean);
  const time = at ? new Date(at).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", hour12: false }) : null;
  return (
    <div className="md-surface enter-fade flex w-full shrink-0 flex-col overflow-hidden rounded-[10px] border border-[#e5e5ea] bg-white">
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        className="motion-colors flex items-center gap-2 px-3 py-2.5 text-left hover:bg-hover"
        aria-expanded={open}
      >
        <MessageSquare size={14} className="shrink-0 text-ink-3" />
        <span className="shrink-0 text-[13px] font-medium text-ink">来自侧边聊天的笔记</span>
        <span
          className={`min-w-0 transition-opacity duration-[var(--dur-base)] flex-1 truncate text-[13px] text-ink-4 ${open ? "opacity-0" : "opacity-100"}`}
        >
          {summary}
        </span>
        <ChevronDown
          size={14}
          className="shrink-0 text-ink-4 transition-transform duration-[var(--dur-base)]"
          style={{ transform: open ? "rotate(180deg)" : undefined }}
        />
      </button>
      <div className="fold" data-open={open}>
        <div>
          <div className="border-t border-[#ececf0] px-3 py-2.5">
            <Markdown text={text} size="view" />
          </div>
          {time && <div className="px-3 pb-2.5 text-[11px] text-ink-4">写入于 {time} · 模型已看到</div>}
        </div>
      </div>
    </div>
  );
}
