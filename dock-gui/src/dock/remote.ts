// 远程 Dock 的连接名单（Tauri 侧 remote.rs 管；令牌只在钥匙串里，这里拿不到）。
// `active` 空 = 本机 Dock。换连接后 Tauri 发 `dock://connection`，所有窗口整页重载。
// 两种连法：直连 `url`，或 `ssh`——GUI 自己开 SSH 隧道（tunnel.rs），远端没在跑时可经 SSH 启动。

import { invoke } from "@tauri-apps/api/core";

export interface SshTarget {
  host: string;
  user: string;
  port?: number | null;
  /** 私钥文件；空着走 ssh-agent / ~/.ssh/config。 */
  identity?: string | null;
  /** 远端 `dock serve --remote --bind 127.0.0.1:<这个>`。 */
  remotePort: number;
  /** 远端 Dock 没在跑时经 SSH 跑这条，如 `systemctl start dock`。 */
  startCommand?: string | null;
}

export interface Remote {
  id: string;
  name: string;
  url: string;
  ssh?: SshTarget | null;
}

export interface Remotes {
  active: string | null;
  remotes: Remote[];
}

/** 列表 / 横条上显示的位置：直连给地址，SSH 给 `ssh user@host`。 */
export const remoteWhere = (r: Remote) =>
  r.ssh ? `ssh ${r.ssh.user}@${r.ssh.host}${r.ssh.port && r.ssh.port !== 22 ? `:${r.ssh.port}` : ""}` : r.url;

/** Tauri 侧「不会自己好」的错误前缀（remote.rs `FATAL`）；给人看时去掉。 */
export const FATAL_PREFIX = "[fatal] ";
const plain = (e: unknown) => new Error(String(e).replace(FATAL_PREFIX, ""));

export const remoteList = () => invoke<Remotes>("remote_list");

export const remoteSave = (r: { id?: string; name: string; url: string; ssh?: SshTarget | null; token?: string }) =>
  invoke<Remote>("remote_save", {
    id: r.id ?? null,
    name: r.name,
    url: r.url,
    ssh: r.ssh ?? null,
    token: r.token || null,
  });

export const remoteDelete = (id: string) => invoke<void>("remote_delete", { id });

/** 换当前连接（`null` = 本机）。成功后各窗口会收到 `dock://connection` 重载。 */
export const connectionUse = (id: string | null) => invoke<void>("connection_use", { id });

interface Conn {
  ws: string;
  token?: string;
  tunnel?: number;
}

/**
 * 试连一个远程 Dock：鉴权 + `initialize`，回 Dock 的名字和版本。
 * 传 `id` 用已存的配置；传 `url` / `ssh` + `token` 试还没存的。走 SSH 的开临时隧道，测完关掉。
 */
export async function remoteTest(
  target: { id: string } | { url: string; token: string } | { ssh: SshTarget; token: string },
): Promise<{ name?: string; version?: string }> {
  const conn: Conn =
    "id" in target
      ? await invoke<Conn>("remote_connection", { id: target.id }).catch((e: unknown) => {
          throw plain(e);
        })
      : "ssh" in target
        ? {
            ...(await invoke<Conn>("remote_tunnel_open", { ssh: target.ssh }).catch((e: unknown) => {
              throw plain(e);
            })),
            token: target.token.trim(),
          }
        : { ws: await invoke<string>("remote_ws_url", { url: target.url }), token: target.token.trim() };
  try {
    return await probe(conn.ws, conn.token ?? "");
  } finally {
    if (conn.tunnel !== undefined) void invoke("remote_tunnel_close", { tunnel: conn.tunnel });
  }
}

async function probe(ws: string, token: string): Promise<{ name?: string; version?: string }> {
  if (!token) throw new Error("要填设备令牌");
  const socket = new WebSocket(ws);
  const timer = window.setTimeout(() => socket.close(), 10_000);
  try {
    await new Promise<void>((resolve, reject) => {
      socket.onopen = () => resolve();
      socket.onerror = () => reject(new Error(`连不上 ${ws}`));
      socket.onclose = () => reject(new Error(`连不上 ${ws}`));
    });
    const call = (id: number, method: string, params: object) =>
      new Promise<Record<string, unknown>>((resolve, reject) => {
        socket.onmessage = (ev) => {
          const frame = JSON.parse(String(ev.data));
          if (frame.id !== id) return;
          if (frame.error) reject(new Error(frame.error.message ?? "连接被拒绝"));
          else resolve(frame.result ?? {});
        };
        socket.onclose = () => reject(new Error("连接被关闭"));
        socket.send(JSON.stringify({ id, method, params }));
      });
    await call(1, "connection/authenticate", { token });
    const init = await call(2, "initialize", {});
    return (init.serverInfo as { name?: string; version?: string }) ?? {};
  } finally {
    window.clearTimeout(timer);
    socket.onclose = null;
    socket.close();
  }
}
