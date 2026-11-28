// 沙箱：`~/.dock/config.toml` 的 `[sandbox]`（docs/SANDBOX.md「配置」）。
// 单个会话的开关在会话里（行为轨迹面板、/sandbox on|off）；这里管新开的会话默认开不开、
// 审计记多少、限制多严、策略规则。规则和限制每次现读，改了马上生效；
// 审计的三项在出口代理第一次起时读，改了要重启 Dock。

import { useState } from "react";
import { ArrowDown, ArrowUp, Plus, RotateCw, X } from "lucide-react";
import { store, useApp } from "../../dock/store";
import { imeComposing } from "../../dock/ime";
import { canTrace, sandboxStatus } from "../../features/sandbox-trace/api";
import { useConfigKeys } from "./configKeys";
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
  Select,
  TextInput,
  Toggle,
  smallButton,
  useLoad,
} from "./ui";

/** 出口代理起来时才读的键：改了提示重启。 */
const RESTART_KEYS = ["sandbox.decrypt", "sandbox.body_cap_kb", "sandbox.redact_secrets"];
/** 和 `load_sandbox_config` 的上限一样。 */
const BODY_CAP_MAX_KB = 64 * 1024;

type Network = "allow" | "proxy" | "off";
type Action = "allow" | "ask" | "deny";

/** config/get 回的一条规则：原样（写错的也在，`action` 可能不认识）。 */
interface RuleRow {
  host: string | null;
  command: string | null;
  action: string | null;
}

const ACTIONS: { value: Action; label: string }[] = [
  { value: "allow", label: "放行" },
  { value: "ask", label: "询问" },
  { value: "deny", label: "阻断" },
];

const valid = (r: RuleRow) => !!r.host !== !!r.command && ACTIONS.some((a) => a.value === r.action);

