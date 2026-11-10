// 右侧「审查」面板：看改动的 diff，code-review 的批注落在行上（同 `review:<runId>` 那个面板的视图）。
// - `changes:worktree`：项目 git 仓库相对 HEAD 的改动 + 未跟踪的新文件（dock.1 `vcs/worktree/diff`）；
// - `changes:turn:<turnId>`：某一轮 agent 改的（`turn/changes/diff`）。
// 头栏的下拉在两者之间切；批注取这个会话最近一次审本地改动的 code-review，「让 Agent 审查」再跑一次。

import { useEffect, useMemo, useState } from "react";
import { ChevronDown, GitCompareArrows, RefreshCw, Sparkles, X } from "lucide-react";
import { codeReviewOf, turnChangesOf, type ReviewFinding } from "dock-core";
import { store, useThread, useTurnChanges, useWorkflows } from "../../dock/store";
import { IconButton } from "../../components/chrome";
import { MenuButton } from "../../components/Popover";
import { ReviewDiff } from "../../components/review/ReviewDiff";
import { useFeatureHost, type PanelProps } from "../registry";

type Diff =
  | { kind: "loading" }
  | { kind: "ok"; diff: string; note: string | null }
  | { kind: "error"; message: string };

const WORKTREE = "worktree";

export function ChangesPanel({ kind, sessionId, onClose, lead, controls }: PanelProps) {
  const host = useFeatureHost();
  // 面板 kind 是开它时给的；之后在下拉里切，不换面板。
  const [source, setSource] = useState(kind.slice("changes:".length) || WORKTREE);
  useEffect(() => setSource(kind.slice("changes:".length) || WORKTREE), [kind]);
  const turnId = source.startsWith("turn:") ? source.slice("turn:".length) : null;
  const [diff, setDiff] = useState<Diff>({ kind: "loading" });
  const [nonce, setNonce] = useState(0);
  const [asking, setAsking] = useState(false);
  const thread = useThread(sessionId);
  const turnChanges = useTurnChanges(sessionId);
  const workflows = useWorkflows(sessionId);
  const changes = turnId ? turnChangesOf(turnChanges, turnId) : null;

  // 有改动的轮次，按对话里的先后；标签用那一轮的第一句话。
  const turns = useMemo(
    () =>
      thread.turns.flatMap((t, i) => {
        const c = turnChangesOf(turnChanges, t.id);
        if (!c) return [];
        const user = t.items.find((x) => x.kind === "user");
        const text = user && user.kind === "user" ? user.text.trim().replace(/\s+/g, " ") : "";
        return [{ id: `turn:${t.id}`, label: `第 ${i + 1} 轮${text ? `：${text.slice(0, 18)}${text.length > 18 ? "…" : ""}` : ""}`, hint: `${c.files.length} 个文件` }];
      }),
    [thread.turns, turnChanges],
  );
  const options = [{ id: WORKTREE, label: "工作区改动", hint: "git，相对 HEAD" }, ...turns];
  const current = options.find((o) => o.id === source) ?? { id: source, label: "这一轮的改动", hint: "" };

  // 最近一次审本地改动（不是 PR）的 code-review：它的批注落在这份 diff 上。
  const findings: readonly ReviewFinding[] = useMemo(() => {
    for (const id of [...workflows.order].reverse()) {
      const run = workflows.byId[id];
      const review = run.status === "completed" ? codeReviewOf(run) : null;
      if (review && !review.pr) return review.findings;
    }
    return [];
  }, [workflows]);
  const reviewing = workflows.order.some((id) => workflows.byId[id].name === "code-review" && workflows.byId[id].status === "running");

  // 改动变了（又改了、撤销了）就重新取：看某一轮时只看那一轮，看工作区时任何一轮变了都算。
  const changesKey = (turnId ? (changes ? [changes] : []) : Object.values(turnChanges.byTurn))
    .map((c) => `${c.turnId}:${c.added}/${c.removed}/${c.files.length}/${c.reverted}`)
    .join(",");
  useEffect(() => {
    if (!sessionId) return;
    let live = true;
    setDiff({ kind: "loading" });
    const load = turnId
      ? store.turnDiff(sessionId, turnId).then((d): Diff => ({ kind: "ok", diff: d, note: changes?.reverted ? "这一轮的改动已经撤销了，下面是当时改了什么。" : null }))
      : store.worktreeDiff(sessionId).then((r): Diff => {
          if (!r.available) return { kind: "error", message: r.hint || r.reason || "拿不到工作区的改动" };
          const notes = [
            r.truncated ? "改动太大，只显示了前 4MB。" : "",
            r.untrackedOmitted ? `另有 ${r.untrackedOmitted} 个未跟踪的文件没列出来。` : "",
          ].filter(Boolean);
          return { kind: "ok", diff: r.diff ?? "", note: notes.join(" ") || null };
        });
    load
      .then((d) => live && setDiff(d))
      .catch((e) => live && setDiff({ kind: "error", message: e instanceof Error ? e.message : String(e) }));
    return () => {
      live = false;
    };
    // changes 只看内容变没变。
  }, [sessionId, turnId, nonce, changesKey]); // eslint-disable-line react-hooks/exhaustive-deps

  const askAgent = async () => {
    if (!sessionId || asking) return;
    setAsking(true);
    const focus = changes ? ` 只看这一轮改的文件：${changes.files.map((f) => f.path).join("、")}` : "";
    const r = await store.runSlash(sessionId, `/code-review${focus}`);
    setAsking(false);
    if (r) host.toast("已让 Agent 开始审查，批注出来后落在这里");
  };

  return (
    <aside className="glass-pane enter-fade relative flex min-h-0 min-w-0 flex-1 flex-col bg-[#fafafa]">
      <header className="flex h-11 shrink-0 items-center gap-2 border-b border-[#ececf0] px-3" data-tauri-drag-region>
        {lead}
        <GitCompareArrows size={16} className="shrink-0 text-[#636366]" />
        <span className="shrink-0 text-[13px] font-semibold text-ink">审查</span>
        <MenuButton
          title="看哪份改动"
          align="left"
          side="bottom"
          header="看哪份改动"
          options={options}
          value={current.id}
          onPick={setSource}
          className="motion-colors flex h-7 min-w-0 items-center gap-1 rounded-md px-2 text-[12px] font-medium text-ink-3 hover:bg-hover"
          trigger={
            <>
              <span className="min-w-0 truncate">{current.label}</span>
              <ChevronDown size={11} className="shrink-0 text-ink-4" />
            </>
          }
        />
        <div className="min-w-2 flex-1" data-tauri-drag-region />
        <button
          type="button"
          disabled={!sessionId || asking || reviewing}
          title={reviewing ? "code-review 正在跑" : "跑一次内置的 code-review，批注落在这份 diff 上"}
          onClick={() => void askAgent()}
          className="motion-colors flex h-7 shrink-0 items-center gap-1 rounded-md border border-line-strong bg-white px-2 text-[12px] font-medium text-ink-2 hover:bg-hover disabled:opacity-50"
        >
          <Sparkles size={12} />
          {reviewing ? "审查中…" : "让 Agent 审查"}
        </button>
        <IconButton icon={RefreshCw} label="重新取改动" size={13} onClick={() => setNonce((n) => n + 1)} className="size-7 text-ink-4" />
        {controls}
        <span className="mx-0.5 h-4 w-px bg-line" />
        <IconButton icon={X} label="关闭审查" size={14} onClick={onClose} className="size-7 text-ink-4" />
      </header>
      <div className="thin-scroll flex min-h-0 flex-1 flex-col overflow-auto p-3">
        {!sessionId ? (
          <div className="flex flex-1 items-center justify-center text-[13px] text-ink-4">先打开一个会话</div>
        ) : diff.kind === "loading" ? (
          <div className="flex flex-col gap-2">
            {[0, 1, 2, 3].map((i) => (
              <div key={i} className="skeleton h-4 rounded" style={{ width: `${90 - i * 12}%` }} />
            ))}
          </div>
        ) : diff.kind === "error" ? (
          <div className="flex flex-col gap-2">
            <div className="rounded-lg border border-[#fecaca] bg-[#fef2f2] px-3 py-2 text-[12.5px] text-[#c0392b]">{diff.message}</div>
            {turns.length > 0 && <div className="text-[12px] text-ink-4">可以在上面的下拉里改看某一轮 agent 改的文件。</div>}
          </div>
        ) : (
          <div className="flex flex-col gap-2">
            {diff.note && <div className="text-[11.5px] text-[#b25e00]">{diff.note}</div>}
            {findings.length > 0 && <div className="text-[11.5px] text-ink-4">批注来自这个会话最近一次 code-review；之后又改过的话可能对不上行。</div>}
            <ReviewDiff diff={diff.diff} findings={findings} />
          </div>
        )}
      </div>
    </aside>
  );
}
