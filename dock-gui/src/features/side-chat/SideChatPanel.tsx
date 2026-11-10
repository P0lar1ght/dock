// 右侧「侧边聊天」：从当前会话分叉的只读旁问（Dock `thread/aside/*`，能力 `sideChat`）。
// 设计：wonder-ui/chat/SideChat（eme 空 / die 回答中 / jun 闲 / bad 主线在等 / iso 权限 /
// big·led·cat·aln 写进主线 / sop 关闭确认 / did 主会话已关闭）。
//
// - 带着主会话的上下文、只读工具，可以连续追问；主线照跑不被打断。
// - 每个主会话最多一个，GUI 开着期间切走再切回还在；关面板就结束（Dock 不落盘）。
// - 回主线：每条回复下「带回输入框」（只填不发）；头栏「写进主线」让侧聊整理一段结论，
//   改好确认后写进主会话的上下文（不开新的一轮），主会话里出现一张笔记卡。

import { useEffect, useMemo, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { AlertCircle, ArrowUp, Check, Copy, CornerDownLeft, GitMerge, Loader2, MessageSquare, Shield, Square, X } from "lucide-react";
import { isRunning, pendingItems } from "dock-core";
import { store, useApp, useThread } from "../../dock/store";
import { imeComposing } from "../../dock/ime";
import { IconButton } from "../../components/chrome";
import { Pending } from "../../components/Pending";
import { Transcript, TranscriptOptionsContext } from "../../components/Transcript";
import { useFeatureHost, type PanelProps } from "../registry";

/** 写进主线的笔记最多多少字（同 Dock `SIDE_NOTE_MAX_CHARS`）。 */
const NOTE_MAX = 2000;

type Handback =
  | { step: "idle" }
  | { step: "drafting"; seq: number }
  | { step: "preview"; note: string }
  | { step: "done" }
  | { step: "error"; reason: string };

export function SideChatPanel({ sessionId, onClose, lead, controls }: PanelProps) {
  const app = useApp();
  const parentId = sessionId;
  const asideId = parentId ? app.asides.get(parentId) ?? null : null;
  const ended = !!parentId && app.asidesEnded.has(parentId);
  const parentTitle = app.sessions.find((s) => s.id === parentId)?.title ?? "当前会话";
  const thread = useThread(asideId);
  const running = asideId ? isRunning(thread) : false;
  const pending = asideId ? pendingItems(thread)[0] : undefined;
  const [handback, setHandback] = useState<Handback>({ step: "idle" });
  const [confirming, setConfirming] = useState(false);
  const host = useFeatureHost();

  // 换了主会话：写进主线的流程各管各的，回到起点。
  useEffect(() => {
    setHandback({ step: "idle" });
    setConfirming(false);
  }, [parentId]);

  const hasReply = thread.turns.some((t) => t.status !== "running" && t.items.some((i) => i.kind === "text"));
  const canMerge = !!asideId && hasReply && !running && handback.step !== "drafting";

  const draft = async () => {
    if (!asideId) return;
    const seq = Date.now();
    setHandback({ step: "drafting", seq });
    try {
      const note = await store.draftSideNote(asideId);
      // 起草期间点了「取消」或换了一次：这次的结果作废。
      setHandback((h) => (h.step === "drafting" && h.seq === seq ? { step: "preview", note } : h));
    } catch (e) {
      const reason = e instanceof Error ? e.message : String(e);
      setHandback((h) => (h.step === "drafting" && h.seq === seq ? { step: "error", reason } : h));
    }
  };

  const merge = async (note: string) => {
    if (!parentId) return;
    try {
      await store.mergeSideNote(parentId, note);
      setHandback({ step: "done" });
      host.toast(`已写进 ${parentTitle}`);
    } catch (e) {
      setHandback({ step: "error", reason: e instanceof Error ? e.message : String(e) });
    }
  };

  // 关面板就结束这段旁问。还在跑、或有没写进去的结论时先问一句。
  const unfinished = running || handback.step === "drafting" || handback.step === "preview";
  const finish = async () => {
    setConfirming(false);
    if (parentId) await store.closeAside(parentId);
    onClose();
  };
  const close = () => (unfinished ? setConfirming(true) : void finish());

  const ReplyActions = useMemo(
    () =>
      function ReplyActions({ text }: { text: string }) {
        const [copied, setCopied] = useState(false);
        return (
          <div className="flex items-center gap-1 text-[11px] text-ink-4">
            <button
              type="button"
              title="把这条回复引用好填进主会话的输入框（只填不发）"
              onClick={() => {
                if (!parentId) return;
                store.fillComposer(parentId, quoted(text), "append");
                host.toast("已带回主线输入框");
              }}
              className="motion-colors flex h-6 items-center gap-1 rounded-md px-1.5 hover:bg-hover hover:text-ink-2"
            >
              <CornerDownLeft size={12} />
              带回输入框
            </button>
            <button
              type="button"
              onClick={() => {
                void navigator.clipboard.writeText(text).then(() => {
                  setCopied(true);
                  window.setTimeout(() => setCopied(false), 1500);
                });
              }}
              className="motion-colors flex h-6 items-center gap-1 rounded-md px-1.5 hover:bg-hover hover:text-ink-2"
            >
              {copied ? <Check size={12} /> : <Copy size={12} />}
              {copied ? "已复制" : "复制"}
            </button>
          </div>
        );
      },
    [parentId, host],
  );
  // 旁问线程不在名册里，文件路径按主会话的目录开。
  const options = useMemo(
    () => ({ compact: true, rewind: false, ReplyActions, filesThreadId: parentId ?? undefined }),
    [ReplyActions, parentId],
  );

  const header = (
    <div className="flex shrink-0 flex-col border-b border-[#ececf0]">
      <div className="flex h-11 items-center gap-2 px-3" data-tauri-drag-region>
        {lead}
        <MessageSquare size={16} className="shrink-0 text-ink-3" />
        <span className="shrink-0 text-[13px] font-semibold text-ink">侧边聊天</span>
        <span className="min-w-2 flex-1" />
        {asideId && !ended && (
          <button
            type="button"
            disabled={!canMerge}
            title={canMerge ? "让侧边聊天整理一段结论，确认后写进主线" : "先问点什么、等它答完"}
            onClick={() => void draft()}
            className={`motion-colors flex h-7 shrink-0 items-center gap-1.5 rounded-[7px] border px-2 text-[12px] font-medium disabled:opacity-40 ${
              handback.step === "drafting" || handback.step === "preview"
                ? "border-transparent bg-selected text-ink"
                : "border-line-strong bg-white text-ink-2 enabled:hover:bg-hover"
            }`}
          >
            <GitMerge size={13} />
            写进主线
          </button>
        )}
        {controls}
        <span className="mx-0.5 h-4 w-px shrink-0 bg-[#e8e8ec]" />
        <IconButton icon={X} label="结束侧边聊天" size={14} onClick={close} className="size-7 text-ink-4" />
      </div>
      <div className="-mt-1.5 truncate pb-2 pl-[62px] pr-3 text-[11px] text-ink-4">
        分叉自 {parentTitle} · {ended ? "已结束" : "只读"}
      </div>
    </div>
  );

  if (!parentId) {
    return (
      <Shell header={header}>
        <Centered title="先选一个会话" body={["侧边聊天从当前会话分叉，带着它的上下文。"]} />
      </Shell>
    );
  }

  if (ended) {
    return (
      <Shell header={header}>
        <Centered
          title="主会话已关闭，这段侧边聊天也结束了"
          action={
            <button
              type="button"
              onClick={() => {
                store.dismissAsideEnded(parentId);
                onClose();
              }}
              className="motion-colors mt-2 h-[26px] rounded-[7px] border border-line-strong bg-white px-3 text-[11px] font-medium text-ink-2 hover:bg-hover"
            >
              关闭面板
            </button>
          }
        />
      </Shell>
    );
  }

  return (
    <Shell header={header}>
      <ParentBanner parentId={parentId} />
      {thread.turns.length === 0 ? (
        <Centered
          title="问点什么，不打断主线"
          body={["带着这个会话的上下文，只读，答案不进主线。", "关掉面板就结束这段旁问。"]}
        />
      ) : (
        <TranscriptOptionsContext.Provider value={options}>
          <Transcript thread={thread} sessionKey={asideId!} />
        </TranscriptOptionsContext.Provider>
      )}
      <div className="flex-1" />
      <HandbackCard
        state={handback}
        onCancel={() => setHandback({ step: "idle" })}
        onRetry={() => void draft()}
        onMerge={(note) => void merge(note)}
      />
      {asideId && pending && (
        <div className="px-3">
          <Pending
            key={pending.id}
            item={pending}
            noAlways
            onPermission={(allow) => store.resolvePermission(asideId, allow, false)}
            onAnswer={(answers) => store.answer(asideId, answers)}
            onPlan={(decision) => store.resolvePlan(asideId, decision)}
            onElicit={(action) => store.resolveElicit(asideId, action)}
            onElicitOpen={() => store.openElicit(asideId)}
          />
        </div>
      )}
      <AsideComposer
        disabled={handback.step === "drafting"}
        running={running}
        onSend={(text) => store.sendAside(parentId, text)}
        onStop={() => asideId && store.cancel(asideId)}
      />
      {confirming && <CloseConfirm onCancel={() => setConfirming(false)} onConfirm={() => void finish()} />}
    </Shell>
  );
}

/** 带回主线输入框的引用块（同 TUI `/tab back`）：注明出处，整段引用。 */
function quoted(text: string): string {
  const body = text
    .trim()
    .split("\n")
    .map((l) => `> ${l}`)
    .join("\n");
  return `（来自侧边聊天）\n${body}\n\n`;
}

function Shell({ header, children }: { header: ReactNode; children: ReactNode }) {
  return (
    <aside className="glass-pane enter-fade relative flex min-h-0 min-w-0 flex-1 flex-col bg-[#fafafa]">
      {header}
      {children}
    </aside>
  );
}

function Centered({ title, body = [], action }: { title: string; body?: string[]; action?: ReactNode }) {
  return (
    <div className="enter-fade flex min-h-0 flex-1 flex-col items-center justify-center gap-2 px-8 py-6 text-center">
      <span className="mb-1 flex size-10 items-center justify-center rounded-full bg-[#f0f0f2]">
        <MessageSquare size={18} className="text-ink-4" />
      </span>
      <span className="text-[13px] font-medium text-ink">{title}</span>
      {body.map((line) => (
        <span key={line} className="text-xs text-ink-4">
          {line}
        </span>
      ))}
      {action}
    </div>
  );
}

/**
 * 主线的状态横幅（设计 bad）：在等批准 / 回答时常驻；跑完 / 出错时出现 3 秒。
 * 主线就在旁边，横幅只是提醒，不跳转。
 */
function ParentBanner({ parentId }: { parentId: string }) {
  const app = useApp();
  const parent = useThread(parentId);
  const running = isRunning(parent);
  const waiting = app.waiting.has(parentId);
  const [flash, setFlash] = useState<string | null>(null);
  const was = useRef(running);
  useEffect(() => {
    if (was.current && !running) {
      const last = parent.turns[parent.turns.length - 1];
      if (last && !last.steered && last.status !== "cancelled") {
        setFlash(last.status === "failed" ? "主线出错了" : "主线已完成");
        const t = window.setTimeout(() => setFlash(null), 3000);
        was.current = running;
        return () => window.clearTimeout(t);
      }
    }
    was.current = running;
  }, [running, parent]);
  const text = waiting ? "主线在等你批准或回答" : flash;
  if (!text) return null;
  return (
    <div className="enter-fade flex shrink-0 items-center gap-2 border-b border-[#0a84ff1a] bg-[#0a84ff14] px-3 py-2 text-[12px] text-ink-2">
      <Shield size={13} className="shrink-0 text-accent" />
      {text}
    </div>
  );
}

function HandbackCard({
  state,
  onCancel,
  onRetry,
  onMerge,
}: {
  state: Handback;
  onCancel: () => void;
  onRetry: () => void;
  onMerge: (note: string) => void;
}) {
  const [note, setNote] = useState("");
  useEffect(() => {
    if (state.step === "preview") setNote(state.note);
  }, [state]);
  const card = "enter-up mx-3 mb-2 flex shrink-0 flex-col gap-2 rounded-[10px] border p-3";
  switch (state.step) {
    case "idle":
      return null;
    case "drafting":
      return (
        <div className={`${card} flex-row items-center border-[#e5e5ea] bg-white`}>
          <Loader2 size={14} className="shrink-0 animate-spin text-ink-4" />
          <span className="flex-1 text-[13px] text-ink-2">正在整理结论…</span>
          <button type="button" onClick={onCancel} className={secondaryBtn}>
            取消
          </button>
        </div>
      );
    case "preview": {
      const n = [...note].length;
      const over = n > NOTE_MAX;
      return (
        <div className={`${card} border-[#e5e5ea] bg-[#f5f5f7]`}>
          <span className="text-[12px] font-medium text-ink-2">写进主线的笔记</span>
          <div className="flex flex-col rounded-[8px] border border-accent bg-white focus-within:shadow-[0_0_0_3px_#0a84ff1a]">
            <textarea
              value={note}
              onChange={(e) => setNote(e.target.value)}
              rows={6}
              className="thin-scroll min-h-0 resize-none bg-transparent px-2.5 pt-2 text-[13px] leading-[1.6] text-ink outline-none"
            />
            <span className={`self-end px-2 pb-1 font-mono text-[11px] ${over ? "text-[#c0392b]" : "text-ink-4"}`}>
              {n} / {NOTE_MAX}
            </span>
          </div>
          <span className="text-[11px] text-ink-4">
            主线在跑：下一步并入；闲着：下次对话时模型能看到。不会开始新的一轮。
          </span>
          <div className="flex justify-end gap-2">
            <button type="button" onClick={onCancel} className={secondaryBtn}>
              取消
            </button>
            <button
              type="button"
              disabled={over || !note.trim()}
              onClick={() => onMerge(note.trim())}
              className="motion-colors h-[26px] rounded-[7px] bg-[#1c1c1e] px-3 text-[11px] font-medium text-white disabled:bg-[#c7c7cc] active:scale-[0.97]"
            >
              写入主线
            </button>
          </div>
        </div>
      );
    }
    case "done":
      return (
        <div className={`${card} flex-row items-center border-[#e5e5ea] bg-white`}>
          <Check size={14} className="shrink-0 text-[#34c759]" />
          <span className="flex-1 text-[13px] text-ink-2">已写进主线</span>
          <button type="button" onClick={onCancel} className="text-[12px] text-ink-4 hover:text-ink-2">
            好的
          </button>
        </div>
      );
    case "error":
      return (
        <div className={`${card} border-[#fecaca] bg-[#fef2f2]`}>
          <span className="flex items-center gap-2 text-xs font-semibold text-[#c0392b]">
            <AlertCircle size={14} className="shrink-0 text-[#ff3b30]" />
            没能整理出结论
          </span>
          <span className="break-words text-xs text-[#c0392b]">{state.reason}</span>
          <div className="flex justify-end gap-2">
            <button type="button" onClick={onCancel} className={secondaryBtn}>
              取消
            </button>
            <button type="button" onClick={onRetry} className={secondaryBtn}>
              重试
            </button>
          </div>
        </div>
      );
  }
}

const secondaryBtn =
  "motion-colors h-[26px] shrink-0 rounded-[7px] border border-line-strong bg-white px-2.5 text-[11px] font-medium text-ink-2 hover:bg-hover active:scale-[0.97]";

/** 侧边聊天的输入框：照 act 缩到 420，没有模型 / 推理 chip。Enter 发，Shift+Enter 换行；在跑时排队。 */
function AsideComposer({
  disabled,
  running,
  onSend,
  onStop,
}: {
  disabled: boolean;
  running: boolean;
  onSend: (text: string) => Promise<boolean>;
  onStop: () => void;
}) {
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const send = async () => {
    const message = text.trim();
    if (!message || busy || disabled) return;
    setBusy(true);
    if (await onSend(message)) setText("");
    setBusy(false);
  };
  const onKey = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Enter" && !e.shiftKey && !imeComposing(e)) {
      e.preventDefault();
      void send();
    }
  };
  return (
    <div className="shrink-0 px-3 pb-3 pt-1">
      <div className="flex flex-col rounded-[16px] border border-[#e5e5ea] bg-white px-3 pb-2 pt-2.5 shadow-[0_1px_2px_rgba(0,0,0,0.04)] focus-within:border-[#c7c7cc]">
        <textarea
          value={text}
          disabled={disabled}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={onKey}
          rows={2}
          placeholder={disabled ? "正在整理结论…" : "问点什么…"}
          className="thin-scroll max-h-40 min-h-0 resize-none bg-transparent text-[14px] leading-[1.6] text-ink outline-none placeholder:text-[#c7c7cc]"
        />
        <div className="flex justify-end">
          {running && !text.trim() ? (
            <button
              type="button"
              aria-label="停止"
              title="停止侧边聊天这一轮"
              onClick={onStop}
              className="motion-colors flex size-8 items-center justify-center rounded-full bg-[#1c1c1e] text-white active:scale-[0.95]"
            >
              <Square size={12} fill="currentColor" />
            </button>
          ) : (
            <button
              type="button"
              aria-label="发送"
              disabled={!text.trim() || busy || disabled}
              onClick={() => void send()}
              className="motion-colors flex size-8 items-center justify-center rounded-full bg-[#1c1c1e] text-white disabled:bg-[#c7c7cc] active:scale-[0.95]"
            >
              <ArrowUp size={16} />
            </button>
          )}
        </div>
      </div>
    </div>
  );
}

function CloseConfirm({ onCancel, onConfirm }: { onCancel: () => void; onConfirm: () => void }) {
  return (
    <div className="enter-fade absolute inset-0 z-10 flex items-start justify-center bg-black/5 pt-16" onClick={onCancel}>
      <div
        className="enter-up flex w-[300px] flex-col gap-2 rounded-[12px] border border-[#e5e5ea] bg-white p-4 shadow-[0_8px_30px_rgba(0,0,0,0.12)]"
        onClick={(e) => e.stopPropagation()}
      >
        <span className="text-[13px] font-semibold text-ink">结束这段侧边聊天？</span>
        <span className="text-xs text-ink-4">内容不会保存。</span>
        <div className="mt-1 flex justify-end gap-2">
          <button type="button" onClick={onCancel} className={secondaryBtn}>
            取消
          </button>
          <button
            type="button"
            onClick={onConfirm}
            className="motion-colors h-[26px] rounded-[7px] bg-[#1c1c1e] px-3 text-[11px] font-medium text-white active:scale-[0.97]"
          >
            结束
          </button>
        </div>
      </div>
    </div>
  );
}
