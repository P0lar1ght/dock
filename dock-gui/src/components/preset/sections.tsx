// 预设编辑器的各分区（设计 emil / paty / rone / moyo / tway / monk / oban）。
// 只画和改草稿；保存、校验结果、弹窗在 PresetStudio。

import { useState } from "react";
import {
  Bot,
  ChartColumn,
  Check,
  FileText,
  Globe,
  Lock,
  Plus,
  Search,
  Sparkles,
  Trash2,
  Info,
  Wrench,
  type LucideIcon,
} from "lucide-react";
import type { CatalogTool, PresetDetail, PresetDraft, SubagentDraft } from "../../model";
import { ICON_LIBRARY, iconLabel, presetIcon, type IconCategory } from "../presetIcon";
import { PromptEditor } from "./PromptEditor";
import { PromptRewrite, SuggestButton, SuggestPanel, type Suggestions } from "./ai";
import { ToolModeSwitch, ToolPicker } from "./ToolPicker";
import { Field, SectionHeader, Segmented, Switch } from "./ui";
import { input, inputError, inputLocked, toolCountLabel } from "./util";

export interface Errors {
  name?: string;
  persona?: string;
  /** 按子代理下标。 */
  agents: Record<number, { id?: string; persona?: string }>;
}

type Patch = (p: Partial<PresetDraft>) => void;

// ---- 基本信息 ----

const ICON_TABS: { id: IconCategory | "all"; label: string }[] = [
  { id: "all", label: "全部" },
  { id: "work", label: "工作" },
  { id: "status", label: "状态" },
  { id: "meaning", label: "语义" },
];

