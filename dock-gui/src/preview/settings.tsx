// 预览：设置页各分组（#settings-<分组>[-<状态>]）。dock.call 换成假数据，不连 Dock，
// 给无头浏览器截图对照 wonder-ui/settings。

import { useState } from "react";
import { dock } from "../dock/client";
import { store } from "../dock/store";
import { Settings, type SettingsSection } from "../components/settings/index";
import { ModelsPane } from "../components/settings/Models";
import { PluginsPane } from "../components/settings/Plugins";

const model = (id: string, label: string, apiBase: string, ctx: number, backends: string[], auth: string, extra = {}) => ({
  id,
  label,
  description: "",
  apiBase,
  contextWindow: ctx,
  backends,
  auth,
  keyEnv: null,
  keyEnvSet: false,
  editable: true,
  default: false,
  ...extra,
});

const MODELS = [
  model("atria-dawn-preview", "Atria-Dawn-Preview", "https://api.atria-asi.ai/v1", 262144, ["responses", "chat_completions", "messages"], "key"),
  model("atria-dawn-preview-intern", "Atria-Dawn-Preview (intern)", "https://discovery-api.intern-ai.org.cn/v1", 262144, ["messages"], "env", {
    keyEnv: "INTERN_API_KEY",
  }),
  model("deepseek-flash", "deepseek-flash", "https://api.deepseek.com/v1", 1048576, ["responses", "chat_completions", "messages"], "env", {
    keyEnv: "DEEPSEEK_API_KEY",
    keyEnvSet: true,
    default: true,
  }),
];

const tool = (name: string, enabled = true) => ({ name, description: "", enabled });
const MCP = [
  {
    name: "github",
    status: "connected",
    detail: "12 个工具",
    toolCount: 12,
    enabledToolCount: 10,
    tools: [tool("list_issues"), tool("create_issue"), tool("delete_repo", false)],
    editable: true,
    builtin: false,
  },
  { name: "cua-driver", status: "connected", detail: "", toolCount: 24, enabledToolCount: 24, tools: [], editable: false, builtin: true },
  { name: "linear", status: "needs_auth", detail: "需要登录", toolCount: 0, enabledToolCount: 0, tools: [], editable: true, builtin: false },
  {
    name: "slack",
    status: "failed",
    detail: "401 Unauthorized: token revoked",
    toolCount: 0,
    enabledToolCount: 0,
    tools: [],
    editable: true,
    builtin: false,
  },
];

const variant = location.hash.split("-").slice(2).join("-");

const CUA: Record<string, unknown> = {
  missing: { state: "missing", detail: "", driverPath: null, driverEnv: null, perms: null, job: null },
  installing: {
    state: "missing",
    detail: "",
    driverPath: null,
    driverEnv: null,
    perms: null,
    job: { action: "install", lines: ["$ curl -fsSL https://cua.ai/driver/install.sh | bash", "Downloading CuaDriver.app…", "Extracting…"], running: true, ok: null, message: null },
  },
  perm: {
    state: "needs_permission",
    detail: "",
    driverPath: "/Applications/CuaDriver.app",
    driverEnv: null,
    perms: { required: true, accessibility: false, screenRecording: true },
    job: null,
  },
  connected: {
    state: "connected",
    detail: "24 个工具",
    driverPath: "/Applications/CuaDriver.app",
    driverEnv: null,
    perms: { required: true, accessibility: true, screenRecording: true },
    job: null,
  },
  down: { state: "unreachable", detail: "找不到 /Applications/CuaDriver.app，或进程已退出", driverPath: null, driverEnv: null, perms: null, job: null },
};

const values: Record<string, unknown> = {
  "browser.headed": null,
  "toolset.web_fetch.timeout_secs": 60,
  "toolset.web_fetch.max_content_length": 10485760,
  "toolset.web_fetch.max_markdown_length": 100000,
  "toolset.web_fetch.allowed_domains": ["docs.rs", "developer.mozilla.org"],
  "toolset.web_fetch.proxy_endpoint": "http://127.0.0.1:7890",
  "toolset.web_fetch.allow_local": true,
  "memory.enabled": true,
  "memory.flush.soft_threshold_tokens": 4000,
  "memory.flush.max_flush_write_chars": 8000,
  "memory.dream.min_hours": 24,
  "memory.dream.min_sessions": 5,
  "memory.embedding.model": "text-embedding-3-small",
  "memory.embedding.base": "https://api.openai.com/v1",
  "memory.embedding.dimensions": 1024,
};

