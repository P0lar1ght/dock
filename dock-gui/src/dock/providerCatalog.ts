// 供应商目录：models.dev（opencode 用的开源供应商 / 模型目录）。
// 打包一份精简快照（scripts/update-provider-catalog.mjs 生成），设置页可经 Dock 拉新的，存在 localStorage。
// 目录只用来「填表」：选了供应商带出地址 / 协议 / 密钥名，开了模型把能力和单价写进 [model.<id>]；
// 之后以 config.toml 为准，Dock 不读这份目录。

import { useEffect, useState } from "react";
import { EMPTY_MODEL, providerCatalogFetch, type ModelEntry } from "./settingsApi";

export interface CatalogModel {
  id: string;
  name?: string;
  reasoning?: boolean;
  tool_call?: boolean;
  attachment?: boolean;
  modalities?: { input?: string[]; output?: string[] };
  limit?: { context?: number; output?: number };
  cost?: { input?: number; output?: number; cache_read?: number; cache_write?: number };
  release_date?: string;
  status?: string;
}

export interface CatalogProvider {
  id: string;
  name: string;
  api?: string;
  npm?: string;
  env?: string[];
  doc?: string;
  models: Record<string, CatalogModel>;
}

export type Catalog = Record<string, CatalogProvider>;

const CACHE_KEY = "dock.providerCatalog";

interface CatalogState {
  catalog: Catalog;
  /** 拉新的时间；null = 用的是打包快照。 */
  fetchedAt: number | null;
}

let state: CatalogState | null = null;
let loading: Promise<CatalogState> | null = null;
const listeners = new Set<() => void>();

function readCache(): CatalogState | null {
  try {
    const raw = localStorage.getItem(CACHE_KEY);
    if (!raw) return null;
    const parsed = JSON.parse(raw) as CatalogState;
    return parsed.catalog && typeof parsed.catalog === "object" ? parsed : null;
  } catch {
    return null;
  }
}

async function load(): Promise<CatalogState> {
  if (state) return state;
  loading ??= (async () => {
    const cached = readCache();
    if (cached) return cached;
    // 快照只在设置页用：按需加载，不进启动包。
    const snapshot = (await import("./providerCatalog.snapshot.json")).default as unknown as Catalog;
    return { catalog: snapshot, fetchedAt: null };
  })();
  state = await loading;
  return state;
}

/** 经 Dock 拉 models.dev，成功就替换并缓存。失败抛出（文案给人看）。 */
export async function refreshCatalog(): Promise<void> {
  const { catalog } = await providerCatalogFetch();
  if (!catalog || typeof catalog !== "object") throw new Error("models.dev 回的不是目录");
  // 只留连得上的供应商和能用的模型：整份三四 MB，localStorage 放不下。
  const slim: Catalog = {};
  for (const p of supportedProviders(catalog as Catalog)) {
    slim[p.id] = { ...p, models: Object.fromEntries(usableModels(p).map((m) => [m.id, m])) };
  }
  state = { catalog: slim, fetchedAt: Date.now() };
  try {
    localStorage.setItem(CACHE_KEY, JSON.stringify(state));
  } catch {
    // 存不下就只在这次打开里用。
  }
  listeners.forEach((l) => l());
}

export function useCatalog(): CatalogState | null {
  const [s, setS] = useState(state);
  useEffect(() => {
    const on = () => setS(state);
    listeners.add(on);
    void load().then(on);
    return () => void listeners.delete(on);
  }, []);
  return s;
}

// ---- 供应商 → Dock 连接 ----

export interface Connection {
  apiBaseUrl: string;
  /** 第一条 = 默认协议。 */
  apiBackends: string[];
}

/** 目录里没写 `api` 的官方 SDK 供应商：按 npm 包认基址。 */
const SDK_BASES: Record<string, Connection> = {
  "@ai-sdk/openai": { apiBaseUrl: "https://api.openai.com/v1", apiBackends: ["responses", "chat_completions"] },
  "@ai-sdk/anthropic": { apiBaseUrl: "https://api.anthropic.com/v1", apiBackends: ["messages"] },
  "@ai-sdk/google": { apiBaseUrl: "https://generativelanguage.googleapis.com/v1beta/openai", apiBackends: ["chat_completions"] },
  "@ai-sdk/xai": { apiBaseUrl: "https://api.x.ai/v1", apiBackends: ["responses", "chat_completions"] },
  "@ai-sdk/mistral": { apiBaseUrl: "https://api.mistral.ai/v1", apiBackends: ["chat_completions"] },
  "@ai-sdk/groq": { apiBaseUrl: "https://api.groq.com/openai/v1", apiBackends: ["chat_completions"] },
  "@ai-sdk/deepseek": { apiBaseUrl: "https://api.deepseek.com", apiBackends: ["chat_completions"] },
  "@ai-sdk/cerebras": { apiBaseUrl: "https://api.cerebras.ai/v1", apiBackends: ["chat_completions"] },
  "@ai-sdk/togetherai": { apiBaseUrl: "https://api.together.xyz/v1", apiBackends: ["chat_completions"] },
  "@ai-sdk/deepinfra": { apiBaseUrl: "https://api.deepinfra.com/v1/openai", apiBackends: ["chat_completions"] },
  "@ai-sdk/perplexity": { apiBaseUrl: "https://api.perplexity.ai", apiBackends: ["chat_completions"] },
  "@ai-sdk/fireworks": { apiBaseUrl: "https://api.fireworks.ai/inference/v1", apiBackends: ["chat_completions"] },
  "@ai-sdk/cohere": { apiBaseUrl: "https://api.cohere.ai/compatibility/v1", apiBackends: ["chat_completions"] },
};

