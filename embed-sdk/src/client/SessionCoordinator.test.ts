import assert from 'node:assert/strict';
import test from 'node:test';

import type { ThreadHistoryResult, ThreadSummary, TranscriptEvent } from '../protocol/responses.js';
import type { AgentSessionOperations } from '../session/AgentSession.js';
import { SessionCoordinator } from './SessionCoordinator.js';
import type { ThreadClient } from './ThreadClient.js';
import type { ThreadStore } from './ThreadStore.js';

const live: ThreadSummary = { id: 'live', title: '当前会话', workspaceId: 'ws', createdAt: 0, updatedAt: 0 };

function event(seq: number, method: string, payload: Record<string, unknown>): TranscriptEvent {
  return { timestamp: String(seq), threadId: 'live', turnId: 't1', seq, method, payload };
}

/** 网关第 1 页：`thread/start` 之后新会话的 id 还是 `live`，seq 从头数。 */
function gateway() {
  let history: ThreadHistoryResult = {
    thread: { ...live, title: '旧会话' },
    messages: [],
    events: [
      event(1, 'turn/started', { status: 'running' }),
      event(2, 'item/user_message', { content: '旧消息' }),
      event(3, 'turn/completed', { status: 'completed' })
    ]
  };
  const subscribed: number[] = [];
  const threadApi = {
    create: async () => {
      history = { thread: live, messages: [], events: [] };
      return live;
    },
    // 归档第 1 页：归档的那份拿到别的 id，`live` 换成空会话。
    archive: async () => {
      history = { thread: live, messages: [], events: [] };
      return { ...live, id: 's-old', title: '旧会话', archivedAt: 1 };
    },
    // 恢复：归档的那份换回 `live`。
    restore: async () => {
      history = {
        thread: { ...live, title: '旧会话' },
        messages: [],
        events: [event(1, 'item/user_message', { content: '恢复回来的消息' })]
      };
      return { ...live, title: '旧会话' };
    },
    history: async () => history,
    environment: async () => ({ threadId: 'live', context: { maxContextTokens: 1000, estimatedTokens: 0 } }),
    subscribe: async (_id: string, _ws: string, sinceSeq: number) => {
      subscribed.push(sinceSeq);
      return { ok: true };
    }
  } as unknown as ThreadClient;
  const threadStore = {
    setActiveWorkspace: () => undefined,
    setLastThread: () => undefined,
    clearLastThread: () => undefined
  } as unknown as ThreadStore;
  const operations = {
    listTurnQueue: async () => ({ active: null, items: [] })
  } as unknown as AgentSessionOperations;
  const coordinator = new SessionCoordinator({
    threadApi,
    threadStore,
    createOperations: () => operations,
    requireWorkspace: () => undefined,
    origin: () => 'http://host.test'
  });
  return { coordinator, subscribed };
}

test('new thread that reuses the live id starts from an empty session', async () => {
  const { coordinator, subscribed } = gateway();
  const old = await coordinator.restore('live', 'ws');
  assert.equal(old.lastSeq, 3);
  assert.ok(old.state.messages.some((message) => message.content === '旧消息'));

  const fresh = await coordinator.create('ws');
  assert.notEqual(fresh, old, '旧会话对象不能拿来装新会话');
  assert.equal(fresh.lastSeq, 0);
  assert.equal(fresh.state.messages.length, 0);
  assert.deepEqual(subscribed, [3, 0], '新会话从头订阅');
  assert.equal(coordinator.activeSession, fresh);

  // 新会话的第一条事件 seq 从 1 数：要收得进去，不能当成重复丢掉。
  fresh.receive({ method: 'item/user_message', params: { threadId: 'live', turnId: 't1', seq: 1, content: '新消息' } });
  assert.ok(fresh.state.messages.some((message) => message.content === '新消息'));
});

test('archiving the live thread drops the stale live session; restoring brings the archived one back fresh', async () => {
  const { coordinator } = gateway();
  const old = await coordinator.restore('live', 'ws');
  assert.ok(old.state.messages.length > 0);

  const archived = await coordinator.archive('live', 'ws');
  assert.equal(archived.id, 's-old');
  assert.equal(coordinator.activeSession, undefined, '手里的 live 是归档走的那份，不能再当活动会话');
  const emptied = await coordinator.restore('live', 'ws');
  assert.notEqual(emptied, old);
  assert.equal(emptied.state.messages.length, 0, 'live 现在是空会话');

  await coordinator.restoreArchived('s-old', 'ws');
  const back = await coordinator.restore('live', 'ws');
  assert.notEqual(back, emptied, '换回来的那份要重拉');
  assert.ok(back.state.messages.some((message) => message.content === '恢复回来的消息'));
});
