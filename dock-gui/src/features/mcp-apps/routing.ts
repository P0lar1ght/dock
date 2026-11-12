// 内联（Gen UI）还是常驻面板（UI Plugin）：宿主按标准信号 + 用户动作决定，不要服务器声明。
// - 默认内联：一次调用一份界面；
// - 界面请求 `fullscreen`、或用户点「在面板打开」→ 开常驻面板，键 (服务器, ui:// 资源, 项目)；
// - 面板开着时，同一项目里挂同一资源的调用都送进面板，对话流只留一条小条。
//
// 面板 kind：`mcp-app:<server>:<uri>`（两段各自 encodeURIComponent）；项目取当前会话的 cwd，
// 不进 kind——换到别的项目的会话，面板就换成那个项目的那一份。
//
// 跨窗口：面板能拖成独立窗（另一个 WebView，各有一份这个模块）。每扇窗把自己开着的面板广播出去
// （Tauri 事件），别的窗口记成「远端」：内联卡照样知道面板开着，调用经事件送过去；点「在面板打开」
// 聚焦那扇窗，不在本窗再开一份。新开的窗一出生就问一遍谁开着什么，并收到主窗口最后送过的那次调用。

import { useSyncExternalStore } from "react";
import { emit, listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { invoke } from "@tauri-apps/api/core";
import type { McpAppTool } from "dock-core";
import type { ToolStatus } from "dock-core";
import { IN_TAURI } from "../../components/chrome";
import { gatewayArg } from "../../dock/store";

export const MCP_APP_PANEL = "mcp-app";

export const panelKind = (server: string, uri: string) =>
  `${MCP_APP_PANEL}:${encodeURIComponent(server)}:${encodeURIComponent(uri)}`;

/** `mcp-app:<server>:<uri>` → 两段；别的 kind 回 null。 */
export function parsePanelKind(kind: string): { server: string; uri: string } | null {
  if (!kind.startsWith(`${MCP_APP_PANEL}:`)) return null;
  const [server, uri] = kind.slice(MCP_APP_PANEL.length + 1).split(":");
  if (!server || !uri) return null;
  try {
    return { server: decodeURIComponent(server), uri: decodeURIComponent(uri) };
  } catch {
    return null;
  }
}

/** 面板去重的键：同一服务器、同一资源、同一项目。 */
export const sinkKey = (server: string, uri: string, cwd: string | null) => `${server}\n${uri}\n${cwd ?? ""}`;

/** 送进面板的一次调用（内联卡每次状态变了都再送一遍，同一个 itemId）。 */
export interface RoutedCall {
  threadId: string;
  itemId: string;
  tool: McpAppTool;
  args: Record<string, unknown>;
  status: ToolStatus;
}

type Sink = (call: RoutedCall) => void;

/** 面板是哪个 kind、跟着哪个会话：别的窗口要聚焦它时用（Rust 按这两样定窗口 label）。 */
export interface SinkInfo {
  kind: string;
  sessionId: string | null;
}

interface Local {
  sink: Sink;
  info: SinkInfo;
}

interface Remote extends SinkInfo {
  window: string;
}

const sinks = new Map<string, Local>();
/** 别的窗口开着的面板（键同 sinks）。 */
const remote = new Map<string, Remote>();
/** 每个键最后送出的那次调用：面板拖成独立窗后补给新窗。 */
const lastCall = new Map<string, RoutedCall>();
/** 面板还没挂上（刚请求打开）时先存着，挂上后取走。 */
const seeds = new Map<string, RoutedCall>();
const listeners = new Set<() => void>();
let version = 0;

function changed() {
  version += 1;
  listeners.forEach((fn) => fn());
}

/** 面板挂上时登记（同一个键只有一扇）。返回取消函数。 */
export function registerSink(key: string, sink: Sink, info: SinkInfo): () => void {
  const local = { sink, info };
  sinks.set(key, local);
  changed();
  announce(key, info, true);
  const seed = seeds.get(key);
  if (seed) {
    seeds.delete(key);
    sink(seed);
  }
  return () => {
    if (sinks.get(key) === local) {
      sinks.delete(key);
      changed();
      announce(key, info, false);
    }
  };
}

export const hasSink = (key: string) => sinks.has(key) || remote.has(key);

/** 送进开着的面板（本窗或别的窗）；都没开就回 false。 */
export function route(key: string, call: RoutedCall): boolean {
  lastCall.set(key, call);
  const local = sinks.get(key);
  if (local) {
    local.sink(call);
    return true;
  }
  const r = remote.get(key);
  if (!r) return false;
  send(key, call, r.window);
  return true;
}

/**
 * 面板开在别的窗口：把那扇窗提到前面（同一会话同一面板 Rust 只开一扇，已开就聚焦），
 * 带上这次调用。回 false = 没有别的窗口开着它。
 */
export function focusRemote(key: string, call: RoutedCall | null): boolean {
  const r = remote.get(key);
  if (!r) return false;
  if (call) route(key, call);
  void invoke("open_panel_window", { panel: r.kind, sessionId: r.sessionId ?? "", gateway: gatewayArg(), x: 0, y: 0 }).catch(() => {});
  return true;
}

/** 请求打开面板时带上这次调用：面板挂上后第一件事就是显示它。 */
export function seed(key: string, call: RoutedCall) {
  if (!route(key, call)) seeds.set(key, call);
}

/** 这个面板开在哪：本窗、别的窗口、没开（订阅：面板开关时重渲染）。 */
export function useSinkOpen(key: string): "here" | "window" | null {
  useSyncExternalStore(
    (fn) => {
      listeners.add(fn);
      return () => {
        listeners.delete(fn);
      };
    },
    () => version,
  );
  return sinks.has(key) ? "here" : remote.has(key) ? "window" : null;
}

// ---- 跨窗口（Tauri 事件；不在 Tauri 里时什么也不做） ----

const BUS = { sink: "mcp-app/sink", query: "mcp-app/query", route: "mcp-app/route" };
const me = IN_TAURI ? getCurrentWindow().label : "main";

interface SinkEvent extends SinkInfo {
  key: string;
  window: string;
  open: boolean;
}

function announce(key: string, info: SinkInfo, open: boolean) {
  if (IN_TAURI) void emit(BUS.sink, { key, ...info, window: me, open } satisfies SinkEvent).catch(() => {});
}

function send(key: string, call: RoutedCall, to: string) {
  if (IN_TAURI) void emit(BUS.route, { key, call, to }).catch(() => {});
}

if (IN_TAURI) {
  void listen<SinkEvent>(BUS.sink, ({ payload: p }) => {
    if (p.window === me) return;
    if (p.open) {
      remote.set(p.key, { kind: p.kind, sessionId: p.sessionId, window: p.window });
      // 刚拖出去的面板：把这边最后送过的那次调用补过去（同一次调用面板按 itemId 认，重复无妨）。
      const last = lastCall.get(p.key);
      if (last) send(p.key, last, p.window);
    } else if (remote.get(p.key)?.window === p.window) {
      remote.delete(p.key);
    }
    changed();
  });
  void listen<{ window: string }>(BUS.query, ({ payload }) => {
    if (payload.window === me) return;
    for (const [key, local] of sinks) announce(key, local.info, true);
  });
  void listen<{ key: string; call: RoutedCall; to: string }>(BUS.route, ({ payload }) => {
    if (payload.to === me) sinks.get(payload.key)?.sink(payload.call);
  });
  // 别的窗口关了（Rust 在窗口销毁时广播）：它开着的面板都算关了。关窗那扇自己的 pagehide 广播
  // 在 WebView 拆掉时常常发不出去，靠这条兜住。
  void listen<string>("window/destroyed", ({ payload: label }) => {
    let any = false;
    for (const [key, r] of remote) {
      if (r.window === label) {
        remote.delete(key);
        any = true;
      }
    }
    if (any) changed();
  });
  // 新窗口：问一遍别的窗口开着什么面板。
  void emit(BUS.query, { window: me }).catch(() => {});
  // 关窗：开着的面板一并报关（卸载副作用在关窗时不一定跑得到）。
  window.addEventListener("pagehide", () => {
    for (const [key, local] of sinks) announce(key, local.info, false);
  });
}
