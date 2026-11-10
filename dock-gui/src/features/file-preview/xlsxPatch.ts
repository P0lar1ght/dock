// 改 xlsx 里的单元格：只动被改的那几格的 XML，别的原样留着。
//
// 不用 SheetJS 整份重写：社区版写出来会丢样式、图表、图片、批注、条件格式。这里在原来的
// zip 里只换掉对应的工作表 XML（其余条目字节不变），格子保留原来的样式（`s`），字符串
// 直接写成内联（不碰 sharedStrings），工作表的范围（dimension）跟着放大。
// 有公式的工作簿设 fullCalcOnLoad：Excel 打开时重算，公式不会拿着旧值。
//
// 纯字符串处理，不依赖 DOM（Worker 里没有 DOMParser）；zip 的读写由调用方做（xlsx.worker.ts）。

export type CellValue = { kind: "empty" } | { kind: "number"; n: number } | { kind: "string"; s: string };

export interface CellEdit {
  /** 从 0 起，对着 A1。 */
  r: number;
  c: number;
  value: CellValue;
}

export class PatchError extends Error {}

/** 0 → A。 */
export function colName(c: number): string {
  let s = "";
  for (let n = c + 1; n > 0; n = Math.floor((n - 1) / 26)) s = String.fromCharCode(65 + ((n - 1) % 26)) + s;
  return s;
}

/** A → 0。 */
export function colIndex(name: string): number {
  let n = 0;
  for (const ch of name) n = n * 26 + (ch.charCodeAt(0) - 64);
  return n - 1;
}

const esc = (s: string) =>
  s
    // XML 1.0 不认的控制字符直接去掉。
    .replace(/[\u0000-\u0008\u000b\u000c\u000e-\u001f￾￿]/g, "")
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");

/** 第 `index` 张工作表在 zip 里的路径（照 workbook.xml 的顺序和它的关系表）。 */
export function sheetPath(workbookXml: string, relsXml: string, index: number): string {
  const sheets = [...workbookXml.matchAll(/<(?:\w+:)?sheet\b[^>]*>/g)].map((m) => m[0]);
  const tag = sheets[index];
  if (!tag) throw new PatchError(`没有第 ${index + 1} 张工作表`);
  const rid = /\br:id="([^"]+)"/.exec(tag)?.[1] ?? /\b\w+:id="([^"]+)"/.exec(tag)?.[1];
  if (!rid) throw new PatchError("工作表缺关系 id");
  const rel = [...relsXml.matchAll(/<(?:\w+:)?Relationship\b[^>]*>/g)].map((m) => m[0]).find((t) => t.includes(`Id="${rid}"`));
  const target = rel && /\bTarget="([^"]+)"/.exec(rel)?.[1];
  if (!target) throw new PatchError("找不到工作表文件");
  return target.startsWith("/") ? target.slice(1) : `xl/${target.replace(/^\.\//, "")}`;
}

/** 一格的 XML：带上原来的样式（和别的属性，`t` 除外）。 */
function cellXml(p: string, ref: string, attrs: string, value: CellValue): string {
  const keep = attrs
    .replace(/\s+r="[^"]*"/, "")
    .replace(/\s+t="[^"]*"/, "")
    .replace(/\s*\/$/, "");
  const head = `<${p}c r="${ref}"${keep}`;
  if (value.kind === "empty") return `${head}/>`;
  if (value.kind === "number") return `${head}><${p}v>${value.n}</${p}v></${p}c>`;
  const space = /^\s|\s$|\n/.test(value.s) ? ' xml:space="preserve"' : "";
  return `${head} t="inlineStr"><${p}is><${p}t${space}>${esc(value.s)}</${p}t></${p}is></${p}c>`;
}

interface Span {
  start: number;
  end: number;
}

