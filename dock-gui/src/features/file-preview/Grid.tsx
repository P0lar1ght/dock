// 虚拟滚动的表格：只画看得见的行和列（上下左右各多画几格），几十万行也只有几百个格子。
// 表头和行号粘在边上。列宽按样本估，加载中行数每翻一倍补估一次、只变宽。
// 行是外面的数组（边加载边往里推），`version` 变了才重画。
//
// 给了 `edit` 就能改：点格子选中，方向键 / Tab 移动，回车 / F2 / 双击 / 直接打字开始改，
// Delete 清空，⌘Z / ⇧⌘Z 撤销重做，⌘C 复制。右下多出一行一列空的，往里填就是加数据。
// 输入框是单行的 textarea：⌥↩ 换行（CSV 的格子里可以有换行）。
//
// 选中的格子上一直盖着一个透明的 textarea 并拿着焦点：直接打字（包括中文输入法的组字）
// 落在它上面，一有字就变成可见的输入框接着改。div 拿焦点时输入法组不了字。

import {
  memo,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type MouseEvent as ReactMouseEvent,
  type ReactNode,
} from "react";
import { imeComposing } from "../../dock/ime";
import type { Merge } from "./xlsx.worker";

const ROW_H = 26;
const HEAD_H = 28;
const OVERSCAN_ROWS = 8;
const OVERSCAN_COLS = 2;
/** 估列宽：开头看这么多行，全表再均匀取这么多行。 */
const WIDTH_SAMPLE = 300;
const MIN_W = 56;
const MAX_W = 320;
/** 数、金额、百分比、日期、时间。 */
const NUMERIC_TEXT = /^[-+(]?[$¥€£]?[\d,.:/ -]*\d[%)]?$/;

export interface GridEdit {
  /** 这一格能不能改（公式格不能）。`ri` 是 `rows` 的下标。 */
  canEdit: (ri: number, c: number) => boolean;
  /** 改好一格（`ri` 可能等于 `rows.length`：在末尾那行空行里填）。 */
  onCommit: (ri: number, c: number, text: string) => void;
  /** 改过、还没存的格子：`${ri}:${c}`。 */
  changed: Set<string>;
  onUndo: () => void;
  onRedo: () => void;
  /** 右键：在格子 / 行号上给行，在表头上给列。 */
  onMenu?: (e: ReactMouseEvent, at: { ri?: number; c?: number }) => void;
  /** 想改不能改的格子：告诉人为什么。 */
  onBlocked?: (ri: number, c: number) => void;
}

export interface GridProps {
  rows: string[][];
  /** 外面往 `rows` 里推了新行就加一。 */
  version: number;
  /** 数据从 `rows` 的第几行开始（CSV 第 0 行是表头）。 */
  start: number;
  cols: number;
  /** 表头：文字（CSV 第一行），或者 Excel 那样的列字母。 */
  header: string[] | "letters";
  numeric: boolean[];
  /** 合并单元格（行号按 `rows` 的下标）。 */
  merges?: Merge[];
  edit?: GridEdit | null;
}

interface Cell {
  ri: number;
  c: number;
}

