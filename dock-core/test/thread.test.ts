import assert from 'node:assert/strict';
import { test } from 'node:test';

import { EMPTY_THREAD, isRunning, parseEvent, pendingItems, reduceThread, replayHistory, type ThreadState } from '../src/index.ts';

let seq = 0;
/** 按网关实时推送的外形造一条通知：平铺 params，毫秒数字串 timestamp。 */
function note(method: string, params: Record<string, unknown> = {}, turnId = 't1', at = 1000 + seq) {
  seq += 1;
  const event = parseEvent(method, { seq, threadId: 'th', turnId, timestamp: String(at), ...params });
  assert.ok(event, method);
  return event;
}

function run(...events: ReturnType<typeof note>[]): ThreadState {
  return events.reduce(reduceThread, EMPTY_THREAD);
}

test('一轮：用户消息、文字、工具、文字，按出现顺序', () => {
  seq = 0;
  const s = run(
    note('turn/started', {}, 't1', 1000),
    note('item/user_message', { content: '列目录' }, 't1', 1000),
    note('item/message_delta', { delta: '好的，' }),
    note('item/message_delta', { delta: '我看看' }),
    note('item/tool_started', { toolCallId: 'c1', toolName: 'list_dir', arguments: { target_directory: 'src' } }),
    note('item/tool_completed', { toolCallId: 'c1', toolName: 'list_dir', output: 'a.ts', status: 'completed' }),
    note('item/message_delta', { delta: '只有 a.ts' }),
    note('turn/completed', { status: 'completed' }, 't1', 4000),
  );
  const [turn] = s.turns;
  assert.equal(turn.status, 'completed');
  assert.equal(turn.endedAt! - turn.startedAt, 3000);
  assert.deepEqual(
    turn.items.map((i) => i.kind),
    ['user', 'text', 'tool', 'text'],
  );
  const tool = turn.items[2];
  assert.equal(tool.kind === 'tool' && tool.arguments.target_directory, 'src', '保留原始参数');
  assert.equal(turn.items[1].kind === 'text' && turn.items[1].text, '好的，我看看');
  assert.equal(isRunning(s), false);
});

test('seq 不大于已处理的事件是重复，跳过（订阅与拉历史重叠）', () => {
  seq = 0;
  const a = note('turn/started');
  const b = note('item/message_delta', { delta: 'x' });
  const s = run(a, b, b, a);
  assert.equal(s.turns.length, 1);
  assert.equal(s.turns[0].items.length, 1);
});

test('失败与停止：状态、错误原文；跑到一半的工具随一轮结束记为 cancelled', () => {
  seq = 0;
  const failed = run(note('turn/started'), note('turn/completed', { status: 'failed', error: 'HTTP 500' }));
  assert.equal(failed.turns[0].status, 'failed');
  assert.equal(failed.turns[0].error, 'HTTP 500');

  const stopped = run(
    note('turn/started', {}, 't2'),
    note('item/tool_started', { toolCallId: 'c', toolName: 'bash', arguments: {} }, 't2'),
    note('permission/requested', { requestId: 'p1', toolName: 'bash', summary: 'sleep' }, 't2'),
    note('turn/completed', { status: 'cancelled' }, 't2'),
  );
  const items = stopped.turns[0].items;
  assert.equal(items[0].kind === 'tool' && items[0].status, 'cancelled');
  assert.equal(items[1].kind === 'permission' && items[1].decision, 'cancelled');
  assert.deepEqual(pendingItems(stopped), [], '一轮结束后没有还在等的交互');
});

test('交互：权限、提问、计划在等用户时进 pendingItems，resolved 后移出', () => {
  seq = 0;
  let s = run(
    note('turn/started'),
    note('permission/requested', { requestId: 'p1', toolName: 'bash', summary: 'ls' }),
    note('interaction/requested', {
      interactionId: 'q1',
      questions: [{ id: 'q', header: 'h', question: '选哪个？', options: [{ label: 'A', description: 'a', recommended: false }] }],
    }),
  );
  assert.deepEqual(
    pendingItems(s).map((i) => i.kind),
    ['permission', 'question'],
  );
  s = reduceThread(s, note('permission/resolved', { requestId: 'p1', decision: 'approve', always: false }));
  s = reduceThread(s, note('interaction/resolved', { interactionId: 'q1' }));
  assert.deepEqual(pendingItems(s), []);
  assert.equal(isRunning(s), true);
});

