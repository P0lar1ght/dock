// 代码块语法高亮（Shiki）。Shiki 和各语言语法按需加载：第一次用到才 import，之后共用一个实例。
// 结果按「主题 + 语言 + 代码」缓存：流式时前面没变的代码块重渲染不再高亮一遍。

import { useEffect, useState } from "react";
import type { ThemedToken } from "shiki";

/** 设置 › 外观里可选的配色（只放浅色：GUI 第一版只做浅色）。 */
export const CODE_THEMES = [
  { value: "github-light", label: "GitHub" },
  { value: "one-light", label: "One Light" },
  { value: "vitesse-light", label: "Vitesse" },
  { value: "catppuccin-latte", label: "Catppuccin Latte" },
  { value: "solarized-light", label: "Solarized" },
  { value: "min-light", label: "Min" },
];

export interface Highlighted {
  /** 高亮的是哪段代码：流式时旧结果是新代码的前缀，就先画旧的、尾巴按原文补上。 */
  code: string;
  lines: ThemedToken[][];
  bg: string;
}

type Highlighter = Awaited<ReturnType<(typeof import("shiki"))["createHighlighter"]>>;

let instance: Promise<{ hl: Highlighter; known: Set<string> }> | null = null;
const loading = new Map<string, Promise<void>>();

function highlighter() {
  instance ??= import("shiki").then(async (shiki) => ({
    hl: await shiki.createHighlighter({ themes: [], langs: [] }),
    known: new Set([...Object.keys(shiki.bundledLanguages), ...Object.keys(shiki.bundledThemes)]),
  }));
  return instance;
}

/** 语言 / 主题没加载过就加载一次（并发调用共用同一次加载）。 */
function ensure(kind: "lang" | "theme", id: string, hl: Highlighter): Promise<void> {
  const key = `${kind}:${id}`;
  let p = loading.get(key);
  if (!p) {
    // id 已对过 bundledLanguages / bundledThemes（见 highlight）。
    p = kind === "lang" ? hl.loadLanguage(id as never) : hl.loadTheme(id as never);
    loading.set(key, p);
  }
  return p;
}

const cache = new Map<string, Highlighted>();
const CACHE_MAX = 300;

async function highlight(code: string, lang: string, theme: string): Promise<Highlighted> {
  const { hl, known } = await highlighter();
  const id = known.has(lang) ? lang : "text";
  const themeId = known.has(theme) ? theme : "github-light";
  await Promise.all([id === "text" ? null : ensure("lang", id, hl), ensure("theme", themeId, hl)]);
  const out = hl.codeToTokens(code, { lang: id as never, theme: themeId as never });
  return { code, lines: out.tokens, bg: out.bg ?? "" };
}

const keyOf = (code: string, lang: string, theme: string) => `${theme}\u0000${lang}\u0000${code}`;

/**
 * 高亮结果；还没好时是上一次的结果（可能是更短的旧代码）或 `null`。
 * 代码在变（流式）时攒 80ms 再高亮，不每个增量都算一遍。
 */
export function useHighlight(code: string, lang: string, theme: string): Highlighted | null {
  const key = keyOf(code, lang, theme);
  const [last, setLast] = useState<{ theme: string; value: Highlighted } | null>(null);
  const hit = cache.get(key);
  useEffect(() => {
    if (cache.has(key)) return;
    let alive = true;
    const timer = window.setTimeout(() => {
      highlight(code, lang, theme)
        .then((value) => {
          if (cache.size >= CACHE_MAX) cache.delete(cache.keys().next().value!);
          cache.set(key, value);
          if (alive) setLast({ theme, value });
        })
        // 高亮失败就停在原文：代码照样能看能复制。
        .catch((e) => console.warn("代码高亮失败", lang, e));
    }, 80);
    return () => {
      alive = false;
      window.clearTimeout(timer);
    };
    // key 已含 code / lang / theme。
  }, [key]);
  if (hit) return hit;
  return last && last.theme === theme ? last.value : null;
}

/** 预热：设置页切主题时先把主题载进来，回到对话里不闪。 */
export function preloadTheme(theme: string) {
  void highlighter().then(({ hl, known }) => (known.has(theme) ? ensure("theme", theme, hl) : undefined));
}
