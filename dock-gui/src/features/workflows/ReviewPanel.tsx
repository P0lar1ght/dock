// 右侧面板 `review:<runId>`：一次 code-review 审的那份改动 + 落在行上的批注。
// diff 用 run 记下的那条命令现取（dock.1 `vcs/diff`）：本地改动取的是现在的工作区，审完又改过
// 的话批注可能对不上行，面板上写明。

import { useEffect, useState } from "react";
import { GitPullRequest, MessageSquareWarning, RefreshCw, X } from "lucide-react";
import { codeReviewOf } from "dock-core";
import { dock } from "../../dock/client";
import { useWorkflows } from "../../dock/store";
import { IconButton } from "../../components/chrome";
import { ReviewDiff } from "../../components/review/ReviewDiff";
import type { PanelProps } from "../registry";

type Diff = { kind: "loading" } | { kind: "ok"; diff: string; truncated: boolean } | { kind: "error"; message: string };

export function ReviewPanel({ kind, sessionId, onClose, lead, controls }: PanelProps) {
  const runId = kind.slice("review:".length);
  const run = useWorkflows(sessionId).byId[runId];
  const review = run ? codeReviewOf(run) : null;
  const [diff, setDiff] = useState<Diff>({ kind: "loading" });
  const [nonce, setNonce] = useState(0);
  const command = review?.diffCommand ?? "";

  useEffect(() => {
    if (!sessionId || !command) return;
    let live = true;
    setDiff({ kind: "loading" });
    dock
      .call<{ available: boolean; diff?: string; truncated?: boolean; hint?: string; reason?: string }>("vcs/diff", { threadId: sessionId, command })
      .then((r) => {
        if (!live) return;
        if (r.available) setDiff({ kind: "ok", diff: r.diff ?? "", truncated: !!r.truncated });
        else setDiff({ kind: "error", message: r.hint || r.reason || "拿不到这份改动" });
      })
      .catch((e) => live && setDiff({ kind: "error", message: e instanceof Error ? e.message : String(e) }));
    return () => {
      live = false;
    };
  }, [sessionId, command, nonce]);

  return (
    <aside className="glass-pane enter-fade relative flex min-h-0 min-w-0 flex-1 flex-col bg-[#fafafa]">
      <header className="flex h-11 shrink-0 items-center gap-2 border-b border-[#ececf0] px-3" data-tauri-drag-region>
        {lead}
        {review?.pr ? <GitPullRequest size={16} className="shrink-0 text-[#636366]" /> : <MessageSquareWarning size={16} className="shrink-0 text-[#636366]" />}
        <span className="shrink-0 text-[13px] font-semibold text-ink">{review?.pr ? `PR #${review.pr} 审查批注` : "审查批注"}</span>
        {command && <span className="min-w-0 truncate font-mono text-[11px] text-ink-4">{command}</span>}
        <div className="min-w-2 flex-1" data-tauri-drag-region />
        <IconButton icon={RefreshCw} label="重新取改动" size={13} onClick={() => setNonce((n) => n + 1)} className="size-7 text-ink-4" />
        {controls}
        <span className="mx-0.5 h-4 w-px bg-line" />
        <IconButton icon={X} label="关闭审查批注" size={14} onClick={onClose} className="size-7 text-ink-4" />
      </header>
      <div className="thin-scroll flex min-h-0 flex-1 flex-col overflow-auto p-3">
        {!review ? (
          <div className="flex flex-1 items-center justify-center text-[13px] text-ink-4">这次审查的结果已经不在 Dock 里了（Dock 重启过？）</div>
        ) : diff.kind === "loading" ? (
          <div className="flex flex-col gap-2">
            {[0, 1, 2, 3].map((i) => (
              <div key={i} className="skeleton h-4 rounded" style={{ width: `${90 - i * 12}%` }} />
            ))}
          </div>
        ) : diff.kind === "error" ? (
          <div className="flex flex-col gap-3">
            <div className="rounded-lg border border-[#fecaca] bg-[#fef2f2] px-3 py-2 text-[12.5px] text-[#c0392b]">拿不到这份改动：{diff.message}</div>
            <ReviewDiff diff="" findings={review.findings} />
          </div>
        ) : (
          <div className="flex flex-col gap-2">
            {!review.pr && <div className="text-[11.5px] text-ink-4">改动取自现在的工作区：审完之后又改过的话，批注可能对不上行。</div>}
            {diff.truncated && <div className="text-[11.5px] text-[#b25e00]">这份改动太大，只显示了前 4MB。</div>}
            <ReviewDiff diff={diff.diff} findings={review.findings} />
          </div>
        )}
      </div>
    </aside>
  );
}
