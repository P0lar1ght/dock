// 电脑操控 (CUA)：cua-driver 状态机，每个状态一张卡（sms 未安装 / ton 安装中 / guy 缺授权 /
// jog 已连接 / cos 未连上）。安装 / 授权在 Dock 后台跑，这里每秒轮询一次进度。

import { useEffect, useRef, useState } from "react";
import { Check, Circle, LoaderCircle, X } from "lucide-react";
import { store } from "../../dock/store";
import { prefs, usePrefs } from "../../dock/prefs";
import { cuaAction, cuaStatus, errorText, mcpEnable, type CuaStatus } from "../../dock/settingsApi";
import { Card, ErrorLine, Loading, Page, Row, Section, Toggle, flashSaved, primaryButton, secondaryButton } from "./ui";

const statusCard = "flex flex-col gap-3 rounded-xl border bg-white p-4 shadow-sm";

export function CuaPane({ onStatus, onOpenGeneral }: { onStatus: (s: CuaStatus) => void; onOpenGeneral: () => void }) {
  const [s, setS] = useState<CuaStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const p = usePrefs();
  const onStatusRef = useRef(onStatus);
  onStatusRef.current = onStatus;

  const apply = (next: CuaStatus) => {
    setS(next);
    onStatusRef.current(next);
  };
  const load = () =>
    cuaStatus().then(apply, (e) => setError(errorText(e)));

  // 进页面先让 Dock 重探一次（装没装、授没授权），一秒后读结果。
  useEffect(() => {
    cuaAction("refresh").then(apply, () => {}).finally(() => window.setTimeout(load, 1200));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  // 有动作在跑就每秒拉一次进度。
  const running = !!s?.job?.running;
  useEffect(() => {
    if (!running) return;
    const t = window.setInterval(load, 1000);
    return () => window.clearInterval(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [running]);

  const act = async (action: "install" | "grant" | "refresh") => {
    setError(null);
    try {
      apply(await cuaAction(action));
      if (action === "refresh") window.setTimeout(load, 1200);
    } catch (e) {
      setError(errorText(e));
    }
  };
  const toggle = async (on: boolean) => {
    setBusy(true);
    try {
      const r = await mcpEnable("cua-driver", on);
      if (r.error) setError(r.error);
      else flashSaved();
    } catch (e) {
      setError(errorText(e));
    }
    await load();
    setBusy(false);
  };
  const reconnect = async () => {
    setBusy(true);
    setError(null);
    await store.mcpReconnect("cua-driver").catch((e) => setError(errorText(e)));
    await load();
    setBusy(false);
  };

  if (!s) return error ? <ErrorLine>{error}</ErrorLine> : <Loading />;
  const job = s.job;
  const subtitle =
    job?.running ? (job.action === "install" ? "正在安装 cua-driver…" : "正在申请系统权限…") : "cua-driver 状态与权限";

  return (
    <Page title="电脑操控 (CUA)" subtitle={subtitle}>
      {error && <ErrorLine>{error}</ErrorLine>}
      {job && <JobCard job={job} steps={s.installSteps} onDismiss={() => act("refresh")} />}

      {!job?.running && s.state === "missing" && (
        <div className={`${statusCard} border-line-soft`}>
          <span className="text-[15px] font-semibold text-ink">未安装 cua-driver</span>
          <span className="text-[12.5px] text-ink-4">将执行官方脚本 https://cua.ai/driver/install.sh</span>
          <ol className="flex list-decimal flex-col gap-1 pl-5 text-[12.5px] text-ink-2">
            {s.installSteps.map((l) => (
              <li key={l}>{l}</li>
            ))}
          </ol>
          <div className="flex gap-2">
            <button type="button" onClick={() => act("install")} className={primaryButton}>
              安装
            </button>
            <button type="button" onClick={() => act("refresh")} className={secondaryButton}>
              重新检测
            </button>
          </div>
        </div>
      )}

      {!job?.running && s.state === "needs_permission" && (
        <div className={`${statusCard} border-[#ffd8a8]`}>
          <span className="text-[15px] font-semibold text-[#c93400]">缺授权</span>
          <span className="text-[12.5px] text-ink-4">工具能调，但会在系统权限上失败。</span>
          <div className="flex flex-col gap-1.5">
            <PermRow label="辅助功能" ok={!!s.perms?.accessibility} />
            <PermRow label="屏幕录制" ok={!!s.perms?.screenRecording} />
          </div>
          <div className="flex gap-2">
            <button type="button" onClick={() => act("grant")} className={primaryButton}>
              去授权
            </button>
            <button type="button" onClick={() => act("refresh")} className={secondaryButton}>
              重新检测
            </button>
          </div>
        </div>
      )}

      {(s.state === "unreachable" || s.state === "needs_auth") && (
        <div className={`${statusCard} border-[#fecaca] bg-[#fef2f2]`}>
          <span className="flex items-center gap-2 text-[15px] font-semibold text-[#d70015]">
            <span className="size-2 rounded-full bg-[#ff3b30]" />
            未连上
          </span>
          <span className="font-mono text-xs break-words text-[#c0392b]">{s.detail || "Dock 没给原因"}</span>
          <div className="flex gap-2">
            <button type="button" disabled={busy} onClick={reconnect} className={primaryButton}>
              {busy && <LoaderCircle size={13} className="animate-spin" />}
              重新连接
            </button>
            <button type="button" onClick={() => act("refresh")} className={secondaryButton}>
              重新检测
            </button>
          </div>
        </div>
      )}

      {s.state === "not_mounted" && (
        <div className={`${statusCard} border-line-soft text-[13px] text-ink-3`}>这个 Dock 没挂 mcp-client，用不了电脑操控。</div>
      )}

      {(s.state === "connected" || s.state === "disabled" || s.state === "needs_permission") && (
        <>
          {s.state !== "needs_permission" && (
            <div className={`${statusCard} border-line-soft`}>
              <span className="flex items-center gap-2 text-[13px]">
                <span className={`size-2 rounded-full ${s.state === "connected" ? "bg-[#34c759]" : "bg-ink-5"}`} />
                <span className={`font-semibold ${s.state === "connected" ? "text-[#248a3d]" : "text-ink-3"}`}>
                  {s.state === "connected" ? "已连接" : "已停用"}
                </span>
                {s.detail && <span className="text-ink-4">· {s.detail}</span>}
              </span>
              {s.driverPath && <span className="font-mono text-xs text-ink-3">{s.driverPath}</span>}
            </div>
          )}
          <Section title="设置">
            <Card>
              <Row label="启用" desc="关了 agent 就看不到桌面操作工具">
                <Toggle on={s.state !== "disabled"} disabled={busy} onChange={toggle} />
              </Row>
              <Row
                label="driver 路径"
                desc={s.driverEnv ? `由 DOCK_CUA_DRIVER 指定：${s.driverEnv}` : "自动发现：PATH → ~/.local/bin → /Applications/CuaDriver.app"}
              >
                <span className="text-[12px] text-ink-4">{s.driverEnv ? "指定路径" : "自动发现"}</span>
              </Row>
              <Row label="在画面上显示 agent 光标" desc="桌面面板的实时画面上画出 agent 正在点哪">
                <Toggle
                  on={p.agentCursor}
                  onChange={(v) => {
                    prefs.set("agentCursor", v);
                    flashSaved();
                  }}
                />
              </Row>
              <Row label={<span className="font-normal text-ink-3">桌面操作和 bash 走同一道审批</span>}>
                <button type="button" onClick={onOpenGeneral} className="text-[12px] font-medium text-accent hover:underline">
                  常规 › 默认审批
                </button>
              </Row>
            </Card>
          </Section>
        </>
      )}
    </Page>
  );
}

function PermRow({ label, ok }: { label: string; ok: boolean }) {
  return (
    <div className="flex items-center justify-between text-[13px] text-ink">
      {label}
      {ok ? <Check size={15} className="text-[#34c759]" /> : <X size={15} className="text-[#ff3b30]" />}
    </div>
  );
}

function JobCard({
  job,
  steps,
  onDismiss,
}: {
  job: NonNullable<CuaStatus["job"]>;
  steps: string[];
  onDismiss: () => void;
}) {
  const logRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    logRef.current?.scrollTo({ top: logRef.current.scrollHeight });
  }, [job.lines.length]);
  const title = job.action === "install" ? "安装 cua-driver" : "授予系统权限";
  return (
    <div className={`${statusCard} ${job.ok === false ? "border-[#fecaca]" : "border-line-soft"}`}>
      <div className="flex items-center gap-2 text-[13px] font-medium">
        {job.running ? (
          <LoaderCircle size={15} className="animate-spin text-accent" />
        ) : job.ok ? (
          <Check size={15} className="text-[#34c759]" />
        ) : (
          <X size={15} className="text-[#ff3b30]" />
        )}
        <span className="flex-1 text-ink">{job.running ? `正在${title}…` : (job.message ?? title)}</span>
        {!job.running && (
          <button type="button" onClick={onDismiss} className="text-[12px] text-ink-4 hover:text-ink-2">
            关闭
          </button>
        )}
      </div>
      {job.action === "install" && (
        <div className="flex flex-col gap-1.5 pl-0.5">
          {steps.map((l) => {
            // Dock 只给脚本的输出行，对不上具体步骤：不猜进度，跑完才全打勾，跑着看下面的日志。
            const done = job.ok === true;
            return (
              <span key={l} className="flex items-center gap-2 text-[12.5px]">
                {done ? (
                  <span className="flex size-3.5 items-center justify-center rounded-full bg-[#34c759]">
                    <Check size={9} strokeWidth={3} className="text-white" />
                  </span>
                ) : (
                  <Circle size={14} className="text-ink-5" />
                )}
                <span className={done ? "text-ink" : "text-ink-4"}>{l}</span>
              </span>
            );
          })}
        </div>
      )}
      {job.lines.length > 0 && (
        <div ref={logRef} className="thin-scroll flex max-h-[160px] flex-col gap-1 overflow-y-auto rounded-lg bg-ink px-3 py-2.5 font-mono text-[11px] text-[#e5e5ea]">
          {job.lines.map((l, i) => (
            <span key={i} className="break-all">
              {l}
            </span>
          ))}
        </div>
      )}
    </div>
  );
}
