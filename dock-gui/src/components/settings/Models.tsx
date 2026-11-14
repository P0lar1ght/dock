// 模型与密钥：列表（uta）/ 空（nab）/ 删除确认（tom）/ 配置解析失败（aes）/ 编辑（log、seo）。
// 写 ~/.dock/config.toml 的 [model."<id>"]；密钥只进不出（model/get 只说有没有）。

import { useEffect, useMemo, useRef, useState } from "react";
import { ChevronDown, FolderOpen, GripVertical, LoaderCircle, MoreHorizontal, Pencil, Plus } from "lucide-react";
import { openPath, openUrl, revealItemInDir } from "@tauri-apps/plugin-opener";
import { formatTokens } from "../chrome";
import { Popover } from "../Popover";
import { useApp, type ModelInfo } from "../../dock/store";
import {
  EMPTY_MODEL,
  authCancel,
  authLogin,
  authLogout,
  authModels,
  authStatus,
  onAuthChanged,
  configEnv,
  configGet,
  configStatus,
  errorText,
  modelDefault,
  modelDelete,
  modelGet,
  modelListing,
  modelSave,
  modelTest,
  type AuthModel,
  type AuthProviderStatus,
  type ConfigStatus,
  type ModelEntry,
  type ModelListing,
  type TestResult,
} from "../../dock/settingsApi";
import { ProvidersTab } from "./Providers";
import {
  Banner,
  Card,
  Checkbox,
  EnvPill,
  ErrorLine,
  Loading,
  Page,
  Pill,
  Row,
  Section,
  Segmented,
  Select,
  Tabs,
  TextInput,
  Toggle,
  autosave,
  primaryButton,
  secondaryButton,
  smallButton,
  host,
  useLoad,
} from "./ui";

const BACKENDS = ["responses", "chat_completions", "messages"] as const;
const SHORT: Record<string, string> = { responses: "responses", chat_completions: "chat", messages: "messages" };
const EFFORTS = ["low", "medium", "high", "xhigh"];

const TEMPLATES: { label: string; entry: Partial<ModelEntry> }[] = [
  {
    label: "OpenAI 兼容",
    entry: {
      apiBaseUrl: "https://api.openai.com/v1",
      apiBackends: ["responses", "chat_completions"],
      envKey: "OPENAI_API_KEY",
    },
  },
  {
    label: "Anthropic 兼容",
    entry: {
      apiBaseUrl: "https://api.anthropic.com/v1",
      apiBackends: ["messages"],
      envKey: "ANTHROPIC_API_KEY",
      maxOutputTokens: 16384,
    },
  },
  {
    label: "本地 Ollama",
    entry: { apiBaseUrl: "http://127.0.0.1:11434/v1", apiBackends: ["chat_completions"], envKey: null },
  },
];

type Editing = { entry: ModelEntry; originalId: string | null };

/** `editId`：进来直接打开这条的编辑页（预览截图用；"" = 添加）。 */
export function ModelsPane({ editId }: { editId?: string } = {}) {
  const [editing, setEditing] = useState<Editing | null>(null);
  const status = useLoad(configStatus);
  const listing = useLoad(modelListing);
  useEffect(() => {
    if (editId === undefined) return;
    if (editId === "") setEditing({ entry: { ...EMPTY_MODEL }, originalId: null });
    else modelGet(editId).then((entry) => setEditing({ entry, originalId: editId }), () => {});
  }, [editId]);
  const reloadAll = () => {
    void status.reload();
    void listing.reload();
  };

  if (editing) {
    return (
      <ModelEditor
        key={editing.originalId ?? "new"}
        initial={editing.entry}
        originalId={editing.originalId}
        takenIds={(listing.data?.models ?? []).map((m) => m.id).filter((id) => id !== editing.originalId)}
        onDone={(next) => {
          setEditing(null);
          if (next) listing.set(next);
          else reloadAll();
        }}
      />
    );
  }
  return (
    <ModelList
      status={status.data ?? null}
      listing={listing.data ?? null}
      error={listing.error ?? status.error ?? null}
      loading={listing.loading && !listing.data}
      onListing={listing.set}
      onReload={reloadAll}
      onEdit={setEditing}
    />
  );
}

// ---- 列表 ----

