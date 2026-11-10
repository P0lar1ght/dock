// 画布页面的装配：模型写的 HTML → 沙箱 iframe 的 srcdoc。
//
// - 注入桥 `window.dock`（data / onData / setData / send）和报错回传；
// - `dock:lib/<名字>` 换成随包带的库（data: URL，离线可用）。
//
// 桥脚本塞在第一行、不带换行，库走 src 不内联：页面报错的行号就是 vN.html 的行号。
// 沙箱不同源，localStorage 一碰就抛错：桥换成内存版，页面照常跑（刷新即丢）。

import type { ThreadState } from "dock-core";
import { calledTool } from "./toolCall";

/** 随包带的库。键是 `dock:lib/` 后面的名字。 */
const LIBS: Record<string, () => Promise<string>> = {
  "chart.js": () => import("../../node_modules/chart.js/dist/chart.umd.min.js?raw").then((m) => m.default),
};

const libCache = new Map<string, Promise<string>>();

function libUrl(name: string): Promise<string | null> {
  const load = LIBS[name];
  if (!load) return Promise.resolve(null);
  let p = libCache.get(name);
  if (!p) {
    p = load().then((src) => `data:text/javascript;base64,${toBase64(src)}`);
    libCache.set(name, p);
  }
  return p;
}

function toBase64(text: string): string {
  const bytes = new TextEncoder().encode(text);
  let bin = "";
  for (let i = 0; i < bytes.length; i += 0x8000) bin += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(bin);
}

