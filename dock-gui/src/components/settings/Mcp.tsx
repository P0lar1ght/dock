// MCP 服务：列表（gel）/ 添加本地命令（bud）/ 添加远程 HTTP（aud）/ 浏览器登录中（has）。
// 写 ~/.dock/config.toml 的 [mcp_servers.<name>]，写完 Dock 当场对账（连上 / 断开 / 重连）。

import { useEffect, useState } from "react";
import { ChevronDown, LoaderCircle, MoreHorizontal, Plus, RefreshCw, X } from "lucide-react";
import { store, useApp, type McpServerInfo } from "../../dock/store";
import { Popover } from "../Popover";
import {
  EMPTY_MCP,
  errorText,
  mcpDelete,
  mcpEnable,
  mcpGet,
  mcpLogin,
  mcpSave,
  mcpToolEnable,
  type McpEntry,
} from "../../dock/settingsApi";
import {
  Banner,
  Card,
  ErrorLine,
  Loading,
  Page,
  Pill,
  Row,
  Section,
  Segmented,
  TextInput,
  Toggle,
  autosave,
  primaryButton,
  secondaryButton,
  smallButton,
  useLoad,
} from "./ui";

type Editing = { entry: McpEntry; originalName: string | null };

export function McpPane({ onCount }: { onCount: (n: number) => void }) {
  const list = useLoad(store.mcpList);
  const [editing, setEditing] = useState<Editing | null>(null);
  useEffect(() => {
    if (list.data) onCount(list.data.length);
  }, [list.data, onCount]);

  if (editing) {
    return (
      <McpEditor
        key={editing.originalName ?? "new"}
        initial={editing.entry}
        originalName={editing.originalName}
        taken={(list.data ?? []).map((s) => s.name).filter((n) => n !== editing.originalName)}
        onDone={(servers) => {
          setEditing(null);
          if (servers) list.set(servers);
          else void list.reload();
        }}
      />
    );
  }
  return (
    <McpList
      servers={list.data ?? null}
      error={list.error ?? null}
      onServers={list.set}
      onEdit={async (name) => {
        if (name === null) return setEditing({ entry: { ...EMPTY_MCP }, originalName: null });
        try {
          setEditing({ entry: await mcpGet(name), originalName: name });
        } catch (e) {
          alert(errorText(e));
        }
      }}
    />
  );
}

function McpList({
  servers,
  error,
  onServers,
  onEdit,
}: {
  servers: McpServerInfo[] | null;
  error: string | null;
  onServers: (s: McpServerInfo[]) => void;
  onEdit: (name: string | null) => void;
}) {
  const [reloading, setReloading] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const reloadConfig = async () => {
    setReloading(true);
    setNotice(null);
    try {
      const r = await store.mcpReload();
      onServers(r.servers);
      setNotice(r.summary);
    } catch (e) {
      setNotice(errorText(e));
    } finally {
      setReloading(false);
    }
  };
  return (
    <Page
      title="MCP 服务"
      subtitle={`管理本地与远程 MCP${servers ? ` · ${servers.length} 个服务` : ""}`}
      actions={
        <>
          <button type="button" onClick={reloadConfig} disabled={reloading} className={secondaryButton} title="重读 config.toml 里的 [mcp_servers.*]">
            <RefreshCw size={13} className={reloading ? "animate-spin" : ""} />
            重新加载配置
          </button>
          <button type="button" onClick={() => onEdit(null)} className={primaryButton}>
            <Plus size={13} />
            添加服务
          </button>
        </>
      }
    >
      {notice && <div className="enter-fade text-[12.5px] text-ink-3">{notice}</div>}
      {error && <ErrorLine>读不到 MCP 状态：{error}</ErrorLine>}
      {!servers && !error && <Loading />}
      {servers && servers.length === 0 && (
        <Card className="px-4 py-3.5 text-[13px] text-ink-3">还没有 MCP 服务。点「添加服务」，或在 ~/.dock/config.toml 里写 [mcp_servers.&lt;名字&gt;]。</Card>
      )}
      <div className="flex flex-col gap-2.5">
        {servers?.map((s) => (
          <McpCard key={s.name} server={s} onServers={onServers} onEdit={() => onEdit(s.name)} />
        ))}
      </div>
    </Page>
  );
}

