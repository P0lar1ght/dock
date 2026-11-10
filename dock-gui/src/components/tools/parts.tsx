// 工具卡正文的公用零件：输出面板（头几行 + 展开全部 + 悬停复制）、高亮代码、
// 可点的文件行、状态徽标、键值表。各工具的画法（renderers.tsx）只拼这些。

import { useContext, useEffect, useState, type CSSProperties, type ReactNode } from "react";
import { Check, ChevronDown, Copy, Folder, Image as ImageIcon } from "lucide-react";
import { dock } from "../../dock/client";
import { ImageViewer } from "../ImageViewer";
import { usePrefs } from "../../dock/prefs";
import { FOLDER_COLOR, fileType } from "../fileType";
import { isDark, Tokens } from "../markdown/CodeBlock";
import { useHighlight } from "../markdown/highlight";
import { PathLinkContext } from "../markdown/pathLink";

/** 收起时最多露这么多行。 */
export const PEEK_LINES = 12;

/** 输出面板：圆角浅框；`copy` 给了就在右上角悬停出复制。 */
export function Panel({
  children,
  copy,
  tone = "plain",
  className = "",
}: {
  children: ReactNode;
  copy?: string;
  tone?: "plain" | "terminal" | "danger" | "muted";
  className?: string;
}) {
  const bg =
    tone === "terminal"
      ? "border-line bg-[#f6f6f8]"
      : tone === "danger"
        ? "border-[#fecaca] bg-[#fef6f6]"
        : tone === "muted"
          ? "border-line-strong bg-fill"
          : "border-line bg-white";
  // 玻璃材质下普通面板是会话里的一块玻璃（styles.css md-surface）；红色的报错面板保持原样。
  const glass = tone === "danger" ? "" : "md-surface";
  return (
    <div className={`${glass} group/panel relative min-w-0 overflow-hidden rounded-lg border ${bg} ${className}`}>
      {copy !== undefined && copy.trim() && <CopyCorner text={copy} />}
      {children}
    </div>
  );
}

function CopyCorner({ text }: { text: string }) {
  const [done, setDone] = useState(false);
  return (
    <button
      type="button"
      title="复制"
      aria-label="复制"
      onClick={(e) => {
        e.stopPropagation();
        void navigator.clipboard.writeText(text).then(() => {
          setDone(true);
          window.setTimeout(() => setDone(false), 1200);
        });
      }}
      className="motion-colors absolute right-1.5 top-1.5 z-[2] flex size-6 items-center justify-center rounded-md border border-line bg-white/90 text-ink-4 opacity-0 shadow-sm backdrop-blur hover:text-ink-2 group-hover/panel:opacity-100"
    >
      {done ? <Check size={12} className="text-[#34c759]" /> : <Copy size={12} />}
    </button>
  );
}

/** 「展开全部 N 行 / 收起」条。 */
export function MoreBar({ hidden, open, onToggle, unit = "行" }: { hidden: number; open: boolean; onToggle: () => void; unit?: string }) {
  return (
    <button
      type="button"
      onClick={onToggle}
      className="md-surface-head motion-colors flex w-full items-center justify-center gap-1 border-t border-line-soft bg-[#fafafb] py-1 text-[11px] text-ink-4 hover:bg-hover hover:text-ink-2"
    >
      <ChevronDown size={12} className={`transition-transform duration-[var(--dur-base)] ${open ? "rotate-180" : ""}`} />
      {open ? "收起" : `展开全部（还有 ${hidden} ${unit}）`}
    </button>
  );
}

/** 只露前 PEEK_LINES 个，其余点开。 */
export function usePeek<T>(rows: T[], peek = PEEK_LINES) {
  const [open, setOpen] = useState(false);
  const long = rows.length > peek + 2;
  return {
    shown: long && !open ? rows.slice(0, peek) : rows,
    bar: long ? <MoreBar hidden={rows.length - peek} open={open} onToggle={() => setOpen((v) => !v)} /> : null,
  };
}

/**
 * 高亮代码 + 行号。`numbers` 是每行的行号（read_file 的输出不一定从 1 开始）；不给就从 1 数。
 * 太长的先露 PEEK_LINES 行。
 */