export function BasicSection({
  detail,
  draft,
  patch,
  errors,
}: {
  detail: PresetDetail;
  draft: PresetDraft;
  patch: Patch;
  errors: Errors | null;
}) {
  const [tab, setTab] = useState<IconCategory | "all">("all");
  const builtin = detail.builtin;
  const Current = (draft.icon && ICON_LIBRARY.find((i) => i.name === draft.icon)?.icon) || presetIcon(detail.id);
  return (
    <>
      <SectionHeader
        title="预设基本信息"
        subtitle={
          builtin
            ? `内置预设「${detail.name}」的基础信息，支持定制说明并覆盖保存`
            : "配置预设的名称、展示图标、简介描述以及在 Dock 目录中的保存位置"
        }
      />
      <div className="flex max-w-[800px] flex-col gap-5">
        <Field
          label={builtin ? "预设显示名称 (内置只读)" : "预设显示名称"}
          hint={`ID: ${detail.id}${builtin ? " (built-in)" : ""}`}
          error={errors?.name}
        >
          {builtin ? (
            <div className={`${input} ${inputLocked} flex items-center justify-between`}>
              {draft.name}
              <Lock size={13} className="text-ink-4" />
            </div>
          ) : (
            <input
              value={draft.name}
              onChange={(e) => patch({ name: e.target.value })}
              placeholder="请输入预设名称（不能为空）"
              maxLength={40}
              className={`${input} ${errors?.name ? inputError : ""}`}
            />
          )}
        </Field>

        <div className="flex flex-col gap-2">
          <span className="text-[13px] font-semibold text-ink">预设图标</span>
          <div className="flex flex-col gap-3 rounded-xl border border-line-strong bg-[#fafafc] p-3.5">
            <div className="flex items-center justify-between">
              <div className="flex items-center gap-2.5">
                <span className="flex size-9 items-center justify-center rounded-lg bg-accent text-white shadow">
                  <Current key={draft.icon ?? detail.id} size={20} className="pop-in" />
                </span>
                <span className="flex flex-col">
                  <span className="text-[12.5px] font-semibold text-ink">
                    {builtin ? `${detail.name} · 内置图标` : draft.icon ? iconLabel(draft.icon) : "默认图标"}
                  </span>
                  <span className="font-mono text-[11px] text-ink-4">
                    {builtin ? "内置预设保留语义图标" : draft.icon ? `${draft.icon} (lucide)` : "从下面选一个"}
                  </span>
                </span>
              </div>
              {!builtin && (
                <div className="flex gap-0.5 rounded-lg bg-line-strong p-0.5">
                  {ICON_TABS.map((t) => (
                    <button
                      key={t.id}
                      type="button"
                      onClick={() => setTab(t.id)}
                      className={`motion-colors rounded-md px-2.5 py-1 text-[11.5px] ${
                        tab === t.id ? "bg-white font-semibold text-ink shadow-sm" : "font-medium text-ink-3 hover:text-ink"
                      }`}
                    >
                      {t.label}
                    </button>
                  ))}
                </div>
              )}
            </div>
            {!builtin && (
              <div className="grid grid-cols-8 gap-2 border-t border-line-soft pt-3">
                {ICON_LIBRARY.filter((i) => tab === "all" || i.category === tab).map(({ name, icon: I }) => {
                  const on = name === draft.icon;
                  return (
                    <button
                      key={name}
                      type="button"
                      title={iconLabel(name)}
                      aria-pressed={on}
                      onClick={() => patch({ icon: name })}
                      className={`motion-colors flex size-9 items-center justify-center rounded-lg bg-white active:scale-95 ${
                        on ? "border-2 border-accent text-accent shadow" : "border border-line-strong text-ink-2 hover:border-[#d1d1d6]"
                      }`}
                    >
                      <I size={16} />
                    </button>
                  );
                })}
              </div>
            )}
          </div>
        </div>

        <Field label={builtin ? "一句话描述 (可定制)" : "一句话描述"}>
          <input
            value={draft.description}
            onChange={(e) => patch({ description: e.target.value })}
            placeholder="这个 Agent 擅长什么"
            className={input}
          />
        </Field>

        {builtin ? (
          <Field label="覆盖生效位置">
            <div className={`${input} ${inputLocked} flex items-center justify-between text-[12.5px]`}>
              <span className="truncate">全局覆盖文件：~/.dock/presets/{detail.id}/agent.yml</span>
              <span className="shrink-0 rounded-sm border border-[#d1d1d6] bg-white px-2 py-0.5 text-[11px] font-medium text-ink-3">
                单向覆盖
              </span>
            </div>
          </Field>
        ) : (
          <div className="grid grid-cols-2 gap-4">
            <Field label="在列表里的排序权重">
              <label className={`${input} flex items-center gap-2`}>
                <input
                  type="number"
                  value={draft.order ?? ""}
                  onChange={(e) => patch({ order: e.target.value === "" ? null : Math.trunc(Number(e.target.value)) })}
                  placeholder="不排"
                  className="min-w-0 flex-1 bg-transparent outline-none select-text placeholder:text-ink-5"
                />
                <span className="shrink-0 text-[11.5px] text-ink-4">(数字越小越靠前)</span>
              </label>
            </Field>
            <Field label="保存位置">
              <Segmented
                value={detail.origin === "project" ? "project" : "user"}
                onChange={() => {}}
                options={[
                  {
                    id: "user",
                    label: "全局 (~/.dock/presets)",
                    disabled: detail.origin === "project",
                  },
                  {
                    id: "project",
                    label: "当前项目 (.dock/presets)",
                    disabled: detail.origin !== "project",
                    title: "即将推出：桌面端现在只写全局预设",
                  },
                ]}
              />
            </Field>
          </div>
        )}
      </div>
    </>
  );
}

// ---- 角色提示词 ----

