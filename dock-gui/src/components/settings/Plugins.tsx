// 插件与技能（ose / bis / yox）：三个页签。
// - 插件：永久插件（用户级 + 每个项目，按目录认）+ 动态插件（会话里定义、只在内存里）；
// - 技能：每一层发现的技能，被更高层同名遮住的标出来；
// - 密钥：插件用 host.secret 读的 $DOCK_HOME/secrets.json（只列名字，值只进不出）。

import { Fragment, useState, type ReactNode } from "react";
import { usePluginRows, type PluginRowProps } from "../../features/registry";
import { ChevronDown, FolderOpen, LoaderCircle, Plus, Trash2 } from "lucide-react";
import { Popover } from "../Popover";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { useApp } from "../../dock/store";
import {
  errorText,
  pluginDelete,
  pluginDiscard,
  pluginEnable,
  pluginList,
  pluginPromote,
  secretDelete,
  secretList,
  secretSet,
  skillList,
  type DiskPlugin,
  type PluginListing,
  type SessionPlugin,
  type SkillRow,
} from "../../dock/settingsApi";
import {
  Card,
  ErrorLine,
  Loading,
  Modal,
  Page,
  Pill,
  Section,
  Tabs,
  TextInput,
  Toggle,
  autosave,
  primaryButton,
  secondaryButton,
  smallButton,
  useLoad,
} from "./ui";

type Tab = "plugins" | "skills" | "secrets";

export function PluginsPane({ initialTab = "plugins" }: { initialTab?: Tab } = {}) {
  const [tab, setTab] = useState<Tab>(initialTab);
  return (
    <Page title="插件与技能" subtitle="Cordis 动态插件、斜杠技能与插件密钥">
      <Tabs
        value={tab}
        onChange={setTab}
        tabs={[
          { value: "plugins", label: "插件" },
          { value: "skills", label: "技能" },
          { value: "secrets", label: "密钥" },
        ]}
      />
      <div key={tab} className="enter-fade flex flex-col gap-5">
        {tab === "plugins" ? <PluginsTab /> : tab === "skills" ? <SkillsTab /> : <SecretsTab />}
      </div>
    </Page>
  );
}

const rowClass = "group flex items-center justify-between gap-3 border-b border-[#f0f0f3] px-4 py-3 last:border-b-0";
/** 行本身（分隔线在外层，下面还要挂设置卡的展开区）。 */
const rowInner = "group flex items-center justify-between gap-3 px-4 py-3";
const tilde = (p: string) => p.replace(/^\/Users\/[^/]+/, "~");
const baseName = (p: string) => p.replace(/\/+$/, "").split("/").pop() || p;
const dangerButton = "motion-colors flex h-7 items-center rounded-md bg-[#ff3b30] px-3 text-[11.5px] font-medium text-white hover:bg-[#e0352b]";

/** 一个永久插件现在是什么状态：只看 Dock 给的字段，不猜。 */
function diskState(p: DiskPlugin): { label: string; kind: "green" | "gray" | "orange" | "red"; title?: string } {
  if (p.error) return { label: "加载失败", kind: "red" };
  if (p.shadowedBy) return { label: "被同名插件占用", kind: "orange", title: `已经装着 ${tilde(p.shadowedBy)} 的同名插件` };
  if (!p.enabled) return { label: "已停用", kind: "gray" };
  if (p.running) return { label: "运行中", kind: "green" };
  if (p.loaded) return { label: "已停止", kind: "gray" };
  return { label: "未加载", kind: "gray", title: "打开这个项目的会话时自动加载；也可以现在就加载" };
}

