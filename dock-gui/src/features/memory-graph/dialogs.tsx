// 记忆页的弹窗：新建笔记、重命名、遗忘确认，以及点连线时冒出来的小卡片。

import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { ArrowLeftRight, Link2, Link2Off, LoaderCircle, Plus } from "lucide-react";
import { ErrorLine, Modal, primaryButton, secondaryButton, Segmented, smallButton } from "../../components/settings/ui";
import { imeComposing } from "../../dock/ime";
import { errorText } from "../../dock/settingsApi";
import {
  noteCreate,
  noteForget,
  noteRename,
  scopeName,
  type MemoryScope,
  type NoteContent,
  type NoteKind,
  type Relocated,
} from "./api";
import type { ViewEdge, ViewNode } from "./view";

const fieldClass =
  "h-8 w-full select-text rounded-lg border border-line-strong bg-white px-2.5 text-[13px] text-ink outline-none placeholder:text-ink-4 focus:border-accent";
const labelClass = "text-xs font-semibold text-ink-4";

/** 和 Dock 给文件起名的规则一致（dock-memory 的 note_stem）：留字母数字和汉字，其余连成 `-`。 */
export function suggestName(title: string): string {
  let out = "";
  let dash = false;
  for (const ch of title.trim()) {
    if (/[\p{L}\p{N}]/u.test(ch)) {
      if (dash && out) out += "-";
      dash = false;
      out += ch.toLowerCase();
    } else dash = true;
  }
  return [...out].slice(0, 48).join("").replace(/^-+|-+$/g, "");
}

