// 预览里把 `[[目标|显示文字]]` 画成可点的链接。
// 规则和 Dock 的 dock-memory/src/links.rs 一致：围栏代码块和行内代码里的不算。
// 这里只管把写法换成 Markdown 链接；「目标」指向哪篇笔记由 Dock 说了算（memory/note/get 的 targets）。

/** 预览里笔记链接的 href 前缀（后面是 encodeURIComponent 过的目标）。 */
export const NOTE_HREF = "#dock-note=";

/** href 是笔记链接就回它的目标（`[[…]]` 里写的名字），否则 null。 */
export function wikilinkTarget(href: string | null | undefined): string | null {
  const at = href?.indexOf(NOTE_HREF) ?? -1;
  if (!href || at < 0) return null;
  try {
    return decodeURIComponent(href.slice(at + NOTE_HREF.length));
  } catch {
    return null;
  }
}

const FORBIDDEN = /[[\]|#^]/;

function fence(line: string): { ch: string; len: number; bare: boolean } | null {
  const trimmed = line.replace(/^ {0,3}/, "");
  if (line.length - trimmed.length > 3 || /^ /.test(trimmed)) return null;
  const ch = trimmed[0];
  if (ch !== "`" && ch !== "~") return null;
  let len = 0;
  while (trimmed[len] === ch) len += 1;
  if (len < 3) return null;
  const rest = trimmed.slice(len);
  if (ch === "`" && rest.includes("`")) return null;
  return { ch, len, bare: rest.trim() === "" };
}

function linkifyLine(line: string): string {
  let out = "";
  let i = 0;
  while (i < line.length) {
    const c = line[i];
    if (c === "`") {
      let run = 0;
      while (line[i + run] === "`") run += 1;
      // 行内代码：找到同样长度的收尾才算。
      let j = i + run;
      let close = -1;
      while (j < line.length) {
        if (line[j] !== "`") {
          j += 1;
          continue;
        }
        let len = 0;
        while (line[j + len] === "`") len += 1;
        if (len === run) {
          close = j + len;
          break;
        }
        j += len;
      }
      const end = close < 0 ? i + run : close;
      out += line.slice(i, end);
      i = end;
      continue;
    }
    if (c === "[" && line[i + 1] === "[") {
      const close = line.indexOf("]]", i + 2);
      const inner = close < 0 ? null : line.slice(i + 2, close);
      const parsed = inner === null ? null : parseInner(inner);
      if (parsed && close >= 0) {
        // 嵌入写法 `![[x]]`：去掉前面的 `!`，不然会被当成图片。
        if (out.endsWith("!")) out = out.slice(0, -1);
        const href = NOTE_HREF + encodeURIComponent(parsed.target).replace(/\(/g, "%28").replace(/\)/g, "%29");
        out += `[${parsed.text}](${href})`;
        i = close + 2;
        continue;
      }
    }
    out += c;
    i += 1;
  }
  return out;
}

function parseInner(inner: string): { target: string; text: string } | null {
  const bar = inner.indexOf("|");
  const left = bar < 0 ? inner : inner.slice(0, bar);
  const alias = bar < 0 ? "" : inner.slice(bar + 1).trim();
  const hash = left.indexOf("#");
  const target = (hash < 0 ? left : left.slice(0, hash)).trim();
  if (!target || target.length > 200 || FORBIDDEN.test(target) || /[[\]]/.test(alias)) return null;
  return { target, text: alias || target };
}

/** 正文里的笔记链接换成 Markdown 链接。 */
export function linkifyWikilinks(text: string): string {
  let open: { ch: string; len: number } | null = null;
  return text
    .split("\n")
    .map((line) => {
      const mark = fence(line);
      if (mark) {
        if (!open) open = { ch: mark.ch, len: mark.len };
        else if (mark.ch === open.ch && mark.len >= open.len && mark.bare) open = null;
        return line;
      }
      return open ? line : linkifyLine(line);
    })
    .join("\n");
}
