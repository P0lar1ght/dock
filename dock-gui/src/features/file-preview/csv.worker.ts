// 大 CSV / TSV 在这里解析：主线程只管收块、画表。字节 → 认编码 → 流式解码 → 增量解析，
// 行分批发回去（一批太大主线程反序列化会掉帧）。

import { DelimitedParser, sniffEncoding, sniffEol, sniffSeparator } from "./csv";
import { decodeBase64 } from "./bytes";

export type ToCsvWorker =
  | { t: "start"; sep: string | null; encoding: string | null; maxRows: number }
  | { t: "text"; text: string }
  | { t: "bytes"; data: string }
  | { t: "end" };

export type FromCsvWorker =
  | { t: "rows"; rows: string[][] }
  | { t: "info"; sep: string; encoding: string; bom: boolean }
  /** 行数到上限了：后面的不要了。 */
  | { t: "capped" }
  /** 读完了：原文件的换行、末尾有没有换行（存回去照着来）。 */
  | { t: "done"; eol: "\n" | "\r\n"; trailingNewline: boolean };

const BATCH = 5000;

let parser: DelimitedParser | null = null;
let sep: string | null = null;
let encoding: string | null = null;
let decoder: TextDecoder | null = null;
let maxRows = Infinity;
let count = 0;
let capped = false;
let bom = false;
let eol: "\n" | "\r\n" | null = null;
let last = "";

const post = (m: FromCsvWorker) => (self as unknown as Worker).postMessage(m);

function feed(text: string) {
  if (capped || !text) return;
  if (!parser) {
    if (text.charCodeAt(0) === 0xfeff) bom = true;
    sep ??= sniffSeparator(text);
    parser = new DelimitedParser(sep);
    post({ t: "info", sep, encoding: encoding ?? "utf-8", bom });
  }
  if (!eol && /[\r\n]/.test(text)) eol = sniffEol(text);
  last = text[text.length - 1];
  emit(parser.push(text));
}

function emit(rows: string[][]) {
  if (rows.length === 0) return;
  if (count + rows.length >= maxRows) {
    rows = rows.slice(0, maxRows - count);
    capped = true;
  }
  count += rows.length;
  for (let i = 0; i < rows.length; i += BATCH) post({ t: "rows", rows: rows.slice(i, i + BATCH) });
  if (capped) post({ t: "capped" });
}

self.onmessage = (e: MessageEvent<ToCsvWorker>) => {
  const m = e.data;
  if (m.t === "start") {
    sep = m.sep;
    encoding = m.encoding;
    maxRows = m.maxRows;
  } else if (m.t === "text") feed(m.text);
  else if (m.t === "bytes") {
    const bytes = decodeBase64(m.data);
    if (!decoder) {
      // 解码器会吃掉 BOM：先看一眼，存回去要带上。
      if (bytes[0] === 0xef && bytes[1] === 0xbb && bytes[2] === 0xbf) bom = true;
      encoding ??= sniffEncoding(bytes);
      decoder = new TextDecoder(encoding);
    }
    feed(decoder.decode(bytes, { stream: true }));
  } else if (m.t === "end") {
    if (decoder) feed(decoder.decode());
    if (parser && !capped) emit(parser.end());
    post({ t: "done", eol: eol ?? "\n", trailingNewline: last === "\n" || last === "\r" });
  }
};
