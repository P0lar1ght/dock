// 一个线程派出的子代理：每个的状态（`subagent/updated`）+ 它自己的对话
// （`subagent/event`，用同一个 `reduceThread` 按轮组织）。纯函数、不可变更新。
//
// 子代理只活在 Dock 的内存里：中途接入（或 Dock 重启过）时它的对话可能不全，
// `complete` 为假；客户端要看完整过程时取一次 `subagent/history` 交给 `withHistory`。

import type { DockEvent, SubagentActivity, SubagentInfo } from './events.ts';
import { parseSubagent } from './events.ts';
import type { ThreadState, TurnItem } from './thread.ts';
import { EMPTY_THREAD, reduceThread, replayHistory } from './thread.ts';

export interface Subagent {
  info: SubagentInfo;
  /** 它自己的对话。 */
  thread: ThreadState;
  /** 对话从头就有：从第一条事件看起，或者拿到过 `subagent/history`。 */
  complete: boolean;
  /** 不全时攒着的实时事件，`withHistory` 回放完历史再接上。 */
  backlog: DockEvent[];
}

export interface SubagentsState {
  byId: Record<string, Subagent>;
  /** 先出现的在前。 */
  order: string[];
}

export const EMPTY_SUBAGENTS: SubagentsState = { byId: {}, order: [] };

/** 不全时最多攒这么多条实时事件（流式增量很密）。 */
const BACKLOG_CAP = 2000;

/** `subagent/list` 的结果铺底。已有的保留对话，只换状态。 */
export function seedSubagents(state: SubagentsState, agents: readonly Record<string, unknown>[]): SubagentsState {
  let next = state;
  for (const raw of agents) {
    const info = parseSubagent(raw);
    if (info) next = withInfo(next, info);
  }
  return next;
}

/** 实时推送。不是子代理的事件原样返回。 */
export function reduceSubagents(state: SubagentsState, event: DockEvent): SubagentsState {
  if (event.method === 'subagent/updated') return withInfo(state, event.agent);
  if (event.method !== 'subagent/event') return state;
  const existing = state.byId[event.agentId] ?? fresh(placeholder(event.agentId));
  const thread = reduceThread(existing.thread, event.event);
  // 第一条看到的就是它的第一条事件，才算从头看起。
  const complete = existing.complete || (existing.thread.seq === 0 && event.event.seq === 1);
  const backlog = complete ? [] : [...existing.backlog, event.event].slice(-BACKLOG_CAP);
  return put(state, { ...existing, thread, complete, backlog });
}

/** `subagent/history` 的结果：回放它，再接上期间攒的实时事件。 */
export function withHistory(
  state: SubagentsState,
  agentId: string,
  history: { agent?: Record<string, unknown>; events: readonly Record<string, unknown>[] },
): SubagentsState {
  const info = history.agent ? parseSubagent(history.agent) : null;
  const existing = state.byId[agentId] ?? fresh(info ?? placeholder(agentId));
  let thread = replayHistory(history.events);
  for (const event of existing.backlog) thread = reduceThread(thread, event);
  return put(state, { ...existing, info: info ?? existing.info, thread, complete: true, backlog: [] });
}

// ---- 查询 ----

/** 派它的那次 `task` 调用对应的子代理。 */
export function subagentForToolCall(state: SubagentsState, toolCallId: string): Subagent | null {
  for (const id of state.order) {
    const s = state.byId[id];
    if (s.info.toolCallId === toolCallId) return s;
  }
  return null;
}

export function isActive(s: Subagent): boolean {
  return s.info.status === 'running';
}

/** 调过的工具数：对话从头就有时数它的，否则不比快照少。 */
export function toolCallCount(s: Subagent): number {
  let n = 0;
  for (const turn of s.thread.turns) for (const item of turn.items) if (item.kind === 'tool') n++;
  return s.complete ? n : Math.max(n, s.info.toolCalls);
}

/** 此刻在做什么：从它的对话里算；对话里看不出来就用快照。不在跑返回 `null`。 */
export function currentActivity(s: Subagent): SubagentActivity | null {
  if (!isActive(s)) return null;
  const turn = s.thread.turns[s.thread.turns.length - 1];
  const last: TurnItem | undefined = turn?.status === 'running' ? turn.items[turn.items.length - 1] : undefined;
  if (last?.kind === 'tool' && last.status === 'running') {
    return { kind: 'tool', toolName: last.name, arguments: last.arguments };
  }
  if (last?.kind === 'reasoning' && last.endedAt === null) return { kind: 'thinking' };
  if (last?.kind === 'text') return { kind: 'replying' };
  return s.info.activity;
}

/** 用时：在跑时从 `startedAt` 算到 `now`，停下用快照里定住的。 */
export function elapsedMs(s: Subagent, now: number): number {
  if (isActive(s) && s.info.startedAt > 0) return Math.max(0, now - s.info.startedAt);
  return s.info.durationMs;
}

/**
 * 父级派的任务正文。网关给的是原文：开头 `[类型] 描述`、末尾 `---` 后面的回报说明
 * 都是 Dock 加的，这里去掉；认不出就原样返回。
 */
export function taskPrompt(s: Subagent, text: string): string {
  let body = text;
  const head = `[${s.info.subagentType}] ${s.info.description}\n\n`;
  if (body.startsWith(head)) body = body.slice(head.length);
  const tail = body.lastIndexOf('\n\n---\n');
  if (tail >= 0 && body.slice(tail + 6).startsWith('启动你的代理')) body = body.slice(0, tail);
  return body;
}

// ---- 内部 ----

function withInfo(state: SubagentsState, info: SubagentInfo): SubagentsState {
  const existing = state.byId[info.agentId];
  return put(state, existing ? { ...existing, info } : fresh(info));
}

function fresh(info: SubagentInfo): Subagent {
  return { info, thread: EMPTY_THREAD, complete: false, backlog: [] };
}

/** 状态还没到、事件先到了（不该发生，但别丢事件）。 */
function placeholder(agentId: string): SubagentInfo {
  return {
    agentId,
    toolCallId: null,
    subagentType: '',
    role: '子代理',
    description: '',
    status: 'running',
    startedAt: 0,
    durationMs: 0,
    toolCalls: 0,
    output: '',
    error: null,
    activity: null,
  };
}

function put(state: SubagentsState, s: Subagent): SubagentsState {
  const id = s.info.agentId;
  const order = state.byId[id] ? state.order : [...state.order, id];
  return { byId: { ...state.byId, [id]: s }, order };
}