function PluginsTab() {
  const list = useLoad(pluginList);
  const [busy, setBusy] = useState<string | null>(null);
  const act = async (key: string, f: () => Promise<PluginListing>) => {
    setBusy(key);
    const r = await autosave(f);
    if (r) list.set(r);
    setBusy(null);
  };
  if (list.loading && !list.data) return <Loading />;
  if (list.error) return <ErrorLine>读不到插件：{list.error}</ErrorLine>;
  const data = list.data!;
  const user = data.disk.filter((p) => p.scope === "user");
  const byProject = new Map<string, DiskPlugin[]>();
  for (const p of data.disk.filter((x) => x.scope === "project" && x.project)) {
    byProject.set(p.project!, [...(byProject.get(p.project!) ?? []), p]);
  }

  return (
    <>
      <Section
        title="永久插件"
        right={<span className="text-[11.5px] text-ink-4">写在磁盘上，启动 / 打开项目的会话时自动加载</span>}
      >
        <div className="flex flex-col gap-3">
          <DiskGroup
            title="用户级"
            root={data.userRoot}
            rows={user}
            empty="还没有。在会话里让 agent 写一个并 cordis_promote，或把动态插件「保存为永久」。"
            busy={busy}
            act={act}
          />
          {[...byProject].map(([project, rows]) => (
            <DiskGroup
              key={project}
              title={`项目 · ${baseName(project)}`}
              root={rows[0].root}
              rows={rows}
              busy={busy}
              act={act}
            />
          ))}
          {byProject.size === 0 && (
            <span className="text-[12px] text-ink-4">
              {data.projects.length} 个项目里都没有 .dock/plugins。项目插件放在 &lt;项目&gt;/.dock/plugins/&lt;id&gt;/。
            </span>
          )}
        </div>
      </Section>

      <Section
        title={`动态插件${data.session.length ? ` (${data.session.length})` : ""}`}
        right={<span className="text-[11.5px] text-ink-4">会话里临时定义，只在内存里，Dock 重启就没了</span>}
      >
        <Card>
          {data.session.length === 0 && (
            <div className="px-4 py-3 text-[12.5px] text-ink-4">没有。agent 在会话里用 cordis_define 定义的插件会先出现在这里。</div>
          )}
          {data.session.map((p) => (
            <SessionRow key={p.id} plugin={p} busy={busy === p.id} act={act} />
          ))}
        </Card>
      </Section>
    </>
  );
}

function DiskGroup({
  title,
  root,
  rows,
  empty,
  busy,
  act,
}: {
  title: string;
  root: string;
  rows: DiskPlugin[];
  empty?: string;
  busy: string | null;
  act: (key: string, f: () => Promise<PluginListing>) => Promise<void>;
}) {
  // 连远程 Dock 时插件目录在远端，本机访达打不开。
  const remote = useApp().remote;
  const [confirm, setConfirm] = useState<string | null>(null);
  return (
    <div className="flex flex-col gap-1.5">
      <div className="flex items-center justify-between gap-2">
        <span className="min-w-0 truncate text-[12px] text-ink-3">
          <span className="font-medium text-ink-2">{title}</span>
          <span className="ml-2 font-mono text-[11px] text-ink-4">{tilde(root)}</span>
        </span>
        {!remote && (
          <button type="button" onClick={() => revealItemInDir(root).catch(() => {})} className="shrink-0 text-[11.5px] text-ink-4 hover:text-ink-2">
            在访达中显示
          </button>
        )}
      </div>
      <Card>
        {rows.length === 0 && <div className="px-4 py-3 text-[12.5px] text-ink-4">{empty}</div>}
        {rows.map((p) => {
          const st = diskState(p);
          const canLoad = p.enabled && !p.loaded && !p.shadowedBy && !p.error;
          return (
            <PluginRowSlot key={p.path} pluginId={p.id}>
              {({ action, panel }) => (
                <div className="flex flex-col border-b border-[#f0f0f3] last:border-b-0">
                  <div className="group flex items-center justify-between gap-3 px-4 py-3">
                    <div className="flex min-w-0 flex-col gap-0.5">
                      <div className="flex flex-wrap items-center gap-2">
                        <span className="font-mono text-[12.5px] text-ink">{p.id}</span>
                        {p.name && p.name !== p.id && <span className="text-[12.5px] text-ink-2">{p.name}</span>}
                        <Pill kind={st.kind} title={st.title}>
                          {st.label}
                        </Pill>
                      </div>
                      {p.purpose && <span className="truncate text-xs text-ink-4">{p.purpose}</span>}
                    </div>
                    <div className="flex shrink-0 items-center gap-2">
                      {action}
                      {confirm === p.path ? (
                        <span className="enter-fade flex items-center gap-1.5">
                          <span className="text-[12px] text-ink-3">删除这个目录？</span>
                          <button type="button" onClick={() => setConfirm(null)} className={smallButton}>
                            取消
                          </button>
                          <button
                            type="button"
                            onClick={() => {
                              setConfirm(null);
                              void act(p.path, () => pluginDelete(p.path));
                            }}
                            className={dangerButton}
                          >
                            删除
                          </button>
                        </span>
                      ) : (
                        <span className="flex gap-1 opacity-0 transition-opacity group-focus-within:opacity-100 group-hover:opacity-100">
                          {!remote && (
                            <button
                              type="button"
                              aria-label="在访达中显示"
                              title="在访达中显示"
                              onClick={() => revealItemInDir(p.path).catch(() => {})}
                              className="rounded-md p-1.5 text-ink-4 hover:bg-hover hover:text-ink"
                            >
                              <FolderOpen size={13} />
                            </button>
                          )}
                          <button
                            type="button"
                            aria-label="删除"
                            title="删除"
                            onClick={() => setConfirm(p.path)}
                            className="rounded-md p-1.5 text-ink-4 hover:bg-[#ff3b3014] hover:text-[#ff3b30]"
                          >
                            <Trash2 size={13} />
                          </button>
                        </span>
                      )}
                      {canLoad && (
                        <button
                          type="button"
                          disabled={busy === p.path}
                          onClick={() => act(p.path, () => pluginEnable(p.path, true))}
                          className={smallButton}
                          title="现在就装上并运行（不用等打开这个项目的会话）"
                        >
                          {busy === p.path && <LoaderCircle size={11} className="animate-spin" />}
                          立即加载
                        </button>
                      )}
                      <Toggle
                        on={p.enabled}
                        label={p.enabled ? "停用" : "启用"}
                        disabled={busy === p.path || (!!p.error && !p.purpose)}
                        onChange={(v) => act(p.path, () => pluginEnable(p.path, v))}
                      />
                    </div>
                  </div>
                  {panel}
                  {p.error && <div className="border-t border-[#fee2e2] bg-[#fef2f2] px-4 py-2 text-xs break-words text-[#d70015]">{p.error}</div>}
                </div>
              )}
            </PluginRowSlot>
          );
        })}
      </Card>
    </div>
  );
}

