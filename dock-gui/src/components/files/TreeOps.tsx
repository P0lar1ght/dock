// 文件树上的改动：行内起名（新建 / 改名）、拖动移动、没保存的修改要不要保存。
// 拖动用 pointer 事件自己做，不用 HTML5 拖放：Tauri 的原生文件拖放会接管 WebView 的拖放事件。

import { useEffect, useRef, useState, type PointerEvent as ReactPointerEvent } from "react";
import { FileText, Folder } from "lucide-react";
import { imeComposing } from "../../dock/ime";
import { isWithin, parentOf } from "../../dock/fsEdit";

/** 树里的一行输入框：新建文件 / 文件夹、改名。回车提交，Esc 取消；失焦时有改动就提交。 */
export function NameInput({
  depth,
  kind,
  initial = "",
  error,
  onSubmit,
  onCancel,
}: {
  depth: number;
  kind: "file" | "dir";
  initial?: string;
  /** 上一次提交失败的原因（重名之类）：输入框留着让人改。 */
  error?: string | null;
  onSubmit: (name: string) => void;
  onCancel: () => void;
}) {
  const [value, setValue] = useState(initial);
  const ref = useRef<HTMLInputElement>(null);
  const done = useRef(false);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    el.focus();
    // 改名时先选中不带扩展名的那段（像访达）。
    const dot = initial.lastIndexOf(".");
    el.setSelectionRange(0, kind === "file" && dot > 0 ? dot : initial.length);
  }, []);
  const submit = () => {
    const name = value.trim();
    if (!name || name === initial) {
      done.current = true;
      onCancel();
    } else onSubmit(name);
  };
  const Icon = kind === "dir" ? Folder : FileText;
  return (
    <div className="flex flex-col" style={{ paddingLeft: 8 + depth * 16 }}>
      <div className="flex h-7 items-center gap-1.5 pr-2">
        <span className="size-3.5 shrink-0" />
        <Icon size={14} className="shrink-0 text-ink-4" />
        <input
          ref={ref}
          value={value}
          spellCheck={false}
          placeholder={kind === "dir" ? "文件夹名" : "文件名（可带路径，如 docs/a.md）"}
          onChange={(e) => setValue(e.target.value)}
          onBlur={() => !done.current && submit()}
          onKeyDown={(e) => {
            if (imeComposing(e)) return;
            if (e.key === "Enter") {
              e.preventDefault();
              submit();
            } else if (e.key === "Escape") {
              e.preventDefault();
              done.current = true;
              onCancel();
            }
          }}
          className={`h-6 min-w-0 flex-1 rounded-[5px] border bg-white px-1.5 text-[13px] text-ink outline-none placeholder:text-ink-5 ${
            error ? "border-[#ff3b30]" : "border-accent"
          }`}
        />
      </div>
      {error && <span className="enter-fade pb-1 pl-[42px] pr-2 text-[11px] leading-[1.4] text-[#ff3b30]">{error}</span>}
    </div>
  );
}

export interface DragState {
  /** 拖的是哪个路径。 */
  from: string;
  x: number;
  y: number;
  /** 指着的目标文件夹（"" 是根）；null = 这儿放不了。 */
  target: string | null;
}

/**
 * 拖动移动：行上按下、挪过 4 px 才算拖。指着文件夹就放进它，指着文件就放进那个文件的文件夹，
 * 指着空白处放到根。停在收起的文件夹上一会儿会展开它。拖过之后吞掉那一下点击（别顺手打开）。
 */