test('提问：多选题带 multiSelect，旧网关没这个字段时算单选', () => {
  seq = 0;
  const s = run(
    note('turn/started'),
    note('interaction/requested', {
      interactionId: 'q1',
      questions: [
        { id: 'a', question: '多选？', options: [], multiSelect: true },
        { id: 'b', question: '单选？', options: [] },
      ],
    }),
  );
  const item = pendingItems(s)[0];
  assert.deepEqual(
    item.kind === 'question' && item.questions.map((q) => q.multiSelect),
    [true, false],
  );
});

test('工具失败照网关的 status，不按输出猜', () => {
  seq = 0;
  const s = run(
    note('turn/started'),
    note('item/tool_started', { toolCallId: 'c', toolName: 'bash', arguments: { command: 'false' } }),
    note('item/tool_completed', { toolCallId: 'c', toolName: 'bash', output: 'Error 看着像错但成功了', status: 'completed' }),
  );
  const tool = s.turns[0].items[0];
  assert.equal(tool.kind === 'tool' && tool.status, 'completed');
});

test('权限门拒绝的工具是 denied，和普通失败分开', () => {
  seq = 0;
  const s = run(
    note('turn/started'),
    note('item/tool_started', { toolCallId: 'c', toolName: 'bash', arguments: { command: 'rm -rf x' } }),
    note('item/tool_completed', { toolCallId: 'c', toolName: 'bash', output: '权限被拒绝', status: 'denied' }),
  );
  const tool = s.turns[0].items[0];
  assert.equal(tool.kind === 'tool' && tool.status, 'denied');
});

test('没见过 turn/started 的事件（订阅晚了）补一轮，不丢', () => {
  seq = 0;
  const s = run(note('item/message_delta', { delta: '中途加入' }, 't9'));
  assert.equal(s.turns[0].id, 't9');
  assert.equal(s.turns[0].items[0].kind, 'text');
});

test('replayHistory 吃 thread/history 的 events 外形（payload 包一层、seq 在外面）', () => {
  const s = replayHistory([
    { method: 'turn/started', seq: 1, timestamp: '100000', payload: { turnId: 't1', status: 'running' } },
    { method: 'item/user_message', seq: 2, timestamp: '100000', payload: { turnId: 't1', content: 'hi' } },
    { method: 'turn/completed', seq: 3, timestamp: '130000', payload: { turnId: 't1', status: 'completed' } },
  ]);
  assert.equal(s.seq, 3);
  assert.equal(s.turns[0].endedAt! - s.turns[0].startedAt, 30000);
  assert.equal(s.turns[0].items[0].kind === 'user' && s.turns[0].items[0].text, 'hi');
});

test('没变的轮次保持同一个引用（UI 按引用比较）', () => {
  seq = 0;
  const s1 = run(note('turn/started', {}, 't1'), note('turn/completed', {}, 't1'), note('turn/started', {}, 't2'));
  const s2 = reduceThread(s1, note('item/message_delta', { delta: 'x' }, 't2'));
  assert.equal(s2.turns[0], s1.turns[0]);
  assert.notEqual(s2.turns[1], s1.turns[1]);
});

test('认不出的方法返回 null（协议只做加法）', () => {
  assert.equal(parseEvent('future/thing', {}), null);
});

test('思考：连续增量并成一条，来了正文就算想完（endedAt 取正文的时间）', () => {
  seq = 0;
  const s = run(
    note('turn/started', {}, 't1', 1000),
    note('item/reasoning_delta', { delta: '先看' }, 't1', 1100),
    note('item/reasoning_delta', { delta: '文件' }, 't1', 1500),
    note('item/message_delta', { delta: '好' }, 't1', 4100),
    note('item/reasoning_delta', { delta: '再想' }, 't1', 5000),
    note('turn/completed', { status: 'completed' }, 't1', 6000),
  );
  const items = s.turns[0].items;
  assert.deepEqual(
    items.map((i) => [i.kind, 'text' in i ? i.text : '']),
    [
      ['reasoning', '先看文件'],
      ['text', '好'],
      ['reasoning', '再想'],
    ],
  );
  const [first, , second] = items;
  assert.ok(first.kind === 'reasoning' && first.startedAt === 1100 && first.endedAt === 4100);
  // 一轮结束时还开着的思考也收口。
  assert.ok(second.kind === 'reasoning' && second.endedAt === 6000);
});

