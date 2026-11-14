// 设置页用的 dock.1 方法（只有桌面 GUI 的受信连接能调，见网关 handlers/settings.rs）。
// 读写都只碰 ~/.dock/config.toml 与 $DOCK_HOME 下的文件；密钥只进不出。

import { dock, RpcError } from "./client";
import type { McpServerInfo, ModelInfo } from "./store";

/** 错误给人看的那句：去掉 `method: ` 前缀。 */
export function errorText(e: unknown): string {
  if (e instanceof RpcError) return e.body.message;
  return e instanceof Error ? e.message : String(e);
}

// ---- 配置文件 ----

export interface ConfigStatus {
  dockHome: string;
  path: string;
  exists: boolean;
  error: { message: string; line: number | null } | null;
}

export const configStatus = () => dock.call<ConfigStatus>("config/status");

export interface ConfigValues {
  values: Record<string, unknown>;
  env: Record<"DOCK_MODEL" | "DOCK_MEMORY" | "DOCK_BROWSER_HEADED" | "DOCK_CUA_DRIVER", string | null>;
  memoryForcedOff: boolean;
}

export const configGet = () => dock.call<ConfigValues>("config/get");

/** `value: null` = 删掉这个键，回到默认。 */
export const configSet = (key: string, value: unknown) =>
  dock.call<{ values: Record<string, unknown>; appliesNextLaunch: boolean }>("config/set", { key, value });

// ---- 模型 ----

export interface ModelEntry {
  id: string;
  name: string | null;
  description: string | null;
  apiBaseUrl: string | null;
  apiModel: string | null;
  apiBackends: string[];
  authScheme: string | null;
  keyMode: "env" | "inline";
  envKey: string | null;
  /** 只进不出：`undefined` = 保留文件里的，`""` = 删掉。 */
  apiKey?: string;
  hasApiKey: boolean;
  contextWindow: number | null;
  maxOutputTokens: number | null;
  reasoning: boolean | null;
  reasoningEffort: string | null;
  reasoningEfforts: string[] | null;
  supportsImages: boolean | null;
  promptCache: boolean | null;
  pricing: { input: number | null; cacheRead: number | null; cacheWrite: number | null; output: number | null } | null;
  overrides: Record<string, { apiBaseUrl: string | null; authScheme: string | null; apiModel: string | null }>;
  /** 连接继承自哪个 `[provider.<id>]`；设了它，地址、协议、密钥都可以不写。 */
  provider?: string | null;
}

export const EMPTY_MODEL: ModelEntry = {
  id: "",
  name: null,
  description: null,
  apiBaseUrl: null,
  apiModel: null,
  apiBackends: ["responses"],
  authScheme: null,
  keyMode: "env",
  envKey: null,
  hasApiKey: false,
  contextWindow: null,
  maxOutputTokens: null,
  reasoning: null,
  reasoningEffort: null,
  reasoningEfforts: null,
  supportsImages: null,
  promptCache: null,
  pricing: null,
  overrides: {},
};

export interface ModelListing {
  models: ModelInfo[];
  default: string;
  configDefault: string | null;
}

export const modelListing = () => dock.call<ModelListing>("model/list");
export const modelGet = async (id: string) => (await dock.call<{ model: ModelEntry }>("model/get", { id })).model;
export const modelSave = (model: ModelEntry, originalId: string | null, makeDefault: boolean) =>
  dock.call<ModelListing>("model/save", { model, originalId, makeDefault });
export const modelDelete = (id: string) => dock.call<ModelListing>("model/delete", { id });
export const modelDefault = (id: string) => dock.call<ModelListing>("model/default", { id });

/** 订阅通道的账号状态（`auth/status`，只给受信连接）。老 Dock 没有这个方法，调用会失败。 */
export interface AuthAccount {
  clientId: string;
  email: string | null;
  /** 还登着。false = 登出了，但注册还在：「Continue with …」直接复用它。 */
  signedIn: boolean;
  /** 这份授权能不能拿套餐额度推理。 */
  planUsage: boolean;
  /** access token 过期时刻（unix 秒；登出了是 0）。 */
  expiresAt: number;
  active: boolean;
}

