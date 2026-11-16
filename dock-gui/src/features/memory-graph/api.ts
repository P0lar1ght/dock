// 记忆图谱（dock.1 `memory/*`，见网关 handlers/memory.rs）。
// 笔记是 Dock 的 `$DOCK_HOME/memory/` 下的 Markdown，笔记之间用 `[[文件名]]` 链接。
// 全部只认受信连接；笔记变了推 `memory/changed`（连接级），收到后重拉图。

import { dock } from "../../dock/client";

export type NoteKind = "topic" | "observation";
/** `flush` / `remember` 是 Dock 自己写的（文件名带时间），`authored` 是 agent 或人起名写的。 */
export type NoteOrigin = "flush" | "remember" | "authored";

export interface MemoryScope {
  /** 记忆目录下那一层的目录名：`global` / `workspace-<slug>`。 */
  id: string;
  kind: "global" | "workspace";
  /** 给人看的短名（项目名）；全局是 `global`，界面自己换成「全局」。 */
  label: string;
  /** 是不是 `cwd` 那个项目。 */
  current: boolean;
  /** 这个 scope 的笔记在不在图里。 */
  selected: boolean;
  topics: number;
  observations: number;
}

export interface MemoryNote {
  /** `<scope>/<相对路径>`，如 `global/topics/prefs.md`。改名 / 挪位置后会变。 */
  id: string;
  scope: string;
  kind: NoteKind;
  origin: NoteOrigin;
  /** 文件名去掉 `.md`：`[[链接]]` 里写的就是它。 */
  name: string;
  title: string;
  /** 第一行正文。 */
  summary: string;
  /** Dock 那台机器上的绝对路径。 */
  path: string;
  bytes: number;
  modifiedMs: number | null;
  /** 嵌套目录和旧版平铺的观察只读。 */
  writable: boolean;
  /** 正文超过读取上限，图上只算了开头一段。 */
  truncated?: boolean;
}

export interface MemoryEdge {
  source: string;
  target: string;
  /** `link`：正文里写了 `[[…]]`（source 引用 target）；`similar`：内容相近、还没链上。 */
  kind: "link" | "similar";
  /** 相似度 0–1；链接恒为 1。 */
  score: number;
}

export interface MemoryGraph {
  /** agent 现在用不用记忆（`[memory] enabled`）。关着也能看、能改。 */
  enabled: boolean;
  /** 被 `DOCK_MEMORY=0` 强制关闭。 */
  forcedOff: boolean;
  home: string;
  currentScope: string | null;
  /** 磁盘上所有 scope 的概况（没选进图里的也在）。 */
  scopes: MemoryScope[];
  notes: MemoryNote[];
  edges: MemoryEdge[];
  /** 指向不存在的笔记的链接。 */
  unresolved: { source: string; target: string }[];
  /** 笔记太多，没算相似建议。 */
  similarSkipped: boolean;
}

export interface NoteContent extends MemoryNote {
  text: string;
  /** 正文指纹：保存、遗忘时带回去，用来发现期间被改过。 */
  hash: string;
}

export interface LinkedNote {
  id: string;
  title: string;
  /** 链接所在的那一行。 */
  context: string;
  line: number;
}

export interface NoteDetail {
  note: NoteContent;
  links: {
    outgoing: LinkedNote[];
    backlinks: LinkedNote[];
    unresolved: string[];
    /** 正文里每种 `[[写法]]` 指向的笔记（悬空为 null）。 */
    targets: { target: string; id: string | null }[];
  };
}

export type ScopeMode = "current" | "all";

/** `cwd` 是「当前项目」的目录；没有就只看全局。 */
export const memoryGraph = (cwd: string | null, scopes: ScopeMode) =>
  dock.call<MemoryGraph>("memory/graph", { cwd: cwd ?? undefined, scopes });

export const memorySearch = (query: string, cwd: string | null, scopes: ScopeMode) =>
  dock
    .call<{ hits: { id: string; line: number; snippet: string }[] }>("memory/search", { query, cwd: cwd ?? undefined, scopes, limit: 200 })
    .then((r) => r.hits);

export const noteGet = (id: string) => dock.call<NoteDetail>("memory/note/get", { id });

export const noteSave = (id: string, text: string, baseHash: string | null) =>
  dock.call<{ note: NoteContent }>("memory/note/save", { id, text, baseHash: baseHash ?? undefined }).then((r) => r.note);

export const noteCreate = (p: { scope: string; kind: NoteKind; title: string; text?: string }) =>
  dock.call<{ note: NoteContent }>("memory/note/create", p).then((r) => r.note);

export interface Relocated {
  note: NoteContent;
  oldId: string;
  /** 链接被跟着改写了的其它笔记。 */
  relinked: string[];
}

export const noteRename = (id: string, p: { title?: string; name?: string }) => dock.call<Relocated>("memory/note/rename", { id, ...p });

export const noteMove = (id: string, p: { scope?: string; kind?: NoteKind }) => dock.call<Relocated>("memory/note/move", { id, ...p });

export const noteForget = (id: string, hash: string) => dock.call<{ archivedTo: string | null }>("memory/note/forget", { id, hash });

export const linkNotes = (from: string, to: string) => dock.call<{ note: NoteContent; changed: boolean }>("memory/link", { from, to });

/** 断开 `from` 里指向 `to`（笔记 id）的链接；`dangling` 是悬空链接里写的名字。 */
export const unlinkNotes = (from: string, target: { to: string } | { dangling: string }) =>
  dock.call<{ note: NoteContent; changed: boolean }>("memory/unlink", "to" in target ? { from, to: target.to } : { from, target: target.dangling });

/** 有笔记变了（界面、agent、外部编辑器都算）。返回取消函数。 */
export function onMemoryChanged(fn: () => void): () => void {
  return dock.onNotification((n) => {
    if (n.method === "memory/changed") fn();
  });
}

/** 这台 Dock 有没有记忆图谱（老版本的远程 Dock 没有）。 */
export const memorySupported = () => dock.capabilities.memoryGraph === true;

/** scope 给人看的名字。 */
export const scopeName = (scope: Pick<MemoryScope, "kind" | "label">) => (scope.kind === "global" ? "全局" : scope.label);