test('工具结果里的图：带网关给的 index，旧网关不带就是空数组', () => {
  seq = 0;
  const s = run(
    note('turn/started'),
    note('item/tool_started', { toolCallId: 'shot', toolName: 'mcp_cua-driver__get_window_state', arguments: {} }),
    note('item/tool_completed', {
      toolCallId: 'shot',
      toolName: 'mcp_cua-driver__get_window_state',
      output: 'tree',
      status: 'completed',
      attachments: [{ type: 'image', index: 1, mimeType: 'image/png', width: 1440, height: 900, byteLength: 2048 }],
    }),
    note('item/tool_started', { toolCallId: 'old', toolName: 'bash', arguments: {} }),
    note('item/tool_completed', { toolCallId: 'old', toolName: 'bash', output: '', status: 'completed' }),
  );
  const [shot, old] = s.turns[0].items;
  assert.deepEqual(shot.kind === 'tool' && shot.images.map((i) => [i.index, i.width, i.height]), [[1, 1440, 900]]);
  assert.deepEqual(old.kind === 'tool' && old.images, []);
});

/** 压缩进展只推不记：`seq` 为 0，不受去重影响。 */
function progress(params: Record<string, unknown>) {
  const event = parseEvent('context/compacted', { seq: 0, threadId: 'th', turnId: 't1', timestamp: '0', ...params });
  assert.ok(event);
  return event;
}

test('压缩：进展是线程级状态，完成标记落在轮里，完成推送把前后占用补到标记上', () => {
  seq = 0;
  let s = run(note('turn/started', {}, 't1'), note('item/user_message', { content: '读完整个仓库' }));
  s = reduceThread(
    s,
    progress({ status: 'running', trigger: 'auto', phase: 'summary', attempt: 2, maxAttempts: 3, retryReason: '摘要过短', outputTokens: 120, beforeTokens: 180000, elapsedMs: 4200 }),
  );
  assert.equal(s.compaction?.status, 'running');
  assert.equal(s.compaction?.attempt, 2);
  assert.equal(s.compaction?.retryReason, '摘要过短');
  assert.equal(s.compaction?.afterTokens, null);
  const sameSeq = s.seq;

  s = reduceThread(s, note('item/compaction', { itemId: 'compaction-9', status: 'completed' }));
  s = reduceThread(s, progress({ status: 'completed', trigger: 'auto', phase: 'apply', beforeTokens: 180000, afterTokens: 21000, elapsedMs: 9000 }));
  assert.equal(s.compaction?.status, 'completed');
  const item = s.turns[0].items.at(-1);
  assert.deepEqual(item?.kind === 'compaction' && [item.trigger, item.beforeTokens, item.afterTokens, item.elapsedMs], ['auto', 180000, 21000, 9000]);
  // 紧接着又压一次、没有新标记：同一个标记换成最新的数。
  s = reduceThread(s, progress({ status: 'completed', trigger: 'manual', phase: 'apply', beforeTokens: 21000, afterTokens: 15000, elapsedMs: 4000 }));
  const again = s.turns[0].items.at(-1);
  assert.deepEqual(again?.kind === 'compaction' && [again.trigger, again.beforeTokens, again.afterTokens], ['manual', 21000, 15000]);
  s = reduceThread(s, note('turn/started', {}, 't2'));
  assert.equal(s.compaction?.status, 'completed', '压成了的留着');
  assert.ok(s.seq > sameSeq, '标记占序号，进展不占');

  const replayed = replayHistory([
    { method: 'turn/started', seq: 1, timestamp: '1', payload: { turnId: 't1' } },
    { method: 'item/compaction', seq: 2, timestamp: '2', payload: { turnId: 't1', itemId: 'compaction-2' } },
  ]);
  const old = replayed.turns[0].items[0];
  assert.deepEqual(old.kind === 'compaction' && [old.trigger, old.beforeTokens, old.afterTokens], [null, null, null], '回放历史没有数字');
  assert.equal(replayed.compaction, null);
});

test('压缩失败与停止：带原因，标记不动', () => {
  seq = 0;
  let s = run(note('turn/started', {}, 't1'));
  s = reduceThread(s, progress({ status: 'failed', phase: 'summary', attempt: 3, maxAttempts: 3, error: '压缩失败：摘要过短。' }));
  assert.equal(s.compaction?.status, 'failed');
  assert.equal(s.compaction?.error, '压缩失败：摘要过短。');
  s = reduceThread(s, progress({ status: 'cancelled' }));
  assert.equal(s.compaction?.status, 'cancelled');
  s = reduceThread(s, note('turn/started', {}, 't2'));
  assert.equal(s.compaction, null, '下一轮开始，没压成的那次就交代完了');
  assert.equal(parseEvent('context/compacted', { status: 'weird' }), null);
});
