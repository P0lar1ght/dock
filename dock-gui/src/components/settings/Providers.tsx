// 模型 › 供应商：按供应商分组，一家一个密钥，模型逐个开关（参考用户给的截图）。
// 供应商写 [provider.<id>]，密钥进 secrets.json（config 只记名字）；开一个模型 = 写一条
// [model."<供应商>/<模型>"] 挂到供应商上，能力和单价抄 models.dev 目录。

import { useEffect, useMemo, useRef, useState } from "react";
import { Check, ChevronDown, ChevronRight, Eye, EyeOff, LoaderCircle, Pencil, Plus, RefreshCw, Search, Trash2, X } from "lucide-react";
import { formatTokens } from "../chrome";
import type { ModelInfo } from "../../dock/store";
import {
  errorText,
  modelDefault,
  modelDelete,
  modelListing,
  modelSave,
  modelTest,
  providerDelete,
  providerList,
  providerModels,
  providerSave,
  type ModelListing,
  type ProviderEntry,
  type TestResult,
  type UpstreamModel,
} from "../../dock/settingsApi";
import {
  KEY_NAME,
  connectionFor,
  keyNameFor,
  modelEntryFor,
  modelIdFor,
  providerIdFor,
  refreshCatalog,
  supportedProviders,
  usableModels,
  useCatalog,
  type Catalog,
  type CatalogModel,
  type CatalogProvider,
} from "../../dock/providerCatalog";
import {
  ErrorLine,
  Loading,
  Modal,
  Pill,
  Segmented,
  TextInput,
  Toggle,
  autosave,
  host,
  primaryButton,
  secondaryButton,
  smallButton,
  useLoad,
} from "./ui";

const CUSTOM = "__custom__";
const iconButton =
  "motion-colors flex size-7 items-center justify-center rounded-md text-ink-4 hover:bg-hover hover:text-ink-2 disabled:opacity-40";

export function ProvidersTab({
  listing,
  onListing,
  onEditModel,
}: {
  listing: ModelListing | null;
  onListing: (l: ModelListing) => void;
  /** 打开完整的模型编辑页（高级选项）。 */
  onEditModel: (id: string) => void;
}) {
  const providers = useLoad(providerList);
  const catalogState = useCatalog();
  const catalog = catalogState?.catalog ?? null;
  const [dialog, setDialog] = useState<{ edit: ProviderEntry | null } | null>(null);
  const [expanded, setExpanded] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const [refreshError, setRefreshError] = useState<string | null>(null);

  const list = providers.data?.providers ?? [];
  const models = listing?.models ?? [];
  const defaultId = listing?.configDefault ?? listing?.default ?? "";

  const refresh = async () => {
    setRefreshing(true);
    setRefreshError(null);
    try {
      await refreshCatalog();
    } catch (e) {
      setRefreshError(errorText(e));
    }
    setRefreshing(false);
  };

  if (providers.error) return <ErrorLine>读不到供应商：{providers.error}（Dock 版本太旧？）</ErrorLine>;
  if (providers.loading && !providers.data) return <Loading />;

  return (
    <div className="flex flex-col gap-3">
      <div className="flex items-center justify-between gap-3">
        <span className="min-w-0 truncate text-[11.5px] text-ink-4">
          {refreshError ? (
            <span className="text-[#c0392b]">更新失败：{refreshError}</span>
          ) : catalogState?.fetchedAt ? (
            `供应商目录：models.dev，${new Date(catalogState.fetchedAt).toLocaleDateString()} 更新`
          ) : (
            "供应商目录：models.dev（随应用打包的快照）"
          )}
        </span>
        <div className="flex shrink-0 items-center gap-2">
          <button type="button" disabled={refreshing} onClick={() => void refresh()} className={smallButton} title="经 Dock 从 models.dev 拉最新的供应商和模型">
            <RefreshCw size={11} className={refreshing ? "animate-spin" : ""} />
            更新目录
          </button>
          <button type="button" onClick={() => setDialog({ edit: null })} className={smallButton}>
            <Plus size={11} />
            添加供应商
          </button>
        </div>
      </div>

      {list.length === 0 && (
        <div className="enter-fade flex flex-col items-center gap-3 py-16 text-center">
          <span className="flex size-10 items-center justify-center rounded-full bg-fill text-ink-4">
            <Plus size={18} />
          </span>
          <span className="text-[14px] font-semibold text-ink">还没有供应商</span>
          <span className="text-[12px] text-ink-4">选一家、填上 API Key，就能打开它的模型</span>
          <button type="button" onClick={() => setDialog({ edit: null })} className={primaryButton}>
            <Plus size={13} />
            添加供应商
          </button>
        </div>
      )}

      {list.map((p) => (
        <ProviderCard
          key={p.id}
          provider={p}
          catalog={p.catalog && catalog ? (catalog[p.catalog] ?? null) : null}
          models={models.filter((m) => m.provider === p.id)}
          defaultId={defaultId}
          hasDefault={!!listing?.configDefault}
          expanded={expanded === p.id}
          onExpand={(on) => setExpanded(on ? p.id : null)}
          onEdit={() => setDialog({ edit: p })}
          onListing={onListing}
          onEditModel={onEditModel}
          onDeleted={(next) => {
            providers.set(next);
            // 删掉的模型里可能有默认模型：整份重读。
            modelListing().then(onListing, () => {});
          }}
        />
      ))}

      {dialog && (
        <ProviderDialog
          edit={dialog.edit}
          catalog={catalog}
          taken={list.map((p) => p.id)}
          onClose={() => setDialog(null)}
          onSaved={(next, id) => {
            providers.set(next);
            setDialog(null);
            // 新加的直接展开模型列表：下一步就是挑模型。
            if (!dialog.edit) setExpanded(id);
          }}
        />
      )}
    </div>
  );
}

