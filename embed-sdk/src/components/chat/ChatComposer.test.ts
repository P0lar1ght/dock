import assert from 'node:assert/strict';
import test from 'node:test';

import { shouldComplete } from './ChatComposer.js';

const command = (name: string, available = true) => ({ name, title: name, description: '', available });

test('Enter completes a partial slash command', () => {
  assert.equal(shouldComplete({ draft: '/he', slashCommands: [command('/help')], slashCommandIndex: 0 }), true);
});

test('Enter sends when the draft already is the highlighted command', () => {
  assert.equal(shouldComplete({ draft: '/help', slashCommands: [command('/help')], slashCommandIndex: 0 }), false);
  assert.equal(shouldComplete({ draft: ' /help ', slashCommands: [command('/help')], slashCommandIndex: 0 }), false);
});

test('Enter does not complete an unavailable command', () => {
  assert.equal(shouldComplete({ draft: '/sc', slashCommands: [command('/screenshot', false)], slashCommandIndex: 0 }), false);
});
