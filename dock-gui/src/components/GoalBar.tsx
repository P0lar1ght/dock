// 进行中的目标：贴在输入框顶上的一条。设计：wonder-ui/chat/ComposerGoalContext（had ③–⑥）。
// 进行中走表、可暂停 / 清除 / 展开；展开看完整目标、模型最近报的进展、能改目标。
// 模型报完成后变绿停 4 秒自己收起（只在亲眼看到它从进行中变成完成时；重开会话不再弹）。

import { useEffect, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { CircleCheck, Goal, Maximize2, Minimize2, Pause, Pencil, Play, Trash2, X } from "lucide-react";
import type { GoalInfo } from "../model";
import { imeComposing } from "../dock/ime";

const DONE_MS = 4000;
/** 收起动画的时长（离场比进场快）。 */
const LEAVE_MS = 140;

export function GoalBar({
  goal,
  onPause,
  onResume,
  onClear,
  onEdit,
}: {
  goal: GoalInfo | null;
  onPause: () => void;
  onResume: () => void;
  onClear: () => void;
  onEdit: (content: string) => void;
}) {
  const [expanded, setExpanded] = useState(false);
  const [editing, setEditing] = useState<string | null>(null);
  const [done, setDone] = useState(false);
  const [leaving, setLeaving] = useState(false);
  const seen = useRef<GoalInfo["status"] | null>(null);
  const elapsed = useElapsed(goal);

  // 亲眼看到「进行中 / 暂停 → 完成」才亮绿条；4 秒后淡出并清掉 Dock 里留着的完成状态。
  useEffect(() => {
    const before = seen.current;
    seen.current = goal?.status ?? null;
    if (goal?.status !== "completed") {
      setDone(false);
      setLeaving(false);
      return;
    }
    if (before !== "active" && before !== "paused") return;
    setDone(true);
    setExpanded(false);
    setEditing(null);
    const fade = window.setTimeout(() => setLeaving(true), DONE_MS);
    const clear = window.setTimeout(onClear, DONE_MS + LEAVE_MS);
    return () => {
      window.clearTimeout(fade);
      window.clearTimeout(clear);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [goal?.status]);

  if (!goal || (goal.status === "completed" && !done)) return null;

  const paused = goal.status === "paused";
  const shell = `mx-8 -mb-4 flex flex-col border border-b-0 px-3.5 pb-4 transition-[opacity,transform] duration-[var(--dur-fast)] ease-[var(--ease-in)] ${
    leaving ? "translate-y-2 opacity-0" : "enter-up"
  } rounded-t-[16px]`;

  if (done) {
    return (
      <div role="status" className={`${shell} border-[#c9ecd3] bg-[#eaf8ee]`}>
        <div className="flex h-10 items-center gap-2">
          <CircleCheck size={16} className="pop-in shrink-0 text-[#248a3d]" />
          <span className="shrink-0 text-[13px] font-semibold text-ink">目标已完成</span>
          <span className="min-w-0 truncate text-[13px] text-ink-3">{goal.summary}</span>
          <span className="shrink-0 text-[13px] text-ink-4">· 用时 {formatGoalElapsed(elapsed)}</span>
          <button
            type="button"
            title="收起"
            onClick={() => {
              setLeaving(true);
              window.setTimeout(onClear, LEAVE_MS);
            }}
            className="motion-colors ml-auto flex size-7 shrink-0 items-center justify-center rounded-md text-ink-4 hover:bg-[#d8f0de] hover:text-ink-2"
          >
            <X size={14} />
          </button>
        </div>
      </div>
    );
  }

  const saveEdit = () => {
    const next = editing?.trim();
    if (next && next !== goal.summary) onEdit(next);
    setEditing(null);
  };
  const onEditKey = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (imeComposing(e)) return;
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      saveEdit();
    } else if (e.key === "Escape") {
      e.preventDefault();
      setEditing(null);
    }
  };

  const actions = (
    <div className="ml-auto flex shrink-0 items-center gap-0.5 text-ink-4">
      <IconAction title="清除目标" onClick={onClear}>
        <Trash2 size={14} />
      </IconAction>
      {paused ? (
        <button
          type="button"
          title="继续这个目标"
          onClick={onResume}
          className="motion-colors flex h-7 items-center gap-1 rounded-full bg-[#0a84ff1a] px-2.5 text-xs font-medium text-accent hover:bg-[#0a84ff26] active:scale-95"
        >
          <Play size={12} />
          继续
        </button>
      ) : (
        <IconAction title="暂停：这一轮结束后不再自动续跑" onClick={onPause}>
          <Pause size={14} />
        </IconAction>
      )}
      <IconAction title={expanded ? "收起" : "展开"} onClick={() => setExpanded((v) => !v)}>
        {expanded ? <Minimize2 size={14} /> : <Maximize2 size={14} />}
      </IconAction>
    </div>
  );

  return (
    <div className={`md-surface ${shell} border-line-strong bg-[#f5f5f7]`}>
      <div className="flex h-10 items-center gap-2">
        <Goal size={16} className={`shrink-0 ${paused ? "text-ink-4" : "text-accent"}`} />
        <span className="shrink-0 text-[13px] font-semibold text-ink">{paused ? "已暂停的目标" : "进行中的目标"}</span>
        {!expanded && (
          <span className={`min-w-0 truncate text-[13px] ${paused ? "text-ink-4" : "text-ink-3"}`}>{goal.summary}</span>
        )}
        <span className="shrink-0 text-[13px] tabular-nums text-ink-4" title={usageTitle(goal, elapsed)}>
          · {formatGoalElapsed(elapsed)}
          {goal.budget?.ms ? ` / ${formatGoalElapsed(goal.budget.ms)}` : ""}
          {goal.requests > 0 || goal.budget?.requests
            ? ` · ${goal.requests}${goal.budget?.requests ? `/${goal.budget.requests}` : ""} 次请求`
            : ""}
        </span>
        {actions}
      </div>
      <div className="fold" data-open={expanded} aria-hidden={!expanded}>
        <div>
          <div className="flex flex-col gap-3 pb-1 pl-6 pt-1">
            {editing === null ? (
              <div className="flex items-start gap-2">
                <div className="min-w-0 flex-1 whitespace-pre-wrap break-words text-[14px] leading-snug text-ink select-text">
                  {goal.summary}
                </div>
                <button
                  type="button"
                  title="改目标"
                  onClick={() => setEditing(goal.summary)}
                  className="motion-colors flex h-6 shrink-0 items-center gap-1 rounded-md border border-line-strong bg-white px-1.5 text-xs font-medium text-ink-3 hover:bg-hover"
                >
                  <Pencil size={12} />
                  编辑
                </button>
              </div>
            ) : (
              <div className="flex flex-col gap-1.5">
                <textarea
                  autoFocus
                  rows={2}
                  value={editing}
                  onChange={(e) => setEditing(e.target.value)}
                  onKeyDown={onEditKey}
                  className="thin-scroll resize-none rounded-lg border border-[#d1d1d6] bg-white px-2.5 py-1.5 text-[14px] leading-snug text-ink outline-none focus:border-accent"
                />
                <div className="flex items-center justify-end gap-1.5 text-xs">
                  <span className="mr-auto text-[11px] text-ink-5">Enter 保存 · Esc 取消</span>
                  <button
                    type="button"
                    onClick={() => setEditing(null)}
                    className="motion-colors h-6 rounded-md px-2 text-ink-3 hover:bg-hover"
                  >
                    取消
                  </button>
                  <button
                    type="button"
                    onClick={saveEdit}
                    disabled={!editing.trim()}
                    className="motion-colors h-6 rounded-md bg-ink px-2.5 font-medium text-white disabled:bg-ink-5"
                  >
                    保存
                  </button>
                </div>
              </div>
            )}
            {goal.progress && (
              <div className="flex flex-col gap-1 border-l-2 border-[#0a84ff4d] pl-3">
                <span className="text-[11px] font-medium text-ink-4">最新进展</span>
                <span className="whitespace-pre-wrap break-words text-[13px] leading-snug text-ink-2 select-text">
                  {goal.progress}
                </span>
              </div>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}

function IconAction({ title, onClick, children }: { title: string; onClick: () => void; children: ReactNode }) {
  return (
    <button
      type="button"
      title={title}
      aria-label={title}
      onClick={onClick}
      className="motion-colors flex size-7 items-center justify-center rounded-md hover:bg-hover hover:text-ink-2 active:scale-95"
    >
      {children}
    </button>
  );
}

/** 目标已经跑了多久：Dock 给的时长 + 进行中时从拿到那刻起走的表（每秒刷新）。 */
function useElapsed(goal: GoalInfo | null): number {
  const [now, setNow] = useState(() => Date.now());
  const ticking = goal?.status === "active";
  useEffect(() => {
    if (!ticking) return;
    const t = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(t);
  }, [ticking]);
  if (!goal) return 0;
  return goal.elapsedMs + (ticking ? Math.max(0, now - goal.at) : 0);
}

/** 用量的悬停说明：用了多少、上限多少（`/goal budget` 设，默认不限）。 */
function usageTitle(goal: GoalInfo, elapsed: number): string {
  const used = `已跑 ${formatGoalElapsed(elapsed)}，用了 ${goal.requests} 次请求`;
  const b = goal.budget;
  const cap = b?.ms ? `上限 ${formatGoalElapsed(b.ms)}` : b?.requests ? `上限 ${b.requests} 次请求` : "不设上限";
  return `${used}；${cap}（/goal budget 2h 或 300 设，off 取消）`;
}

/** `12s` / `1m 12s` / `1h 02m`。 */
export function formatGoalElapsed(ms: number): string {
  const s = Math.floor(ms / 1000);
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m ${String(s % 60).padStart(2, "0")}s`;
  return `${Math.floor(m / 60)}h ${String(m % 60).padStart(2, "0")}m`;
}