const STATUS: Record<McpServerInfo["status"], { label: string; kind: "green" | "red" | "orange" | "gray" }> = {
  connected: { label: "已连接", kind: "green" },
  failed: { label: "连接失败", kind: "red" },
  needs_auth: { label: "需要登录", kind: "orange" },
  disabled: { label: "已停用", kind: "gray" },
};

function McpCard({
  server: s,
  onServers,
  onEdit,
}: {
  server: McpServerInfo;
  onServers: (s: McpServerInfo[]) => void;
  onEdit: () => void;
}) {
  const remote = useApp().remote;
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState<null | "reconnect" | "toggle" | "login" | "delete">(null);
  const [confirm, setConfirm] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const st = STATUS[s.status];
  const off = s.status === "disabled";
  const bad = s.status === "failed";

  const refresh = async () => onServers(await store.mcpList());
  const run = async (kind: NonNullable<typeof busy>, f: () => Promise<unknown>) => {
    setBusy(kind);
    setError(null);
    try {
      await f();
    } catch (e) {
      setError(errorText(e));
    }
    await refresh().catch(() => {});
    setBusy(null);
  };

  const tools =
    s.enabledToolCount === s.toolCount ? `${s.toolCount} 个工具` : `${s.toolCount} 个工具 · ${s.enabledToolCount} 个已启用`;
  const reason = error ?? (bad ? s.detail : null);
  const menuItem = "motion-colors flex h-8 w-full items-center rounded-md px-2.5 text-left text-[12.5px] hover:bg-hover";

  return (
    <div className={`group flex flex-col overflow-hidden rounded-xl border bg-white shadow-sm ${bad ? "border-[#fecaca]" : "border-line-soft"}`}>
      <div className="flex items-center justify-between gap-3 px-4 py-3">
        <div className="flex min-w-0 flex-col gap-0.5">
          <div className="flex items-center gap-2">
            <span className={`text-sm font-semibold ${off ? "text-ink-4" : "text-ink"}`}>{s.name}</span>
            {s.builtin && !s.editable && <Pill>内置</Pill>}
            <Pill kind={st.kind}>{st.label}</Pill>
          </div>
          {s.status === "connected" && <span className="text-xs text-ink-4">{tools}</span>}
        </div>
        <div className="flex shrink-0 items-center gap-2">
          {/* OAuth 登录在 Dock 那台机器上开浏览器、等回环回调：连远程时只能去远端登录。 */}
          {s.status === "needs_auth" && busy !== "login" && !remote && (
            <button
              type="button"
              onClick={() => run("login", () => mcpLogin(s.name))}
              className="motion-colors flex h-7 items-center rounded-md bg-ink px-2.5 text-xs font-medium text-white hover:bg-ink-2"
            >
              登录
            </button>
          )}
          {s.status === "needs_auth" && remote && (
            <span className="text-[11.5px] text-ink-4" title="OAuth 要在远端机器的浏览器里完成">
              在远端用 dock 登录
            </span>
          )}
          {bad && (
            <button type="button" disabled={!!busy} onClick={() => run("reconnect", () => store.mcpReconnect(s.name))} className={smallButton}>
              <RefreshCw size={11} className={busy === "reconnect" ? "animate-spin" : ""} />
              重试
            </button>
          )}
          {s.status === "connected" && s.toolCount > 0 && (
            <button
              type="button"
              aria-label={open ? "收起工具" : "展开工具"}
              onClick={() => setOpen((v) => !v)}
              className="motion-colors flex size-7 items-center justify-center rounded-md text-ink-4 hover:bg-hover"
            >
              <ChevronDown size={14} className={`transition-transform ${open ? "rotate-180" : ""}`} />
            </button>
          )}
          <Popover
            title="更多"
            side="bottom"
            align="right"
            className="flex size-7 items-center justify-center rounded-md text-ink-4 opacity-0 hover:bg-hover group-hover:opacity-100 focus:opacity-100"
            panelClassName="min-w-[150px] px-1"
            trigger={<MoreHorizontal size={14} />}
          >
            {(close) => (
              <>
                {s.editable && (
                  <button type="button" className={menuItem} onClick={() => (close(), onEdit())}>
                    编辑
                  </button>
                )}
                {!off && (
                  <button type="button" className={menuItem} onClick={() => (close(), void run("reconnect", () => store.mcpReconnect(s.name)))}>
                    重新连接
                  </button>
                )}
                {s.editable && (
                  <>
                    <div className="my-1 h-px bg-line-soft" />
                    <button type="button" className={`${menuItem} text-[#ff3b30]`} onClick={() => (close(), setConfirm(true))}>
                      删除
                    </button>
                  </>
                )}
                {!s.editable && <div className="px-2.5 py-1.5 text-[11.5px] text-ink-4">内置服务只能停用</div>}
              </>
            )}
          </Popover>
          <Toggle
            on={!off}
            disabled={busy === "toggle"}
            label={off ? "启用" : "停用"}
            onChange={(v) =>
              run("toggle", async () => {
                const r = await mcpEnable(s.name, v);
                if (r.error) throw new Error(r.error);
              })
            }
          />
        </div>
      </div>

      {busy === "login" && (
        <div className="enter-fade mx-4 mb-3 flex flex-col gap-2 rounded-lg bg-[#f5f5f7] p-3">
          <span className="flex items-center gap-2 text-[13px] font-medium text-ink">
            <LoaderCircle size={14} className="animate-spin text-accent" />
            正在浏览器中登录…
          </span>
          <span className="text-xs text-ink-4">已打开系统浏览器完成 {s.name} 的授权。在浏览器里完成授权后，这里会自动刷新。</span>
        </div>
      )}

      {confirm && (
        <div className="enter-fade mx-4 mb-3 flex items-center justify-between gap-3 rounded-lg border border-[#fecaca] bg-[#fef2f2] px-3 py-2">
          <span className="text-[12.5px] text-[#c0392b]">从 config.toml 删掉 {s.name}，断开连接？</span>
          <span className="flex gap-1.5">
            <button type="button" onClick={() => setConfirm(false)} className={smallButton}>
              取消
            </button>
            <button
              type="button"
              disabled={busy === "delete"}
              onClick={() =>
                run("delete", async () => {
                  const r = await mcpDelete(s.name);
                  onServers(r.servers);
                })
              }
              className="motion-colors flex h-7 items-center rounded-md bg-[#ff3b30] px-3 text-[11.5px] font-medium text-white hover:bg-[#e0352b]"
            >
              删除
            </button>
          </span>
        </div>
      )}

      {reason && (
        <div className="enter-fade border-t border-[#fee2e2] bg-[#fef2f2] px-4 py-2.5 font-mono text-xs break-words text-[#d70015]">{reason}</div>
      )}

      <div className="fold" data-open={open}>
        <div>
          <div className="border-t border-[#f0f0f3]">
            {s.tools.map((t) => (
              <div key={t.name} className="flex items-center justify-between gap-3 border-b border-[#f5f5f7] px-4 py-2 last:border-b-0">
                <div className="flex min-w-0 flex-col">
                  <span className={`font-mono text-xs ${t.enabled ? "text-ink" : "text-ink-4"}`}>{t.name}</span>
                  {t.description && <span className="truncate text-[11px] text-ink-4">{t.description}</span>}
                </div>
                <Toggle
                  on={t.enabled}
                  label={t.name}
                  onChange={async (v) => {
                    const r = await autosave(() => mcpToolEnable(s.name, t.name, v));
                    if (r) onServers(r.servers);
                  }}
                />
              </div>
            ))}
          </div>
        </div>
      </div>
    </div>
  );
}