export function SandboxPane() {
  const c = useConfigKeys();
  const app = useApp();
  const status = useLoad(() => (canTrace() ? sandboxStatus() : Promise.resolve(null)));
  const [restartNote, setRestartNote] = useState(false);
  const [restarting, setRestarting] = useState(false);
  if (c.loading) return <Loading />;

  const write = async (key: string, value: unknown) => {
    const r = await c.write(key, value);
    if (r && RESTART_KEYS.includes(key)) setRestartNote(true);
    return r;
  };
  /** 默认开的开关：开 = 删掉键回默认，关 = 写 false（配置文件里只留改过的）。 */
  const onByDefault = (key: string) => ({
    on: c.get<boolean>(key) ?? true,
    onChange: (v: boolean) => void write(key, v ? null : false),
  });
  /** 默认关的开关。 */
  const offByDefault = (key: string) => ({
    on: c.get<boolean>(key) ?? false,
    onChange: (v: boolean) => void write(key, v ? true : null),
  });

  const confine = c.get<boolean>("sandbox.confine") ?? false;
  const network = c.get<Network>("sandbox.network") ?? "allow";
  const redact = c.get<boolean>("sandbox.redact_secrets") ?? false;
  const s = status.data;
  // 老 Dock（或 GUI 更新了、Dock 进程还没重启）的白名单里没有 `sandbox.*`：写一个就报
  // 「设置页不能改 …」。先看 config/get 回没回这些键，没回就整页只读、提示重启。
  const supported = !c.data || "sandbox.confine" in c.data.values;
  const linux = s?.backend === "landlock";
  const networkDesc = !confine
    ? "开了「命令进系统沙箱」才生效"
    : network === "allow"
      ? "照常出网：走审计代理的记下来，直连的看不到"
      : network === "proxy"
        ? `只能经审计代理出网，一条不漏；本机的开发服务器照常连${linux ? "（Linux 上内核暂不强制挡直连）" : ""}`
        : "命令不能联网";

  return (
    <Page
      title="沙箱"
      subtitle="看得见、管得住 agent 做的事，但不减它能做的事"
      banner={
        !supported ? (
          <Banner
            kind="warn"
            action={
              !app.remote && (
                <button
                  type="button"
                  disabled={restarting}
                  onClick={async () => {
                    setRestarting(true);
                    await store.restartDock();
                    window.setTimeout(() => setRestarting(false), 3000);
                  }}
                  className="flex items-center gap-1 text-[12px] font-medium text-[#8a5a00] disabled:opacity-50"
                >
                  <RotateCw size={12} className={restarting ? "animate-spin" : ""} />
                  重启 Dock
                </button>
              )
            }
          >
            连着的 Dock 还不认沙箱设置（版本旧，或更新后还没重启），这一页先只读
          </Banner>
        ) : (
          restartNote && (
          <Banner
            kind="info"
            action={
              <div className="flex items-center gap-2">
                {!app.remote && (
                  <button
                    type="button"
                    disabled={restarting}
                    onClick={async () => {
                      setRestarting(true);
                      await store.restartDock();
                      setRestartNote(false);
                      window.setTimeout(() => setRestarting(false), 3000);
                    }}
                    className="flex items-center gap-1 text-[12px] font-medium text-[#0040dd] disabled:opacity-50"
                  >
                    <RotateCw size={12} className={restarting ? "animate-spin" : ""} />
                    重启 Dock
                  </button>
                )}
                <button type="button" onClick={() => setRestartNote(false)} className="text-[12px] text-[#0040dd]">
                  知道了
                </button>
              </div>
            }
          >
            审计的设置在重启 Dock 之后生效（出口代理起来时读）
          </Banner>
          )
        )
      }
    >
      {c.error && <ErrorLine>读不到配置：{c.error}</ErrorLine>}
      <fieldset disabled={!supported} className={`contents ${supported ? "" : "[&>section]:pointer-events-none [&>section]:opacity-50"}`}>

      <Section title="默认">
        <Card>
          <Row
            label="新开的会话默认开沙箱"
            desc="已开的会话不变；单个会话在「行为轨迹」面板里切，或用 /sandbox on|off"
          >
            <Toggle {...offByDefault("sandbox.enabled")} label="新开的会话默认开沙箱" />
          </Row>
        </Card>
      </Section>

      <Section title="审计" right={<span className="text-[11px] text-ink-4">改了重启 Dock 生效</span>}>
        <Card>
          <Row
            label="解密 HTTPS"
            desc="命令的 HTTPS 在审计代理处用 Dock 自己的根证书解开，记下请求包和响应包；关了只记目的地和字节数"
          >
            <Toggle {...onByDefault("sandbox.decrypt")} label="解密 HTTPS" />
          </Row>
          <Row label="正文最多记" desc={`每个请求 / 响应的正文，超出的只记长度；最多 ${BODY_CAP_MAX_KB} KB`}>
            <TextInput
              value={c.get<number>("sandbox.body_cap_kb")?.toString() ?? ""}
              onCommit={(v) => {
                const n = Number(v.trim());
                const ok = v.trim() !== "" && Number.isFinite(n) && n >= 0;
                void write("sandbox.body_cap_kb", ok ? Math.min(Math.round(n), BODY_CAP_MAX_KB) : null);
              }}
              placeholder="256"
              suffix="KB"
              width={110}
            />
          </Row>
          <Row
            label="遮住凭据头"
            desc={
              redact
                ? "Authorization、Cookie 这类头只留长度和哈希，看不到原文"
                : "关着（默认）：头原样记，HTTP 历史里看得到真实的请求；日志里因此有凭据，导出前留意"
            }
          >
            <Toggle {...offByDefault("sandbox.redact_secrets")} label="遮住凭据头" />
          </Row>
        </Card>
      </Section>

      <Section title="限制">
        <Card>
          {s !== undefined && s !== null && (
            <Row
              label="系统沙箱"
              desc={
                s.backend
                  ? s.limits?.length
                    ? `这台机器上做不到：${s.limits.join("；")}`
                    : undefined
                  : `${s.backendReason ?? "本机不可用"}。开了限制也不起作用，命令照常走权限确认`
              }
            >
              {s.backend ? <Pill kind="green">可用 · {s.backend}</Pill> : <Pill kind="red">不可用</Pill>}
            </Row>
          )}
          <Row
            label="命令进系统沙箱"
            desc="bash / monitor 的命令只能写工作区、读不到 ~/.ssh 这类密钥；在沙箱里跑的命令不再弹权限框"
          >
            <Toggle {...offByDefault("sandbox.confine")} label="命令进系统沙箱" />
          </Row>
          <Row label="网络" desc={networkDesc} disabled={!confine}>
            <Segmented<Network>
              value={network}
              disabled={!confine}
              options={[
                { value: "allow", label: "照常" },
                { value: "proxy", label: "只经审计代理" },
                { value: "off", label: "不联网" },
              ]}
              onChange={(v) => void write("sandbox.network", v === "allow" ? null : v)}
            />
          </Row>
          <Row
            label="依赖缓存可写"
            desc="~/.cargo/registry、~/.npm、~/.cache 等：装依赖、构建要写它们"
            disabled={!confine}
          >
            <Toggle {...onByDefault("sandbox.build_caches")} disabled={!confine} label="依赖缓存可写" />
          </Row>
          <Row label="额外可写的目录" desc="绝对路径或 ~/ 开头；工作区本来就能写" top disabled={!confine}>
            <Chips
              values={c.get<string[]>("sandbox.writable_roots") ?? []}
              onChange={(v) => void write("sandbox.writable_roots", v.length ? v : null)}
              check={pathError}
              placeholder="~/build"
              disabled={!confine}
            />
          </Row>
          <Row label="额外读不到的路径" desc="比如存了 token 的 ~/.npmrc" top disabled={!confine}>
            <Chips
              values={c.get<string[]>("sandbox.deny_read") ?? []}
              onChange={(v) => void write("sandbox.deny_read", v.length ? v : null)}
              check={pathError}
              placeholder="~/.npmrc"
              disabled={!confine}
            />
          </Row>
          <Row
            label="保留的环境变量"
            desc="名字带 KEY / TOKEN / SECRET / PASSWORD 的变量默认不传给命令；要留下的写在这里"
            top
            disabled={!confine}
          >
            <Chips
              values={c.get<string[]>("sandbox.keep_env") ?? []}
              onChange={(v) => void write("sandbox.keep_env", v.length ? v : null)}
              check={(v) => (/^[A-Za-z_][A-Za-z0-9_]*$/.test(v) ? null : `${v} 不是环境变量名`)}
              placeholder="GITHUB_TOKEN"
              disabled={!confine}
            />
          </Row>
        </Card>
      </Section>

      <Rules rows={c.get<RuleRow[]>("sandbox.rules") ?? []} onChange={(rows) => void write("sandbox.rules", rows.length ? rows : null)} />
      </fieldset>
    </Page>
  );
}

