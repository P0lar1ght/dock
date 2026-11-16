// 右侧笔记抽屉：局部图谱、一篇笔记的正文（预览 / 编辑）、它引用了谁、谁引用了它、相似建议，
// 以及改名、升为主题、挪到全局、遗忘这些操作。不盖住图：开着抽屉照样能在图上点别的笔记。

import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import {
  ArrowUpRight,
  Copy,
  CornerDownLeft,
  Ellipsis,
  FolderOpen,
  Globe,
  Link2,
  Link2Off,
  LoaderCircle,
  Pencil,
  Plus,
  Sparkles,
  Trash2,
  TypeOutline,
  X,
} from "lucide-react";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { IN_TAURI } from "../../components/chrome";
import { Markdown } from "../../components/Markdown";
import { ago, fullTime } from "../../components/navPage";
import { Popover } from "../../components/Popover";
import { Banner, Pill, primaryButton, secondaryButton, smallButton } from "../../components/settings/ui";
import { RpcError } from "../../dock/client";
import { errorText } from "../../dock/settingsApi";
import { fileEditorEntry } from "../registry";
import {
  linkNotes,
  noteGet,
  noteMove,
  noteSave,
  scopeName,
  unlinkNotes,
  type MemoryGraph,
  type MemoryNote,
  type NoteDetail,
} from "./api";
import { ForgetDialog, RenameDialog } from "./dialogs";
import { LocalGraph } from "./LocalGraph";
import type { GraphSettings } from "./settings";
import { GROUP_COLOR, groupOf, suggestionsFor } from "./view";
import { linkifyWikilinks, wikilinkTarget } from "./wikilinks";

export const DRAWER_WIDTH = 440;

