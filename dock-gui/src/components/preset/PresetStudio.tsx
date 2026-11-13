// 预设编辑器：同 TUI `/preset` 画布，改人设、工具名单、子代理名册。盖在窗口最上层。
// 设计：wonder-ui/preset/PresetEditor*（emil 基本信息 / moyo 提示词 / tway 工具 / monk 名册 /
// oban 子代理 / paty 内置 / slur 未保存 / rone 校验 / sero 损坏 / tamp 搜索无结果）。
// 读写经 Dock `preset/get` / `preset/update` / `tool/catalog`；保存整份写回。

import { useCallback, useEffect, useMemo, useState } from "react";
import { openPath } from "@tauri-apps/plugin-opener";
import {
  ArrowLeft,
  CircleAlert,
  Sparkles,
  ExternalLink,
  FileText,
  FileWarning,
  Folder,
  GitFork,
  Info,
  LoaderCircle,
  RotateCcw,
  TriangleAlert,
  Wrench,
  type LucideIcon,
} from "lucide-react";
import type { AiPresetDraft, CatalogTool, PresetDetail, PresetDraft, SubagentDraft } from "../../model";
import { store } from "../../dock/store";
import { TrafficLights } from "../chrome";
import { ICON_LIBRARY } from "../presetIcon";
import { DraftBanner, type Suggestions } from "./ai";
import { AskPanel } from "./AskPanel";
import { AgentEditor, AgentsSection, BasicSection, PromptSection, ToolsSection, type Errors } from "./sections";
import { ToolModeSwitch, ToolPicker } from "./ToolPicker";
import { charDelta, copyId, toolCountLabel } from "./util";

type Section = "basic" | "prompt" | "tools" | "agents";

const NAV: { id: Section; label: string; icon: LucideIcon }[] = [
  { id: "basic", label: "基本信息", icon: Info },
  { id: "prompt", label: "角色提示词", icon: FileText },
  { id: "tools", label: "工具配置", icon: Wrench },
  { id: "agents", label: "子代理名册", icon: GitFork },
];

const ROLE_ID = /^[a-z0-9][a-z0-9-]{0,31}$|^[㐀-鿿]{1,16}$/;

function toDraft(d: PresetDetail): PresetDraft {
  return {
    name: d.name,
    description: d.description,
    icon: d.icon,
    order: d.order,
    persona: d.persona,
    replacePrompt: d.replacePrompt,
    tools: d.tools,
    agents: d.agents.map((a) => ({ ...a })),
  };
}

function validate(d: PresetDraft): Errors {
  const e: Errors = { agents: {} };
  if (!d.name.trim()) e.name = "预设名称不能为空";
  if (d.replacePrompt && !d.persona.trim()) e.persona = "整份替换系统提示时，角色提示词不能为空";
  d.agents.forEach((a, i) => {
    const bad: { id?: string; persona?: string } = {};
    if (!a.id) bad.id = "子代理 id 不能为空";
    else if (!ROLE_ID.test(a.id)) bad.id = "id 只能是小写字母、数字、-（字母或数字开头），或 1–16 个汉字";
    else if (d.agents.some((b, j) => j !== i && b.id === a.id)) bad.id = `id 重复：${a.id}`;
    if (a.replacePrompt && !a.persona.trim()) bad.persona = "整份替换系统提示时，提示词不能为空";
    if (bad.id || bad.persona) e.agents[i] = bad;
  });
  return e;
}

function errorCount(e: Errors) {
  return (e.name ? 1 : 0) + (e.persona ? 1 : 0) + Object.values(e.agents).reduce((n, a) => n + (a.id ? 1 : 0) + (a.persona ? 1 : 0), 0);
}

/** 未保存确认里的「改了什么」。 */
function changes(a: PresetDraft, b: PresetDraft): string[] {
  const out: string[] = [];
  if (a.name !== b.name || a.description !== b.description || a.icon !== b.icon || a.order !== b.order) out.push("基本信息");
  if (a.persona !== b.persona) out.push(`角色提示词 (${charDelta(a.persona, b.persona)})`);
  else if (a.replacePrompt !== b.replacePrompt) out.push("替换模式");
  if (JSON.stringify(a.tools) !== JSON.stringify(b.tools)) out.push("已选工具变更");
  if (JSON.stringify(a.agents) !== JSON.stringify(b.agents)) out.push("子代理名册变更");
  return out;
}

