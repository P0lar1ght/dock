// 插件挂在一轮回答下面的卡片（`turn/cards/list` / `turn/cards/updated`，能力 `turnCards`；
// Dock 插件用 Rhai `host.card` 发）。按钮点了走 `turn/cards/action`（受信连接）。纯函数。

import { parseView, type Tone, type ViewNode } from './plugins.ts';

export interface TurnCard {
  /** 线上的 id，`turn/cards/action` 用它。 */
  id: string;
  /** 发卡的插件 id。 */
  plugin: string;
  title: string;
  text: string;
  tone: Tone;
  view: ViewNode | null;
  actions: { id: string; label: string }[];
  /** 插件还在、按钮还能点。插件停了卡片留着，按钮失效。 */
  live: boolean;
}

export interface TurnCardsState {
  /** 按 `turnId`（同 `item/user_message` 的 `turnId`）。 */
  byTurn: Record<string, TurnCard[]>;
}

export const EMPTY_TURN_CARDS: TurnCardsState = { byTurn: {} };

const TONES: readonly Tone[] = ['default', 'muted', 'accent', 'success', 'warning', 'danger'];

type Raw = Record<string, unknown>;
const obj = (v: unknown): Raw => (v && typeof v === 'object' && !Array.isArray(v) ? (v as Raw) : {});
const str = (v: unknown): string => (typeof v === 'string' ? v : '');

/** 一张卡；没有 id 或标题的不要。 */
export function parseTurnCard(raw: unknown): TurnCard | null {
  const o = obj(raw);
  const id = str(o.id);
  const title = str(o.title);
  if (!id || !title) return null;
  const tone = TONES.includes(o.tone as Tone) ? (o.tone as Tone) : 'default';
  const actions = (Array.isArray(o.actions) ? o.actions : [])
    .map((a) => {
      const x = obj(a);
      const aid = str(x.id);
      return aid ? { id: aid, label: str(x.label) || aid } : null;
    })
    .filter((a): a is { id: string; label: string } => a !== null);
  return {
    id,
    plugin: str(o.plugin),
    title,
    text: str(o.text),
    tone,
    view: o.view == null ? null : parseView(o.view),
    actions,
    live: o.live === true,
  };
}

function cardsOf(raw: unknown): TurnCard[] {
  return (Array.isArray(raw) ? raw : []).map(parseTurnCard).filter((c): c is TurnCard => c !== null);
}

/** `turn/cards/list` 的结果铺底（整份换掉）。 */
export function seedTurnCards(turns: readonly unknown[]): TurnCardsState {
  const byTurn: Record<string, TurnCard[]> = {};
  for (const t of turns) {
    const o = obj(t);
    const turnId = str(o.turnId);
    const cards = cardsOf(o.cards);
    if (turnId && cards.length > 0) byTurn[turnId] = cards;
  }
  return { byTurn };
}

/** `turn/cards/updated` 推送 → `{ threadId, turnId, cards }`；别的通知返回 `null`。 */
export function parseTurnCardsPush(
  method: string,
  params: unknown,
): { threadId: string; turnId: string; cards: TurnCard[] } | null {
  if (method !== 'turn/cards/updated') return null;
  const o = obj(params);
  const threadId = str(o.threadId);
  const turnId = str(o.turnId);
  if (!threadId || !turnId) return null;
  return { threadId, turnId, cards: cardsOf(o.cards) };
}

/** 推送并进状态：这一轮整份换掉，空了就删掉这一轮。 */
export function applyTurnCards(state: TurnCardsState, turnId: string, cards: TurnCard[]): TurnCardsState {
  const byTurn = { ...state.byTurn };
  if (cards.length === 0) delete byTurn[turnId];
  else byTurn[turnId] = cards;
  return { byTurn };
}

export function turnCardsOf(state: TurnCardsState, turnId: string): TurnCard[] {
  return state.byTurn[turnId] ?? [];
}