function SessionRow({
  plugin: p,
  busy,
  act,
}: {
  plugin: SessionPlugin;
  busy: boolean;
  act: (key: string, f: () => Promise<PluginListing>) => Promise<void>;
}) {
  const [confirm, setConfirm] = useState(false);
  const from = p.threadId ? `来自「${p.threadTitle || "未命名会话"}」${p.cwd ? ` · ${baseName(p.cwd)}` : ""}` : "定义它的会话已经关了";
  const item = "motion-colors flex w-full flex-col items-start rounded-md px-2.5 py-1.5 text-left hover:bg-hover";
  return (
    <PluginRowSlot pluginId={p.id}>
      {({ action, panel }) => (
        <div className="flex flex-col border-b border-[#f0f0f3] last:border-b-0">
          <div className={rowInner}>
            <div className="flex min-w-0 flex-col gap-0.5">
              <div className="flex flex-wrap items-center gap-2">
                <span className="font-mono text-[12.5px] text-ink">{p.id}</span>
                {p.name && p.name !== p.id && <span className="text-[12.5px] text-ink-2">{p.name}</span>}
                {p.factory && <Pill>{p.factory}</Pill>}
                {p.error ? <Pill kind="red">启动失败</Pill> : <Pill kind={p.running ? "green" : "gray"}>{p.running ? "运行中" : "已停止"}</Pill>}
              </div>
              {p.purpose && <span className="truncate text-xs text-ink-4">{p.purpose}</span>}
              <span className="truncate text-[11px] text-ink-4">{from}</span>
              {p.error && <span className="text-[11px] break-words text-[#d70015]">{p.error}</span>}
            </div>
            <div className="flex shrink-0 items-center gap-1.5">
              {action}
              {confirm ? (
                <span className="enter-fade flex items-center gap-1.5">
                  <span className="text-[12px] text-ink-3">停掉并丢掉？</span>
                  <button type="button" onClick={() => setConfirm(false)} className={smallButton}>
                    取消
                  </button>
                  <button
                    type="button"
                    onClick={() => {
                      setConfirm(false);
                      void act(p.id, () => pluginDiscard(p.id));
                    }}
                    className={dangerButton}
                  >
                    丢弃
                  </button>
                </span>
              ) : (
                <>
                  <Popover
                    title="保存为永久"
                    side="bottom"
                    align="right"
                    className={smallButton}
                    panelClassName="min-w-[220px] px-1"
                    trigger={
                      <>
                        {busy && <LoaderCircle size={11} className="animate-spin" />}
                        保存为永久
                        <ChevronDown size={11} />
                      </>
                    }
                  >
                    {(close) => (
                      <>
                        <button
                          type="button"
                          className={item}
                          onClick={() => {
                            close();
                            void act(p.id, () => pluginPromote(p.id, "user"));
                          }}
                        >
                          <span className="text-[12.5px] text-ink">用户级</span>
                          <span className="text-[11px] text-ink-4">~/.dock/plugins，所有项目都加载</span>
                        </button>
                        <button
                          type="button"
                          disabled={!p.cwd}
                          className={`${item} disabled:opacity-50`}
                          onClick={() => {
                            close();
                            void act(p.id, () => pluginPromote(p.id, "project"));
                          }}
                        >
                          <span className="text-[12.5px] text-ink">这个项目{p.cwd ? `（${baseName(p.cwd)}）` : ""}</span>
                          <span className="text-[11px] text-ink-4">
                            {p.cwd ? `${tilde(p.cwd)}/.dock/plugins，只跟着这个项目` : "会话关了，不知道是哪个项目"}
                          </span>
                        </button>
                      </>
                    )}
                  </Popover>
                  <button type="button" onClick={() => setConfirm(true)} className="rounded-md px-2 py-1 text-[12px] font-medium text-[#ff3b30] hover:bg-[#ff3b3014]">
                    丢弃
                  </button>
                </>
              )}
            </div>
          </div>
          {panel}
        </div>
      )}
    </PluginRowSlot>
  );
}