const pathError = (v: string) =>
  v === "~" || v.startsWith("~/") || v.startsWith("/") ? null : `${v} 不是绝对路径（也可以 ~/ 开头）`;

/** 一串值的小胶囊 + 输入框（回车 / 逗号 / 失焦加一个）。 */
function Chips({
  values,
  onChange,
  check,
  placeholder,
  disabled,
}: {
  values: string[];
  onChange: (next: string[]) => void;
  /** 不对就回一句为什么，不加。 */
  check: (v: string) => string | null;
  placeholder: string;
  disabled?: boolean;
}) {
  const [draft, setDraft] = useState("");
  const [error, setError] = useState<string | null>(null);
  const add = () => {
    const v = draft.trim();
    if (!v) return setError(null);
    const why = check(v);
    if (why) return setError(why);
    setDraft("");
    setError(null);
    if (!values.includes(v)) onChange([...values, v]);
  };
  return (
    <div className="flex w-[300px] flex-col items-end gap-1">
      <div className="flex flex-wrap items-center justify-end gap-1.5">
        {values.map((v) => (
          <span key={v} className="flex items-center gap-1 rounded-full bg-[#f2f2f7] py-0.5 pl-2 pr-1 font-mono text-[11px] text-ink-2">
            {v}
            <button
              type="button"
              aria-label={`移除 ${v}`}
              disabled={disabled}
              onClick={() => onChange(values.filter((x) => x !== v))}
              className="rounded-full p-0.5 text-ink-4 hover:bg-hover hover:text-ink disabled:pointer-events-none"
            >
              <X size={10} />
            </button>
          </span>
        ))}
        <input
          value={draft}
          disabled={disabled}
          spellCheck={false}
          onChange={(e) => (setDraft(e.target.value), setError(null))}
          onKeyDown={(e) => {
            if ((e.key === "Enter" || e.key === ",") && !imeComposing(e)) {
              e.preventDefault();
              add();
            }
          }}
          onBlur={add}
          placeholder={placeholder}
          className={`h-7 w-[140px] rounded-lg border bg-white px-2 font-mono text-[11.5px] outline-none focus:border-accent disabled:bg-window ${
            error ? "border-[#ff3b30]" : "border-line-strong"
          }`}
        />
      </div>
      {error && <span className="enter-fade text-[11px] text-[#d70015]">{error}</span>}
    </div>
  );
}

