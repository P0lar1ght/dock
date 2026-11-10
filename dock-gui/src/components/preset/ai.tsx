// 编辑器里的 AI 小助手：提示词润色 / 扩写（设计 PresetAiPromptDiff）、按描述推荐工具
// （设计 PresetAiToolsRecommend）、AI 草稿提示条（设计 PresetAiDraft）。都经 Dock 调一次
// 默认模型，只给建议：用户点接受 / 采纳才改草稿，保存照旧走编辑器。

import { useState } from "react";
import { Check, LoaderCircle, Plus, RotateCcw, Sparkles, Wand2, X } from "lucide-react";
import { store } from "../../dock/store";
import { Markdown } from "../Markdown";
import { groupOf, toolSummary } from "./toolGroups";

// ---- 提示词润色 / 扩写 ----

type Mode = "polish" | "expand";

/** 编辑框工具栏上的「AI 润色 / AI 扩写」，出对比浮层；接受后可以撤销一次。 */
export function PromptRewrite({
  persona,
  context,
  onApply,
}: {
  persona: string;
  /** 预设的一句话说明，给模型当背景。 */
  context: string;
  onApply: (next: string) => void;
}) {
  const [busy, setBusy] = useState<Mode | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [proposal, setProposal] = useState<{ mode: Mode; text: string } | null>(null);
  const [undo, setUndo] = useState<string | null>(null);
  const empty = !persona.trim();

  const ask = async (mode: Mode) => {
    setBusy(mode);
    setError(null);
    try {
      setProposal({ mode, text: await store.rewritePersona(persona, mode, context) });
    } catch (e) {
      setError(e instanceof Error ? e.message.replace(/^preset\/rewrite: /, "") : String(e));
    } finally {
      setBusy(null);
    }
  };

  const button = (mode: Mode, label: string, Icon: typeof Sparkles) => (
    <button
      type="button"
      disabled={empty || busy !== null}
      onClick={() => ask(mode)}
      title={empty ? "先写几句再让 AI 改" : undefined}
      className="motion-colors flex h-6 items-center gap-1 rounded-md border border-[#0a84ff40] bg-[#0a84ff0d] px-2 text-[11.5px] font-medium text-accent hover:bg-[#0a84ff1a] disabled:opacity-40"
    >
      {busy === mode ? <LoaderCircle size={11} className="animate-spin" /> : <Icon size={11} />}
      {label}
    </button>
  );

  const before = [...persona.trim()].length;
  const after = proposal ? [...proposal.text].length : 0;
  return (
    <>
      {error && (
        <span className="enter-fade max-w-[260px] truncate text-[11px] text-[#ff3b30]" title={error}>
          {error}
        </span>
      )}
      {undo !== null && (
        <button
          type="button"
          onClick={() => {
            onApply(undo);
            setUndo(null);
          }}
          className="enter-fade flex items-center gap-1 text-[11.5px] text-ink-3 hover:text-ink"
        >
          <RotateCcw size={11} /> 撤销改写
        </button>
      )}
      {button("polish", "AI 润色", Sparkles)}
      {button("expand", "AI 扩写", Wand2)}

      {proposal && (
        <div className="enter-fade absolute inset-0 z-[60] flex items-center justify-center bg-[#00000040] p-8 backdrop-blur-[3px]">
          <div className="glass-dialog pop-in flex h-[640px] max-h-full w-[960px] max-w-full flex-col overflow-hidden rounded-2xl border border-[#00000015] bg-white shadow-[0px_24px_64px_rgba(0,0,0,0.24)]">
            <div className="flex shrink-0 items-center justify-between gap-4 border-b border-line px-5 py-3.5">
              <span className="flex items-center gap-2.5">
                <span className="flex size-8 items-center justify-center rounded-lg bg-[#f5f3ff] text-[#7c3aed]">
                  <Sparkles size={16} />
                </span>
                <span className="flex flex-col">
                  <span className="flex items-center gap-2 text-[14px] font-semibold text-ink">
                    AI {proposal.mode === "polish" ? "润色" : "扩写"}对比
                    <span className="rounded-md bg-[#f5f3ff] px-1.5 py-0.5 text-[10.5px] font-medium text-[#7c3aed]">
                      {proposal.mode === "polish" ? "意思不变" : "补齐结构"}
                    </span>
                  </span>
                  <span className="text-xs text-ink-4">左边是现在的提示词，右边是 AI 的建议；接受后整段替换</span>
                </span>
              </span>
              <span className="flex items-center gap-2 font-mono text-[11px]">
                <span className="rounded-md bg-[#fff1f0] px-2 py-0.5 text-[#d70015]">原 {before} 字</span>
                <span className="rounded-md bg-[#effaf3] px-2 py-0.5 text-[#15803d]">
                  {after >= before ? "+" : ""}
                  {after - before} → {after} 字
                </span>
              </span>
            </div>
            <div className="grid min-h-0 flex-1 grid-cols-2">
              <div className="flex min-h-0 flex-col border-r border-line bg-[#fafafc]">
                <span className="flex shrink-0 items-center justify-between border-b border-line px-4 py-2 text-xs">
                  <span className="flex items-center gap-1.5 font-semibold text-ink-2">
                    <span className="size-1.5 rounded-full bg-[#ff3b30]" />
                    改写前（当前版本）
                  </span>
                </span>
                <div className="thin-scroll min-h-0 flex-1 overflow-y-auto p-4">
                  <pre className="whitespace-pre-wrap break-words rounded-xl border border-[#ffd0cc] bg-[#fff8f7] p-3.5 font-mono text-xs leading-relaxed text-[#8a1c14] select-text">
                    {persona.trim()}
                  </pre>
                </div>
              </div>
              <div className="flex min-h-0 flex-col">
                <span className="flex shrink-0 items-center justify-between border-b border-line bg-[#f3fbf6] px-4 py-2 text-xs">
                  <span className="flex items-center gap-1.5 font-semibold text-[#15803d]">
                    <span className="size-1.5 rounded-full bg-[#34c759]" />
                    AI 建议（接受后替换）
                  </span>
                </span>
                <div className="thin-scroll min-h-0 flex-1 overflow-y-auto p-4">
                  <div className="rounded-xl border border-[#bbf7d0] bg-[#f6fdf8] px-4 py-3">
                    <Markdown text={proposal.text} size="card" />
                  </div>
                </div>
              </div>
            </div>
            <div className="flex shrink-0 items-center justify-between border-t border-line bg-[#fbfbfb] px-5 py-3">
              <span className="text-xs text-ink-4">接受后编辑框工具栏会出现「撤销改写」，可以一键换回原文。</span>
              <span className="flex items-center gap-2">
                <button
                  type="button"
                  onClick={() => setProposal(null)}
                  className="motion-colors h-[34px] rounded-lg border border-line-strong bg-white px-3.5 text-[13px] font-medium text-ink hover:bg-hover"
                >
                  放弃
                </button>
                <button
                  type="button"
                  onClick={() => {
                    setUndo(persona);
                    onApply(proposal.text);
                    setProposal(null);
                  }}
                  className="motion-colors flex h-[34px] items-center gap-1.5 rounded-lg bg-accent px-4 text-[13px] font-medium text-white shadow hover:bg-[#0077ed] active:scale-[0.98]"
                >
                  <Check size={14} />
                  接受并替换
                </button>
              </span>
            </div>
          </div>
        </div>
      )}
    </>
  );
}

