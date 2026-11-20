// 插件卡片的数据（`turn/cards/*`）：每个线程一份，第一次有人看这个线程时拉一次
// `turn/cards/list`，之后跟 `turn/cards/updated` 推送。自己一份小 store，不进会话大 store。

import { useSyncExternalStore } from "react";
import {
  EMPTY_TURN_CARDS,
  applyTurnCards,
  parseTurnCardsPush,
  seedTurnCards,
  type TurnCardsState,
} from "dock-core";
import { dock } from "../../dock/client";

const byThread = new Map<string, TurnCardsState>();
const loading = new Set<string>();
const listeners = new Set<() => void>();
let subscribed = false;

function emit() {
  for (const fn of listeners) fn();
}

function ensureSubscribed() {
  if (subscribed) return;
  subscribed = true;
  dock.onNotification((n) => {
    const push = parseTurnCardsPush(n.method, n.params);
    if (!push) return;
    const state = byThread.get(push.threadId) ?? EMPTY_TURN_CARDS;
    byThread.set(push.threadId, applyTurnCards(state, push.turnId, push.cards));
    emit();
  });
}

function load(threadId: string) {
  if (byThread.has(threadId) || loading.has(threadId)) return;
  // 老 Dock 没有这个能力：当没有卡片。
  if (!dock.capabilities.turnCards) return;
  loading.add(threadId);
  dock
    .call<{ turns: unknown[] }>("turn/cards/list", { threadId })
    .then((r) => {
      // 拉的这段时间里来过推送：推送更新，保留它。
      const pushed = byThread.get(threadId);
      const seeded = seedTurnCards(r.turns);
      byThread.set(threadId, pushed ? { byTurn: { ...seeded.byTurn, ...pushed.byTurn } } : seeded);
      emit();
    })
    .catch(() => {})
    .finally(() => loading.delete(threadId));
}

export function useTurnCards(threadId: string): TurnCardsState {
  ensureSubscribed();
  load(threadId);
  return useSyncExternalStore(
    (fn) => {
      listeners.add(fn);
      return () => listeners.delete(fn);
    },
    () => byThread.get(threadId) ?? EMPTY_TURN_CARDS,
  );
}

/** 点卡片上的按钮：回一句给用户的提示（可能没有）。 */
export const cardAction = (threadId: string, cardId: string, action: string) =>
  dock
    .call<{ toast?: string | null }>("turn/cards/action", { threadId, cardId, action })
    .then((r) => r.toast ?? null);
