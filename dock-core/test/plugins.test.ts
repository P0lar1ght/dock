import assert from 'node:assert/strict';
import { test } from 'node:test';

import {
  MAX_DEPTH,
  parsePluginPush,
  parseStatusItems,
  parseSurface,
  parseSurfaceAction,
  parseSurfaceList,
  parseSurfaceWeb,
  parseToolView,
  parseToolViews,
  parseView,
  splitPanelFooter,
  viewActions,
  viewToolName,
  type ViewNode,
} from '../src/index.ts';

test('视图树：字符串当 text，缺省值补齐，不认识的节点留占位', () => {
  const v = parseView({
    type: 'stack',
    children: [
      '你好',
      { type: 'kv', items: [{ label: '版本', value: 3 }, { label: '状态', value: '运行中', tone: 'success' }] },
      { type: 'progress', value: 1.7, label: '上传中' },
      { type: 'chart' },
      {},
    ],
  });
  assert.deepEqual(v, {
    type: 'stack',
    gap: 'm',
    children: [
      { type: 'text', text: '你好', tone: 'default', weight: 'normal', size: 'm' },
      {
        type: 'kv',
        items: [
          { label: '版本', value: '3', tone: 'default' },
          { label: '状态', value: '运行中', tone: 'success' },
        ],
      },
      { type: 'progress', value: 1, label: '上传中' },
      { type: 'unsupported', kind: 'chart' },
      { type: 'unsupported', kind: '（缺 type）' },
    ],
  });
});

test('视图树：不安全的链接改成灰字', () => {
  assert.deepEqual(parseView({ type: 'link', label: '日志', url: 'javascript:alert(1)' }), {
    type: 'text',
    text: '日志（已拦下不安全的链接）',
    tone: 'muted',
    weight: 'normal',
    size: 'm',
  });
  assert.equal(parseView({ type: 'link', label: '日志', url: 'HTTPS://x.dev' }).type, 'link');
});

test('视图树：太深、条目太多都截掉，末尾补一行说明', () => {
  let deep: unknown = 'leaf';
  for (let i = 0; i < MAX_DEPTH + 2; i += 1) deep = { type: 'stack', children: [deep] };
  const v = parseView(deep);
  assert.equal(v.type, 'stack');
  const last = (v as ViewNode & { type: 'stack' }).children.at(-1);
  assert.deepEqual(last, { type: 'text', text: '（视图过大，后面的部分没有显示）', tone: 'muted', weight: 'normal', size: 's' });

  const rows = Array.from({ length: 600 }, (_, i) => [`r${i}`]);
  const big = parseView({ type: 'table', columns: ['c'], rows });
  assert.equal(big.type, 'stack', '截断时外面包一层');
  const table = (big as ViewNode & { type: 'stack' }).children[0] as ViewNode & { type: 'table' };
  assert.equal(table.rows.length, 499, '表格自己占 1 个，条目最多 499');
});

test('动作：按出现顺序去重，默认跳过折叠的 section', () => {
  const v = parseView({
    type: 'stack',
    children: [
      { type: 'list', items: [{ title: 'a', action: 'open' }, { title: 'b' }] },
      { type: 'section', title: '更多', collapsed: true, children: [{ type: 'button', label: '归档', action: 'archive' }] },
      { type: 'button', label: '打开', action: 'open' },
      { type: 'empty', title: '空', action: 'deploy', label: '立即部署' },
    ],
  });
  assert.deepEqual(viewActions(v), ['open', 'deploy']);
  assert.deepEqual(viewActions(v, true), ['open', 'archive', 'deploy']);
});

test('面板底部按钮：根 stack 的最后一行全是按钮才固定到底部', () => {
  const fixed = parseView({
    type: 'stack',
    children: [
      { type: 'kv', items: [] },
      { type: 'row', children: [{ type: 'button', label: '部署', action: 'deploy', style: 'primary' }] },
    ],
  });
  const { body, footer } = splitPanelFooter(fixed);
  assert.equal(footer?.children.length, 1);
  assert.equal((body as ViewNode & { type: 'stack' }).children.length, 1);

  const mixed = parseView({
    type: 'stack',
    children: [{ type: 'row', children: [{ type: 'badge', text: '成功' }, { type: 'button', label: 'x', action: 'x' }] }],
  });
  assert.equal(splitPanelFooter(mixed).footer, null, '混了别的节点就留在正文里');
  assert.equal(splitPanelFooter(parseView({ type: 'text', text: 'x' })).footer, null);
});

test('插件面板：列表、单个面板、动作结果', () => {
  assert.deepEqual(parseSurfaceList({ surfaces: [{ id: 'memo', title: '便签', hud: true }, { id: 'traj', title: '轨迹', web: true }, { title: '没 id' }] }), [
    { id: 'memo', title: '便签', hud: true, web: false },
    { id: 'traj', title: '轨迹', hud: false, web: true },
  ]);
  assert.equal(parseSurfaceWeb({ html: '<p>x</p>' }), '<p>x</p>');
  assert.equal(parseSurfaceWeb({}), null);
  const s = parseSurface({
    id: 'deploy',
    title: '部署助手',
    body: '环境：prod',
    view: { type: 'text', text: '环境：prod' },
    actions: [{ id: 'deploy', label: '部署' }, { id: 'rollback' }],
  });
  assert.equal(s?.view?.type, 'text');
  assert.deepEqual(s?.actions, [
    { id: 'deploy', label: '部署' },
    { id: 'rollback', label: 'rollback' },
  ]);
  assert.equal(parseSurface({ id: 'x', title: 'X', body: '', view: null, actions: [] })?.view, null);
  assert.deepEqual(parseSurfaceAction({ closed: false, surface: null }), { closed: true, surface: null }, '插件在动作里注销了面板');
});

test('状态项：文字原样，色调认不出算 default，空的丢掉', () => {
  assert.deepEqual(
    parseStatusItems({
      items: [
        { id: 'deploy', text: '部署中 60%', tone: 'accent', tooltip: '部署助手', surface: 'deploy' },
        { id: 'ci', text: 'CI 通过', tone: 'rainbow', tooltip: null, surface: null },
        { id: 'blank', text: '' },
      ],
    }),
    [
      { id: 'deploy', text: '部署中 60%', tone: 'accent', tooltip: '部署助手', surface: 'deploy' },
      { id: 'ci', text: 'CI 通过', tone: 'default', tooltip: null, surface: null },
    ],
  );
});

test('工具卡视图：透过 use_tool 找里面那颗；view 为 null 用通用卡片', () => {
  assert.equal(viewToolName('use_tool', { tool_name: 'deploy_status', tool_input: {} }), 'deploy_status');
  assert.equal(viewToolName('use_tool', {}), 'use_tool');
  assert.equal(viewToolName('bash', { tool_name: 'x' }), 'bash');
  assert.deepEqual(parseToolViews({ tools: ['deploy_status', 3, ''] }), ['deploy_status']);
  assert.equal(parseToolView({ view: null }), null);
  assert.equal(parseToolView({ view: { type: 'divider' } })?.type, 'divider');
});

test('连接级推送：认得的三种，别的回 null', () => {
  assert.deepEqual(parsePluginPush('surface/changed', { id: 'memo' }), { kind: 'surface', id: 'memo' });
  assert.deepEqual(parsePluginPush('status/changed', { id: 'ci' }), { kind: 'status', id: 'ci' });
  assert.deepEqual(parsePluginPush('tool/views/changed', { name: 'bash' }), { kind: 'toolViews', name: 'bash' });
  assert.equal(parsePluginPush('turn/started', {}), null);
});