/**
 * 功能登记的插件行扩展（插件设置卡的「配置」+ 展开区），见 features/registry.ts。
 * 多个登记就一层层套：操作按登记顺序排在原有操作左边，展开区依次排在行下面。
 */
function PluginRowSlot({ pluginId, children }: PluginRowProps) {
  const rows = usePluginRows();
  const render = (i: number, action: ReactNode[], panel: ReactNode[]): ReactNode => {
    if (i === rows.length) return children({ action: <>{action}</>, panel: <>{panel}</> });
    const Row = rows[i];
    return (
      <Row pluginId={pluginId}>
        {(parts) => render(i + 1, [...action, <Fragment key={i}>{parts.action}</Fragment>], [...panel, <Fragment key={i}>{parts.panel}</Fragment>])}
      </Row>
    );
  };
  return <>{render(0, [], [])}</>;
}

function SkillsTab() {
  const list = useLoad(skillList);
  if (list.loading && !list.data) return <Loading />;
  if (list.error) return <ErrorLine>读不到技能：{list.error}</ErrorLine>;
  const skills = list.data ?? [];
  const global = skills.filter((s) => !s.project);
  const byProject = new Map<string, SkillRow[]>();
  for (const s of skills.filter((x) => x.project)) byProject.set(s.project!, [...(byProject.get(s.project!) ?? []), s]);
  return (
    <>
      <Section title="内置与用户级" right={<span className="text-[11.5px] text-ink-4">所有项目都看得到</span>}>
        <SkillCard rows={global} empty="还没有。放一个 SKILL.md 到 ~/.dock/skills/<名字>/。" />
      </Section>
      {[...byProject].map(([project, rows]) => (
        <Section key={project} title={`项目 · ${baseName(project)}`} right={<span className="font-mono text-[11px] text-ink-4">{tilde(project)}</span>}>
          <SkillCard rows={rows} />
        </Section>
      ))}
      {byProject.size === 0 && (
        <span className="text-[12px] text-ink-4">打开过的项目里都没有项目技能（skills/、.agents/skills/、.dock/skills/）。</span>
      )}
    </>
  );
}

function SkillCard({ rows, empty }: { rows: SkillRow[]; empty?: string }) {
  const remote = useApp().remote;
  return (
    <Card>
      {rows.length === 0 && <div className="px-4 py-3 text-[12.5px] text-ink-4">{empty}</div>}
      {rows.map((s) => {
        const hidden = s.shadowed;
        return (
          <div key={s.path} className={`${rowClass} ${hidden ? "bg-[#fff8ec]" : ""}`}>
            <div className="flex min-w-0 flex-col gap-0.5">
              <div className="flex flex-wrap items-center gap-2">
                <span className={`font-mono text-[12.5px] ${hidden ? "text-ink-4" : "text-ink"}`}>/{s.name}</span>
                {hidden && <Pill kind="orange">被同名技能遮住</Pill>}
                {!hidden && s.shadowedIn.length > 0 && (
                  <Pill kind="orange" title={s.shadowedIn.map(tilde).join("\n")}>
                    在 {s.shadowedIn.map(baseName).join("、")} 里被项目技能盖住
                  </Pill>
                )}
                {!s.userInvocable && <Pill title="只能由 agent 用 skill 工具加载">不可手动调用</Pill>}
              </div>
              {s.description && <span className="truncate text-xs text-ink-4">{s.description}</span>}
              <span className="truncate font-mono text-[11px] text-ink-5">{tilde(s.path)}</span>
            </div>
            <div className="flex shrink-0 items-center gap-2">
              <Pill>{s.scope}</Pill>
              {!remote && (
                <button
                  type="button"
                  aria-label="在访达中显示"
                  onClick={() => revealItemInDir(s.path).catch(() => {})}
                  className="rounded-md p-1.5 text-ink-4 opacity-0 hover:bg-hover hover:text-ink group-hover:opacity-100"
                >
                  <FolderOpen size={13} />
                </button>
              )}
            </div>
          </div>
        );
      })}
    </Card>
  );
}

