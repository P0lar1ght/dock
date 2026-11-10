// 分块拉文件字节（dock.1 `fs/read { raw }`）：先拉一小块让首屏快出来，后面的块越拉越大，
// 同时挂两个请求把往返藏起来。字节不在这里解码：base64 原样交给 Worker。

import { dock } from "../../dock/client";
import { base64Bytes, decodeBase64 } from "./bytes";

export interface RawChunk {
  offset: number;
  /** base64。 */
  data: string;
  size: number;
  modifiedMs?: number;
  eof: boolean;
}

/** 网关支不支持分块读（老 Dock 没有）。 */
export const canReadRaw = () => !!dock.capabilities.workspaceFilesRaw;

const FIRST = 256 * 1024;
/** 本机一块最多 4 MB（网关上限）；远程走 SSH 隧道，块小点，别把其它消息堵在后面。 */
const MAX_LOCAL = 4 * 1024 * 1024;
const MAX_REMOTE = 1024 * 1024;
const IN_FLIGHT = 2;

export class FileChangedError extends Error {
  constructor() {
    super("读的时候文件变了");
  }
}

/**
 * 从 `from` 开始按顺序把块交给 `onChunk`，读到末尾、读满 `limit` 字节或 `signal` 取消为止。
 * 中途文件大小 / 修改时间变了抛 {@link FileChangedError}。回读到哪儿、文件多大。
 */
export async function pumpFile(opts: {
  threadId: string;
  path: string;
  from?: number;
  limit?: number;
  /** 连的是远程 Dock（`useApp().remote`）。 */
  remote: boolean;
  signal: AbortSignal;
  onChunk: (chunk: RawChunk) => void;
}): Promise<{ size: number; end: number }> {
  const { threadId, path, signal, onChunk } = opts;
  const from = opts.from ?? 0;
  const stop = from + (opts.limit ?? Number.POSITIVE_INFINITY);
  const read = (offset: number, length: number) =>
    dock.call<RawChunk>("fs/read", { threadId, path, raw: true, offset, length });

  // 第一块单独拉：拿到文件大小才知道后面要几块。
  const first = await read(from, Math.min(FIRST, stop - from));
  if (signal.aborted) return { size: first.size, end: from };
  onChunk(first);
  let end = from + base64Bytes(first.data);
  const total = Math.min(first.size, stop);

  let next = end;
  let length = FIRST;
  const queue: Promise<RawChunk>[] = [];
  const issue = () => {
    while (queue.length < IN_FLIGHT && next < total) {
      length = Math.min(length * 2, opts.remote ? MAX_REMOTE : MAX_LOCAL);
      const n = Math.min(length, total - next);
      queue.push(read(next, n));
      next += n;
    }
  };
  try {
    issue();
    while (queue.length > 0) {
      const chunk = await queue.shift()!;
      if (signal.aborted) break;
      if (chunk.size !== first.size || chunk.modifiedMs !== first.modifiedMs) throw new FileChangedError();
      onChunk(chunk);
      end = chunk.offset + base64Bytes(chunk.data);
      issue();
    }
  } finally {
    // 取消或出错时还挂着的请求：结果不要了，也别冒成未处理的 rejection。
    for (const p of queue) p.catch(() => {});
  }
  return { size: first.size, end };
}

/** 读 `[begin, end)` 这一段字节（超过网关一块的上限就分几次）。PDF 按需取段用。 */
export async function readRange(threadId: string, path: string, begin: number, end: number): Promise<Uint8Array> {
  const out = new Uint8Array(Math.max(0, end - begin));
  let offset = begin;
  while (offset < end) {
    const chunk = await dock.call<RawChunk>("fs/read", {
      threadId,
      path,
      raw: true,
      offset,
      length: Math.min(MAX_LOCAL, end - offset),
    });
    const bytes = decodeBase64(chunk.data);
    if (bytes.length === 0) break;
    out.set(bytes, offset - begin);
    offset += bytes.length;
  }
  return out;
}