/** 改一张工作表的 XML。行 / 格都得带 `r`（Excel 写的都带）；没带的不认，免得写错位置。 */
export function patchSheetXml(xml: string, edits: CellEdit[]): string {
  const open = /<(\w+:)?sheetData\b[^>]*?(\/?)>/.exec(xml);
  if (!open) throw new PatchError("工作表里没有 sheetData");
  const p = open[1] ?? "";
  let out = xml;
  // <sheetData/> 展开成一对。
  if (open[2] === "/") out = out.slice(0, open.index) + `<${p}sheetData></${p}sheetData>` + out.slice(open.index + open[0].length);

  const byRow = new Map<number, CellEdit[]>();
  for (const e of edits) {
    const list = byRow.get(e.r) ?? [];
    // 同一格改了几次：留最后一次。
    const at = list.findIndex((x) => x.c === e.c);
    if (at >= 0) list[at] = e;
    else list.push(e);
    byRow.set(e.r, list);
  }

  for (const [r, cells] of byRow) out = patchRow(out, p, r, cells);
  return growDimension(out, edits);
}

function patchRow(xml: string, p: string, r: number, edits: CellEdit[]): string {
  const dataStart = xml.search(new RegExp(`<${p}sheetData\\b`));
  const dataEnd = xml.indexOf(`</${p}sheetData>`, dataStart);
  const rowRe = new RegExp(`<${p}row\\b([^>]*?)(/?)>`, "g");
  rowRe.lastIndex = dataStart;
  let insertAt = dataEnd;
  let found: { span: Span; attrs: string; selfClosing: boolean; inner: Span } | null = null;
  for (let m = rowRe.exec(xml); m && m.index < dataEnd; m = rowRe.exec(xml)) {
    const n = /\br="(\d+)"/.exec(m[1]);
    if (!n) throw new PatchError("这个表格的行没写行号，面板里改不了");
    const num = Number(n[1]);
    if (num === r + 1) {
      const selfClosing = m[2] === "/";
      const innerStart = m.index + m[0].length;
      const close = selfClosing ? innerStart : xml.indexOf(`</${p}row>`, innerStart);
      found = {
        span: { start: m.index, end: selfClosing ? innerStart : close + `</${p}row>`.length },
        attrs: m[1],
        selfClosing,
        inner: { start: innerStart, end: close },
      };
      break;
    }
    if (num > r + 1) {
      insertAt = m.index;
      break;
    }
  }
  if (!found) {
    const cells = edits
      .slice()
      .sort((a, b) => a.c - b.c)
      .filter((e) => e.value.kind !== "empty")
      .map((e) => cellXml(p, `${colName(e.c)}${r + 1}`, "", e.value))
      .join("");
    if (!cells) return xml;
    return `${xml.slice(0, insertAt)}<${p}row r="${r + 1}">${cells}</${p}row>${xml.slice(insertAt)}`;
  }
  let inner = found.selfClosing ? "" : xml.slice(found.inner.start, found.inner.end);
  for (const e of edits) inner = patchCell(inner, p, r, e);
  // 行上的 spans 只是提示，改了列之后可能不准：去掉（Excel 不要求）。
  const attrs = found.attrs.replace(/\s+spans="[^"]*"/, "");
  return `${xml.slice(0, found.span.start)}<${p}row${attrs}>${inner}</${p}row>${xml.slice(found.span.end)}`;
}

function patchCell(row: string, p: string, r: number, e: CellEdit): string {
  const ref = `${colName(e.c)}${r + 1}`;
  const cellRe = new RegExp(`<${p}c\\b([^>]*?)(/>|>[\\s\\S]*?</${p}c>)`, "g");
  for (let m = cellRe.exec(row); m; m = cellRe.exec(row)) {
    const cr = /\br="([A-Z]+)(\d+)"/.exec(m[1]);
    if (!cr) throw new PatchError("这个表格的单元格没写位置，面板里改不了");
    const c = colIndex(cr[1]);
    if (c === e.c) {
      if (/<(\w+:)?f\b/.test(m[2])) throw new PatchError(`${ref} 是公式，面板里不能改`);
      return row.slice(0, m.index) + cellXml(p, ref, m[1], e.value) + row.slice(m.index + m[0].length);
    }
    if (c > e.c) {
      if (e.value.kind === "empty") return row;
      return row.slice(0, m.index) + cellXml(p, ref, "", e.value) + row.slice(m.index);
    }
  }
  return e.value.kind === "empty" ? row : row + cellXml(p, ref, "", e.value);
}

