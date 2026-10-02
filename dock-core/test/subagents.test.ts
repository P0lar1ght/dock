import assert from 'node:assert/strict';
import { test } from 'node:test';

import {
  currentActivity,
  EMPTY_SUBAGENTS,
  EMPTY_THREAD,
  parseEvent,
  reduceSubagents,
  reduceThread,
  seedSubagents,
  subagentForToolCall,
  taskPrompt,
  toolCallCount,
  withHistory,
  type DockEvent,
  type SubagentsState,
} from '../src/index.ts';

const agent = (status: string, extra: Record<string, unknown> = {}) => ({
  agentId: 'kid',
  toolCallId: 'call-1',
  subagentType: 'explore',
  role: '探索',
  description: '看一眼',
  status,
  startedAt: 1000,
  durationMs: 0,
  toolCalls: 0,
  output: '',
  ...extra,
});

/** 网关的推送外形：平铺 params，`seq` 为 0。 */
function updated(status: string, extra: Record<string, unknown> = {}): DockEvent {
  const e = parseEvent('subagent/updated', { seq: 0, threadId: 'th', turnId: 't1', timestamp: '1', agent: agent(status, extra) });
  assert.ok(e);
  return e;
}

/** 子代理自己的一条事件，包成 `subagent/event`（`event` 同历史条目）。 */
function child(seq: number, method: string, payload: Record<string, unknown> = {}, turnId = 't1'): DockEvent {
  const e = parseEvent('subagent/event', {
    seq: 0,
    threadId: 'th',
    turnId: 'parent-turn',
    timestamp: '1',
    agentId: 'kid',
    event: { seq, method, timestamp: String(1000 + seq), turnId, payload: { threadId: 'th', turnId, ...payload } },
  });
  assert.ok(e, method);
  return e;
}

const run = (state: SubagentsState, ...events: DockEvent[]) => events.reduce(reduceSubagents, state);

test('状态 + 对话：从头看起的子代理是完整的，工具数和当前动作从对话里算', () => {
  const s = run(
    EMPTY_SUBAGENTS,
    updated('running'),
    child(1, 'turn/started'),
    child(2, 'item/user_message', { content: '[explore] 看一眼\n\n列出 src\n\n---\n启动你的代理（agent_id 为 "main"）看不到…' }),
    child(3, 'item/tool_started', { toolCallId: 'r1', toolName: 'read_file', arguments: { target_file: 'a.ts' } }),
  );
  const kid = s.byId.kid;
  assert.equal(kid.complete, true);
  assert.equal(toolCallCount(kid), 1);
  assert.deepEqual(currentActivity(kid), { kind: 'tool', toolName: 'read_file', arguments: { target_file: 'a.ts' } });
  assert.equal(subagentForToolCall(s, 'call-1'), kid);
  const first = kid.thread.turns[0].items[0];
  assert.equal(first.kind === 'user' && taskPrompt(kid, first.text), '列出 src', '去掉 Dock 加的开头和回报说明');

  const idle = run(s, child(4, 'turn/completed', { status: 'completed' }), updated('idle', { output: '找到 3 个' }));
  assert.equal(idle.byId.kid.info.status, 'idle');
  assert.equal(currentActivity(idle.byId.kid), null, '停下了就没有当前动作');
});

test('子代理的事件不进父线程的时间线', () => {
  const parent = [updated('running'), child(1, 'turn/started')].reduce(reduceThread, EMPTY_THREAD);
  assert.equal(parent, EMPTY_THREAD);
});

test('中途接入：对话不全，攒着实时事件，拿到历史后接上', () => {
  let s = seedSubagents(EMPTY_SUBAGENTS, [agent('running', { toolCalls: 4 })]);
  s = run(s, child(7, 'item/message_delta', { delta: '还在' }));
  assert.equal(s.byId.kid.complete, false);
  assert.equal(toolCallCount(s.byId.kid), 4, '不全时不比快照少');

  s = withHistory(s, 'kid', {
    events: [
      { seq: 1, method: 'turn/started', turnId: 't1', timestamp: '1001', payload: {} },
      { seq: 2, method: 'item/user_message', turnId: 't1', timestamp: '1002', payload: { content: '任务' } },
      { seq: 6, method: 'item/message_delta', turnId: 't1', timestamp: '1006', payload: { delta: '我' } },
    ],
  });
  const kid = s.byId.kid;
  assert.equal(kid.complete, true);
  assert.equal(kid.backlog.length, 0);
  const text = kid.thread.turns[0].items.find((i) => i.kind === 'text');
  assert.equal(text?.kind === 'text' && text.text, '我还在', '历史之后接上攒着的增量');
});

test('父级中途发来的话带 origin', () => {
  const s = run(EMPTY_SUBAGENTS, updated('running'), child(1, 'turn/started'), child(2, 'item/user_message', { content: '也看 gateway', origin: 'parent' }));
  const item = s.byId.kid.thread.turns[0].items[0];
  assert.equal(item.kind === 'user' && item.origin, 'parent');
});

test('停下的子代理拿到历史后不再接攒着的旧推送（Dock 收掉它后按会话回放，序号对不上）', () => {
  let s = seedSubagents(EMPTY_SUBAGENTS, [agent('running')]);
  s = run(s, child(9, 'item/message_delta', { delta: '旧推送' }));
  s = withHistory(s, 'kid', {
    agent: agent('completed'),
    events: [
      { seq: 1, method: 'turn/started', turnId: 't1', timestamp: '1001', payload: {} },
      { seq: 2, method: 'item/message_delta', turnId: 't1', timestamp: '1002', payload: { delta: '旧推送' } },
      { seq: 3, method: 'turn/completed', turnId: 't1', timestamp: '1003', payload: { status: 'completed' } },
    ],
  });
  const text = s.byId.kid.thread.turns[0].items.find((i) => i.kind === 'text');
  assert.equal(text?.kind === 'text' && text.text, '旧推送', '不重复');
  assert.equal(s.byId.kid.info.status, 'completed');
});