function ModelList({
  status,
  listing,
  error,
  loading,
  onListing,
  onReload,
  onEdit,
}: {
  status: ConfigStatus | null;
  listing: ModelListing | null;
  error: string | null;
  loading: boolean;
  onListing: (l: ModelListing) => void;
  onReload: () => void;
  onEdit: (e: Editing) => void;
}) {
  // 连远程 Dock 时 config.toml 在远端：不给本机编辑器 / 访达打开。
  const remote = useApp().remote;
  const [dockModelEnv, setDockModelEnv] = useState<string | null>(null);
  useEffect(() => {
    configGet().then((c) => setDockModelEnv(c.env.DOCK_MODEL), () => {});
  }, []);

  const broken = status?.error ?? null;
  const all = listing?.models ?? [];
  // 挂在供应商下的模型在「供应商」页签里管；订阅通道自动发现的在账号卡片里列；
  // 这里只列自己写全连接的。
  const models = all.filter((m) => !m.provider && !m.discovered);
  const [tab, setTab] = useState<"providers" | "custom">("providers");
  const startNew = (seed: Partial<ModelEntry> = {}) => onEdit({ entry: { ...EMPTY_MODEL, ...seed }, originalId: null });
  const edit = async (id: string) => {
    try {
      onEdit({ entry: await modelGet(id), originalId: id });
    } catch (e) {
      alert(errorText(e));
    }
  };
  const duplicate = async (id: string) => {
    try {
      const src = await modelGet(id);
      // 复制不带行内密钥（不回给界面）：换成同名环境变量或让人重填。
      startNew({ ...src, id: `${src.id}-copy`, keyMode: src.keyMode === "inline" ? "env" : src.keyMode, hasApiKey: false });
    } catch (e) {
      alert(errorText(e));
    }
  };

  const defaultId = listing?.configDefault ?? listing?.default ?? "";
  const templateMenu = (
    <Popover
      title="从模板开始"
      side="bottom"
      align="left"
      className={secondaryButton}
      panelClassName="min-w-[180px] px-1"
      trigger={
        <>
          从模板开始
          <ChevronDown size={13} />
        </>
      }
    >
      {(close) =>
        TEMPLATES.map((t) => (
          <button
            key={t.label}
            type="button"
            onClick={() => {
              close();
              startNew(t.entry);
            }}
            className="motion-colors flex h-8 w-full items-center rounded-md px-2.5 text-left text-[12.5px] text-ink hover:bg-hover"
          >
            {t.label}
          </button>
        ))
      }
    </Popover>
  );

  return (
    <Page
      title="模型与密钥"
      subtitle={
        remote ? `保存在远程 Dock「${remote.name}」的 config.toml，改完新对话立即生效` : "保存在 ~/.dock/config.toml，改完新对话立即生效"
      }
      banner={
        broken && (
          <Banner
            kind="error"
            action={
              status &&
              !remote && (
                <button type="button" onClick={() => openPath(status.path).catch(() => {})} className={smallButton}>
                  在编辑器中打开
                </button>
              )
            }
          >
            config.toml {broken.line ? `第 ${broken.line} 行` : ""}解析失败，Dock 读不到任何模型：{broken.message}
          </Banner>
        )
      }
      actions={
        <>
          {!remote && (
            <button
              type="button"
              disabled={!status}
              onClick={() => status && revealItemInDir(status.exists ? status.path : status.dockHome).catch(() => {})}
              className={secondaryButton}
            >
              <FolderOpen size={13} />
              在访达中显示
            </button>
          )}
          {tab === "custom" && (
            <button type="button" disabled={!!broken} onClick={() => startNew()} className={primaryButton}>
              <Plus size={13} />
              添加模型
            </button>
          )}
        </>
      }
    >
      {error && !broken && <ErrorLine>读不到模型目录：{error}</ErrorLine>}
      {loading && <Loading />}
      {broken && (
        <div className="flex flex-col gap-2.5">
          <div className="h-[52px] rounded-xl bg-[#f5f5f7]" />
          <div className="h-[52px] rounded-xl bg-[#f5f5f7]" />
        </div>
      )}
      {!broken && all.length > 0 && (
        <div className="flex flex-col gap-1.5">
          <span className="text-[13px] font-medium text-ink">默认模型</span>
          <div className="flex items-center gap-2">
            <Select
              width={280}
              value={defaultId}
              disabled={!!dockModelEnv}
              options={all.map((m) => ({ value: m.id, label: m.label }))}
              onChange={async (id) => {
                const next = await autosave(() => modelDefault(id));
                if (next) onListing(next);
              }}
            />
            {dockModelEnv && <EnvPill name="DOCK_MODEL" />}
          </div>
          <span className="text-xs text-ink-4">
            {dockModelEnv
              ? `现在用的是 DOCK_MODEL=${dockModelEnv}；去掉这个环境变量后这里才生效`
              : "新对话用这个模型；DOCK_MODEL 环境变量会覆盖它"}
          </span>
        </div>
      )}
      {!broken && (
        <SubscriptionSection
          remote={remote !== null}
          defaultId={defaultId}
          onDefault={async (id) => {
            const next = await autosave(() => modelDefault(id));
            if (next) onListing(next);
          }}
          onCatalogChanged={onReload}
        />
      )}
      {!broken && (
        <Tabs
          value={tab}
          tabs={[
            { value: "providers", label: "供应商" },
            { value: "custom", label: models.length ? `自定义模型 · ${models.length}` : "自定义模型" },
          ]}
          onChange={setTab}
        />
      )}
      {!broken && tab === "providers" && <ProvidersTab listing={listing} onListing={onListing} onEditModel={(id) => void edit(id)} />}
      {!broken && tab === "custom" && listing && models.length === 0 && (
        <div className="enter-fade flex flex-col items-center gap-3 py-24 text-center">
          <span className="flex size-10 items-center justify-center rounded-full bg-fill text-ink-4">
            <Plus size={18} />
          </span>
          <span className="text-[14px] font-semibold text-ink">没有自定义模型</span>
          <span className="text-[12px] text-ink-4">供应商目录里没有的端点（自建网关、本地服务）在这里手填连接</span>
          <div className="flex items-center gap-2">
            <button type="button" onClick={() => startNew()} className={primaryButton}>
              <Plus size={13} />
              添加模型
            </button>
            {templateMenu}
          </div>
          <span className="text-[11.5px] text-ink-4">{TEMPLATES.map((t) => t.label).join(" · ")}</span>
        </div>
      )}
      {!broken && tab === "custom" && models.length > 0 && (
        <>
          <div className="flex flex-col gap-2.5">
            {models.map((m) => (
              <ModelCard
                key={m.id}
                model={m}
                isDefault={m.id === defaultId}
                nextDefault={all.find((x) => x.id !== m.id)?.label ?? null}
                onEdit={() => edit(m.id)}
                onDuplicate={() => duplicate(m.id)}
                onDefault={async () => {
                  const next = await autosave(() => modelDefault(m.id));
                  if (next) onListing(next);
                }}
                onDeleted={onListing}
              />
            ))}
          </div>
          <button type="button" onClick={onReload} className="self-start text-[12px] text-ink-4 hover:text-ink-2">
            手动改过 config.toml？点这里重新读取
          </button>
        </>
      )}
    </Page>
  );
}

