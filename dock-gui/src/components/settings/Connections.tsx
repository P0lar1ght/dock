// 网关页最上面的「连接」：本机 Dock 一直开着，远程可以同时打开多条。
// - 「打开」连上并切过去；「切换」只换主界面在看哪一台；「关闭」拆掉那条套接字和隧道。
// - 切走不停任务、不重载页面。关掉的是 GUI 这边，远端的 dock serve 继续跑。
// - 新建 / 编辑在弹框里：名字、连法（直连地址 / SSH 隧道）、设备令牌（只进钥匙串；编辑时空着 = 不换）。
// - SSH 隧道由 GUI 自己开；远端 Dock 没在跑时按「启动命令」经 SSH 拉起来。
// - 「测试连接」真的鉴权 + initialize 一次，回 Dock 版本。

import { useState } from "react";
import { Check, LoaderCircle } from "lucide-react";
import { closeGateway, focusGateway, openGateway, useApp, useGateways } from "../../dock/store";
import { open } from "@tauri-apps/plugin-dialog";
import {
  remoteDelete,
  remoteList,
  remoteSave,
  remoteTest,
  remoteWhere,
  type Remote,
  type SshTarget,
} from "../../dock/remote";
import { errorText } from "../../dock/settingsApi";
import { imeComposing } from "../../dock/ime";
import {
  Card,
  ErrorLine,
  Loading,
  Modal,
  Row,
  Section,
  TextInput,
  autosave,
  dangerText,
  primaryButton,
  secondaryButton,
  smallButton,
  Segmented,
  useLoad,
} from "./ui";

export function ConnectionsSection() {
  const app = useApp();
  const host = useGateways();
  const list = useLoad(remoteList);
  const [editing, setEditing] = useState<Remote | "new" | null>(null);
  const [switching, setSwitching] = useState<string | null>(null);
  const opened = new Set(host.entries.map((e) => e.id));

  const run = async (label: string, fn: () => Promise<void> | void) => {
    setSwitching(label);
    if ((await autosave(async () => { await fn(); })) === null) setSwitching(null);
  };

  return (
    <Section title="连接">
      <span className="-mt-1 text-xs text-ink-4">GUI 连本机 Dock，或经反向代理 / SSH 隧道连远程 Dock（dock serve --remote）</span>
      <Card>
        {list.loading && !list.data && (
          <div className="px-4 py-3">
            <Loading />
          </div>
        )}
        {list.error && (
          <div className="px-4 py-3">
            <ErrorLine>{list.error}</ErrorLine>
          </div>
        )}
        {list.data && (
          <ConnectionRow
            label="本机 Dock"
            desc="GUI 自己拉起的 dock serve"
            current={host.id === "local"}
            opened
            switching={switching === "local"}
            onUse={() => run("local", () => focusGateway(null))}
          />
        )}
        {list.data?.remotes.map((r) => (
          <ConnectionRow
            key={r.id}
            label={r.name}
            desc={<span className="font-mono">{remoteWhere(r)}</span>}
            current={host.id === r.id}
            opened={opened.has(r.id)}
            failed={host.id === r.id && !!app.fatal}
            switching={switching === r.id || switching === `close:${r.id}`}
            onUse={() => run(r.id, () => (opened.has(r.id) ? focusGateway(r.id) : openGateway(r.id)))}
            onClose={() => run(`close:${r.id}`, () => closeGateway(r.id))}
            onEdit={() => setEditing(r)}
            onDelete={async () => {
              if ((await autosave(() => remoteDelete(r.id))) !== null) list.reload();
            }}
          />
        ))}
        <button
          type="button"
          onClick={() => setEditing("new")}
          className="motion-colors px-4 py-3 text-left text-[12.5px] font-medium text-accent hover:bg-[#fafafc]"
        >
          + 添加远程 Dock
        </button>
      </Card>
      {editing && (
        <RemoteEditor
          remote={editing === "new" ? null : editing}
          onClose={() => setEditing(null)}
          onSaved={() => {
            setEditing(null);
            list.reload();
          }}
        />
      )}
    </Section>
  );
}

