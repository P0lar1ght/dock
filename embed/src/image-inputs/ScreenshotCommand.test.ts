import assert from 'node:assert/strict';
import test from 'node:test';

import { isScreenshotCommand, parseScreenshotCommand } from './ScreenshotCommand.js';

test('parseScreenshotCommand keeps --screen as a capture turn', () => {
  const parsed = parseScreenshotCommand('/screenshot --screen 这是什么');
  assert.equal(typeof parsed, 'object');
  assert.deepEqual(parsed, {
    message: '这是什么',
    imageInputs: [{ type: 'screenshot', detail: 'auto', capture: 'screen' }]
  });
});

test('isScreenshotCommand matches embed slash capture', () => {
  assert.equal(isScreenshotCommand('/screenshot --screen'), true);
  assert.equal(isScreenshotCommand('/help'), false);
});
