import { test } from 'node:test';
import assert from 'node:assert/strict';
import { EMPTY_TURN_CHANGES, parseEvent, reduceTurnChanges, reduceThread, seedTurnChanges, turnChangesOf, EMPTY_THREAD } from '../src/index.ts';

const pushed = (changes: Record<string, unknown>) =>
  parseEvent('turn/changes/updated', { seq: 0, threadId: 'th', turnId: changes.turnId, changes })!;

test('每轮改动：铺底、推送覆盖、全改回原样的不算有改动', () => {
  const seeded = seedTurnChanges([
    { turnId: 't1', files: [{ path: 'a.ts', absPath: '/p/a.ts', status: 'modified', added: 2, removed: 1 }], added: 2, removed: 1 },
    { files: [] },
  ]);
  assert.deepEqual(Object.keys(seeded.byTurn), ['t1']);
  const file = turnChangesOf(seeded, 't1')!.files[0];
  assert.deepEqual(file, { path: 'a.ts', absPath: '/p/a.ts', status: 'modified', added: 2, removed: 1, skipped: null });

  const event = pushed({ turnId: 't2', files: [{ path: 'big.bin', status: 'weird', skipped: '文件太大' }], reverted: true });
  const next = reduceTurnChanges(seeded, event);
  const t2 = turnChangesOf(next, 't2')!;
  assert.equal(t2.reverted, true);
  assert.equal(t2.files[0].status, 'modified', '认不出的状态当改过');
  assert.equal(t2.files[0].skipped, '文件太大');

  const emptied = reduceTurnChanges(next, pushed({ turnId: 't1', files: [] }));
  assert.equal(turnChangesOf(emptied, 't1'), null);
  assert.equal(reduceTurnChanges(EMPTY_TURN_CHANGES, parseEvent('turn/started', { seq: 1, turnId: 't1' })!), EMPTY_TURN_CHANGES);
  assert.equal(reduceThread(EMPTY_THREAD, event), EMPTY_THREAD, '线程状态不管它');
});
