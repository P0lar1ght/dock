// 改动卡上点了某个文件：右侧「审查」面板（刚打开或早就开着）切到那一轮，取到 diff 后滚到这个文件。
// 面板 kind 只带轮次（`changes:turn:<turnId>`），不带文件：带了的话每点一个文件面板就换一页重挂。

import { useSyncExternalStore } from "react";
import type { TurnChanges } from "dock-core";

export interface FocusRequest {
  sessionId: string;
  turnId: string;
  /** diff 里的路径（同改动列表的 `path`：工作区里是相对路径，外面是绝对路径）。 */
  path: string;
  /** 每次点击都不同：同一个文件再点一次也再滚一次。 */
  seq: number;
  /** 点的时刻：面板晚开时，太旧的请求不认。 */
  at: number;
}

let pending: FocusRequest | null = null;
let seq = 0;
const listeners = new Set<() => void>();

export function focusChangedFile(request: Omit<FocusRequest, "seq" | "at">) {
  pending = { ...request, seq: ++seq, at: Date.now() };
  listeners.forEach((l) => l());
}

export function useFocusRequest(): FocusRequest | null {
  return useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    () => pending,
  );
}

/** 一轮改动的签名：变了就该重取 diff。带上每个文件和它的行数——只看总数，换了个文件、
 *  总行数碰巧一样时会认不出来。 */
export function changesSignature(c: TurnChanges): string {
  return `${c.turnId}:${c.reverted}:${c.files.map((f) => `${f.path}+${f.added}-${f.removed}`).join("|")}`;
}
