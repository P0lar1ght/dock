// 插件面板：右侧面板槽里的一种（kind `plugin:<面板 id>`）。设计：wonder-ui/plugins/
// PanelPluginDeploy（dvd）、Busy（gul）、Closed（chi）、Error（now）、Picker（acm）。
// - 头栏同别的面板：拖手 · Puzzle + 标题 … 分屏 · 新窗口 | 关闭（插件面板还不能拖出去）。
// - 有视图树就画视图；根 stack 最后一行全是按钮 → 固定在面板底部（dock-core splitPanelFooter）。
//   没有视图：画正文文本，声明的动作当底部按钮。
// - 点按钮 / 列表行 → `surface/action`；执行中被点的转圈，同一面板别的都禁用。
// - 插件自己关了面板（动作回 closed、或面板被注销）：收起，会话区顶部轻提示。
// - 拉不到（插件停了 / 画正文出错）：卡片「面板暂时打不开」+ 原因 + 重试。
// - 面板自带 web 界面（`web: true`）：正文换成沙箱 iframe（WebPanel），头栏照旧。

import { useCallback, useEffect, useRef, useState } from "react";
import { AlertTriangle, Puzzle, X } from "lucide-react";
import { splitPanelFooter, type Surface } from "dock-core";
import { IconButton } from "../../components/chrome";
import { ToolRow } from "../../components/ToolPanel";
import { useFeatureHost, type PanelProps, type PanelEntryGroupProps, type SplitGroupProps } from "../registry";
import { onPluginPush, surfaceAction, surfaceGet } from "./api";
import { surfaceIdOf, surfaceKind } from "./panelKind";
import { useSurfaces } from "./surfaces";
import { ViewButton, ViewTree } from "./ViewTree";
import { WebPanel } from "./WebPanel";

export function PluginPanel({ kind, sessionId, onClose, lead, controls, detached = false }: PanelProps) {
  const id = surfaceIdOf(kind) ?? "";
  const host = useFeatureHost();
  const [surface, setSurface] = useState<Surface | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  // 拉到过就记着标题：插件关面板时提示「<标题> 已关闭面板」。
  const title = useRef<string | null>(null);

  const closedByPlugin = useCallback(() => {
    host.toast(`${title.current ?? id} 已关闭面板`);
    onClose();
  }, [host, id, onClose]);

  const load = useCallback(() => {
    surfaceGet(id).then(
      (s) => {
        if (!s) return;
        title.current = s.title;
        setSurface(s);
        setError(null);
      },
      (e: unknown) => {
        const message = e instanceof Error ? e.message.replace(/^[\w/]+: /, "") : String(e);
        // 开着的面板被插件注销了：当作插件自己关了它。
        if (title.current && /没有面板/.test(message)) closedByPlugin();
        else setError(message);
      },
    );
  }, [id, closedByPlugin]);

  useEffect(() => {
    load();
    return onPluginPush((p) => p.kind === "surface" && p.id === id && load());
  }, [id, load]);

  const act = async (action: string) => {
    setBusy(action);
    try {
      const r = await surfaceAction(id, action);
      if (r.closed) closedByPlugin();
      else if (r.surface) setSurface(r.surface);
    } catch (e) {
      setError(e instanceof Error ? e.message.replace(/^[\w/]+: /, "") : String(e));
    } finally {
      setBusy(null);
    }
  };

  const name = surface?.title ?? title.current ?? id;
  let body: React.ReactNode = null;
  let footer: React.ReactNode = null;
  if (error && !surface) {
    body = (
      <div className="flex flex-1 items-start justify-center p-6 pt-24">
        <div className="enter-fade flex w-full max-w-[370px] flex-col items-center gap-2 rounded-xl border border-line-strong bg-white px-6 py-7 text-center">
          <AlertTriangle size={24} className="text-[#c93400]" />
          <span className="text-[14px] font-semibold text-ink">面板暂时打不开</span>
          <span className="text-xs text-ink-4">{error}</span>
          <button type="button" onClick={load} className="motion-colors mt-2 flex h-8 items-center rounded-lg bg-ink px-4 text-xs font-medium text-white hover:bg-ink-2">
            重试
          </button>
        </div>
      </div>
    );
  } else if (surface?.view) {
    const parts = splitPanelFooter(surface.view);
    body = <ViewTree node={parts.body} onAction={act} busy={busy} />;
    if (parts.footer) {
      footer = parts.footer.children.map(
        (b, i) => b.type === "button" && <ViewButton key={i} label={b.label} action={b.action} style={b.style} onAction={act} busy={busy} />,
      );
    }
  } else if (surface) {
    body = <pre className="whitespace-pre-wrap break-words font-sans text-[13px] leading-normal text-ink">{surface.body}</pre>;
    if (surface.actions.length > 0) {
      footer = surface.actions.map((a, i) => (
        <ViewButton key={a.id} label={a.label} action={a.id} style={i === surface.actions.length - 1 ? "primary" : "secondary"} onAction={act} busy={busy} />
      ));
    }
  } else {
    body = <div className="h-24 animate-pulse rounded-lg bg-fill" />;
  }

  return (
    <aside className="glass-pane enter-fade relative flex min-h-0 min-w-0 flex-1 flex-col bg-[#fafafa]">
      <header className="flex h-11 shrink-0 items-center gap-2 border-b border-[#ececf0] px-3" data-tauri-drag-region>
        {lead}
        <Puzzle size={16} className="shrink-0 text-ink-3" />
        <span className="min-w-0 truncate text-[13px] font-semibold text-ink">{name}</span>
        <div className="min-w-2 flex-1" data-tauri-drag-region />
        {controls}
        <span className="mx-0.5 h-4 w-px bg-line" />
        <IconButton icon={X} label={`关闭${name}`} size={14} onClick={onClose} className="size-7 text-ink-4" />
      </header>
      {surface?.web ? (
        // 插件自带 web 界面：整块交给沙箱 iframe（终端那边照旧画视图 / 正文）。
        <WebPanel surfaceId={id} activeThread={sessionId} detached={detached} onClosed={closedByPlugin} />
      ) : error && !surface ? (
        body
      ) : (
        <>
          <div className="thin-scroll min-h-0 flex-1 overflow-auto px-4 py-3">{body}</div>
          {footer && <div className="flex shrink-0 items-center justify-end gap-2 border-t border-line-soft px-4 py-3">{footer}</div>}
        </>
      )}
      {error && surface && (
        <div className="enter-fade absolute inset-x-3 bottom-16 rounded-lg border border-[#fecaca] bg-[#fef2f2] px-3 py-2 text-xs text-[#d70015]">{error}</div>
      )}
    </aside>
  );
}

