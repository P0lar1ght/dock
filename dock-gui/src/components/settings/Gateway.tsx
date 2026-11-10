// 网关与设备（hah / joy / its）：连哪台 Dock（Connections.tsx）、本机 Dock 进程、网页配对
// （GUI 拉起的 dock serve 没有 TUI，待批的请求在这里批）、远程设备令牌（dock device）。
// 待批请求每 2 秒刷新一次。连着远程 Dock 时只有「连接」：下面几块管的是本机这一台。

import { useEffect, useState } from "react";
import { Copy, LoaderCircle, RotateCw } from "lucide-react";
import { store, useApp } from "../../dock/store";
import { prefs, usePrefs } from "../../dock/prefs";
import {
  deviceAdd,
  deviceList,
  deviceRevoke,
  errorText,
  pairingAccept,
  pairingList,
  pairingResolve,
  pairingRevoke,
  type DeviceRow,
  type PairingListing,
} from "../../dock/settingsApi";
import {
  Card,
  ErrorLine,
  Loading,
  Modal,
  Page,
  Row,
  Section,
  TextInput,
  Toggle,
  autosave,
  flashSaved,
  primaryButton,
  secondaryButton,
  smallButton,
  useLoad,
} from "./ui";
import { imeComposing } from "../../dock/ime";
import { ConnectionsSection } from "./Connections";

function ago(ms: number | null): string {
  if (!ms) return "从未";
  const s = Math.max(0, (Date.now() - ms) / 1000);
  if (s < 60) return "刚刚";
  if (s < 3600) return `${Math.floor(s / 60)} 分钟前`;
  if (s < 86400) return `${Math.floor(s / 3600)} 小时前`;
  return `${Math.floor(s / 86400)} 天前`;
}

export function GatewayPane() {
  const app = useApp();
  return (
    <Page title="网关与设备" subtitle="连接、网页配对与远程设备">
      <ConnectionsSection />
      {/* 连着远程时：本机进程、网页配对（远程模式没有配对入口）都不适用；设备令牌管的是远端那台。 */}
      {!app.remote && <LocalGateway />}
      <DevicesSection />
    </Page>
  );
}

function LocalGateway() {
  const app = useApp();
  const p = usePrefs();
  const [pairing, setPairing] = useState<PairingListing | null>(null);
  const [pairError, setPairError] = useState<string | null>(null);
  const [restarting, setRestarting] = useState(false);

  useEffect(() => {
    let alive = true;
    const tick = () =>
      pairingList().then(
        (r) => alive && (setPairing(r), setPairError(null)),
        (e) => alive && setPairError(errorText(e)),
      );
    void tick();
    const t = window.setInterval(tick, 2000);
    return () => {
      alive = false;
      window.clearInterval(t);
    };
  }, []);

  const connected = app.connection === "open";
  return (
    <>
      <Section title="本机连接">
        <Card>
          <Row
            label={
              <span className="flex items-center gap-2">
                <span className={`size-2 rounded-full ${connected ? "bg-[#34c759]" : "bg-[#ff9f0a]"}`} />
                {connected ? "Dock 进程运行中" : "正在连接 Dock…"}
              </span>
            }
            desc={pairing ? <span className="font-mono">{pairing.listen}{pairing.remote ? " · 远程模式" : ""}</span> : undefined}
          >
            <button
              type="button"
              disabled={restarting}
              onClick={async () => {
                setRestarting(true);
                await store.restartDock();
                window.setTimeout(() => setRestarting(false), 3000);
              }}
              className={secondaryButton}
            >
              {restarting ? <LoaderCircle size={13} className="animate-spin" /> : <RotateCw size={13} />}
              重启 Dock
            </button>
          </Row>
        </Card>
      </Section>

      <Section title="网页配对">
        {pairError && <ErrorLine>读不到配对状态：{pairError}</ErrorLine>}
        <Card>
          <Row label="允许网页连接" desc="嵌入了 Dock 的网页要先在这里批准；关掉后新的请求直接拒（已配对的不受影响）">
            <Toggle
              on={p.acceptPairing}
              onChange={async (v) => {
                prefs.set("acceptPairing", v);
                const r = await autosave(() => pairingAccept(v));
                if (r) setPairing(r);
              }}
            />
          </Row>
          {pairing?.pending.map((r) => (
            <div key={r.id} className="enter-fade flex items-center justify-between gap-3 border-b border-[#f0f0f3] bg-[#0a84ff0a] px-4 py-3">
              <span className="min-w-0 text-[13px] text-ink">
                <span className="font-mono">{r.origin}</span> 请求连接
                <span className="ml-2 text-xs text-ink-4">{r.application}</span>
              </span>
              <span className="flex shrink-0 gap-1.5">
                <button
                  type="button"
                  onClick={async () => {
                    const next = await autosave(() => pairingResolve(r.id, false));
                    if (next) setPairing(next);
                  }}
                  className={smallButton}
                >
                  拒绝
                </button>
                <button
                  type="button"
                  onClick={async () => {
                    const next = await autosave(() => pairingResolve(r.id, true));
                    if (next) setPairing(next);
                  }}
                  className="motion-colors flex h-7 items-center rounded-md bg-ink px-3 text-[11.5px] font-medium text-white hover:bg-ink-2"
                >
                  允许
                </button>
              </span>
            </div>
          ))}
          {pairing?.bindings.map((b) => (
            <Row key={b.origin} label={<span className="font-mono text-[12.5px]">{b.origin}</span>} desc={`${b.application} · 配对于 ${ago(b.boundAt)}`}>
              <button
                type="button"
                onClick={async () => {
                  const next = await autosave(() => pairingRevoke(b.origin));
                  if (next) setPairing(next);
                }}
                className="rounded-md px-2 py-1 text-[12px] font-medium text-[#ff3b30] hover:bg-[#ff3b3014]"
              >
                撤销
              </button>
            </Row>
          ))}
          {pairing && pairing.bindings.length === 0 && pairing.pending.length === 0 && (
            <div className="px-4 py-3 text-[12.5px] text-ink-4">还没有配对过的网页。</div>
          )}
        </Card>
      </Section>

    </>
  );
}