const FIXTURES: Record<string, (p: Record<string, unknown>) => unknown> = {
  "config/status": () => ({
    dockHome: "/Users/me/.dock",
    path: "/Users/me/.dock/config.toml",
    exists: true,
    error: variant === "broken" ? { message: "expected `.`, `=`", line: 42 } : null,
  }),
  "config/get": () => ({
    values,
    env: { DOCK_MODEL: null, DOCK_MEMORY: variant === "forced" ? "0" : null, DOCK_BROWSER_HEADED: null, DOCK_CUA_DRIVER: null },
    memoryForcedOff: variant === "forced",
  }),
  "config/env": (p) => Object.fromEntries((p.names as string[]).map((n) => [n, true])),
  "model/list": () => ({
    models: variant === "empty" ? [] : MODELS,
    default: "deepseek-flash",
    configDefault: "deepseek-flash",
  }),
  "model/get": () => ({
    model: {
      id: "deepseek-flash",
      name: "deepseek-flash",
      description: "DeepSeek flash",
      apiBaseUrl: "https://api.deepseek.com/v1",
      apiModel: null,
      apiBackends: ["responses", "chat_completions", "messages"],
      authScheme: null,
      keyMode: "env",
      envKey: "DEEPSEEK_API_KEY",
      hasApiKey: false,
      contextWindow: 1000000,
      maxOutputTokens: null,
      reasoning: true,
      reasoningEffort: "medium",
      reasoningEfforts: ["low", "medium", "high"],
      supportsImages: false,
      promptCache: true,
      pricing: { input: 0.14, cacheRead: 0.01, cacheWrite: 0.14, output: 0.28 },
      overrides: { messages: { apiBaseUrl: "https://api.deepseek.com/anthropic", authScheme: "bearer", apiModel: null } },
    },
  }),
  "model/test": () => ({ ok: true, ms: 420, backend: "responses" }),
  "mcp/list": () => ({ servers: MCP }),
  "cua/status": () => ({ installSteps: ["下载安装脚本", "安装 CuaDriver.app", "请求系统权限"], ...(CUA[variant] as object ?? CUA.connected) }),
  "cua/action": () => ({ installSteps: ["下载安装脚本", "安装 CuaDriver.app", "请求系统权限"], ...(CUA[variant] as object ?? CUA.connected) }),
  "browser/status": () => ({ state: "connected", detail: "18 个工具", headed: false, headedEnv: false }),
  "plugin/list": () => ({
    disk: [
      { id: "dock-telemetry", name: "Telemetry", purpose: "遥测与用量上报", scope: "user", project: null, root: "/Users/me/.dock/plugins", path: "/Users/me/.dock/plugins/dock-telemetry", enabled: true, loaded: true, running: true, shadowedBy: null, error: null },
      { id: "broken-demo", name: "broken-demo", purpose: "", scope: "user", project: null, root: "/Users/me/.dock/plugins", path: "/Users/me/.dock/plugins/broken-demo", enabled: false, loaded: false, running: false, shadowedBy: null, error: "rhai plugin needs source.rhai" },
      { id: "cyberspace", name: "Cyberspace Search", purpose: "鹰图 Hunter 与 FOFA 网络空间测绘 API 封装：注册为模型可调用的工具", scope: "project", project: "/Users/me/Desktop/AILab/dock", root: "/Users/me/Desktop/AILab/dock/.dock/plugins", path: "/Users/me/Desktop/AILab/dock/.dock/plugins/cyberspace", enabled: true, loaded: false, running: false, shadowedBy: null, error: null },
      { id: "acme-lint", name: "acme-lint", purpose: "项目内 lint 钩子", scope: "project", project: "/Users/me/work/acme", root: "/Users/me/work/acme/.dock/plugins", path: "/Users/me/work/acme/.dock/plugins/acme-lint", enabled: true, loaded: true, running: true, shadowedBy: null, error: null },
    ],
    session: [
      { id: "tmp-hook-3", name: "tmp-hook", purpose: "会话临时钩子", factory: "rhai", running: true, error: null, threadId: "t1", threadTitle: "重构 store", cwd: "/Users/me/work/acme" },
      { id: "probe-1", name: "probe", purpose: "", factory: "echo", running: false, error: null, threadId: null, threadTitle: null, cwd: null },
    ],
    userRoot: "/Users/me/.dock/plugins",
    projects: ["/Users/me/Desktop/AILab/dock", "/Users/me/work/acme"],
  }),
  "skill/list": () => ({
    skills: [
      { name: "create-workflow", description: "写一个 workflow 脚本", scope: "内置", path: "/Users/me/.dock/bundled/skills/create-workflow/SKILL.md", userInvocable: true, project: null, shadowed: false, shadowedIn: [] },
      { name: "review", description: "代码审查", scope: "用户", path: "/Users/me/.dock/skills/review/SKILL.md", userInvocable: true, project: null, shadowed: false, shadowedIn: ["/Users/me/work/acme"] },
      { name: "git-commit", description: "按仓库约定提交", scope: "agents", path: "/Users/me/Desktop/AILab/dock/.agents/skills/git-commit/SKILL.md", userInvocable: true, project: "/Users/me/Desktop/AILab/dock", shadowed: false, shadowedIn: [] },
      { name: "review", description: "acme 的审查规则", scope: "项目", path: "/Users/me/work/acme/.dock/skills/review/SKILL.md", userInvocable: true, project: "/Users/me/work/acme", shadowed: false, shadowedIn: [] },
    ],
  }),
  "secret/list": () => ({ names: ["github", "linear", "slack"], path: "/Users/me/.dock/secrets.json" }),
  "pairing/list": () => ({
    accepting: true,
    listen: "127.0.0.1:18991",
    remote: false,
    pending: [{ id: "p1", application: "page", origin: "http://localhost:5173", createdAt: Date.now(), expiresAt: Date.now() + 60000 }],
    bindings: [{ application: "docs", origin: "http://localhost:3000", boundAt: Date.now() - 2 * 86400000 }],
  }),
  "device/list": () => ({ devices: [{ id: "d1", name: "laptop", createdAt: 0, lastUsedAt: Date.now() - 2 * 3600000 }] }),
};

