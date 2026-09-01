import assert from 'node:assert/strict';
import test from 'node:test';

import { initialSessionState } from './SessionState.js';
import type { SessionMessage } from './MessageModel.js';
import { TransientScreenshotPreviewStore } from './TransientScreenshotPreviewStore.js';

const thread = {
  id: 'live',
  title: 'test',
  workspaceId: 'default',
  createdAt: 0,
  updatedAt: 0
};

test('matching turnId consumes leftover unclaimed so a later TUI image is not reused', () => {
  mockBlobUrls();
  const store = new TransientScreenshotPreviewStore();
  store.add('t1', preview('first', 10, 10, 32));
  const afterFirst = store.project(state([
    user('t1', { width: 10, height: 10, byteLength: 32 })
  ]));
  assert.equal(afterFirst.messages[0]?.attachments?.[0]?.previewUrl, 'blob:1');

  const afterSecond = store.project(state([
    afterFirst.messages[0]!,
    user('t2', { width: 597, height: 840, byteLength: 69000 })
  ]));
  assert.equal(afterSecond.messages[0]?.attachments?.[0]?.previewUrl, 'blob:1');
  assert.equal(afterSecond.messages[1]?.attachments?.[0]?.previewUrl, undefined);
});

test('unclaimed preview binds once to a mismatched transcript turnId', () => {
  mockBlobUrls();
  const store = new TransientScreenshotPreviewStore();
  store.add('t8', preview('embed', 12, 8, 64));
  const bound = store.project(state([
    user('t1', { width: 12, height: 8, byteLength: 64 })
  ]));
  assert.equal(bound.messages[0]?.attachments?.[0]?.previewUrl, 'blob:1');

  const later = store.project(state([
    bound.messages[0]!,
    user('t2', { width: 12, height: 8, byteLength: 64 })
  ]));
  assert.equal(later.messages[1]?.attachments?.[0]?.previewUrl, undefined);
});

test('unclaimed preview is not applied to a newer image with different bytes', () => {
  mockBlobUrls();
  const store = new TransientScreenshotPreviewStore();
  store.add('t1', preview('old', 10, 10, 32));
  const later = store.project(state([
    user('t2', { width: 597, height: 840, byteLength: 69000 })
  ]));
  assert.equal(later.messages[0]?.attachments?.[0]?.previewUrl, undefined);
});

function mockBlobUrls() {
  let n = 0;
  globalThis.URL.createObjectURL = () => `blob:${++n}`;
  globalThis.URL.revokeObjectURL = () => undefined;
}

function preview(name: string, width: number, height: number, byteLength: number) {
  return {
    blob: new Blob([new Uint8Array(byteLength)], { type: 'image/png' }),
    source: 'screenshot' as const,
    name,
    mimeType: 'image/png' as const,
    width,
    height,
    byteLength
  };
}

function user(
  turnId: string,
  attachment: { width: number; height: number; byteLength: number }
): SessionMessage {
  return {
    id: `${turnId}:user`,
    turnId,
    role: 'user',
    content: '[Image #1] 这是什么',
    attachments: [{
      type: 'image',
      mimeType: 'image/png',
      width: attachment.width,
      height: attachment.height,
      byteLength: attachment.byteLength
    }],
    status: 'completed',
    startedSeq: Number(turnId.slice(1)) || 1,
    updatedSeq: Number(turnId.slice(1)) || 1
  };
}

function state(messages: SessionMessage[]) {
  return { ...initialSessionState(thread), messages, lastSeq: messages.length };
}