export function CodeView({ code, lang, numbers }: { code: string; lang: string; numbers?: number[] }) {
  const { codeTheme } = usePrefs();
  const hl = useHighlight(lang === "text" ? "" : code, lang, codeTheme);
  const lines = code.split("\n");
  const tokens = hl && hl.code === code ? hl.lines : null;
  const rows = lines.map((text, i) => ({ text, n: numbers?.[i] ?? i + 1, tokens: tokens?.[i] ?? null }));
  const { shown, bar } = usePeek(rows);
  return (
    <Panel copy={code}>
      {/* 底色走 --code-bg（同 CodeBlock）：玻璃材质下 styles.css 调半透，行号栏也跟着透、不再钉住。 */}
      <div
        className="md-code thin-scroll overflow-x-auto py-1.5 font-mono text-[11.5px] leading-[1.6]"
        style={{ "--code-bg": hl?.bg || "#fff" } as CSSProperties}
        data-dark={isDark(hl?.bg || "") || undefined}
      >
        {shown.map((r, i) => (
          <div key={i} className="flex min-w-max">
            <span className="md-code-gutter sticky left-0 w-10 shrink-0 select-none pr-3 text-right text-[10.5px] text-ink-5" style={{ background: "var(--code-bg)" }}>
              {r.n}
            </span>
            <span className="whitespace-pre pr-4 text-ink-2">{r.tokens ? <Tokens lines={[r.tokens]} tail="" /> : r.text || " "}</span>
          </div>
        ))}
      </div>
      {bar}
    </Panel>
  );
}

/** 文件 / 目录一行：类型图标 + 名字；会话目录里的文件能点开（文件面板）。 */
export function FileRow({ path, label, dir = false, extra }: { path: string; label?: string; dir?: boolean; extra?: ReactNode }) {
  const linker = useContext(PathLinkContext);
  // `a.rs:12` 这种带行号的：图标按文件名认。
  const name = path.replace(/\/$/, "").replace(/:\d+(?::\d+)?$/, "").split("/").pop() ?? path;
  const t = fileType(name);
  const Icon = dir ? Folder : t.icon;
  const target = !dir && linker ? linker.resolve(path) : null;
  const text = label ?? path;
  const slash = text.lastIndexOf("/", text.length - 2);
  const body = (
    <>
      <Icon size={13} className="shrink-0" style={{ color: dir ? FOLDER_COLOR : t.color }} fill={dir ? `${FOLDER_COLOR}26` : "none"} />
      <span className="min-w-0 truncate font-mono text-[11.5px]">
        {slash > 0 && <span className="text-ink-4">{text.slice(0, slash + 1)}</span>}
        <span className={dir ? "text-ink-2" : "text-ink"}>{slash > 0 ? text.slice(slash + 1) : text}</span>
      </span>
      {extra}
    </>
  );
  if (!target || !linker) return <div className="flex h-6 min-w-0 items-center gap-2 px-2.5">{body}</div>;
  return (
    <button
      type="button"
      title="在文件面板中打开"
      onClick={() => linker.open(target)}
      className="motion-colors flex h-6 min-w-0 items-center gap-2 rounded-md px-2.5 text-left hover:bg-[#eef5ff]"
    >
      {body}
    </button>
  );
}

export type BadgeTone = "green" | "blue" | "red" | "amber" | "gray" | "purple";

const BADGE: Record<BadgeTone, string> = {
  green: "bg-[#e8f7ee] text-[#1a7f37]",
  blue: "bg-[#eaf3ff] text-[#0a6fd6]",
  red: "bg-[#fee2e2] text-[#b91c1c]",
  amber: "bg-[#fff4dc] text-[#8a5a00]",
  gray: "bg-fill text-ink-3",
  purple: "bg-[#f3ecff] text-[#7c3aed]",
};

export function Badge({ tone, children, mono = false }: { tone: BadgeTone; children: ReactNode; mono?: boolean }) {
  return (
    <span className={`inline-flex h-[18px] shrink-0 items-center rounded-[5px] px-1.5 text-[10.5px] font-medium ${BADGE[tone]} ${mono ? "font-mono" : ""}`}>
      {children}
    </span>
  );
}