export function NoteDrawer({
  id,
  graph,
  revision,
  remote,
  settings,
  colors,
  onSelect,
  onMoved,
  onChanged,
  onCreateFromLink,
  onDirty,
  onClose,
}: {
  id: string;
  graph: MemoryGraph;
  /** 图每重拉一次加一：抽屉跟着重读这篇（agent 改了它，或别处的链接变了）。 */
  revision: number;
  /** 连的是远程 Dock：文件在那台机器上，本机访达打不开。 */
  remote: boolean;
  settings: GraphSettings;
  /** 按条件着色（局部图谱和大图一致）。 */
  colors: Map<string, string> | null;
  onSelect: (id: string) => void;
  /** 改名 / 挪位置后 id 变了。 */
  onMoved: (id: string | null) => void;
  /** 改了东西：让页面重拉图。 */
  onChanged: () => void;
  /** 点了指向还不存在的笔记的链接：去新建它。 */
  onCreateFromLink: (title: string, scope: string) => void;
  /** 有没有没保存的修改：页面在换一篇 / 关抽屉之前先问一句。 */
  onDirty: (dirty: boolean) => void;
  onClose: () => void;
}) {
  const [detail, setDetail] = useState<NoteDetail | null>(null);
  const [error, setError] = useState<string | null>(null);
  // 编辑状态记的是「正在改哪一篇」：换了一篇，这一帧起就不算在编辑，不会拦住新这篇的读取。
  const [editFor, setEditFor] = useState<string | null>(null);
  const [dirtyFor, setDirtyFor] = useState<string | null>(null);
  const editing = editFor === id;
  const dirty = editing && dirtyFor === id;
  const setEditing = (on: boolean) => setEditFor(on ? id : null);
  const setDirty = (on: boolean) => setDirtyFor(on ? id : null);
  const [saving, setSaving] = useState(false);
  const [conflict, setConflict] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);
  const [dialog, setDialog] = useState<"rename" | "forget" | null>(null);
  const draft = useRef<(() => string) | null>(null);
  const aside = useRef<HTMLElement>(null);
  const editingRef = useRef(false);
  editingRef.current = editing;
  useEffect(() => onDirty(editing && dirty), [editing, dirty, onDirty]);
  useEffect(() => () => onDirty(false), [onDirty]);

  // 换了一篇：编辑状态清掉。
  useEffect(() => {
    setEditFor(null);
    setDirtyFor(null);
    setConflict(false);
    setError(null);
    setDialog(null);
    draft.current = null;
  }, [id]);

  useEffect(() => {
    let live = true;
    // 正在改这一篇的时候不拿盘上的新版本盖掉草稿；保存时对不上指纹会提示。
    // 退出编辑时（`editing` 变了）再读一次：编辑期间盘上可能已经被改过。
    if (editingRef.current) return;
    noteGet(id)
      .then((d) => {
        if (!live) return;
        setDetail(d);
        setError(null);
      })
      .catch((e) => {
        if (!live) return;
        // 这篇没了（被遗忘、被改名）：收起抽屉。
        if (e instanceof RpcError && e.code === "not_found") onMoved(null);
        else setError(errorText(e));
      });
    return () => {
      live = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [id, revision, editing]);

  const note = detail?.note.id === id ? detail.note : null;
  const links = detail?.note.id === id ? detail.links : null;
  // 还没读到全文时先用图上那一份（标题、类别）把抽屉撑起来。
  const head: MemoryNote | null = note ?? graph.notes.find((n) => n.id === id) ?? null;
  const scope = graph.scopes.find((s) => s.id === head?.scope);
  const suggestions = useMemo(() => suggestionsFor(graph, id), [graph, id]);

  const run = useCallback(
    async (label: string, work: () => Promise<void>) => {
      setBusy(label);
      setError(null);
      try {
        await work();
      } catch (e) {
        setError(errorText(e));
      } finally {
        setBusy(null);
      }
    },
    [],
  );

  const save = async (force: boolean) => {
    if (!note || !draft.current) return;
    setSaving(true);
    setError(null);
    try {
      const saved = await noteSave(id, draft.current(), force ? null : note.hash);
      setDetail((d) => (d ? { ...d, note: saved } : d));
      setEditing(false);
      setDirty(false);
      setConflict(false);
      onChanged();
    } catch (e) {
      if (e instanceof RpcError && e.code === "conflict") setConflict(true);
      else setError(errorText(e));
    } finally {
      setSaving(false);
    }
  };

  const reload = () =>
    noteGet(id)
      .then((d) => {
        setDetail(d);
        setEditing(false);
        setDirty(false);
        setConflict(false);
      })
      .catch((e) => setError(errorText(e)));

  const link = (to: string) =>
    run("link", async () => {
      await linkNotes(id, to);
      setDetail(await noteGet(id));
      onChanged();
    });
  const unlink = (target: { to: string } | { dangling: string }) =>
    run("unlink", async () => {
      await unlinkNotes(id, target);
      setDetail(await noteGet(id));
      onChanged();
    });
  const move = (to: { scope?: string; kind?: "topic" }) =>
    run("move", async () => {
      const moved = await noteMove(id, to);
      onMoved(moved.note.id);
      onChanged();
    });

  // Esc：先退出编辑（没改过的话），再关抽屉。弹窗开着时它自己处理。
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      // 别处已经处理了这一下 Esc（图谱设置面板先关自己）。
      if (e.key !== "Escape" || dialog || e.defaultPrevented) return;
      // 抽屉里有菜单开着（更多、链接到…）：这一下 Esc 是关菜单的，别连抽屉一起关。
      // 局部图谱那种展开 / 收起的折叠按钮（data-disclosure）不算菜单。
      if (aside.current?.querySelector('[aria-expanded="true"]:not([data-disclosure])')) return;
      // 图谱设置面板开着：这一下 Esc 归它（面板自己在 window 上听，可能排在后面）。
      if (document.querySelector("[data-graph-settings]")) return;
      if (editing) {
        if (!dirty) setEditing(false);
        return;
      }
      onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [dialog, editing, dirty, onClose]);

  /** 预览里点了笔记链接。 */
  const followLink = (e: React.MouseEvent) => {
    const anchor = (e.target as HTMLElement).closest("a");
    const target = wikilinkTarget(anchor?.getAttribute("href"));
    if (target === null) return;
    e.preventDefault();
    e.stopPropagation();
    const resolved = links?.targets.find((t) => t.target === target);
    if (resolved?.id) onSelect(resolved.id);
    else if (head) onCreateFromLink(target, head.scope);
  };

  const Editor = fileEditorEntry()?.Editor;
  const group = head ? groupOf(graph, head.scope) : "other";

  return (
    <>
    <aside
      ref={aside}
      className="glass-dialog slide-in-right absolute bottom-0 right-0 top-0 z-20 flex flex-col border-l border-line-strong bg-white shadow-[-16px_0px_40px_rgba(0,0,0,0.06)]"
      style={{ width: DRAWER_WIDTH }}
    >
      <div className="flex shrink-0 flex-col gap-2 border-b border-line-soft px-5 pb-4 pt-4">
        <div className="flex items-center gap-1.5">
          {head && (
            <>
              <Pill kind={head.kind === "topic" ? "blue" : "gray"}>{head.kind === "topic" ? "主题" : "观察"}</Pill>
              <span className="flex min-w-0 items-center gap-1.5 text-[11.5px] text-ink-4">
                <span className="size-2 shrink-0 rounded-full" style={{ background: GROUP_COLOR[group] }} />
                <span className="truncate">{scope ? scopeName(scope) : head.scope}</span>
              </span>
              {!head.writable && <Pill title="嵌套目录和旧版目录里的笔记只能看">只读</Pill>}
            </>
          )}
          <span className="flex-1" />
          {head?.writable && !editing && (
            <HeadButton
              label="编辑"
              disabled={!note}
              onClick={() => {
                fileEditorEntry()?.preload();
                setEditing(true);
              }}
            >
              <Pencil size={14} />
            </HeadButton>
          )}
          {head && (
            <Popover
              title="更多"
              side="bottom"
              align="right"
              className="flex size-7 items-center justify-center rounded-md text-ink-3 hover:bg-hover hover:text-ink"
              trigger={<Ellipsis size={15} />}
            >
              {(close) => (
                <>
                  {head.writable && (
                    <MenuRow icon={TypeOutline} label="重命名…" onClick={() => (close(), setDialog("rename"))} />
                  )}
                  {head.writable && head.kind === "observation" && (
                    <MenuRow icon={ArrowUpRight} label="升为主题" hint="整理好的稳定笔记" onClick={() => (close(), void move({ kind: "topic" }))} />
                  )}
                  {head.writable && head.scope !== "global" && (
                    <MenuRow icon={Globe} label="移到全局" hint="所有项目都看得到" onClick={() => (close(), void move({ scope: "global" }))} />
                  )}
                  {head.writable && head.scope === "global" && graph.currentScope && (
                    <MenuRow icon={CornerDownLeft} label="移到当前项目" onClick={() => (close(), void move({ scope: graph.currentScope ?? undefined }))} />
                  )}
                  <MenuRow icon={Copy} label="复制文件路径" onClick={() => (close(), void navigator.clipboard.writeText(head.path))} />
                  {IN_TAURI && !remote && (
                    <MenuRow icon={FolderOpen} label="在访达中显示" onClick={() => (close(), void revealItemInDir(head.path).catch(() => {}))} />
                  )}
                  {head.writable && (
                    <>
                      <div className="my-1 border-t border-line-soft" />
                      <MenuRow icon={Trash2} label="遗忘…" danger onClick={() => (close(), setDialog("forget"))} />
                    </>
                  )}
                </>
              )}
            </Popover>
          )}
          <HeadButton label="关闭" onClick={onClose}>
            <X size={15} />
          </HeadButton>
        </div>
        <h2 className="select-text text-base font-semibold leading-snug text-ink">{head?.title ?? "…"}</h2>
        {head && (
          <div className="flex min-w-0 items-center gap-1.5 text-[11.5px] text-ink-4">
            <span className="truncate font-mono" title={head.path}>
              {head.name}.md
            </span>
            {head.modifiedMs && (
              <>
                <span>·</span>
                <span className="shrink-0" title={fullTime(head.modifiedMs)}>
                  {ago(head.modifiedMs)}修改
                </span>
              </>
            )}
            <span>·</span>
            <span className="shrink-0">{formatBytes(head.bytes)}</span>
            {busy && <LoaderCircle size={12} className="ml-1 shrink-0 animate-spin" />}
          </div>
        )}
      </div>

      {(error || conflict) && (
        <div className="flex shrink-0 flex-col gap-2 px-5 pt-3">
          {error && <Banner kind="error">{error}</Banner>}
          {conflict && (
            <Banner
              kind="warn"
              action={
                <span className="flex shrink-0 items-center gap-1.5">
                  <button type="button" className={smallButton} onClick={() => void reload()}>
                    放弃我的修改
                  </button>
                  <button type="button" className={smallButton} onClick={() => void save(true)}>
                    覆盖保存
                  </button>
                </span>
              }
            >
              这篇笔记在你编辑期间被改过。
            </Banner>
          )}
        </div>
      )}

      {editing && note ? (
        <>
          <div className="glass-keep flex min-h-0 flex-1 flex-col overflow-hidden bg-white">
            {Editor ? (
              <Editor
                path={`${note.name}.md`}
                initial={note.text}
                onChange={(read) => {
                  draft.current = read;
                  setDirty(true);
                }}
                onSave={() => void save(false)}
              />
            ) : (
              <textarea
                defaultValue={note.text}
                spellCheck={false}
                autoFocus
                onChange={(e) => {
                  const value = e.target.value;
                  draft.current = () => value;
                  setDirty(true);
                }}
                onKeyDown={(e) => {
                  if ((e.metaKey || e.ctrlKey) && e.key === "s") {
                    e.preventDefault();
                    void save(false);
                  }
                }}
                className="thin-scroll min-h-0 flex-1 resize-none select-text px-5 py-4 font-mono text-[12.5px] leading-relaxed text-ink outline-none"
              />
            )}
          </div>
          <div className="flex shrink-0 items-center gap-2 border-t border-line-soft px-5 py-3">
            <span className="min-w-0 flex-1 truncate text-[11.5px] text-ink-4">用 [[文件名]] 链接别的笔记 · ⌘S 保存</span>
            <button
              type="button"
              className={secondaryButton}
              onClick={() => {
                setEditing(false);
                setDirty(false);
                setConflict(false);
              }}
            >
              取消
            </button>
            <button type="button" className={primaryButton} disabled={!dirty || saving} onClick={() => void save(false)}>
              {saving && <LoaderCircle size={13} className="animate-spin" />}
              保存
            </button>
          </div>
        </>
      ) : (
        <div className="thin-scroll flex min-h-0 flex-1 flex-col gap-5 overflow-y-auto px-5 py-4">
          {!note ? (
            !error && (
              <div className="flex flex-col gap-2.5">
                {[88, 96, 72, 90, 60].map((w, i) => (
                  <div key={i} className="skeleton h-3.5 rounded" style={{ width: `${w}%` }} />
                ))}
              </div>
            )
          ) : (
            <>
              <LocalGraph graph={graph} id={id} settings={settings} colors={colors} onOpen={onSelect} onCreate={onCreateFromLink} />
              {note.text.trim() ? (
                <div className="select-text" onClickCapture={followLink}>
                  <Markdown text={linkifyWikilinks(note.text)} size="view" />
                </div>
              ) : (
                <span className="text-[12.5px] text-ink-4">这篇笔记是空的。</span>
              )}

              <Section title="链接到" count={links?.outgoing.length ?? 0}>
                {links?.outgoing.map((l) => (
                  <LinkRow key={l.id} title={l.title} graph={graph} id={l.id} onOpen={() => onSelect(l.id)}>
                    {note.writable && (
                      <RowAction label="断开这条链接" onClick={() => void unlink({ to: l.id })}>
                        <Link2Off size={13} />
                      </RowAction>
                    )}
                  </LinkRow>
                ))}
                {links?.unresolved.map((target) => (
                  <div key={target} className="group flex min-h-8 items-center gap-2 rounded-lg px-2 py-1">
                    <span className="size-2 shrink-0 rounded-full border border-dashed border-ink-4" />
                    <span className="min-w-0 flex-1 truncate text-[13px] text-ink-3">{target}</span>
                    <span className="shrink-0 text-[11px] text-ink-4">还没写</span>
                    <RowAction label="新建这篇笔记" onClick={() => onCreateFromLink(target, note.scope)}>
                      <Plus size={13} />
                    </RowAction>
                    {note.writable && (
                      <RowAction label="去掉这条链接" onClick={() => void unlink({ dangling: target })}>
                        <Link2Off size={13} />
                      </RowAction>
                    )}
                  </div>
                ))}
                {note.writable && <LinkPicker graph={graph} from={note} linked={new Set(links?.outgoing.map((l) => l.id))} onPick={(to) => void link(to)} />}
              </Section>

              <Section title="被引用" count={links?.backlinks.length ?? 0} empty="还没有笔记链接到这一篇">
                {links?.backlinks.map((l) => (
                  <LinkRow key={l.id} title={l.title} graph={graph} id={l.id} context={l.context} onOpen={() => onSelect(l.id)} />
                ))}
              </Section>

              {suggestions.length > 0 && (
                <Section title="相似建议" count={suggestions.length} icon={<Sparkles size={12} className="text-ink-4" />}>
                  {suggestions.map((s) => (
                    <LinkRow key={s.note.id} title={s.note.title} graph={graph} id={s.note.id} onOpen={() => onSelect(s.note.id)} meta={`${Math.round(s.score * 100)}%`}>
                      {note.writable && (
                        <button type="button" className={`${smallButton} h-6`} disabled={busy !== null} onClick={() => void link(s.note.id)}>
                          <Link2 size={12} />
                          链接
                        </button>
                      )}
                    </LinkRow>
                  ))}
                </Section>
              )}
            </>
          )}
        </div>
      )}

    </aside>
      {/* 弹窗放在抽屉外面：玻璃材质下抽屉带模糊，里面的 fixed 元素会按抽屉定位。 */}
      {dialog === "rename" && note && (
        <RenameDialog
          note={note}
          onClose={() => setDialog(null)}
          onRenamed={(moved) => {
            setDialog(null);
            setDetail((d) => (d ? { ...d, note: moved.note } : d));
            onMoved(moved.note.id);
            onChanged();
          }}
        />
      )}
      {dialog === "forget" && note && (
        <ForgetDialog
          note={note}
          backlinks={links?.backlinks.length ?? 0}
          onClose={() => setDialog(null)}
          onForgotten={() => {
            setDialog(null);
            onMoved(null);
            onChanged();
          }}
        />
      )}
    </>
  );
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  return `${(bytes / 1024).toFixed(bytes < 10 * 1024 ? 1 : 0)} KB`;
}

