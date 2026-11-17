import assert from 'node:assert/strict';
import { test } from 'node:test';

import { EMPTY_THREAD, feedToolInput, liveToolCall, parseEvent, partialJson, reduceThread, type ToolInputDelta, type ToolInputStream } from '../src/index.ts';

function delta(chunk: number, text: string, toolName = 'mcp_whiteboard__whiteboard_edit'): ToolInputDelta {
  const e = parseEvent('item/tool_input_delta', { threadId: 't', turnId: 'u', toolCallId: 'c1', toolName, chunk, delta: text, seq: 0 });
  assert.ok(e && e.method === 'item/tool_input_delta');
  return e;
}

function feedAll(pieces: string[], toolName?: string): ToolInputStream {
  let s: ToolInputStream | undefined;
  pieces.forEach((p, i) => (s = feedToolInput(s, delta(i, p, toolName))));
  return s!;
}

test('parseEvent: item/tool_input_delta 的字段', () => {
  const e = delta(3, '{"a"');
  assert.equal(e.toolCallId, 'c1');
  assert.equal(e.toolName, 'mcp_whiteboard__whiteboard_edit');
  assert.equal(e.chunk, 3);
  assert.equal(e.delta, '{"a"');
  assert.equal(e.seq, 0);
});

test('reduceThread: 流式参数不进时间线', () => {
  assert.equal(reduceThread(EMPTY_THREAD, delta(0, '{')), EMPTY_THREAD);
});

test('feedToolInput: 按 chunk 依次拼；重复的旧段忽略；跳段作废', () => {
  let s = feedToolInput(undefined, delta(0, '{"a":'));
  s = feedToolInput(s, delta(1, '1'));
  assert.equal(s.raw, '{"a":1');
  assert.equal(feedToolInput(s, delta(1, '1')), s, '重复的段忽略');
  const gap = feedToolInput(s, delta(3, '}'));
  assert.equal(gap.broken, true);
  assert.equal(feedToolInput(gap, delta(4, '}')), gap, '作废之后不再拼');
  // 中途接入：第一段不是 0。
  assert.equal(feedToolInput(undefined, delta(2, '"x"')).broken, true);
});

test('partialJson: 完整的 JSON 与 JSON.parse 一致', () => {
  for (const text of ['{"a":[1,2,{"b":"c\\n\\u00e9"}],"d":true,"e":null,"f":-1.5e3}', '[]', '"x"', '12', ' { } ']) {
    assert.deepEqual(partialJson(text), JSON.parse(text));
  }
});

test('partialJson: 残缺前缀尽量补全', () => {
  assert.equal(partialJson(''), undefined);
  assert.equal(partialJson('   '), undefined);
  assert.deepEqual(partialJson('{'), {});
  assert.deepEqual(partialJson('{"ad'), {}, '键没写完：丢掉');
  assert.deepEqual(partialJson('{"add"'), {});
  assert.deepEqual(partialJson('{"add":'), {});
  assert.deepEqual(partialJson('{"add":['), { add: [] });
  assert.deepEqual(partialJson('{"add":[{"type":"rect'), { add: [{ type: 'rect' }] }, '没写完的字符串照收');
  assert.deepEqual(partialJson('{"add":[{"type":"rectangle","x":12'), { add: [{ type: 'rectangle', x: 12 }] });
  assert.deepEqual(partialJson('{"x":1.'), { x: 1 }, '没写完的小数收成形的部分');
  assert.deepEqual(partialJson('{"x":-'), {});
  assert.deepEqual(partialJson('{"x":1e'), { x: 1 });
  assert.deepEqual(partialJson('{"ok":tr'), {}, '没写完的字面量丢掉');
  assert.deepEqual(partialJson('{"ok":true,'), { ok: true });
  assert.deepEqual(partialJson('[1,2,'), [1, 2]);
});

test('partialJson: 字符串末尾的半个转义丢掉', () => {
  assert.deepEqual(partialJson('{"t":"a\\'), { t: 'a' });
  assert.deepEqual(partialJson('{"t":"a\\u00'), { t: 'a' });
  assert.deepEqual(partialJson('{"t":"a\\\\'), { t: 'a\\' }, '成对的反斜杠是一个完整的转义');
  assert.deepEqual(partialJson('{"t":"\\u00e9x'), { t: 'éx' });
  assert.deepEqual(partialJson('{"t":"say \\"hi'), { t: 'say "hi' });
});

test('partialJson: partialStrings=false 时没写完的字符串当还没写到', () => {
  assert.deepEqual(partialJson('{"tool_name":"mcp_white', { partialStrings: false }), {});
  assert.deepEqual(partialJson('{"tool_name":"mcp_a","tool_input":{"t":"x', { partialStrings: false }), { tool_name: 'mcp_a', tool_input: {} });
});

test('partialJson: 不是 JSON 回 undefined', () => {
  assert.equal(partialJson('hello'), undefined);
  assert.equal(partialJson('{"a" 1}'), undefined);
});

test('partialJson: 字符串里直接写的换行（不合法）也认', () => {
  assert.deepEqual(partialJson('{"t":"a\nb"}'), { t: 'a\nb' });
});

test('liveToolCall: 普通工具直接给补全的参数', () => {
  const s = feedAll(['{"add":[{"type":"ellipse",', '"x":1}', ',{"ty']);
  assert.deepEqual(liveToolCall(s), { name: 'mcp_whiteboard__whiteboard_edit', args: { add: [{ type: 'ellipse', x: 1 }, {}] } });
});

test('liveToolCall: use_tool 等 tool_name 写完才认，参数取 tool_input', () => {
  const pending = feedAll(['{"tool_name":"mcp_whiteboard__whiteboard_ed'], 'use_tool');
  assert.equal(liveToolCall(pending), null);
  const named = feedAll(['{"tool_name":"mcp_whiteboard__whiteboard_edit",', '"tool_input":{"add":[{"type":"text","text":"Hi'], 'use_tool');
  assert.deepEqual(liveToolCall(named), { name: 'mcp_whiteboard__whiteboard_edit', args: { add: [{ type: 'text', text: 'Hi' }] } });
  // tool_input 是 JSON 字符串（Dock 也认）：一样按残缺 JSON 补全。
  const asString = feedAll(['{"tool_name":"mcp_a","tool_input":"{\\"x\\":[1,'], 'use_tool');
  assert.deepEqual(liveToolCall(asString), { name: 'mcp_a', args: { x: [1] } });
});

test('liveToolCall: 作废的流回 null', () => {
  const s = feedToolInput(undefined, delta(1, '{}'));
  assert.equal(liveToolCall(s), null);
});
