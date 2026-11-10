// dock.1 的最小客户端：WS 上的 JSON-RPC + 通知分发。
//
// 连接步骤：向 Tauri 要 `ws` 地址和凭据 → `connection/authenticate` → `initialize`。
// 凭据：本机 Dock 是一张新 ticket（一次性）；远程 Dock 是设备令牌（钥匙串里的）。
// 断线后重新要凭据再连，已订阅的线程自动重订。令牌无效 / 被撤销就不再重连（`fatal`）。
// 不复用 embed-sdk：它是公开包，还没有多线程（thread/open|start|close）的方法，
// GUI 只需要很薄的一层。

import { invoke } from "@tauri-apps/api/core";
import { FATAL_PREFIX } from "./remote";

export interface RpcErrorBody {
  code: number;
  message: string;
  details?: { code?: string };
}

export class RpcError extends Error {
  constructor(
    public method: string,
    public body: RpcErrorBody,
  ) {
    super(`${method}: ${body.message}`);
  }
  /** 网关的应用层错误码，如 `thread_not_open`、`invalid_params`。 */
  get code(): string | undefined {
    return this.body.details?.code;
  }
}

export interface Notification {
  method: string;
  params: Record<string, unknown> & { threadId?: string };
}

export type ConnectionState = "connecting" | "open" | "closed";

type Pending = { resolve: (v: unknown) => void; reject: (e: unknown) => void; method: string };

interface Connection {
  ws: string;
  ticket?: string;
  token?: string;
}

const RECONNECT_MS = 1000;
/** 远程断线按次数退避，封顶这么久（网不好时别一秒撞一次代理）。 */
const REMOTE_RECONNECT_MAX_MS = 15_000;
/** 网关的关闭码：设备被撤销 / 同一连接鉴权失败太多次。 */
const CLOSE_REVOKED = 4401;
const CLOSE_TOO_MANY_ATTEMPTS = 4429;

export class DockClient {
  private socket: WebSocket | null = null;
  private nextId = 1;
  private pending = new Map<number, Pending>();
  private listeners = new Set<(n: Notification) => void>();
  private stateListeners = new Set<(s: ConnectionState) => void>();
  private subscribed = new Set<string>();
  private ready: Promise<void> | null = null;
  private closedByUser = false;
  state: ConnectionState = "closed";
  /** 不会自己好的失败（令牌无效 / 被撤销）：有值就不再重连，等用户去设置里处理。 */
  fatal: string | null = null;
  private remote = false;
  private attempts = 0;
  capabilities: Record<string, boolean> = {};
  /** `initialize` 回的 Dock 名字和版本（设置页「关于」用）。 */
  serverInfo: { name?: string; version?: string } = {};

  /** 连上并完成鉴权 / 初始化。重复调用复用同一次连接。 */
  connect(): Promise<void> {
    if (this.fatal) return Promise.reject(new Error(this.fatal));
    if (!this.ready) {
      this.closedByUser = false;
      this.ready = this.open().catch((e) => {
        this.ready = null;
        // 半路失败（鉴权被拒、initialize 出错）：这条连接不要了，别让它的 close 再排一次重连。
        const socket = this.socket;
        this.socket = null;
        socket?.close();
        this.setState("closed");
        if (e instanceof RpcError && e.code === "unauthenticated") this.fatal = e.body.message;
        this.reconnectLater();
        throw e;
      });
    }
    return this.ready;
  }

  close() {
    this.closedByUser = true;
    this.socket?.close();
  }

  onNotification(fn: (n: Notification) => void): () => void {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  }

  onState(fn: (s: ConnectionState) => void): () => void {
    this.stateListeners.add(fn);
    return () => this.stateListeners.delete(fn);
  }

  async call<T = unknown>(method: string, params: Record<string, unknown> = {}): Promise<T> {
    await this.connect();
    return this.send<T>(method, params);
  }

  /** 订阅一个线程的推送；重连后会自动重订。 */
  async subscribe(threadId: string) {
    this.subscribed.add(threadId);
    await this.call("thread/subscribe", { threadId });
  }

  async unsubscribe(threadId: string) {
    this.subscribed.delete(threadId);
    await this.call("thread/unsubscribe", { threadId });
  }

