import assert from 'node:assert/strict';
import test from 'node:test';

import { reduceSession } from './SessionReducer.js';
import { initialSessionState } from './SessionState.js';

const thread = { id: 'live', title: 't', workspaceId: 'ws', createdAt: 0, updatedAt: 0 };
const note = (method: string, params: Record<string, unknown>) => ({ method, params: { threadId: 'live', ...params } });
const play = (events: ReturnType<typeof note>[]) =>
  events.reduce((state, event) => reduceSession(state, event), initialSessionState(thread));

test('plan/requested waits for approval, plan/resolved records the decision', () => {
  const asked = play([
    note('turn/started', { turnId: 't1', status: 'running', seq: 1 }),
    note('plan/requested', { turnId: 't1', planId: 'plan-2', body: '# 计划\n1. 改 README', path: '/x/plan.md', empty: false, seq: 2 })
  ]);
  assert.equal(asked.activity, 'approval');
  assert.deepEqual(asked.planApprovals.map((p) => [p.id, p.status, p.body]), [['plan-2', 'pending', '# 计划\n1. 改 README']]);

  const approved = reduceSession(asked, note('plan/resolved', { turnId: 't1', planId: 'plan-2', decision: 'approve', seq: 3 }));
  assert.equal(approved.planApprovals[0]?.status, 'approved');
  assert.equal(approved.activity, 'working');
});

test('a turn that ends without a decision closes its pending plan', () => {
  const ended = play([
    note('turn/started', { turnId: 't1', status: 'running', seq: 1 }),
    note('plan/requested', { turnId: 't1', planId: 'plan-2', body: 'x', seq: 2 }),
    note('turn/completed', { turnId: 't1', status: 'cancelled', seq: 3 })
  ]);
  assert.equal(ended.planApprovals[0]?.status, 'cancelled');
});

test('elicit/requested waits for a response, elicit/resolved closes it', () => {
  const asked = play([
    note('turn/started', { turnId: 't1', status: 'running', seq: 1 }),
    note('elicit/requested', { turnId: 't1', elicitId: 'elicit-2', server: 'linear', heading: '请求确认', message: '创建 issue？', seq: 2 })
  ]);
  assert.equal(asked.activity, 'approval');
  assert.deepEqual(asked.elicitations.map((e) => [e.id, e.server, e.status]), [['elicit-2', 'linear', 'pending']]);
  const done = reduceSession(asked, note('elicit/resolved', { turnId: 't1', elicitId: 'elicit-2', ok: true, seq: 3 }));
  assert.equal(done.elicitations[0]?.status, 'resolved');
});