// ---- 添加 / 编辑 ----

/** 键值行编辑器（环境变量、请求头）。 */
function KeyValues({
  value,
  onChange,
  keyPlaceholder,
  valuePlaceholder,
  secret,
}: {
  value: [string, string][];
  onChange: (v: [string, string][]) => void;
  keyPlaceholder: string;
  valuePlaceholder: string;
  secret?: boolean;
}) {
  return (
    <div className="flex flex-col gap-1.5">
      {value.map(([k, v], i) => (
        <div key={i} className="flex items-center gap-1.5">
          <TextInput value={k} onChange={(x) => onChange(value.map((r, j) => (j === i ? [x, r[1]] : r)))} placeholder={keyPlaceholder} mono width={180} />
          <TextInput
            value={v}
            onChange={(x) => onChange(value.map((r, j) => (j === i ? [r[0], x] : r)))}
            placeholder={valuePlaceholder}
            mono
            width={220}
            type={secret ? "password" : "text"}
          />
          <button
            type="button"
            aria-label="删除这一行"
            onClick={() => onChange(value.filter((_, j) => j !== i))}
            className="rounded-md p-1 text-ink-4 hover:bg-hover hover:text-ink"
          >
            <X size={13} />
          </button>
        </div>
      ))}
      <button type="button" onClick={() => onChange([...value, ["", ""]])} className="self-start text-[12px] font-medium text-accent">
        + 添加
      </button>
    </div>
  );
}