/** 最近一次登录的进度。 */
export interface AuthLoginState {
  state: "pending" | "failed" | "succeeded";
  /** pending：授权页地址。 */
  url?: string | null;
  /** failed：给人看的那句话。 */
  message?: string;
  clientId?: string;
}

/** 订阅通道自动发现、已并入模型目录的一个模型（活动账号的）。 */
export interface AuthModel {
  /** 模型目录里的 id（`chatgpt/<slug>`），可以直接设成默认。 */
  id: string;
  slug: string;
  label: string;
  /** 上游给的一句介绍（英文原样）。 */
  description: string | null;
  contextWindow: number | null;
  reasoningEfforts: string[] | null;
  defaultReasoningEffort: string | null;
  supportsImages: boolean | null;
}

export interface AuthProviderStatus {
  id: string;
  /** 给人看的名字（"ChatGPT"）。 */
  displayName: string;
  /** 用量页（「Manage usage」）。 */
  usageUrl: string | null;
  activeClientId: string | null;
  accounts: AuthAccount[];
  login: AuthLoginState | null;
  /** 自动发现的模型；没拉过 / 用不了是空的。老 Dock 没有这个字段。 */
  models?: AuthModel[];
  /** 上面那份列表的拉取时间（unix 秒）。 */
  modelsFetchedAt?: number | null;
}

export interface AuthStatus {
  providers: AuthProviderStatus[];
  /** 登出时远端撤销没确认。 */
  warning?: string;
}

export const authStatus = () => dock.call<AuthStatus>("auth/status");

/** 发起订阅通道登录。返回里带**授权页地址**——客户端自己开（Dock 那头在后台等）。
 *  `account`：不给 = 活动账号；"new" = 再加一个账号；clientId = 那个账号重新登录。
 *  `scope: "identity"` = 只要身份，不要套餐额度。结果经 `auth/changed` 推送 + `auth/status`。 */
export const authLogin = (provider = "chatgpt", opts: { scope?: "identity"; account?: string } = {}) =>
  dock.call<{ pending: boolean; url?: string | null; provider: string }>("auth/login", {
    provider,
    ...opts,
  });

/** 中断正在跑的登录。 */
export const authCancel = (provider = "chatgpt") =>
  dock.call<{ cancelled: boolean }>("auth/cancel", { provider });

/** 撤销 refresh token、清本地 token（注册留着，下次登录复用）。 */
export const authLogout = (provider = "chatgpt", clientId?: string) =>
  dock.call<AuthStatus>("auth/logout", { provider, clientId });

/** 重拉活动账号能用的模型并入模型目录。登出了 / 没开套餐额度会报错（那一家的模型随之拿掉）。 */
export const authModels = (provider = "chatgpt") =>
  dock.call<{ provider: string; models: AuthModel[] }>("auth/models", { provider });

/** 订阅通道的账号 / 登录状态变了（任何一处：本页、终端 /auth、别的 Dock 进程的登录）。返回取消函数。 */
export function onAuthChanged(fn: () => void): () => void {
  return dock.onNotification((n) => {
    if (n.method === "auth/changed") fn();
  });
}

export type TestResult = { ok: true; ms: number; backend: string } | { ok: false; error: string; backend: string };

/** 表单（可以没保存）发一次最小请求；只给 id 就测配置里那条。 */
export const modelTest = (model: ModelEntry | null, id: string | null, originalId: string | null) =>
  dock.call<TestResult>("model/test", model ? { model, originalId } : { id });

// ---- 个性化（~/.dock/AGENTS.md 与 ~/.dock/USER.md） ----

export interface ProfileFile {
  path: string;
  text: string;
}

export interface Profile {
  /** 自定义指令：每个工作区都带的规约（~/.dock/AGENTS.md）。 */
  instructions: ProfileFile;
  /** 关于你（~/.dock/USER.md）。 */
  about: ProfileFile;
}

export const profileGet = () => dock.call<Profile>("profile/get");
export const profileSet = (kind: "instructions" | "about", text: string) => dock.call<Profile>("profile/set", { kind, text });

// ---- 供应商（[provider.<id>]） ----

/** 密钥从哪来：secrets.json / 环境变量（盖过 secrets.json）/ 文件里行内写的 / 写了名字但哪都没有 / 不要密钥。 */
export type KeySource = "secret" | "env" | "inline" | "missing" | "none";