/** 一列键值：参数表、任务属性。值是对象就压成一行 JSON。 */
export function KeyValues({ rows }: { rows: [string, unknown][] }) {
  if (rows.length === 0) return null;
  return (
    <div className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 px-3 py-2 text-[11.5px]">
      {rows.map(([k, v]) => (
        <div key={k} className="contents">
          <span className="font-mono text-ink-4">{k}</span>
          <ValueText text={typeof v === "string" ? v : JSON.stringify(v)} />
        </div>
      ))}
    </div>
  );
}

/** 键值表里一个值最多露这么多行 / 字（整段代码当参数时）。 */
const VALUE_LINES = 6;
const VALUE_CHARS = 400;

function ValueText({ text }: { text: string }) {
  const [open, setOpen] = useState(false);
  const lines = text.split("\n");
  const long = lines.length > VALUE_LINES + 2 || text.length > VALUE_CHARS + 100;
  const shown = !long || open ? text : lines.slice(0, VALUE_LINES).join("\n").slice(0, VALUE_CHARS) + " …";
  return (
    <span className="min-w-0 whitespace-pre-wrap break-words font-mono text-ink-2">
      {shown}
      {long && (
        <button
          type="button"
          onClick={() => setOpen((o) => !o)}
          className="motion-colors ml-1.5 rounded px-1 font-sans text-[10.5px] text-ink-4 hover:bg-hover hover:text-ink-2"
        >
          {open ? "收起" : lines.length > VALUE_LINES + 2 ? `展开（共 ${lines.length} 行）` : `展开（共 ${text.length} 字）`}
        </button>
      )}
    </span>
  );
}

/** 去掉 ANSI 颜色码（终端输出里常有）。 */
export function stripAnsi(s: string): string {
  return s.replace(/\u001b\[[0-9;?]*[ -/]*[@-~]/g, "");
}

// ---- 工具结果里的图（截图等） ----

const imageCache = new Map<string, string>();

/** 工具结果里的图：展开时才按 `item/image` 取像素，点开看大图。取不到就画尺寸占位。 */
export function ToolImages({
  threadId,
  itemId,
  images,
}: {
  threadId: string;
  itemId: string;
  images: { index: number; width: number; height: number; mimeType: string }[];
}) {
  return (
    <div className="flex flex-wrap gap-2">
      {images.map((img) => (
        <ToolImage key={img.index} threadId={threadId} itemId={itemId} image={img} />
      ))}
    </div>
  );
}

function ToolImage({
  threadId,
  itemId,
  image,
}: {
  threadId: string;
  itemId: string;
  image: { index: number; width: number; height: number; mimeType: string };
}) {
  const key = `${threadId}\u0000${itemId}\u0000${image.index}`;
  const [src, setSrc] = useState<string | null>(() => imageCache.get(key) ?? null);
  const [failed, setFailed] = useState(false);
  const [viewing, setViewing] = useState(false);
  useEffect(() => {
    if (src) return;
    let alive = true;
    dock
      .call<{ mimeType: string; data: string }>("item/image", { threadId, itemId, index: image.index })
      .then((r) => {
        const url = `data:${r.mimeType};base64,${r.data}`;
        imageCache.set(key, url);
        if (alive) setSrc(url);
      })
      .catch(() => alive && setFailed(true));
    return () => {
      alive = false;
    };
  }, [key, src, threadId, itemId, image.index]);
  const caption = `${image.width}×${image.height}`;
  // 缩略图按原图比例，最宽 320、最高 200。
  const scale = Math.min(320 / image.width, 200 / image.height, 1);
  const box = { width: Math.round(image.width * scale), height: Math.round(image.height * scale) };
  return (
    <>
      <button
        type="button"
        title={src ? `查看大图 · ${caption}` : caption}
        disabled={!src}
        onClick={() => setViewing(true)}
        style={box}
        className="motion-colors relative flex shrink-0 items-center justify-center overflow-hidden rounded-lg border border-line-strong bg-fill shadow-sm enabled:hover:shadow-md"
      >
        {src ? (
          <img src={src} alt="" className="enter-fade size-full object-cover" />
        ) : (
          <span className="flex flex-col items-center gap-1 text-ink-4">
            <ImageIcon size={18} strokeWidth={1.5} />
            <span className="text-[10.5px]">{failed ? `${caption} · 取不到原图` : caption}</span>
          </span>
        )}
      </button>
      {viewing && src && <ImageViewer url={src} caption={caption} onClose={() => setViewing(false)} />}
    </>
  );
}
