// 左下角个人信息：点开是账户菜单（参考用户给的截图）。Dock 是本机应用、没有账号，
// 「订阅 / 签到 / 退出登录」换成本机有意义的入口：个性化、设置、模型、帮助。
// 去设置的某一页走 App 已有的 `dock:open-settings` 事件，不往侧栏加回调。

import type { ReactNode } from "react";
import { BookOpen, Check, ChevronRight, CircleHelp, Cpu, ExternalLink, Keyboard, MessageSquareWarning, Settings, User, type LucideIcon } from "lucide-react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Avatar } from "./Avatar";
import { Popover } from "./Popover";
import { displayName, usePrefs } from "../dock/prefs";
import { focusGateway, store, useApp, useGateways, type GatewayAttention } from "../dock/store";
import type { SettingsSection } from "./settings";

const REPO = "https://github.com/P0lar1ght/dock";

const openSettings = (section: SettingsSection) =>
  window.dispatchEvent(new CustomEvent<SettingsSection>("dock:open-settings", { detail: section }));

const link = (url: string) => void openUrl(url).catch(() => window.open(url, "_blank", "noopener"));

function Item({
  icon: Icon,
  label,
  hint,
  external,
  onClick,
}: {
  icon: LucideIcon;
  label: string;
  hint?: ReactNode;
  external?: boolean;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      role="menuitem"
      onClick={onClick}
      className="motion-colors mx-1 flex h-9 items-center gap-2.5 rounded-md px-2.5 text-left text-[13px] text-ink hover:bg-hover"
    >
      <Icon size={15} className="shrink-0 text-ink-2" />
      <span className="min-w-0 flex-1 truncate">{label}</span>
      {hint && <span className="shrink-0 text-[11.5px] text-ink-4">{hint}</span>}
      {external ? <ExternalLink size={12} className="shrink-0 text-ink-4" /> : null}
    </button>
  );
}

const Divider = () => <div className="mx-3 my-1 h-px bg-line-soft" />;

function AttentionDot({ attention }: { attention: GatewayAttention | null }) {
  if (!attention) return null;
  const color = attention === "error" ? "bg-[#ff3b30]" : attention === "waiting" ? "bg-[#ff9f0a]" : "bg-[#34c759]";
  return <span className={`size-1.5 shrink-0 rounded-full ${color}`} />;
}

/** 菜单内容（侧栏底部和紧凑头像入口共用）。 */
function MenuBody({ close }: { close: () => void }) {
  const p = usePrefs();
  const remote = useApp().remote;
  const { id, entries } = useGateways();
  const version = store.serverInfo().version;
  const go = (fn: () => void) => () => {
    close();
    fn();
  };
  return (
    <div role="menu" className="flex flex-col py-1">
      <button
        type="button"
        onClick={go(() => openSettings("personalization"))}
        className="motion-colors mx-1 mb-1 flex items-center gap-2.5 rounded-lg px-2.5 py-2 text-left hover:bg-hover"
      >
        <Avatar size={32} />
        <span className="flex min-w-0 flex-1 flex-col">
          <span className="truncate text-[13.5px] font-semibold text-ink">{displayName(p)}</span>
          <span className="truncate text-[11.5px] text-ink-4">
            {remote ? `远程 Dock · ${remote.name}` : "本机 Dock"}
            {version ? ` · v${version}` : ""}
          </span>
        </span>
        <ChevronRight size={14} className="shrink-0 text-ink-4" />
      </button>
      <Divider />
      {entries.map((e) => (
        <button
          key={e.id}
          type="button"
          role="menuitem"
          onClick={go(() => focusGateway(e.id === "local" ? null : e.id))}
          className="motion-colors mx-1 flex h-8 items-center gap-2 rounded-md px-2.5 text-left text-[13px] text-ink hover:bg-hover"
        >
          <AttentionDot attention={e.attention} />
          <span className="min-w-0 flex-1 truncate">{e.id === "local" ? "本机" : e.name}</span>
          {e.id === id && <Check size={13} className="shrink-0 text-[#34c759]" />}
        </button>
      ))}
      <button
        type="button"
        onClick={go(() => openSettings("gateway"))}
        className="motion-colors mx-1 mb-1 flex h-8 items-center rounded-md px-2.5 text-left text-[12.5px] text-ink-3 hover:bg-hover"
      >
        管理连接
      </button>
      <Divider />
      <Item icon={User} label="个性化" onClick={go(() => openSettings("personalization"))} />
      <Item icon={Settings} label="设置" hint="⌘," onClick={go(() => openSettings("general"))} />
      <Item icon={Cpu} label="模型与密钥" onClick={go(() => openSettings("models"))} />
      <Item icon={Keyboard} label="键盘快捷键" onClick={go(() => openSettings("shortcuts"))} />
      <Divider />
      <Item icon={BookOpen} label="使用文档" external onClick={go(() => link(`${REPO}#readme`))} />
      <Item icon={MessageSquareWarning} label="反馈问题" external onClick={go(() => link(`${REPO}/issues/new`))} />
    </div>
  );
}

/** 展开侧栏底部：头像 + 名字，整行可点。 */
export function ProfileButton() {
  const name = displayName(usePrefs());
  const { id, entries } = useGateways();
  const background = entries.find((e) => e.id !== id && e.attention)?.attention ?? null;
  return (
    <Popover
      title="个人资料与设置"
      side="top"
      align="left"
      wrapperClassName="min-w-0 flex-1"
      className="flex h-9 w-full min-w-0 items-center gap-2 rounded-[10px] px-1.5 text-left hover:bg-hover"
      panelClassName="w-[248px] max-h-[70vh]"
      trigger={
        <>
          <Avatar size={22} />
          <span className="min-w-0 flex-1 truncate text-[12.5px] font-medium text-ink">{name}</span>
          <AttentionDot attention={background} />
        </>
      }
    >
      {(close) => <MenuBody close={close} />}
    </Popover>
  );
}

/** 紧凑头像入口：只有头像，菜单开在右边。 */
export function ProfileAvatarButton() {
  return (
    <Popover
      title="个人资料与设置"
      side="right"
      className="flex size-8 items-center justify-center rounded-full hover:bg-hover"
      panelClassName="w-[248px] max-h-[70vh]"
      trigger={<Avatar size={26} />}
    >
      {(close) => <MenuBody close={close} />}
    </Popover>
  );
}

/** 「帮助」小问号：只有帮助那一组。 */
export function HelpButton({ className = "", size = 15 }: { className?: string; size?: number }) {
  return (
    <Popover
      title="帮助"
      side="top"
      align="right"
      className={`flex items-center justify-center rounded-md text-ink-4 hover:bg-hover hover:text-ink-2 ${className}`}
      panelClassName="w-[200px]"
      trigger={<CircleHelp size={size} />}
    >
      {(close) => (
        <div role="menu" className="flex flex-col py-1">
          <Item icon={BookOpen} label="使用文档" external onClick={() => (close(), link(`${REPO}#readme`))} />
          <Item icon={MessageSquareWarning} label="反馈问题" external onClick={() => (close(), link(`${REPO}/issues/new`))} />
          <Item icon={Keyboard} label="键盘快捷键" onClick={() => (close(), openSettings("shortcuts"))} />
        </div>
      )}
    </Popover>
  );
}