/** AI 起草的草稿 → 编辑器的「还没 id 的预设」。 */
function fromAi(d: AiPresetDraft): PresetDetail {
  return {
    id: "",
    name: d.name,
    description: d.description,
    icon: d.icon,
    order: null,
    persona: d.persona,
    replacePrompt: d.replacePrompt,
    tools: d.tools,
    agents: d.agents.map((a) => ({ ...a, builtin: false, fresh: true })),
    origin: "user",
    builtin: false,
    available: true,
    error: null,
    path: null,
  };
}

const EMPTY: PresetDraft = {
  name: "",
  description: "",
  icon: null,
  order: null,
  persona: "",
  replacePrompt: false,
  tools: null,
  agents: [],
};

export function PresetStudio({
  presetId: initialId,
  aiDraft,
  onClose,
  onOpenMcp,
  onSaved,
}: {
  /** `null` = 还没落盘的 AI 草稿（`aiDraft`），保存时先 preset/create。 */
  presetId: string | null;
  aiDraft?: { draft: AiPresetDraft; description: string } | null;
  onClose: () => void;
  /** 「管理外部 MCP 服务」：关掉编辑器、打开设置的 MCP 页。 */
  onOpenMcp: () => void;
  /** AI 草稿第一次保存成功，回新预设的 id。 */
  onSaved?: (id: string) => void;
}) {
  const [presetId, setPresetId] = useState(initialId);
  // AI 草稿：用户的原描述（重新生成 / 润色的背景）和挑工具的理由；保存后清掉。
  const [ai, setAi] = useState(aiDraft ?? null);
  const [regenerating, setRegenerating] = useState(false);
  const [suggest, setSuggest] = useState<Suggestions | null>(null);
  const [detail, setDetail] = useState<PresetDetail | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [catalog, setCatalog] = useState<CatalogTool[] | null>(null);
  const [original, setOriginal] = useState<PresetDraft | null>(null);
  const [draft, setDraft] = useState<PresetDraft | null>(null);
  const [section, setSection] = useState<Section>("basic");
  const [editing, setEditing] = useState<number | null>(null);
  const [picking, setPicking] = useState(false);
  // 子代理工具弹窗：名单空着时停在「自选」而不是「不用工具」（见 modeOf）。
  const [pickingEmpty, setPickingEmpty] = useState(false);
  const [attempted, setAttempted] = useState(false);
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);
  const [leaving, setLeaving] = useState<null | (() => void)>(null);
  // 旁边的「问问 AI」：只回参考答案；关掉就清空。
  const [asking, setAsking] = useState(false);

  const load = useCallback(async () => {
    if (!presetId) {
      // AI 草稿：什么都还没存，整份都算改动。
      if (!aiDraft) throw new Error("没有预设");
      const d = fromAi(aiDraft.draft);
      setDetail(d);
      setOriginal(EMPTY);
      setDraft(toDraft(d));
      return;
    }
    const d = await store.getPreset(presetId);
    setDetail(d);
    setOriginal(toDraft(d));
    setDraft(toDraft(d));
    // 只在第一次读的时候定下；aiDraft 是初始参数。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [presetId]);

  useEffect(() => {
    load().catch((e) => setLoadError(e instanceof Error ? e.message : String(e)));
    store
      .toolCatalog()
      .then(setCatalog)
      .catch(() => setCatalog([]));
  }, [load]);

  const dirty = !!draft && !!original && JSON.stringify(draft) !== JSON.stringify(original);
  const errors = useMemo(() => (draft ? validate(draft) : null), [draft]);
  const shown = attempted ? errors : null;
  const bad = errors ? errorCount(errors) : 0;

  const patch = (p: Partial<PresetDraft>) => {
    setSaved(false);
    setDraft((d) => (d ? { ...d, ...p } : d));
  };
  const patchAgent = (i: number, p: Partial<SubagentDraft>) =>
    patch({ agents: draft!.agents.map((a, j) => (j === i ? { ...a, ...p } : a)) });

  const save = async (): Promise<boolean> => {
    if (!draft || !detail) return false;
    setAttempted(true);
    setSaveError(null);
    const e = validate(draft);
    if (errorCount(e)) {
      // 跳到第一个出错的分区。
      if (e.name) setSection("basic");
      else if (e.persona) setSection("prompt");
      else {
        setSection("agents");
        setEditing(null);
      }
      return false;
    }
    setSaving(true);
    try {
      let id = presetId;
      if (!id) {
        // AI 草稿第一次保存：先建一个空的用户层预设，再整份写进去。
        id = await store.createPreset(draft.name.trim(), draft.icon ?? "rocket", null);
      }
      await store.updatePreset(id, draft);
      if (id !== presetId) {
        setPresetId(id);
        setAi(null);
        onSaved?.(id);
      } else {
        await load();
      }
      setAttempted(false);
      setSaved(true);
      return true;
    } catch (err) {
      setSaveError(err instanceof Error ? err.message.replace(/^preset\/update: /, "") : String(err));
      return false;
    } finally {
      setSaving(false);
    }
  };

  /** AI 草稿「重新生成」：同一段描述再起草一份，整份换掉。 */
  const regenerate = async () => {
    if (!ai) return;
    setRegenerating(true);
    setSaveError(null);
    try {
      const next = await store.draftPreset(ai.description, ICON_LIBRARY.map((i) => i.name));
      setAi({ draft: next, description: ai.description });
      setDraft(toDraft(fromAi(next)));
      setEditing(null);
    } catch (e) {
      setSaveError(e instanceof Error ? e.message.replace(/^preset\/draft: /, "") : String(e));
    } finally {
      setRegenerating(false);
    }
  };

  /** 工具分区「按描述推荐」。 */
  const askSuggest = async () => {
    if (!draft) return;
    setSuggest({ loading: true, error: null, picks: [], already: 0 });
    try {
      const picks = await store.suggestTools(ai?.description || draft.description || draft.name, draft.persona);
      const have = draft.tools ?? [];
      setSuggest({
        loading: false,
        error: null,
        picks: picks.filter((p) => !have.includes(p.name)),
        already: picks.filter((p) => have.includes(p.name)).length,
      });
    } catch (e) {
      setSuggest({
        loading: false,
        error: e instanceof Error ? e.message.replace(/^preset\/suggestTools: /, "") : String(e),
        picks: [],
        already: 0,
      });
    }
  };
  const acceptSuggest = (names: string[]) => {
    patch({ tools: [...(draft!.tools ?? []), ...names.filter((n) => !(draft!.tools ?? []).includes(n))] });
    setSuggest((s) => {
      if (!s) return s;
      const picks = s.picks.filter((p) => !names.includes(p.name));
      return picks.length ? { ...s, picks, already: s.already + names.length } : null;
    });
  };
  const dismissSuggest = (name: string) =>
    setSuggest((s) => {
      if (!s) return s;
      const picks = s.picks.filter((p) => p.name !== name);
      return picks.length ? { ...s, picks } : null;
    });

  // 离开前有改动就先问（设计 slur）。
  const guard = useCallback(
    (then: () => void) => {
      if (dirty) setLeaving(() => then);
      else then();
    },
    [dirty],
  );

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || e.defaultPrevented) return;
      e.preventDefault();
      if (leaving) setLeaving(null);
      else if (picking) setPicking(false);
      else if (editing !== null) setEditing(null);
      else guard(onClose);
    };
    // 捕获阶段：先于底下设置页的 Esc（它会关掉整个设置页）。
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [leaving, picking, editing, guard, onClose]);

  const addAgent = () => {
    const agents = [
      ...draft!.agents,
      { id: "", name: "", description: "", persona: "", tools: null, replacePrompt: false, listings: false, keepAlive: false, builtin: false, fresh: true },
    ];
    patch({ agents });
    setEditing(agents.length - 1);
  };
  const copyAgent = (i: number) => {
    const a = draft!.agents[i];
    const id = copyId(a.id, draft!.agents.map((x) => x.id));
    patch({ agents: [...draft!.agents, { ...a, id, name: a.name ? `${a.name} 副本` : "", builtin: false, fresh: true }] });
  };
  const removeAgent = (i: number) => patch({ agents: draft!.agents.filter((_, j) => j !== i) });

  const originTag = !detail
    ? null
    : detail.builtin
      ? detail.origin === "shipped"
        ? "系统内置"
        : "内置 · 已修改"
      : detail.origin === "project"
        ? "当前项目"
        : "用户全局";

  // ---- 损坏的预设（设计 sero） ----
  if (detail && !detail.available) {
    return (
      <Frame>
        <Header
          crumbs={["预设管理", "预设配置文件加载异常"]}
          tag={<span className="rounded-full border border-[#ffd0cc] bg-[#fff1f0] px-2 py-0.5 text-[11px] font-medium text-[#ff3b30]">文件损坏</span>}
          actions={
            <button type="button" onClick={onClose} className={lightButton}>
              返回预设管理
            </button>
          }
        />
        <div className="flex min-h-0 flex-1">
          <Nav section={null} onSection={() => {}} disabled />
          <Broken detail={detail} onClose={onClose} onRebuilt={load} />
        </div>
      </Frame>
    );
  }

  const agent = editing !== null ? draft?.agents[editing] : undefined;
  const name = (draft?.name || detail?.name) ?? presetId ?? "新预设";
  const reasons = ai ? Object.fromEntries(ai.draft.toolReasons.map((r) => [r.name, r.reason])) : undefined;
  const aiSummary = draft
    ? [
        `写了 ${[...draft.persona].length} 字提示词`,
        draft.tools === null ? "工具用全部" : `挑了 ${draft.tools.length} 个工具`,
        draft.agents.length ? `配了 ${draft.agents.length} 个子代理` : null,
      ]
        .filter(Boolean)
        .join("、") + "。"
    : "";

  const askToggle = draft && (
    <button
      type="button"
      onClick={() => setAsking((v) => !v)}
      aria-pressed={asking}
      title="边配边问：AI 看得到当前草稿，只给参考，不会改你的配置"
      className={
        asking
          ? "motion-colors flex h-[30px] items-center gap-1.5 rounded-lg border border-[#0a84ff4d] bg-[#0a84ff1a] px-3 text-[13px] font-medium text-accent"
          : lightButton
      }
    >
      <Sparkles size={14} className={asking ? "" : "text-accent"} />问 AI
    </button>
  );

  return (
    <Frame>
      {agent ? (
        <Header
          crumbs={[name, "子代理", `编辑子代理：${agent.id || "新子代理"}`]}
          actions={
            <>
              {askToggle}
              <button type="button" onClick={() => setEditing(null)} className={lightButton}>
                <ArrowLeft size={13} /> 返回名册
              </button>
              <button
                type="button"
                disabled={saving}
                onClick={async () => (await save()) && setEditing(null)}
                className={primaryButton}
              >
                {saving && <LoaderCircle size={13} className="animate-spin" />}
                完成并保存
              </button>
            </>
          }
        />
      ) : (
        <Header
          crumbs={["预设管理", presetId ? `编辑预设：${name} (${presetId})` : `新预设：${name}`]}
          tag={
            ai ? (
              <span className="ml-1 flex items-center gap-1 rounded-full border border-[#e4d9ff] bg-[#f5f3ff] px-2 py-0.5 text-[11px] font-medium text-[#7c3aed]">
                <Sparkles size={10} /> AI 起草
              </span>
            ) : (
              originTag && (
                <span className="ml-1 rounded-full border border-[#d0e4ff] bg-[#eef5ff] px-2 py-0.5 text-[11px] font-medium text-accent">
                  {originTag}
                </span>
              )
            )
          }
          actions={
            <>
              {dirty ? (
                <span className="enter-fade mr-1 flex items-center gap-1.5 text-xs font-medium text-[#ff9500]">
                  <span className="size-2 rounded-full bg-[#ff9500]" />
                  有未保存改动
                </span>
              ) : (
                saved && <span className="enter-fade mr-1 text-xs font-medium text-[#34c759]">已保存</span>
              )}
              {askToggle}
              <button type="button" onClick={() => guard(onClose)} className={lightButton}>
                取消
              </button>
              <button
                type="button"
                disabled={!draft || saving || (attempted && bad > 0)}
                onClick={save}
                className={primaryButton}
              >
                {saving && <LoaderCircle size={13} className="animate-spin" />}
                {detail?.builtin ? "保存全局覆盖" : presetId ? "保存修改" : "保存预设"}
              </button>
            </>
          }
        />
      )}

      {detail?.builtin && !agent && (
        <div className="enter-fade flex h-11 shrink-0 items-center justify-between gap-4 border-b border-[#fef3c7] bg-[#fffbeb] px-6 text-[#92400e]">
          <span className="flex min-w-0 items-center gap-2 text-[12.5px] font-medium">
            <TriangleAlert size={15} className="shrink-0 text-[#d97706]" />
            <span className="truncate">
              系统内置预设：保存后在全局目录 (~/.dock/presets/{detail.id}/) 写一份覆盖，不影响安装包，可随时恢复内置。
            </span>
          </span>
          {detail.origin !== "shipped" && <RestoreBuiltin id={detail.id} onDone={onClose} />}
        </div>
      )}

      <div className="flex min-h-0 flex-1">
        {!agent && (
          <Nav
            section={section}
            onSection={setSection}
            counts={draft ? { tools: toolCountShort(draft.tools), agents: String(draft.agents.length) } : undefined}
            badges={{
              ...(ai && draft
                ? {
                    basic: "AI 生成",
                    prompt: "AI 生成",
                    tools: draft.tools ? `AI (${draft.tools.length})` : "AI",
                    ...(draft.agents.length ? { agents: `AI (${draft.agents.length})` } : {}),
                  }
                : {}),
              ...(suggest?.picks.length ? { tools: `${suggest.picks.length} 待确认` } : {}),
            }}
            errors={
              shown
                ? {
                    basic: shown.name ? 1 : 0,
                    prompt: shown.persona ? 1 : 0,
                    agents: Object.keys(shown.agents).length,
                  }
                : undefined
            }
            tip={
              presetId
                ? `在终端 TUI 里用 /preset 也能找到并编辑「${name}」。`
                : "AI 草稿还没保存：检查各分区后点右上角「保存预设」。"
            }
          />
        )}
        <main
          className={`thin-scroll flex min-h-0 min-w-0 flex-1 flex-col gap-6 overflow-y-auto bg-white ${
            agent ? "px-12 py-8" : section === "tools" ? "px-8 py-6" : "px-12 py-8"
          }`}
        >
          {ai && draft && !agent && (
            <DraftBanner summary={aiSummary} regenerating={regenerating} onRegenerate={regenerate} />
          )}
          {(loadError || saveError || (shown && bad > 0)) && (
            <div className="enter-fade flex shrink-0 items-center justify-between gap-3 rounded-xl border border-[#ffd0cc] bg-[#fff5f5] px-4 py-3">
              <span className="flex items-center gap-2.5 text-[13px] font-semibold text-[#d70015]">
                <CircleAlert size={18} className="shrink-0 text-[#ff3b30]" />
                {loadError
                  ? `读不到这个预设：${loadError}`
                  : saveError
                    ? `保存失败：${saveError}`
                    : `表单存在 ${bad} 项配置错误，请修正后再保存`}
              </span>
              {!loadError && !saveError && <span className="shrink-0 text-xs text-ink-4">标红的输入框改好就能保存</span>}
            </div>
          )}
          {!draft || !detail ? (
            !loadError && (
              <span className="flex items-center gap-2 text-[12.5px] text-ink-4">
                <LoaderCircle size={14} className="animate-spin" /> 正在读取预设…
              </span>
            )
          ) : agent && editing !== null ? (
            <AgentEditor
              key={editing}
              agent={agent}
              onChange={(p) => patchAgent(editing, p)}
              errors={shown?.agents[editing]}
              onPickTools={() => setPicking(true)}
            />
          ) : section === "basic" ? (
            <BasicSection detail={detail} draft={draft} patch={patch} errors={shown} />
          ) : section === "prompt" ? (
            <PromptSection draft={draft} patch={patch} errors={shown} context={ai?.description || draft.description} />
          ) : section === "tools" ? (
            <ToolsSection
              draft={draft}
              patch={patch}
              catalog={catalog}
              onOpenMcp={() => guard(onOpenMcp)}
              reasons={reasons}
              suggest={suggest}
              onSuggest={askSuggest}
              onSuggestAccept={acceptSuggest}
              onSuggestDismiss={dismissSuggest}
              onSuggestClose={() => setSuggest(null)}
            />
          ) : (
            <AgentsSection
              draft={draft}
              errors={shown}
              onAdd={addAgent}
              onEdit={setEditing}
              onCopy={copyAgent}
              onRemove={removeAgent}
            />
          )}
        </main>
        {asking && draft && (
          <AskPanel
            step={agent ? "agents" : section}
            draft={draft}
            agentId={agent && editing !== null ? agent.id : null}
            onClose={() => setAsking(false)}
          />
        )}
      </div>

      {picking && agent && editing !== null && (
        <div className="enter-fade absolute inset-0 z-[60] flex items-center justify-center bg-[#00000040] backdrop-blur-[3px]">
          <div className="glass-dialog pop-in flex h-[640px] max-h-[88%] w-[960px] max-w-[92%] flex-col gap-4 rounded-2xl border border-[#00000015] bg-white p-5 shadow-[0px_24px_64px_rgba(0,0,0,0.24)]">
            <div className="flex shrink-0 items-center justify-between gap-4">
              <span className="flex flex-col gap-0.5">
                <span className="text-[15px] font-semibold text-ink">授权给子代理「{agent.id || "新子代理"}」的工具</span>
                <span className="text-xs text-ink-4">{toolCountLabel(agent.tools)}</span>
              </span>
              <span className="flex items-center gap-2">
                <ToolModeSwitch
                  value={agent.tools}
                  onChange={(tools) => patchAgent(editing, { tools })}
                  catalog={catalog}
                  picking={pickingEmpty}
                  onPicking={setPickingEmpty}
                />
                <button type="button" onClick={() => setPicking(false)} className={primaryButton}>
                  完成
                </button>
              </span>
            </div>
            <ToolPicker
              value={agent.tools}
              onChange={(tools) => patchAgent(editing, { tools })}
              catalog={catalog}
              onOpenMcp={() => guard(onOpenMcp)}
              modeSwitch={false}
              picking={pickingEmpty}
              onPicking={setPickingEmpty}
              compact
            />
          </div>
        </div>
      )}

      {leaving && original && draft && (
        <div className="enter-fade absolute inset-0 z-[70] flex items-center justify-center bg-[#00000040] backdrop-blur-[3px]">
          <div
            role="alertdialog"
            aria-label="放弃未保存的更改？"
            className="glass-dialog pop-in flex w-[460px] flex-col rounded-2xl border border-[#00000015] bg-white p-6 text-center shadow-[0px_24px_64px_rgba(0,0,0,0.24)]"
          >
            <span className="mx-auto mb-3.5 flex size-12 items-center justify-center rounded-full bg-[#ff950018] text-[#ff9500]">
              <TriangleAlert size={24} />
            </span>
            <span className="text-[17px] font-semibold tracking-[-0.2px] text-ink">放弃未保存的更改？</span>
            <span className="mt-2 px-2 text-[13px] leading-relaxed text-ink-3">
              当前预设「{name}」有尚未保存的修改。如果直接离开，修改的内容将会丢失，无法恢复。
            </span>
            <div className="mt-[18px] flex items-center justify-between gap-3 rounded-xl border border-line-strong bg-[#f5f5f7] px-3.5 py-2.5 text-left">
              <span className="flex shrink-0 items-center gap-2 text-xs font-medium text-ink">
                <span className="size-2 rounded-full bg-[#ff9500]" />
                未保存修改项
              </span>
              <span className="truncate text-xs text-ink-4">{changes(original, draft).join(", ")}</span>
            </div>
            <div className="mt-6 flex items-center justify-between pt-1">
              <button
                type="button"
                onClick={() => {
                  const then = leaving;
                  setLeaving(null);
                  then();
                }}
                className="motion-colors h-[34px] rounded-lg border border-[#ff3b3030] px-3.5 text-[13px] font-medium text-[#ff3b30] hover:bg-[#ff3b300d]"
              >
                放弃修改
              </button>
              <span className="flex items-center gap-2">
                <button
                  type="button"
                  onClick={() => setLeaving(null)}
                  className="motion-colors h-[34px] rounded-lg border border-line-strong bg-[#f5f5f7] px-3.5 text-[13px] font-medium text-ink shadow-sm hover:bg-hover"
                >
                  继续编辑
                </button>
                <button
                  type="button"
                  disabled={saving}
                  onClick={async () => {
                    const then = leaving;
                    setLeaving(null);
                    if (await save()) then();
                  }}
                  className={`${primaryButton} h-[34px] px-4`}
                >
                  保存并离开
                </button>
              </span>
            </div>
          </div>
        </div>
      )}
    </Frame>
  );
}