// ---- 订阅通道（auth/status） ----

/** 打开外部链接：桌面端走系统浏览器，浏览器里退回 window.open。 */
const openExternal = (url: string) =>
  void openUrl(url).catch(() => window.open(url, "_blank", "noopener"));

/** 订阅通道：各 provider 的账号、登录进度、登录 / 登出 / 加账号。
 *  老 Dock 没有 `auth/status`（或没装鉴权插件）就整块不显示。变化跟着 `auth/changed` 推送重拉。 */
function SubscriptionSection({
  remote,
  defaultId,
  onDefault,
  onCatalogChanged,
}: {
  remote: boolean;
  defaultId: string;
  onDefault: (id: string) => void;
  /** 自动发现的模型变了（登录 / 登出 / 刷新）：模型目录要重拉，默认模型下拉才看得到。 */
  onCatalogChanged: () => void;
}) {
  const auth = useLoad(authStatus);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  // 回调每次渲染都是新的：经 ref 取最新那个，订阅只挂一次。
  const reload = useRef(auth.reload);
  reload.current = auth.reload;
  const catalogChanged = useRef(onCatalogChanged);
  catalogChanged.current = onCatalogChanged;
  useEffect(
    () =>
      onAuthChanged(() => {
        void reload.current();
        catalogChanged.current();
      }),
    [],
  );

  const run = async (key: string, task: () => Promise<unknown>) => {
    setBusy(key);
    setError(null);
    try {
      await task();
      await auth.reload();
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(null);
    }
  };
  // 登录在 Dock 那头跑；授权页地址立刻回来，**客户端自己开**。
  const login = (provider: string, account?: string) =>
    run(`${provider}:login:${account ?? ""}`, async () => {
      const res = await authLogin(provider, { account });
      if (res?.url) openExternal(res.url);
    });
  const logout = (provider: string, clientId: string) =>
    run(`${provider}:logout:${clientId}`, async () => {
      const res = await authLogout(provider, clientId);
      if (res?.warning) setError(res.warning);
    });

  const providers = auth.data?.providers ?? [];
  if (providers.length === 0) return null;
  return (
    <div className="flex flex-col gap-1.5">
      <span className="text-[13px] font-medium text-ink">订阅通道</span>
      {error && <ErrorLine>{error}</ErrorLine>}
      {remote && (
        <span className="text-xs text-ink-4">
          连的是远程 Dock：授权回调落在远端那台机器上，浏览器在这边接不到。按官方的做法在本机登录，
          再把凭据文件传过去，在远端执行 /auth import。
        </span>
      )}
      {providers.map((p) => (
        <ProviderAccounts
          key={p.id}
          provider={p}
          busy={busy}
          disabled={remote}
          onLogin={login}
          onLogout={logout}
          onCancel={(id) => run(`${id}:cancel`, () => authCancel(id))}
          defaultId={defaultId}
          onDefault={onDefault}
          onRefreshModels={(id) => run(`${id}:models`, () => authModels(id))}
        />
      ))}
    </div>
  );
}