export function useTreeDrag({
  onDrop,
  onHoverDir,
}: {
  onDrop: (from: string, toDir: string) => void;
  /** 停在一个收起的文件夹上：展开它。 */
  onHoverDir: (dir: string) => void;
}) {
  const [drag, setDrag] = useState<DragState | null>(null);
  const latest = useRef({ onDrop, onHoverDir });
  latest.current = { onDrop, onHoverDir };
  const swallowClick = useRef(false);

  const start = (e: ReactPointerEvent, from: string) => {
    if (e.button !== 0) return;
    const x0 = e.clientX;
    const y0 = e.clientY;
    let active = false;
    let hoverTimer = 0;
    let hovered: string | null = null;
    const targetAt = (x: number, y: number): string | null => {
      const el = document.elementFromPoint(x, y) as HTMLElement | null;
      const row = el?.closest<HTMLElement>("[data-tree-dir]");
      const dir = row ? row.dataset.treeDir! : el?.closest("[data-tree-root]") ? "" : null;
      // 不能放进它自己或它下面，也不必放回原来的文件夹。
      if (dir === null || isWithin(dir, from) || dir === parentOf(from)) return null;
      return dir;
    };
    const move = (ev: PointerEvent) => {
      if (!active && Math.hypot(ev.clientX - x0, ev.clientY - y0) < 4) return;
      active = true;
      const target = targetAt(ev.clientX, ev.clientY);
      setDrag({ from, x: ev.clientX, y: ev.clientY, target });
      const over = (document.elementFromPoint(ev.clientX, ev.clientY) as HTMLElement | null)?.closest<HTMLElement>(
        "[data-tree-folder]",
      )?.dataset.treeFolder;
      if (over !== hovered) {
        hovered = over ?? null;
        window.clearTimeout(hoverTimer);
        if (over) hoverTimer = window.setTimeout(() => latest.current.onHoverDir(over), 600);
      }
    };
    const up = (ev: PointerEvent) => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      window.removeEventListener("keydown", key);
      window.clearTimeout(hoverTimer);
      if (!active) return;
      swallowClick.current = true;
      setTimeout(() => (swallowClick.current = false), 0);
      const target = targetAt(ev.clientX, ev.clientY);
      setDrag(null);
      if (target !== null) latest.current.onDrop(from, target);
    };
    const key = (ev: KeyboardEvent) => {
      if (ev.key !== "Escape") return;
      active = false;
      setDrag(null);
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      window.removeEventListener("keydown", key);
      window.clearTimeout(hoverTimer);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
    window.addEventListener("keydown", key);
  };

  /** 包住行的 onClick：刚拖完的那一下不算点击。 */
  const click = (run: () => void) => () => {
    if (!swallowClick.current) run();
  };
  return { drag, start, click };
}

/** 拖动时跟着指针的小标签。 */
export function DragGhost({ drag }: { drag: DragState }) {
  const name = drag.from.slice(drag.from.lastIndexOf("/") + 1);
  return (
    <div
      className="pointer-events-none fixed z-50 flex h-7 items-center gap-1.5 rounded-md border border-line-strong bg-white/95 px-2 text-[12.5px] text-ink shadow-lg"
      style={{ left: drag.x + 12, top: drag.y + 8 }}
    >
      {name}
      <span className="text-[11px] text-ink-4">
        {drag.target === null ? "放不了" : `→ ${drag.target === "" ? "根目录" : drag.target.slice(drag.target.lastIndexOf("/") + 1)}`}
      </span>
    </div>
  );
}

/** 有没保存的修改时，换文件 / 关面板 / 删它之前问一句。 */
export function UnsavedConfirm({
  name,
  onSave,
  onDiscard,
  onCancel,
}: {
  name: string;
  onSave: () => void;
  onDiscard: () => void;
  onCancel: () => void;
}) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        onCancel();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [onCancel]);
  return (
    <div className="enter-fade absolute inset-x-0 bottom-0 top-11 z-30 flex items-start justify-center bg-black/[0.06] pt-16" onMouseDown={onCancel}>
      <div
        className="glass-dialog pop-in flex w-[min(360px,calc(100%-32px))] flex-col gap-3 rounded-xl border border-line-strong bg-white p-4 shadow-[0px_16px_40px_rgba(0,0,0,0.14)]"
        onMouseDown={(e) => e.stopPropagation()}
      >
        <span className="text-[14px] font-semibold text-ink">「{name}」有没保存的修改</span>
        <span className="text-[13px] leading-[1.55] text-ink-3">不保存的话，这些修改会丢掉。</span>
        <div className="flex justify-end gap-2">
          <button type="button" onClick={onCancel} className="motion-colors h-7 rounded-[7px] px-3 text-xs text-ink-2 hover:bg-hover">
            取消
          </button>
          <button
            type="button"
            onClick={onDiscard}
            className="motion-colors h-7 rounded-[7px] border border-line-strong px-3 text-xs text-[#ff3b30] hover:bg-[#ff3b3010]"
          >
            不保存
          </button>
          <button
            type="button"
            autoFocus
            onClick={onSave}
            className="motion-colors h-7 rounded-[7px] bg-accent px-3 text-xs font-medium text-white hover:bg-[#0a74e0]"
          >
            保存
          </button>
        </div>
      </div>
    </div>
  );
}
