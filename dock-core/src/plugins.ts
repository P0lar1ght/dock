// 插件给界面的东西：视图树 dock.view.1、插件面板、状态项、工具卡视图。
// 契约见 `docs/PLUGIN-VIEWS.md`；视图树的规范化与 `cordis-base/src/view.rs` 一一对应，
// 面板 / 状态项 / 工具卡的外形来自 `cordis-gateway/src/handlers/{surface,status,tool_view}.rs`。
// 网关给的已经是规范化过的树，这里再过一遍：类型落地，旧网关或手写数据也不会把界面画坏。

export type Tone = 'default' | 'muted' | 'accent' | 'success' | 'warning' | 'danger';
export type Size = 's' | 'm' | 'l';

export const MAX_DEPTH = 8;
export const MAX_NODES = 500;
export const MAX_TEXT = 20_000;

export interface Badge {
  text: string;
  tone: Tone;
}

export interface KvItem {
  label: string;
  value: string;
  tone: Tone;
}

export interface ListItem {
  title: string;
  subtitle: string | null;
  badge: Badge | null;
  /** 点这一行触发的动作 id。 */
  action: string | null;
}

export type ViewNode =
  | { type: 'stack'; children: ViewNode[]; gap: Size }
  | { type: 'row'; children: ViewNode[]; align: 'start' | 'between' }
  | { type: 'section'; title: string; children: ViewNode[]; collapsed: boolean }
  | { type: 'text'; text: string; tone: Tone; weight: 'normal' | 'bold'; size: Size }
  | { type: 'markdown'; text: string }
  | { type: 'code'; text: string; lang: string | null }
  | { type: 'kv'; items: KvItem[] }
  | { type: 'table'; columns: string[]; rows: string[][] }
  | { type: 'list'; items: ListItem[] }
  | { type: 'badge'; text: string; tone: Tone }
  /** `value` 是 0–1；`null` = 不确定进度。 */
  | { type: 'progress'; value: number | null; label: string | null }
  | { type: 'button'; label: string; action: string; style: 'primary' | 'secondary' | 'danger' }
  | { type: 'link'; label: string; url: string }
  | { type: 'divider' }
  | { type: 'empty'; title: string; text: string | null; action: string | null; label: string | null }
  /** 不认识的节点：界面画一行「不支持的视图：kind」。 */
  | { type: 'unsupported'; kind: string };

const TONES: readonly Tone[] = ['default', 'muted', 'accent', 'success', 'warning', 'danger'];

type Obj = Record<string, unknown>;

function obj(v: unknown): Obj {
  return v !== null && typeof v === 'object' && !Array.isArray(v) ? (v as Obj) : {};
}

function clip(s: string): string {
  const chars = Array.from(s);
  return chars.length <= MAX_TEXT ? s : `${chars.slice(0, MAX_TEXT).join('')}…`;
}

/** 字符串原样；数字、布尔转成字（插件常直接给数）；其余是空串。 */
function textOf(v: unknown): string {
  if (typeof v === 'string') return clip(v);
  if (typeof v === 'number' || typeof v === 'boolean') return String(v);
  return '';
}

function optText(v: unknown): string | null {
  return textOf(v) || null;
}

export function toneOf(v: unknown): Tone {
  return TONES.includes(v as Tone) ? (v as Tone) : 'default';
}

function sizeOf(v: unknown): Size {
  return v === 's' || v === 'l' ? v : 'm';
}

function badgeOf(v: unknown): Badge | null {
  const b = obj(v);
  const text = textOf(b.text);
  return text ? { text, tone: toneOf(b.tone) } : null;
}

/** 只放行 http / https / mailto。 */
function safeUrl(url: string): boolean {
  const lower = url.trim().toLowerCase();
  return ['http://', 'https://', 'mailto:'].some((p) => lower.startsWith(p));
}

interface Budget {
  nodes: number;
  truncated: boolean;
}

/** `kv` / `table` / `list` 的条目也算节点，超出预算的截掉。 */
function takeItems(v: unknown, budget: Budget): unknown[] {
  if (!Array.isArray(v)) return [];
  const n = Math.min(v.length, budget.nodes);
  if (n < v.length) budget.truncated = true;
  budget.nodes -= n;
  return v.slice(0, n);
}

function children(v: unknown, depth: number, budget: Budget): ViewNode[] {
  if (!Array.isArray(v)) return [];
  const out: ViewNode[] = [];
  for (const item of v) {
    const node = parseAt(item, depth + 1, budget);
    if (node) out.push(node);
  }
  return out;
}

