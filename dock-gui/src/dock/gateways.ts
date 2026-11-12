// 同时开着的网关：本机一份，加上打开的每台远程。每台一个 DockClient + DockStore，套接字一直连着。
// 主界面只是一个指针（`store` / `dock` 代理到当前这份）。切走不关进程、不拆别的隧道、不重载页面。
// 独立窗（`?session` / `?panel`）只连自己那一台，不把每条远程都打开。

import { listen } from "@tauri-apps/api/event";
import { useSyncExternalStore } from "react";
import { bindForegroundClient, DockClient } from "./client";
import { gatewayClose, gatewayFocus, gatewayOpen, remoteList, type Remote } from "./remote";
import {
  bindCurrentStore,
  currentStore,
  DockStore,
  type AppState,
  type CanvasState,
} from "./store";
import type { ComposerSettings } from "../model";
import type { SubagentsState, ThreadState, TurnChangesState, WorkflowsState } from "dock-core";

/** 后台需要看一眼：出错优先，其次有人在等，其次有一轮在跑。 */
export type GatewayAttention = "error" | "waiting" | "running" | null;

export interface GatewayView {
  /** `"local"` 或远程连接 id。 */
  id: string;
  name: string;
  remote: Remote | null;
  attention: GatewayAttention;
}

export interface GatewayHost {
  /** 主界面正在看的。`"local"` = 本机。 */
  id: string;
  entries: GatewayView[];
}

interface Entry {
  id: string;
  name: string;
  remote: Remote | null;
  store: DockStore;
  client: DockClient;
  attention: GatewayAttention;
  unwatch: () => void;
}

const entries = new Map<string, Entry>();
let order: string[] = [];
const hostListeners = new Set<() => void>();
let snap: GatewayHost = { id: "local", entries: [] };

const PRESET_KEY = "dock-gui.newChatPreset";
const LAST_SESSION_KEY = "dock-gui.lastSession";

function params() {
  return new URLSearchParams(location.search);
}

/** 主窗口没有 `session` / `panel`。拖出去的窗各自连一台。 */
function isMainWindow() {
  const q = params();
  return !q.has("session") && !q.has("panel");
}

function queryGateway(): string | null {
  return params().get("gateway") || null;
}

function attentionOf(store: DockStore): GatewayAttention {
  const app = store.getApp();
  if (app.fatal || app.exited) return "error";
  if (app.waiting.size) return "waiting";
  if (app.running.size) return "running";
  return null;
}

/** 只有需要亮点的变化才通知界面。后台每个 token 不该让前台整页重画。 */
function signature(store: DockStore): string {
  const app = store.getApp();
  return `${app.connection}|${app.running.size}|${app.waiting.size}|${app.fatal ?? ""}|${app.exited ?? ""}|${app.booted}`;
}

function watch(entry: Entry) {
  let prev = "";
  entry.unwatch = entry.store.subscribe(() => {
    const next = signature(entry.store);
    if (next === prev) return;
    prev = next;
    entry.attention = attentionOf(entry.store);
    publish();
  });
}

function views(): GatewayView[] {
  return order.filter((id) => entries.has(id)).map((id) => {
    const e = entries.get(id)!;
    return { id: e.id, name: e.name, remote: e.remote, attention: e.attention };
  });
}

function foregroundKey(): string {
  return currentStore().client.gatewayId ?? "local";
}

function boundId(): string {
  try {
    return entries.size ? foregroundKey() : "local";
  } catch {
    // 第一台 store 在 `bind` 之前就会连上并通知：这时还没有前台。
    return "local";
  }
}

function publish() {
  snap = { id: boundId(), entries: views() };
  hostListeners.forEach((fn) => fn());
}

export function subscribeHost(fn: () => void) {
  hostListeners.add(fn);
  return () => hostListeners.delete(fn);
}

function getHostSnap() {
  return snap;
}

export function foregroundStore(): DockStore {
  return currentStore();
}

function bind(entry: Entry) {
  bindCurrentStore(entry.store);
  bindForegroundClient(() => entry.client);
}

function make(id: string, name: string, remote: Remote | null, client: DockClient): Entry {
  const store = new DockStore(client, remote);
  store.retain();
  const entry: Entry = { id, name, remote, store, client, attention: null, unwatch: () => {} };
  entries.set(id, entry);
  if (!order.includes(id)) order.push(id);
  watch(entry);
  return entry;
}

function ensure(remote: Remote) {
  const existing = entries.get(remote.id);
  if (existing) {
    existing.remote = remote;
    existing.name = remote.name;
    existing.store.setRemote(remote);
    return;
  }
  make(remote.id, remote.name, remote, new DockClient(remote.id));
}

/** 主界面改看哪一台。`null` / `"local"` = 本机。离开的那台照旧连着，只是不再算「正在看」。 */
export function focusGateway(id: string | null) {
  const next = !id || id === "local" ? "local" : id;
  const entry = entries.get(next);
  if (!entry || foregroundKey() === next) return;
  currentStore().focus([]);
  bind(entry);
  publish();
  void gatewayFocus(next === "local" ? null : next).catch((e) => currentStore().reportError(e));
}

/** 打开一条远程并切过去。本机 Dock 不动。 */
export async function openGateway(id: string) {
  await gatewayOpen(id);
  const list = await remoteList();
  const remote = list.remotes.find((r) => r.id === id);
  if (!remote) throw new Error("没有这个连接");
  ensure(remote);
  focusGateway(id);
  publish();
}