/** 策略规则：按顺序第一条命中的算，所以能上下挪。每改一下就整份写回。 */
function Rules({ rows, onChange }: { rows: RuleRow[]; onChange: (rows: RuleRow[]) => void }) {
  const [kind, setKind] = useState<"host" | "command">("host");
  const [pattern, setPattern] = useState("");
  const [action, setAction] = useState<Action>("deny");

  // 写回时只留这条用得上的字段；写错的原样交回去，Dock 会说是第几条（界面上也标着）。
  const out = (r: RuleRow): RuleRow => {
    if (!valid(r)) return r;
    return r.host ? { host: r.host, command: null, action: r.action } : { host: null, command: r.command, action: r.action };
  };
  const commit = (next: RuleRow[]) => onChange(next.map(out));
  const set = (i: number, patch: Partial<RuleRow>) => commit(rows.map((r, j) => (j === i ? { ...r, ...patch } : r)));
  const move = (i: number, d: -1 | 1) => {
    const next = [...rows];
    [next[i], next[i + d]] = [next[i + d], next[i]];
    commit(next);
  };
  const add = () => {
    const p = pattern.trim();
    if (!p) return;
    commit([...rows, kind === "host" ? { host: p, command: null, action } : { host: null, command: p, action }]);
    setPattern("");
  };

  return (
    <Section title="策略规则" right={<span className="text-[11px] text-ink-4">按顺序第一条命中的算，都不中就放行</span>}>
      <Card>
        {rows.length === 0 && <Row label="没有规则" desc="全部放行" />}
        {rows.map((r, i) =>
          valid(r) ? (
            <div key={i} className="flex items-center gap-2 border-b border-[#f0f0f3] px-4 py-2.5">
              <span className="w-5 shrink-0 text-right font-mono text-[11px] text-ink-4">{i + 1}</span>
              <Segmented<"host" | "command">
                value={r.host ? "host" : "command"}
                options={[
                  { value: "host", label: "目的地" },
                  { value: "command", label: "命令" },
                ]}
                onChange={(k) => set(i, k === "host" ? { host: r.host ?? r.command, command: null } : { host: null, command: r.command ?? r.host })}
              />
              <div className="min-w-0 flex-1">
                <TextInput
                  value={r.host ?? r.command ?? ""}
                  onCommit={(v) => v.trim() && set(i, r.host ? { host: v.trim() } : { command: v.trim() })}
                  mono
                  width="100%"
                />
              </div>
              <Select value={r.action ?? ""} options={ACTIONS} onChange={(v) => set(i, { action: v })} width={88} />
              <RuleButtons i={i} last={rows.length - 1} move={move} remove={() => commit(rows.filter((_, j) => j !== i))} />
            </div>
          ) : (
            <div key={i} className="flex items-center gap-2 border-b border-[#f0f0f3] px-4 py-2.5">
              <span className="w-5 shrink-0 text-right font-mono text-[11px] text-ink-4">{i + 1}</span>
              <Pill kind="red" title="目的地和命令要恰好写一个，action 只能是 allow / ask / deny">
                不生效
              </Pill>
              <span className="min-w-0 flex-1 truncate font-mono text-[11.5px] text-ink-3">
                {[r.host && `host = ${r.host}`, r.command && `command = ${r.command}`, `action = ${r.action ?? "（没写）"}`].filter(Boolean).join("  ")}
              </span>
              <RuleButtons i={i} last={rows.length - 1} move={move} remove={() => commit(rows.filter((_, j) => j !== i))} />
            </div>
          ),
        )}
        <div className="flex items-center gap-2 bg-[#fafafc] px-4 py-2.5">
          <span className="w-5 shrink-0" />
          <Segmented<"host" | "command">
            value={kind}
            options={[
              { value: "host", label: "目的地" },
              { value: "command", label: "命令" },
            ]}
            onChange={setKind}
          />
          <input
            value={pattern}
            spellCheck={false}
            onChange={(e) => setPattern(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !imeComposing(e)) add();
            }}
            placeholder={kind === "host" ? "*.example.com" : "git push*"}
            className="h-8 min-w-0 flex-1 rounded-lg border border-line-strong bg-white px-2.5 font-mono text-xs shadow-sm outline-none placeholder:text-ink-4 focus:border-accent"
          />
          <Select value={action} options={ACTIONS} onChange={(v) => setAction(v as Action)} width={88} />
          <button type="button" disabled={!pattern.trim()} onClick={add} className={smallButton}>
            <Plus size={12} />
            添加
          </button>
        </div>
      </Card>
      <p className="px-1 text-[11.5px] leading-relaxed text-ink-4">
        目的地管命令经审计代理的出网、web_fetch 和浏览器跳转，<span className="font-mono">*.example.com</span> 也管
        example.com 本身；浏览器页面里的子请求问不了人，只认放行 / 阻断。命令按整条通配匹配 bash / monitor。
        只想这次任务临时放行，用 <span className="font-mono">/sandbox allow</span>。
      </p>
    </Section>
  );
}

function RuleButtons({ i, last, move, remove }: { i: number; last: number; move: (i: number, d: -1 | 1) => void; remove: () => void }) {
  const cls = "motion-colors rounded-md p-1 text-ink-4 hover:bg-hover hover:text-ink disabled:pointer-events-none disabled:opacity-30";
  return (
    <div className="flex shrink-0 items-center">
      <button type="button" aria-label="上移" title="上移" disabled={i === 0} onClick={() => move(i, -1)} className={cls}>
        <ArrowUp size={13} />
      </button>
      <button type="button" aria-label="下移" title="下移" disabled={i === last} onClick={() => move(i, 1)} className={cls}>
        <ArrowDown size={13} />
      </button>
      <button type="button" aria-label="删除这条规则" title="删除" onClick={remove} className={`${cls} hover:text-[#ff3b30]`}>
        <X size={13} />
      </button>
    </div>
  );
}