/** 分屏菜单里的「插件面板」一组：没有插件面板时整组不出现。 */
export function PluginSplitGroup({ open, onSplit, close }: SplitGroupProps) {
  const surfaces = useSurfaces();
  if (surfaces.length === 0) return null;
  return (
    <>
      <div className="mx-2.5 my-1 h-px bg-line-soft" />
      <div className="px-2.5 pb-1 pt-1.5 text-[11px] font-medium text-ink-4">插件面板</div>
      {surfaces.map((s) => {
        const kind = surfaceKind(s.id);
        const opened = open.includes(kind);
        return (
          <button
            key={s.id}
            type="button"
            disabled={opened}
            onClick={() => {
              close();
              onSplit(kind);
            }}
            className="motion-colors flex h-8 w-full items-center gap-2.5 rounded-md px-2.5 text-left hover:bg-hover disabled:hover:bg-transparent"
          >
            <Puzzle size={14} className={opened ? "text-ink-5" : "text-ink-3"} />
            <span className={`min-w-0 truncate text-[13px] ${opened ? "text-ink-5" : "text-ink"}`}>{s.title}</span>
            <span className={`min-w-0 flex-1 truncate text-[11px] ${opened ? "text-ink-5" : "text-ink-4"}`}>{s.id}</span>
            {opened && <span className="shrink-0 text-[11px] text-ink-5">已打开</span>}
          </button>
        );
      })}
    </>
  );
}

/** 右侧工具面板里的「插件面板」一组：挂着的插件各一行；没有就整组不出现。 */
export function PluginEntryGroup({ onOpen }: PanelEntryGroupProps) {
  const surfaces = useSurfaces();
  if (surfaces.length === 0) return null;
  return (
    <>
      <div className="mx-1 my-1.5 h-px bg-line-soft" />
      <div className="px-1 pb-1 pt-1.5 text-[11px] font-medium text-ink-4">插件面板</div>
      {surfaces.map((s) => (
        <ToolRow
          key={s.id}
          icon={Puzzle}
          label={s.title}
          hint={s.id}
          title={`${s.title} · ${s.id}`}
          onClick={() => onOpen(surfaceKind(s.id))}
        />
      ))}
    </>
  );
}