export function PromptSection({
  draft,
  patch,
  errors,
  context,
}: {
  draft: PresetDraft;
  patch: Patch;
  errors: Errors | null;
  /** 给 AI 润色 / 扩写的背景（预设的一句话说明，AI 草稿用用户的原描述）。 */
  context: string;
}) {
  const on = draft.replacePrompt;
  return (
    <>
      <SectionHeader title="角色系统提示词" subtitle="设定 Agent 的专业角色身份、思维方式、输出格式规范与边界规则" />
      <div className="flex items-center justify-between gap-4 rounded-xl border border-line-strong bg-[#fafafc] p-3.5">
        <div className="flex flex-1 flex-col gap-1">
          <div className="flex items-center gap-2">
            <span className="text-[13px] font-semibold text-ink">整份替换系统提示 (Override Full Prompt)</span>
            <span
              key={String(on)}
              className={`pop-in rounded-full px-2 py-0.5 text-[11px] font-medium ${
                on ? "bg-[#ff95001a] text-[#c26a00]" : "bg-[#f2f2f7] text-ink-3"
              }`}
            >
              {on ? "已开启 · 整份替换" : "已关闭 · 追加模式"}
            </span>
          </div>
          <span className="text-xs leading-normal text-ink-4">
            {on
              ? "你的提示词就是整份系统提示：Dock 不再注入默认的工具说明与基础指令，要用的规矩得自己写进去。"
              : "当前为追加模式：你的提示词接在 Dock 内置系统提示后面。开启后整份替换，Dock 不再注入工具说明与基础指令。"}
          </span>
        </div>
        <Switch on={on} onChange={(v) => patch({ replacePrompt: v })} label="整份替换系统提示" />
      </div>
      <PromptEditor
        value={draft.persona}
        onChange={(persona) => patch({ persona })}
        invalid={!!errors?.persona}
        actions={<PromptRewrite persona={draft.persona} context={context} onApply={(persona) => patch({ persona })} />}
        placeholder="写下这个 Agent 的角色、做事方式和边界，例如：你是一名代码审查员，只读仓库……"
        footer={
          <>
            <span>支持 Markdown；{on ? "整份替换 Dock 的系统提示" : "追加在 Dock 系统提示之后"}</span>
            {errors?.persona ? (
              <span className="enter-fade font-medium text-[#ff3b30]">{errors.persona}</span>
            ) : (
              <span className="flex items-center gap-1 font-medium text-[#34c759]">
                <span className="size-1.5 rounded-full bg-[#34c759]" />
                {on ? "替换模式" : "追加模式"}
              </span>
            )}
          </>
        }
      />
    </>
  );
}

// ---- 工具 ----

export function ToolsSection({
  draft,
  patch,
  catalog,
  onOpenMcp,
  reasons,
  suggest,
  onSuggest,
  onSuggestAccept,
  onSuggestDismiss,
  onSuggestClose,
}: {
  draft: PresetDraft;
  patch: Patch;
  catalog: CatalogTool[] | null;
  onOpenMcp: () => void;
  /** AI 草稿里挑工具的理由。 */
  reasons?: Record<string, string>;
  suggest: Suggestions | null;
  onSuggest: () => void;
  onSuggestAccept: (names: string[]) => void;
  onSuggestDismiss: (name: string) => void;
  onSuggestClose: () => void;
}) {
  const [picking, setPicking] = useState(false);
  const noContext = !draft.description.trim() && !draft.persona.trim();
  const tools = { value: draft.tools, onChange: (t: string[] | null) => patch({ tools: t }), catalog, picking, onPicking: setPicking };
  return (
    <>
      <SectionHeader
        title="可用工具配置"
        subtitle="为该预设授权调用的核心系统工具与能力接口"
        actions={
          <span className="flex shrink-0 items-center gap-2.5">
            <SuggestButton
              loading={!!suggest?.loading}
              disabled={draft.tools === null || noContext}
              reason={
                draft.tools === null
                  ? "现在是「全部工具」，切到自选后再让 AI 推荐"
                  : noContext
                    ? "先写一句话描述或角色提示词"
                    : "按描述和角色提示词推荐工具"
              }
              onClick={onSuggest}
            />
            <ToolModeSwitch {...tools} />
          </span>
        }
      />
      {suggest && (
        <SuggestPanel s={suggest} onAccept={onSuggestAccept} onDismiss={onSuggestDismiss} onClose={onSuggestClose} />
      )}
      <ToolPicker {...tools} onOpenMcp={onOpenMcp} modeSwitch={false} reasons={reasons} />
    </>
  );
}