// ---- 工具推荐 ----

export interface Suggestions {
  loading: boolean;
  error: string | null;
  /** 还没选上的推荐（待确认）。 */
  picks: { name: string; reason: string }[];
  /** 推荐里本来就选上了的个数。 */
  already: number;
}

/** 「按描述推荐」按钮（工具分区标题行）。 */
export function SuggestButton({
  disabled,
  reason,
  loading,
  onClick,
}: {
  disabled: boolean;
  reason?: string;
  loading: boolean;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      disabled={disabled || loading}
      title={reason}
      onClick={onClick}
      className="motion-colors flex h-8 items-center gap-1.5 rounded-lg border border-[#0a84ff40] bg-[#0a84ff0d] px-3 text-xs font-semibold text-accent hover:bg-[#0a84ff1a] disabled:opacity-40"
    >
      {loading ? <LoaderCircle size={13} className="animate-spin" /> : <Sparkles size={13} />}
      按描述推荐
    </button>
  );
}

/** 推荐结果：逐项采纳 / 忽略，或全部接受。 */
export function SuggestPanel({
  s,
  onAccept,
  onDismiss,
  onClose,
}: {
  s: Suggestions;
  onAccept: (names: string[]) => void;
  onDismiss: (name: string) => void;
  onClose: () => void;
}) {
  if (s.loading) {
    return (
      <div className="enter-fade flex shrink-0 items-center gap-2.5 rounded-xl border border-[#0a84ff30] bg-[#f5f9ff] px-4 py-3 text-[12.5px] text-ink-2">
        <LoaderCircle size={15} className="animate-spin text-accent" />
        AI 正在按描述和提示词挑工具…
      </div>
    );
  }
  if (s.error) {
    return (
      <div className="enter-fade flex shrink-0 items-center justify-between gap-3 rounded-xl border border-[#ffd0cc] bg-[#fff5f5] px-4 py-3 text-[12.5px] text-[#b3261e]">
        <span>推荐失败：{s.error}</span>
        <button type="button" onClick={onClose} className="text-ink-4 hover:text-ink">
          <X size={14} />
        </button>
      </div>
    );
  }
  if (s.picks.length === 0) {
    return (
      <div className="enter-fade flex shrink-0 items-center justify-between gap-3 rounded-xl border border-[#0a84ff30] bg-[#f5f9ff] px-4 py-3 text-[12.5px] text-ink-2">
        <span className="flex items-center gap-2">
          <Check size={15} className="text-[#34c759]" />
          AI 推荐的 {s.already} 项工具都已经选上了。
        </span>
        <button type="button" onClick={onClose} className="text-ink-4 hover:text-ink">
          <X size={14} />
        </button>
      </div>
    );
  }
  return (
    <div className="enter-fade flex shrink-0 flex-col gap-2.5">
      <div className="flex items-center justify-between gap-3 rounded-xl border border-[#0a84ff30] bg-[#f5f9ff] px-4 py-2.5">
        <span className="flex items-center gap-2 text-[12.5px] text-ink-2">
          <Sparkles size={15} className="text-accent" />
          <span className="font-semibold text-accent">AI 推荐了 {s.picks.length} 项工具</span>
          {s.already > 0 && <span className="text-ink-4">（另有 {s.already} 项已经选上）</span>}
          请确认是否授权：
        </span>
        <span className="flex shrink-0 items-center gap-2">
          <button type="button" onClick={onClose} className="px-1 text-xs text-ink-4 hover:text-ink">
            忽略
          </button>
          <button
            type="button"
            onClick={() => onAccept(s.picks.map((p) => p.name))}
            className="motion-colors flex h-7 items-center gap-1 rounded-md bg-accent px-2.5 text-xs font-semibold text-white hover:bg-[#0077ed]"
          >
            <Check size={12} /> 全部接受 ({s.picks.length})
          </button>
        </span>
      </div>
      {s.picks.map((p) => (
        <div
          key={p.name}
          className="enter-up flex flex-col gap-2 rounded-xl border border-[#0a84ff66] bg-white p-3 shadow-[0px_0px_0px_3px_rgba(10,132,255,0.08)]"
        >
          <div className="flex items-center justify-between gap-3">
            <span className="flex min-w-0 items-center gap-2">
              <Plus size={14} className="shrink-0 text-accent" />
              <span className="font-mono text-[12.5px] font-semibold text-ink">{p.name}</span>
              <span className="rounded-sm bg-[#f2f2f7] px-1.5 py-0.5 font-mono text-[10.5px] text-ink-3">{groupOf(p.name).tag}</span>
              <span className="rounded-full border border-[#0a84ff40] px-1.5 py-0.5 text-[10.5px] text-accent">待确认</span>
              <span className="truncate text-[11.5px] text-ink-4">{toolSummary(p.name, "")}</span>
            </span>
            <span className="flex shrink-0 items-center gap-1.5">
              <button
                type="button"
                onClick={() => onDismiss(p.name)}
                title="不要这个"
                className="motion-colors flex size-7 items-center justify-center rounded-md border border-line-strong text-ink-4 hover:bg-hover hover:text-ink"
              >
                <X size={13} />
              </button>
              <button
                type="button"
                onClick={() => onAccept([p.name])}
                className="motion-colors flex h-7 items-center gap-1 rounded-md bg-accent px-2.5 text-xs font-semibold text-white hover:bg-[#0077ed]"
              >
                <Check size={12} /> 采纳
              </button>
            </span>
          </div>
          {p.reason && (
            <span className="rounded-md border border-line-soft bg-[#fafafc] px-2.5 py-1 text-[11.5px] text-ink-3">
              <span className="font-semibold text-accent">推荐理由：</span>
              {p.reason}
            </span>
          )}
        </div>
      ))}
    </div>
  );
}

