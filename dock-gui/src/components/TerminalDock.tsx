// 底部终端。设计：wonder-ui/shell/TerminalDock。
// 每个标签一个登录 shell（src-tauri/src/pty.rs），xterm.js 画。
// 收起时不卸载，shell 保持；标签里 shell 退出就关掉这个标签。

import { useEffect, useRef, useState, type PointerEvent as ReactPointerEvent } from "react";
import { Channel, invoke } from "@tauri-apps/api/core";
import { Terminal, type ITheme } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import { Plus, SquareTerminal, X } from "lucide-react";
import { IconButton, IN_TAURI } from "./chrome";
import { usePrefs, type Material } from "../dock/prefs";

const MIN_HEIGHT = 120;
const DEFAULT_HEIGHT = 196;

/** 浅色主题，跟 Terminal Body 的 #fafafa 底一致；ANSI 色取在白底上读得清的。 */
const THEME: ITheme = {
  background: "#fafafa",
  foreground: "#1c1c1e",
  cursor: "#1c1c1e",
  cursorAccent: "#fafafa",
  selectionBackground: "#0a84ff33",
  black: "#1c1c1e",
  red: "#d70015",
  green: "#248a3d",
  yellow: "#b25000",
  blue: "#0a66d8",
  magenta: "#8944ab",
  cyan: "#0071a4",
  white: "#8e8e93",
  brightBlack: "#636366",
  brightRed: "#ff3b30",
  brightGreen: "#34c759",
  brightYellow: "#c93400",
  brightBlue: "#0a84ff",
  brightMagenta: "#af52de",
  brightCyan: "#32ade6",
  brightWhite: "#c7c7cc",
};

/** 玻璃材质下终端底透明，露出外层的 glass-pane（和会话区同一层玻璃）。 */
function themeFor(material: Material): ITheme {
  return material === "glass" ? { ...THEME, background: "#00000000" } : THEME;
}

interface Tab {
  key: number;
  cwd: string;
  title: string;
}

