import assert from 'node:assert/strict';
import test from 'node:test';

import { DockClientError } from '../protocol/errors.js';
import { imageSubmissionError } from '../controllers/ChatImageInputController.js';

test('imageSubmissionError maps screen capture failures instead of the generic gateway text', () => {
  assert.equal(
    imageSubmissionError(new DockClientError('image_input_capture_failed', 'nope')),
    '未能截取所选屏幕，请再试一次'
  );
  assert.equal(
    imageSubmissionError(new DockClientError('request_timeout', 'imageInputs/put timed out')),
    '截图或图片上传超时，请重试'
  );
  assert.equal(
    imageSubmissionError(new DockClientError('image_input_too_large', 'too big')),
    '截图太大，请改选一个窗口后再试'
  );
  assert.equal(
    imageSubmissionError(new DockClientError('image_input_user_gesture_required', 'gesture')),
    '请再点一次相机按钮截取屏幕'
  );
});