function HeadButton({ label, onClick, disabled, children }: { label: string; onClick: () => void; disabled?: boolean; children: ReactNode }) {
  return (
    <button
      type="button"
      title={label}
      aria-label={label}
      disabled={disabled}
      onClick={onClick}
      className="motion-colors flex size-7 shrink-0 items-center justify-center rounded-md text-ink-3 hover:bg-hover hover:text-ink disabled:opacity-40"
    >
      {children}
    </button>
  );
}

function MenuRow({
  icon: Icon,
  label,
  hint,
  danger,
  onClick,
}: {
  icon: typeof Pencil;
  label: string;
  hint?: string;
  danger?: boolean;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={`motion-colors flex h-8 w-full shrink-0 items-center gap-2.5 px-2.5 text-left text-[13px] hover:bg-hover ${danger ? "text-[#d70015]" : "text-ink-2"}`}
    >
      <Icon size={14} className={danger ? "" : "text-ink-3"} />
      <span className="flex-1 whitespace-nowrap">{label}</span>
      {hint && <span className="whitespace-nowrap pl-3 text-[11px] text-ink-4">{hint}</span>}
    </button>
  );
}

function Section({
  title,
  count,
  icon,
  empty,
  children,
}: {
  title: string;
  count: number;
  icon?: ReactNode;
  empty?: string;
  children: ReactNode;
}) {
  return (
    <section className="flex flex-col gap-1">
      <div className="flex items-center gap-1.5 px-2 text-xs font-semibold text-ink-4">
        {icon}
        {title}
        <span className="font-medium text-ink-5">{count}</span>
      </div>
      {children}
      {count === 0 && empty && <span className="px-2 text-[12.5px] text-ink-4">{empty}</span>}
    </section>
  );
}