const lightButton =
  "motion-colors flex h-[30px] items-center gap-1 rounded-lg border border-line-strong bg-white px-3 text-[13px] font-medium text-ink shadow hover:bg-hover";
const primaryButton =
  "motion-colors flex h-[30px] items-center gap-1.5 rounded-lg bg-accent px-3.5 text-[13px] font-medium text-white shadow hover:bg-[#0077ed] active:scale-[0.98] disabled:bg-line-strong disabled:text-ink-4 disabled:shadow-none";

function toolCountShort(tools: string[] | null) {
  return tools === null ? "全部" : String(tools.length);
}

function Frame({ children }: { children: React.ReactNode }) {
  return <div className="glass-pane enter-fade relative flex h-full w-full flex-col overflow-hidden bg-white text-ink">{children}</div>;
}

function Header({ crumbs, tag, actions }: { crumbs: string[]; tag?: React.ReactNode; actions: React.ReactNode }) {
  return (
    <div className="flex h-[52px] shrink-0 items-center justify-between border-b border-line bg-[#fbfbfb] px-4" data-tauri-drag-region>
      <div className="flex min-w-0 items-center gap-3.5" data-tauri-drag-region>
        <TrafficLights />
        <span className="h-4 w-px shrink-0 bg-line-strong" />
        <div className="flex min-w-0 items-center gap-2" data-tauri-drag-region>
          {crumbs.map((c, i) => (
            <span key={i} className="flex min-w-0 items-center gap-2">
              {i > 0 && <span className="text-xs text-ink-4">/</span>}
              <span className={`truncate text-[13px] ${i === crumbs.length - 1 ? "font-semibold text-ink" : "text-ink-4"}`}>{c}</span>
            </span>
          ))}
          {tag}
        </div>
      </div>
      <div className="flex shrink-0 items-center gap-2.5">{actions}</div>
    </div>
  );
}