function ProviderAccounts({
  provider: p,
  busy,
  disabled,
  onLogin,
  onLogout,
  onCancel,
  defaultId,
  onDefault,
  onRefreshModels,
}: {
  provider: AuthProviderStatus;
  busy: string | null;
  disabled: boolean;
  onLogin: (provider: string, account?: string) => void;
  onLogout: (provider: string, clientId: string) => void;
  onCancel: (provider: string) => void;
  defaultId: string;
  onDefault: (id: string) => void;
  onRefreshModels: (provider: string) => void;
}) {
  const name = p.displayName || p.id;
  const pending = p.login?.state === "pending";
  // 活动账号登着、开了套餐额度，才有可选的模型（否则 Dock 已经把它们从目录里拿掉了）。
  const active = p.accounts.find((a) => a.active);
  const canUseModels = !!active?.signedIn && active.planUsage;
  const models = p.models ?? [];
  const refreshing = busy === `${p.id}:models`;
  // 官方 UI 规范：开始登录的那个按钮叫「Continue with ChatGPT」。
  const continueLabel = `Continue with ${name}`;
  return (
    <div className="flex flex-col gap-1.5 text-xs text-ink-4">
      <div className="flex items-center gap-2">
        <span>
          {name} · {p.accounts.length ? `${p.accounts.length} 个账号` : "还没登录"}
        </span>
        {p.accounts.length === 0 ? (
          <button
            type="button"
            disabled={disabled || busy !== null}
            onClick={() => onLogin(p.id)}
            className={secondaryButton}
          >
            {continueLabel}
          </button>
        ) : (
          <button
            type="button"
            disabled={disabled || busy !== null}
            onClick={() => onLogin(p.id, "new")}
            className={smallButton}
          >
            添加账号
          </button>
        )}
        {p.usageUrl && (
          <button type="button" onClick={() => openExternal(p.usageUrl!)} className={smallButton}>
            Manage usage
          </button>
        )}
      </div>
      {pending && (
        <span className="flex items-center gap-2">
          <LoaderCircle className="size-3.5 animate-spin" />
          <span>在浏览器里完成授权（10 分钟内有效）</span>
          {p.login?.url && (
            <button type="button" onClick={() => openExternal(p.login!.url!)} className={smallButton}>
              重新打开授权页
            </button>
          )}
          <button type="button" onClick={() => onCancel(p.id)} className={smallButton}>
            取消登录
          </button>
        </span>
      )}
      {p.login?.state === "failed" && p.login.message && <ErrorLine>{p.login.message}</ErrorLine>}
      {p.accounts.map((a) => (
        <span key={a.clientId} className="flex items-center gap-2">
          {a.signedIn ? (
            <Pill kind={a.planUsage ? undefined : "orange"}>{a.planUsage ? "套餐额度已开" : "套餐额度未开"}</Pill>
          ) : (
            <Pill kind="orange">已登出</Pill>
          )}
          <span>
            {a.email ?? a.clientId}
            {a.active ? " ·活动" : ""}
          </span>
          {a.signedIn ? (
            <button
              type="button"
              disabled={busy !== null}
              onClick={() => onLogout(p.id, a.clientId)}
              className={smallButton}
            >
              登出
            </button>
          ) : (
            // 登出了注册还在：重新登录直接复用这个账号的注册。
            <button
              type="button"
              disabled={disabled || busy !== null}
              onClick={() => onLogin(p.id, a.clientId)}
              className={smallButton}
            >
              {continueLabel}
            </button>
          )}
        </span>
      ))}
      {canUseModels && (
        <div className="mt-1.5 flex flex-col gap-1.5">
          <div className="flex items-center gap-2">
            <span className="text-[12.5px] font-medium text-ink-2">
              可用模型{models.length ? ` · ${models.length}` : ""}
            </span>
            <button
              type="button"
              disabled={busy !== null}
              onClick={() => onRefreshModels(p.id)}
              className={`${smallButton} gap-1`}
            >
              {refreshing && <LoaderCircle className="size-3 animate-spin" />}
              刷新
            </button>
            {p.modelsFetchedAt ? <span className="text-[11px] text-ink-5">{fetchedAgo(p.modelsFetchedAt)}</span> : null}
          </div>
          {models.length === 0 ? (
            <span>还没拉到这个账号的模型列表，点「刷新」。</span>
          ) : (
            <div className="flex flex-col gap-1.5">
              {models.map((m) => (
                <AuthModelRow
                  key={m.id}
                  model={m}
                  isDefault={m.id === defaultId}
                  disabled={busy !== null}
                  onDefault={() => onDefault(m.id)}
                />
              ))}
            </div>
          )}
          <span>已并入模型目录：输入框旁的模型菜单、终端 /model 都能直接选；用的是 {name} 套餐额度。</span>
        </div>
      )}
    </div>
  );
}

/** 账号卡片里的一个可用模型：名字、目录 id、上下文 / 推理档位 / 图片，设为默认。 */
function AuthModelRow({
  model: m,
  isDefault,
  disabled,
  onDefault,
}: {
  model: AuthModel;
  isDefault: boolean;
  disabled: boolean;
  onDefault: () => void;
}) {
  const efforts = m.reasoningEfforts?.length
    ? m.reasoningEfforts.length > 1
      ? `${m.reasoningEfforts[0]}–${m.reasoningEfforts[m.reasoningEfforts.length - 1]}`
      : m.reasoningEfforts[0]
    : null;
  const facts = [
    m.contextWindow ? `${formatTokens(m.contextWindow)} 上下文` : null,
    efforts ? `推理 ${efforts}${m.defaultReasoningEffort ? `（默认 ${m.defaultReasoningEffort}）` : ""}` : null,
    m.supportsImages ? "可看图" : null,
  ].filter((f): f is string => !!f);
  return (
    <div className="enter-fade flex items-center justify-between gap-3 rounded-xl border border-line-soft bg-white px-3.5 py-2.5 shadow-sm">
      <div className="flex min-w-0 flex-1 flex-col gap-0.5">
        <div className="flex flex-wrap items-center gap-2">
          <span className="text-[13px] font-semibold text-ink">{m.label}</span>
          <span className="font-mono text-[11px] text-ink-4">{m.id}</span>
        </div>
        {facts.length > 0 && (
          <div className="flex flex-wrap items-center gap-1.5 text-[11.5px] text-ink-3">
            {facts.map((f, i) => (
              <span key={f} className="flex items-center gap-1.5">
                {i > 0 && <span className="text-ink-5">·</span>}
                {f}
              </span>
            ))}
          </div>
        )}
        {m.description && <span className="text-[11.5px] text-ink-4">{m.description}</span>}
      </div>
      {isDefault ? (
        <Pill>默认</Pill>
      ) : (
        <button type="button" disabled={disabled} onClick={onDefault} className={smallButton}>
          设为默认
        </button>
      )}
    </div>
  );
}

/** 「3 分钟前更新」之类。 */
function fetchedAgo(unix: number): string {
  const s = Math.max(0, Math.floor(Date.now() / 1000) - unix);
  if (s < 60) return "刚刚更新";
  if (s < 3600) return `${Math.floor(s / 60)} 分钟前更新`;
  if (s < 86_400) return `${Math.floor(s / 3600)} 小时前更新`;
  return `${Math.floor(s / 86_400)} 天前更新`;
}

function KeyPill({ model }: { model: ModelInfo }) {
  // 走订阅通道的模型：密钥不由 config 给，状态看鉴权那边。
  if (model.authProvider) {
    return <Pill title={`鉴权通道 ${model.authProvider}`}>订阅通道 {model.authProvider}</Pill>;
  }
  if (model.auth === "key") return <Pill>已配置密钥</Pill>;
  if (model.auth === "env" && model.keyEnv) {
    return model.keyEnvSet ? (
      <Pill title={`从环境变量 ${model.keyEnv} 读密钥`}>环境变量 {model.keyEnv}</Pill>
    ) : (
      <Pill kind="orange">{model.keyEnv} 未设置</Pill>
    );
  }
  return <Pill kind="orange">未配置密钥</Pill>;
}

