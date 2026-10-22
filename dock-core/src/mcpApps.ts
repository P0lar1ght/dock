// MCP Apps（`io.modelcontextprotocol/ui`，规范 2026-01-26）的宿主一侧：`mcp/app/*` 的结果外形、
// 工具卡认哪个界面、沙箱 iframe 的 CSP。见 cordis-gateway/README.md「MCP Apps」。

import { viewToolName } from './plugins.ts';

/** `mcp/app/tools` 的一项：带界面、或只给界面用的 MCP 工具。 */
export interface McpAppTool {
  server: string;
  /** 服务器上的原名。 */
  name: string;
  /** 模型看到的公名 `mcp_{server}__{tool}`。 */
  publicName: string;
  description: string;
  inputSchema: Record<string, unknown>;
  /** `ui://…`；只给界面用、本身不带界面的工具是 `null`。 */
  resourceUri: string | null;
  modelVisible: boolean;
  appVisible: boolean;
}

/** 资源 `_meta.ui.csp`：界面要连的 / 要加载的外部来源。 */
export interface McpAppCsp {
  connectDomains: string[];
  resourceDomains: string[];
  frameDomains: string[];
  baseUriDomains: string[];
}

/** `mcp/app/resource` 的结果。 */
export interface McpAppResource {
  uri: string;
  mimeType: string | null;
  html: string;
  csp: McpAppCsp | null;
  /** `{ camera?, microphone?, geolocation?, clipboardWrite? }`，原样。 */
  permissions: Record<string, unknown> | null;
  domain: string | null;
  prefersBorder: boolean | null;
}

/** `mcp/app/result` 的 `call`：模型那次调用存下的原始结果。 */
export interface McpAppCallRecord {
  server: string;
  tool: string;
  resourceUri: string;
  arguments: Record<string, unknown>;
  /** 规范的 `CallToolResult`（`content` / `structuredContent` / `isError` / `_meta`）。 */
  result: Record<string, unknown>;
}

const obj = (v: unknown): Record<string, unknown> =>
  v !== null && typeof v === 'object' && !Array.isArray(v) ? (v as Record<string, unknown>) : {};
const str = (v: unknown): string => (typeof v === 'string' ? v : '');
const strings = (v: unknown): string[] => (Array.isArray(v) ? v.filter((s): s is string => typeof s === 'string') : []);

export function parseMcpAppTools(result: unknown): McpAppTool[] {
  const tools = obj(result).tools;
  if (!Array.isArray(tools)) return [];
  const out: McpAppTool[] = [];
  for (const raw of tools) {
    const t = obj(raw);
    const server = str(t.server);
    const name = str(t.name);
    if (!server || !name) continue;
    const uri = str(t.resourceUri);
    out.push({
      server,
      name,
      publicName: str(t.publicName) || `mcp_${server}__${name}`,
      description: str(t.description),
      inputSchema: obj(t.inputSchema),
      resourceUri: uri.startsWith('ui://') ? uri : null,
      modelVisible: t.modelVisible !== false,
      appVisible: t.appVisible !== false,
    });
  }
  return out;
}

function parseCsp(v: unknown): McpAppCsp | null {
  if (v === null || typeof v !== 'object' || Array.isArray(v)) return null;
  const c = obj(v);
  return {
    connectDomains: strings(c.connectDomains),
    resourceDomains: strings(c.resourceDomains),
    frameDomains: strings(c.frameDomains),
    baseUriDomains: strings(c.baseUriDomains),
  };
}

/** 没有 `html` 的结果不算界面：回 `null`。 */
export function parseMcpAppResource(result: unknown): McpAppResource | null {
  const r = obj(result);
  if (typeof r.html !== 'string') return null;
  const permissions = r.permissions;
  return {
    uri: str(r.uri),
    mimeType: typeof r.mimeType === 'string' ? r.mimeType : null,
    html: r.html,
    csp: parseCsp(r.csp),
    permissions: permissions !== null && typeof permissions === 'object' && !Array.isArray(permissions) ? obj(permissions) : null,
    domain: typeof r.domain === 'string' && r.domain ? r.domain : null,
    prefersBorder: typeof r.prefersBorder === 'boolean' ? r.prefersBorder : null,
  };
}

export function parseMcpAppCallRecord(result: unknown): McpAppCallRecord | null {
  const call = obj(result).call;
  if (call === null || typeof call !== 'object') return null;
  const c = obj(call);
  const server = str(c.server);
  const uri = str(c.resourceUri);
  if (!server || !uri.startsWith('ui://')) return null;
  return {
    server,
    tool: str(c.tool),
    resourceUri: uri,
    arguments: obj(c.arguments),
    result: obj(c.result),
  };
}

/**
 * 这张工具卡挂哪个界面：按调用的工具名（经 `use_tool` 的按里面那颗）在 `mcp/app/tools`
 * 里找带 `resourceUri` 的那项。不是 MCP App 工具回 `null`。
 */