function Nav({
  section,
  onSection,
  counts,
  errors,
  badges,
  tip,
  disabled = false,
}: {
  section: Section | null;
  onSection: (s: Section) => void;
  counts?: { tools: string; agents: string };
  errors?: Partial<Record<Section, number>>;
  /** 小标签（「AI 生成」「3 待确认」），盖过计数。 */
  badges?: Partial<Record<Section, string>>;
  tip?: string;
  disabled?: boolean;
}) {
  return (
    <nav className="flex w-[230px] shrink-0 flex-col gap-1 border-r border-line bg-sidebar p-3">
      <span className="px-2 py-1 text-[11px] font-semibold tracking-[0.25px] text-ink-4">{disabled ? "预设分区 (已禁用)" : "预设分区"}</span>
      {NAV.map(({ id, label, icon: Icon }) => {
        const on = id === section;
        const err = errors?.[id] ?? 0;
        const count = id === "tools" ? counts?.tools : id === "agents" ? counts?.agents : undefined;
        return (
          <button
            key={id}
            type="button"
            disabled={disabled}
            onClick={() => onSection(id)}
            className={`motion-colors flex h-[34px] items-center gap-2.5 rounded-lg px-2.5 text-left text-[13px] disabled:opacity-60 ${
              on ? "bg-line-strong font-semibold text-accent shadow" : "font-medium text-ink-2 enabled:hover:bg-hover"
            }`}
          >
            <Icon size={16} className={on ? "text-accent" : "text-ink-3"} />
            <span className="flex-1">{label}</span>
            {err > 0 ? (
              <span className="pop-in rounded-full bg-[#ff3b30] px-1.5 py-0.5 text-[11px] font-semibold leading-none text-white">{err}</span>
            ) : badges?.[id] ? (
              <span
                key={badges[id]}
                className={`pop-in rounded-md px-1.5 py-0.5 text-[10.5px] font-medium leading-none ${
                  badges[id]!.includes("待确认") ? "bg-accent text-white" : "bg-[#f5f3ff] text-[#7c3aed]"
                }`}
              >
                {badges[id]}
              </span>
            ) : (
              count !== undefined && (
                <span
                  className={`rounded-full px-1.5 py-0.5 text-[11px] font-medium leading-none ${
                    on ? "bg-accent text-white" : "bg-line-strong text-ink-3"
                  }`}
                >
                  {count}
                </span>
              )
            )}
          </button>
        );
      })}
      {tip && (
        <div className="mt-auto flex flex-col gap-1.5 border-t border-line px-1 pt-3">
          <span className="text-[11.5px] font-medium text-ink-4">快捷提示</span>
          <span className="text-[11.5px] leading-relaxed text-ink-4">{tip}</span>
        </div>
      )}
    </nav>
  );
}

