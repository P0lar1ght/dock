import { test } from 'node:test';
import assert from 'node:assert/strict';
import { EMPTY_TURN_CARDS, applyTurnCards, parseTurnCardsPush, seedTurnCards, turnCardsOf } from '../src/index.ts';

test('插件卡片：铺底、推送整轮换掉、空了就删、认不出的字段兜底', () => {
  const seeded = seedTurnCards([
    {
      turnId: 't1',
      cards: [
        { id: 'c1', plugin: 'done-check', title: '还没跑测试', text: '改了 2 个 .rs', tone: 'warning', actions: [{ id: 'run', label: '跑测试' }, { id: 'skip' }, { label: '没 id' }], live: true },
        { id: 'c2', title: '' },
      ],
    },
    { turnId: 't2', cards: [] },
  ]);
  assert.deepEqual(Object.keys(seeded.byTurn), ['t1']);
  const [card] = turnCardsOf(seeded, 't1');
  assert.equal(card.tone, 'warning');
  assert.deepEqual(card.actions, [{ id: 'run', label: '跑测试' }, { id: 'skip', label: 'skip' }]);
  assert.equal(card.view, null);
  assert.equal(card.live, true);

  const push = parseTurnCardsPush('turn/cards/updated', {
    threadId: 'th',
    turnId: 't1',
    cards: [{ id: 'c1', title: '测试过了', tone: 'neon', view: { type: 'text', text: 'ok' }, live: false }],
  })!;
  assert.equal(push.threadId, 'th');
  const next = applyTurnCards(seeded, push.turnId, push.cards);
  const [updated] = turnCardsOf(next, 't1');
  assert.equal(updated.tone, 'default', '认不出的配色当默认');
  assert.equal(updated.live, false);
  assert.ok(updated.view, '视图树解析了');

  assert.deepEqual(turnCardsOf(applyTurnCards(next, 't1', []), 't1'), []);
  assert.equal(parseTurnCardsPush('turn/changes/updated', { threadId: 'th', turnId: 't1' }), null);
  assert.deepEqual(turnCardsOf(EMPTY_TURN_CARDS, 'x'), []);
});
