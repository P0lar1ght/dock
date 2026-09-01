import assert from 'node:assert/strict';
import test from 'node:test';

import { applyAssistantDelta } from './MessageProjector.js';
import { reduceSession } from './SessionReducer.js';
import { initialSessionState } from './SessionState.js';

const thread = {
  id: 'live',
  title: 'test',
  workspaceId: 'ws',
  createdAt: 0,
  updatedAt: 0
};

function note(method: string, params: Record<string, unknown>) {
  return { method, params: { threadId: 'live', ...params } };
}

function play(events: ReturnType<typeof note>[]) {
  return events.reduce((state, event) => reduceSession(state, event), initialSessionState(thread));
}

test('token deltas merge into one assistant bubble', () => {
  const state = play([
    note('item/user_message', { turnId: 't1', content: 'hi', seq: 1 }),
    note('item/message_delta', { turnId: 't1', delta: '你', seq: 2 }),
    note('turn/completed', { turnId: 't1', status: 'completed', seq: 3 }),
    note('item/message_delta', { turnId: 't2', delta: '好', seq: 4 }),
    note('turn/completed', { turnId: 't2', status: 'completed', seq: 5 }),
    note('item/message_delta', { turnId: 't3', delta: '。', seq: 6 }),
    note('turn/completed', { turnId: 't3', status: 'completed', seq: 7 })
  ]);
  const assistants = state.messages.filter((message) => message.role === 'assistant');
  assert.equal(assistants.length, 1);
  assert.equal(assistants[0]?.content, '你好。');
  assert.equal(assistants[0]?.status, 'completed');
});

test('accumulated snapshots replace instead of duplicating', () => {
  assert.equal(applyAssistantDelta('你', '你好'), '你好');
  assert.equal(applyAssistantDelta('你', '好'), '你好');
  const state = play([
    note('item/user_message', { turnId: 't1', content: 'hi', seq: 1 }),
    note('item/message_delta', { turnId: 't1', delta: '你', seq: 2 }),
    note('item/message_delta', { turnId: 't1', delta: '你好', seq: 3 }),
    note('item/message_delta', { turnId: 't1', delta: { text: '你好啊' }, seq: 4 })
  ]);
  assert.equal(state.messages.at(-1)?.content, '你好啊');
  assert.equal(state.messages.filter((message) => message.role === 'assistant').length, 1);
});

test('tool split still starts a new assistant bubble', () => {
  const state = play([
    note('item/user_message', { turnId: 't1', content: 'hi', seq: 1 }),
    note('item/message_delta', { turnId: 't1', delta: '先看文件', seq: 2 }),
    note('item/tool_started', {
      turnId: 't1',
      toolCallId: 'c1',
      itemId: 'c1',
      toolName: 'read_file',
      seq: 3
    }),
    note('item/tool_completed', {
      turnId: 't1',
      toolCallId: 'c1',
      itemId: 'c1',
      toolName: 'read_file',
      output: 'ok',
      status: 'completed',
      seq: 4
    }),
    note('item/message_delta', { turnId: 't1', delta: '看完了', seq: 5 })
  ]);
  const assistants = state.messages.filter((message) => message.role === 'assistant');
  assert.equal(assistants.length, 2);
  assert.equal(assistants[0]?.content, '先看文件');
  assert.equal(assistants[0]?.closed, true);
  assert.equal(assistants[1]?.content, '看完了');
});

test('user message keeps image attachment metadata', () => {
  const state = play([
    note('item/user_message', {
      turnId: 't1',
      content: '[Image #1] 这是什么',
      attachments: [{
        type: 'image',
        mimeType: 'image/png',
        width: 12,
        height: 8,
        byteLength: 64
      }],
      seq: 1
    })
  ]);
  const user = state.messages.find((message) => message.role === 'user');
  assert.equal(user?.content, '[Image #1] 这是什么');
  assert.equal(user?.attachments?.length, 1);
  assert.equal(user?.attachments?.[0]?.mimeType, 'image/png');
  assert.equal(user?.attachments?.[0]?.previewUrl, undefined);
});