export function appToolFor(tools: readonly McpAppTool[], toolName: string, args: unknown): McpAppTool | null {
  const name = viewToolName(toolName, args);
  return tools.find((t) => t.publicName === name && t.resourceUri !== null) ?? null;
}

/**
 * 交给界面的工具输入（`ui/notifications/tool-input` 的 `arguments`）：经 `use_tool` 调的按需工具，
 * 是里面那颗的 `tool_input`（对象，或 Dock 也认的 JSON 字符串），不是 `use_tool` 自己那层。
 */
export function appToolArgs(toolName: string, args: unknown): Record<string, unknown> {
  const a = obj(args);
  if (toolName !== 'use_tool') return a;
  const input = a.tool_input;
  if (typeof input === 'string') {
    try {
      return obj(JSON.parse(input));
    } catch {
      return {};
    }
  }
  return obj(input);
}

/** 名字像 MCP 工具（`mcp_{server}__{tool}`）：`mcp/app/tools` 里没有时值得重拉一次。 */
export function looksLikeMcpTool(toolName: string, args: unknown): boolean {
  const name = viewToolName(toolName, args);
  return name.startsWith('mcp_') && name.includes('__');
}

/** 一个界面实例的身份：同一服务器、同一 `ui://` 资源。常驻面板按它（再加项目）去重。 */
export function appKey(server: string, resourceUri: string): string {
  return `${server} ${resourceUri}`;
}

// ---- 沙箱 CSP ----

// 只放行干净的来源：scheme://host[:port]，host 可带一个前导 `*.`。别的（带引号、分号、
// 空格、路径、`*`、`data:` 之类）一律丢掉——来源是服务器给的，不能让它往 CSP 里塞指令。
const SOURCE = /^(https|wss|http|ws):\/\/(\*\.)?[a-z0-9-]+(\.[a-z0-9-]+)*(:\d{1,5})?$/i;
const LOOPBACK_HOST = /^(https?|wss?):\/\/(localhost|127\.0\.0\.1)(:\d{1,5})?$/i;

/** 能进 CSP 的来源。明文 `http` / `ws` 只认本机（开发服务器）。 */
export function cleanSources(list: readonly string[]): string[] {
  const out: string[] = [];
  for (const raw of list) {
    const s = raw.trim().replace(/\/+$/, '');
    if (!SOURCE.test(s)) continue;
    if (/^(http|ws):/i.test(s) && !LOOPBACK_HOST.test(s)) continue;
    if (!out.includes(s)) out.push(s);
  }
  return out;
}

/**
 * 界面文档的 CSP（规范「Security」）：默认不联网；`resourceDomains` 放开脚本 / 样式 /
 * 图片 / 字体 / 媒体，`connectDomains` 放开 fetch / WebSocket，`frameDomains` 放开内嵌页，
 * `baseUriDomains` 放开 `<base>`。内联脚本样式、`data:` / `blob:` 资源总是允许（单文件界面
 * 都这么打包）。
 */
export function appCsp(csp: McpAppCsp | null): string {
  const res = cleanSources(csp?.resourceDomains ?? []);
  const connect = cleanSources(csp?.connectDomains ?? []);
  const frames = cleanSources(csp?.frameDomains ?? []);
  const base = cleanSources(csp?.baseUriDomains ?? []);
  const join = (...parts: string[]) => parts.filter(Boolean).join(' ');
  const r = res.join(' ');
  // 'unsafe-eval'：单文件打包的界面（Excalidraw 之类）里常有 new Function。界面跑在只给
  // allow-scripts 的沙箱 iframe 里（不同源），碰不到宿主，所以放开它不扩大能拿到的东西。
  return [
    "default-src 'none'",
    `script-src ${join("'unsafe-inline'", "'unsafe-eval'", 'data:', 'blob:', r)}`,
    `style-src ${join("'unsafe-inline'", 'data:', 'blob:', r)}`,
    `img-src ${join('data:', 'blob:', r)}`,
    `font-src ${join('data:', 'blob:', r)}`,
    `media-src ${join('data:', 'blob:', r)}`,
    `connect-src ${connect.length ? connect.join(' ') : "'none'"}`,
    "worker-src blob:",
    `frame-src ${frames.length ? frames.join(' ') : "'none'"}`,
    "object-src 'none'",
    `base-uri ${base.length ? base.join(' ') : "'none'"}`,
    "form-action 'none'",
  ].join('; ');
}

/**
 * 资源 `_meta.ui.permissions` → iframe `allow`（规范：宿主可以照给，界面不该假定拿到）。
 * 只照给写剪贴板；摄像头、麦克风、定位要宿主先问过用户，现在一律不给。
 */
export function appAllow(permissions: Record<string, unknown> | null): string {
  if (!permissions) return '';
  const map: [string, string][] = [['clipboardWrite', 'clipboard-write']];
  return map
    .filter(([key]) => permissions[key] !== undefined && permissions[key] !== null && permissions[key] !== false)
    .map(([, feature]) => feature)
    .join('; ');
}