function parseAt(v: unknown, depth: number, budget: Budget): ViewNode | null {
  if (depth > MAX_DEPTH || budget.nodes === 0) {
    budget.truncated = true;
    return null;
  }
  budget.nodes -= 1;
  if (typeof v === 'string') {
    return { type: 'text', text: clip(v), tone: 'default', weight: 'normal', size: 'm' };
  }
  const o = obj(v);
  const kind = typeof o.type === 'string' ? o.type : '';
  switch (kind) {
    case 'stack':
      return { type: 'stack', children: children(o.children, depth, budget), gap: sizeOf(o.gap) };
    case 'row':
      return {
        type: 'row',
        children: children(o.children, depth, budget),
        align: o.align === 'between' ? 'between' : 'start',
      };
    case 'section':
      return {
        type: 'section',
        title: textOf(o.title),
        children: children(o.children, depth, budget),
        collapsed: o.collapsed === true,
      };
    case 'text':
      return {
        type: 'text',
        text: textOf(o.text),
        tone: toneOf(o.tone),
        weight: o.weight === 'bold' ? 'bold' : 'normal',
        size: sizeOf(o.size),
      };
    case 'markdown':
      return { type: 'markdown', text: textOf(o.text) };
    case 'code':
      return { type: 'code', text: textOf(o.text), lang: optText(o.lang) };
    case 'kv':
      return {
        type: 'kv',
        items: takeItems(o.items, budget).map((i) => {
          const it = obj(i);
          return { label: textOf(it.label), value: textOf(it.value), tone: toneOf(it.tone) };
        }),
      };
    case 'table':
      return {
        type: 'table',
        columns: Array.isArray(o.columns) ? o.columns.map(textOf) : [],
        rows: takeItems(o.rows, budget).map((r) => (Array.isArray(r) ? r.map(textOf) : [])),
      };
    case 'list':
      return {
        type: 'list',
        items: takeItems(o.items, budget).map((i) => {
          const it = obj(i);
          return {
            title: textOf(it.title),
            subtitle: optText(it.subtitle),
            badge: badgeOf(it.badge),
            action: optText(it.action),
          };
        }),
      };
    case 'badge':
      return { type: 'badge', text: textOf(o.text), tone: toneOf(o.tone) };
    case 'progress': {
      const value = typeof o.value === 'number' && Number.isFinite(o.value) ? Math.min(1, Math.max(0, o.value)) : null;
      return { type: 'progress', value, label: optText(o.label) };
    }
    case 'button':
      return {
        type: 'button',
        label: textOf(o.label),
        action: textOf(o.action),
        style: o.style === 'primary' || o.style === 'danger' ? o.style : 'secondary',
      };
    case 'link': {
      const label = textOf(o.label);
      const url = textOf(o.url);
      if (safeUrl(url)) return { type: 'link', label, url };
      return { type: 'text', text: `${label}（已拦下不安全的链接）`, tone: 'muted', weight: 'normal', size: 'm' };
    }
    case 'divider':
      return { type: 'divider' };
    case 'empty':
      return {
        type: 'empty',
        title: textOf(o.title),
        text: optText(o.text),
        action: optText(o.action),
        label: optText(o.label),
      };
    default:
      return { type: 'unsupported', kind: kind ? Array.from(kind).slice(0, 40).join('') : '（缺 type）' };
  }
}

/**
 * 一棵视图树：深度最多 8 层、节点（含条目）最多 500 个、文字最多 20000 字。
 * 超出的部分截掉，末尾补一行灰字说明。不是对象也不是字符串时回空 `stack`。
 */
export function parseView(value: unknown): ViewNode {
  const budget: Budget = { nodes: MAX_NODES, truncated: false };
  const root = parseAt(value, 1, budget) ?? { type: 'stack', children: [], gap: 'm' };
  if (!budget.truncated) return root;
  return {
    type: 'stack',
    gap: 'm',
    children: [
      root,
      { type: 'text', text: '（视图过大，后面的部分没有显示）', tone: 'muted', weight: 'normal', size: 's' },
    ],
  };
}

/**
 * 树里能点的动作 id（按出现顺序、去重）：按钮、列表行、空态按钮。
 * 默认跳过折叠的 `section`（看不见就点不到）；`includeCollapsed` 连它们一起算。
 */
export function viewActions(node: ViewNode, includeCollapsed = false): string[] {
  const out: string[] = [];
  const push = (a: string | null) => {
    if (a && !out.includes(a)) out.push(a);
  };
  const walk = (n: ViewNode) => {
    switch (n.type) {
      case 'section':
        if (n.collapsed && !includeCollapsed) return;
        n.children.forEach(walk);
        return;
      case 'stack':
      case 'row':
        n.children.forEach(walk);
        return;
      case 'button':
        push(n.action);
        return;
      case 'list':
        n.items.forEach((i) => push(i.action));
        return;
      case 'empty':
        push(n.action);
        return;
      default:
    }
  };
  walk(node);
  return out;
}

/**
 * 面板底部固定按钮的规则：根是 `stack`、最后一个子节点是只含 `button` 的 `row`，
 * 那一行固定在面板底部（`footer`），其余是可滚动的正文（`body`）。别的形状 `footer` 为 `null`。
 */
export function splitPanelFooter(root: ViewNode): { body: ViewNode; footer: (ViewNode & { type: 'row' }) | null } {
  if (root.type !== 'stack') return { body: root, footer: null };
  const last = root.children[root.children.length - 1];
  if (!last || last.type !== 'row' || last.children.length === 0) return { body: root, footer: null };
  if (!last.children.every((c) => c.type === 'button')) return { body: root, footer: null };
  return { body: { ...root, children: root.children.slice(0, -1) }, footer: last };
}