function ConnectionRow({
  label,
  desc,
  current,
  opened,
  failed,
  switching,
  onUse,
  onClose,
  onEdit,
  onDelete,
}: {
  label: string;
  desc: React.ReactNode;
  current: boolean;
  /** 已经连着（本机永远是）。没打开的远程显示「打开」。 */
  opened: boolean;
  failed?: boolean;
  switching: boolean;
  onUse: () => void;
  onClose?: () => void;
  onEdit?: () => void;
  onDelete?: () => void;
}) {
  const [confirm, setConfirm] = useState(false);
  return (
    <Row
      label={
        <span className="flex items-center gap-2">
          {label}
          {current && (
            <span className={`flex items-center gap-1 text-[11.5px] font-medium ${failed ? "text-[#ff3b30]" : "text-[#34c759]"}`}>
              <Check size={12} />
              {failed ? "当前（连不上）" : "当前"}
            </span>
          )}
        </span>
      }
      desc={desc}
    >
      {confirm ? (
        <span className="enter-fade flex items-center gap-1.5">
          <span className="text-[12px] text-ink-3">令牌会从钥匙串删掉</span>
          <button type="button" onClick={() => setConfirm(false)} className={smallButton}>
            取消
          </button>
          <button
            type="button"
            onClick={onDelete}
            className="motion-colors flex h-7 items-center rounded-md bg-[#ff3b30] px-3 text-[11.5px] font-medium text-white"
          >
            删除
          </button>
        </span>
      ) : (
        <>
          {onEdit && (
            <button type="button" onClick={onEdit} className={smallButton}>
              编辑
            </button>
          )}
          {onDelete && (
            <button type="button" onClick={() => setConfirm(true)} className={dangerText}>
              删除
            </button>
          )}
          {onClose && opened && (
            <button type="button" disabled={switching} onClick={onClose} className={smallButton}>
              关闭
            </button>
          )}
          {!current && (
            <button type="button" disabled={switching} onClick={onUse} className={smallButton}>
              {switching && <LoaderCircle size={12} className="animate-spin" />}
              {opened ? "切换" : "打开"}
            </button>
          )}
        </>
      )}
    </Row>
  );
}

type Mode = "url" | "ssh";

/** 编辑框里的 SSH 字段（都是字符串，存的时候再转）。 */
interface SshDraft {
  host: string;
  user: string;
  port: string;
  identity: string;
  remotePort: string;
  startCommand: string;
}

const sshDraft = (s?: SshTarget | null): SshDraft => ({
  host: s?.host ?? "",
  user: s?.user ?? "root",
  port: s?.port ? String(s.port) : "",
  identity: s?.identity ?? "",
  remotePort: String(s?.remotePort ?? 18990),
  startCommand: s?.startCommand ?? "systemctl start dock",
});

function sshTarget(d: SshDraft): SshTarget {
  const port = d.port.trim() ? Number(d.port) : null;
  return {
    host: d.host.trim(),
    user: d.user.trim(),
    port: port && Number.isInteger(port) ? port : null,
    identity: d.identity.trim() || null,
    remotePort: Number(d.remotePort) || 0,
    startCommand: d.startCommand.trim() || null,
  };
}