function ModelCard({
  model: m,
  isDefault,
  nextDefault,
  onEdit,
  onDuplicate,
  onDefault,
  onDeleted,
}: {
  model: ModelInfo;
  isDefault: boolean;
  nextDefault: string | null;
  onEdit: () => void;
  onDuplicate: () => void;
  onDefault: () => void;
  onDeleted: (l: ModelListing) => void;
}) {
  const [confirm, setConfirm] = useState(false);
  const [test, setTest] = useState<TestResult | "running" | null>(null);

  const runTest = async () => {
    setTest("running");
    try {
      setTest(await modelTest(null, m.id, null));
    } catch (e) {
      setTest({ ok: false, error: errorText(e), backend: "" });
    }
  };
  const remove = async () => {
    const next = await autosave(() => modelDelete(m.id));
    if (next) onDeleted(next);
    else setConfirm(false);
  };

  if (confirm) {
    return (
      <div className="enter-fade flex flex-col gap-2 rounded-xl border border-[#fecaca] bg-[#fef2f2] px-4 py-3.5">
        <div className="flex items-center gap-2">
          <span className="text-sm font-semibold text-ink">{m.label}</span>
          {isDefault && <Pill kind="blue">默认</Pill>}
        </div>
        <span className="text-[12.5px] text-[#c0392b]">
          确定删除这个模型？{isDefault && nextDefault ? `删除后默认改为 ${nextDefault}` : ""}
        </span>
        <div className="flex justify-end gap-2">
          <button type="button" onClick={() => setConfirm(false)} className={smallButton}>
            取消
          </button>
          <button
            type="button"
            onClick={remove}
            className="motion-colors flex h-7 items-center rounded-md bg-[#ff3b30] px-3 text-[11.5px] font-medium text-white hover:bg-[#e0352b]"
          >
            删除
          </button>
        </div>
      </div>
    );
  }

  const menuItem = "motion-colors flex h-8 w-full items-center rounded-md px-2.5 text-left text-[12.5px] hover:bg-hover";
  return (
    <div className="group flex flex-col gap-2 rounded-xl border border-line-soft bg-white px-4 py-3.5 shadow-sm">
      <div className="flex items-start justify-between gap-3">
        <div className="flex min-w-0 flex-1 flex-col gap-1">
          <div className="flex flex-wrap items-center gap-2">
            <span className="text-sm font-semibold text-ink">{m.label}</span>
            {m.label !== m.id && <span className="font-mono text-[11px] text-ink-4">{m.id}</span>}
            {isDefault && <Pill kind="blue">默认</Pill>}
            {!m.editable && <Pill title="写在项目配置或 [[models.catalog]] 里，设置页改不了">项目配置</Pill>}
          </div>
          <div className="flex flex-wrap items-center gap-1.5 text-xs text-ink-3">
            <span>{host(m.apiBase)}</span>
            {m.contextWindow ? (
              <>
                <span className="text-ink-5">·</span>
                <span>上下文 {formatTokens(m.contextWindow)}</span>
              </>
            ) : null}
            <span className="text-ink-5">·</span>
            {m.backends.map((b) => (
              <Pill key={b}>{SHORT[b] ?? b}</Pill>
            ))}
          </div>
          {m.description && <span className="text-xs text-ink-4">{m.description}</span>}
          {test && (
            <span className={`enter-fade text-xs ${test === "running" ? "text-ink-4" : test.ok ? "text-[#248a3d]" : "text-[#d70015]"}`}>
              {test === "running" ? "正在测试连接…" : test.ok ? `连接成功 · 用时 ${test.ms}ms` : test.error}
            </span>
          )}
        </div>
        <div className="flex shrink-0 items-center gap-2">
          <KeyPill model={m} />
          <div className="hidden items-center gap-1 group-focus-within:flex group-hover:flex">
            {m.editable && (
              <button type="button" onClick={onEdit} className={smallButton}>
                <Pencil size={11} />
                编辑
              </button>
            )}
            <Popover
              title="更多"
              side="bottom"
              align="right"
              className="flex size-7 items-center justify-center rounded-md border border-line-strong bg-white text-ink-3 shadow-sm hover:bg-hover"
              panelClassName="min-w-[160px] px-1"
              trigger={<MoreHorizontal size={14} />}
            >
              {(close) => (
                <>
                  {!isDefault && (
                    <button type="button" className={menuItem} onClick={() => (close(), onDefault())}>
                      设为默认
                    </button>
                  )}
                  {m.editable && (
                    <button type="button" className={menuItem} onClick={() => (close(), onDuplicate())}>
                      复制一份
                    </button>
                  )}
                  <button type="button" className={menuItem} onClick={() => (close(), void runTest())}>
                    测试连接
                  </button>
                  {m.editable && (
                    <>
                      <div className="my-1 h-px bg-line-soft" />
                      <button type="button" className={`${menuItem} text-[#ff3b30]`} onClick={() => (close(), setConfirm(true))}>
                        删除
                      </button>
                    </>
                  )}
                </>
              )}
            </Popover>
          </div>
        </div>
      </div>
    </div>
  );
}

// ---- 编辑 ----