/** 走 OpenAI SDK 的端点也开着 /responses（OpenAI 自己、xAI）；别家只认 chat。 */
const RESPONSES_HOSTS = ["api.openai.com", "api.x.ai"];

/**
 * 这个供应商在 Dock 里怎么连。Dock 只说三条 wire（responses / chat_completions / messages）；
 * 要签名、要部署名、地址带模板变量的（Bedrock、Azure、Vertex…）返回 null，界面不列。
 */
export function connectionFor(p: CatalogProvider): Connection | null {
  const api = p.api?.trim().replace(/\/+$/, "");
  if (api) {
    if (api.includes("${") || !/^https?:\/\//.test(api)) return null;
    if (p.npm === "@ai-sdk/anthropic") return { apiBaseUrl: api, apiBackends: ["messages"] };
    const host = safeHost(api);
    return {
      apiBaseUrl: api,
      apiBackends: RESPONSES_HOSTS.includes(host) ? ["responses", "chat_completions"] : ["chat_completions"],
    };
  }
  return (p.npm && SDK_BASES[p.npm]) || null;
}

function safeHost(url: string): string {
  try {
    return new URL(url).host;
  } catch {
    return "";
  }
}

/** 先列这些（常用、国内能直连的在前），其余按名字排。 */
const PINNED = [
  "deepseek",
  "moonshotai-cn",
  "zhipuai",
  "alibaba-cn",
  "minimax-cn",
  "siliconflow-cn",
  "openrouter",
  "openai",
  "anthropic",
  "google",
  "xai",
];

/** Dock 连得上的供应商，常用的在前。 */
export function supportedProviders(catalog: Catalog): CatalogProvider[] {
  const list = Object.values(catalog).filter((p) => p && p.name && connectionFor(p));
  const rank = (p: CatalogProvider) => {
    const i = PINNED.indexOf(p.id);
    return i === -1 ? PINNED.length : i;
  };
  return list.sort((a, b) => rank(a) - rank(b) || a.name.localeCompare(b.name));
}

/** 密钥名的形状（同 secrets.json 的名字规则）。 */
export const KEY_NAME = /^[A-Za-z][A-Za-z0-9_-]{0,63}$/;

/** 目录里的环境变量名（如 DEEPSEEK_API_KEY），不合规则就让 Dock 按 id 起。 */
export function keyNameFor(p: CatalogProvider): string | null {
  const name = p.env?.find((e) => KEY_NAME.test(e));
  return name ?? null;
}

/** 供应商在 config 里的 id：目录 id 能用就用，重名加 -2、-3。 */
export function providerIdFor(base: string, taken: string[]): string {
  let slug = base
    .toLowerCase()
    .replace(/[^a-z0-9_-]+/g, "-")
    .replace(/^[^a-z]+/, "")
    .replace(/-+$/, "");
  if (!slug) slug = "custom";
  slug = slug.slice(0, 56);
  let id = slug;
  for (let n = 2; taken.includes(id); n++) id = `${slug}-${n}`;
  return id;
}

// ---- 模型 ----

/** 能当 Agent 用的：会调工具、出文字、没下架。 */
export function usableModels(p: CatalogProvider): CatalogModel[] {
  return Object.values(p.models ?? {})
    .filter((m) => m && m.id && m.tool_call !== false && m.status !== "deprecated")
    .filter((m) => !m.modalities?.output || m.modalities.output.includes("text"))
    .sort((a, b) => (b.release_date ?? "").localeCompare(a.release_date ?? "") || a.id.localeCompare(b.id));
}

/** config 里挂在供应商下的模型 id：`<供应商>/<上游模型名>`。 */
export const modelIdFor = (providerId: string, upstream: string) => `${providerId}/${upstream}`;

/**
 * 开一个模型时写进 config 的那条。连接全继承供应商；能力和单价抄目录（没有目录信息就都不写，
 * 交给上游默认）。输出上限只在 messages 协议下写：那条协议必填，别的协议不写 = 不发。
 */
export function modelEntryFor(
  providerId: string,
  upstream: string,
  info: { name?: string | null; catalog?: CatalogModel; contextWindow?: number | null },
  backends: string[],
): ModelEntry {
  const m = info.catalog;
  const cost = m?.cost;
  const price = (v: number | undefined) => (typeof v === "number" && Number.isFinite(v) && v >= 0 ? v : null);
  const pricing = cost
    ? { input: price(cost.input), cacheRead: price(cost.cache_read), cacheWrite: price(cost.cache_write), output: price(cost.output) }
    : null;
  return {
    ...EMPTY_MODEL,
    id: modelIdFor(providerId, upstream),
    name: m?.name ?? info.name ?? upstream,
    apiModel: upstream,
    apiBackends: [],
    provider: providerId,
    contextWindow: m?.limit?.context || info.contextWindow || null,
    maxOutputTokens: backends[0] === "messages" ? m?.limit?.output || null : null,
    reasoning: m?.reasoning === false ? false : null,
    supportsImages: m?.modalities?.input ? (m.modalities.input.includes("image") ? null : false) : null,
    pricing: pricing && Object.values(pricing).some((v) => v !== null) ? pricing : null,
  };
}