// ---- 供应商卡 ----

const KEY_PILL: Record<string, { kind: "gray" | "green" | "orange"; text: (name: string) => string }> = {
  secret: { kind: "green", text: () => "已保存密钥" },
  env: { kind: "gray", text: (n) => `环境变量 ${n}` },
  inline: { kind: "gray", text: () => "config 里的密钥" },
  missing: { kind: "orange", text: () => "缺密钥" },
  none: { kind: "gray", text: () => "无需密钥" },
};

function ProviderCard({
  provider: p,
  catalog,
  models,
  defaultId,
  hasDefault,
  expanded,
  onExpand,
  onEdit,
  onListing,
  onEditModel,
  onDeleted,
}: {
  provider: ProviderEntry;
  catalog: CatalogProvider | null;
  models: ModelInfo[];
  defaultId: string;
  hasDefault: boolean;
  expanded: boolean;
  onExpand: (on: boolean) => void;
  onEdit: () => void;
  onListing: (l: ModelListing) => void;
  onEditModel: (id: string) => void;
  onDeleted: (next: { providers: ProviderEntry[] }) => void;
}) {
  const [confirm, setConfirm] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const pill = KEY_PILL[p.keySource ?? "none"];
  const enabled = new Set(models.map((m) => m.id));

  const enable = async (upstream: string, info: { name?: string | null; catalog?: CatalogModel; contextWindow?: number | null }) => {
    const id = modelIdFor(p.id, upstream);
    setBusy(id);
    setError(null);
    const entry = modelEntryFor(p.id, upstream, info, p.apiBackends);
    // 还没有默认模型：第一个开的就当默认，不然新对话没模型可用。
    const next = await autosave(() => modelSave(entry, null, !hasDefault), setError);
    if (next) onListing(next);
    setBusy(null);
  };
  const disable = async (id: string) => {
    setBusy(id);
    setError(null);
    const next = await autosave(() => modelDelete(id), setError);
    if (next) onListing(next);
    setBusy(null);
  };
  const remove = async () => {
    const next = await autosave(() => providerDelete(p.id), setError);
    if (next) onDeleted(next);
    else setConfirm(false);
  };

  return (
    <div className="md-surface flex flex-col overflow-hidden rounded-xl border border-line-soft bg-white shadow-sm">
      <div className="group flex items-center gap-2 px-4 pb-2 pt-3.5">
        <div className="flex min-w-0 flex-1 items-center gap-2">
          <span className="truncate text-[14px] font-semibold text-ink">{p.name ?? catalog?.name ?? p.id}</span>
          <span className="shrink-0 text-[12px] text-ink-4">{host(p.apiBaseUrl)}</span>
          {pill && (
            <Pill kind={pill.kind} title={p.envKey ? `密钥名 ${p.envKey}：先找同名环境变量，再找 secrets.json` : undefined}>
              {pill.text(p.envKey ?? "")}
            </Pill>
          )}
        </div>
        <button type="button" title="编辑供应商" onClick={onEdit} className={iconButton}>
          <Pencil size={14} />
        </button>
        <button type="button" title="删除供应商" onClick={() => setConfirm(true)} className={iconButton}>
          <Trash2 size={14} />
        </button>
      </div>
      {confirm && (
        <div className="enter-fade mx-4 mb-2 flex items-center justify-between gap-2 rounded-lg border border-[#fecaca] bg-[#fef2f2] px-3 py-2">
          <span className="text-[12px] text-[#c0392b]">
            删除这个供应商？它下面的 {models.length} 个模型和保存的密钥一起删掉。
          </span>
          <div className="flex shrink-0 gap-1.5">
            <button type="button" onClick={() => setConfirm(false)} className={smallButton}>
              取消
            </button>
            <button
              type="button"
              onClick={() => void remove()}
              className="motion-colors flex h-7 items-center rounded-md bg-[#ff3b30] px-2.5 text-[11.5px] font-medium text-white hover:bg-[#e0352b]"
            >
              删除
            </button>
          </div>
        </div>
      )}
      {error && <div className="px-4 pb-2"><ErrorLine>{error}</ErrorLine></div>}

      {models.length > 0 && (
        <div className="mx-3 mb-2 flex flex-col rounded-[10px] bg-[#f5f5f7]">
          {models.map((m, i) => (
            <EnabledRow
              key={m.id}
              model={m}
              first={i === 0}
              isDefault={m.id === defaultId}
              busy={busy === m.id}
              onDisable={() => void disable(m.id)}
              onDefault={async () => {
                const next = await autosave(() => modelDefault(m.id), setError);
                if (next) onListing(next);
              }}
              onEdit={() => onEditModel(m.id)}
            />
          ))}
        </div>
      )}

      <button
        type="button"
        onClick={() => onExpand(!expanded)}
        className="motion-colors flex h-9 items-center gap-1 border-t border-line-soft px-4 text-left text-[12.5px] text-ink-3 hover:bg-hover"
      >
        <ChevronRight size={13} className="rotate-open" data-open={expanded} />
        {models.length === 0 ? "选择要用的模型" : "管理模型"}
      </button>
      <div className="fold" data-open={expanded}>
        <div>
          {expanded && (
            <ModelPicker provider={p} catalog={catalog} enabled={enabled} busy={busy} onEnable={enable} onDisable={(id) => void disable(id)} />
          )}
        </div>
      </div>
    </div>
  );
}

