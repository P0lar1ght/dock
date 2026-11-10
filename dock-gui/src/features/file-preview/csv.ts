// CSV / TSV 增量解析：一块一块喂，状态跨块保留（引号里的换行、切在块边上的 `""` 和 `\r\n`）。
// 照 RFC 4180；不在引号里的单元格按切片取，不逐字符拼，几十 MB 也快。
// 主线程（小文件）和 csv.worker.ts（大文件）共用。

const QUOTE = 34;
const LF = 10;
const CR = 13;

export class DelimitedParser {
  private sep: number;
  private row: string[] = [];
  /** 当前单元格已经攒下的部分（跨块或带转义引号时）。 */
  private cell = "";
  private quoted = false;
  /** 引号里刚碰到一个 `"`：下一个字符是 `"` 就是转义，否则引号结束。 */
  private quoteEnd = false;
  /** 上一块以 `\r` 结尾：这一块开头的 `\n` 吃掉。 */
  private afterCR = false;
  private started = false;

  constructor(sep: string) {
    this.sep = sep.charCodeAt(0);
  }

  /** 喂一段文本，回这段里收完的行。 */
  push(text: string): string[][] {
    const out: string[][] = [];
    let s = text;
    if (!this.started) {
      this.started = true;
      if (s.charCodeAt(0) === 0xfeff) s = s.slice(1);
    }
    const n = s.length;
    let i = 0;
    if (this.afterCR) {
      this.afterCR = false;
      if (n > 0 && s.charCodeAt(0) === LF) i = 1;
    }
    let start = i;
    while (i < n) {
      const c = s.charCodeAt(i);
      if (this.quoted) {
        if (this.quoteEnd) {
          this.quoteEnd = false;
          if (c === QUOTE) {
            // `""`：一个引号，接着在引号里。
            this.cell += '"';
            start = i + 1;
            i++;
            continue;
          }
          this.quoted = false;
          start = i;
          continue; // 这个字符按引号外处理
        }
        if (c === QUOTE) {
          this.cell += s.slice(start, i);
          this.quoteEnd = true;
          start = i + 1;
        }
        i++;
        continue;
      }
      if (c === this.sep) {
        this.row.push(this.cell + s.slice(start, i));
        this.cell = "";
        start = i + 1;
      } else if (c === LF || c === CR) {
        this.row.push(this.cell + s.slice(start, i));
        this.cell = "";
        out.push(this.row);
        this.row = [];
        if (c === CR) {
          if (i + 1 < n) {
            if (s.charCodeAt(i + 1) === LF) i++;
          } else this.afterCR = true;
        }
        start = i + 1;
      } else if (c === QUOTE && start === i && this.cell === "") {
        this.quoted = true;
        start = i + 1;
      }
      i++;
    }
    this.cell += s.slice(start, n);
    return out;
  }

  /** 文件读完：收最后一行（没有换行结尾的）。 */
  end(): string[][] {
    this.quoted = false;
    this.quoteEnd = false;
    if (this.cell === "" && this.row.length === 0) return [];
    const last = [...this.row, this.cell];
    this.row = [];
    this.cell = "";
    return [last];
  }
}

/** 猜分隔符：头一行（引号外）逗号、制表符、分号、竖线谁多用谁；都没有就逗号。 */
export function sniffSeparator(text: string): string {
  const counts: Record<string, number> = { ",": 0, "\t": 0, ";": 0, "|": 0 };
  let quoted = false;
  for (let i = 0; i < text.length && i < 64 * 1024; i++) {
    const ch = text[i];
    if (ch === '"') quoted = !quoted;
    else if (!quoted && (ch === "\n" || ch === "\r")) break;
    else if (!quoted && ch in counts) counts[ch]++;
  }
  let best = ",";
  for (const sep of ["\t", ";", "|"]) if (counts[sep] > counts[best]) best = sep;
  return best;
}

/**
 * 按开头的字节认编码：有 BOM 照 BOM；不是合法 UTF-8 的当 GB18030（Excel 在中文系统上
 * 存的 CSV 多半是它）。
 */
export function sniffEncoding(head: Uint8Array): string {
  if (head[0] === 0xef && head[1] === 0xbb && head[2] === 0xbf) return "utf-8";
  if (head[0] === 0xff && head[1] === 0xfe) return "utf-16le";
  if (head[0] === 0xfe && head[1] === 0xff) return "utf-16be";
  try {
    new TextDecoder("utf-8", { fatal: true }).decode(head, { stream: true });
    return "utf-8";
  } catch {
    return "gb18030";
  }
}

/** 整列都是数（可带千分位、小数、%、负号）就右对齐。只看前面一些行。 */
export function numericColumns(rows: string[][], cols: number, sample = 1000): boolean[] {
  const seen = new Array<boolean>(cols).fill(false);
  const ok = new Array<boolean>(cols).fill(true);
  const n = Math.min(rows.length, sample);
  for (let r = 0; r < n; r++) {
    const row = rows[r];
    for (let c = 0; c < cols; c++) {
      if (!ok[c]) continue;
      const v = row[c]?.trim();
      if (!v) continue;
      seen[c] = true;
      if (!NUMBER.test(v)) ok[c] = false;
    }
  }
  return ok.map((v, c) => v && seen[c]);
}

const NUMBER = /^[-+]?[$¥€£]?[\d,]*\.?\d+(?:[eE][-+]?\d+)?%?$/;

/** 原文件的格式：存回去照着来（换行、BOM、末尾换行），只改内容。 */
export interface CsvFormat {
  sep: string;
  encoding: string;
  bom: boolean;
  eol: "\n" | "\r\n";
  trailingNewline: boolean;
}

/** 行 → CSV 文本。含分隔符、引号、换行的格子加引号，引号写成两个（RFC 4180）。 */
export function serializeDelimited(rows: string[][], fmt: Pick<CsvFormat, "sep" | "eol" | "trailingNewline">): string {
  const needs = new RegExp(`[${fmt.sep === "\t" ? "\\t" : fmt.sep.replace(/[\\^\]-]/g, "\\$&")}"\\r\\n]`);
  const cell = (v: string) => (needs.test(v) ? `"${v.replace(/"/g, '""')}"` : v);
  // 末尾整行空的（在空行里填了又删的）不写。
  let end = rows.length;
  while (end > 0 && rows[end - 1].every((v) => !v)) end--;
  const lines = new Array<string>(end);
  for (let i = 0; i < end; i++) lines[i] = rows[i].map(cell).join(fmt.sep);
  return lines.join(fmt.eol) + (fmt.trailingNewline && end > 0 ? fmt.eol : "");
}

/** 看一段文本的换行是 CRLF 还是 LF（没有换行就当 LF）。 */
export function sniffEol(text: string): "\n" | "\r\n" {
  const at = text.indexOf("\n");
  return at > 0 && text[at - 1] === "\r" ? "\r\n" : "\n";
}
