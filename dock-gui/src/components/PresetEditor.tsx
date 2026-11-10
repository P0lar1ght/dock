// 新建预设：默认手动——起名 + 选图标 + 选底子，建好进编辑器一步步配（每步可问 AI）。
// 「让 AI 先起草一份」收在一行里，点开填描述（Dock `preset/draft`），草稿不写盘，交给
// 预设编辑器逐项审过再保存。手动新建写 ~/.dock/presets，不改默认预设。
// 设计：wonder-ui/preset/PresetCreateManual（can 收起 / was 展开）；生成中沿用
// PresetAiGenerating，失败态并进展开面板。

import { useEffect, useRef, useState } from "react";
import {
  ArrowRight,
  Check,
  ChevronDown,
  ChevronRight,
  ChevronUp,
  CircleAlert,
  FileText,
  GitFork,
  LoaderCircle,
  RotateCcw,
  Sparkles,
  Wrench,
  X,
} from "lucide-react";
import type { AiPresetDraft, PresetOption } from "../model";
import { store } from "../dock/store";
import { MenuButton } from "./Popover";
import { ICON_LIBRARY, iconLabel, presetIcon, type IconCategory } from "./presetIcon";
import { imeComposing } from "../dock/ime";

const TABS: { id: IconCategory | "all"; label: string }[] = [
  { id: "all", label: "全部" },
  { id: "work", label: "工作" },
  { id: "status", label: "状态" },
  { id: "meaning", label: "语义" },
];

const EXAMPLES: { label: string; text: string }[] = [
  { label: "代码审查员", text: "帮我审代码：只读，不许改文件、不许跑命令；发现问题按严重程度列出来，给出修改建议。" },
  { label: "技术周报研究员", text: "每周搜集科技新闻和 GitHub 热点，读原文后写一份 Markdown 技术周报，注明出处。" },
  { label: "全栈调试专家", text: "帮我排查前后端 bug：读日志、跑测试和命令复现问题，定位根因后给出最小修复。" },
];

/** 新建之后在编辑器里依次配的几步。 */
const NEXT_STEPS = [
  { label: "角色提示词", icon: FileText },
  { label: "工具配置", icon: Wrench },
  { label: "子代理名册", icon: GitFork },
];

/** 生成的时候会写出哪几块（一次模型调用，拿不到分步进度，只列内容 + 真实用时）。 */
const PARTS = ["理解需求与定位", "编写角色提示词", "挑选工具并写理由", "按需配置子代理"];

type Phase = { kind: "idle" } | { kind: "generating"; since: number } | { kind: "failed"; message: string };

