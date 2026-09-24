// 一个线程的状态：按轮组织的时间线 + 还在等用户的交互。纯函数、不可变更新
// （每次返回新对象，没变的部分共享引用），UI 层可以直接按引用比较。
//
// 保真：工具的原始参数、完整输出都留着——脱敏、截断是展示层的事。

import type { DockEvent, ImageAttachment, Question, ToolEndStatus, TurnEndStatus } from './events.ts';
import { parseHistoryItem } from './events.ts';

export type TurnStatus = 'running' | TurnEndStatus;
export type ToolStatus = 'running' | ToolEndStatus;

export type TurnItem =
  | { kind: 'user'; id: string; text: string; at: number; attachments: ImageAttachment[] }
  /** 助手文字。工具 / 交互之后的文字另起一条，保持出现顺序。 */
  | { kind: 'text'; id: string; text: string }
  | {
      kind: 'tool';
      id: string;
      name: string;
      arguments: Record<string, unknown>;
      output: string;
      status: ToolStatus;
      startedAt: number;
      endedAt: number | null;
    }
  | {
      kind: 'permission';
      id: string;
      toolName: string;
      summary: string;
      /** `null` = 还在等用户；`cancelled` = 没等到回答这一轮就结束了。 */
      decision: 'approve' | 'deny' | 'cancelled' | null;
      always: boolean;
    }
  | { kind: 'question'; id: string; questions: Question[]; resolved: boolean }
  | {
      kind: 'plan';
      id: string;
      path: string;
      body: string;
      empty: boolean;
      decision: 'approve' | 'revise' | 'quit' | 'cancelled' | null;
    }
  | { kind: 'elicit'; id: string; server: string; message: string; heading: string; resolved: boolean };

export interface Turn {
  id: string;
  status: TurnStatus;
  /** 失败原因（`status === 'failed'`）。 */
  error: string | null;
  startedAt: number;
  endedAt: number | null;
  items: TurnItem[];
}

export interface ThreadState {
  /** 已经吃进来的最大 `seq`；不大于它的事件视为重复，跳过。 */
  seq: number;
  turns: Turn[];
}

export const EMPTY_THREAD: ThreadState = { seq: 0, turns: [] };

/** 从 `thread/history` 的 `events` 重建（开着、关着的会话同一条路）。 */
export function replayHistory(events: readonly Record<string, unknown>[]): ThreadState {
  let state = EMPTY_THREAD;
  for (const raw of events) {
    const event = parseHistoryItem(raw);
    if (event) state = reduceThread(state, event);
  }
  return state;
}

export function reduceThread(state: ThreadState, event: DockEvent): ThreadState {
  if (event.seq && event.seq <= state.seq) return state;
  const seq = event.seq || state.seq;

  if (event.method === 'turn/started') {
    const existing = state.turns.find((t) => t.id === event.turnId);
    if (existing) return { ...state, seq };
    const turn: Turn = { id: event.turnId, status: 'running', error: null, startedAt: event.at, endedAt: null, items: [] };
    return { seq, turns: [...state.turns, turn] };
  }

  return withTurn({ ...state, seq }, event, (turn) => {
    switch (event.method) {
      case 'turn/completed':
        return { ...turn, status: event.status, error: event.error ?? null, endedAt: event.at };

      case 'item/user_message':
        return push(turn, {
          kind: 'user',
          id: `u${event.seq}`,
          text: event.content,
          at: event.at,
          attachments: event.attachments,
        });

      case 'item/message_delta': {
        if (!event.delta) return turn;
        const last = turn.items[turn.items.length - 1];
        if (last?.kind === 'text') return replaceLast(turn, { ...last, text: last.text + event.delta });
        return push(turn, { kind: 'text', id: `a${event.seq}`, text: event.delta });
      }

      case 'item/tool_started':
        if (turn.items.some((i) => i.kind === 'tool' && i.id === event.toolCallId)) return turn;
        return push(turn, {
          kind: 'tool',
          id: event.toolCallId,
          name: event.toolName,
          arguments: event.arguments,
          output: '',
          status: 'running',
          startedAt: event.at,
          endedAt: null,
        });

      case 'item/tool_completed': {
        const done = { output: event.output, status: event.status, endedAt: event.at };
        if (turn.items.some((i) => i.kind === 'tool' && i.id === event.toolCallId)) {
          return update(turn, (i) => (i.kind === 'tool' && i.id === event.toolCallId ? { ...i, ...done } : i));
        }
        // 没见过 started（比如订阅晚了）：补一张没有参数的卡。
        return push(turn, {
          kind: 'tool',
          id: event.toolCallId,
          name: event.toolName,
          arguments: {},
          startedAt: event.at,
          ...done,
        });
      }

      case 'permission/requested':
        return push(turn, {
          kind: 'permission',
          id: event.requestId,
          toolName: event.toolName,
          summary: event.summary,
          decision: null,
          always: false,
        });
      case 'permission/resolved':
        return update(turn, (i) =>
          i.kind === 'permission' && i.id === event.requestId ? { ...i, decision: event.decision, always: event.always } : i,
        );

      case 'interaction/requested':
        return push(turn, { kind: 'question', id: event.interactionId, questions: event.questions, resolved: false });
      case 'interaction/resolved':
        return update(turn, (i) => (i.kind === 'question' && i.id === event.interactionId ? { ...i, resolved: true } : i));

      case 'plan/requested':
        return push(turn, {
          kind: 'plan',
          id: event.planId,
          path: event.path,
          body: event.body,
          empty: event.empty,
          decision: null,
        });
      case 'plan/resolved':
        return update(turn, (i) => (i.kind === 'plan' && i.id === event.planId ? { ...i, decision: event.decision } : i));

      case 'elicit/requested':
        return push(turn, {
          kind: 'elicit',
          id: event.elicitId,
          server: event.server,
          message: event.message,
          heading: event.heading,
          resolved: false,
        });
      case 'elicit/resolved':
        return update(turn, (i) => (i.kind === 'elicit' && i.id === event.elicitId ? { ...i, resolved: true } : i));
    }
  });
}

