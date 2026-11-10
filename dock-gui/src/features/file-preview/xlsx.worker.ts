// 表格文件（xlsx / xlsm / xls / ods）在这里解析：块直接写进预先开好的缓冲区，主线程不碰整份字节。
//
// 大文件先只解析前 QUICK_ROWS 行发回去（首屏），再解析整张表把后面的行分批补上。
// 实测 20 万行 × 10 列（30 MB）：只解析前 200 行约 0.5 秒，整张约 2.3 秒。
// 只解析正在看的那张表（`sheets` 选项），切表时再解析那一张。

import { CFB, read, set_cptable, SSF, utils, type CellObject, type WorkSheet } from "xlsx";
import * as cptable from "xlsx/dist/cpexcel.full.mjs";
import { decodeBase64 } from "./bytes";
import { parseInput, type CellEdit } from "./xlsxPatch";
import { applyEdits, type Cfb } from "./xlsxSave";

// 老 .xls 里的中文是按代码页存的（GBK 之类）。
set_cptable(cptable);

export interface Merge {
  s: { r: number; c: number };
  e: { r: number; c: number };
}

export type ToXlsxWorker =
  | { t: "begin"; size: number }
  | { t: "chunk"; offset: number; data: string }
  /** 字节齐了：解析第 `sheet` 张表。 */
  | { t: "sheet"; index: number }
  /** 把这些格子的改动写进 xlsx，回新的字节（还不算数，Dock 存好了再 `commit`）。 */
  | { t: "save"; id: number; edits: { sheet: number; r: number; c: number; text: string }[] }
  /** Dock 存好了：之后的改动在这份上面接着改。 */
  | { t: "commit"; id: number };

export type FromXlsxWorker =
  | { t: "book"; sheets: string[] }
  /** `from` 是这批第一行的行号（从 0 起，对着 A1）。 */
  | { t: "rows"; sheet: number; from: number; rows: string[][]; cols: number }
  /** 一张表解析完：总行数、列数、合并单元格、哪些列是数、哪些格是公式（`r * 16384 + c`）。 */
  | {
      t: "sheetDone";
      sheet: number;
      rows: number;
      cols: number;
      merges: Merge[];
      numeric: boolean[];
      formulas: number[];
    }
  | { t: "error"; sheet: number; message: string }
  | { t: "saved"; id: number; bytes: ArrayBuffer }
  | { t: "saveError"; id: number; message: string };

/** 比这大的文件先出首屏再补全。 */
const QUICK_BYTES = 1024 * 1024;
const QUICK_ROWS = 200;
const BATCH = 5000;

let buf: Uint8Array = new Uint8Array(0);
let sentBook = false;
/** 解析过的表（存的时候看格子的格式：日期、百分比）。 */
const parsed = new Map<number, WorkSheet>();
/** 改好、等 Dock 存好的那份。 */
let pending: { id: number; bytes: Uint8Array } | null = null;
let date1904 = false;

const post = (m: FromXlsxWorker) => (self as unknown as Worker).postMessage(m);

/** 表 → 每格显示的文字（照单元格格式，和 Excel 里看到的一样），从 A1 起对齐。 */
function toRows(ws: WorkSheet): { rows: string[][]; cols: number } {
  const data = (ws["!data"] ?? []) as (CellObject | undefined)[][];
  const ref = ws["!ref"] ? utils.decode_range(ws["!ref"]) : null;
  // 有的文件 !ref 写到 XFD1048576（整表设了格式）：以真有数据的为准。
  const nRows = Math.min(ref ? ref.e.r + 1 : 0, data.length);
  const rows: string[][] = new Array(nRows);
  let cols = 0;
  for (let r = 0; r < nRows; r++) {
    const src = data[r];
    if (!src) {
      rows[r] = [];
      continue;
    }
    const row = new Array<string>(src.length);
    let last = -1;
    for (let c = 0; c < src.length; c++) {
      const cell = src[c];
      const text = cell ? (cell.w ?? (cell.v == null ? "" : String(cell.v))) : "";
      row[c] = text;
      if (text) last = c;
    }
    row.length = last + 1;
    rows[r] = row;
    if (last + 1 > cols) cols = last + 1;
  }
  // 末尾的空行不要。
  let end = rows.length;
  while (end > 0 && rows[end - 1].length === 0) end--;
  rows.length = end;
  return { rows, cols };
}

function numericOf(ws: WorkSheet, cols: number): boolean[] {
  const data = (ws["!data"] ?? []) as (CellObject | undefined)[][];
  // 数的格子占多数就算数字列（表头、标题行是文字也不碍事）；画的时候再按格子判断。
  const nums = new Array<number>(cols).fill(0);
  const texts = new Array<number>(cols).fill(0);
  for (let r = 0; r < Math.min(data.length, 1000); r++) {
    const row = data[r];
    if (!row) continue;
    for (let c = 0; c < Math.min(row.length, cols); c++) {
      const cell = row[c];
      if (!cell || cell.v == null || cell.v === "") continue;
      if (cell.t === "n" || cell.t === "d") nums[c]++;
      else texts[c]++;
    }
  }
  return nums.map((n, c) => n > texts[c]);
}

