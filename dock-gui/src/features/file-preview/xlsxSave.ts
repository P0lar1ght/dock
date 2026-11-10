// 把单元格的改动写回 xlsx：在原来的 zip 里只换改过的工作表 XML（见 xlsxPatch.ts）。
// zip 读写用 SheetJS 自带的 CFB（调用方传进来：Worker 里有，测试里也能直接用）。

import { ensureFullCalc, patchSheetXml, PatchError, sheetPath, type CellEdit } from "./xlsxPatch.ts";

interface CfbEntry {
  content: Uint8Array;
  size?: number;
}

/** SheetJS 的 CFB 用到的那几个函数。 */
export interface Cfb {
  read(data: Uint8Array, opts: { type: "array" }): unknown;
  find(container: unknown, path: string): CfbEntry | null;
  write(container: unknown, opts: { fileType: "zip"; type: "array"; compression: boolean }): ArrayBuffer | Uint8Array;
}

/**
 * `edits`：工作表下标 → 那张表的改动。`recalc`：工作簿里有公式，让 Excel 打开时重算；
 * `"auto"` = 有 calcChain（Excel 有公式时会写它）才重算。
 */
export function applyEdits(
  cfb: Cfb,
  bytes: Uint8Array,
  edits: Map<number, CellEdit[]>,
  recalc: boolean | "auto",
): Uint8Array {
  const zip = cfb.read(bytes, { type: "array" });
  const entry = (path: string) => {
    // CFB 的路径带前导斜杠（`/xl/workbook.xml`）。
    const e = cfb.find(zip, `/${path}`);
    if (!e) throw new PatchError(`文件里缺 ${path}`);
    return e;
  };
  const text = (path: string) => new TextDecoder().decode(entry(path).content);
  const put = (path: string, xml: string) => {
    const e = entry(path);
    e.content = new TextEncoder().encode(xml);
    e.size = e.content.length;
  };
  const workbook = text("xl/workbook.xml");
  const rels = text("xl/_rels/workbook.xml.rels");
  for (const [index, list] of edits) {
    if (list.length === 0) continue;
    const path = sheetPath(workbook, rels, index);
    put(path, patchSheetXml(text(path), list));
  }
  if (recalc === true || (recalc === "auto" && cfb.find(zip, "/xl/calcChain.xml")))
    put("xl/workbook.xml", ensureFullCalc(workbook));
  const out = cfb.write(zip, { fileType: "zip", type: "array", compression: true });
  return out instanceof Uint8Array ? out : new Uint8Array(out);
}