const nul = (s: string) => (s.trim() ? s.trim() : null);
const num = (s: string) => {
  const n = Number(s);
  return s.trim() && Number.isFinite(n) && n >= 0 ? n : null;
};
const isUrl = (s: string) => /^https?:\/\/[^\s/?#]+/.test(s.trim());

function ModelEditor({
  initial,
  originalId,
  takenIds,
  onDone,
}: {
  initial: ModelEntry;
  originalId: string | null;
  takenIds: string[];
  onDone: (listing: ModelListing | null) => void;
}) {
  const [e, setE] = useState<ModelEntry>(initial);
  const [keyDraft, setKeyDraft] = useState("");
  const [showErrors, setShowErrors] = useState(false);
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [test, setTest] = useState<TestResult | "running" | null>(null);
  const [advanced, setAdvanced] = useState(originalId !== null);
  const [openOverride, setOpenOverride] = useState<string | null>(null);
  const [envSet, setEnvSet] = useState<boolean | null>(null);
  const isNew = originalId === null;
  const set = <K extends keyof ModelEntry>(k: K, v: ModelEntry[K]) => setE((x) => ({ ...x, [k]: v }));

  // 变量名停手 400ms 后问一次 Dock 有没有这个环境变量。
  useEffect(() => {
    const name = e.envKey?.trim();
    if (e.keyMode !== "env" || !name) return setEnvSet(null);
    const t = window.setTimeout(() => {
      configEnv([name]).then((r) => setEnvSet(!!r[name]), () => setEnvSet(null));
    }, 400);
    return () => window.clearTimeout(t);
  }, [e.envKey, e.keyMode]);

  const errors = useMemo(() => {
    const out: Partial<Record<"id" | "url" | "backends", string>> = {};
    const id = e.id.trim();
    if (!id) out.id = "模型 id 不能为空";
    else if (/["\u0000-\u001f]/.test(id)) out.id = "模型 id 里不能有引号";
    else if (takenIds.includes(id)) out.id = `已存在同名模型 id：${id}`;
    // 挂在供应商下：地址、协议不写 = 跟供应商走。
    if (!e.apiBaseUrl?.trim()) {
      if (!e.provider) out.url = "接口地址不能为空";
    } else if (!isUrl(e.apiBaseUrl)) out.url = "不是合法 URL";
    if (e.apiBackends.length === 0 && !e.provider) out.backends = "至少勾选一个协议";
    return out;
  }, [e, takenIds]);
  const invalid = Object.keys(errors).length > 0;

  /** 发给 Dock 的那份：行内密钥只在填了新值时带。 */
  const payload = (): ModelEntry => {
    const out: ModelEntry = { ...e, id: e.id.trim() };
    if (e.keyMode === "inline" && keyDraft.trim()) out.apiKey = keyDraft.trim();
    else delete out.apiKey;
    return out;
  };

  const save = async (makeDefault: boolean) => {
    setShowErrors(true);
    if (invalid) return;
    setSaving(true);
    setSaveError(null);
    try {
      onDone(await modelSave(payload(), originalId, makeDefault));
    } catch (err) {
      setSaveError(errorText(err));
      setSaving(false);
    }
  };
  const runTest = async () => {
    setShowErrors(true);
    if (invalid) return;
    setTest("running");
    try {
      setTest(await modelTest(payload(), null, originalId));
    } catch (err) {
      setTest({ ok: false, error: errorText(err), backend: "" });
    }
  };

  // 协议：勾选的按顺序排前面（第一条 = 默认），没勾的跟在后面。
  const order = [...e.apiBackends, ...BACKENDS.filter((b) => !e.apiBackends.includes(b))];
  const [dragging, setDragging] = useState<string | null>(null);
  const toggleBackend = (b: string, on: boolean) =>
    set("apiBackends", on ? [...e.apiBackends, b] : e.apiBackends.filter((x) => x !== b));
  const moveBackend = (from: string, to: string) => {
    if (from === to || !e.apiBackends.includes(from) || !e.apiBackends.includes(to)) return;
    const list = e.apiBackends.filter((x) => x !== from);
    list.splice(list.indexOf(to), 0, from);
    set("apiBackends", list);
  };

  const pricing = e.pricing ?? { input: null, cacheRead: null, cacheWrite: null, output: null };
  const setPrice = (k: keyof typeof pricing, v: string) => {
    const next = { ...pricing, [k]: num(v) };
    set("pricing", Object.values(next).some((x) => x !== null) ? next : null);
  };
  const hasMessages = e.apiBackends.includes("messages");
  const err = (k: keyof typeof errors) => (showErrors ? errors[k] : undefined);

  return (
    <Page
      title={isNew ? "添加模型" : `编辑 ${initial.name ?? initial.id}`}
      subtitle={showErrors && invalid ? "请修正标红的字段后再保存" : "保存后新对话立即生效"}
      back={{ label: "模型与密钥", onClick: () => onDone(null) }}
      banner={saveError && <Banner kind="error">{saveError}</Banner>}
    >
      <Section title="连接">
        {e.provider && (
          <span className="text-[12px] text-ink-4">
            连接继承自供应商「{e.provider}」：地址、协议、密钥留空就跟着它，填了的覆盖它。
          </span>
        )}
        <Card>
          <Row label="显示名">
            <TextInput value={e.name ?? ""} onChange={(v) => set("name", nul(v))} placeholder="留空 = 用模型 id" />
          </Row>
          <Row
            label="模型 id"
            desc={err("id") ? <span className="text-[#ff3b30]">{err("id")}</span> : isNew ? "config.toml 里的条目名" : "编辑时只读（旧会话记着这个 id）"}
          >
            <TextInput value={e.id} onChange={(v) => set("id", v)} mono invalid={!!err("id")} disabled={!isNew} placeholder="如 deepseek-flash" />
          </Row>
          <Row label="描述">
            <TextInput value={e.description ?? ""} onChange={(v) => set("description", nul(v))} placeholder="一句话说明（可选）" />
          </Row>
          <Row label="接口地址" keyHint="api_base_url" desc={err("url") && <span className="text-[#ff3b30]">{err("url")}</span>}>
            <TextInput
              value={e.apiBaseUrl ?? ""}
              onChange={(v) => set("apiBaseUrl", nul(v))}
              mono
              invalid={!!err("url")}
              placeholder="https://api.example.com/v1"
            />
          </Row>
          <Row label="上游模型名" keyHint="api_model">
            <TextInput value={e.apiModel ?? ""} onChange={(v) => set("apiModel", nul(v))} mono placeholder="留空 = 用模型 id" />
          </Row>
          <Row
            label="支持的协议"
            desc={err("backends") ? <span className="text-[#ff3b30]">{err("backends")}</span> : "拖动排序，第一条是默认"}
            top
          >
            <div className="flex w-[280px] flex-col gap-1">
              {order.map((b) => {
                const on = e.apiBackends.includes(b);
                return (
                  <div
                    key={b}
                    draggable={on}
                    onDragStart={() => setDragging(b)}
                    onDragEnd={() => setDragging(null)}
                    onDragOver={(ev) => {
                      if (dragging && on) {
                        ev.preventDefault();
                        moveBackend(dragging, b);
                      }
                    }}
                    className={`motion-colors flex h-8 items-center gap-2 rounded-lg px-2 ${
                      err("backends") ? "bg-[#fef2f2]" : "bg-[#f5f5f7]"
                    } ${dragging === b ? "opacity-60" : ""}`}
                  >
                    <GripVertical size={13} className={on ? "cursor-grab text-ink-4" : "text-ink-5"} />
                    <Checkbox checked={on} onChange={(v) => toggleBackend(b, v)} label={b} />
                    <span className="flex-1 text-[12.5px] text-ink">{b}</span>
                    {on && e.apiBackends[0] === b && <span className="text-[10px] font-semibold text-accent">默认</span>}
                  </div>
                );
              })}
            </div>
          </Row>
          <Row label="鉴权方式" desc="自动 = messages 用 x-api-key，另两条用 Bearer">
            <Segmented
              value={e.authScheme ?? "auto"}
              options={[
                { value: "auto", label: "自动" },
                { value: "bearer", label: "Bearer" },
                { value: "x_api_key", label: "x-api-key" },
              ]}
              onChange={(v) => set("authScheme", v === "auto" ? null : v)}
            />
          </Row>
        </Card>
      </Section>

      <Section title="密钥">
        <Card>
          <Row label="密钥来源">
            <Segmented
              value={e.keyMode}
              options={[
                { value: "env", label: "环境变量" },
                { value: "inline", label: "直接填写" },
              ]}
              onChange={(v) => set("keyMode", v)}
            />
          </Row>
          {e.keyMode === "env" ? (
            <Row label="变量名" desc="Dock 启动时的环境里要有它（改了要重启 Dock 才读得到）">
              <TextInput value={e.envKey ?? ""} onChange={(v) => set("envKey", nul(v))} mono width={200} placeholder="DEEPSEEK_API_KEY" />
              {envSet !== null && (
                <span className={`text-[11px] font-medium ${envSet ? "text-[#248a3d]" : "text-[#c93400]"}`}>
                  {envSet ? "已检测到 ✓" : "当前进程里没有这个变量"}
                </span>
              )}
            </Row>
          ) : (
            <Row
              label="密钥"
              desc={<span className="text-[#c93400]">会以明文写进 config.toml，建议用环境变量</span>}
            >
              <TextInput
                value={keyDraft}
                onChange={setKeyDraft}
                type="password"
                mono
                width={240}
                placeholder={e.hasApiKey ? "已保存（留空 = 不改）" : "sk-…"}
              />
            </Row>
          )}
          <Row label={<button type="button" disabled={test === "running"} onClick={runTest} className={secondaryButton}>
                {test === "running" && <LoaderCircle size={13} className="animate-spin" />}
                测试连接
              </button>}>
            {test && test !== "running" && (
              <span className={`enter-fade max-w-[420px] text-right text-xs ${test.ok ? "text-[#248a3d]" : "text-[#d70015]"}`}>
                {test.ok ? `连接成功 · 用时 ${test.ms}ms` : test.error}
              </span>
            )}
          </Row>
        </Card>
      </Section>

      <Section
        title="高级"
        right={
          <button type="button" onClick={() => setAdvanced((v) => !v)} className="flex items-center gap-1 text-[11.5px] text-ink-4 hover:text-ink-2">
            <ChevronDown size={12} className={`transition-transform ${advanced ? "" : "-rotate-90"}`} />
            {advanced ? "收起" : "展开"}
          </button>
        }
      >
        <div className="fold" data-open={advanced}>
          <div>
            <Card>
              <Row label="上下文窗口" desc="token，不填按 128k 估">
                <TextInput value={e.contextWindow?.toString() ?? ""} onChange={(v) => set("contextWindow", num(v))} width={120} placeholder="128000" />
              </Row>
              <Row label="单次输出上限" desc="不填不发（messages 协议兜底 16384）">
                <TextInput value={e.maxOutputTokens?.toString() ?? ""} onChange={(v) => set("maxOutputTokens", num(v))} width={120} placeholder="不填不发" />
              </Row>
              <Row label="推理" desc="关 = 这个模型没有推理档，不发任何推理参数">
                <Toggle on={e.reasoning !== false} onChange={(v) => set("reasoning", v ? null : false)} />
              </Row>
              {e.reasoning !== false && (
                <>
                  <Row label="默认强度" desc="不选 = 不发，用上游默认">
                    <Select
                      value={e.reasoningEffort ?? ""}
                      placeholder="上游默认"
                      options={(e.reasoningEfforts ?? EFFORTS).map((x) => ({ value: x, label: x }))}
                      onChange={(v) => set("reasoningEffort", v || null)}
                    />
                  </Row>
                  <Row label="可选档位" desc="/effort 菜单列哪几档">
                    <div className="flex gap-1">
                      {EFFORTS.map((x) => {
                        const list = e.reasoningEfforts ?? EFFORTS;
                        const on = list.includes(x);
                        return (
                          <button
                            key={x}
                            type="button"
                            onClick={() => {
                              const next = on ? list.filter((y) => y !== x) : EFFORTS.filter((y) => y === x || list.includes(y));
                              set("reasoningEfforts", next.length === EFFORTS.length ? null : next);
                            }}
                            className={`motion-colors rounded-full px-2 py-0.5 text-[11px] font-medium ${on ? "bg-[#0a84ff1a] text-accent" : "bg-[#f2f2f7] text-ink-3"}`}
                          >
                            {x}
                          </button>
                        );
                      })}
                    </div>
                  </Row>
                </>
              )}
              <Row label="支持图片" desc="关 = 截图类工具结果不进请求体">
                <Toggle on={e.supportsImages !== false} onChange={(v) => set("supportsImages", v ? null : false)} />
              </Row>
              {hasMessages && (
                <Row label="提示缓存" desc="已勾选 messages，可开启；自建代理不认 cache_control 时关掉">
                  <Toggle on={e.promptCache !== false} onChange={(v) => set("promptCache", v ? null : false)} />
                </Row>
              )}
              <Row label="单价 (USD / 百万 token)" desc="写了 /usage 才算得出金额，约数，不是账单" top>
                <div className="flex gap-2">
                  {(
                    [
                      ["input", "输入"],
                      ["cacheRead", "缓存读"],
                      ["cacheWrite", "缓存写"],
                      ["output", "输出"],
                    ] as const
                  ).map(([k, label]) => (
                    <label key={k} className="flex flex-col gap-1">
                      <span className="text-[10.5px] text-ink-4">{label}</span>
                      <TextInput value={pricing[k]?.toString() ?? ""} onChange={(v) => setPrice(k, v)} width={70} placeholder="—" />
                    </label>
                  ))}
                </div>
              </Row>
              <Row label="按协议覆盖" desc="同一端点各协议入口不同时用，如 DeepSeek 的 /anthropic" top>
                <div className="flex w-[380px] flex-col gap-1.5">
                  {e.apiBackends.map((b) => {
                    const o = e.overrides[b] ?? { apiBaseUrl: null, authScheme: null, apiModel: null };
                    const open = openOverride === b;
                    const setO = (k: keyof typeof o, v: string | null) =>
                      set("overrides", { ...e.overrides, [b]: { ...o, [k]: v } });
                    const filled = !!(o.apiBaseUrl || o.authScheme || o.apiModel);
                    return (
                      <div key={b} className="overflow-hidden rounded-lg border border-line-soft bg-[#fafafc]">
                        <button
                          type="button"
                          onClick={() => setOpenOverride(open ? null : b)}
                          className="flex h-8 w-full items-center gap-1.5 px-2.5 text-left text-[12.5px] text-ink"
                        >
                          <ChevronDown size={12} className={`text-ink-4 transition-transform ${open ? "" : "-rotate-90"}`} />
                          <span className="flex-1">{b}</span>
                          {filled && <span className="text-[10.5px] text-accent">已覆盖</span>}
                        </button>
                        <div className="fold" data-open={open}>
                          <div className="flex flex-col gap-1.5 px-2.5 pb-2.5">
                            {(
                              [
                                ["apiBaseUrl", "地址", "留空 = 用上面的"],
                                ["apiModel", "上游模型名", "留空 = 继承"],
                              ] as const
                            ).map(([k, label, ph]) => (
                              <div key={k} className="flex items-center justify-between gap-2">
                                <span className="text-[11.5px] text-ink-3">{label}</span>
                                <TextInput value={o[k] ?? ""} onChange={(v) => setO(k, nul(v))} mono width={250} placeholder={ph} />
                              </div>
                            ))}
                            <div className="flex items-center justify-between gap-2">
                              <span className="text-[11.5px] text-ink-3">鉴权</span>
                              <Select
                                width={250}
                                value={o.authScheme ?? ""}
                                placeholder="继承"
                                options={[
                                  { value: "bearer", label: "Bearer" },
                                  { value: "x_api_key", label: "x-api-key" },
                                ]}
                                onChange={(v) => setO("authScheme", v || null)}
                              />
                            </div>
                          </div>
                        </div>
                      </div>
                    );
                  })}
                </div>
              </Row>
            </Card>
          </div>
        </div>
      </Section>

      <div className="flex justify-end gap-2 pb-4">
        <button type="button" onClick={() => onDone(null)} className={secondaryButton}>
          取消
        </button>
        {isNew && (
          <button type="button" disabled={saving} onClick={() => save(true)} className={secondaryButton}>
            保存并设为默认
          </button>
        )}
        <button type="button" disabled={saving || (showErrors && invalid)} onClick={() => save(false)} className={primaryButton}>
          {saving && <LoaderCircle size={13} className="animate-spin" />}
          保存
        </button>
      </div>
    </Page>
  );
}
