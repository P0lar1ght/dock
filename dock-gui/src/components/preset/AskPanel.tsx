// 预设编辑器旁的「问问 AI」：用户自己配，边配边问；回答只供参考，不改任何字段。
// 设计：wonder-ui/preset/PresetAskPanel（pus.wro 对话中 / hit.til 空状态）。
// Dock `preset/ask` 无状态：每问一次带上之前的问答、当前草稿（未保存）和所在步骤。
// 关掉面板就清空，不落盘。

import { useEffect, useRef, useState } from "react";
import { ArrowUp, Copy, Eye, LoaderCircle, MessageCircleQuestion, Sparkles, X } from "lucide-react";
import type { PresetDraft } from "../../model";
import { store, type AskTurn } from "../../dock/store";
import { Markdown } from "../Markdown";
import { imeComposing } from "../../dock/ime";

export type AskStep = "basic" | "prompt" | "tools" | "agents";

const SUGGESTED: Record<AskStep, string[]> = {
  basic: ["说明里应该写清楚哪些事？", "这个预设和内置的「编码」有什么区别？"],
  prompt: ["这段提示词哪里写得不清楚？", "还应该补上哪些边界（不许做什么）？", "输出格式这样写模型会严格遵守吗？"],
  tools: ["这些工具里哪些会改文件或跑命令？", "按我的提示词，还缺哪些工具？", "哪些工具可以去掉？"],
  agents: ["这类工作值得拆子代理吗？", "子代理之间怎么分工？", "子代理需要哪些工具？"],
};

const STEP_LABEL: Record<AskStep, string> = {
  basic: "基本信息",
  prompt: "角色提示词",
  tools: "工具配置",
  agents: "子代理名册",
};

/** 上下文条：在看哪一步，带一个关键数字。 */
function contextLabel(step: AskStep, draft: PresetDraft, agentId: string | null) {
  if (agentId !== null) return `正在看：子代理 ${agentId || "（新）"}`;
  const extra =
    step === "prompt"
      ? `${[...draft.persona].length} 字`
      : step === "tools"
        ? draft.tools === null
          ? "全部工具"
          : `已选 ${draft.tools.length} 个`
        : step === "agents"
          ? `${draft.agents.length} 个子代理`
          : null;
  return `正在看：${STEP_LABEL[step]}${extra ? ` · ${extra}` : ""}`;
}

