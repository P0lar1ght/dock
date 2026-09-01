import assert from 'node:assert/strict';
import test from 'node:test';

import { argumentSummary, isShellTool, truncatedOutput } from './toolCard.js';

test('shell summary prefers command', () => {
  assert.equal(isShellTool('bash'), true);
  assert.equal(argumentSummary('bash', '{"command":"ls -la"}'), 'ls -la');
});

test('read_file summary prefers path', () => {
  assert.equal(argumentSummary('read_file', '{"target_file":"src/a.ts","offset":1}'), 'src/a.ts');
});

test('truncated output keeps head and tail', () => {
  const body = Array.from({ length: 12 }, (_, i) => `L${String(i + 1).padStart(2, '0')}`).join('\n');
  const out = truncatedOutput(body);
  assert.match(out.text, /L01/);
  assert.match(out.text, /L02/);
  assert.match(out.text, /\+7 lines/);
  assert.match(out.text, /L11/);
  assert.match(out.text, /L12/);
  assert.equal(out.text.includes('L05'), false);
});