/** 一条相关笔记：点标题跳过去，右边是这条关系上能做的事。 */
function LinkRow({
  id,
  title,
  graph,
  context,
  meta,
  onOpen,
  children,
}: {
  id: string;
  title: string;
  graph: MemoryGraph;
  /** 反链：那篇里链接所在的那一行。 */
  context?: string;
  meta?: string;
  onOpen: () => void;
  children?: ReactNode;
}) {
  const note = graph.notes.find((n) => n.id === id);
  const color = note ? GROUP_COLOR[groupOf(graph, note.scope)] : GROUP_COLOR.other;
  return (
    <div className="group flex min-h-8 items-start gap-2 rounded-lg px-2 py-1 hover:bg-hover">
      <span
        className={`mt-[7px] size-2 shrink-0 rounded-full ${note?.kind === "observation" ? "border-2" : ""}`}
        style={note?.kind === "observation" ? { borderColor: color } : { background: color }}
      />
      <button type="button" onClick={onOpen} className="flex min-w-0 flex-1 flex-col gap-0.5 py-0.5 text-left">
        <span className="truncate text-[13px] font-medium text-ink">{title}</span>
        {context && <span className="line-clamp-2 text-[11.5px] leading-snug text-ink-4">{context}</span>}
      </button>
      {meta && <span className="mt-1 shrink-0 text-[11px] tabular-nums text-ink-4">{meta}</span>}
      {children && <span className="mt-0.5 flex shrink-0 items-center gap-1">{children}</span>}
    </div>
  );
}