export function PresetEditor({
  presets,
  defaultBase,
  onClose,
  onCreated,
  onDrafted,
  onOpenModels,
}: {
  presets: PresetOption[];
  /** 手动新建时的底子（新对话页当前选的预设）。 */
  defaultBase: string | null;
  onClose: () => void;
  onCreated: (presetId: string) => void;
  /** AI 起草完成：交给预设编辑器审。 */
  onDrafted: (draft: AiPresetDraft, description: string) => void;
  /** 失败页的「前往 模型与密钥」。 */
  onOpenModels: () => void;
}) {
  const [description, setDescription] = useState("");
  const [name, setName] = useState("");
  const [icon, setIcon] = useState("rocket");
  const [tab, setTab] = useState<IconCategory | "all">("all");
  const usable = presets.filter((p) => p.available);
  const [base, setBase] = useState<string | null>(
    usable.some((p) => p.id === defaultBase) ? defaultBase : (usable[0]?.id ?? null),
  );
  const [phase, setPhase] = useState<Phase>({ kind: "idle" });
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // 每次生成一个号；取消 / 重来后，旧的结果回来直接丢掉（Dock 那边的调用停不下来）。
  const run = useRef(0);
  // AI 起草收在一行里，点开才出描述框：默认手动分步配。
  const [aiOpen, setAiOpen] = useState(false);
  const nameRef = useRef<HTMLInputElement>(null);
  const descRef = useRef<HTMLTextAreaElement>(null);
  useEffect(() => nameRef.current?.focus(), []);
  useEffect(() => {
    if (aiOpen) descRef.current?.focus();
  }, [aiOpen]);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const Icon = ICON_LIBRARY.find((i) => i.name === icon)?.icon ?? presetIcon(null);
  const basePreset = usable.find((p) => p.id === base);
  const canDraft = description.trim() !== "" && phase.kind !== "generating";

  const generate = async () => {
    if (!description.trim()) return;
    const id = ++run.current;
    setError(null);
    setPhase({ kind: "generating", since: Date.now() });
    try {
      const draft = await store.draftPreset(description.trim(), ICON_LIBRARY.map((i) => i.name));
      if (id !== run.current) return;
      // 用户自己起了名 / 挑了图标就以用户的为准。
      onDrafted(
        { ...draft, name: name.trim() || draft.name, icon: icon !== "rocket" ? icon : (draft.icon ?? icon) },
        description.trim(),
      );
    } catch (e) {
      if (id !== run.current) return;
      setPhase({ kind: "failed", message: e instanceof Error ? e.message.replace(/^preset\/draft: /, "") : String(e) });
    }
  };
  const cancelGenerate = () => {
    run.current++;
    setPhase({ kind: "idle" });
  };

  const manual = async () => {
    if (!name.trim()) {
      setError("先起个名字");
      nameRef.current?.focus();
      return;
    }
    setSaving(true);
    setError(null);
    try {
      onCreated(await store.createPreset(name.trim(), icon, base));
    } catch (e) {
      setError(e instanceof Error ? e.message.replace(/^preset\/create: /, "") : String(e));
      setSaving(false);
    }
  };

  return (
    <div className="glass-scrim enter-fade absolute inset-0 z-30 flex items-center justify-center bg-[#f5f5f7]/85 p-8 backdrop-blur-sm">
      <div
        role="dialog"
        aria-label="新建 Agent 预设"
        className="glass-dialog pop-in thin-scroll flex max-h-full w-[560px] max-w-full flex-col gap-4 overflow-y-auto rounded-2xl border border-line-strong bg-white p-6 shadow-[0px_12px_36px_rgba(0,0,0,0.08)]"
      >
        {phase.kind === "generating" ? (
          <Generating since={phase.since} description={description.trim()} onCancel={cancelGenerate} />
        ) : (
          <>
            <div className="flex flex-col gap-0.5">
              <span className="text-base font-semibold text-ink">新建 Agent 预设</span>
              <span className="text-[12.5px] text-ink-4">先起名、挑图标，再在编辑器里一步步配。每一步都可以在旁边问 AI。</span>
            </div>

            <div className="flex items-center gap-3 rounded-xl border border-line-strong bg-[#fbfbfd] px-3 py-2.5">
              <span className="flex size-10 shrink-0 items-center justify-center rounded-[10px] border border-[#0a84ff66] bg-[#0a84ff14] text-accent">
                <Icon key={icon} size={18} className="pop-in" />
              </span>
              <div className="flex min-w-0 flex-1 flex-col gap-1.5">
                <input
                  ref={nameRef}
                  value={name}
                  onChange={(e) => setName(e.target.value)}
                  onKeyDown={(e) => !imeComposing(e) && e.key === "Enter" && manual()}
                  placeholder={aiOpen ? "预设名称（起草时可以留空）" : "预设名称，如：代码审查员"}
                  maxLength={40}
                  className="motion-colors h-8 rounded-lg border border-[#d1d1d6] bg-white px-2.5 text-[13px] font-medium text-ink outline-none select-text placeholder:font-normal placeholder:text-ink-4 focus:border-accent focus:shadow-[0px_0px_0px_3px_rgba(10,132,255,0.12)]"
                />
                <div className="flex min-w-0 items-center gap-1 text-[11.5px] text-ink-4">
                  <span className="shrink-0">以</span>
                  <MenuButton
                    title="从哪个预设复制提示词、工具和子代理"
                    side="bottom"
                    align="left"
                    header="以这个预设为底"
                    options={usable.map((p) => ({ id: p.id, label: p.label, hint: p.description, icon: presetIcon(p.id) }))}
                    value={base ?? ""}
                    onPick={setBase}
                    className="flex shrink-0 items-center gap-0.5 rounded-md px-1 font-medium text-ink-2 hover:bg-hover"
                    trigger={
                      <>
                        {basePreset?.label ?? "空白"}
                        <ChevronDown size={11} className="text-ink-4" />
                      </>
                    }
                  />
                  <span className="truncate">为底：复制它的提示词、工具和子代理，再改</span>
                </div>
              </div>
            </div>

            <div className="flex flex-col gap-2">
              <div className="flex items-center justify-between px-0.5">
                <span className="text-[12.5px] font-semibold text-ink">图标</span>
                <div className="flex items-center gap-1">
                  {TABS.map((t) => (
                    <button
                      key={t.id}
                      type="button"
                      onClick={() => setTab(t.id)}
                      className={`motion-colors rounded-md px-2 py-0.5 text-[11px] ${
                        tab === t.id ? "bg-[#0a84ff1a] font-medium text-accent" : "text-ink-3 hover:bg-hover"
                      }`}
                    >
                      {t.label}
                    </button>
                  ))}
                </div>
              </div>
              <div className="grid grid-cols-8 gap-1.5 rounded-xl border border-line-strong bg-[#fbfbfd] p-2">
                {ICON_LIBRARY.filter((i) => tab === "all" || i.category === tab).map(({ name: n, icon: I }) => {
                  const on = n === icon;
                  return (
                    <button
                      key={n}
                      type="button"
                      title={iconLabel(n)}
                      aria-pressed={on}
                      onClick={() => setIcon(n)}
                      className={`motion-colors relative flex size-9 items-center justify-center justify-self-center rounded-lg border active:scale-95 ${
                        on
                          ? "border-accent bg-white text-accent shadow-[0px_0px_0px_2px_rgba(10,132,255,0.2)]"
                          : "border-transparent text-ink-3 hover:border-line-strong hover:bg-white hover:text-ink"
                      }`}
                    >
                      <I size={16} />
                      {on && (
                        <span className="pop-in absolute -right-0.5 -top-0.5 flex size-3.5 items-center justify-center rounded-full bg-accent text-white">
                          <Check size={9} strokeWidth={3} />
                        </span>
                      )}
                    </button>
                  );
                })}
              </div>
            </div>

            <div className="flex items-center gap-1.5 rounded-xl bg-[#f5f5f7] px-3 py-2 text-[11.5px] text-ink-3">
              <span className="mr-1 shrink-0 font-medium text-ink-2">接下来</span>
              {NEXT_STEPS.map(({ label, icon: I }, i) => (
                <span key={label} className="flex items-center gap-1.5">
                  {i > 0 && <ChevronRight size={11} className="text-ink-5" />}
                  <span className="flex items-center gap-1">
                    <I size={12} />
                    {label}
                  </span>
                </span>
              ))}
            </div>

            {aiOpen ? (
              <div className="enter-fade flex flex-col gap-2.5 rounded-xl border border-[#0a84ff4d] bg-[#0a84ff0a] px-3 py-3">
                <div className="flex items-center justify-between">
                  <span className="flex items-center gap-2 text-[12.5px] font-semibold text-ink">
                    <Sparkles size={14} className="text-accent" />
                    让 AI 先起草一份
                  </span>
                  <button
                    type="button"
                    onClick={() => setAiOpen(false)}
                    className="motion-colors flex items-center gap-0.5 text-[11px] text-ink-4 hover:text-ink"
                  >
                    收起 <ChevronUp size={12} />
                  </button>
                </div>
                {phase.kind === "failed" && (
                  <div className="enter-fade flex flex-col gap-1.5 rounded-lg border border-[#ffd0cc] bg-[#fff5f5] px-3 py-2.5">
                    <span className="flex items-center gap-2 text-[12.5px] font-semibold text-[#d70015]">
                      <CircleAlert size={14} className="shrink-0" />
                      起草失败（已保留你的输入）
                    </span>
                    <span className="text-xs leading-relaxed text-[#b3261e]">{phase.message}</span>
                    <span className="flex items-center gap-1.5 text-xs text-ink-3">
                      <button type="button" onClick={onOpenModels} className="flex items-center gap-0.5 text-accent hover:underline">
                        查看「设置 → 模型与密钥」 <ArrowRight size={11} />
                      </button>
                      <span className="text-ink-5">·</span>
                      或者收起，手动配置
                    </span>
                  </div>
                )}
                <div className="motion-colors flex flex-col rounded-lg border border-[#d1d1d6] bg-white p-2.5 focus-within:border-accent focus-within:shadow-[0px_0px_0px_3px_rgba(10,132,255,0.12)]">
                  <textarea
                    ref={descRef}
                    value={description}
                    onChange={(e) => setDescription(e.target.value)}
                    onKeyDown={(e) => {
                      if (e.key === "Enter" && (e.metaKey || e.ctrlKey) && !imeComposing(e)) generate();
                    }}
                    rows={3}
                    maxLength={2000}
                    placeholder="用一两句话描述你想要的 Agent，例如：帮我审查 PR 并自动跑测试的 Rust CI 工程师…"
                    className="min-h-[54px] resize-none bg-transparent text-[13px] leading-relaxed text-ink outline-none select-text placeholder:text-ink-4"
                  />
                  <div className="mt-1 flex items-center gap-1.5 border-t border-line-soft pt-2">
                    <span className="text-[11px] text-ink-4">示例：</span>
                    {EXAMPLES.map((x) => (
                      <button
                        key={x.label}
                        type="button"
                        onClick={() => setDescription(x.text)}
                        className="motion-colors rounded-md border border-line-strong bg-white px-2 py-0.5 text-[11px] text-ink-3 hover:border-accent hover:text-accent"
                      >
                        {x.label}
                      </button>
                    ))}
                  </div>
                </div>
                <div className="flex items-center justify-between gap-3">
                  <span className="text-[11px] text-ink-4">起草后进编辑器逐项审，每处都可以改。保存前不写盘</span>
                  <button
                    type="button"
                    disabled={!canDraft}
                    onClick={generate}
                    title="⌘Enter"
                    className="motion-colors flex h-[28px] shrink-0 items-center gap-1.5 rounded-[7px] border border-[#0a84ff66] bg-white px-3 text-xs font-medium text-accent hover:bg-[#0a84ff0d] disabled:opacity-40"
                  >
                    {phase.kind === "failed" ? <RotateCcw size={12} /> : <Sparkles size={12} />}
                    {phase.kind === "failed" ? "重试起草" : "起草并打开编辑器"}
                  </button>
                </div>
              </div>
            ) : (
              <button
                type="button"
                onClick={() => setAiOpen(true)}
                className="motion-colors flex items-center justify-between rounded-xl border border-dashed border-[#d1d1d6] px-3 py-2.5 text-left hover:border-accent hover:bg-[#0a84ff08]"
              >
                <span className="flex items-center gap-2.5">
                  <Sparkles size={14} className="shrink-0 text-accent" />
                  <span className="flex flex-col">
                    <span className="text-[12.5px] font-medium text-ink">不想从空白开始？让 AI 先起草一份</span>
                    <span className="text-[11px] text-ink-4">起草后在编辑器里逐项审，保存前不会写盘</span>
                  </span>
                </span>
                <ChevronDown size={14} className="shrink-0 text-ink-4" />
              </button>
            )}

            {error && <div className="enter-fade text-xs text-[#c0392b]">{error}</div>}

            <div className="flex items-center justify-between border-t border-line pt-3">
              <button type="button" onClick={onClose} className="motion-colors text-[13px] font-medium text-ink-3 hover:text-ink">
                取消
              </button>
              <button
                type="button"
                disabled={saving}
                onClick={manual}
                title={name.trim() ? "按上面的名字、图标和底子新建，然后进编辑器" : "先起个名字"}
                className="motion-colors flex h-[34px] items-center gap-1.5 rounded-lg bg-accent px-4 text-[13px] font-medium text-white shadow hover:bg-[#0077ed] active:scale-[0.98] disabled:opacity-50"
              >
                {saving && <LoaderCircle size={13} className="animate-spin" />}
                创建并开始配置
                <ArrowRight size={13} />
              </button>
            </div>
          </>
        )}
      </div>
    </div>
  );
}