/** 工作表的范围（`<dimension ref="A1:J20"/>`）放大到盖住改过的格子。 */
function growDimension(xml: string, edits: CellEdit[]): string {
  const m = /<(\w+:)?dimension\b[^>]*\bref="([A-Z]+)(\d+)(?::([A-Z]+)(\d+))?"/.exec(xml);
  if (!m || edits.length === 0) return xml;
  let c0 = colIndex(m[2]);
  let r0 = Number(m[3]) - 1;
  let c1 = m[4] ? colIndex(m[4]) : c0;
  let r1 = m[5] ? Number(m[5]) - 1 : r0;
  for (const e of edits) {
    if (e.value.kind === "empty") continue;
    c0 = Math.min(c0, e.c);
    r0 = Math.min(r0, e.r);
    c1 = Math.max(c1, e.c);
    r1 = Math.max(r1, e.r);
  }
  const ref = `${colName(c0)}${r0 + 1}:${colName(c1)}${r1 + 1}`;
  return xml.replace(/(<(?:\w+:)?dimension\b[^>]*\bref=")[^"]*"/, `$1${ref}"`);
}

/** 让 Excel 打开时重算公式（改过的格子可能被公式引用）。calcPr 的位置有规定，按顺序插。 */
export function ensureFullCalc(workbookXml: string): string {
  const calc = /<(\w+:)?calcPr\b([^>]*?)(\/?)>/.exec(workbookXml);
  if (calc) {
    if (/\bfullCalcOnLoad="(1|true)"/.test(calc[2])) return workbookXml;
    const attrs = calc[2].replace(/\s+fullCalcOnLoad="[^"]*"/, "") + ' fullCalcOnLoad="1"';
    return (
      workbookXml.slice(0, calc.index) +
      `<${calc[1] ?? ""}calcPr${attrs}${calc[3]}>` +
      workbookXml.slice(calc.index + calc[0].length)
    );
  }
  const p = /<(\w+:)?workbook\b/.exec(workbookXml)?.[1] ?? "";
  // calcPr 排在 sheets / functionGroups / externalReferences / definedNames 之后。
  let at = -1;
  for (const tag of ["sheets", "functionGroups", "externalReferences", "definedNames"]) {
    const close = new RegExp(`</${p}${tag}>|<${p}${tag}\\b[^>]*/>`, "g");
    for (let m = close.exec(workbookXml); m; m = close.exec(workbookXml)) at = Math.max(at, m.index + m[0].length);
  }
  if (at < 0) throw new PatchError("工作簿里没有 sheets");
  return `${workbookXml.slice(0, at)}<${p}calcPr fullCalcOnLoad="1"/>${workbookXml.slice(at)}`;
}

/** 把输入的文字转成格子的值：数（含千分位、百分比、按原格式认的日期）、空、文字。 */
export function parseInput(
  text: string,
  hint: { date?: boolean; percent?: boolean; date1904?: boolean } = {},
): CellValue {
  const t = text.trim();
  if (t === "") return { kind: "empty" };
  if (hint.date) {
    const d = /^(\d{4})[-/.](\d{1,2})[-/.](\d{1,2})(?:[ T](\d{1,2}):(\d{2})(?::(\d{2}))?)?$/.exec(t);
    if (d) {
      const ms = Date.UTC(+d[1], +d[2] - 1, +d[3], +(d[4] ?? 0), +(d[5] ?? 0), +(d[6] ?? 0));
      const epoch = hint.date1904 ? Date.UTC(1904, 0, 1) : Date.UTC(1899, 11, 30);
      return { kind: "number", n: (ms - epoch) / 86_400_000 };
    }
  }
  const pct = /^([-+]?[\d,]*\.?\d+)%$/.exec(t);
  if (pct && hint.percent) return { kind: "number", n: Number(pct[1].replace(/,/g, "")) / 100 };
  if (/^[-+]?(?:\d{1,3}(?:,\d{3})+|\d+)?(?:\.\d+)?(?:[eE][-+]?\d+)?$/.test(t) && /\d/.test(t)) {
    const n = Number(t.replace(/,/g, ""));
    if (Number.isFinite(n)) return { kind: "number", n };
  }
  // 原样当文字：保留前后空格。
  return { kind: "string", s: text };
}
