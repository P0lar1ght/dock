// 一个线程每轮改了哪些文件（`turn/changes/list` / `turn/changes/updated`，能力 `turnChanges`）。
// Dock 只认 agent 的编辑工具（`search_replace` / `write_file`），bash 里改的不算。纯函数、不可变更新。

import type { DockEvent, TurnChanges } from './events.ts';
import { parseTurnChanges } from './events.ts';

export interface TurnChangesState {
  /** 按 `turnId`（同 `item/user_message` 的 `turnId`）。 */
  byTurn: Record<string, TurnChanges>;
}

export const EMPTY_TURN_CHANGES: TurnChangesState = { byTurn: {} };

/** `turn/changes/list` 的结果铺底（整份换掉：撤回过的轮次不在里面了）。 */
export function seedTurnChanges(turns: readonly Record<string, unknown>[]): TurnChangesState {
  const byTurn: Record<string, TurnChanges> = {};
  for (const raw of turns) {
    const changes = parseTurnChanges(raw);
    if (changes) byTurn[changes.turnId] = changes;
  }
  return { byTurn };
}

/** 实时推送。别的事件原样返回。 */
export function reduceTurnChanges(state: TurnChangesState, event: DockEvent): TurnChangesState {
  if (event.method !== 'turn/changes/updated') return state;
  return { byTurn: { ...state.byTurn, [event.changes.turnId]: event.changes } };
}

/** 这一轮有没有要给人看的改动（全改回原样的不算）。 */
export function turnChangesOf(state: TurnChangesState, turnId: string): TurnChanges | null {
  const changes = state.byTurn[turnId];
  return changes && changes.files.length > 0 ? changes : null;
}
