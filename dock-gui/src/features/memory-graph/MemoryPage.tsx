// 侧栏「记忆」：跨会话记忆的关系图与管理。
//
// 笔记是点，正文里的 `[[文件名]]` 是有方向的连线；内容相近但还没链上的是相似建议，
// 默认只在悬停 / 选中那篇时画成虚线。显示、力导向、按条件着色在画布右上角的设置里。
// 默认看「当前项目 + 全局」（一个会话里 agent 看得到的那些），可以切到全部项目。
// 图和列表是同一批笔记的两种看法；点一篇在右侧抽屉里读、改、建链、遗忘。
// agent 写了新记忆，Dock 推 `memory/changed`，图上就地多出一个点。

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { BrainCircuit, ChevronDown, CircleOff, Plus, RefreshCw, Search, X } from "lucide-react";
import { EmptyCard, NavPageShell } from "../../components/navPage";
import { MenuButton } from "../../components/Popover";
import { Banner, Modal, primaryButton, secondaryButton, Segmented } from "../../components/settings/ui";
import { useApp } from "../../dock/gateways";
import { errorText } from "../../dock/settingsApi";
import type { NavPageProps } from "../registry";
import {
  linkNotes,
  memoryGraph,
  memorySearch,
  memorySupported,
  onMemoryChanged,
  unlinkNotes,
  type MemoryGraph,
  type MemoryScope,
  type ScopeMode,
} from "./api";
import { EdgeCard, NewNoteDialog } from "./dialogs";
import { GraphCanvas, type GraphHandle, type LegendGroup } from "./GraphCanvas";
import { DRAWER_WIDTH, NoteDrawer } from "./NoteDrawer";
import { NoteList } from "./NoteList";
import { GROUP_SLOTS, useGraphSettings } from "./settings";
import { buildView, DEFAULT_FILTERS, type Filters, type ViewEdge, type ViewNode } from "./view";

const openMemorySettings = () => window.dispatchEvent(new CustomEvent("dock:open-settings", { detail: "memory" }));

/** 标题和文件名里有没有每个词（正文交给 Dock 搜）。 */
function titleMatches(graph: MemoryGraph, query: string): Set<string> {
  const terms = query.trim().toLowerCase().split(/\s+/).filter(Boolean);
  const ids = new Set<string>();
  if (terms.length === 0) return ids;
  for (const n of graph.notes) {
    const hay = `${n.title}\n${n.name}`.toLowerCase();
    if (terms.every((t) => hay.includes(t))) ids.add(n.id);
  }
  return ids;
}