/** 生成中（设计 PresetAiGenerating）：锁定的描述 + 会写出的几块 + 真实用时。 */
function Generating({ since, description, onCancel }: { since: number; description: string; onCancel: () => void }) {
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), 250);
    return () => clearInterval(t);
  }, []);
  const seconds = Math.floor((now - since) / 1000);
  return (
    <>
      <div className="flex items-start justify-between gap-3">
        <div className="flex flex-col gap-0.5">
          <span className="flex items-center gap-2 text-base font-semibold text-ink">
            AI 正在生成预设
            <span className="pulse-halo size-2 rounded-full bg-accent" />
          </span>
          <span className="text-[12.5px] text-ink-4">正在调用默认模型起草整份配置，通常 5–20 秒</span>
        </div>
        <span className="flex shrink-0 items-center gap-1 rounded-full border border-[#0a84ff30] bg-[#0a84ff10] px-2.5 py-0.5 font-mono text-[11px] font-medium text-accent">
          <LoaderCircle size={11} className="animate-spin" />
          已用 {seconds}s
        </span>
      </div>
      <div className="flex flex-col gap-1.5">
        <span className="text-xs text-ink-3">需求描述 (已锁定)</span>
        <div className="max-h-24 overflow-y-auto rounded-xl border border-line-strong bg-[#f5f5f7] px-3.5 py-2.5 text-[13px] leading-relaxed text-ink-2">
          {description}
        </div>
      </div>
      <div className="flex flex-col gap-2 rounded-xl border border-line-strong bg-[#fafafc] p-3.5">
        <div className="flex items-center justify-between">
          <span className="text-[12.5px] font-semibold text-ink">这次会写出</span>
          <span className="text-[11px] text-ink-4">一次生成，写完一起回来</span>
        </div>
        {PARTS.map((p, i) => (
          <div
            key={p}
            className="enter-up flex items-center gap-2.5 rounded-lg px-2 py-1.5 text-[13px] text-ink-2"
            style={{ animationDelay: `${i * 60}ms` }}
          >
            <LoaderCircle size={14} className="shrink-0 animate-spin text-accent" />
            {p}
          </div>
        ))}
      </div>
      <div className="flex items-center justify-between border-t border-line pt-3">
        <button
          type="button"
          onClick={onCancel}
          className="motion-colors flex h-[34px] items-center gap-1.5 rounded-lg border border-line-strong bg-white px-3.5 text-[13px] font-medium text-ink shadow-[0px_1px_0px_rgba(0,0,0,0.05)] hover:bg-hover"
        >
          <X size={13} />
          取消生成
        </button>
        <span className="flex h-[34px] items-center gap-1.5 rounded-lg bg-accent px-4 text-[13px] font-medium text-white opacity-80 shadow">
          <LoaderCircle size={13} className="animate-spin" />
          正在生成预设…
        </span>
      </div>
    </>
  );
}
