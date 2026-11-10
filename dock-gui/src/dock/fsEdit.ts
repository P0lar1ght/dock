// 文件面板改工作区：dock.1 的 fs/write|mkdir|rename|trash（能力 workspaceEdit，只认受信连接）。
// 路径都相对会话 cwd。Dock 那边写盘成功后会记一条，agent 下一步收到「用户改了这些文件」。

import { dock, RpcError } from "./client";

/** 连着的 Dock 支不支持在面板里改文件（老 Dock 没有）。 */
export const canEditFiles = () => !!dock.capabilities.workspaceEdit;

export interface SavedFile {
  path: string;
  size: number;
  modifiedMs?: number;
}

/** 保存。`baseModifiedMs` 是打开时的修改时间：期间被改过（多半是 agent）就抛 conflict，不写。 */
export function saveFile(threadId: string, path: string, text: string, baseModifiedMs?: number): Promise<SavedFile> {
  return dock.call<SavedFile>("fs/write", { threadId, path, text, ...(baseModifiedMs ? { baseModifiedMs } : {}) });
}

/** 连着的 Dock 能不能按字节写（表格之类的二进制、大文件分块上传）。 */
export const canEditBytes = () => !!dock.capabilities.workspaceEditBytes;

/** 一次直接发的上限；再大就分块上传（网关一帧 16 MiB，base64 要大三分之一）。 */
const INLINE_MAX = 3 * 1024 * 1024;

/**
 * 按字节保存（表格文件）。小的一次发完；大的先 fs/upload 分块暂存（本机 2 MB、远程 1 MB 一块），
 * 再一次提交——提交时才比对修改时间、换上文件，传到一半断了磁盘上的文件不动。
 */
export async function saveBytes(
  threadId: string,
  path: string,
  bytes: Uint8Array,
  baseModifiedMs?: number,
  opts: { remote?: boolean; onProgress?: (sent: number, total: number) => void } = {},
): Promise<SavedFile> {
  const base = baseModifiedMs ? { baseModifiedMs } : {};
  if (bytes.length <= INLINE_MAX)
    return dock.call<SavedFile>("fs/write", { threadId, path, data: toBase64(bytes), ...base });
  const uploadId = `gui-${crypto.randomUUID()}`;
  const chunk = opts.remote ? 1024 * 1024 : 2 * 1024 * 1024;
  for (let offset = 0; offset < bytes.length; offset += chunk) {
    const piece = bytes.subarray(offset, offset + chunk);
    await dock.call("fs/upload", { threadId, uploadId, offset, data: toBase64(piece) });
    opts.onProgress?.(offset + piece.length, bytes.length);
  }
  // 带上总大小：Dock 对一下，丢了块就不写。
  return dock.call<SavedFile>("fs/write", { threadId, path, uploadId, size: bytes.length, ...base });
}

function toBase64(bytes: Uint8Array): string {
  const native = (bytes as unknown as { toBase64?: () => string }).toBase64;
  if (native) return native.call(bytes);
  let bin = "";
  for (let i = 0; i < bytes.length; i += 0x8000) bin += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(bin);
}

/** 新建文件（中间缺的文件夹一并建）；已存在就报错，不覆盖。 */
export function createFile(threadId: string, path: string, text = ""): Promise<SavedFile> {
  return dock.call<SavedFile>("fs/write", { threadId, path, text, create: true });
}

export function makeDir(threadId: string, path: string): Promise<{ path: string }> {
  return dock.call<{ path: string }>("fs/mkdir", { threadId, path });
}

/** 改名 / 移动；目标已存在就报错。回新的相对路径。 */
export function movePath(threadId: string, from: string, to: string): Promise<{ path: string }> {
  return dock.call<{ path: string }>("fs/rename", { threadId, from, to });
}

/** 移到系统废纸篓（能在访达里放回去）。 */
export function trashPath(threadId: string, path: string): Promise<void> {
  return dock.call("fs/trash", { threadId, path }).then(() => undefined);
}

/** 保存时撞上「期间被改过」。 */
export const isConflict = (e: unknown) => e instanceof RpcError && e.code === "conflict";

/** 错误给人看的那句（去掉方法名前缀）。 */
export function editError(e: unknown): string {
  if (e instanceof RpcError) return e.body.message;
  return e instanceof Error ? e.message : String(e);
}

/** `a/b/c.md` → `a/b`；顶层是 `""`。 */
export const parentOf = (path: string) => (path.includes("/") ? path.slice(0, path.lastIndexOf("/")) : "");
export const baseName = (path: string) => path.slice(path.lastIndexOf("/") + 1);
export const joinPath = (dir: string, name: string) => (dir ? `${dir}/${name}` : name);

/** `from` 改到 `to` 之后，`path`（它自己或它下面的）变成什么；不相干就原样回。 */
export function remapPath(path: string, from: string, to: string): string {
  if (path === from) return to;
  if (path.startsWith(`${from}/`)) return `${to}${path.slice(from.length)}`;
  return path;
}

/** `path` 是不是 `dir` 自己或在它下面。 */
export const isWithin = (path: string, dir: string) => path === dir || path.startsWith(`${dir}/`);
