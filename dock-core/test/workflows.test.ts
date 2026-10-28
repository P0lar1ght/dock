import assert from 'node:assert/strict';
import { test } from 'node:test';

import {
  EMPTY_THREAD,
  EMPTY_WORKFLOWS,
  parseEvent,
  reduceThread,
  reduceWorkflows,
  seedWorkflows,
  workflowElapsedMs,
  workflowForToolCall,
  workflowOfAgent,
  workflowPhases,
  type DockEvent,
} from '../src/index.ts';

const run = (status: string, extra: Record<string, unknown> = {}) => ({
  runId: 'wf_1',
  toolCallId: 'call-wf',
  name: 'deep-research',
  description: '调研',
  builtin: true,
  status,
  phases: [
    { title: 'Plan', detail: '拆问题' },
    { title: 'Research', detail: '并行查' },
  ],
  currentPhase: 'Plan',
  startedAt: 1000,
  durationMs: 0,
  agentBudget: 128,
  agentsUsed: 1,
  agents: [{ agentId: 'kid-1', label: 'research-planner', phase: 'Plan', status: 'running', durationMs: 0, tokensUsed: 0, latestReport: null }],
  logs: ['planning'],
  ...extra,
});

/** 网关的推送外形：平铺 params，`seq` 为 0。 */
function updated(status: string, extra: Record<string, unknown> = {}): DockEvent {
  const e = parseEvent('workflow/updated', { seq: 0, threadId: 'th', turnId: 't1', timestamp: '1', run: run(status, extra) });
  assert.ok(e);
  return e;
}

test('workflow/updated 解析成 run，坏的丢掉', () => {
  const e = updated('running');
  assert.equal(e.method, 'workflow/updated');
  if (e.method !== 'workflow/updated') return;
  assert.equal(e.run.runId, 'wf_1');
  assert.equal(e.run.agents[0].phase, 'Plan');
  assert.equal(e.run.result, null);
  assert.equal(parseEvent('workflow/updated', { seq: 0, threadId: 'th', turnId: 't1', timestamp: '1', run: {} }), null);
});

test('推送按 run id 换新，先起的在前；铺底不打乱顺序', () => {
  let state = seedWorkflows(EMPTY_WORKFLOWS, [run('running'), run('completed', { runId: 'wf_2', toolCallId: 'call-2' })]);
  assert.deepEqual(state.order, ['wf_1', 'wf_2']);
  state = reduceWorkflows(state, updated('completed', { result: '# 报告', durationMs: 5000 }));
  assert.deepEqual(state.order, ['wf_1', 'wf_2']);
  assert.equal(state.byId.wf_1.status, 'completed');
  assert.equal(state.byId.wf_1.result, '# 报告');
  assert.equal(workflowForToolCall(state, 'call-2')?.runId, 'wf_2');
  assert.equal(workflowOfAgent(state, 'kid-1')?.runId, 'wf_1');
  assert.equal(workflowOfAgent(state, 'nobody'), null);
});

test('旧快照不盖新的：拉列表的结果晚于推送到达', () => {
  let state = reduceWorkflows(EMPTY_WORKFLOWS, updated('completed', { revision: 9 }));
  state = seedWorkflows(state, [run('running', { revision: 3 })]);
  assert.equal(state.byId.wf_1.status, 'completed');
  state = reduceWorkflows(state, updated('completed', { revision: 10, result: 'x' }));
  assert.equal(state.byId.wf_1.result, 'x');
});

test('用时：在跑算到现在，结束用定住的', () => {
  const e = updated('running');
  if (e.method !== 'workflow/updated') return;
  assert.equal(workflowElapsedMs(e.run, 4000), 3000);
  const done = updated('completed', { durationMs: 7000 });
  if (done.method !== 'workflow/updated') return;
  assert.equal(workflowElapsedMs(done.run, 99999), 7000);
});

test('按声明的阶段分组：空阶段留着，阶段外的归最后', () => {
  const e = updated('running', {
    agents: [
      { agentId: 'a', label: 'planner', phase: 'Plan', status: 'completed' },
      { agentId: 'b', label: 'stray', phase: null, status: 'running' },
    ],
  });
  if (e.method !== 'workflow/updated') return;
  const groups = workflowPhases(e.run);
  assert.deepEqual(
    groups.map((g) => [g.phase, g.agents.map((a) => a.agentId)]),
    [
      ['Plan', ['a']],
      ['Research', []],
      [null, ['b']],
    ],
  );
});

test('workflow/updated 不进线程自己的时间线', () => {
  const e = updated('running');
  assert.equal(reduceThread(EMPTY_THREAD, e), EMPTY_THREAD);
});