export const Grid = memo(function Grid({ rows, version, start, cols: dataCols, header, numeric, merges, edit }: GridProps) {
  const ref = useRef<HTMLDivElement>(null);
  const [view, setView] = useState({ top: 0, left: 0, w: 0, h: 0 });
  const [sel, setSel] = useState<Cell | null>(null);
  const [editing, setEditingState] = useState<(Cell & { text: string }) | null>(null);
  // 收起输入框会让它失焦、触发 onBlur：拿 ref 判断还在不在改，Esc 取消的不会又被提交。
  const editingRef = useRef(editing);
  const setEditing = (next: (Cell & { text: string }) | null) => {
    editingRef.current = next;
    setEditingState(next);
  };

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    let frame = 0;
    const read = () => {
      frame = 0;
      setView({ top: el.scrollTop, left: el.scrollLeft, w: el.clientWidth, h: el.clientHeight });
    };
    const onScroll = () => {
      if (!frame) frame = requestAnimationFrame(read);
    };
    read();
    const ro = new ResizeObserver(read);
    ro.observe(el);
    el.addEventListener("scroll", onScroll, { passive: true });
    return () => {
      ro.disconnect();
      el.removeEventListener("scroll", onScroll);
      if (frame) cancelAnimationFrame(frame);
    };
  }, []);

  // 退出编辑：选中和输入框都收起。
  useEffect(() => {
    if (!edit) {
      setSel(null);
      setEditing(null);
    }
  }, [!edit]);

  const dataRows = Math.max(0, rows.length - start);
  // 编辑时右下多一行一列空的：往里填就是加数据。
  const rowCount = dataRows + (edit ? 1 : 0);
  const cols = dataCols + (edit ? 1 : 0);
  // 行数每翻一倍重估一次列宽（样本均匀取在全表 + 末尾），几十万行也只估二十来次；
  // 只变宽不变窄，滚着滚着列不会突然缩回去。
  const scale = dataRows <= WIDTH_SAMPLE ? dataRows : WIDTH_SAMPLE + Math.floor(Math.log2(dataRows));
  const widthsRef = useRef<number[]>([]);
  const widths = useMemo(() => {
    const next = columnWidths(rows, start, cols, header);
    const prev = widthsRef.current;
    const out = next.map((w, c) => Math.max(w, prev[c] ?? 0));
    widthsRef.current = out;
    return out;
  }, [rows, start, scale, cols, header]);
  const xs = useMemo(() => {
    const out = new Array<number>(cols + 1);
    out[0] = 0;
    for (let c = 0; c < cols; c++) out[c + 1] = out[c] + widths[c];
    return out;
  }, [widths, cols]);

  const numW = Math.max(44, String(rowCount).length * 8 + 20);
  const totalW = numW + xs[cols];
  const totalH = HEAD_H + rowCount * ROW_H;

  const r0 = Math.max(0, Math.floor((view.top - HEAD_H) / ROW_H) - OVERSCAN_ROWS);
  const r1 = Math.min(rowCount, Math.ceil((view.top + view.h) / ROW_H) + OVERSCAN_ROWS);
  const c0 = Math.max(0, upper(xs, view.left) - 1 - OVERSCAN_COLS);
  const c1 = Math.min(cols, upper(xs, view.left + view.w - numW) + OVERSCAN_COLS);

  const visibleCols: number[] = [];
  for (let c = c0; c < c1; c++) visibleCols.push(c);
  const headText = (c: number) => (header === "letters" ? columnName(c) : (header[c] ?? ""));
  // 数字列里像数的格子靠右（Excel 那样按格子，不是整列：表头、备注这类文字照样靠左）。
  const align = (c: number, text?: string) => (numeric[c] && (text === undefined || NUMERIC_TEXT.test(text)) ? "justify-end text-right" : "");
  const textAt = (ri: number, c: number) => rows[ri]?.[c] ?? "";

  // ---- 编辑 ----

  /** 让一格露出来（被表头 / 行号挡住也算看不见）。 */
  const reveal = (ri: number, c: number) => {
    const el = ref.current;
    if (!el) return;
    const top = HEAD_H + (ri - start) * ROW_H;
    if (ri >= start) {
      if (top - HEAD_H < el.scrollTop) el.scrollTop = top - HEAD_H;
      else if (top + ROW_H > el.scrollTop + el.clientHeight) el.scrollTop = top + ROW_H - el.clientHeight;
    }
    const left = numW + xs[c];
    const right = left + widths[c];
    if (left - numW < el.scrollLeft) el.scrollLeft = left - numW;
    else if (right > el.scrollLeft + el.clientWidth) el.scrollLeft = right - el.clientWidth;
  };
  // CSV 的表头（`rows[0]`）也能改：选中能上到它。
  const minRi = header !== "letters" ? 0 : start;
  const input = useRef<HTMLTextAreaElement>(null);
  const select = (cell: Cell) => {
    const ri = Math.min(Math.max(cell.ri, minRi), start + rowCount - 1);
    const c = Math.min(Math.max(cell.c, 0), cols - 1);
    setSel({ ri, c });
    reveal(ri, c);
  };
  // 选中了就让盖在格子上的输入框拿焦点（打字、输入法都落在它上面）。
  useLayoutEffect(() => {
    if (sel && edit) input.current?.focus({ preventScroll: true });
  }, [sel?.ri, sel?.c, !!editing, !!edit]);
  const startEdit = (cell: Cell, text?: string) => {
    if (!edit) return;
    if (!edit.canEdit(cell.ri, cell.c)) {
      edit.onBlocked?.(cell.ri, cell.c);
      return;
    }
    setSel(cell);
    reveal(cell.ri, cell.c);
    setEditing({ ...cell, text: text ?? textAt(cell.ri, cell.c) });
  };
  const commit = (move?: { dr: number; dc: number }) => {
    const cur = editingRef.current;
    if (!cur || !edit) return;
    setEditing(null);
    if (cur.text !== textAt(cur.ri, cur.c)) edit.onCommit(cur.ri, cur.c, cur.text);
    if (move) select({ ri: cur.ri + move.dr, c: cur.c + move.dc });
  };
  const cancel = () => setEditing(null);

  const cellAt = (target: EventTarget): Cell | null => {
    const el = (target as HTMLElement).closest?.<HTMLElement>("[data-ri]");
    if (!el) return null;
    return { ri: Number(el.dataset.ri), c: Number(el.dataset.c ?? 0) };
  };

  /** 没在改时落在透明输入框上的键：移动、开始改、清空、撤销。打字不拦（交给 onChange）。 */
  const navKey = (e: ReactKeyboardEvent) => {
    if (!edit || !sel) return;
    const mod = e.metaKey || e.ctrlKey;
    const move = (dr: number, dc: number) => {
      e.preventDefault();
      select({ ri: sel.ri + dr, c: sel.c + dc });
    };
    if (mod && e.key.toLowerCase() === "z") {
      e.preventDefault();
      if (e.shiftKey) edit.onRedo();
      else edit.onUndo();
    } else if (mod && e.key.toLowerCase() === "c") {
      e.preventDefault();
      void navigator.clipboard.writeText(textAt(sel.ri, sel.c));
    } else if (e.key === "ArrowUp") move(-1, 0);
    else if (e.key === "ArrowDown") move(1, 0);
    else if (e.key === "ArrowLeft") move(0, -1);
    else if (e.key === "ArrowRight") move(0, 1);
    else if (e.key === "Tab") move(0, e.shiftKey ? -1 : 1);
    else if (e.key === "Enter" || e.key === "F2") {
      e.preventDefault();
      startEdit(sel);
    } else if (e.key === "Backspace" || e.key === "Delete") {
      e.preventDefault();
      if (!textAt(sel.ri, sel.c)) return;
      if (edit.canEdit(sel.ri, sel.c)) edit.onCommit(sel.ri, sel.c, "");
      else edit.onBlocked?.(sel.ri, sel.c);
    }
  };
  /** 透明输入框里来了字（直接打字、输入法开始组字）：从这些字开始改（像 Excel）。 */
  const typed = (text: string) => {
    if (!sel || editingRef.current) return;
    if (!edit?.canEdit(sel.ri, sel.c)) {
      edit?.onBlocked?.(sel.ri, sel.c);
      if (input.current) input.current.value = "";
      return;
    }
    setEditing({ ...sel, text });
  };

  const body: ReactNode[] = [];
  for (let r = r0; r < r1; r++) {
    const ri = start + r;
    const row = rows[ri];
    const phantom = ri >= rows.length;
    body.push(
      <div
        key={r}
        className={`group absolute left-0 ${r % 2 ? "bg-[#fafafb]" : "bg-white"} hover:bg-[#eef5ff]`}
        style={{ top: HEAD_H + r * ROW_H, height: ROW_H, width: totalW }}
      >
        <div
          data-ri={ri}
          data-rownum
          className={`sticky left-0 z-[1] flex h-full items-center justify-end border-b border-r border-line-soft pr-2 text-[11.5px] text-ink-5 ${
            r % 2 ? "bg-[#fafafb]" : "bg-white"
          } group-hover:bg-[#eef5ff]`}
          style={{ width: numW }}
        >
          {phantom ? "+" : r + 1}
        </div>
        {(row || phantom) &&
          visibleCols.map((c) => {
            const text = row?.[c];
            const dirty = edit?.changed.has(`${ri}:${c}`);
            return (
              <div
                key={c}
                data-ri={ri}
                data-c={c}
                className={`absolute top-0 flex h-full items-center border-b border-r border-line-soft px-2.5 text-ink ${align(c, text)} ${
                  dirty ? "bg-[#fff4cc]" : ""
                } ${phantom || c >= dataCols ? "bg-[#fcfcfd]" : ""}`}
                style={{ left: numW + xs[c], width: widths[c] }}
                title={text && text.length > 12 ? text : undefined}
              >
                <span className="truncate">{text}</span>
              </div>
            );
          })}
      </div>,
    );
  }

  const mergeCells: ReactNode[] = [];
  if (merges) {
    for (const m of merges) {
      const top = m.s.r - start;
      const bottom = m.e.r - start;
      if (bottom < r0 || top >= r1 || m.e.c < c0 || m.s.c >= c1 || m.s.c >= cols) continue;
      const right = Math.min(m.e.c + 1, cols);
      const text = rows[m.s.r]?.[m.s.c];
      mergeCells.push(
        <div
          key={`${m.s.r}:${m.s.c}`}
          data-ri={m.s.r}
          data-c={m.s.c}
          className={`absolute flex items-center border-b border-r border-line-soft bg-white px-2.5 text-ink ${
            numeric[m.s.c] ? "justify-end" : "justify-center"
          } ${edit?.changed.has(`${m.s.r}:${m.s.c}`) ? "bg-[#fff4cc]" : ""}`}
          style={{
            top: HEAD_H + top * ROW_H,
            left: numW + xs[m.s.c],
            width: xs[right] - xs[m.s.c],
            height: (bottom - top + 1) * ROW_H,
          }}
          title={text}
        >
          <span className="line-clamp-3 text-center">{text}</span>
        </div>,
      );
    }
  }

  // 选中框 / 输入框的位置（表头那一行在 CSV 里是 `ri = 0`，画在粘住的表头里）。
  const boxOf = (cell: Cell) => ({
    left: numW + xs[cell.c],
    width: widths[cell.c] ?? 120,
    top: cell.ri < start ? 0 : HEAD_H + (cell.ri - start) * ROW_H,
  });
  // 选中的格子上那个输入框：没在改时透明（只接键和输入法），改的时候显出来。
  const box = editing ?? sel;
  const editor =
    edit && box && box.c < cols ? (
      <textarea
        ref={input}
        // 盖在格子上：双击、右键落在它身上也要认得是哪一格。
        data-ri={box.ri}
        data-c={box.c}
        rows={1}
        spellCheck={false}
        value={editing ? editing.text : ""}
        onChange={(e) => (editingRef.current ? setEditing({ ...editingRef.current, text: e.target.value }) : typed(e.target.value))}
        onCompositionStart={() => typed("")}
        onBlur={() => commit()}
        onKeyDown={(e) => {
          if (!editingRef.current) return navKey(e);
          if (imeComposing(e)) return;
          // 改着就按 ⌘S：先把这一格交上去，再让宿主存（不拦，键接着冒泡）。
          if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "s") return commit();
          if (e.key === "Enter" && e.altKey) {
            // ⌥↩：格子里换行。
            e.preventDefault();
            const t = e.currentTarget;
            const cur = editingRef.current;
            setEditing({ ...cur, text: `${cur.text.slice(0, t.selectionStart)}\n${cur.text.slice(t.selectionEnd)}` });
          } else if (e.key === "Enter") {
            e.preventDefault();
            commit({ dr: e.shiftKey ? -1 : 1, dc: 0 });
          } else if (e.key === "Tab") {
            e.preventDefault();
            commit({ dr: 0, dc: e.shiftKey ? -1 : 1 });
          } else if (e.key === "Escape") {
            e.preventDefault();
            cancel();
          }
        }}
        className={`absolute resize-none overflow-hidden px-2 py-[3px] text-[12.5px] leading-[16px] outline-none ${
          editing
            ? "z-[4] border-2 border-accent bg-white text-ink shadow-[0px_4px_14px_rgba(0,0,0,0.12)]"
            : "z-[2] cursor-cell border-2 border-accent bg-transparent text-transparent caret-transparent"
        }`}
        style={{ ...boxOf(box), height: box.ri < start ? HEAD_H : ROW_H, minWidth: editing ? 140 : undefined }}
      />
    ) : null;

  return (
    <div
      ref={ref}
      onMouseDown={(e) => {
        if (!edit || e.button !== 0 || e.target === input.current) return;
        const cell = cellAt(e.target);
        if (!cell || (e.target as HTMLElement).closest("[data-rownum]")) return;
        // 别让被点的 div 抢走焦点：焦点留在格子上的输入框。
        e.preventDefault();
        if (editingRef.current && (editingRef.current.ri !== cell.ri || editingRef.current.c !== cell.c)) commit();
        select(cell);
      }}
      onDoubleClick={(e) => {
        const cell = cellAt(e.target);
        if (cell && !(e.target as HTMLElement).closest("[data-rownum]")) startEdit(cell);
      }}
      onContextMenu={(e) => {
        if (!edit?.onMenu) return;
        const el = (e.target as HTMLElement).closest<HTMLElement>("[data-ri],[data-col]");
        if (!el) return;
        e.preventDefault();
        if (el.dataset.col !== undefined) edit.onMenu(e, { c: Number(el.dataset.col) });
        else {
          const ri = Number(el.dataset.ri);
          const c = el.dataset.c === undefined ? undefined : Number(el.dataset.c);
          if (c !== undefined) select({ ri, c });
          edit.onMenu(e, { ri, c });
        }
      }}
      className="glass-keep thin-scroll relative min-h-0 flex-1 overflow-auto bg-white text-[12.5px] tabular-nums outline-none"
      data-version={version}
    >
      <div className="relative" style={{ width: totalW, height: totalH }}>
        <div className="sticky top-0 z-[3] flex border-b border-line bg-[#f5f5f7]" style={{ width: totalW, height: HEAD_H }}>
          <div className="sticky left-0 z-[1] shrink-0 border-r border-line bg-[#f5f5f7]" style={{ width: numW, height: HEAD_H }} />
          {visibleCols.map((c) => (
            <div
              key={c}
              data-col={c}
              {...(header !== "letters" && edit ? { "data-ri": 0, "data-c": c } : {})}
              className={`absolute top-0 flex h-full items-center border-r border-line px-2.5 font-semibold text-ink-2 ${
                header === "letters" ? "justify-center font-medium text-ink-3" : align(c)
              } ${header !== "letters" && edit?.changed.has(`0:${c}`) ? "bg-[#fff4cc]" : ""}`}
              style={{ left: numW + xs[c], width: widths[c] }}
              title={headText(c)}
            >
              <span className="truncate">{headText(c)}</span>
            </div>
          ))}
          {box && box.ri < start && editor}
        </div>
        {body}
        {mergeCells}
        {box && box.ri >= start && editor}
      </div>
    </div>
  );
});

