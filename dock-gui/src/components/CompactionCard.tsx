// 对话流末尾的压缩卡：正在压缩、重试、没压成、被停掉。压完了收成一行 FeedCompressedNote
// （Transcript 的 compaction 项）。设计：wonder-ui/chat/FeedCompaction（Feed Compaction States，tarr）。
//
// 压缩那几秒没有流式输出，这张卡是唯一在动的东西：标题走一道光，进度条是不确定的扫光
// （摘要多长事先不知道，不给假百分比），右边计时 0.1s 一跳。

import { useRef } from "react";
import { AlertCircle, Check, CircleSlash, LoaderCircle, Shrink, TriangleAlert } from "lucide-react";
import type { CompactionPhase, CompactionProgress } from "dock-core";
import { formatTokens } from "./chrome";
import { Ticker, formatSeconds } from "./ToolCard";

const PHASES: { id: CompactionPhase; label: string }[] = [
  { id: "memory", label: "整理记忆" },
  { id: "summary", label: "生成摘要" },
  { id: "apply", label: "替换历史" },
];

export function CompactionCard({ progress, onRetry }: { progress: CompactionProgress; onRetry?: () => void }) {
  if (progress.status === "failed") return <Failed progress={progress} onRetry={onRetry} />;
  if (progress.status === "cancelled") {
    return (
      <div className="enter-fade flex shrink-0 items-center gap-1.5 text-[11.5px] text-[#8e8e93]">
        <CircleSlash size={12} className="shrink-0" />
        上下文压缩已停止 · 当前上下文已保留
      </div>
    );
  }
  if (progress.status !== "running") return null;
  return <Running progress={progress} />;
}

function Running({ progress }: { progress: CompactionProgress }) {
  // 推送只在换阶段、摘要又写了一段时来；计时按第一次见到它时倒推的起点自己走。
  const since = useRef(Date.now() - progress.elapsedMs);
  const current = PHASES.findIndex((p) => p.id === progress.phase);
  const meta =
    progress.phase === "memory"
      ? "写入记忆…"
      : progress.phase === "apply"
        ? "替换历史…"
        : `已写 ${formatTokens(progress.outputTokens)} tokens`;
  return (
    <div className="md-surface enter-up flex shrink-0 flex-col gap-3 rounded-[10px] border border-[#e5e5ea] bg-[#f5f5f7] px-3.5 py-3">
      <div className="flex items-center justify-between gap-2">
        <div className="flex min-w-0 items-center gap-2.5">
          <span className="flex size-6 shrink-0 items-center justify-center rounded-md bg-[#e8f1ff] text-accent">
            <Shrink size={14} />
          </span>
          <span className="shimmer-title text-xs font-semibold">正在压缩上下文</span>
          <span className="shrink-0 rounded-sm bg-[#e8e8ec] px-1.5 py-0.5 text-[11px] text-ink-3">
            {progress.trigger === "auto" ? "自动" : "手动"}
          </span>
        </div>
        <div className="flex shrink-0 items-center gap-2">
          <Ticker since={since.current} />
          <LoaderCircle size={14} className="animate-spin text-accent" />
        </div>
      </div>
      <div className="flex items-center gap-2">
        {PHASES.map((phase, i) => (
          <Step key={phase.id} label={phase.label} state={i < current ? "done" : i === current ? "now" : "todo"} last={i === PHASES.length - 1} />
        ))}
      </div>
      <div className="flex items-center gap-3">
        <div className="relative h-[3px] flex-1 overflow-hidden rounded-full bg-[#e8e8ec]">
          <div className="sweep absolute inset-y-0 w-[34%] rounded-full bg-[linear-gradient(90deg,rgba(10,132,255,0)_0%,#0a84ff_50%,rgba(10,132,255,0)_100%)]" />
        </div>
        <span className="shrink-0 font-mono text-[11px] tabular-nums text-ink-3">{meta}</span>
      </div>
      {progress.attempt > 1 && (
        <div className="enter-fade flex items-center gap-1.5 rounded-md bg-[#fff8eb] px-2 py-1.5 text-[11px] text-[#b25000]">
          <TriangleAlert size={12} className="shrink-0 text-[#ff9f0a]" />
          <span className="min-w-0 truncate">
            第 {progress.attempt}/{progress.maxAttempts} 次{progress.retryReason ? ` · 上次${progress.retryReason}` : ""}
          </span>
        </div>
      )}
      <div className="text-[11px] text-[#8e8e93]">压缩前 {formatTokens(progress.beforeTokens)}</div>
    </div>
  );
}

function Step({ label, state, last }: { label: string; state: "done" | "now" | "todo"; last: boolean }) {
  return (
    <>
      <span className="flex shrink-0 items-center gap-1.5">
        {state === "done" ? (
          <span className="flex size-3.5 items-center justify-center rounded-full bg-[#34c759] text-white">
            <Check size={10} strokeWidth={3} />
          </span>
        ) : state === "now" ? (
          <span className="flex size-3.5 items-center justify-center rounded-full bg-[#dbeafe]">
            <span className="size-1.5 rounded-full bg-accent" />
          </span>
        ) : (
          <span className="size-3.5 rounded-full border border-[#c7c7cc]" />
        )}
        <span
          className={`motion-colors text-[11px] ${
            state === "done" ? "text-ink-3" : state === "now" ? "font-medium text-accent" : "text-[#aeaeb2]"
          }`}
        >
          {label}
        </span>
      </span>
      {!last && <span className={`h-px flex-1 ${state === "done" ? "bg-[#c7c7cc]" : "bg-[#e5e5ea]"}`} />}
    </>
  );
}

function Failed({ progress, onRetry }: { progress: CompactionProgress; onRetry?: () => void }) {
  return (
    <div className="enter-up flex shrink-0 flex-col gap-2 rounded-[10px] border border-[#fecaca] bg-[#fef2f2] p-3.5">
      <div className="flex items-center justify-between gap-2">
        <span className="flex items-center gap-2 text-xs font-semibold text-[#c0392b]">
          <AlertCircle size={16} className="shrink-0 text-[#ff3b30]" />
          上下文压缩失败
        </span>
        {onRetry && (
          <button
            type="button"
            onClick={onRetry}
            className="motion-colors flex h-6 shrink-0 items-center rounded-md border border-[#fecaca] bg-white px-2.5 text-[11px] font-medium text-[#c0392b] shadow-sm hover:bg-[#fff5f5] active:scale-[0.97]"
          >
            重试
          </button>
        )}
      </div>
      <div className="whitespace-pre-wrap break-words text-xs text-[#c0392b]">{progress.error ?? "Dock 没给原因。"}</div>
      <div className="text-[11px] text-[#c0392b]/70">
        试了 {progress.attempt} 次 · {formatSeconds(progress.elapsedMs)} · 当前上下文已保留，可以继续对话
      </div>
    </div>
  );
}