/** 关掉一条远程的套接字和隧道。远端进程继续跑。本机关不掉。 */
export async function closeGateway(id: string) {
  if (!id || id === "local") return;
  const entry = entries.get(id);
  if (!entry) {
    await gatewayClose(id);
    return;
  }
  const was = foregroundKey() === id;
  if (was) entry.store.focus([]);
  entry.unwatch();
  entry.store.release();
  entries.delete(id);
  order = order.filter((x) => x !== id);
  if (was) {
    const local = entries.get("local");
    if (local) bind(local);
  }
  publish();
  await gatewayClose(id);
}

function drop(id: string) {
  const entry = entries.get(id);
  if (!entry) return;
  const was = foregroundKey() === id;
  entry.unwatch();
  entry.store.release();
  entries.delete(id);
  order = order.filter((x) => x !== id);
  if (was) {
    const local = entries.get("local");
    if (local) {
      bind(local);
      void gatewayFocus(null).catch(() => {});
    }
  }
  publish();
}

function onReconnect(id: string) {
  const entry = entries.get(id);
  if (!entry) return;
  entry.client.bounce();
  remoteList()
    .then((r) => {
      const remote = r.remotes.find((x) => x.id === id);
      if (!remote || !entries.has(id)) return;
      entry.remote = remote;
      entry.name = remote.name;
      entry.store.setRemote(remote);
      publish();
    })
    .catch(() => {});
}

function listenHost() {
  listen<string>("dock://gateway-reconnect", (e) => onReconnect(e.payload)).catch(() => {});
  listen<string>("dock://gateway-closed", (e) => drop(e.payload)).catch(() => {});
}

function bootMain() {
  make("local", "本机 Dock", null, new DockClient(null));
  bind(entries.get("local")!);
  publish();
  remoteList()
    .then((r) => {
      for (const id of r.opened) {
        const remote = r.remotes.find((x) => x.id === id);
        if (remote) ensure(remote);
      }
      if (r.foreground && entries.has(r.foreground)) focusGateway(r.foreground);
      else publish();
    })
    .catch(() => {});
  listenHost();
}

/** 拖出的窗只连 URL 上那一台，不把主窗口打开的每条远程都连一遍。 */
function bootChild() {
  const id = queryGateway();
  const key = id ?? "local";
  const entry = make(key, id ? "远程 Dock" : "本机 Dock", null, new DockClient(id));
  bind(entry);
  publish();
  if (!id) return;
  remoteList()
    .then((r) => {
      const remote = r.remotes.find((x) => x.id === id);
      if (!remote || !entries.has(id)) return;
      entry.remote = remote;
      entry.name = remote.name;
      entry.store.setRemote(remote);
      publish();
    })
    .catch(() => {});
  listen<string>("dock://gateway-reconnect", (e) => {
    if (e.payload === id) entry.client.bounce();
  }).catch(() => {});
  listen<string>("dock://gateway-closed", (e) => {
    if (e.payload === id) entry.client.close();
  }).catch(() => {});
}

let booted = false;

/**
 * 模块求值时还不能 `new DockStore`：`store.ts` 再导出本文件，求值顺序会先跑到这里，
 * 类还在暂时死区里。入口在两边的模块都求值完之后再调。
 */
export function bootGateways() {
  if (booted) return;
  booted = true;
  if (isMainWindow()) bootMain();
  else bootChild();
}

function scoped(base: string, gw: string) {
  return gw && gw !== "local" ? `${base}@${gw}` : base;
}

/** 当前前台的网关 id；还没接上时当本机。给非 React 的调用（开窗）用。 */
export function gatewayArg(): string | null {
  try {
    const id = foregroundKey();
    return id === "local" ? null : id;
  } catch {
    return null;
  }
}

/** 新对话默认预设的 localStorage key。本机沿用原来的，远程加 `@id`。 */
export function presetKey(gw = gatewayArg() ?? "local") {
  return scoped(PRESET_KEY, gw);
}

/** 上次在看的会话。本机沿用原来的，远程加 `@id`。 */
export function lastSessionKey(gw = gatewayArg() ?? "local") {
  return scoped(LAST_SESSION_KEY, gw);
}

export function useGateways(): GatewayHost {
  return useSyncExternalStore(subscribeHost, getHostSnap);
}

/** `"local"` 或远程连接 id。 */
export function useGatewayId(): string {
  return useGateways().id;
}

export function useForegroundStore(): DockStore {
  return useSyncExternalStore(subscribeHost, foregroundStore);
}

export function useCanvas(): CanvasState {
  const s = useForegroundStore();
  return useSyncExternalStore(s.subscribe, s.getCanvas);
}

export function useApp(): AppState {
  const s = useForegroundStore();
  return useSyncExternalStore(s.subscribe, s.getApp);
}

export function useThread(id: string | null): ThreadState {
  const s = useForegroundStore();
  return useSyncExternalStore(s.subscribe, () => s.getThread(id));
}

export function useSubagents(id: string | null): SubagentsState {
  const s = useForegroundStore();
  return useSyncExternalStore(s.subscribe, () => s.getSubagents(id));
}

export function useTurnChanges(id: string | null): TurnChangesState {
  const s = useForegroundStore();
  return useSyncExternalStore(s.subscribe, () => s.getTurnChanges(id));
}

export function useWorkflows(id: string | null): WorkflowsState {
  const s = useForegroundStore();
  return useSyncExternalStore(s.subscribe, () => s.getWorkflows(id));
}

export function useEnv(id: string | null): ComposerSettings {
  const s = useForegroundStore();
  return useSyncExternalStore(s.subscribe, () => s.getEnv(id));
}