// ---- 插件面板 `surface/*` ----

/** `surface/list` 的一项：谁都能看，只有 id 和标题。 */
export interface SurfaceSummary {
  id: string;
  title: string;
  /** 也算一个状态项（旧插槽的 `hud: true`）。 */
  hud: boolean;
  /** 自带 web 界面（`surface/web`）：GUI 用沙箱 iframe 画它。 */
  web: boolean;
}

export interface SurfaceAction {
  id: string;
  label: string;
}

/** `surface/get` 的面板：有视图就画视图，否则画 `body` 文本。 */
export interface Surface {
  id: string;
  title: string;
  body: string;
  view: ViewNode | null;
  actions: SurfaceAction[];
  /** 自带 web 界面：GUI 画 `surface/web` 的 HTML，终端照旧画视图 / 正文。 */
  web: boolean;
}

export function parseSurfaceList(result: unknown): SurfaceSummary[] {
  const list = obj(result).surfaces;
  if (!Array.isArray(list)) return [];
  return list
    .map(obj)
    .filter((s) => typeof s.id === 'string' && s.id)
    .map((s) => ({ id: s.id as string, title: textOf(s.title) || (s.id as string), hud: s.hud === true, web: s.web === true }));
}

/** `surface/get` 的 `surface`；不是对象（如 `null`）回 `null`。 */
export function parseSurface(value: unknown): Surface | null {
  if (value === null || typeof value !== 'object') return null;
  const s = obj(value);
  const id = textOf(s.id);
  if (!id) return null;
  const actions = Array.isArray(s.actions)
    ? s.actions.map(obj).map((a) => ({ id: textOf(a.id), label: textOf(a.label) || textOf(a.id) }))
    : [];
  return {
    id,
    title: textOf(s.title) || id,
    body: typeof s.body === 'string' ? s.body : '',
    view: s.view == null ? null : parseView(s.view),
    actions: actions.filter((a) => a.id),
    web: s.web === true,
  };
}

/** `surface/web` → 面板自带的 HTML；不是字符串回 `null`。 */
export function parseSurfaceWeb(result: unknown): string | null {
  const html = obj(result).html;
  return typeof html === 'string' ? html : null;
}

/** `surface/action` 的结果：`closed` 为真时面板该收起（插件自己关了或注销了）。 */
export function parseSurfaceAction(result: unknown): { closed: boolean; surface: Surface | null } {
  const r = obj(result);
  const surface = parseSurface(r.surface);
  return { closed: r.closed === true || surface === null, surface };
}

// ---- 状态项 `status/*` ----

export interface StatusItem {
  id: string;
  /** 插件给的原文：界面只能截断，不能改写。 */
  text: string;
  tone: Tone;
  tooltip: string | null;
  /** 点它打开哪个插件面板。 */
  surface: string | null;
}

export function parseStatusItems(result: unknown): StatusItem[] {
  const items = obj(result).items;
  if (!Array.isArray(items)) return [];
  return items
    .map(obj)
    .map((i) => ({
      id: textOf(i.id),
      text: textOf(i.text),
      tone: toneOf(i.tone),
      tooltip: optText(i.tooltip),
      surface: optText(i.surface),
    }))
    .filter((i) => i.id && i.text);
}

// ---- 工具卡视图 `tool/*` ----

/** `tool/views` → 有卡片视图的工具名。 */
export function parseToolViews(result: unknown): string[] {
  const tools = obj(result).tools;
  return Array.isArray(tools) ? tools.filter((t): t is string => typeof t === 'string' && t !== '') : [];
}

/** `tool/view` → 这次调用的视图；插件这次不给 / 没登记时是 `null`。 */
export function parseToolView(result: unknown): ViewNode | null {
  const view = obj(result).view;
  return view == null ? null : parseView(view);
}

/**
 * 按哪个工具名找视图：按需工具（插件的工具都是）经 `use_tool { tool_name, tool_input }`
 * 调用，要看里面那颗。和 `cordis-spine` 的 `effective_call` 同一条规则。
 */
export function viewToolName(toolName: string, args: unknown): string {
  if (toolName === 'use_tool') {
    const inner = obj(args).tool_name;
    if (typeof inner === 'string' && inner) return inner;
  }
  return toolName;
}

// ---- 连接级推送 ----

/** 插件相关的连接级推送（不用订阅）。收到就重拉对应的那一份。 */
export type PluginPush =
  | { kind: 'surface'; id: string }
  | { kind: 'status'; id: string }
  | { kind: 'toolViews'; name: string };

/** 认不出的方法回 `null`。 */
export function parsePluginPush(method: string, params: unknown): PluginPush | null {
  const p = obj(params);
  switch (method) {
    case 'surface/changed':
      return { kind: 'surface', id: textOf(p.id) };
    case 'status/changed':
      return { kind: 'status', id: textOf(p.id) };
    case 'tool/views/changed':
      return { kind: 'toolViews', name: textOf(p.name) };
    default:
      return null;
  }
}