function EnabledRow({
  model: m,
  first,
  isDefault,
  busy,
  onDisable,
  onDefault,
  onEdit,
}: {
  model: ModelInfo;
  first: boolean;
  isDefault: boolean;
  busy: boolean;
  onDisable: () => void;
  onDefault: () => void;
  onEdit: () => void;
}) {
  const [test, setTest] = useState<TestResult | "running" | null>(null);
  const runTest = async () => {
    setTest("running");
    try {
      setTest(await modelTest(null, m.id, null));
    } catch (e) {
      setTest({ ok: false, error: errorText(e), backend: "" });
    }
  };
  return (
    <div className={`group flex min-h-[48px] items-center gap-2 px-3 py-2 ${first ? "" : "border-t border-line-soft"}`}>
      <div className="flex min-w-0 flex-1 flex-col gap-0.5">
        <div className="flex min-w-0 items-center gap-2">
          <span className="truncate text-[13.5px] text-ink">{m.label}</span>
          {isDefault && <Pill kind="blue">默认</Pill>}
          {m.contextWindow ? <span className="shrink-0 text-[11px] text-ink-4">{formatTokens(m.contextWindow)}</span> : null}
        </div>
        {test && test !== "running" && (
          <span className={`enter-fade truncate text-[11.5px] ${test.ok ? "text-[#248a3d]" : "text-[#d70015]"}`} title={test.ok ? undefined : test.error}>
            {test.ok ? `连接成功 · ${test.ms}ms` : test.error}
          </span>
        )}
      </div>
      <div className="flex items-center gap-1 opacity-0 transition-opacity duration-[var(--dur-fast)] group-focus-within:opacity-100 group-hover:opacity-100">
        {!isDefault && (
          <button type="button" onClick={onDefault} className="motion-colors rounded-md px-1.5 py-1 text-[11.5px] text-ink-3 hover:bg-hover hover:text-ink">
            设为默认
          </button>
        )}
        <button type="button" title="编辑（上下文、推理、单价…）" onClick={onEdit} className={iconButton}>
          <Pencil size={13} />
        </button>
      </div>
      <button type="button" title="测试连接" disabled={test === "running"} onClick={() => void runTest()} className={iconButton}>
        {test === "running" ? (
          <LoaderCircle size={14} className="animate-spin" />
        ) : test && test.ok ? (
          <Check size={14} className="pop-in text-[#34c759]" />
        ) : (
          <RefreshCw size={14} />
        )}
      </button>
      {busy ? <LoaderCircle size={14} className="mx-2.5 animate-spin text-ink-4" /> : <Toggle on label={`停用 ${m.label}`} onChange={onDisable} />}
    </div>
  );
}