// ---- AI 草稿提示条 ----

export function DraftBanner({
  summary,
  regenerating,
  onRegenerate,
}: {
  summary: string;
  regenerating: boolean;
  onRegenerate: () => void;
}) {
  return (
    <div className="enter-fade flex shrink-0 items-center justify-between gap-3 rounded-xl border border-[#0a84ff30] bg-[#f5f9ff] px-4 py-3">
      <span className="flex min-w-0 items-center gap-2.5 text-[12.5px] text-ink-2">
        <span className="flex size-7 shrink-0 items-center justify-center rounded-lg bg-[#0a84ff14] text-accent">
          <Sparkles size={15} />
        </span>
        <span className="min-w-0">
          <span className="font-semibold text-accent">这是 AI 起草的预设配置</span>
          <span className="text-ink-3">：{summary}请检查后再保存。</span>
        </span>
      </span>
      <button
        type="button"
        disabled={regenerating}
        onClick={onRegenerate}
        className="motion-colors flex h-7 shrink-0 items-center gap-1 rounded-md border border-line-strong bg-white px-2.5 text-xs font-medium text-ink hover:bg-hover disabled:opacity-60"
      >
        {regenerating ? <LoaderCircle size={12} className="animate-spin" /> : <RotateCcw size={12} />}
        {regenerating ? "重新生成中…" : "重新生成"}
      </button>
    </div>
  );
}