  private setState(s: ConnectionState) {
    this.state = s;
    this.stateListeners.forEach((fn) => fn(s));
  }

  private async open(): Promise<void> {
    this.setState("connecting");
    const conn = await invoke<Connection>("dock_connection").catch((e: unknown) => {
      // 不会自己好的（拿不到令牌、SSH 认证失败、远端 Dock 起不来）：重试只会再弹授权框 /
      // 再撞一次认证，交给用户处理。
      const message = String(e);
      if (message.startsWith(FATAL_PREFIX)) this.fatal = message.slice(FATAL_PREFIX.length);
      throw new Error(message.replace(FATAL_PREFIX, ""));
    });
    this.remote = conn.token !== undefined;
    await new Promise<void>((resolve, reject) => {
      const socket = new WebSocket(conn.ws);
      this.socket = socket;
      socket.onopen = () => resolve();
      socket.onerror = () => reject(new Error(`连不上 Dock（${conn.ws}）`));
      socket.onmessage = (ev) => this.receive(String(ev.data));
      socket.onclose = (ev) => this.dropped(socket, ev.code);
    });
    await this.send(
      "connection/authenticate",
      conn.token !== undefined ? { token: conn.token } : { ticket: conn.ticket },
    );
    const init = await this.send<{
      capabilities?: Record<string, boolean>;
      serverInfo?: { name?: string; version?: string };
    }>("initialize", {});
    this.capabilities = init.capabilities ?? {};
    this.serverInfo = init.serverInfo ?? {};
    // 重订之前订过的线程。Dock 重启过的话它们已经不开着了：订不上就丢掉，
    // 不能让一个过期的订阅把整条连接拖死（store 重连后会按新列表重订）。
    for (const threadId of [...this.subscribed]) {
      await this.send("thread/subscribe", { threadId }).catch(() => this.subscribed.delete(threadId));
    }
    this.attempts = 0;
    this.setState("open");
  }

  private send<T>(method: string, params: Record<string, unknown>): Promise<T> {
    const socket = this.socket;
    if (!socket || socket.readyState !== WebSocket.OPEN) {
      return Promise.reject(new Error("Dock 连接未就绪"));
    }
    const id = this.nextId++;
    return new Promise<T>((resolve, reject) => {
      this.pending.set(id, { resolve: resolve as (v: unknown) => void, reject, method });
      socket.send(JSON.stringify({ id, method, params }));
    });
  }

  private receive(text: string) {
    let frame: { id?: number; result?: unknown; error?: RpcErrorBody; method?: string; params?: unknown };
    try {
      frame = JSON.parse(text);
    } catch {
      return;
    }
    if (typeof frame.id === "number" && this.pending.has(frame.id)) {
      const p = this.pending.get(frame.id)!;
      this.pending.delete(frame.id);
      if (frame.error) p.reject(new RpcError(p.method, frame.error));
      else p.resolve(frame.result);
      return;
    }
    if (frame.method) {
      const n = { method: frame.method, params: (frame.params ?? {}) as Notification["params"] };
      this.listeners.forEach((fn) => fn(n));
    }
  }

  private dropped(socket: WebSocket, code?: number) {
    if (this.socket !== socket) return;
    this.socket = null;
    this.ready = null;
    for (const p of this.pending.values()) p.reject(new Error("Dock 连接已断开"));
    this.pending.clear();
    if (code === CLOSE_REVOKED) this.fatal = "这台设备的令牌已被撤销";
    else if (code === CLOSE_TOO_MANY_ATTEMPTS) this.fatal = "鉴权失败次数太多，连接被断开";
    this.setState("closed");
    this.reconnectLater();
  }

  /** 排一次重连。失败由 `connect` 自己再排下一次；`fatal` 了就停。 */
  private reconnectLater() {
    if (this.closedByUser || this.fatal) return;
    this.attempts += 1;
    const delay = this.remote
      ? Math.min(RECONNECT_MS * 2 ** (this.attempts - 1), REMOTE_RECONNECT_MAX_MS)
      : RECONNECT_MS;
    setTimeout(() => {
      if (!this.ready) this.connect().catch(() => {});
    }, delay);
  }
}

export const dock = new DockClient();