function RestoreBuiltin({ id, onDone }: { id: string; onDone: () => void }) {
  const [confirm, setConfirm] = useState(false);
  const [busy, setBusy] = useState(false);
  const restore = async () => {
    setBusy(true);
    try {
      await store.deletePreset(id);
      onDone();
    } finally {
      setBusy(false);
    }
  };
  return confirm ? (
    <span className="enter-fade flex shrink-0 items-center gap-2 text-xs">
      丢掉你对它的全部修改？
      <button type="button" onClick={() => setConfirm(false)} className="rounded-md px-2 py-1 font-medium hover:bg-[#fef3c7]">
        取消
      </button>
      <button
        type="button"
        disabled={busy}
        onClick={restore}
        className="h-7 rounded-md bg-[#d97706] px-2.5 font-semibold text-white hover:bg-[#b45309]"
      >
        恢复内置
      </button>
    </span>
  ) : (
    <button
      type="button"
      onClick={() => setConfirm(true)}
      className="motion-colors h-7 shrink-0 rounded-md border border-[#fcd34d] bg-white px-2.5 text-xs font-semibold text-[#b45309] shadow-[0px_1px_0px_rgba(0,0,0,0.05)] hover:bg-[#fffbeb]"
    >
      恢复内置默认
    </button>
  );
}