export interface ProviderEntry {
  id: string;
  name: string | null;
  /** models.dev 的供应商 id（只给界面用）。 */
  catalog: string | null;
  apiBaseUrl: string | null;
  apiBackends: string[];
  authScheme: string | null;
  /** 密钥名：先找同名环境变量，再找 secrets.json。 */
  envKey: string | null;
  hasApiKey: boolean;
  overrides: Record<string, { apiBaseUrl: string | null; authScheme: string | null; apiModel: string | null }>;
  keySource?: KeySource;
}

export const providerList = () => dock.call<{ providers: ProviderEntry[] }>("provider/list");
/** `apiKey` 存进 secrets.json（名字 = envKey，没写按 id 起）；不带 = 不改密钥。 */
export const providerSave = (provider: ProviderEntry, apiKey: string | null, originalId: string | null) =>
  dock.call<{ providers: ProviderEntry[] }>("provider/save", { provider, apiKey: apiKey || undefined, originalId });
/** 连挂在下面的模型、没别人用的密钥一起删。 */
export const providerDelete = (id: string) => dock.call<{ providers: ProviderEntry[] }>("provider/delete", { id });

export interface UpstreamModel {
  id: string;
  name: string | null;
  contextWindow: number | null;
}

export type UpstreamResult = { ok: true; models: UpstreamModel[] } | { ok: false; error: string };

/** 上游 `GET /models`：配置里的供应商（id）或还没存的表单（provider + apiKey）。 */
export const providerModels = (target: { id: string } | { provider: ProviderEntry; apiKey: string | null }) =>
  dock.call<UpstreamResult>(
    "provider/models",
    "id" in target ? target : { provider: target.provider, apiKey: target.apiKey || undefined },
  );

/** 经 Dock 拉 models.dev 的供应商目录（精简过）。 */
export const providerCatalogFetch = () => dock.call<{ catalog: unknown; source: string }>("provider/catalog");

// ---- MCP ----

export interface McpEntry {
  name: string;
  transport: "stdio" | "http";
  command: string;
  args: string[];
  env: Record<string, string>;
  url: string;
  bearerTokenEnvVar: string | null;
  headers: Record<string, string>;
  oauthClientId: string | null;
  oauthScopes: string[];
  startupTimeoutSec: number | null;
  enabled: boolean;
}

export const EMPTY_MCP: McpEntry = {
  name: "",
  transport: "stdio",
  command: "",
  args: [],
  env: {},
  url: "",
  bearerTokenEnvVar: null,
  headers: {},
  oauthClientId: null,
  oauthScopes: [],
  startupTimeoutSec: null,
  enabled: true,
};

type McpListing = { servers: McpServerInfo[]; summary?: string; error?: string };

export const mcpGet = async (name: string) => (await dock.call<{ server: McpEntry }>("mcp/get", { name })).server;
export const mcpSave = (server: McpEntry, originalName: string | null) =>
  dock.call<McpListing>("mcp/save", { server, originalName });
export const mcpDelete = (name: string) => dock.call<McpListing>("mcp/delete", { name });
export const mcpEnable = (name: string, enabled: boolean) => dock.call<McpListing>("mcp/enable", { name, enabled });
export const mcpToolEnable = (server: string, tool: string, enabled: boolean) =>
  dock.call<McpListing>("mcp/tool/enable", { server, tool, enabled });
export const mcpLogin = (name: string) => dock.call<McpListing>("mcp/login", { name });

// ---- CUA / 浏览器 ----

export interface CuaStatus {
  state: "not_mounted" | "missing" | "needs_permission" | "disabled" | "needs_auth" | "unreachable" | "connected";
  detail: string;
  driverPath: string | null;
  driverEnv: string | null;
  perms: { required: boolean; accessibility?: boolean; screenRecording?: boolean } | null;
  job: { action: "install" | "grant"; lines: string[]; running: boolean; ok: boolean | null; message: string | null } | null;
  installSteps: string[];
}

export const cuaStatus = () => dock.call<CuaStatus>("cua/status");
export const cuaAction = (action: "install" | "grant" | "refresh") => dock.call<CuaStatus>("cua/action", { action });

