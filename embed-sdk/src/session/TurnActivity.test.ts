import assert from 'node:assert/strict';
import test from 'node:test';

import { hasPendingWork } from '../controllers/ThreadController.js';
import { reduceSession } from './SessionReducer.js';
import { initialSessionState } from './SessionState.js';

const thread = { id: 'live', title: 't', workspaceId: 'ws', createdAt: 0, updatedAt: 0 };
const note = (method: string, params: Record<string, unknown>) => ({ method, params: { threadId: 'live', ...params } });
const play = (events: ReturnType<typeof note>[]) =>
  events.reduce((state, event) => reduceSession(state, event), initialSessionState(thread));

test('网关单开的纯交互轮（MCP elicitation）：回应完回到空闲，不显示成在忙', () => {
  const asked = play([
    note('turn/started', { turnId: 't1', status: 'running', seq: 1 }),
    note('elicit/requested', { turnId: 't1', elicitId: 'elicit-2', server: 'sungods', message: 'Choose a generator', seq: 2 })
  ]);
  assert.equal(asked.activity, 'approval', '交互卡片照旧在等用户');
  assert.equal(asked.elicitations[0]?.status, 'pending');
  assert.equal(hasPendingWork(asked), false);

  const answered = reduceSession(asked, note('elicit/resolved', { turnId: 't1', elicitId: 'elicit-2', seq: 3 }));
  assert.equal(answered.elicitations[0]?.status, 'resolved');
  assert.equal(answered.activity, 'idle', '这一轮只有交互：agent 没有在跑');
  assert.equal(hasPendingWork(answered), false);
});

test('纯交互轮里的提问 / 计划 / 权限：回应完都回到空闲', () => {
  const question = play([
    note('turn/started', { turnId: 't1', status: 'running', seq: 1 }),
    note('interaction/requested', {
      turnId: 't1', interactionId: 'q1', kind: 'user_input',
      questions: [{ id: 'a', header: 'h', question: '选哪个？', options: [{ label: 'x', description: 'x' }, { label: 'y', description: 'y' }] }],
      seq: 2
    }),
    note('interaction/resolved', { interactionId: 'q1', seq: 3 })
  ]);
  assert.equal(question.userInputRequests[0]?.status, 'resolved');
  assert.equal(question.activity, 'idle');

  const plan = play([
    note('turn/started', { turnId: 't1', status: 'running', seq: 1 }),
    note('plan/requested', { turnId: 't1', planId: 'plan-2', body: 'x', seq: 2 }),
    note('plan/resolved', { planId: 'plan-2', decision: 'approve', seq: 3 })
  ]);
  assert.equal(plan.activity, 'idle');

  const permission = play([
    note('turn/started', { turnId: 't1', status: 'running', seq: 1 }),
    note('permission/requested', { turnId: 't1', requestId: 'p1', toolName: 'mcp_x', seq: 2 })
  ]);
  assert.equal(permission.turns.t1?.status, 'waiting_permission');
  assert.equal(hasPendingWork(permission), false, '只有权限请求的轮不算有活在跑');
  const approved = reduceSession(permission, note('permission/resolved', { requestId: 'p1', decision: 'approve', seq: 3 }));
  assert.equal(approved.activity, 'idle');
});

test('同一轮还有交互没回应：接着等', () => {
  const state = play([
    note('turn/started', { turnId: 't1', status: 'running', seq: 1 }),
    note('elicit/requested', { turnId: 't1', elicitId: 'e1', server: 's', message: 'm1', seq: 2 }),
    note('elicit/requested', { turnId: 't1', elicitId: 'e2', server: 's', message: 'm2', seq: 3 }),
    note('elicit/resolved', { elicitId: 'e1', seq: 4 })
  ]);
  assert.equal(state.activity, 'approval');
});

test('真在干活的轮：回应交互后照旧算在跑', () => {
  const asked = play([
    note('turn/started', { turnId: 't1', status: 'running', seq: 1 }),
    note('item/user_message', { turnId: 't1', content: '装一下这个 JAR', seq: 2 }),
    note('item/tool_started', { turnId: 't1', toolCallId: 'c1', toolName: 'mcp_sungods__install', seq: 3 }),
    note('permission/requested', { turnId: 't1', requestId: 'p1', toolName: 'mcp_sungods__install', seq: 4 })
  ]);
  assert.equal(hasPendingWork(asked), true);
  const approved = reduceSession(asked, note('permission/resolved', { requestId: 'p1', decision: 'approve', seq: 5 }));
  assert.equal(approved.activity, 'working');
  assert.equal(hasPendingWork(approved), true);

  const running = play([
    note('turn/started', { turnId: 't1', status: 'running', seq: 1 }),
    note('item/user_message', { turnId: 't1', content: 'hi', seq: 2 })
  ]);
  assert.equal(hasPendingWork(running), true, '用户刚发出去，这一轮在跑');
});
