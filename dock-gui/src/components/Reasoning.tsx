import { useEffect, useState } from "react";
import { ChevronRight } from "lucide-react";
import type { TurnItem } from "dock-core";
import { visibleReasoningText } from "dock-core";
import { prefs } from "../dock/prefs";
import { formatElapsed } from "./chrome";

/**
 * 模型的思考。想的时候：流光「正在思考 · Ns」，下面滚动露最新几行；想完：折成「思考了 Ns ›」，
 * 点开看全文。思考是给人看的旁白，字比正文小一号、灰一档。
 *
 * `compact`：子代理面板用（面板整体小一号，正文再小一号）。行为不变。
 */
export function Reasoning({
  item,
  compact,
}: {
  item: Extract<TurnItem, { kind: "reasoning" }>;
  compact?: boolean;
}) {
  const live = item.endedAt === null;
  // 设置 › 外观 › 默认展开思考过程。
  const [open, setOpen] = useState(() => prefs.get().expandReasoning);
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!live) return;
    const t = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(t);
  }, [live]);
  const elapsed = formatElapsed((item.endedAt ?? now) - item.startedAt);
  // 旧会话 / 跨 chunk 残片：item.text 里可能还带着 AQEA…</signature>，卡片只画摘要。
  const text = visibleReasoningText(item.text).trim();
  // 类名要写全（Tailwind 扫源码）：动态拼出来的类名会被 purge 掉。
  const buttonSize = compact ? "text-[13px]" : "text-[14px]";
  const bodySize = compact ? "text-[12px]" : "text-[13px]";
  return (
    <div className="enter-fade flex shrink-0 flex-col gap-1.5">
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        className={`group flex h-7 w-fit items-center gap-1 text-left text-ink-4 hover:text-ink-2 ${buttonSize}`}
      >
        {live ? (
          <>
            <span className="shimmer-text">正在思考</span>
            <span className="text-ink-5">· {elapsed}</span>
          </>
        ) : (
          <>思考了 {elapsed}</>
        )}
        <ChevronRight size={15} className="rotate-open" data-open={open} />
      </button>
      {open ? (
        <div
          className={`enter-fade whitespace-pre-wrap break-words border-l-2 border-line pl-3 leading-[1.6] text-ink-4 ${bodySize}`}
        >
          {text}
        </div>
      ) : (
        live &&
        text && (
          // 只露最新三行，上沿渐隐；新字进来底部对齐，像在往上滚。
          <div
            className={`flex max-h-[4.8em] flex-col justify-end overflow-hidden border-l-2 border-line pl-3 leading-[1.6] text-ink-4 [mask-image:linear-gradient(to_bottom,transparent,black_45%)] ${bodySize}`}
          >
            <div className="whitespace-pre-wrap break-words">{text}</div>
          </div>
        )
      )}
    </div>
  );
}