function RemoteEditor({ remote, onClose, onSaved }: { remote: Remote | null; onClose: () => void; onSaved: () => void }) {
  const [name, setName] = useState(remote?.name ?? "");
  const [mode, setMode] = useState<Mode>(remote && !remote.ssh ? "url" : "ssh");
  const [url, setUrl] = useState(remote?.url ?? "");
  const [ssh, setSsh] = useState<SshDraft>(() => sshDraft(remote?.ssh));
  const [token, setToken] = useState("");
  const [busy, setBusy] = useState(false);
  const [testing, setTesting] = useState(false);
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const field = (k: keyof SshDraft) => (v: string) => setSsh((d) => ({ ...d, [k]: v }));
  const where = mode === "ssh" ? ssh.host.trim() && ssh.user.trim() && Number(ssh.remotePort) > 0 : url.trim();
  const ready = name.trim() && where && (remote || token.trim());

  const test = async () => {
    setTesting(true);
    setResult(null);
    try {
      // 配置和令牌都没动就用存着的；动了就试新填的（令牌没填时用不了存着的，得重填）。
      const target = sshTarget(ssh);
      const untouched =
        remote &&
        !token.trim() &&
        (mode === "ssh" ? JSON.stringify(remote.ssh) === JSON.stringify(target) : !remote.ssh && url.trim() === remote.url);
      const info = untouched
        ? await remoteTest({ id: remote.id })
        : mode === "ssh"
          ? await remoteTest({ ssh: target, token })
          : await remoteTest({ url, token });
      setResult({ ok: true, text: `已连上 ${info.name ?? "Dock"} ${info.version ?? ""}`.trim() });
    } catch (e) {
      setResult({ ok: false, text: errorText(e) });
    }
    setTesting(false);
  };

  const save = async () => {
    if (!ready) return;
    setBusy(true);
    setError(null);
    try {
      await remoteSave({ id: remote?.id, name, url: mode === "url" ? url : "", ssh: mode === "ssh" ? sshTarget(ssh) : null, token });
      onSaved();
    } catch (e) {
      setError(errorText(e));
    }
    setBusy(false);
  };

  const pickKey = async () => {
    const picked = await open({ multiple: false, directory: false, title: "选择 SSH 私钥" }).catch(() => null);
    if (typeof picked === "string") field("identity")(picked);
  };

  return (
    <Modal onClose={onClose}>
      <span className="text-base font-semibold text-ink">{remote ? "编辑远程 Dock" : "添加远程 Dock"}</span>
      <span className="text-xs text-ink-4">在远端跑 dock device add &lt;名字&gt; 拿令牌。令牌只存进 macOS 钥匙串。</span>
      <div className="flex flex-col gap-2.5" onKeyDown={(e) => e.key === "Enter" && !imeComposing(e) && save()}>
        <Field label="名字">
          <TextInput value={name} onChange={setName} placeholder="如 vps" width="100%" autoFocus={!remote} />
        </Field>
        <Field label="连接方式">
          <div className="flex">
          <Segmented<Mode>
            value={mode}
            onChange={setMode}
            options={[
              { value: "ssh", label: "SSH 隧道", title: "GUI 自己开 SSH 隧道，不用对外开端口" },
              { value: "url", label: "地址", title: "反向代理 / Tailscale 的 https 地址，或自己开的隧道" },
            ]}
          />
          </div>
        </Field>
        {mode === "url" ? (
          <Field label="地址">
            <TextInput value={url} onChange={setUrl} placeholder="https://dock.example.com" width="100%" mono />
          </Field>
        ) : (
          <div className="enter-fade flex flex-col gap-2.5">
            <div className="flex gap-2">
              <div className="w-[96px]">
                <Field label="用户">
                  <TextInput value={ssh.user} onChange={field("user")} placeholder="root" width="100%" mono />
                </Field>
              </div>
              <div className="min-w-0 flex-1">
                <Field label="主机">
                  <TextInput value={ssh.host} onChange={field("host")} placeholder="8.8.8.8 或 ~/.ssh/config 里的名字" width="100%" mono />
                </Field>
              </div>
              <div className="w-[64px]">
                <Field label="端口">
                  <TextInput value={ssh.port} onChange={field("port")} placeholder="22" width="100%" mono />
                </Field>
              </div>
            </div>
            <Field label="私钥（可选，空着用 ssh-agent / ~/.ssh/config）">
              <div className="flex gap-2">
                <div className="min-w-0 flex-1">
                  <TextInput value={ssh.identity} onChange={field("identity")} placeholder="~/.ssh/id_ed25519" width="100%" mono />
                </div>
                <button type="button" onClick={pickKey} className={secondaryButton}>
                  选择…
                </button>
              </div>
            </Field>
            <div className="flex gap-2">
              <div className="w-[96px]">
                <Field label="远端端口">
                  <TextInput value={ssh.remotePort} onChange={field("remotePort")} placeholder="18990" width="100%" mono />
                </Field>
              </div>
              <div className="min-w-0 flex-1">
                <Field label="启动命令（远端没在跑时经 SSH 执行）">
                  <TextInput value={ssh.startCommand} onChange={field("startCommand")} placeholder="systemctl start dock" width="100%" mono />
                </Field>
              </div>
            </div>
          </div>
        )}
        <Field label="设备令牌">
          <TextInput
            value={token}
            onChange={setToken}
            placeholder={remote ? "不换就留空" : "dock device add 打印的那串"}
            type="password"
            width="100%"
            mono
          />
        </Field>
      </div>
      <span className="text-[11.5px] text-ink-4">
        {mode === "ssh" ? "远端 Dock 只绑 127.0.0.1，GUI 经 SSH 转过去，不用对外开端口。" : "要用 https://；自己开的 SSH 隧道可以填 http://127.0.0.1:端口。"}
      </span>
      {result && (
        <span className={`enter-fade text-xs ${result.ok ? "text-[#248a3d]" : "text-[#ff3b30]"}`}>{result.text}</span>
      )}
      {error && <span className="text-xs text-[#ff3b30]">{error}</span>}
      <div className="flex items-center justify-between gap-2">
        <button
          type="button"
          disabled={testing || !where || (!remote && !token.trim())}
          onClick={test}
          className={secondaryButton}
        >
          {testing && <LoaderCircle size={13} className="animate-spin" />}
          {testing && mode === "ssh" ? "正在建隧道…" : "测试连接"}
        </button>
        <span className="flex gap-2">
          <button type="button" onClick={onClose} className={secondaryButton}>
            取消
          </button>
          <button type="button" disabled={!ready || busy} onClick={save} className={primaryButton}>
            {busy && <LoaderCircle size={13} className="animate-spin" />}
            保存
          </button>
        </span>
      </div>
    </Modal>
  );
}

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <label className="flex flex-col gap-1">
      <span className="text-xs font-medium text-ink-3">{label}</span>
      {children}
    </label>
  );
}
