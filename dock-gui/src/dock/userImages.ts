// 用户消息里的图：本窗口发的有本地预览；刷新、重开 Dock、换台机器连远程之后
// 本地预览没了，就按内容摘要向 Dock 要像素（`item/image { threadId, digest }`）。
//
// 取回来的是 object URL，按「线程 id + 摘要」缓存：同一张图在一屏里只取一次。

import { dock } from "./client";

const cache = new Map<string, string>();
const inflight = new Map<string, Promise<string | null>>();

function key(threadId: string, digest: string): string {
  return `${threadId}#${digest}`;
}

/** 已经取过的图（object URL）。没取过返回 undefined。 */
export function cachedUserImage(threadId: string, digest: string): string | undefined {
  return cache.get(key(threadId, digest));
}

/**
 * 取一张用户图片的像素。没有像素就返回 null（老网关没这个能力、或图已经不在了）。
 *
 * 失败只返回 null，不弹错误：历史消息里的图取不到不该打断聊天。
 */
export function fetchUserImage(threadId: string, digest: string): Promise<string | null> {
  const k = key(threadId, digest);
  const hit = cache.get(k);
  if (hit) return Promise.resolve(hit);
  const running = inflight.get(k);
  if (running) return running;
  const p = dock
    .call<{ mimeType: string; data: string }>("item/image", { threadId, digest })
    .then((r) => {
      const bytes = base64Bytes(r.data);
      if (!bytes) return null;
      const url = URL.createObjectURL(new Blob([bytes], { type: r.mimeType }));
      cache.set(k, url);
      return url;
    })
    .catch(() => null)
    .finally(() => inflight.delete(k));
  inflight.set(k, p);
  return p;
}

/** base64 → 字节。 */
function base64Bytes(b64: string): Uint8Array<ArrayBuffer> | null {
  try {
    const raw = atob(b64);
    const out = new Uint8Array(new ArrayBuffer(raw.length));
    for (let i = 0; i < raw.length; i++) out[i] = raw.charCodeAt(i);
    return out;
  } catch {
    return null;
  }
}