function RowAction({ label, onClick, children }: { label: string; onClick: () => void; children: ReactNode }) {
  return (
    <button
      type="button"
      title={label}
      aria-label={label}
      onClick={onClick}
      className="motion-colors flex size-6 shrink-0 items-center justify-center rounded-md text-ink-4 opacity-0 hover:bg-selected hover:text-ink focus-visible:opacity-100 group-hover:opacity-100"
    >
      {children}
    </button>
  );
}

/** 「链接到…」：从图上的笔记里挑一篇（按标题 / 文件名过滤）。 */
function LinkPicker({ graph, from, linked, onPick }: { graph: MemoryGraph; from: MemoryNote; linked: Set<string>; onPick: (id: string) => void }) {
  const [query, setQuery] = useState("");
  const candidates = useMemo(() => {
    const q = query.trim().toLowerCase();
    return graph.notes
      .filter((n) => n.id !== from.id && !linked.has(n.id))
      .filter((n) => !q || n.title.toLowerCase().includes(q) || n.name.toLowerCase().includes(q))
      .sort((a, b) => (b.modifiedMs ?? 0) - (a.modifiedMs ?? 0))
      .slice(0, 40);
  }, [graph.notes, from.id, linked, query]);
  return (
    <Popover
      title="链接到另一篇笔记"
      side="bottom"
      align="left"
      wrapperClassName="self-start"
      panelClassName="w-[300px] max-h-80"
      className="ml-1 mt-0.5 flex h-7 items-center gap-1.5 rounded-md px-1.5 text-[12.5px] font-medium text-accent hover:bg-[#0a84ff14]"
      trigger={
        <>
          <Plus size={13} />
          链接到…
        </>
      }
    >
      {(close) => (
        <>
          <input
            autoFocus
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="按标题或文件名找"
            className="mx-1.5 mb-1 mt-0.5 h-7 shrink-0 select-text rounded-md border border-line-strong bg-white px-2 text-[12.5px] text-ink outline-none placeholder:text-ink-4 focus:border-accent"
          />
          {candidates.length === 0 && <span className="px-2.5 py-2 text-[12.5px] text-ink-4">没有可以链接的笔记</span>}
          {candidates.map((n) => (
            <button
              key={n.id}
              type="button"
              onClick={() => {
                close();
                onPick(n.id);
              }}
              className="motion-colors flex min-h-8 shrink-0 items-center gap-2 px-2.5 py-1 text-left hover:bg-hover"
            >
              <span
                className={`size-2 shrink-0 rounded-full ${n.kind === "observation" ? "border-2" : ""}`}
                style={n.kind === "observation" ? { borderColor: GROUP_COLOR[groupOf(graph, n.scope)] } : { background: GROUP_COLOR[groupOf(graph, n.scope)] }}
              />
              <span className="min-w-0 flex-1 truncate text-[13px] text-ink">{n.title}</span>
            </button>
          ))}
        </>
      )}
    </Popover>
  );
}