// ---- 查询 ----

/** 最后一轮还在跑。 */
export function isRunning(state: ThreadState): boolean {
  return state.turns[state.turns.length - 1]?.status === 'running';
}

export type PendingItem = Extract<TurnItem, { kind: 'permission' | 'question' | 'plan' | 'elicit' }>;

/** 还在等用户处理的交互（最多各一条：网关一次只报队首）。 */
export function pendingItems(state: ThreadState): PendingItem[] {
  const out: PendingItem[] = [];
  for (const turn of state.turns) {
    for (const item of turn.items) {
      if (
        (item.kind === 'permission' && item.decision === null) ||
        (item.kind === 'question' && !item.resolved) ||
        (item.kind === 'plan' && item.decision === null) ||
        (item.kind === 'elicit' && !item.resolved)
      ) {
        out.push(item);
      }
    }
  }
  return out;
}

/** 一轮结束后，还挂着的交互不会再有 resolved（网关换页 / 取消时直接丢了）。 */
function settle(turn: Turn): Turn {
  if (turn.status === 'running') return turn;
  let changed = false;
  const items = turn.items.map((i): TurnItem => {
    if (i.kind === 'tool' && i.status === 'running') {
      changed = true;
      return { ...i, status: 'cancelled', endedAt: turn.endedAt };
    }
    if (i.kind === 'permission' && i.decision === null) {
      changed = true;
      return { ...i, decision: 'cancelled' };
    }
    if ((i.kind === 'question' || i.kind === 'elicit') && !i.resolved) {
      changed = true;
      return { ...i, resolved: true };
    }
    if (i.kind === 'plan' && i.decision === null) {
      changed = true;
      return { ...i, decision: 'cancelled' };
    }
    return i;
  });
  return changed ? { ...turn, items } : turn;
}

// ---- 内部 ----

function withTurn(state: ThreadState, event: DockEvent, f: (turn: Turn) => Turn): ThreadState {
  let index = state.turns.findIndex((t) => t.id === event.turnId);
  let turns = state.turns;
  if (index < 0) {
    // 订阅晚了、没见过这一轮的 turn/started：补一轮。
    turns = [...turns, { id: event.turnId, status: 'running', error: null, startedAt: event.at, endedAt: null, items: [] }];
    index = turns.length - 1;
  }
  const before = turns[index];
  const after = settle(f(before));
  if (after === before && turns === state.turns) return state;
  const next = turns === state.turns ? [...turns] : turns;
  next[index] = after;
  return { ...state, turns: next };
}

function push(turn: Turn, item: TurnItem): Turn {
  return { ...turn, items: [...turn.items, item] };
}

function replaceLast(turn: Turn, item: TurnItem): Turn {
  return { ...turn, items: [...turn.items.slice(0, -1), item] };
}

function update(turn: Turn, f: (item: TurnItem) => TurnItem): Turn {
  let changed = false;
  const items = turn.items.map((i) => {
    const n = f(i);
    if (n !== i) changed = true;
    return n;
  });
  return changed ? { ...turn, items } : turn;
}