export function AskPanel({
  step,
  draft,
  agentId,
  onClose,
}: {
  step: AskStep;
  draft: PresetDraft;
  /** 正在编辑的子代理（`null` = 不在子代理编辑页）。 */
  agentId: string | null;
  onClose: () => void;
}) {
  const [turns, setTurns] = useState<AskTurn[]>([]);
  const [input, setInput] = useState("");
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState<number | null>(null);
  // 每问一个号；关面板 / 换问题后旧回答回来就丢掉（Dock 那边的调用停不下来）。
  const run = useRef(0);
  const listRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  useEffect(() => inputRef.current?.focus(), []);
  useEffect(() => () => void run.current++, []);
  useEffect(() => {
    listRef.current?.scrollTo({ top: listRef.current.scrollHeight, behavior: "smooth" });
  }, [turns, pending]);

  const ask = async (question: string) => {
    const q = question.trim();
    if (!q || pending) return;
    const id = ++run.current;
    const history = turns;
    setTurns([...history, { role: "user", text: q }]);
    setInput("");
    setError(null);
    setPending(true);
    try {
      const answer = await store.askPreset(q, history, draft, step);
      if (id !== run.current) return;
      setTurns((t) => [...t, { role: "assistant", text: answer }]);
    } catch (e) {
      if (id !== run.current) return;
      setError(e instanceof Error ? e.message.replace(/^preset\/ask: /, "") : String(e));
    } finally {
      if (id === run.current) setPending(false);
    }
  };

  const copy = (i: number, text: string) => {
    void navigator.clipboard.writeText(text).then(() => {
      setCopied(i);
      setTimeout(() => setCopied((c) => (c === i ? null : c)), 1500);
    });
  };

  return (
    <aside className="enter-fade flex w-[360px] min-h-0 shrink-0 flex-col border-l border-line bg-[#fafafc]" aria-label="问问 AI">
      <div className="flex shrink-0 items-start justify-between border-b border-line-soft px-4 pb-3 pt-4">
        <div className="flex flex-col gap-0.5">
          <span className="flex items-center gap-1.5 text-[15px] font-semibold text-ink">
            <Sparkles size={14} className="text-accent" />
            问问 AI
          </span>
          <span className="text-xs text-ink-4">只供参考，不会改动你的配置</span>
        </div>
        <button
          type="button"
          onClick={onClose}
          title="关闭（问答清空）"
          className="motion-colors flex size-6 items-center justify-center rounded-md text-ink-4 hover:bg-hover hover:text-ink"
        >
          <X size={14} />
        </button>
      </div>

      <div className="flex shrink-0 px-4 pt-3">
        <span className="flex items-center gap-1.5 rounded-md border border-line-strong bg-white px-2 py-1 text-[11px] text-ink-3">
          <Eye size={12} />
          {contextLabel(step, draft, agentId)}
        </span>
      </div>

      <div ref={listRef} className="thin-scroll flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto p-4" style={{ "--chat-text": "13px" } as React.CSSProperties}>
        {turns.length === 0 && !pending ? (
          <div className="flex flex-1 flex-col justify-center gap-4">
            <div className="flex flex-col items-center gap-2 text-center">
              <span className="flex size-9 items-center justify-center rounded-full bg-[#0a84ff1a] text-accent">
                <MessageCircleQuestion size={16} />
              </span>
              <span className="text-[13px] font-semibold text-ink">配到哪一步都可以问</span>
              <span className="max-w-[260px] text-xs leading-[1.6] text-ink-4">
                AI 能看到你正在编辑的草稿，但不会替你改任何字段。看完回答，自己决定怎么配。
              </span>
            </div>
            <div className="flex flex-col gap-1.5">
              <span className="px-1 text-[11px] font-semibold text-ink-4">针对这一步</span>
              {SUGGESTED[agentId !== null ? "agents" : step].map((q) => (
                <button
                  key={q}
                  type="button"
                  onClick={() => ask(q)}
                  className="motion-colors rounded-lg border border-line-strong bg-white px-3 py-2 text-left text-[12.5px] text-ink-2 hover:border-accent hover:text-accent"
                >
                  {q}
                </button>
              ))}
            </div>
          </div>
        ) : (
          turns.map((t, i) =>
            t.role === "user" ? (
              <div key={i} className="enter-up flex justify-end">
                <span className="max-w-[260px] whitespace-pre-wrap rounded-2xl rounded-br-md bg-[#e9e9ee] px-3 py-2 text-[13px] leading-[1.55] text-ink">
                  {t.text}
                </span>
              </div>
            ) : (
              <div key={i} className="enter-up flex flex-col gap-1.5">
                <Markdown text={t.text} />
                <button
                  type="button"
                  onClick={() => copy(i, t.text)}
                  className="motion-colors flex w-fit items-center gap-1 text-[11px] text-ink-4 hover:text-ink"
                >
                  <Copy size={12} />
                  {copied === i ? "已复制" : "复制回答"}
                </button>
              </div>
            ),
          )
        )}
        {pending && (
          <span className="enter-fade flex items-center gap-2 text-xs text-ink-4">
            <LoaderCircle size={13} className="animate-spin text-accent" />
            正在想…
          </span>
        )}
        {error && <span className="enter-fade text-xs text-[#c0392b]">没问到：{error}</span>}
      </div>

      <div className="flex shrink-0 flex-col gap-1.5 border-t border-line-soft px-4 pb-4 pt-3">
        <div className="motion-colors flex items-end gap-2 rounded-xl border border-line-strong bg-white px-3 py-2.5 shadow-[0px_1px_0px_rgba(0,0,0,0.05)] focus-within:border-accent">
          <textarea
            ref={inputRef}
            value={input}
            onChange={(e) => setInput(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.shiftKey && !imeComposing(e)) {
                e.preventDefault();
                ask(input);
              }
            }}
            rows={2}
            maxLength={4000}
            placeholder="问点什么，比如：这个工具会改文件吗？"
            className="min-h-10 flex-1 resize-none bg-transparent text-[13px] leading-[1.55] text-ink outline-none select-text placeholder:text-[#aeaeb2]"
          />
          <button
            type="button"
            disabled={!input.trim() || pending}
            onClick={() => ask(input)}
            title="发送"
            className="motion-colors flex size-[26px] shrink-0 items-center justify-center rounded-[7px] bg-ink text-white disabled:opacity-30"
          >
            <ArrowUp size={14} />
          </button>
        </div>
        <span className="text-[11px] text-ink-4">Enter:send · 回答会带上当前草稿，关掉面板就清空</span>
      </div>
    </aside>
  );
}