function McpEditor({
  initial,
  originalName,
  taken,
  onDone,
}: {
  initial: McpEntry;
  originalName: string | null;
  taken: string[];
  onDone: (servers: McpServerInfo[] | null) => void;
}) {
  const [e, setE] = useState(initial);
  const [args, setArgs] = useState<string[]>(initial.args.length ? initial.args : [""]);
  const [env, setEnv] = useState<[string, string][]>(Object.entries(initial.env));
  const [headers, setHeaders] = useState<[string, string][]>(Object.entries(initial.headers));
  const [oauth, setOauth] = useState(!!(initial.oauthClientId || initial.oauthScopes.length));
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const set = <K extends keyof McpEntry>(k: K, v: McpEntry[K]) => setE((x) => ({ ...x, [k]: v }));
  const stdio = e.transport === "stdio";

  const name = e.name.trim();
  const nameError = !name
    ? "名称不能为空"
    : !/^[A-Za-z0-9_-]{1,64}$/.test(name)
      ? "名称只能用字母、数字、- 和 _"
      : taken.includes(name)
        ? `已存在同名服务：${name}`
        : null;
  const targetError = stdio
    ? !e.command.trim() && "命令不能为空"
    : !/^https?:\/\/[^\s/]+/.test(e.url.trim()) && "URL 不是合法地址";
  const [shown, setShown] = useState(false);

  const save = async () => {
    setShown(true);
    if (nameError || targetError) return;
    setSaving(true);
    setError(null);
    const entry: McpEntry = {
      ...e,
      name,
      args: args.map((a) => a.trim()).filter(Boolean),
      env: Object.fromEntries(env.filter(([k]) => k.trim())),
      headers: Object.fromEntries(headers.filter(([k]) => k.trim())),
      oauthClientId: oauth ? e.oauthClientId : null,
      oauthScopes: oauth ? e.oauthScopes : [],
    };
    try {
      const r = await mcpSave(entry, originalName);
      const row = r.servers.find((s) => s.name === name);
      // 保存成功但没连上：留在表单里把原因给人看，免得以为好了。
      if (row && (row.status === "failed" || row.status === "needs_auth")) {
        setNotice(
          row.status === "needs_auth"
            ? `已保存。${name} 需要登录：回到列表点「登录」。`
            : `已保存，但没连上：${row.detail || "Dock 没给原因"}`,
        );
        setSaving(false);
        return;
      }
      onDone(r.servers);
    } catch (err) {
      setError(errorText(err));
      setSaving(false);
    }
  };

  return (
    <Page
      title={originalName ? `编辑 ${originalName}` : "添加 MCP 服务"}
      subtitle="配置本地命令或远程 HTTP 服务；保存后立刻连接"
      back={{ label: "MCP 服务", onClick: () => onDone(null) }}
      banner={
        <>
          {error && <Banner kind="error">{error}</Banner>}
          {notice && (
            <Banner kind="warn" action={<button type="button" onClick={() => onDone(null)} className={smallButton}>回到列表</button>}>
              {notice}
            </Banner>
          )}
        </>
      }
    >
      <Section title="基本信息">
        <Card>
          <Row label="名称" desc={shown && nameError ? <span className="text-[#ff3b30]">{nameError}</span> : "工具名会是 mcp_<名称>__<工具>"}>
            <TextInput value={e.name} onChange={(v) => set("name", v)} mono invalid={shown && !!nameError} placeholder="github" />
          </Row>
          <Row label="类型">
            <Segmented
              value={e.transport}
              options={[
                { value: "stdio", label: "本地命令 stdio" },
                { value: "http", label: "远程 HTTP" },
              ]}
              onChange={(v) => set("transport", v)}
            />
          </Row>
        </Card>
      </Section>

      {stdio ? (
        <Section title="本地命令 stdio">
          <Card>
            <Row label="命令" desc={shown && targetError ? <span className="text-[#ff3b30]">{targetError}</span> : undefined}>
              <TextInput value={e.command} onChange={(v) => set("command", v)} mono invalid={shown && !!targetError} placeholder="npx" />
            </Row>
            <Row label="参数" desc="每行一个" top>
              <div className="flex flex-col gap-1.5">
                {args.map((a, i) => (
                  <div key={i} className="flex items-center gap-1.5">
                    <TextInput value={a} onChange={(v) => setArgs(args.map((x, j) => (j === i ? v : x)))} mono width={300} placeholder="-y" />
                    <button
                      type="button"
                      aria-label="删除这个参数"
                      onClick={() => setArgs(args.length > 1 ? args.filter((_, j) => j !== i) : [""])}
                      className="rounded-md p-1 text-ink-4 hover:bg-hover hover:text-ink"
                    >
                      <X size={13} />
                    </button>
                  </div>
                ))}
                <button type="button" onClick={() => setArgs([...args, ""])} className="self-start text-[12px] font-medium text-accent">
                  + 添加
                </button>
              </div>
            </Row>
            <Row label="环境变量" desc="会写进 config.toml" top>
              <KeyValues value={env} onChange={setEnv} keyPlaceholder="GITHUB_TOKEN" valuePlaceholder="值" secret />
            </Row>
            <Row label="启动超时">
              <TextInput
                value={e.startupTimeoutSec?.toString() ?? ""}
                onChange={(v) => set("startupTimeoutSec", Number(v) > 0 ? Math.round(Number(v)) : null)}
                placeholder="30"
                suffix="秒"
                width={110}
              />
            </Row>
          </Card>
        </Section>
      ) : (
        <Section title="远程 HTTP">
          <Card>
            <Row label="URL" desc={shown && targetError ? <span className="text-[#ff3b30]">{targetError}</span> : undefined}>
              <TextInput value={e.url} onChange={(v) => set("url", v)} mono invalid={shown && !!targetError} placeholder="https://mcp.example.com/mcp" />
            </Row>
            <Row label="Bearer 令牌" desc="从这个环境变量读（不把令牌写进文件）">
              <TextInput
                value={e.bearerTokenEnvVar ?? ""}
                onChange={(v) => set("bearerTokenEnvVar", v.trim() || null)}
                mono
                width={220}
                placeholder="LINEAR_API_KEY"
              />
            </Row>
            <Row label="请求头" top>
              <KeyValues value={headers} onChange={setHeaders} keyPlaceholder="X-Header" valuePlaceholder="值" />
            </Row>
            <Row label="OAuth" desc="服务器要浏览器登录时填；保存后在列表里点「登录」" top>
              <div className="flex flex-col items-end gap-2">
                <Toggle on={oauth} onChange={setOauth} />
                {oauth && (
                  <div className="enter-fade flex flex-col gap-1.5">
                    <TextInput
                      value={e.oauthClientId ?? ""}
                      onChange={(v) => set("oauthClientId", v.trim() || null)}
                      mono
                      placeholder="client id（可选）"
                    />
                    <TextInput
                      value={e.oauthScopes.join(" ")}
                      onChange={(v) => set("oauthScopes", v.split(/[\s,]+/).filter(Boolean))}
                      mono
                      placeholder="scopes，空格分隔"
                    />
                  </div>
                )}
              </div>
            </Row>
            <Row label="启动超时">
              <TextInput
                value={e.startupTimeoutSec?.toString() ?? ""}
                onChange={(v) => set("startupTimeoutSec", Number(v) > 0 ? Math.round(Number(v)) : null)}
                placeholder="30"
                suffix="秒"
                width={110}
              />
            </Row>
          </Card>
        </Section>
      )}

      <div className="flex justify-end gap-2 pb-4">
        <button type="button" onClick={() => onDone(null)} className={secondaryButton}>
          取消
        </button>
        <button type="button" disabled={saving} onClick={save} className={primaryButton}>
          {saving && <LoaderCircle size={13} className="animate-spin" />}
          {saving ? "正在连接…" : "保存并连接"}
        </button>
      </div>
    </Page>
  );
}