function DevicesSection() {
  const devices = useLoad(deviceList);
  const [adding, setAdding] = useState(false);
  return (
    <>
      <Section title="远程设备">
        <span className="-mt-1 text-xs text-ink-4">远程访问只绑本机地址，TLS 交给反向代理 / Tailscale（dock serve --remote）</span>
        <Card>
          {devices.loading && !devices.data && (
            <div className="px-4 py-3">
              <Loading />
            </div>
          )}
          {devices.error && <div className="px-4 py-3"><ErrorLine>{devices.error}</ErrorLine></div>}
          {devices.data?.map((d) => (
            <DeviceItem key={d.id} device={d} onChanged={devices.set} />
          ))}
          <button type="button" onClick={() => setAdding(true)} className="motion-colors px-4 py-3 text-left text-[12.5px] font-medium text-accent hover:bg-[#fafafc]">
            + 添加设备
          </button>
        </Card>
      </Section>

      {adding && (
        <AddDevice
          onClose={() => setAdding(false)}
          onAdded={(list) => {
            devices.set(list);
            flashSaved();
          }}
        />
      )}
    </>
  );
}

function DeviceItem({ device: d, onChanged }: { device: DeviceRow; onChanged: (l: DeviceRow[]) => void }) {
  const [confirm, setConfirm] = useState(false);
  return (
    <Row label={d.name} desc={`最后使用 ${ago(d.lastUsedAt)}`}>
      {confirm ? (
        <span className="enter-fade flex items-center gap-1.5">
          <span className="text-[12px] text-ink-3">撤销后它立刻断开</span>
          <button type="button" onClick={() => setConfirm(false)} className={smallButton}>
            取消
          </button>
          <button
            type="button"
            onClick={async () => {
              const r = await autosave(() => deviceRevoke(d.id));
              if (r) onChanged(r);
            }}
            className="motion-colors flex h-7 items-center rounded-md bg-[#ff3b30] px-3 text-[11.5px] font-medium text-white"
          >
            撤销
          </button>
        </span>
      ) : (
        <button type="button" onClick={() => setConfirm(true)} className="rounded-md px-2 py-1 text-[12px] font-medium text-[#ff3b30] hover:bg-[#ff3b3014]">
          撤销
        </button>
      )}
    </Row>
  );
}

/** 两步：先起名（joy），再只显示一次令牌（its）。 */
function AddDevice({ onClose, onAdded }: { onClose: () => void; onAdded: (l: DeviceRow[]) => void }) {
  const [name, setName] = useState("");
  const [token, setToken] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);

  if (token) {
    return (
      <Modal>
        <span className="text-base font-semibold text-ink">设备令牌</span>
        <span className="text-xs text-[#d70015]">关掉后再也看不到。请立即复制并妥善保存。</span>
        <div className="flex items-center gap-2 rounded-lg border border-line-strong bg-window px-3 py-2.5">
          <span className="min-w-0 flex-1 truncate font-mono text-xs text-ink select-text">{token}</span>
          <button
            type="button"
            onClick={() => {
              void navigator.clipboard.writeText(token);
              setCopied(true);
            }}
            className="motion-colors flex h-[26px] shrink-0 items-center gap-1 rounded-md bg-ink px-2.5 text-[11px] font-medium text-white"
          >
            <Copy size={11} />
            {copied ? "已复制" : "复制"}
          </button>
        </div>
        <span className="text-xs text-ink-4">在远程客户端里填这串（SDK 的 deviceToken）。</span>
        <button type="button" onClick={onClose} className={`${secondaryButton} justify-center`}>
          完成
        </button>
      </Modal>
    );
  }
  const submit = async () => {
    if (!name.trim()) return;
    setBusy(true);
    setError(null);
    try {
      const r = await deviceAdd(name.trim());
      onAdded(r.devices);
      setToken(r.token);
    } catch (e) {
      setError(errorText(e));
    }
    setBusy(false);
  };
  return (
    <Modal onClose={onClose}>
      <span className="text-base font-semibold text-ink">添加设备</span>
      <span className="text-xs text-ink-4">给这台设备起个名字，撤销时认得出来。</span>
      <div onKeyDown={(e) => e.key === "Enter" && !imeComposing(e) && submit()}>
        <TextInput value={name} onChange={setName} placeholder="如 laptop" width="100%" autoFocus />
      </div>
      {error && <span className="text-xs text-[#ff3b30]">{error}</span>}
      <div className="flex justify-end gap-2">
        <button type="button" onClick={onClose} className={secondaryButton}>
          取消
        </button>
        <button type="button" disabled={!name.trim() || busy} onClick={submit} className={primaryButton}>
          {busy && <LoaderCircle size={13} className="animate-spin" />}
          生成令牌
        </button>
      </div>
    </Modal>
  );
}