export function MemoryPage({ insetLeft, cwd, projects }: NavPageProps) {
  const remote = !!useApp().remote;
  const [project, setProject] = useState<string | null>(cwd ?? projects[0]?.path ?? null);
  const [mode, setMode] = useState<ScopeMode>("current");
  const [graph, setGraph] = useState<MemoryGraph | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  /** 图每重拉一次加一：抽屉跟着重读。 */
  const [revision, setRevision] = useState(0);
  const [filters, setFilters] = useState<Filters>(DEFAULT_FILTERS);
  const settings = useGraphSettings();
  const [view, setView] = useState<"graph" | "list">("graph");
  const [query, setQuery] = useState("");
  const [contentHits, setContentHits] = useState<Map<string, string> | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const [creating, setCreating] = useState<{ title?: string; scope?: string } | null>(null);
  const [edgeCard, setEdgeCard] = useState<{ edge: ViewEdge; source: ViewNode; target: ViewNode; at: { x: number; y: number } } | null>(null);
  const [toast, setToast] = useState<{ text: string; key: number } | null>(null);
  const canvas = useRef<GraphHandle>(null);
  const supported = memorySupported();
  /** 抽屉里有没保存的修改。 */
  const dirty = useRef(false);
  const noteDirty = useCallback((value: boolean) => {
    dirty.current = value;
  }, []);
  /** 有没保存的修改时想做的事：先问一句，确认了再做。 */
  const [discarding, setDiscarding] = useState<{ run: () => void } | null>(null);
  const guarded = useCallback((run: () => void) => {
    if (dirty.current) setDiscarding({ run });
    else run();
  }, []);
  /** 刚改名 / 挪过位置的那篇：id 换了，图还没重拉到它。 */
  const awaited = useRef<string | null>(null);

  const request = useRef(0);
  const load = useCallback(
    (quiet = false) => {
      if (!supported) return Promise.resolve();
      const seq = (request.current += 1);
      if (!quiet) setRefreshing(true);
      return memoryGraph(project, mode)
        .then((g) => {
          // 期间又发了一次（切了项目 / 范围）：这一份过时了。
          if (seq !== request.current) return;
          setGraph(g);
          setError(null);
          setRevision((r) => r + 1);
        })
        .catch((e) => seq === request.current && setError(errorText(e)))
        .finally(() => seq === request.current && setRefreshing(false));
    },
    [project, mode, supported],
  );
  useEffect(() => {
    void load();
    // 笔记变了（界面、agent、外部编辑器）：悄悄重拉，图保持原样只补差异。
    return onMemoryChanged(() => void load(true));
  }, [load]);

  // 按内容搜：标题 / 文件名在本地立刻过滤，正文交给 Dock 找（稍等一下再发，别每敲一个字发一次）。
  useEffect(() => {
    const q = query.trim();
    if (!q) {
      setContentHits(null);
      return;
    }
    let live = true;
    const timer = window.setTimeout(() => {
      memorySearch(q, project, mode)
        .then((hits) => live && setContentHits(new Map(hits.map((h) => [h.id, h.snippet]))))
        .catch(() => live && setContentHits(new Map()));
    }, 180);
    return () => {
      live = false;
      window.clearTimeout(timer);
    };
  }, [query, project, mode, revision]);

  const matched = useMemo(() => {
    if (!query.trim() || !graph) return null;
    const ids = titleMatches(graph, query);
    for (const id of contentHits?.keys() ?? []) ids.add(id);
    return ids;
  }, [query, graph, contentHits]);

  // 按条件着色：每组和搜索框一样，标题 / 文件名本地比，正文交给 Dock；稍等一下再发，别每敲一个字发一次。
  const [groupContent, setGroupContent] = useState<(Set<string> | null)[]>([]);
  const groupQueries = settings.groups.map((g) => g.query.trim());
  const groupKey = groupQueries.join("\u0000");
  useEffect(() => {
    const queries = groupKey ? groupKey.split("\u0000") : [];
    let live = true;
    setGroupContent(queries.map(() => null));
    const timer = window.setTimeout(() => {
      void Promise.all(
        queries.map((q) =>
          q
            ? memorySearch(q, project, mode)
                .then((hits) => new Set(hits.map((h) => h.id)))
                .catch(() => new Set<string>())
            : Promise.resolve(new Set<string>()),
        ),
      ).then((sets) => live && setGroupContent(sets));
    }, 250);
    return () => {
      live = false;
      window.clearTimeout(timer);
    };
  }, [groupKey, project, mode, revision]);
  /** 每组命中的笔记；还在等正文结果的组是 null。 */
  const groupHits = useMemo(
    () =>
      settings.groups.map((g, i) => {
        const content = groupContent[i];
        if (!graph || !g.query.trim() || content === undefined || content === null) return null;
        const ids = titleMatches(graph, g.query);
        for (const id of content) ids.add(id);
        return ids;
      }),
    [settings.groups, groupContent, graph],
  );
  /** id → 颜色：排在前面的组优先。一组都没写条件时不着色（照常按范围上色）。 */
  const colors = useMemo(() => {
    if (!settings.groups.some((g) => g.query.trim())) return null;
    const map = new Map<string, string>();
    settings.groups.forEach((g, i) => {
      for (const id of groupHits[i] ?? []) if (!map.has(id)) map.set(id, GROUP_SLOTS[g.slot]);
    });
    return map;
  }, [settings.groups, groupHits]);
  const legendGroups: LegendGroup[] = useMemo(
    () => settings.groups.filter((g) => g.query.trim()).map((g) => ({ query: g.query.trim(), color: GROUP_SLOTS[g.slot] })),
    [settings.groups],
  );

  const shown = useMemo(() => (graph ? buildView(graph, filters, { ghosts: settings.ghosts }) : { nodes: [], edges: [] }), [graph, filters, settings.ghosts]);
  const shownIds = useMemo(() => new Set(shown.nodes.map((n) => n.id)), [shown.nodes]);
  const scopeLabels = useMemo(() => {
    const present = new Set(shown.nodes.map((n) => n.scope));
    const labels = new Map<string, string>();
    if (!graph || present.size < 3) return labels;
    for (const s of graph.scopes) if (present.has(s.id)) labels.set(s.id, s.kind === "global" ? "全局" : s.label);
    return labels;
  }, [graph, shown.nodes]);
  const listed = useMemo(() => {
    if (!graph) return [];
    return graph.notes.filter((n) => (n.kind === "topic" ? filters.topics : filters.observations)).filter((n) => !matched || matched.has(n.id));
  }, [graph, filters.topics, filters.observations, matched]);

  // 选中的那篇不在了（被遗忘、切了范围）：收起抽屉。刚改名的那篇要等图重拉到新 id，不算不在。
  useEffect(() => {
    if (!selected || !graph) return;
    if (graph.notes.some((n) => n.id === selected)) {
      if (awaited.current === selected) awaited.current = null;
    } else if (awaited.current !== selected) setSelected(null);
  }, [graph, selected]);

  const select = useCallback(
    (id: string | null, center = false) =>
      guarded(() => {
        setEdgeCard(null);
        setSelected(id);
        if (id && center) window.setTimeout(() => canvas.current?.focus(id), 0);
      }),
    [guarded],
  );

  const act = async (work: () => Promise<{ changed: boolean }>, done: string) => {
    setEdgeCard(null);
    try {
      const result = await work();
      setToast({ text: result.changed ? done : "已经是这样了", key: Date.now() });
      await load(true);
    } catch (e) {
      setError(errorText(e));
    }
  };
  const titleOf = (id: string) => graph?.notes.find((n) => n.id === id)?.title ?? id;
  const link = (from: string, to: string) => act(() => linkNotes(from, to), `已在「${titleOf(from)}」里链接到「${titleOf(to)}」`);
  const unlink = (from: string, to: string) => act(() => unlinkNotes(from, { to }), `已断开「${titleOf(from)}」到「${titleOf(to)}」的链接`);

  const total = graph?.scopes.reduce((sum, s) => sum + s.topics + s.observations, 0) ?? 0;
  const counts = useMemo(() => {
    const notes = graph?.notes ?? [];
    return {
      topics: notes.filter((n) => n.kind === "topic").length,
      observations: notes.filter((n) => n.kind === "observation").length,
      links: graph?.edges.filter((e) => e.kind === "link").length ?? 0,
      similar: graph?.edges.filter((e) => e.kind === "similar").length ?? 0,
    };
  }, [graph]);
  /** 新建时能选的范围：全局、当前项目，再加图上已经有的其他项目。 */
  const writableScopes: MemoryScope[] = useMemo(() => {
    if (!graph) return [];
    return graph.scopes.filter((s) => s.kind === "global" || s.current || s.selected);
  }, [graph]);
  const projectName = projects.find((p) => p.path === project)?.name;

  const actions = (
    <>
      {projects.length > 0 && (
        <MenuButton
          title="切换项目"
          side="bottom"
          align="right"
          value={project ?? ""}
          onPick={(path) =>
            guarded(() => {
              setProject(path);
              setSelected(null);
            })
          }
          options={projects.map((p) => ({ id: p.path, label: p.name, hint: p.path }))}
          trigger={
            <span className={`${secondaryButton} max-w-[200px]`}>
              <span className="truncate">{projectName ?? "选择项目"}</span>
              <ChevronDown size={13} className="shrink-0 text-ink-4" />
            </span>
          }
        />
      )}
      <Segmented
        value={mode}
        onChange={(m) =>
          guarded(() => {
            setMode(m);
            setSelected(null);
          })
        }
        options={[
          { value: "current", label: "当前项目", title: "当前项目和全局的记忆：一个会话里 agent 看得到的那些" },
          { value: "all", label: "全部项目", title: "这台 Dock 上所有项目的记忆" },
        ]}
      />
      <button type="button" className={primaryButton} disabled={!graph} onClick={() => setCreating({})}>
        <Plus size={13} />
        新建笔记
      </button>
    </>
  );

  const subtitle = graph
    ? `${graph.notes.length} 篇笔记 · ${counts.links} 条链接${counts.similar ? ` · ${counts.similar} 条相似建议` : ""}${graph.similarSkipped ? " · 笔记太多，没算相似建议" : ""} · 用 [[文件名]] 互相链接`
    : "跨会话记忆的关系图：笔记之间用 [[文件名]] 互相链接";

  return (
    <NavPageShell title="记忆" subtitle={subtitle} actions={supported ? actions : undefined} insetLeft={insetLeft}>
      {!supported ? (
        <EmptyCard icon={CircleOff} title="这台 Dock 还不支持记忆图谱" body="连接的 Dock 版本较旧。升级到带 memoryGraph 能力的版本后再来。" />
      ) : (
        <>
          {(error || (graph && !graph.enabled && graph.notes.length > 0)) && (
            <div className="flex shrink-0 flex-col gap-2 px-8 pt-3">
              {error && (
                <Banner
                  kind="error"
                  action={
                    <button type="button" className="shrink-0 font-medium hover:underline" onClick={() => void load()}>
                      重试
                    </button>
                  }
                >
                  {error}
                </Banner>
              )}
              {graph && !graph.enabled && graph.notes.length > 0 && (
                <Banner
                  kind="info"
                  action={
                    !graph.forcedOff && (
                      <button type="button" className="shrink-0 font-medium hover:underline" onClick={openMemorySettings}>
                        去设置开启
                      </button>
                    )
                  }
                >
                  {graph.forcedOff ? "记忆被 DOCK_MEMORY=0 强制关闭：" : "记忆功能关着："}agent 现在不会读到这些笔记，也不会写新的。这里照常能看、能改。
                </Banner>
              )}
            </div>
          )}

          {/* 一篇笔记都没有时不摆搜索和筛选：没东西可筛。 */}
          <div className={`flex shrink-0 flex-wrap items-center gap-2 px-8 py-3 ${graph && graph.notes.length === 0 ? "hidden" : ""}`}>
            <label className="flex h-8 w-60 items-center gap-2 rounded-lg border border-line-strong bg-white px-2.5 shadow-sm focus-within:border-accent">
              <Search size={13} className="shrink-0 text-ink-4" />
              <input
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                onKeyDown={(e) => e.key === "Escape" && (e.stopPropagation(), setQuery(""))}
                placeholder="搜标题和正文"
                className="min-w-0 flex-1 select-text bg-transparent text-[12.5px] text-ink outline-none placeholder:text-ink-4"
              />
              {query && (
                <>
                  <span className="shrink-0 text-[11px] tabular-nums text-ink-4">{matched?.size ?? 0}</span>
                  <button type="button" title="清空" aria-label="清空" onClick={() => setQuery("")} className="shrink-0 text-ink-4 hover:text-ink">
                    <X size={12} />
                  </button>
                </>
              )}
            </label>
            <Segmented
              value={view}
              onChange={setView}
              options={[
                { value: "graph", label: "图谱" },
                { value: "list", label: "列表" },
              ]}
            />
            <span className="mx-1 h-4 w-px bg-line" />
            <Chip on={filters.topics} onClick={() => setFilters((f) => ({ ...f, topics: !f.topics }))} title="整理好的稳定笔记（topics/）">
              主题 {counts.topics}
            </Chip>
            <Chip on={filters.observations} onClick={() => setFilters((f) => ({ ...f, observations: !f.observations }))} title="还没整理的新观察（observations/_inbox/）">
              观察 {counts.observations}
            </Chip>
            {view === "graph" && (
              <Chip on={filters.orphans} onClick={() => setFilters((f) => ({ ...f, orphans: !f.orphans }))} title="没有任何链接的笔记（相似建议不算）">
                孤立笔记
              </Chip>
            )}
            <span className="flex-1" />
            <button
              type="button"
              title="刷新"
              aria-label="刷新"
              onClick={() => void load()}
              className="motion-colors flex size-8 items-center justify-center rounded-lg text-ink-3 hover:bg-hover hover:text-ink"
            >
              <RefreshCw size={14} className={refreshing ? "animate-spin" : ""} />
            </button>
          </div>

          <div className="relative flex min-h-0 flex-1 border-t border-line-soft">
            {!graph ? (
              !error && <LoadingGraph />
            ) : graph.notes.length === 0 ? (
              <EmptyCard
                icon={BrainCircuit}
                title={mode === "current" && total > 0 ? "这个项目还没有记忆" : "还没有记忆"}
                body={
                  graph.enabled
                    ? "agent 会把对话里值得记住的东西写成笔记，笔记之间互相链接就连成了图。也可以自己先写一篇。"
                    : "记忆功能还没开启。开启后 agent 会把值得记住的东西写成笔记；你也可以先自己写。"
                }
              >
                <div className="mt-1 flex items-center gap-2">
                  {!graph.enabled && !graph.forcedOff && (
                    <button type="button" className={secondaryButton} onClick={openMemorySettings}>
                      去设置开启
                    </button>
                  )}
                  {mode === "current" && total > 0 && (
                    <button type="button" className={secondaryButton} onClick={() => setMode("all")}>
                      看全部项目（{total} 篇）
                    </button>
                  )}
                  <button type="button" className={primaryButton} onClick={() => setCreating({})}>
                    <Plus size={13} />
                    新建笔记
                  </button>
                </div>
              </EmptyCard>
            ) : view === "graph" ? (
              <>
                <GraphCanvas
                  ref={canvas}
                  nodes={shown.nodes}
                  edges={shown.edges}
                  scopeLabels={scopeLabels}
                  currentScope={graph.currentScope}
                  selectedId={selected}
                  highlight={matched}
                  settings={settings}
                  colors={colors}
                  legendGroups={legendGroups}
                  groupHits={groupHits.map((ids) => (ids ? [...ids].filter((id) => shownIds.has(id)).length : null))}
                  insetRight={selected ? DRAWER_WIDTH : 0}
                  onSelect={(node) => {
                    if (node?.ghost) {
                      setEdgeCard(null);
                      setCreating({ title: node.label, scope: node.scope });
                    } else select(node?.id ?? null);
                  }}
                  onLink={(from, to) => void link(from.id, to.id)}
                  onEdge={(edge, source, target, at) => {
                    // 指向还没有的笔记的线：直接去新建那一篇。
                    if (edge.kind === "ghost") setCreating({ title: target.label, scope: source.scope });
                    else setEdgeCard({ edge, source, target, at });
                  }}
                />
                {shown.nodes.length === 0 && (
                  <div className="pointer-events-none absolute inset-0 flex items-center justify-center text-[12.5px] text-ink-4">筛选条件下没有笔记</div>
                )}
                {edgeCard && (
                  <EdgeCard
                    {...edgeCard}
                    onLink={(from, to) => void link(from, to)}
                    onUnlink={(from, to) => void unlink(from, to)}
                    onOpen={(id) => select(id)}
                    onClose={() => setEdgeCard(null)}
                  />
                )}
              </>
            ) : (
              <NoteList
                graph={graph}
                notes={listed}
                selectedId={selected}
                snippets={contentHits}
                insetRight={selected ? DRAWER_WIDTH : 0}
                onSelect={(id) => select(id)}
              />
            )}

            {selected && graph && (
              <NoteDrawer
                id={selected}
                graph={graph}
                revision={revision}
                remote={remote}
                settings={settings}
                colors={colors}
                onSelect={(id) => select(id, true)}
                onMoved={(id) => {
                  awaited.current = id;
                  setSelected(id);
                }}
                onDirty={noteDirty}
                onChanged={() => void load(true)}
                onCreateFromLink={(title, scope) => setCreating({ title, scope })}
                onClose={() => select(null)}
              />
            )}
            {toast && (
              <div key={toast.key} className="toast-2s pointer-events-none absolute inset-x-0 top-3 z-30 flex justify-center">
                <span className="rounded-full bg-ink/90 px-3 py-1.5 text-[12px] text-white shadow-sm">{toast.text}</span>
              </div>
            )}
          </div>

          {discarding && (
            <Modal onClose={() => setDiscarding(null)} width={380}>
              <div className="text-[15px] font-semibold text-ink">放弃没保存的修改？</div>
              <div className="text-[12.5px] leading-relaxed text-ink-3">这篇笔记改了还没保存，离开的话这些修改就没了。</div>
              <div className="flex justify-end gap-2 pt-1">
                <button type="button" className={secondaryButton} onClick={() => setDiscarding(null)}>
                  继续编辑
                </button>
                <button
                  type="button"
                  className={primaryButton}
                  onClick={() => {
                    dirty.current = false;
                    const { run } = discarding;
                    setDiscarding(null);
                    run();
                  }}
                >
                  放弃修改
                </button>
              </div>
            </Modal>
          )}
          {creating && graph && (
            <NewNoteDialog
              scopes={writableScopes}
              initial={creating}
              onClose={() => setCreating(null)}
              onCreated={(note) => {
                setCreating(null);
                void load(true).then(() => select(note.id, true));
              }}
            />
          )}
        </>
      )}
    </NavPageShell>
  );
}

function Chip({ on, onClick, title, children }: { on: boolean; onClick: () => void; title?: string; children: React.ReactNode }) {
  return (
    <button
      type="button"
      title={title}
      aria-pressed={on}
      onClick={onClick}
      className={`motion-colors flex h-7 items-center rounded-full border px-2.5 text-xs ${
        on ? "border-[#0a84ff40] bg-[#0a84ff14] font-medium text-ink" : "border-line-strong bg-white text-ink-4 hover:text-ink-2"
      }`}
    >
      {children}
    </button>
  );
}

/** 第一次读图时的占位：几颗慢慢亮起来的点，别让整页空着。 */
function LoadingGraph() {
  return (
    <div className="flex flex-1 items-center justify-center">
      <div className="flex items-center gap-3">
        {[0, 1, 2].map((i) => (
          <span key={i} className="skeleton size-3 rounded-full" style={{ animationDelay: `${i * 160}ms` }} />
        ))}
      </div>
    </div>
  );
}
