import assert from 'node:assert/strict';
import test from 'node:test';

import { commandOutputFromSlash, commandOutputLooksLikeMarkdown } from './CommandOutput.js';

test('notice with title and markdown body becomes command output', () => {
  const output = commandOutputFromSlash({
    ok: true,
    kind: 'notice',
    notice: { title: '当前计划', body: '## 背景\n\n对齐后再动手。' }
  });
  assert.deepEqual(output, {
    kind: 'notice',
    title: '当前计划',
    body: '## 背景\n\n对齐后再动手。'
  });
});

test('applied confirmation is command output, not an error', () => {
  const output = commandOutputFromSlash({
    ok: true,
    kind: 'applied',
    notice: { title: '', body: '已开始新会话' }
  });
  assert.deepEqual(output, {
    kind: 'applied',
    title: '已完成',
    body: '已开始新会话'
  });
});

test('empty notice is ignored', () => {
  assert.equal(commandOutputFromSlash({
    ok: true,
    kind: 'notice',
    notice: { title: '  ', body: '' }
  }), undefined);
});

test('filled slash results are not command output', () => {
  assert.equal(commandOutputFromSlash({
    ok: true,
    kind: 'filled',
    fill: '/goal '
  }), undefined);
});

test('plan markdown is rendered as markdown', () => {
  assert.equal(commandOutputLooksLikeMarkdown('## 背景\n\n对齐后再动手。'), true);
  assert.equal(commandOutputLooksLikeMarkdown('```mermaid\nflowchart LR\n```'), true);
});

test('help and usage stay preformatted', () => {
  assert.equal(commandOutputLooksLikeMarkdown('/help  斜杠命令\n/view-plan  查看当前计划'), false);
  // 抄 `session_usage_block_text` 的真实输出：对齐靠空格，markdown 会毁掉它。
  assert.equal(
    commandOutputLooksLikeMarkdown('  输入(未命中):  1,024 · 19%\n  缓存命中:      4,096 · 81%'),
    false,
  );
});
