import assert from 'node:assert/strict';
import { test } from 'node:test';

import {
  appAllow,
  appCsp,
  appKey,
  appToolArgs,
  appToolFor,
  cleanSources,
  looksLikeMcpTool,
  parseMcpAppCallRecord,
  parseMcpAppResource,
  parseMcpAppTools,
  parseViewCall,
  viewToolsForExpose,
} from '../src/index.ts';

const tools = parseMcpAppTools({
  tools: [
    { server: 'board', name: 'draw', publicName: 'mcp_board__draw', resourceUri: 'ui://board/app.html', modelVisible: true, appVisible: true },
    { server: 'board', name: 'save', publicName: 'mcp_board__save', resourceUri: 'ui://board/app.html', modelVisible: false },
    { server: 'board', name: 'load', resourceUri: null, modelVisible: false },
    { server: 'x', name: 'bad', resourceUri: 'https://evil/app' },
    { name: 'no-server' },
  ],
});

test('mcp/app/tools：缺省补齐，不是 ui:// 的资源不认，缺 server / name 的丢掉', () => {
  assert.equal(tools.length, 4);
  assert.equal(tools[1].modelVisible, false);
  assert.equal(tools[1].appVisible, true);
  assert.equal(tools[2].publicName, 'mcp_board__load');
  assert.equal(tools[3].resourceUri, null);
  assert.deepEqual(parseMcpAppTools(null), []);
});

test('工具卡认界面：直调和经 use_tool 都认，没有资源的不算', () => {
  assert.equal(appToolFor(tools, 'mcp_board__draw', {})?.name, 'draw');
  assert.equal(appToolFor(tools, 'use_tool', { tool_name: 'mcp_board__draw', tool_input: {} })?.name, 'draw');
  assert.equal(appToolFor(tools, 'mcp_board__load', {}), null);
  assert.equal(appToolFor(tools, 'bash', {}), null);
  assert.equal(looksLikeMcpTool('use_tool', { tool_name: 'mcp_new__x' }), true);
  assert.equal(looksLikeMcpTool('bash', {}), false);
  assert.equal(appKey('board', 'ui://board/app.html'), 'board ui://board/app.html');
});

test('mcp/app/resource 与 mcp/app/result 的外形', () => {
  const r = parseMcpAppResource({
    uri: 'ui://board/app.html',
    mimeType: 'text/html;profile=mcp-app',
    html: '<p>hi</p>',
    csp: { connectDomains: ['https://api.x'] },
    permissions: { clipboardWrite: {} },
    domain: null,
    prefersBorder: false,
  });
  assert.equal(r?.html, '<p>hi</p>');
  assert.deepEqual(r?.csp?.resourceDomains, []);
  assert.equal(r?.prefersBorder, false);
  assert.equal(parseMcpAppResource({ uri: 'ui://a' }), null);

  const call = parseMcpAppCallRecord({
    call: { server: 'board', tool: 'draw', resourceUri: 'ui://board/app.html', arguments: { a: 1 }, result: { content: [] } },
  });
  assert.equal(call?.arguments.a, 1);
  assert.equal(parseMcpAppCallRecord({ call: null }), null);
});

test('CSP：默认不联网；来源只认干净的 scheme://host，塞不进别的指令', () => {
  const none = appCsp(null);
  assert.match(none, /connect-src 'none'/);
  assert.match(none, /frame-src 'none'/);
  assert.match(none, /default-src 'none'/);

  const csp = appCsp({
    connectDomains: ['https://api.x.com', "https://a.com; script-src *", 'http://evil.com', 'http://localhost:5173'],
    resourceDomains: ['https://*.cdn.com/', "'unsafe-hashes'", '*', 'data:'],
    frameDomains: ['https://www.youtube.com'],
    baseUriDomains: [],
  });
  assert.match(csp, /connect-src https:\/\/api\.x\.com http:\/\/localhost:5173;/);
  assert.match(csp, /script-src [^;]*https:\/\/\*\.cdn\.com/);
  assert.match(csp, /frame-src https:\/\/www\.youtube\.com;/);
  // Worker 也是脚本：资源域名要放开（Cesium 从 CDN 起 Worker）。
  assert.match(csp, /worker-src blob: https:\/\/\*\.cdn\.com;/);
  assert.match(appCsp(null), /worker-src blob:;/);
  assert.doesNotMatch(csp, /evil|a\.com;|unsafe-hashes| \*[ ;]/);
  assert.deepEqual(cleanSources(['https://a.com', 'https://a.com/']), ['https://a.com']);
});

test('权限 → iframe allow：只照给写剪贴板，摄像头 / 麦克风 / 定位不给', () => {
  assert.equal(appAllow({ clipboardWrite: {}, camera: {}, microphone: {}, geolocation: {} }), 'clipboard-write');
  assert.equal(appAllow(null), '');
});

test('交给界面的工具输入：use_tool 拆出里面那颗的参数（对象或 JSON 字符串都认）', () => {
  assert.deepEqual(appToolArgs('mcp_board__draw', { a: 1 }), { a: 1 });
  assert.deepEqual(appToolArgs('use_tool', { tool_name: 'mcp_board__draw', tool_input: { a: 1 } }), { a: 1 });
  assert.deepEqual(appToolArgs('use_tool', { tool_name: 'mcp_board__draw', tool_input: '{"a":2}' }), { a: 2 });
  assert.deepEqual(appToolArgs('use_tool', { tool_name: 'x', tool_input: 'not json' }), {});
  assert.deepEqual(appToolArgs('use_tool', { tool_name: 'x' }), {});
});

test('viewToolsForExpose keeps named tools once, with string descriptions and object schemas', () => {
  const tools = viewToolsForExpose({
    tools: [
      { name: 'get_selection', description: '选中的元素', inputSchema: { type: 'object' }, _meta: { x: 1 } },
      { name: 'get_selection', description: '重复的' },
      { name: '', description: '没名字' },
      { name: 'focus', description: 3, inputSchema: [1] },
      'junk',
    ],
  });
  assert.deepEqual(tools, [
    { name: 'get_selection', description: '选中的元素', inputSchema: { type: 'object' } },
    { name: 'focus' },
  ]);
  assert.deepEqual(viewToolsForExpose(null), []);
  assert.deepEqual(viewToolsForExpose({ tools: 'no' }), []);
});

test('parseViewCall needs view, call and tool name; arguments default to {}', () => {
  assert.deepEqual(parseViewCall({ viewId: 'v1', callId: 'c1', name: 'focus', arguments: { ids: ['a'] } }), {
    viewId: 'v1',
    callId: 'c1',
    name: 'focus',
    arguments: { ids: ['a'] },
  });
  assert.deepEqual(parseViewCall({ viewId: 'v1', callId: 'c1', name: 'focus', arguments: [1] })?.arguments, {});
  assert.equal(parseViewCall({ viewId: 'v1', name: 'focus' }), null);
  assert.equal(parseViewCall('x'), null);
});