const LIB_RE = /(["'])dock:lib\/([\w.-]+)\1/g;

/** 页面 → 面板的消息。 */
export type FromCanvas =
  | { type: "ready" }
  | { type: "setData"; data: unknown }
  | { type: "send"; text: string }
  | { type: "error"; message: string; line: number | null; col: number | null }
  /** 焦点在画布里时按的 ⌘ / ⌃ 组合键：转给外面，应用快捷键照常用（编辑键留在页面里）。 */
  | { type: "key"; key: string; code: string; metaKey: boolean; ctrlKey: boolean; shiftKey: boolean; altKey: boolean };

/** JSON 放进 <script> 里：挡住 `</script>` 和 U+2028/2029。 */
function scriptJson(value: unknown): string {
  return JSON.stringify(value ?? null)
    .replace(/</g, "\\u003c")
    .replace(/\u2028/g, "\\u2028")
    .replace(/\u2029/g, "\\u2029");
}

function bridge(data: unknown): string {
  // 一行，别加换行（见文件头）。
  const js = `(function(){var P=window.parent,L=[];function post(m){m.__dock=1;try{P.postMessage(m,"*")}catch(e){}}function emit(d){L.slice().forEach(function(f){try{f(d)}catch(e){report(e)}})}function report(e,line,col){var msg=e&&e.message?(e.name?e.name+": ":"")+e.message:String(e);post({type:"error",message:msg,line:line||null,col:col||null})}var dock={data:${scriptJson(data)},onData:function(f){L.push(f);return function(){L=L.filter(function(x){return x!==f})}},setData:function(d){dock.data=d;post({type:"setData",data:d});emit(d)},send:function(t){post({type:"send",text:String(t)})}};window.dock=dock;window.addEventListener("message",function(e){if(e.source!==P)return;var m=e.data;if(m&&m.__dock&&m.type==="data"){dock.data=m.data;emit(m.data)}});window.addEventListener("error",function(e){if(e.error||e.message)report(e.error||e.message,e.lineno,e.colno)});window.addEventListener("unhandledrejection",function(e){report(e.reason)});window.addEventListener("DOMContentLoaded",function(){post({type:"ready"})});window.addEventListener("keydown",function(e){if(!(e.metaKey||e.ctrlKey)||/^[acvxzy]$/i.test(e.key))return;post({type:"key",key:e.key,code:e.code,metaKey:e.metaKey,ctrlKey:e.ctrlKey,shiftKey:e.shiftKey,altKey:e.altKey})});try{window.localStorage}catch(e){function M(){var s={};return{getItem:function(k){return Object.prototype.hasOwnProperty.call(s,k)?s[k]:null},setItem:function(k,v){s[k]=String(v)},removeItem:function(k){delete s[k]},clear:function(){s={}},key:function(i){var k=Object.keys(s)[i];return k===undefined?null:k},get length(){return Object.keys(s).length}}}try{Object.defineProperty(window,"localStorage",{value:M(),configurable:true});Object.defineProperty(window,"sessionStorage",{value:M(),configurable:true})}catch(e2){}}})();`;
  return `<script>${js}</script>`;
}

/** 组装 srcdoc。也是「导出自包含 HTML」：库成了 data: URL、数据写在桥里，离线双击能开。 */
export async function buildDoc(html: string, data: unknown): Promise<string> {
  const names = new Set<string>();
  for (const m of html.matchAll(LIB_RE)) names.add(m[2]);
  const urls = new Map<string, string | null>();
  await Promise.all([...names].map(async (n) => urls.set(n, await libUrl(n))));
  const body = html.replace(LIB_RE, (whole, q: string, name: string) => {
    const url = urls.get(name);
    return url ? `${q}${url}${q}` : whole;
  });
  const tag = bridge(data);
  // 放进 <head> 开头（同一行）；没有 <head> 就放在 <html> 后或最前。
  const head = body.match(/<head[^>]*>/i);
  if (head?.index !== undefined) {
    const at = head.index + head[0].length;
    return body.slice(0, at) + tag + body.slice(at);
  }
  const root = body.match(/<html[^>]*>/i);
  if (root?.index !== undefined) {
    const at = root.index + root[0].length;
    return body.slice(0, at) + tag + body.slice(at);
  }
  const doctype = body.match(/^\s*<!doctype[^>]*>/i);
  if (doctype) return doctype[0] + tag + body.slice(doctype[0].length);
  return tag + body;
}

/** 不认识的库名（给面板提示用）。 */
export function unknownLibs(html: string): string[] {
  const out = new Set<string>();
  for (const m of html.matchAll(LIB_RE)) if (!LIBS[m[2]]) out.add(m[2]);
  return [...out];
}

// ---- 对话流里的画布工具项 ----

export const CANVAS_TOOLS = new Set(["canvas_create", "canvas_edit", "canvas_data"]);

export interface CanvasEvent {
  /** 工具项 id。 */
  itemId: string;
  tool: "canvas_create" | "canvas_edit" | "canvas_data";
  status: "running" | "completed" | "failed" | "cancelled" | "denied";
  /** 跑完之前 create 没有 id。 */
  canvasId: string | null;
  title: string | null;
  version: number | null;
  note: string | null;
  error: string | null;
}

const ID_RE = /\b(canvas-\d+)\b/;
const VERSION_RE = /\bv(\d+)\b/;
const TITLE_RE = /「([^」]*)」/;

type ToolLike = { id: string; name: string; arguments: unknown; output?: string; status: CanvasEvent["status"] };

export function canvasEvent(item: ToolLike): CanvasEvent | null {
  const call = calledTool({ name: item.name, arguments: (item.arguments ?? {}) as Record<string, unknown> });
  if (!CANVAS_TOOLS.has(call.name)) return null;
  const args = call.args;
  const out = item.output ?? "";
  const done = item.status === "completed";
  const str = (v: unknown) => (typeof v === "string" && v ? v : null);
  return {
    itemId: item.id,
    tool: call.name as CanvasEvent["tool"],
    status: item.status,
    canvasId: str(args.id) ?? (done ? (out.match(ID_RE)?.[1] ?? null) : null),
    title: str(args.title) ?? (done ? (out.match(TITLE_RE)?.[1] ?? null) : null),
    version: done ? Number(out.match(VERSION_RE)?.[1] ?? NaN) || null : null,
    note: str(args.note),
    error: item.status === "failed" ? out.replace(/^Error:\s*/, "") : null,
  };
}

/** 线程里所有画布工具项，按出现顺序。 */
export function canvasEvents(thread: ThreadState): CanvasEvent[] {
  const out: CanvasEvent[] = [];
  for (const turn of thread.turns) {
    for (const item of turn.items) {
      if (item.kind !== "tool") continue;
      const ev = canvasEvent(item as ToolLike);
      if (ev) out.push(ev);
    }
  }
  return out;
}

// ---- 交回 agent 的话（填进输入框，不自动发） ----

export function fixPrompt(title: string, id: string, err: { version: number; message: string; line: number | null }): string {
  const at = err.line ? `（v${err.version}.html 第 ${err.line} 行）` : `（v${err.version}）`;
  return `来自画布「${title}」（${id}）运行出错${at}：\n${err.message}\n请用 canvas_edit 修好。`;
}

export function sendPrompt(title: string, id: string, text: string): string {
  return `来自画布「${title}」（${id}）：${text}`;
}

/** 每块画布的标题和最新一条工具项（对话流里的卡片用）。 */
export function canvasIndex(events: CanvasEvent[]): { titles: Map<string, string>; latest: Map<string, string> } {
  const titles = new Map<string, string>();
  const latest = new Map<string, string>();
  for (const ev of events) {
    if (!ev.canvasId) continue;
    if (ev.title) titles.set(ev.canvasId, ev.title);
    latest.set(ev.canvasId, ev.itemId);
  }
  return { titles, latest };
}