export interface BrowserStatus {
  state: "not_mounted" | "missing" | "disabled" | "connected" | "unreachable";
  detail: string;
  headed: boolean;
  headedEnv: boolean;
}

export const browserStatus = () => dock.call<BrowserStatus>("browser/status");

// ---- 插件 / 技能 / 密钥 ----

/** 永久插件：磁盘上的一个插件目录（按目录认：不同项目可以有同名插件）。 */
export interface DiskPlugin {
  id: string;
  name: string;
  purpose: string;
  scope: "user" | "project";
  /** 项目级的那个项目目录；用户级是 null。 */
  project: string | null;
  root: string;
  path: string;
  /** plugin.toml 的 enabled。 */
  enabled: boolean;
  /** 注册表里装的就是这一份（项目还没开过会话时是 false）。 */
  loaded: boolean;
  running: boolean;
  /** 同名插件已经从别处装上了。 */
  shadowedBy: string | null;
  error: string | null;
}

/** 动态插件：agent 在会话里 cordis_define 的，只在内存里。 */
export interface SessionPlugin {
  id: string;
  name: string;
  purpose: string;
  factory: string;
  running: boolean;
  error: string | null;
  /** 定义它的会话（关了就是 null）。 */
  threadId: string | null;
  threadTitle: string | null;
  cwd: string | null;
}

export interface PluginListing {
  disk: DiskPlugin[];
  session: SessionPlugin[];
  userRoot: string;
  projects: string[];
}

export const pluginList = () => dock.call<PluginListing>("plugin/list");
export const pluginEnable = (path: string, enabled: boolean) => dock.call<PluginListing>("plugin/enable", { path, enabled });
export const pluginDelete = (path: string) => dock.call<PluginListing>("plugin/delete", { path });
export const pluginPromote = (id: string, scope: "user" | "project") =>
  dock.call<PluginListing>("plugin/promote", { id, scope });
export const pluginDiscard = (id: string) => dock.call<PluginListing>("plugin/discard", { id });

export interface SkillRow {
  name: string;
  description: string;
  scope: string;
  path: string;
  userInvocable: boolean;
  /** 项目层的那个项目目录；内置 / 用户级是 null。 */
  project: string | null;
  /** 项目层：被同一项目里更高层的同名技能遮住。 */
  shadowed: boolean;
  /** 内置 / 用户级：在这些项目里被项目自己的同名技能盖住。 */
  shadowedIn: string[];
}

export const skillList = async () => (await dock.call<{ skills: SkillRow[] }>("skill/list")).skills;

type Secrets = { names: string[]; path: string };
export const secretList = () => dock.call<Secrets>("secret/list");
export const secretSet = (name: string, value: string) => dock.call<Secrets>("secret/set", { name, value });
export const secretDelete = (name: string) => dock.call<Secrets>("secret/delete", { name });

// ---- 配对与设备 ----

export interface PairingListing {
  accepting: boolean;
  listen: string;
  remote: boolean;
  pending: { id: string; application: string; origin: string; createdAt: number; expiresAt: number }[];
  bindings: { application: string; origin: string; boundAt: number }[];
}

export const pairingList = () => dock.call<PairingListing>("pairing/list");
export const pairingResolve = (id: string, approve: boolean) =>
  dock.call<PairingListing>("pairing/resolve", { id, approve });
export const pairingRevoke = (origin: string) => dock.call<PairingListing>("pairing/revoke", { origin });
export const pairingAccept = (accepting: boolean) => dock.call<PairingListing>("pairing/accept", { accepting });

export interface DeviceRow {
  id: string;
  name: string;
  createdAt: number;
  lastUsedAt: number | null;
}

export const deviceList = async () => (await dock.call<{ devices: DeviceRow[] }>("device/list")).devices;
export const deviceAdd = (name: string) =>
  dock.call<{ devices: DeviceRow[]; device: { id: string; name: string }; token: string }>("device/add", { name });
export const deviceRevoke = async (id: string) =>
  (await dock.call<{ devices: DeviceRow[] }>("device/revoke", { id })).devices;

/** 这些环境变量在 Dock 进程里设了没有（只回有没有）。 */
export const configEnv = (names: string[]) => dock.call<Record<string, boolean>>("config/env", { names });