export function TerminalDock({ open, cwd, onClose }: { open: boolean; cwd: string; onClose: () => void }) {
  const [tabs, setTabs] = useState<Tab[]>([]);
  const [active, setActive] = useState<number | null>(null);
  const [height, setHeight] = useState(DEFAULT_HEIGHT);
  const [dragging, setDragging] = useState(false);

  // key 从现有标签推出来，不用模块级计数（热更新会把它清零，撞 key）。
  const add = (onlyIfEmpty = false) =>
    setTabs((t) => {
      if (onlyIfEmpty && t.length) return t;
      const key = Math.max(0, ...t.map((x) => x.key)) + 1;
      setActive(key);
      return [...t, { key, cwd, title: "zsh" }];
    });
  // 关标签 = 卸载 TermView，它的清理会杀掉 shell。
  const remove = (key: number) => {
    const rest = tabs.filter((x) => x.key !== key);
    setTabs(rest);
    if (active === key) setActive(rest[rest.length - 1]?.key ?? null);
    if (rest.length === 0) onClose();
  };

  // 第一次展开才开 shell；最后一个标签关掉后再展开，重新开一个。
  useEffect(() => {
    if (open) add(true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  // 拖顶边改高度：最高到窗口的六成。
  const startDrag = (e: ReactPointerEvent) => {
    e.preventDefault();
    const startY = e.clientY;
    const startH = height;
    setDragging(true);
    const move = (ev: PointerEvent) => {
      const max = Math.round(window.innerHeight * 0.6);
      setHeight(Math.min(max, Math.max(MIN_HEIGHT, startH + startY - ev.clientY)));
    };
    const up = () => {
      setDragging(false);
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };

  return (
    <div
      style={{ height: open ? height : 0 }}
      aria-hidden={!open}
      className={`glass-backdrop relative flex shrink-0 flex-col overflow-hidden bg-fill ${open ? "border-t border-line-strong" : ""} ${
        dragging ? "" : "transition-[height] duration-[var(--dur-slow)] ease-[var(--ease-out)]"
      }`}
    >
      <div
        onPointerDown={startDrag}
        className="absolute inset-x-0 -top-0.5 z-10 h-1.5 cursor-row-resize hover:bg-accent/30"
        title="拖动调整高度"
      />
      <div className="glass-bar flex h-8 shrink-0 items-center gap-1 border-b border-line-strong bg-sidebar px-2">
        {tabs.map((tab) => {
          const on = tab.key === active;
          return (
            <div
              key={tab.key}
              onClick={() => setActive(tab.key)}
              className={`pop-in motion-colors group flex h-6 max-w-[200px] cursor-default items-center gap-1.5 rounded-md border px-2.5 ${
                on ? "border-line-strong bg-white" : "border-transparent hover:bg-hover"
              }`}
            >
              <SquareTerminal size={11} className="shrink-0 text-ink-3" />
              <span className={`truncate text-[11.5px] font-medium ${on ? "text-ink" : "text-ink-3"}`}>{tab.title}</span>
              <button
                type="button"
                title="关闭这个终端"
                onClick={(e) => {
                  e.stopPropagation();
                  remove(tab.key);
                }}
                className="shrink-0 rounded text-ink-5 hover:text-ink-2"
              >
                <X size={11} />
              </button>
            </div>
          );
        })}
        <IconButton icon={Plus} label="新建终端" size={12} onClick={() => add()} className="size-[22px] text-ink-4" />
        <div className="flex-1" />
        <IconButton icon={X} label="收起终端 ^`" size={13} onClick={onClose} className="size-[22px] text-ink-4" />
      </div>
      <div className="glass-pane relative min-h-0 flex-1 bg-pane">
        {tabs.map((tab) => (
          <TermView
            key={tab.key}
            tab={tab}
            visible={open && tab.key === active}
            height={height}
            onTitle={(title) => setTabs((t) => t.map((x) => (x.key === tab.key ? { ...x, title } : x)))}
            onExit={() => remove(tab.key)}
          />
        ))}
      </div>
    </div>
  );
}

/** shell 标题多是 `user@host:~/a/b`，标签上只留最后一级目录。 */
function shortTitle(title: string): string {
  const path = title.includes(":") ? title.slice(title.indexOf(":") + 1).trim() : title.trim();
  if (path === "~" || path === "/") return path;
  return path.split("/").filter(Boolean).pop() ?? path;
}

type PtyMessage = ArrayBuffer | number[] | { exit: number };

function TermView({
  tab,
  visible,
  height,
  onTitle,
  onExit,
}: {
  tab: Tab;
  visible: boolean;
  height: number;
  onTitle: (title: string) => void;
  onExit: () => void;
}) {
  const hostRef = useRef<HTMLDivElement>(null);
  const termRef = useRef<{ term: Terminal; fit: FitAddon } | null>(null);
  const cb = useRef({ onTitle, onExit });
  cb.current = { onTitle, onExit };
  const { material } = usePrefs();
  const materialRef = useRef(material);
  materialRef.current = material;

  // 切材质时换底色，不重建终端。
  useEffect(() => {
    if (termRef.current) termRef.current.term.options.theme = themeFor(material);
  }, [material]);

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;
    const term = new Terminal({
      theme: themeFor(materialRef.current),
      allowTransparency: true,
      // 装了 Nerd Font 就用它，提示符里的图标字形才画得出来。
      fontFamily: '"Liga SFMono Nerd Font", "SFMono Nerd Font", "Symbols Nerd Font Mono", "SF Mono", Menlo, monospace',
      fontSize: 12,
      lineHeight: 1.3,
      cursorBlink: true,
      macOptionIsMeta: true,
      scrollback: 5000,
      allowProposedApi: false,
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(host);
    fit.fit();
    termRef.current = { term, fit };

    if (!IN_TAURI) {
      term.write("浏览器预览里没有终端，在 Tauri 窗口里打开。\r\n");
      return () => term.dispose();
    }

    let id: number | null = null;
    let disposed = false;
    const out = new Channel<PtyMessage>();
    out.onmessage = (m) => {
      if (m instanceof ArrayBuffer) term.write(new Uint8Array(m));
      else if (Array.isArray(m)) term.write(Uint8Array.from(m));
      else if (!disposed) cb.current.onExit();
    };
    invoke<number>("pty_spawn", { cwd: tab.cwd, cols: term.cols, rows: term.rows, out })
      .then((pty) => {
        id = pty;
        if (disposed) void invoke("pty_kill", { id: pty });
      })
      .catch((e) => term.write(`\x1b[31m${String(e)}\x1b[0m\r\n`));

    const input = term.onData((data) => {
      if (id !== null) void invoke("pty_write", { id, data });
    });
    const resize = term.onResize(({ cols, rows }) => {
      if (id !== null) void invoke("pty_resize", { id, cols, rows });
    });
    const title = term.onTitleChange((t) => {
      const name = shortTitle(t);
      if (name) cb.current.onTitle(name);
    });

    return () => {
      disposed = true;
      input.dispose();
      resize.dispose();
      title.dispose();
      if (id !== null) void invoke("pty_kill", { id });
      term.dispose();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 显示出来、改高度、窗口变宽窄时重算行列；刚显示时顺手聚焦。
  useEffect(() => {
    const t = termRef.current;
    const host = hostRef.current;
    if (!visible || !t || !host) return;
    const refit = () => {
      if (host.clientHeight > 0) t.fit.fit();
    };
    const timer = window.setTimeout(() => {
      refit();
      t.term.focus();
    }, 200);
    const ro = new ResizeObserver(refit);
    ro.observe(host);
    return () => {
      window.clearTimeout(timer);
      ro.disconnect();
    };
  }, [visible, height]);

  return (
    <div
      ref={hostRef}
      className={`absolute inset-0 py-1.5 pl-3 transition-opacity duration-[var(--dur-fast)] ${
        visible ? "opacity-100" : "pointer-events-none opacity-0"
      }`}
    />
  );
}
