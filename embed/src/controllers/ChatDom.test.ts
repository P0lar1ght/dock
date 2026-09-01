import assert from 'node:assert/strict';
import test from 'node:test';

import { isNearBottom } from './ChatDom.js';

test('isNearBottom follows the list when the user is at the end', () => {
  assert.equal(isNearBottom({ scrollHeight: 800, scrollTop: 740, clientHeight: 60 }), true);
  assert.equal(isNearBottom({ scrollHeight: 800, scrollTop: 736, clientHeight: 60 }), true);
});

test('isNearBottom lets the user scroll away without being yanked back', () => {
  assert.equal(isNearBottom({ scrollHeight: 800, scrollTop: 200, clientHeight: 60 }), false);
  assert.equal(isNearBottom({ scrollHeight: 800, scrollTop: 0, clientHeight: 60 }), false);
});