// ---- 子代理名册 ----

const AGENT_LOOKS: { icon: LucideIcon; tone: string }[] = [
  { icon: Globe, tone: "bg-[#eef5ff] text-accent" },
  { icon: ChartColumn, tone: "bg-[#f0fdf4] text-[#16a34a]" },
  { icon: Bot, tone: "bg-[#f5f3ff] text-[#7c3aed]" },
  { icon: Search, tone: "bg-[#fff7ed] text-[#ea580c]" },
  { icon: FileText, tone: "bg-[#f2f2f7] text-ink-3" },
  { icon: Sparkles, tone: "bg-[#fdf2f8] text-[#db2777]" },
];

function agentLook(i: number) {
  return AGENT_LOOKS[i % AGENT_LOOKS.length];
}

export function AgentsSection({
  draft,
  errors,
  onAdd,
  onEdit,
  onCopy,
  onRemove,
}: {
  draft: PresetDraft;
  errors: Errors | null;
  onAdd: () => void;
  onEdit: (i: number) => void;
  onCopy: (i: number) => void;
  onRemove: (i: number) => void;
}) {
  return (
    <>
      <SectionHeader
        title="子代理名册 (Sub-agents)"
        subtitle="为该 Agent 配置专属派发的子代理。各子代理拥有独立的角色职责、提示词与工具范围"
        actions={
          <button
            type="button"
            onClick={onAdd}
            className="motion-colors flex h-8 shrink-0 items-center gap-1.5 rounded-lg bg-ink px-3.5 text-[12.5px] font-medium text-white shadow hover:bg-ink-2 active:scale-[0.98]"
          >
            <Plus size={13} />
            新建子代理
          </button>
        }
      />
      <div className="flex max-w-[920px] flex-col gap-3.5">
        {draft.agents.length === 0 && (
          <div className="enter-fade flex flex-col items-center gap-2 rounded-xl border border-dashed border-[#d1d1d6] px-6 py-10 text-center">
            <span className="text-[13px] font-semibold text-ink">还没有子代理</span>
            <span className="text-xs text-ink-4">主 Agent 可以把一块活派给子代理在后台做，比如只读的代码探索、网页调研。</span>
          </div>
        )}
        {draft.agents.map((a, i) => {
          const { icon: Icon, tone } = agentLook(i);
          const bad = errors?.agents[i];
          return (
            <div
              key={i}
              className={`enter-up flex items-center justify-between gap-3 rounded-xl border bg-white p-4 shadow ${
                bad ? "border-[#ff3b30]" : "border-line-strong"
              }`}
            >
              <div className="flex min-w-0 flex-1 items-center gap-3.5">
                <span className={`flex size-10 shrink-0 items-center justify-center rounded-lg ${tone}`}>
                  <Icon size={20} />
                </span>
                <div className="flex min-w-0 flex-col gap-1">
                  <div className="flex items-center gap-2">
                    <span className="text-sm font-semibold text-ink">{a.id || "（未命名）"}</span>
                    {a.name && a.name !== a.id && <span className="text-[12.5px] text-ink-3">{a.name}</span>}
                    <span className="rounded-sm bg-[#f2f2f7] px-2 py-0.5 text-[11px] font-medium text-ink-3">
                      {a.builtin ? "内置角色" : "预设专属"}
                    </span>
                  </div>
                  <span className={`truncate text-[12.5px] ${bad ? "text-[#ff3b30]" : "text-ink-4"}`}>
                    {bad ? (bad.id ?? bad.persona) : a.description || "（没有职责说明）"}
                  </span>
                </div>
              </div>
              <span className="flex shrink-0 items-center gap-1.5 rounded-md border border-line-strong bg-[#fafafc] px-2.5 py-1 text-xs font-medium text-ink-2">
                <Wrench size={14} className="text-accent" />
                {toolCountLabel(a.tools)}
              </span>
              <div className="flex shrink-0 items-center gap-1.5">
                <button
                  type="button"
                  onClick={() => onCopy(i)}
                  className="motion-colors h-7 rounded-md border border-line-strong bg-white px-2.5 text-xs font-medium text-ink-2 hover:bg-hover"
                >
                  复制
                </button>
                <button
                  type="button"
                  onClick={() => onEdit(i)}
                  className="motion-colors h-7 rounded-md border border-[#d0e4ff] bg-[#eef5ff] px-2.5 text-xs font-semibold text-accent hover:bg-[#e2eeff]"
                >
                  编辑
                </button>
                <button
                  type="button"
                  onClick={() => onRemove(i)}
                  disabled={a.builtin}
                  title={a.builtin ? "内置角色删不掉；在预设管理里「恢复内置」可以还原它" : "从名册里删除（保存后生效）"}
                  className="motion-colors flex h-7 items-center rounded-md px-2 text-[#ff3b30] hover:bg-[#ff3b3014] disabled:opacity-30 disabled:hover:bg-transparent"
                >
                  <Trash2 size={14} />
                </button>
              </div>
            </div>
          );
        })}
        <div className="flex items-center gap-3 rounded-xl border border-line-strong bg-[#fafafc] px-4 py-3.5 text-[12.5px] text-ink-3">
          <Info size={16} className="shrink-0 text-accent" />
          子代理由主 Agent 通过 task 工具派发，在后台运行；跑完一轮会自动通知主 Agent，也能用 send_message 继续交流。
        </div>
      </div>
    </>
  );
}