// ---- 挑模型 ----

interface PickRow {
  upstream: string;
  name: string;
  catalog?: CatalogModel;
  contextWindow: number | null;
  /** 只在上游列表里、目录里没有。 */
  upstreamOnly: boolean;
}

function ModelPicker({
  provider: p,
  catalog,
  enabled,
  busy,
  onEnable,
  onDisable,
}: {
  provider: ProviderEntry;
  catalog: CatalogProvider | null;
  enabled: Set<string>;
  busy: string | null;
  onEnable: (upstream: string, info: { name?: string | null; catalog?: CatalogModel; contextWindow?: number | null }) => void;
  onDisable: (id: string) => void;
}) {
  const [query, setQuery] = useState("");
  const [manual, setManual] = useState("");
  const [upstream, setUpstream] = useState<UpstreamModel[] | null>(null);
  const [upstreamError, setUpstreamError] = useState<string | null>(null);

  // 用这把密钥问上游有哪些模型：目录里没有的新模型、或者这把密钥能用的子集。
  useEffect(() => {
    let alive = true;
    providerModels({ id: p.id }).then(
      (r) => {
        if (!alive) return;
        if (r.ok) setUpstream(r.models);
        else setUpstreamError(r.error);
      },
      (e) => alive && setUpstreamError(errorText(e)),
    );
    return () => {
      alive = false;
    };
  }, [p.id, p.apiBaseUrl, p.keySource]);

  const rows = useMemo<PickRow[]>(() => {
    const out: PickRow[] = (catalog ? usableModels(catalog) : []).map((m) => ({
      upstream: m.id,
      name: m.name ?? m.id,
      catalog: m,
      contextWindow: m.limit?.context ?? null,
      upstreamOnly: false,
    }));
    const known = new Set(out.map((r) => r.upstream));
    for (const u of upstream ?? []) {
      if (known.has(u.id)) continue;
      out.push({ upstream: u.id, name: u.name ?? u.id, contextWindow: u.contextWindow, upstreamOnly: true });
    }
    return out;
  }, [catalog, upstream]);

  const q = query.trim().toLowerCase();
  const shown = q ? rows.filter((r) => r.upstream.toLowerCase().includes(q) || r.name.toLowerCase().includes(q)) : rows;
  const upstreamIds = upstream ? new Set(upstream.map((u) => u.id)) : null;

  return (
    <div className="flex flex-col gap-2 px-4 pb-3.5 pt-1">
      <div className="flex items-center gap-2">
        <TextInput value={query} onChange={setQuery} placeholder={`搜索 ${rows.length} 个模型`} width="100%" suffix={<Search size={12} />} />
      </div>
      <div className="text-[11px] text-ink-4">
        {upstream === null && !upstreamError && "正在向供应商查询可用模型…"}
        {upstream &&
          (catalog
            ? `这把密钥能用 ${upstream.length} 个模型；目录里有、但上游没列出的标「未列出」`
            : `这把密钥能用 ${upstream.length} 个模型`)}
        {upstreamError && <span title={upstreamError}>没查到上游模型列表，只按目录显示（{upstreamError.slice(0, 60)}）</span>}
      </div>
      <div className="thin-scroll flex max-h-[320px] flex-col overflow-y-auto rounded-[10px] border border-line-soft">
        {shown.length === 0 && <div className="px-3 py-4 text-center text-[12px] text-ink-4">没有匹配的模型</div>}
        {shown.map((r, i) => {
          const id = modelIdFor(p.id, r.upstream);
          const on = enabled.has(id);
          return (
            <div key={r.upstream} className={`flex min-h-[40px] items-center gap-2 px-3 py-1.5 ${i ? "border-t border-line-soft" : ""}`}>
              <div className="flex min-w-0 flex-1 flex-col">
                <span className="truncate text-[13px] text-ink">{r.name}</span>
                <span className="flex min-w-0 items-center gap-1.5 text-[11px] text-ink-4">
                  <span className="truncate font-mono">{r.upstream}</span>
                  {r.contextWindow ? <span className="shrink-0">· {formatTokens(r.contextWindow)}</span> : null}
                  {r.catalog?.reasoning && <span className="shrink-0">· 推理</span>}
                  {r.catalog?.modalities?.input?.includes("image") && <span className="shrink-0">· 图片</span>}
                  {r.upstreamOnly && catalog && <span className="shrink-0 text-accent">· 目录外</span>}
                  {upstreamIds && !r.upstreamOnly && !upstreamIds.has(r.upstream) && <span className="shrink-0 text-[#c93400]">· 未列出</span>}
                </span>
              </div>
              {busy === id ? (
                <LoaderCircle size={14} className="mx-2.5 animate-spin text-ink-4" />
              ) : (
                <Toggle
                  on={on}
                  label={`${on ? "停用" : "启用"} ${r.name}`}
                  onChange={(v) => (v ? onEnable(r.upstream, { name: r.name, catalog: r.catalog, contextWindow: r.contextWindow }) : onDisable(id))}
                />
              )}
            </div>
          );
        })}
      </div>
      <div className="flex items-center gap-2">
        <TextInput value={manual} onChange={setManual} mono placeholder="不在列表里？填上游模型名" width="100%" />
        <button
          type="button"
          disabled={!manual.trim() || enabled.has(modelIdFor(p.id, manual.trim()))}
          onClick={() => {
            const name = manual.trim();
            const known = rows.find((r) => r.upstream === name);
            onEnable(name, { name, catalog: known?.catalog, contextWindow: known?.contextWindow });
            setManual("");
          }}
          className={smallButton}
        >
          <Plus size={11} />
          添加
        </button>
      </div>
    </div>
  );
}

