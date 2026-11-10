// 会话里模型起草的预设（`propose_preset`）：草稿卡，用户点「在编辑器中打开」逐项审、
// 自己保存。设计：wonder-ui/preset/PresetDraftCard（los 待审 / jet 已保存）。
// Dock 不写盘；保存走预设编辑器（App 听 `dock:review-preset-draft`）。存成了哪个预设
// 记在 localStorage（按线程 + 调用 id），回看会话时卡片仍显示「已保存」。

import { useEffect, useState } from "react";
import { ArrowUpRight, Bot, FileText, Folder, GitFork, LoaderCircle, Pencil, Wrench } from "lucide-react";
import type { AiPresetDraft } from "../../model";

export interface ReviewPresetDraft {
  draft: AiPresetDraft;
  description: string;
  /** 这张卡的键：保存后回 `dock:preset-draft-saved`。 */
  origin: string;
}

const SAVED_KEY = (origin: string) => `dock.presetDraftSaved:${origin}`;

const text = (v: unknown) => (typeof v === "string" ? v.trim() : "");

/** 模型给的参数 → 编辑器的草稿（Dock 那边已按同样规矩校验过；编辑器保存时再校验一次）。 */
export function draftFromArgs(args: Record<string, unknown>): AiPresetDraft {
  const picks = Array.isArray(args.tools)
    ? args.tools
        .map((t) => (typeof t === "string" ? { name: t, reason: "" } : { name: text((t as { name?: unknown }).name), reason: text((t as { reason?: unknown }).reason) }))
        .filter((t) => t.name)
    : [];
  const agents = Array.isArray(args.subagents)
    ? args.subagents.slice(0, 3).map((a) => {
        const o = (a ?? {}) as Record<string, unknown>;
        const tools = Array.isArray(o.tools) ? o.tools.filter((t): t is string => typeof t === "string") : [];
        return {
          id: text(o.id).toLowerCase(),
          name: text(o.name) || text(o.id),
          description: text(o.description),
          persona: text(o.persona),
          tools: tools.length ? tools : null,
          replacePrompt: false,
          listings: false,
        };
      })
    : [];
  return {
    name: text(args.name) || "新预设",
    description: text(args.description),
    icon: null,
    persona: text(args.persona),
    replacePrompt: false,
    tools: picks.length ? picks.map((p) => p.name) : null,
    toolReasons: picks,
    agents,
  };
}

export function PresetDraftCard({ origin, args, running }: { origin: string; args: Record<string, unknown>; running: boolean }) {
  const [saved, setSaved] = useState<string | null>(() => localStorage.getItem(SAVED_KEY(origin)));
  useEffect(() => {
    const on = (e: Event) => {
      const d = (e as CustomEvent<{ origin: string; id: string }>).detail;
      if (d.origin === origin) setSaved(d.id);
    };
    window.addEventListener("dock:preset-draft-saved", on);
    return () => window.removeEventListener("dock:preset-draft-saved", on);
  }, [origin]);

  const draft = draftFromArgs(args);
  const open = () =>
    window.dispatchEvent(
      new CustomEvent<ReviewPresetDraft>("dock:review-preset-draft", {
        detail: { draft, description: draft.description || draft.name, origin },
      }),
    );

  return (
    <div className="md-surface enter-fade mt-1 flex w-full shrink-0 flex-col overflow-hidden rounded-[10px] border border-[#e5e5ea] bg-white">
      <div className="flex items-center gap-2 px-3 py-2.5">
        <Bot size={14} className="shrink-0 text-accent" />
        <span className="shrink-0 text-[13px] font-medium text-ink">预设草稿</span>
        <span className="min-w-0 flex-1 truncate text-[13px] text-ink-3">
          {draft.name}
          {draft.description && ` · ${draft.description}`}
        </span>
        {running ? (
          <span className="flex shrink-0 items-center gap-1 text-[11px] text-ink-4">
            <LoaderCircle size={11} className="animate-spin" /> 起草中
          </span>
        ) : saved ? (
          <span className="shrink-0 rounded-full border border-[#34c75966] bg-[#34c7591a] px-2 py-0.5 text-[11px] font-medium text-[#1f8a3b]">已保存</span>
        ) : (
          <span className="shrink-0 rounded-full border border-[#ff9f0a66] bg-[#ff9f0a1a] px-2 py-0.5 text-[11px] font-medium text-[#b25000]">等你审阅 · 未保存</span>
        )}
      </div>
      {!running && (
        <div className="flex flex-col gap-2.5 border-t border-[#ececf0] px-3 py-2.5">
          <div className="grid grid-cols-3 gap-2">
            <Cell icon={FileText} label="角色提示词" value={`${[...draft.persona].length} 字 · 追加到内置提示后`} />
            <Cell
              icon={Wrench}
              label={draft.tools === null ? "工具" : `工具 ${draft.tools.length}`}
              value={draft.tools === null ? "全部工具" : draft.tools.join(" · ")}
              mono={draft.tools !== null}
            />
            <Cell
              icon={GitFork}
              label={`子代理 ${draft.agents.length}`}
              value={draft.agents.length ? draft.agents.map((a) => a.name || a.id).join("、") : "不拆子代理"}
            />
          </div>
          <div className="flex items-center justify-between gap-3">
            <span className="flex min-w-0 items-center gap-1 text-[11px] text-ink-4">
              <Folder size={12} className="shrink-0" />
              <span className="truncate">
                {saved ? `已写入 ~/.dock/presets/${saved}/ · 新对话里就能选它` : "还没写盘：在编辑器里改完再保存，任何一处都可以改"}
              </span>
            </span>
            {saved ? (
              <button
                type="button"
                onClick={() => window.dispatchEvent(new CustomEvent("dock:edit-preset", { detail: saved }))}
                className="motion-colors flex h-[26px] shrink-0 items-center gap-1 rounded-[7px] border border-line-strong bg-white px-3 text-[11px] font-semibold text-ink hover:bg-hover"
              >
                再次编辑 <Pencil size={12} className="text-ink-3" />
              </button>
            ) : (
              <button
                type="button"
                onClick={open}
                className="motion-colors flex h-[26px] shrink-0 items-center gap-1 rounded-[7px] bg-ink px-3 text-[11px] font-semibold text-white hover:bg-ink-2"
              >
                在编辑器中打开 <ArrowUpRight size={12} />
              </button>
            )}
          </div>
        </div>
      )}
    </div>
  );
}

function Cell({ icon: I, label, value, mono }: { icon: typeof Wrench; label: string; value: string; mono?: boolean }) {
  return (
    <div className="flex min-w-0 flex-col gap-1 rounded-lg bg-[#f7f7f8] px-2.5 py-2">
      <span className="flex items-center gap-1 text-[11px] text-ink-4">
        <I size={12} />
        {label}
      </span>
      <span className={`truncate text-ink ${mono ? "font-mono text-xs" : "text-[12.5px]"}`} title={value}>
        {value}
      </span>
    </div>
  );
}

/** 编辑器把这张卡的草稿存成了预设（App 发 `dock:preset-draft-saved`）：记下来，回看会话时仍是「已保存」。 */
export function rememberSavedDrafts() {
  window.addEventListener("dock:preset-draft-saved", (e) => {
    const d = (e as CustomEvent<{ origin: string; id: string }>).detail;
    localStorage.setItem(SAVED_KEY(d.origin), d.id);
  });
}