export function NewNoteDialog({
  scopes,
  initial,
  onClose,
  onCreated,
}: {
  /** 能往里建笔记的 scope（当前项目、全局，以及图上已有的其他项目）。 */
  scopes: MemoryScope[];
  initial: { title?: string; scope?: string };
  onClose: () => void;
  onCreated: (note: NoteContent) => void;
}) {
  const [title, setTitle] = useState(initial.title ?? "");
  const [body, setBody] = useState("");
  const [kind, setKind] = useState<NoteKind>("topic");
  const [scope, setScope] = useState(initial.scope ?? scopes.find((s) => s.current)?.id ?? scopes[0]?.id ?? "global");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const titleRef = useRef<HTMLInputElement>(null);
  useEffect(() => titleRef.current?.focus(), []);

  const create = async () => {
    if (!title.trim() || busy) return;
    setBusy(true);
    setError(null);
    try {
      onCreated(await noteCreate({ scope, kind, title: title.trim(), text: body }));
    } catch (e) {
      setError(errorText(e));
      setBusy(false);
    }
  };
  // 范围多的时候（全部项目）换成下拉；两三个时直接分段选。
  const options = scopes.map((s) => ({ value: s.id, label: s.current ? `${scopeName(s)}（当前项目）` : scopeName(s) }));

  return (
    <Modal onClose={onClose} width={480}>
      <div className="text-[15px] font-semibold text-ink">新建笔记</div>
      <label className="flex flex-col gap-1.5">
        <span className={labelClass}>标题</span>
        <input
          ref={titleRef}
          value={title}
          onChange={(e) => setTitle(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && !imeComposing(e) && void create()}
          placeholder="例如：登录方案的决定"
          className={fieldClass}
        />
        {title.trim() && (
          <span className="text-[11.5px] text-ink-4">
            文件名 <span className="font-mono">{suggestName(title) || "note-<时间>"}.md</span>，别的笔记用{" "}
            <span className="font-mono">[[{suggestName(title) || "…"}]]</span> 链接它
          </span>
        )}
      </label>
      <div className="flex items-center gap-3">
        <div className="flex flex-col gap-1.5">
          <span className={labelClass}>类别</span>
          <Segmented
            value={kind}
            onChange={setKind}
            options={[
              { value: "topic", label: "主题", title: "整理好的稳定笔记" },
              { value: "observation", label: "观察", title: "还没整理的新发现" },
            ]}
          />
        </div>
        <div className="flex min-w-0 flex-1 flex-col gap-1.5">
          <span className={labelClass}>放在</span>
          {options.length <= 3 ? (
            <div className="flex">
              <Segmented value={scope} onChange={setScope} options={options} />
            </div>
          ) : (
            <select value={scope} onChange={(e) => setScope(e.target.value)} className={fieldClass}>
              {options.map((o) => (
                <option key={o.value} value={o.value}>
                  {o.label}
                </option>
              ))}
            </select>
          )}
        </div>
      </div>
      <label className="flex flex-col gap-1.5">
        <span className={labelClass}>正文（可以先空着）</span>
        <textarea
          value={body}
          onChange={(e) => setBody(e.target.value)}
          rows={6}
          spellCheck={false}
          placeholder="Markdown。用 [[文件名]] 链接别的笔记。"
          className="thin-scroll w-full resize-none select-text rounded-lg border border-line-strong bg-white px-2.5 py-2 font-mono text-[12.5px] leading-relaxed text-ink outline-none placeholder:font-sans placeholder:text-ink-4 focus:border-accent"
        />
      </label>
      {error && <ErrorLine>{error}</ErrorLine>}
      <div className="flex justify-end gap-2 pt-1">
        <button type="button" className={secondaryButton} onClick={onClose}>
          取消
        </button>
        <button type="button" className={primaryButton} disabled={!title.trim() || busy} onClick={() => void create()}>
          {busy ? <LoaderCircle size={13} className="animate-spin" /> : <Plus size={13} />}
          新建
        </button>
      </div>
    </Modal>
  );
}

export function RenameDialog({ note, onClose, onRenamed }: { note: NoteContent; onClose: () => void; onRenamed: (moved: Relocated) => void }) {
  const [title, setTitle] = useState(note.title);
  const [name, setName] = useState(note.name);
  // Dock 自己起的带时间的文件名（flush-2026…）没有含义：默认让文件名跟着标题走。
  const [follow, setFollow] = useState(note.origin !== "authored");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const effectiveName = follow ? suggestName(title) || note.name : name.trim();
  const changed = title.trim() !== note.title || (effectiveName !== "" && effectiveName !== note.name);

  const submit = async () => {
    if (!title.trim() || !changed || busy) return;
    setBusy(true);
    setError(null);
    try {
      onRenamed(
        await noteRename(note.id, {
          title: title.trim() !== note.title ? title.trim() : undefined,
          name: effectiveName !== note.name ? effectiveName : undefined,
        }),
      );
    } catch (e) {
      setError(errorText(e));
      setBusy(false);
    }
  };

  return (
    <Modal onClose={onClose} width={440}>
      <div className="text-[15px] font-semibold text-ink">重命名</div>
      <label className="flex flex-col gap-1.5">
        <span className={labelClass}>标题</span>
        <input
          autoFocus
          value={title}
          onChange={(e) => setTitle(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && !imeComposing(e) && void submit()}
          className={fieldClass}
        />
      </label>
      <label className="flex flex-col gap-1.5">
        <span className={labelClass}>文件名</span>
        <div className="flex items-center gap-2">
          <input
            value={follow ? effectiveName : name}
            disabled={follow}
            onChange={(e) => setName(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && !imeComposing(e) && void submit()}
            className={`${fieldClass} font-mono disabled:bg-[#f5f5f7] disabled:text-ink-3`}
          />
          <span className="shrink-0 font-mono text-[12.5px] text-ink-4">.md</span>
        </div>
        <label className="flex items-center gap-1.5 text-[12px] text-ink-3">
          <input type="checkbox" checked={follow} onChange={(e) => setFollow(e.target.checked)} />
          跟着标题走
        </label>
      </label>
      <span className="text-[11.5px] leading-relaxed text-ink-4">
        别的笔记里指向它的 <span className="font-mono">[[{note.name}]]</span> 会一起改写，链接不会断。
      </span>
      {error && <ErrorLine>{error}</ErrorLine>}
      <div className="flex justify-end gap-2 pt-1">
        <button type="button" className={secondaryButton} onClick={onClose}>
          取消
        </button>
        <button type="button" className={primaryButton} disabled={!title.trim() || !changed || busy} onClick={() => void submit()}>
          {busy && <LoaderCircle size={13} className="animate-spin" />}
          保存
        </button>
      </div>
    </Modal>
  );
}

export function ForgetDialog({
  note,
  backlinks,
  onClose,
  onForgotten,
}: {
  note: NoteContent;
  /** 有几篇笔记链接到它（遗忘后这些链接会悬空）。 */
  backlinks: number;
  onClose: () => void;
  onForgotten: () => void;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const forget = async () => {
    setBusy(true);
    setError(null);
    try {
      await noteForget(note.id, note.hash);
      onForgotten();
    } catch (e) {
      setError(errorText(e));
      setBusy(false);
    }
  };
  return (
    <Modal onClose={onClose} width={400}>
      <div className="text-[15px] font-semibold text-ink">遗忘这篇笔记？</div>
      <div className="text-[12.5px] leading-relaxed text-ink-3">
        「{note.title}」会从记忆里拿掉：agent 不再读到它，也搜不到。文件挪进这个范围的 <span className="font-mono">archive/</span> 目录，不会真的删除。
        {backlinks > 0 && <> 有 {backlinks} 篇笔记链接到它，那些链接会变成「还没写」。</>}
      </div>
      {error && <ErrorLine>{error}</ErrorLine>}
      <div className="flex justify-end gap-2 pt-1">
        <button type="button" className={secondaryButton} onClick={onClose}>
          取消
        </button>
        <button
          type="button"
          disabled={busy}
          onClick={() => void forget()}
          className="motion-colors flex h-8 shrink-0 items-center gap-1.5 rounded-lg bg-[#ff3b30] px-3 text-[12.5px] font-medium text-white shadow-sm hover:bg-[#e0352b] disabled:opacity-50"
        >
          {busy && <LoaderCircle size={13} className="animate-spin" />}
          遗忘
        </button>
      </div>
    </Modal>
  );
}

/**
 * 点了图上的一条线：链接可以断开；相似建议可以转成链接（链接写在其中一篇里，
 * 所以要说清楚写在哪篇——默认写在线的起点那篇，可以对调）。
 */
export function EdgeCard({
  edge,
  source,
  target,
  at,
  insetRight,
  onLink,
  onUnlink,
  onOpen,
  onClose,
}: {
  edge: ViewEdge;
  source: ViewNode;
  target: ViewNode;
  at: { x: number; y: number };
  /** 右边被抽屉盖住的宽度。 */
  insetRight: number;
  onLink: (from: string, to: string) => void;
  onUnlink: (from: string, to: string) => void;
  onOpen: (id: string) => void;
  onClose: () => void;
}) {
  const [swapped, setSwapped] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState({ left: Math.max(8, at.x - 140), top: at.y + 12 });
  // 量出卡片多大再摆：右边不伸到抽屉底下，下面放不下就翻到点击处上面。
  useLayoutEffect(() => {
    const card = ref.current;
    const box = card?.parentElement;
    if (!card || !box) return;
    const [w, h] = [card.offsetWidth, card.offsetHeight];
    const left = Math.max(8, Math.min(at.x - w / 2, box.clientWidth - insetRight - w - 8));
    const below = at.y + 12;
    const top = below + h > box.clientHeight - 8 ? Math.max(8, at.y - 12 - h) : below;
    setPos({ left, top });
  }, [at.x, at.y, insetRight, edge.kind]);
  useEffect(() => {
    const onDown = (e: MouseEvent) => !ref.current?.contains(e.target as Node) && onClose();
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    // 这一下点击本身不算「点外面」。
    const timer = window.setTimeout(() => window.addEventListener("mousedown", onDown), 0);
    window.addEventListener("keydown", onKey);
    return () => {
      window.clearTimeout(timer);
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [onClose]);

  const [from, to] = swapped ? [target, source] : [source, target];
  const canWrite = (n: ViewNode) => !!n.note?.writable;
  return (
    <div
      ref={ref}
      className="glass-popover pop-in absolute z-10 flex w-[280px] flex-col gap-2 rounded-xl border border-line-strong bg-white p-3 shadow-[0px_8px_28px_rgba(0,0,0,0.12)]"
      style={pos}
    >
      {edge.kind === "similar" ? (
        <>
          <div className="text-[12.5px] font-semibold text-ink">内容相近 {Math.round(edge.score * 100)}%</div>
          <div className="flex items-center gap-1.5 text-[12.5px] leading-snug text-ink-3">
            <span className="min-w-0 flex-1">
              在「<NoteName node={from} onOpen={onOpen} />」里加一条指向「<NoteName node={to} onOpen={onOpen} />」的链接
            </span>
            <button
              type="button"
              title="对调方向"
              aria-label="对调方向"
              onClick={() => setSwapped((v) => !v)}
              className="motion-colors flex size-6 shrink-0 items-center justify-center rounded-md text-ink-4 hover:bg-hover hover:text-ink"
            >
              <ArrowLeftRight size={13} />
            </button>
          </div>
          <button type="button" className={`${smallButton} self-start`} disabled={!canWrite(from)} title={canWrite(from) ? undefined : "这篇笔记只读"} onClick={() => onLink(from.id, to.id)}>
            <Link2 size={12} />
            建立链接
          </button>
        </>
      ) : (
        <>
          <div className="text-[12.5px] font-semibold text-ink">{edge.mutual ? "互相链接" : "链接"}</div>
          <div className="text-[12.5px] leading-snug text-ink-3">
            「<NoteName node={source} onOpen={onOpen} />」{edge.mutual ? "和" : "引用了"}「<NoteName node={target} onOpen={onOpen} />」{edge.mutual ? "互相引用" : ""}
          </div>
          <div className="flex flex-wrap gap-1.5">
            <button type="button" className={smallButton} disabled={!canWrite(source)} onClick={() => onUnlink(source.id, target.id)}>
              <Link2Off size={12} />
              {edge.mutual ? `断开「${short(source.label)}」里的` : "断开"}
            </button>
            {edge.mutual && (
              <button type="button" className={smallButton} disabled={!canWrite(target)} onClick={() => onUnlink(target.id, source.id)}>
                <Link2Off size={12} />
                断开「{short(target.label)}」里的
              </button>
            )}
          </div>
        </>
      )}
    </div>
  );
}

const short = (text: string, max = 8) => (text.length > max ? `${text.slice(0, max - 1)}…` : text);

/** 句子里的笔记名：点了打开那一篇。flush 摘要的标题是一整句话，卡片里只放开头，全文在悬停提示里。 */
function NoteName({ node, onOpen }: { node: ViewNode; onOpen: (id: string) => void }) {
  return (
    <span
      role="link"
      tabIndex={0}
      title={node.label}
      onClick={() => onOpen(node.id)}
      onKeyDown={(e) => e.key === "Enter" && onOpen(node.id)}
      className="cursor-pointer font-medium text-ink hover:underline"
    >
      {short(node.label, 16)}
    </span>
  );
}