// ---- 添加 / 编辑供应商 ----

const BACKEND_OPTIONS = [
  { value: "chat_completions", label: "Chat" },
  { value: "responses", label: "Responses" },
  { value: "messages", label: "Messages" },
];

function ProviderDialog({
  edit,
  catalog,
  taken,
  onClose,
  onSaved,
}: {
  edit: ProviderEntry | null;
  catalog: Catalog | null;
  taken: string[];
  onClose: () => void;
  onSaved: (next: { providers: ProviderEntry[] }, id: string) => void;
}) {
  const options = useMemo(() => (catalog ? supportedProviders(catalog) : []), [catalog]);
  const [choice, setChoice] = useState<string>(edit ? (edit.catalog ?? CUSTOM) : "");
  const picked = choice && choice !== CUSTOM ? (options.find((o) => o.id === choice) ?? null) : null;
  const [apiKey, setApiKey] = useState("");
  const [showKey, setShowKey] = useState(false);
  const [name, setName] = useState(edit?.name ?? "");
  const [baseUrl, setBaseUrl] = useState(edit?.apiBaseUrl ?? "");
  const [backend, setBackend] = useState(edit?.apiBackends[0] ?? "chat_completions");
  const [advanced, setAdvanced] = useState(choice === CUSTOM);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // 换了供应商：地址、协议跟着目录走（编辑时第一次渲染不覆盖已存的值）。
  const first = useRef(true);
  useEffect(() => {
    if (first.current && edit) {
      first.current = false;
      return;
    }
    first.current = false;
    if (choice === CUSTOM) {
      setAdvanced(true);
      return;
    }
    const conn = picked && connectionFor(picked);
    if (conn) {
      setBaseUrl(conn.apiBaseUrl);
      setBackend(conn.apiBackends[0]);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [choice]);

  const custom = choice === CUSTOM;
  const urlOk = /^https?:\/\/[^\s/?#]+/.test(baseUrl.trim());
  const canSave = !!choice && urlOk && (!custom || !!name.trim()) && !saving;
  const keyless = /^https?:\/\/(127\.0\.0\.1|localhost)[:/]/.test(baseUrl.trim());

  const save = async () => {
    setSaving(true);
    setError(null);
    const conn = picked ? connectionFor(picked) : null;
    // 目录给的协议列表（如 OpenAI 的 responses + chat）保留，只是把选的那条放第一。
    const backends = conn && conn.apiBackends.includes(backend) ? [backend, ...conn.apiBackends.filter((b) => b !== backend)] : [backend];
    const id = edit?.id ?? providerIdFor(picked?.id ?? name, taken);
    const envKey = edit?.envKey ?? (picked ? keyNameFor(picked) : null);
    const entry: ProviderEntry = {
      id,
      name: (custom ? name.trim() : picked?.name) || edit?.name || null,
      catalog: picked?.id ?? null,
      apiBaseUrl: baseUrl.trim(),
      apiBackends: backends,
      authScheme: edit?.authScheme ?? null,
      envKey: envKey && KEY_NAME.test(envKey) ? envKey : null,
      hasApiKey: false,
      overrides: edit?.overrides ?? {},
    };
    try {
      const next = await providerSave(entry, apiKey.trim() || null, edit?.id ?? null);
      onSaved(next, id);
    } catch (e) {
      setError(errorText(e));
      setSaving(false);
    }
  };

  return (
    <Modal onClose={onClose} width={640}>
      <div className="flex items-center justify-between">
        <h2 className="text-[18px] font-semibold text-ink">{edit ? `编辑 ${edit.name ?? edit.id}` : "添加供应商"}</h2>
        <button type="button" title="关闭" onClick={onClose} className={iconButton}>
          <X size={16} />
        </button>
      </div>

      {/* 不用 label 包：按钮在 label 里会被它改名（读屏只念「提供商」），点标题也会弹列表。 */}
      <div className="flex flex-col gap-1.5">
        <span className="text-[13px] font-medium text-ink">提供商</span>
        <ProviderCombo value={choice} options={options} loading={!catalog} disabled={!!edit} onChange={setChoice} />
      </div>

      {custom && (
        <label className="flex flex-col gap-1.5">
          <span className="text-[13px] font-medium text-ink">名称</span>
          <TextInput value={name} onChange={setName} placeholder="如：公司网关" width="100%" />
        </label>
      )}

      <label className="flex flex-col gap-1.5">
        <span className="text-[13px] font-medium text-ink">API Key</span>
        <TextInput
          value={apiKey}
          onChange={setApiKey}
          type={showKey ? "text" : "password"}
          mono
          width="100%"
          placeholder={
            edit && edit.keySource && edit.keySource !== "missing" && edit.keySource !== "none"
              ? "已保存（留空 = 不改）"
              : keyless
                ? "本地服务可以不填"
                : "请输入 API Key"
          }
          suffix={
            <button type="button" title={showKey ? "隐藏" : "显示"} onClick={() => setShowKey((v) => !v)} className="flex items-center text-ink-4 hover:text-ink-2">
              {showKey ? <Eye size={14} /> : <EyeOff size={14} />}
            </button>
          }
        />
        {edit?.keySource === "env" && (
          <span className="text-[11.5px] text-[#c93400]">
            环境变量 {edit.envKey} 设着，Dock 会优先用它；这里填的要等去掉那个变量才生效
          </span>
        )}
        <span className="text-[11.5px] text-ink-4">
          存在 Dock 的 secrets.json（只有你能读），config.toml 里只记名字{picked?.doc && (
            <>
              {" · "}
              <a href={picked.doc} target="_blank" rel="noreferrer" className="text-accent hover:underline">
                去哪拿密钥
              </a>
            </>
          )}
        </span>
      </label>

      {choice && (
        <div className="flex flex-col gap-2">
          <button type="button" onClick={() => setAdvanced((v) => !v)} className="flex items-center gap-1 self-start text-[12px] text-ink-4 hover:text-ink-2">
            <ChevronDown size={12} className={`transition-transform duration-[var(--dur-base)] ${advanced ? "" : "-rotate-90"}`} />
            接口地址与协议
          </button>
          <div className="fold" data-open={advanced}>
            <div className="flex flex-col gap-2.5 pb-1">
              <TextInput value={baseUrl} onChange={setBaseUrl} mono width="100%" invalid={!!baseUrl && !urlOk} placeholder="https://api.example.com/v1" />
              <div className="flex items-center justify-between gap-2">
                <span className="text-[12px] text-ink-3">协议</span>
                <Segmented value={backend} options={BACKEND_OPTIONS} onChange={setBackend} />
              </div>
            </div>
          </div>
        </div>
      )}

      {error && <ErrorLine>{error}</ErrorLine>}

      <div className="flex justify-end gap-2 pt-2">
        <button type="button" onClick={onClose} className={secondaryButton}>
          取消
        </button>
        <button type="button" disabled={!canSave} onClick={() => void save()} className={primaryButton}>
          {saving && <LoaderCircle size={13} className="animate-spin" />}
          保存
        </button>
      </div>
    </Modal>
  );
}

/** 可搜索的供应商下拉：目录里两百多家，原生 select 翻不动。 */
function ProviderCombo({
  value,
  options,
  loading,
  disabled,
  onChange,
}: {
  value: string;
  options: CatalogProvider[];
  loading: boolean;
  disabled: boolean;
  onChange: (id: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const listRef = useRef<HTMLDivElement>(null);
  const q = query.trim().toLowerCase();
  const shown = [
    ...options.filter((o) => !q || o.name.toLowerCase().includes(q) || o.id.includes(q)),
    ...(!q || "自定义 openai custom".includes(q) ? [{ id: CUSTOM, name: "自定义（OpenAI / Anthropic 兼容）", models: {} } as CatalogProvider] : []),
  ];
  const label = value === CUSTOM ? "自定义（OpenAI / Anthropic 兼容）" : options.find((o) => o.id === value)?.name;

  useEffect(() => setActive(0), [query]);
  useEffect(() => {
    (listRef.current?.children[active] as HTMLElement | undefined)?.scrollIntoView({ block: "nearest" });
  }, [active]);

  const pick = (id: string) => {
    onChange(id);
    setOpen(false);
    setQuery("");
  };

  return (
    <div className="relative">
      <button
        type="button"
        aria-haspopup="listbox"
        aria-expanded={open}
        disabled={disabled}
        onClick={() => setOpen((o) => !o)}
        className="motion-colors flex h-10 w-full items-center gap-2 rounded-lg border border-line-strong bg-white px-3 text-left shadow-sm hover:border-ink-5 disabled:bg-window disabled:shadow-none"
      >
        <span className={`min-w-0 flex-1 truncate text-[13px] ${label ? "text-ink" : "text-ink-4"}`}>
          {label ?? (loading ? "正在读取供应商目录…" : "请选择提供商")}
        </span>
        <ChevronDown size={14} className="shrink-0 text-ink-4" />
      </button>
      {open && (
        <div className="glass-popover pop-in absolute inset-x-0 top-[calc(100%+6px)] z-10 flex origin-top flex-col overflow-hidden rounded-xl border border-line-strong bg-white shadow-[0px_12px_36px_rgba(0,0,0,0.14)]">
          <div className="flex items-center gap-2 border-b border-line-soft px-3">
            <Search size={13} className="text-ink-4" />
            <input
              autoFocus
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "ArrowDown") (e.preventDefault(), setActive((a) => Math.min(a + 1, shown.length - 1)));
                else if (e.key === "ArrowUp") (e.preventDefault(), setActive((a) => Math.max(a - 1, 0)));
                else if (e.key === "Enter" && shown[active]) (e.preventDefault(), pick(shown[active].id));
              }}
              placeholder={`搜索 ${options.length} 家供应商`}
              className="h-9 min-w-0 flex-1 bg-transparent text-[13px] text-ink outline-none placeholder:text-ink-4"
            />
          </div>
          <div ref={listRef} className="thin-scroll flex max-h-[260px] flex-col overflow-y-auto p-1">
            {shown.map((o, i) => (
              <button
                key={o.id}
                type="button"
                onMouseEnter={() => setActive(i)}
                onClick={() => pick(o.id)}
                className={`flex h-8 shrink-0 items-center gap-2 rounded-md px-2.5 text-left text-[12.5px] ${i === active ? "bg-hover" : ""}`}
              >
                <span className="min-w-0 flex-1 truncate text-ink">{o.name}</span>
                {o.id !== CUSTOM && (
                  <span className="shrink-0 text-[11px] text-ink-4">{Object.keys(o.models ?? {}).length} 个模型</span>
                )}
                {o.id === value && <Check size={13} className="shrink-0 text-accent" />}
              </button>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}