// ---- 子代理详情 ----

export function AgentEditor({
  agent,
  onChange,
  errors,
  onPickTools,
}: {
  agent: SubagentDraft;
  onChange: (a: Partial<SubagentDraft>) => void;
  errors: { id?: string; persona?: string } | undefined;
  onPickTools: () => void;
}) {
  const chars = [...agent.persona].length;
  return (
    <div className="mx-auto flex w-full max-w-[880px] flex-col gap-5">
      <div className="flex items-start justify-between border-b border-line-soft pb-4">
        <div className="flex flex-col gap-1">
          <h2 className="text-[19px] font-semibold text-ink">子代理配置：{agent.id || "新子代理"}</h2>
          <p className="text-[12.5px] text-ink-4">配置子代理在后台运行时的专属身份、系统提示与受控工具面</p>
        </div>
        {agent.builtin && (
          <span className="rounded-md bg-[#0a84ff1a] px-2.5 py-1 text-[11.5px] font-semibold text-accent">内置角色</span>
        )}
      </div>

      <div className="grid grid-cols-2 gap-3.5">
        <Field
          label="子代理标识名称"
          hint={agent.fresh ? "task 的 subagent_type" : "id 不能改"}
          error={errors?.id}
        >
          {agent.fresh ? (
            <input
              value={agent.id}
              onChange={(e) => onChange({ id: e.target.value.trim() })}
              placeholder="如 web-researcher（小写字母、数字、-，或 1–16 个汉字）"
              autoCapitalize="off"
              autoCorrect="off"
              spellCheck={false}
              maxLength={32}
              className={`${input} h-[34px] font-mono text-[12.5px] ${errors?.id ? inputError : ""}`}
            />
          ) : (
            <div className={`${input} ${inputLocked} flex h-[34px] items-center justify-between font-mono text-[12.5px]`}>
              {agent.id}
              <Lock size={13} className="text-ink-4" />
            </div>
          )}
        </Field>
        <Field label="显示名称">
          <input
            value={agent.name}
            onChange={(e) => onChange({ name: e.target.value })}
            placeholder={agent.id || "默认用 id"}
            className={`${input} h-[34px] text-[12.5px]`}
          />
        </Field>
      </div>

      <Field label="子代理职责说明">
        <input
          value={agent.description}
          onChange={(e) => onChange({ description: e.target.value })}
          placeholder="一句话说明它负责什么（主 Agent 按这个决定派给谁）"
          className={`${input} h-[34px] text-[12.5px]`}
        />
      </Field>

      <div
        className={`flex flex-col gap-3 rounded-xl border bg-white p-4 shadow-[0px_1px_0px_rgba(0,0,0,0.05)] ${
          errors?.persona ? "border-[#ff3b30]" : "border-line-strong"
        }`}
      >
        <div className="flex items-center justify-between">
          <span className="flex items-center gap-2">
            <span className="text-[13px] font-semibold text-ink">子代理角色提示词 (System Prompt)</span>
            <span className="font-mono text-[11px] text-ink-4">{chars} 字符</span>
          </span>
          <span className="flex items-center gap-2 text-xs font-medium text-ink-2">
            整份替换系统提示
            <Switch size="sm" on={agent.replacePrompt} onChange={(v) => onChange({ replacePrompt: v })} label="整份替换系统提示" />
          </span>
        </div>
        <textarea
          value={agent.persona}
          onChange={(e) => onChange({ persona: e.target.value })}
          spellCheck={false}
          placeholder="你是一个专注于……的子代理。"
          className="thin-scroll h-[128px] resize-none rounded-lg border border-[#d1d1d6] bg-[#fafafc] p-3 font-mono text-xs leading-relaxed text-ink outline-none select-text placeholder:text-ink-5 focus:border-accent"
        />
        {errors?.persona && <span className="enter-fade text-[11.5px] text-[#ff3b30]">⊗ {errors.persona}</span>}
      </div>

      <div className="grid grid-cols-2 gap-4">
        <div className="flex flex-col gap-2.5 rounded-xl border border-line-strong bg-white p-4 shadow-[0px_1px_0px_rgba(0,0,0,0.05)]">
          <div className="flex items-center justify-between">
            <span className="text-[13px] font-semibold text-ink">
              授权给子代理的工具{agent.tools && agent.tools.length > 0 ? ` (${agent.tools.length})` : ""}
            </span>
            <button type="button" onClick={onPickTools} className="text-[11.5px] text-accent hover:underline">
              自选勾选
            </button>
          </div>
          {agent.tools === null || agent.tools.length === 0 ? (
            <span className="pt-1 text-xs text-ink-4">
              {agent.tools === null ? "全部已注册工具（以后新挂的也算）" : "不用工具：它只能对话"}
            </span>
          ) : (
            <div className="flex flex-wrap gap-1.5 pt-1">
              {agent.tools.map((t) => (
                <span key={t} className="rounded-md border border-line-strong bg-[#f2f2f7] px-2 py-0.5 font-mono text-[11px] text-ink">
                  {t}
                </span>
              ))}
            </div>
          )}
        </div>
        <div className="flex flex-col justify-between gap-2 rounded-xl border border-line-strong bg-white p-4 shadow-[0px_1px_0px_rgba(0,0,0,0.05)]">
          <div className="flex items-center justify-between gap-3">
            <span className="flex flex-col">
              <span className="text-[13px] font-semibold text-ink">带技能 / 工作流清单</span>
              <span className="mt-0.5 text-[11.5px] text-ink-4">让子代理的系统提示带上 Skills 与工作流目录</span>
            </span>
            <Switch color="green" on={agent.listings} onChange={(v) => onChange({ listings: v })} label="带技能 / 工作流清单" />
          </div>
          {agent.listings ? (
            <span className="enter-fade flex items-center gap-1 rounded-md border border-[#bbf7d0] bg-[#f0fdf4] px-2.5 py-1 text-[11px] text-[#15803d]">
              <Check size={11} /> 已开启：还要它的工具里有 skill / workflow，清单才会发
            </span>
          ) : (
            <span className="text-[11px] text-ink-4">关着省上下文：窄职责的子代理很少用得上技能。</span>
          )}
        </div>
      </div>
    </div>
  );
}