function SecretsTab() {
  const list = useLoad(secretList);
  const [adding, setAdding] = useState(false);
  const [confirm, setConfirm] = useState<string | null>(null);
  if (list.loading && !list.data) return <Loading />;
  if (list.error) return <ErrorLine>读不到 secrets.json：{list.error}</ErrorLine>;
  const names = list.data?.names ?? [];
  return (
    <>
      <Section
        title={`密钥列表 · ${list.data?.path.replace(/^\/Users\/[^/]+/, "~") ?? ""}`}
        right={
          <button type="button" onClick={() => setAdding(true)} className={primaryButton}>
            <Plus size={13} />
            添加密钥
          </button>
        }
      >
        <Card>
          {names.length === 0 && <div className="px-4 py-3 text-[12.5px] text-ink-4">还没有。插件里 host.secret("名字") 读的就是这里。</div>}
          {names.map((n) => (
            <div key={n} className={rowClass}>
              <div className="flex flex-col gap-0.5">
                <span className="font-mono text-[12.5px] text-ink">{n}</span>
                <span className="font-mono text-xs tracking-widest text-ink-4">••••••••••••</span>
              </div>
              {confirm === n ? (
                <span className="enter-fade flex items-center gap-1.5">
                  <button type="button" onClick={() => setConfirm(null)} className={smallButton}>
                    取消
                  </button>
                  <button
                    type="button"
                    onClick={async () => {
                      setConfirm(null);
                      const r = await autosave(() => secretDelete(n));
                      if (r) list.set(r);
                    }}
                    className="motion-colors flex h-7 items-center rounded-md bg-[#ff3b30] px-3 text-[11.5px] font-medium text-white"
                  >
                    删除
                  </button>
                </span>
              ) : (
                <button type="button" onClick={() => setConfirm(n)} className="rounded-md px-2 py-1 text-[12px] font-medium text-[#ff3b30] hover:bg-[#ff3b3014]">
                  删除
                </button>
              )}
            </div>
          ))}
        </Card>
      </Section>
      <span className="text-[12px] text-ink-4">也可以用环境变量 DOCK_SECRET_&lt;名字&gt;（名字转大写，非字母数字换成 _）</span>
      {adding && (
        <AddSecret
          onClose={() => setAdding(false)}
          onSaved={(r) => {
            list.set(r);
            setAdding(false);
          }}
        />
      )}
    </>
  );
}

function AddSecret({ onClose, onSaved }: { onClose: () => void; onSaved: (r: { names: string[]; path: string }) => void }) {
  const [name, setName] = useState("");
  const [value, setValue] = useState("");
  const [error, setError] = useState<string | null>(null);
  const valid = /^[A-Za-z][A-Za-z0-9_-]{0,63}$/.test(name);
  return (
    <Modal onClose={onClose}>
      <span className="text-base font-semibold text-ink">添加密钥</span>
      <div className="flex flex-col gap-2">
        <TextInput value={name} onChange={setName} placeholder="名字，如 github" mono width="100%" autoFocus invalid={!!name && !valid} />
        <TextInput value={value} onChange={setValue} placeholder="值" type="password" mono width="100%" />
        {name && !valid && <span className="text-xs text-[#ff3b30]">字母开头，只能用字母、数字、- 和 _</span>}
        {error && <span className="text-xs text-[#ff3b30]">{error}</span>}
      </div>
      <div className="flex justify-end gap-2">
        <button type="button" onClick={onClose} className={secondaryButton}>
          取消
        </button>
        <button
          type="button"
          disabled={!valid || !value}
          onClick={async () => {
            try {
              onSaved(await secretSet(name, value));
            } catch (e) {
              setError(errorText(e));
            }
          }}
          className={primaryButton}
        >
          保存
        </button>
      </div>
    </Modal>
  );
}