/** 公式格的位置（`r * 16384 + c`）：面板里不让改。 */
function formulasOf(ws: WorkSheet): number[] {
  const data = (ws["!data"] ?? []) as (CellObject | undefined)[][];
  const out: number[] = [];
  for (let r = 0; r < data.length; r++) {
    const row = data[r];
    if (!row) continue;
    for (let c = 0; c < row.length; c++) if (row[c]?.f) out.push(r * 16384 + c);
  }
  return out;
}

/** 写进去之前把输入转成值：照原格子的格式认日期、百分比。 */
function toEdit(sheet: number, r: number, c: number, text: string): CellEdit {
  const cell = ((parsed.get(sheet)?.["!data"] ?? []) as (CellObject | undefined)[][])[r]?.[c];
  const fmt = typeof cell?.z === "string" ? cell.z : "";
  return {
    r,
    c,
    value: parseInput(text, { date: !!fmt && SSF.is_date(fmt), percent: fmt.includes("%"), date1904 }),
  };
}

function save(id: number, edits: { sheet: number; r: number; c: number; text: string }[]) {
  try {
    const bySheet = new Map<number, CellEdit[]>();
    for (const e of edits) {
      const list = bySheet.get(e.sheet) ?? [];
      list.push(toEdit(e.sheet, e.r, e.c, e.text));
      bySheet.set(e.sheet, list);
    }
    // 有公式就让 Excel 打开时重算（改的格子可能被引用）；看不出来就交给 applyEdits 看 calcChain。
    const knownFormulas = [...parsed.values()].some((ws) => formulasOf(ws).length > 0);
    const bytes = applyEdits(CFB as Cfb, buf, bySheet, knownFormulas || "auto");
    pending = { id, bytes };
    const copy = bytes.slice().buffer;
    (self as unknown as Worker).postMessage({ t: "saved", id, bytes: copy } satisfies FromXlsxWorker, [copy]);
  } catch (e) {
    post({ t: "saveError", id, message: e instanceof Error ? e.message : String(e) });
  }
}

function parse(index: number, sheetRows?: number) {
  // 默认日期格式（m/d/yy）换成 yyyy-mm-dd。
  const wb = read(buf, {
    type: "array",
    dense: true,
    cellNF: true,
    dateNF: "yyyy-mm-dd",
    sheets: index,
    ...(sheetRows ? { sheetRows } : {}),
  });
  date1904 = !!wb.Workbook?.WBProps?.date1904;
  if (!sentBook) {
    sentBook = true;
    post({ t: "book", sheets: wb.SheetNames });
  }
  const ws = wb.Sheets[wb.SheetNames[index]];
  return ws ?? null;
}

function sheet(index: number) {
  try {
    let sent = 0;
    if (buf.length > QUICK_BYTES) {
      const quick = parse(index, QUICK_ROWS);
      if (quick) {
        const { rows, cols } = toRows(quick);
        // 首屏只到 QUICK_ROWS 行；不满说明整张就这么多，下面整张解析还是要做（合并单元格、列类型）。
        post({ t: "rows", sheet: index, from: 0, rows, cols });
        sent = rows.length;
      }
    }
    const ws = parse(index);
    if (!ws) {
      post({ t: "sheetDone", sheet: index, rows: 0, cols: 0, merges: [], numeric: [], formulas: [] });
      return;
    }
    const { rows, cols } = toRows(ws);
    for (let i = sent; i < rows.length; i += BATCH)
      post({ t: "rows", sheet: index, from: i, rows: rows.slice(i, i + BATCH), cols });
    parsed.set(index, ws);
    const merges = ((ws["!merges"] ?? []) as Merge[]).map((m) => ({ s: m.s, e: m.e }));
    post({
      t: "sheetDone",
      sheet: index,
      rows: rows.length,
      cols,
      merges,
      numeric: numericOf(ws, cols),
      formulas: formulasOf(ws),
    });
  } catch (e) {
    post({ t: "error", sheet: index, message: e instanceof Error ? e.message : String(e) });
  }
}

self.onmessage = (e: MessageEvent<ToXlsxWorker>) => {
  const m = e.data;
  if (m.t === "begin") buf = new Uint8Array(m.size);
  else if (m.t === "chunk") buf.set(decodeBase64(m.data), m.offset);
  else if (m.t === "sheet") sheet(m.index);
  else if (m.t === "save") save(m.id, m.edits);
  else if (m.t === "commit" && pending?.id === m.id) {
    buf = pending.bytes;
    pending = null;
  }
};