/** 第一个大于 `x` 的下标。 */
function upper(xs: number[], x: number): number {
  let lo = 0;
  let hi = xs.length;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (xs[mid] <= x) lo = mid + 1;
    else hi = mid;
  }
  return lo;
}

/** 0 → A，25 → Z，26 → AA。 */
export function columnName(c: number): string {
  let s = "";
  for (let n = c + 1; n > 0; n = Math.floor((n - 1) / 26)) s = String.fromCharCode(65 + ((n - 1) % 26)) + s;
  return s;
}

/** 按文字估宽：中日韩字符算两个。 */
function textWidth(text: string): number {
  let w = 0;
  for (let i = 0; i < text.length && i < 64; i++) w += text.charCodeAt(i) > 0x2e80 ? 13 : 7.6;
  return w;
}

/** 估列宽的样本：前 WIDTH_SAMPLE 行、全表均匀取的 WIDTH_SAMPLE 行、最后 100 行。 */
function sampleRows(start: number, end: number): number[] {
  const out: number[] = [];
  const n = end - start;
  for (let r = start; r < Math.min(end, start + WIDTH_SAMPLE); r++) out.push(r);
  if (n > WIDTH_SAMPLE) {
    const step = n / WIDTH_SAMPLE;
    for (let i = 0; i < WIDTH_SAMPLE; i++) out.push(start + Math.floor(i * step));
    for (let r = Math.max(start, end - 100); r < end; r++) out.push(r);
  }
  return out;
}

function columnWidths(rows: string[][], start: number, cols: number, header: string[] | "letters"): number[] {
  const out = new Array<number>(cols).fill(0);
  if (header !== "letters") for (let c = 0; c < cols; c++) out[c] = textWidth(header[c] ?? "") + 8;
  for (const r of sampleRows(start, rows.length)) {
    const row = rows[r];
    if (!row) continue;
    for (let c = 0; c < Math.min(cols, row.length); c++) {
      const w = textWidth(row[c]);
      if (w > out[c]) out[c] = w;
    }
  }
  const min = header === "letters" ? 72 : MIN_W;
  return out.map((w) => Math.min(MAX_W, Math.max(min, Math.ceil(w + 28))));
}
