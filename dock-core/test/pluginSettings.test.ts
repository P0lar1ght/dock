import assert from 'node:assert/strict';
import { test } from 'node:test';

import {
  checkSettingsField,
  parsePluginSettings,
  parsePluginSettingsList,
  parsePluginSettingsPush,
  parsePluginSettingsSet,
  settingsErrors,
} from '../src/index.ts';

/** 网关 `plugin/settings/get` 的外形（schema 已规范化）。 */
const snapshot = parsePluginSettings({
  pluginId: 'deploy',
  schema: {
    title: '部署助手',
    fields: [
      { key: 'region', type: 'select', label: '区域', description: null, default: 'cn', required: false, options: ['cn', 'us'] },
      { key: 'token', type: 'secret', label: '访问令牌', description: '只进不出', default: null, required: false },
      { key: 'verbose', type: 'boolean', label: '详细日志', description: null, default: null, required: false },
      { key: 'timeout', type: 'number', label: '超时秒数', description: '10–600', default: 30, required: false, min: 10, max: 600 },
      { key: 'host', type: 'string', label: 'host', description: null, default: null, required: true },
      { key: 'chart', type: 'chart', label: '未来的类型' },
    ],
  },
  values: { region: 'cn', verbose: null, timeout: 30, host: null },
  secrets: { token: true },
})!;

test('快照：认不出 type 的字段丢掉，密钥只有设没设', () => {
  assert.equal(snapshot.schema.title, '部署助手');
  assert.deepEqual(
    snapshot.schema.fields.map((f) => f.key),
    ['region', 'token', 'verbose', 'timeout', 'host'],
  );
  assert.equal(snapshot.schema.fields[3].min, 10);
  assert.equal(snapshot.schema.fields[1].description, '只进不出');
  assert.deepEqual(snapshot.secrets, { token: true });
  assert.equal(parsePluginSettings({ schema: {} }), null);
});

test('单个字段：文案和 Dock 一样，数字串转成数', () => {
  const [region, , verbose, timeout, host] = snapshot.schema.fields;
  assert.deepEqual(checkSettingsField(timeout, '5'), { ok: false, error: '最小 10' });
  assert.deepEqual(checkSettingsField(timeout, 601), { ok: false, error: '最大 600' });
  assert.deepEqual(checkSettingsField(timeout, ' 45 '), { ok: true, value: 45 });
  assert.deepEqual(checkSettingsField(timeout, ''), { ok: false, error: '要一个数' });
  assert.deepEqual(checkSettingsField(region, 'eu'), { ok: false, error: '只能是 cn / us' });
  assert.deepEqual(checkSettingsField(verbose, 'yes'), { ok: false, error: '要开或关' });
  assert.deepEqual(checkSettingsField(host, '  '), { ok: false, error: '必填' });
  assert.deepEqual(checkSettingsField(host, 'a\nb'), { ok: false, error: '只能一行' });
});

test('一组值：有错才禁用保存；密钥空串不改；null 清回默认，必填没默认不能清', () => {
  assert.deepEqual(settingsErrors(snapshot.schema, { region: 'us', token: '', timeout: '60' }), {});
  assert.deepEqual(settingsErrors(snapshot.schema, { timeout: 5, host: null, nope: 1 }), {
    timeout: '最小 10',
    host: '必填',
    nope: '没有这个字段',
  });
  assert.deepEqual(settingsErrors(snapshot.schema, { region: null }), {}, '有 default 的可以清');
});

test('列表、保存结果、推送', () => {
  assert.deepEqual(parsePluginSettingsList({ plugins: [{ pluginId: 'deploy', title: '部署助手' }, {}] }), [
    { pluginId: 'deploy', title: '部署助手' },
  ]);
  assert.deepEqual(parsePluginSettingsSet({ ok: false, errors: { timeout: '最小 10' } }), {
    ok: false,
    errors: { timeout: '最小 10' },
  });
  const saved = parsePluginSettingsSet({ ok: true, settings: { pluginId: 'deploy', schema: { title: 'x', fields: [] }, values: {}, secrets: {} } });
  assert.equal(saved.ok && saved.settings?.pluginId, 'deploy');
  assert.deepEqual(parsePluginSettingsPush('plugin/settings/changed', { pluginId: 'deploy' }), { pluginId: 'deploy' });
  assert.equal(parsePluginSettingsPush('status/changed', { id: 'x' }), null);
});
