// 回复里行内代码写的文件路径（`src/app.ts:42`）点了在文件面板里打开。
// 只认像路径的：带 `/` 且末段有扩展名，或不带 `/` 但扩展名是常见文件类型（`store.ts`、`README.md`），
// 免得 `self.db`、`console.log` 这种也变成链接。文件面板只读会话目录里的（dock.1 fs/*），
// 绝对路径要落在会话目录下才认。

import { createContext } from "react";

export interface PathTarget {
  /** 相对会话目录。 */
  path: string;
  line: number | null;
}

/** 对话流给 Markdown 的「打开文件」：`resolve` 判断这串能不能开（渲染时调），`open` 去开。 */
export interface PathLinker {
  resolve: (code: string) => PathTarget | null;
  open: (target: PathTarget) => void;
}

export const PathLinkContext = createContext<PathLinker | null>(null);

/** 不带 `/` 时只认这些扩展名。 */
const FILE_EXT = new Set(
  (
    "rs ts tsx js jsx mjs cjs json jsonl md mdx toml yaml yml py go java kt kts swift c h cc cpp hpp cs rb php " +
    "sh zsh bash fish css scss less html htm vue svelte sql txt lock xml ini cfg conf gradle proto graphql " +
    "lua dart ex exs zig r mm plist csv tsv svg png jpg jpeg gif webp pdf ipynb dockerfile"
  ).split(" "),
);
/** 没有扩展名、但一眼是文件的名字。 */
const BARE_FILES = new Set(["Makefile", "Dockerfile", "LICENSE", "Justfile", "Gemfile", "Rakefile", "Procfile"]);

/** 拆出路径和行号；不像路径返回 `null`。 */
export function parsePath(code: string): { path: string; line: number | null } | null {
  const raw = code.trim();
  if (!raw || raw.length > 260 || /\s/.test(raw) || raw.includes("://")) return null;
  if (/[*?<>|"'(){}[\]$`,;=]/.test(raw) || raw.startsWith("-")) return null;
  // `a.ts:42`、`a.ts:42:7`、`a.ts#L42`
  const m = raw.match(/^(.+?)(?::(\d+)(?::\d+)?|#L(\d+)(?:-L?\d+)?)?$/);
  if (!m) return null;
  const path = m[1];
  const line = m[2] ?? m[3];
  const name = path.slice(path.lastIndexOf("/") + 1);
  if (!name || name === "." || name === "..") return null;
  const dot = name.lastIndexOf(".");
  const ext = dot > 0 ? name.slice(dot + 1) : "";
  const hasExt = /^[A-Za-z][A-Za-z0-9]{0,9}$/.test(ext);
  const dotfile = dot === 0 && /^\.[A-Za-z][\w.-]*$/.test(name);
  if (path.includes("/")) {
    if (!(hasExt || dotfile || BARE_FILES.has(name))) return null;
    if (!/[A-Za-z]/.test(path.replace(/\.[^.]*$/, ""))) return null;
  } else if (!((hasExt && FILE_EXT.has(ext.toLowerCase())) || dotfile || BARE_FILES.has(name))) {
    return null;
  }
  return { path, line: line ? Number(line) : null };
}

/**
 * 换成相对 `cwd` 的路径：相对路径去掉 `./`；绝对路径 / `~/` 要在 `cwd` 下面，否则 `null`
 * （文件面板只看会话目录）。
 */
export function relativeTo(path: string, cwd: string): string | null {
  let p = path;
  if (p.startsWith("~/")) {
    // 主目录从会话目录推（远程 Dock 的主目录不是本机的）。
    const home = cwd.match(/^(\/Users\/[^/]+|\/home\/[^/]+|\/root)(?=\/|$)/)?.[1];
    if (!home) return null;
    p = home + p.slice(1);
  }
  if (p.startsWith("/")) {
    const root = cwd.replace(/\/+$/, "");
    if (!p.startsWith(root + "/")) return null;
    p = p.slice(root.length + 1);
  }
  p = p.replace(/^(\.\/)+/, "");
  if (!p || p.split("/").some((part) => part === "..")) return null;
  return p;
}