// 换掉真连接：只回假数据。
(dock as unknown as { call: (m: string, p?: Record<string, unknown>) => Promise<unknown> }).call = async (method, params = {}) => {
  const f = FIXTURES[method];
  if (!f) throw new Error(`${method}: 预览里没有假数据`);
  return f(params);
};
dock.serverInfo = { name: "dock", version: "0.1.2" };

export function isSettingsPreview() {
  return location.hash.startsWith("#settings-");
}

export function SettingsPreview() {
  const section = location.hash.split("-")[1] as SettingsSection;
  const [s, setS] = useState<SettingsSection>(section);
  // 预设管理用 store 里的预设：给两条假的。
  (store as unknown as { setApp?: (a: object) => void }).setApp?.({
    presets: [
      { id: "coding", label: "编码", description: "写代码、跑测试", builtin: true, origin: "shipped", available: true },
      { id: "watch", label: "守望", description: "多角色守望", builtin: true, origin: "user", available: true },
    ],
    defaultPreset: "coding",
  });
  if (location.hash === "#settings-skilltab") {
    return (
      <div className="flex justify-center px-10 py-8">
        <PluginsPane initialTab="skills" />
      </div>
    );
  }
  if (location.hash === "#settings-modeledit") {
    return (
      <div className="flex justify-center px-10 py-8">
        <ModelsPane editId="deepseek-flash" />
      </div>
    );
  }
  return (
    <div style={{ width: 1440, height: 900 }}>
      <Settings section={s} onSection={setS} onClose={() => {}} />
    </div>
  );
}