function Broken({ detail, onClose, onRebuilt }: { detail: PresetDetail; onClose: () => void; onRebuilt: () => Promise<void> }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const rebuild = async () => {
    setBusy(true);
    setError(null);
    try {
      // 用默认模板重建：只留名字（= id），其余空；工具不限。
      await store.updatePreset(detail.id, {
        name: detail.id,
        description: "",
        icon: null,
        order: null,
        persona: "",
        replacePrompt: false,
        tools: null,
        agents: [],
      });
      await onRebuilt();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <main className="flex min-w-0 flex-1 items-center justify-center bg-window p-8">
      <div className="glass-dialog pop-in flex w-[480px] max-w-full flex-col items-center gap-3 rounded-2xl border border-line-strong bg-white p-7 text-center shadow-[0px_8px_28px_rgba(0,0,0,0.06)]">
        <span className="flex size-12 items-center justify-center rounded-2xl bg-[#fff1f0] text-[#ff3b30]">
          <FileWarning size={22} />
        </span>
        <span className="text-[17px] font-semibold text-ink">预设配置文件解析失败</span>
        {detail.path && (
          <span className="flex max-w-full items-center gap-1.5 rounded-md border border-line-strong bg-[#f5f5f7] px-2.5 py-1 font-mono text-[11px] text-ink-3">
            <Folder size={12} className="shrink-0" />
            <span className="truncate">{detail.path.replace(/^\/Users\/[^/]+/, "~")}</span>
          </span>
        )}
        <span className="text-[13px] leading-relaxed text-ink-3">
          Dock 读这个预设的 YAML 时出错，没法显示图形化表单。可以在外部编辑器里修好语法，或者用默认模板重建（原内容会被覆盖）。
        </span>
        <pre className="w-full overflow-x-auto whitespace-pre-wrap rounded-xl bg-[#1c1c1e] p-4 text-left font-mono text-[11.5px] leading-relaxed text-[#ffd60a] select-text">
          <span className="mb-2 block border-b border-[#ffffff1a] pb-2 text-[10.5px] text-[#8e8e93]">YAML 解析错误</span>
          {detail.error}
        </pre>
        {error && <span className="text-xs text-[#ff3b30]">{error}</span>}
        <div className="mt-2 flex items-center gap-2.5">
          <button type="button" disabled={busy} onClick={rebuild} className={`${primaryButton} h-8`}>
            {busy ? <LoaderCircle size={13} className="animate-spin" /> : <RotateCcw size={13} />}
            用默认模板重建
          </button>
          {detail.path && (
            <button type="button" onClick={() => openPath(detail.path!).catch(() => {})} className={`${lightButton} h-8`}>
              <ExternalLink size={13} /> 在外部编辑器打开
            </button>
          )}
          <button type="button" onClick={onClose} className="px-2 text-[13px] text-ink-4 hover:text-ink">
            返回
          </button>
        </div>
      </div>
    </main>
  );
}
