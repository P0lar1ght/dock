// 拖出主窗口的独立面板窗（`index.html?panel=<种类>&session=<id>`）。设计：wonder-ui/pages/SidePanelsSplit 的 D。
//
// 同 ChatWindow：另一个 WebView = 另一份 store 和另一条 WS，连同一个 Dock 进程。
// 只画这一个面板，跟着拖出时的会话；关掉面板就是关掉窗口。
// 功能登记的面板（插件面板）不跟会话，见 FeaturePanelWindow。

import { useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Pin, PinOff } from "lucide-react";
import { FeatureHostContext, panelFor, type FeatureHost } from "../features/registry";
import { IconButton } from "./chrome";
import { store, useApp } from "../dock/store";
import type { SidePanel } from "./ToolPanel";
import { BrowserPanel } from "./BrowserPanel";
import { CanvasPanel } from "./CanvasPanel";
import { DesktopPanel } from "./DesktopPanel";
import { FilesPanel } from "./FilesPanel";
import { SubagentPanel } from "./SubagentPanel";

export function PanelWindow({ kind, sessionId }: { kind: SidePanel; sessionId: string | null }) {
  const feature = panelFor(kind);
  if (feature?.perSession && sessionId) return <SessionFeaturePanelWindow kind={kind} sessionId={sessionId} />;
  if (feature) return <FeaturePanelWindow kind={kind} />;
  if (!sessionId) return null;
  return <SessionPanelWindow kind={kind} sessionId={sessionId} />;
}

/**
 * 功能登记的面板（插件面板）的独立窗：不跟会话，头栏多一个「置顶」——
 * 挂在副屏上看进度、当遥控器用时，别被别的窗口盖住。
 */
function FeaturePanelWindow({ kind }: { kind: string }) {
  const [onTop, setOnTop] = useState(false);
  const win = getCurrentWindow();
  const onClose = () => void win.close();
  const host: FeatureHost = {
    // 独立窗里打开别的面板没有地方放：交给主窗口不在这一版。
    openPanel: () => {},
    closePanel: onClose,
    // 插件自己关了面板：窗口直接关，提示没有意义。
    toast: () => {},
  };
  const lead = <span className="w-[62px] shrink-0" data-tauri-drag-region />;
  const controls = (
    <IconButton
      icon={onTop ? PinOff : Pin}
      label={onTop ? "取消置顶" : "置顶"}
      size={14}
      onClick={() => {
        const next = !onTop;
        void win.setAlwaysOnTop(next).then(() => setOnTop(next));
      }}
      className={`size-7 ${onTop ? "bg-selected text-ink" : "text-ink-4"}`}
    />
  );
  const Panel = panelFor(kind)!.Panel;
  return (
    <FeatureHostContext.Provider value={host}>
      <div className="glass-window flex h-full w-full overflow-hidden bg-window">
        <Panel kind={kind} sessionId={null} onClose={onClose} lead={lead} controls={controls} detached />
      </div>
    </FeatureHostContext.Provider>
  );
}

/**
 * 跟会话的功能面板（MCP App 常驻面板）的独立窗：同 SessionPanelWindow 跟着拖出时的会话，
 * 头栏写会话名；面板自己画（功能登记的组件）。
 */
function SessionFeaturePanelWindow({ kind, sessionId }: { kind: string; sessionId: string }) {
  const dock = useApp();
  const session = dock.sessions.find((s) => s.id === sessionId) ?? null;
  useEffect(() => {
    store.select(sessionId);
    store.focus([sessionId]);
  }, [sessionId]);
  const onClose = () => void getCurrentWindow().close();
  const host: FeatureHost = { openPanel: () => {}, closePanel: onClose, toast: () => {} };
  const lead = <span className="w-[62px] shrink-0" data-tauri-drag-region />;
  const controls = session && (
    <span className="max-w-[200px] shrink truncate rounded-md bg-fill px-1.5 py-0.5 text-[11px] text-ink-4" title={session.title}>
      {session.title}
    </span>
  );
  const Panel = panelFor(kind)!.Panel;
  return (
    <FeatureHostContext.Provider value={host}>
      <div className="glass-window flex h-full w-full overflow-hidden bg-window">
        <Panel kind={kind} sessionId={sessionId} onClose={onClose} lead={lead} controls={controls} detached />
      </div>
    </FeatureHostContext.Provider>
  );
}

function SessionPanelWindow({ kind, sessionId }: { kind: SidePanel; sessionId: string }) {
  const dock = useApp();
  const session = dock.sessions.find((s) => s.id === sessionId) ?? null;
  const [finding, setFinding] = useState(false);

  useEffect(() => {
    store.select(sessionId);
    store.focus([sessionId]);
  }, [sessionId]);

  // ⌘P 照主窗口：文件窗里弹快速打开。
  useEffect(() => {
    if (kind !== "files") return;
    const onKey = (e: KeyboardEvent) => {
      if (e.metaKey && !e.shiftKey && !e.altKey && e.key.toLowerCase() === "p") {
        e.preventDefault();
        setFinding(true);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [kind]);

  // 红绿灯（titleBarStyle: Overlay）占住头栏左边。
  const lead = <span className="w-[62px] shrink-0" data-tauri-drag-region />;
  // 哪个会话的面板：标题栏旁边写会话名。
  const controls = session && (
    <span className="max-w-[200px] shrink truncate rounded-md bg-fill px-1.5 py-0.5 text-[11px] text-ink-4" title={session.title}>
      {session.title}
    </span>
  );
  // WKWebView 里 window.close() 关不掉 Tauri 开的窗：走窗口 API（capabilities 要 core:window:allow-close）。
  const onClose = () => void getCurrentWindow().close();

  return (
    <div className="glass-window flex h-full w-full overflow-hidden bg-window">
      {kind === "browser" ? (
        <BrowserPanel sessionId={sessionId} onClose={onClose} lead={lead} controls={controls} />
      ) : kind === "desktop" ? (
        <DesktopPanel sessionId={sessionId} onClose={onClose} lead={lead} controls={controls} />
      ) : kind === "canvas" ? (
        <CanvasPanel sessionId={sessionId} target={null} onClose={onClose} lead={lead} controls={controls} />
      ) : kind === "subagents" ? (
        <SubagentPanel sessionId={sessionId} target={null} onClose={onClose} lead={lead} controls={controls} />
      ) : (
        <FilesPanel
          sessionId={sessionId}
          cwd={session?.cwd ?? null}
          finding={finding}
          onFinding={setFinding}
          onClose={onClose}
          lead={lead}
          controls={controls}
        />
      )}
    </div>
  );
}
