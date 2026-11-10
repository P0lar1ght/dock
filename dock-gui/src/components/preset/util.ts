// 预设编辑器的样式串和小函数。单独放：组件文件只导出组件，Fast Refresh 才不整页重挂。

export const input =
  "motion-colors h-9 w-full rounded-lg border border-[#d1d1d6] bg-white px-3 text-[13px] text-ink shadow-[0px_1px_0px_rgba(0,0,0,0.05)] outline-none select-text placeholder:text-ink-5 focus:border-accent";
export const inputError = "border-[#ff3b30] bg-[#fff5f5] focus:border-[#ff3b30]";
export const inputLocked = "border-line-strong bg-[#f2f2f7] text-ink-3";

/** 按字符数算「+N / -N 字符」。 */
export function charDelta(before: string, after: string): string {
  const d = [...after].length - [...before].length;
  return d === 0 ? "内容改动" : `${d > 0 ? "+" : ""}${d} 字符`;
}

/** 复制子代理时起一个不重名的 id（汉字 id 追加「副本」，其余追加 -copy / -copy-N）。 */
export function copyId(id: string, taken: string[]): string {
  const cjk = /^[㐀-鿿]+$/.test(id);
  for (let n = 1; n < 100; n++) {
    const next = cjk ? `${id}副本${n > 1 ? "一二三四五六七八九"[n - 2] ?? n : ""}` : `${id}-copy${n > 1 ? `-${n}` : ""}`;
    if (!taken.includes(next)) return next;
  }
  return `${id}-${Date.now()}`;
}

export type Mode = "all" | "pick" | "none";

/** `[]` 既是「不用工具」也是「自选但还没勾」：`picking` 说明用户正停在自选里。 */
export function modeOf(tools: string[] | null, picking = false): Mode {
  return tools === null ? "all" : tools.length === 0 && !picking ? "none" : "pick";
}

/** 侧栏 / 名册上的工具数说明。 */
export function toolCountLabel(tools: string[] | null): string {
  return tools === null ? "全部工具" : tools.length === 0 ? "不用工具" : `${tools.length} 个工具`;
}
