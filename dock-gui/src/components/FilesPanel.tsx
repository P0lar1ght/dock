// 文件面板：只读看会话的工作目录，左树右预览。设计：wonder-ui/pages/FilesPanel。
//
// 全走 dock.1 的 `fs/*`（路径相对会话 cwd），不读本机磁盘：远程 Dock 的文件在另一台机器上。
// 能改（能力 workspaceEdit）：文本文件点「编辑」用 CodeMirror 改、⌘S 保存（期间被 agent 改过
// 会拦下来让人选）；树上右键 / 头栏新建文件和文件夹、改名、拖动移动、移到废纸篓。
// 改完 Dock 会告诉这一页的 agent（下一步收到一条提醒）。
//
// 蓝点 = 这个会话里 agent 用写文件工具改过的文件（从对话流里取，不猜）。它每改完一个，
// 展开着的目录和正在看的文件跟着重读。

import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type MouseEvent as ReactMouseEvent,
  type PointerEvent as ReactPointerEvent,
  type MutableRefObject,
  type ReactNode,
} from "react";
import {
  AppWindow,
  Archive,
  AtSign,
  ChevronDown,
  ChevronRight,
  Copy,
  Eye,
  EyeOff,
  FilePlus,
  FileText,
  Folder,
  FolderOpen,
  FolderPlus,
  MessageSquare,
  PenLine,
  Pencil,
  RotateCw,
  Search,
  SquareArrowOutUpRight,
  Trash2,
  X,
} from "lucide-react";
import { openPath, revealItemInDir } from "@tauri-apps/plugin-opener";
import type { ThreadState } from "dock-core";
import { dock } from "../dock/client";
import { store, useApp, useThread } from "../dock/store";
import { IconButton } from "./chrome";
import { Markdown } from "./Markdown";
import { imeComposing } from "../dock/ime";
import type { PathTarget } from "./markdown/pathLink";
import { Tokens } from "./markdown/CodeBlock";
import { useHighlight } from "./markdown/highlight";
import { usePrefs } from "../dock/prefs";
import { FOLDER_COLOR, fileType } from "./fileType";
import { fileEditorEntry, filePreviewFor, useFeatureHost } from "../features/registry";
import { ContextMenu, type MenuEntry } from "./SessionMenu";
import { DragGhost, NameInput, UnsavedConfirm, useTreeDrag, type DragState } from "./files/TreeOps";
import {
  baseName,
  canEditFiles,
  createFile,
  editError,
  isConflict,
  isWithin,
  joinPath,
  makeDir,
  movePath,
  parentOf,
  remapPath,
  saveFile,
  trashPath,
  type SavedFile,
} from "../dock/fsEdit";

interface Entry {
  name: string;
  path: string;
  kind: "dir" | "file" | "symlink";
  size?: number | null;
  modifiedMs?: number | null;
}

interface Dir {
  entries: Entry[];
  truncated: boolean;
}

type FileView =
  | { kind: "text"; path: string; size: number; modifiedMs?: number; text: string; truncated: boolean }
  | { kind: "image"; path: string; size: number; modifiedMs?: number; mime: string; data: string }
  | { kind: "binary"; path: string; size: number; modifiedMs?: number };

/** 树上正在起名的那一行：新建（在 `dir` 下）或改名（`path`）。`error` 是上次提交失败的原因。 */
type Naming =
  | { kind: "file" | "dir"; dir: string; error?: string | null }
  | { kind: "rename"; path: string; isDir: boolean; error?: string | null };

type FileAction = "new-file" | "new-dir" | "rename" | "trash" | "copy" | "mention" | "open-external" | "reveal";

/** 正在编辑、还没保存的文件：换文件 / 关面板前要问。`discard` = 选了「不保存」，丢掉改动。 */
type Dirty = { name: string; save: () => Promise<boolean>; discard: () => void } | null;

const WRITE_TOOLS: Record<string, string[]> = {
  write_file: ["target_file", "file_path"],
  search_replace: ["file_path", "target_file"],
  edit_file: ["target_file", "file_path"],
};

export function FilesPanel({
  sessionId,
  cwd,
  target = null,
  onTargetHandled,
  finding,
  onFinding,
  onClose,
  lead,
  controls,
}: {
  sessionId: string | null;
  /** 会话目录（本机路径，给「在访达中显示」用）。 */
  cwd: string | null;
  /** 对话流里点了文件路径：`seq` 变了就找到并打开它（有行号就跳到那一行）。 */
  target?: (PathTarget & { seq: number }) | null;
  /** `target` 处理完了（打开了或交给了快速打开）。 */
  onTargetHandled?: () => void;
  /** 快速打开（⌘P）开着没有：App 管，⌘P 在面板外也能按。 */
  finding: boolean;
  onFinding: (open: boolean) => void;
  onClose: () => void;
  /** 头栏最左边（分屏 / 独立窗口时的拖手、红绿灯占位）。 */
  lead?: ReactNode;
  /** 关闭按钮左边（分屏菜单、在新窗口打开）。 */
  controls?: ReactNode;
}) {
  const app = useApp();
  const thread = useThread(sessionId);
  const connected = app.connection === "open";
  const [root, setRoot] = useState<string | null>(null);
  const [dirs, setDirs] = useState<Map<string, Dir>>(new Map());
  const [expanded, setExpanded] = useState<Set<string>>(new Set([""]));
  const [hidden, setHidden] = useState(false);
  const [selected, setSelected] = useState<string | null>(null);
  const [file, setFile] = useState<FileView | { kind: "loading" } | { kind: "error"; message: string } | null>(null);
  const [error, setError] = useState<string | null>(null);
  /** 打开后要跳到的行（对话流里点的 `a.ts:42`）。 */
  const [jump, setJump] = useState<{ path: string; line: number; seq: number } | null>(null);
  /** 路径找不准时，快速打开预先填的名字。 */
  const [quickQuery, setQuickQuery] = useState("");
  const host = useFeatureHost();
  const canEdit = connected && canEditFiles();
  const [menu, setMenu] = useState<{ x: number; y: number; entry: Entry | null } | null>(null);
  const [naming, setNaming] = useState<Naming | null>(null);
  /** 新建完文件直接进编辑。 */
  const [editRequest, setEditRequest] = useState<{ path: string; seq: number } | null>(null);
  const dirty = useRef<Dirty>(null);
  const [confirm, setConfirm] = useState<{ run: () => void } | null>(null);
  /** 正在编辑的文件有没保存的修改：先问，再做 `run`。 */
  const guarded = (run: () => void) => (dirty.current ? setConfirm({ run }) : run());

  const { paths: changed, writes } = useMemo(() => agentChanges(thread, root), [thread, root]);
  // 按写的次数而不是按路径集合触发重读：同一个文件再改一次，集合不变，内容变了。
  const changedKey = writes.join("\n");

  // 换会话：从头来。
  useEffect(() => {
    setRoot(null);
    setDirs(new Map());
    setExpanded(new Set([""]));
    setSelected(null);
    setFile(null);
    setError(null);
    setNaming(null);
    dirty.current = null;
  }, [sessionId]);

  const loadDir = useCallback(
    async (path: string) => {
      if (!sessionId) return;
      try {
        const r = await dock.call<{ root: string; path: string; entries: Entry[]; truncated: boolean }>("fs/list", {
          threadId: sessionId,
          path,
          hidden,
        });
        setRoot(r.root);
        setError(null);
        setDirs((prev) => new Map(prev).set(path, { entries: r.entries, truncated: r.truncated }));
      } catch (e) {
        if (path === "") setError(message(e));
        else setDirs((prev) => new Map(prev).set(path, { entries: [], truncated: false }));
      }
    },
    [sessionId, hidden],
  );

  const loadFile = useCallback(
    async (path: string, quiet = false) => {
      if (!sessionId) return;
      if (!quiet) setFile({ kind: "loading" });
      try {
        const previewer = filePreviewFor(path.slice(path.lastIndexOf("/") + 1));
        previewer?.preload?.();
        // 表格 / 文档这类二进制格式由预览器自己分块拉：这里只要大小和修改时间。
        if (previewer?.binary) {
          const meta = await dock.call<FileView>("fs/read", { threadId: sessionId, path, maxBytes: 1 });
          setFile({ kind: "binary", path: meta.path, size: meta.size, modifiedMs: meta.modifiedMs });
          return;
        }
        setFile(await dock.call<FileView>("fs/read", { threadId: sessionId, path }));
      } catch (e) {
        setFile({ kind: "error", message: message(e) });
      }
    },
    [sessionId],
  );

  // 连上 / 切隐藏文件 / agent 改了文件：展开着的目录全重读，正在看的文件也重读。
  const expandedRef = useRef(expanded);
  expandedRef.current = expanded;
  const refresh = useCallback(() => {
    for (const path of expandedRef.current) void loadDir(path);
  }, [loadDir]);
  useEffect(() => {
    if (connected && sessionId) refresh();
  }, [connected, sessionId, refresh, changedKey]);
  const selectedRef = useRef(selected);
  selectedRef.current = selected;
  useEffect(() => {
    if (connected && selectedRef.current) void loadFile(selectedRef.current, true);
  }, [changedKey, connected, loadFile]);

  const toggle = (path: string) => {
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path);
      else {
        next.add(path);
        if (!dirs.has(path)) void loadDir(path);
      }
      return next;
    });
  };

  const open = (path: string) => {
    setSelected(path);
    setJump(null);
    void loadFile(path);
  };

  // 快速打开选中深处的文件：把一路上的目录都展开。
  const reveal = (path: string, line: number | null = null) => {
    const parts = path.split("/");
    const parents = parts.slice(0, -1).map((_, i) => parts.slice(0, i + 1).join("/"));
    setExpanded((prev) => new Set([...prev, ...parents]));
    parents.forEach((p) => !dirs.has(p) && void loadDir(p));
    open(path);
    if (line) setJump({ path, line, seq: Date.now() });
  };

  // 对话流里点的路径：找到就打开；找不准（不存在、同名的好几个）就弹快速打开、填上文件名。
  useEffect(() => {
    if (!target || !sessionId) return;
    let alive = true;
    void locate(sessionId, target.path).then((found) => {
      if (!alive) return;
      onTargetHandled?.();
      if (found) guarded(() => reveal(found, target.line));
      else {
        setQuickQuery(target.path.slice(target.path.lastIndexOf("/") + 1));
        onFinding(true);
      }
    });
    return () => {
      alive = false;
    };
    // 只在新的点击（seq）或换会话时跑；reveal / onFinding 每次渲染都是新的。
  }, [target?.seq, sessionId]);

  const closeFinding = () => {
    setQuickQuery("");
    onFinding(false);
  };

  const mention = (path: string) => sessionId && store.fillComposer(sessionId, `@${path} `, "append");

  // ---- 改文件 ----

  /** 展开这些文件夹（连同上面的）并重读。 */
  const expandDirs = (paths: string[]) => {
    const all = new Set<string>();
    for (const p of paths) {
      const parts = p ? p.split("/") : [];
      for (let i = 1; i <= parts.length; i++) all.add(parts.slice(0, i).join("/"));
    }
    setExpanded((prev) => new Set([...prev, ...all]));
    for (const d of all) void loadDir(d);
  };

  const startNew = (kind: "file" | "dir", dir: string) => {
    expandDirs([dir]);
    setNaming({ kind, dir });
  };

  const fail = (e: unknown) => store.reportError(new Error(editError(e)));

  const submitNaming = async (name: string) => {
    if (!sessionId || !naming) return;
    try {
      if (naming.kind === "rename") {
        const to = joinPath(parentOf(naming.path), name);
        const from = naming.path;
        await moveWithGuard(from, to);
        return;
      }
      const path = joinPath(naming.dir, name.replace(/^\/+/, ""));
      if (naming.kind === "dir") {
        const made = await makeDir(sessionId, path);
        setNaming(null);
        void loadDir(naming.dir);
        expandDirs([made.path]);
      } else {
        const made = await createFile(sessionId, path);
        setNaming(null);
        void loadDir(naming.dir);
        expandDirs([parentOf(made.path)]);
        guarded(() => {
          open(made.path);
          setEditRequest({ path: made.path, seq: Date.now() });
        });
      }
    } catch (e) {
      setNaming((n) => n && { ...n, error: editError(e) });
    }
  };

  /** 改名 / 移动之后：展开状态、选中的文件跟着换路径，两头的文件夹重读。 */
  const afterMove = (from: string, to: string) => {
    const nextExpanded = new Set([...expandedRef.current].map((p) => remapPath(p, from, to)));
    setExpanded(nextExpanded);
    setDirs((prev) => new Map([...prev].filter(([k]) => !isWithin(k, from))));
    for (const d of new Set([parentOf(from), parentOf(to), ...[...nextExpanded].filter((p) => isWithin(p, to))]))
      void loadDir(d);
    // 移到收起的文件夹里：展开它，挪过去的东西看得见。
    expandDirs([parentOf(to)]);
    const sel = selectedRef.current;
    if (sel && isWithin(sel, from)) {
      const next = remapPath(sel, from, to);
      setSelected(next);
      void loadFile(next, true);
    }
  };

  /** 改名 / 移动；动到正在编辑的文件（或它的文件夹）且没保存时先问。 */
  const moveWithGuard = (from: string, to: string) =>
    new Promise<void>((resolve, reject) => {
      const run = () =>
        movePath(sessionId!, from, to).then((r) => {
          setNaming(null);
          afterMove(from, r.path);
          resolve();
        }, reject);
      const sel = selectedRef.current;
      if (sel && isWithin(sel, from)) guarded(run);
      else run();
    });

  const trash = (path: string) => {
    if (!sessionId) return;
    const run = async () => {
      try {
        await trashPath(sessionId, path);
        if (selectedRef.current && isWithin(selectedRef.current, path)) {
          dirty.current = null;
          setSelected(null);
          setFile(null);
        }
        setExpanded((prev) => new Set([...prev].filter((p) => !isWithin(p, path))));
        void loadDir(parentOf(path));
        host.toast(`已移到废纸篓：${baseName(path)}`);
      } catch (e) {
        fail(e);
      }
    };
    const sel = selectedRef.current;
    if (sel && isWithin(sel, path)) guarded(() => void run());
    else void run();
  };

  const drag = useTreeDrag({
    onDrop: (from, toDir) => void moveWithGuard(from, joinPath(toDir, baseName(from))).catch(fail),
    onHoverDir: (dir) => !expandedRef.current.has(dir) && expandDirs([dir]),
  });

  const runAction = (action: FileAction, entry: Entry | null) => {
    const path = entry?.path ?? "";
    const isDir = !entry || entry.kind === "dir";
    if (action === "new-file" || action === "new-dir") startNew(action === "new-file" ? "file" : "dir", isDir ? path : parentOf(path));
    else if (action === "rename" && entry) setNaming({ kind: "rename", path, isDir });
    else if (action === "trash" && entry) trash(path);
    else if (action === "copy") void navigator.clipboard.writeText(path);
    else if (action === "mention") mention(path);
    else if (action === "open-external" && cwd) void openPath(`${cwd}/${path}`).catch(store.reportError);
    else if (action === "reveal" && cwd) void revealItemInDir(`${cwd}/${path}`).catch(store.reportError);
  };

  const menuEntries = (entry: Entry | null): MenuEntry<FileAction>[] => {
    const local = !app.remote && !!cwd;
    const isDir = !entry || entry.kind === "dir";
    return [
      canEdit && isDir ? { action: "new-file", icon: FilePlus, label: "新建文件" } : null,
      canEdit && isDir ? { action: "new-dir", icon: FolderPlus, label: "新建文件夹" } : null,
      null,
      canEdit && entry ? { action: "rename", icon: PenLine, label: "重命名", hint: "F2" } : null,
      canEdit && entry ? { action: "trash", icon: Trash2, label: "移到废纸篓", hint: "⌘⌫", danger: true } : null,
      null,
      entry ? { action: "copy", icon: Copy, label: "复制路径" } : null,
      entry ? { action: "mention", icon: AtSign, label: "插入输入框" } : null,
      local && entry && !isDir ? { action: "open-external", icon: AppWindow, label: "用默认应用打开" } : null,
      local && entry ? { action: "reveal", icon: SquareArrowOutUpRight, label: "在访达中显示" } : null,
    ];
  };

  const onRowMenu = (e: ReactMouseEvent, entry: Entry | null) => {
    e.preventDefault();
    e.stopPropagation();
    if (menuEntries(entry).some(Boolean)) setMenu({ x: e.clientX, y: e.clientY, entry });
  };

  /** 树上的键：F2 改名、⌘⌫ 移到废纸篓（对着打开的那个文件）。 */
  const onTreeKey = (e: ReactKeyboardEvent) => {
    if (!canEdit || !selected || naming) return;
    if (e.key === "F2") {
      e.preventDefault();
      setNaming({ kind: "rename", path: selected, isDir: false });
    } else if (e.key === "Backspace" && e.metaKey) {
      e.preventDefault();
      trash(selected);
    }
  };

  return (
    <aside className="glass-pane enter-fade relative flex min-h-0 min-w-0 flex-1 flex-col bg-white">
      <header className="flex h-11 shrink-0 items-center gap-2.5 border-b border-line-soft px-4" data-tauri-drag-region>
        {lead}
        <Folder size={16} className="shrink-0 text-ink-3" />
        <span className="shrink-0 text-[13px] font-semibold text-ink">文件</span>
        {sessionId && root && (
          <span className="flex h-6 min-w-0 items-center truncate rounded-full bg-fill px-2.5 font-mono text-xs text-ink-3" title={root}>
            {tildify(root)}
          </span>
        )}
        <div className="flex-1" data-tauri-drag-region />
        {sessionId && (
          <>
            {canEdit && (
              <>
                <IconButton
                  icon={FilePlus}
                  label="新建文件"
                  size={14}
                  onClick={() => startNew("file", selected ? parentOf(selected) : "")}
                  className="size-7 text-ink-4"
                />
                <IconButton
                  icon={FolderPlus}
                  label="新建文件夹"
                  size={14}
                  onClick={() => startNew("dir", selected ? parentOf(selected) : "")}
                  className="size-7 text-ink-4"
                />
              </>
            )}
            <IconButton
              icon={hidden ? Eye : EyeOff}
              label={hidden ? "不显示隐藏文件" : "显示隐藏文件"}
              size={14}
              onClick={() => setHidden((v) => !v)}
              className="size-7 text-ink-4"
            />
            <IconButton icon={RotateCw} label="刷新" size={14} onClick={refresh} className="size-7 text-ink-4" />
          </>
        )}
        {controls}
        {(sessionId || controls) && <div className="h-4 w-px shrink-0 bg-line-strong" />}
        <IconButton icon={X} label="关闭文件面板" size={14} onClick={() => guarded(onClose)} className="size-7 text-ink-4" />
      </header>

      {!sessionId ? (
        <Empty icon={MessageSquare} title="先选一个会话" text="文件面板显示当前会话的工作目录。新对话选好目录后这里就有内容。" />
      ) : error ? (
        <Empty icon={Folder} title="读不了这个目录" text={error} action={{ label: "重试", run: refresh }} />
      ) : (
        <div className="flex min-h-0 flex-1">
          <div className="flex w-[clamp(200px,38%,270px)] shrink-0 flex-col border-r border-line-soft bg-pane">
            <div className="px-3 pb-2 pt-3">
              <button
                type="button"
                onClick={() => onFinding(true)}
                className="motion-colors flex h-8 w-full items-center gap-2 rounded-lg border border-line-strong bg-white px-2.5 text-left hover:border-ink-5"
              >
                <Search size={14} className="shrink-0 text-ink-4" />
                <span className="flex-1 text-[13px] text-ink-5">按名字找文件</span>
                <span className="font-mono text-[11px] text-ink-4">⌘P</span>
              </button>
            </div>
            <div
              data-tree-root
              onContextMenu={(e) => onRowMenu(e, null)}
              onKeyDown={onTreeKey}
              className={`thin-scroll flex min-h-0 flex-1 flex-col gap-px overflow-y-auto px-2 pb-2 transition-colors duration-[var(--dur-fast)] ${
                drag.drag?.target === "" ? "bg-[#0a84ff0d]" : ""
              }`}
            >
              {dirs.has("") ? (
                <Tree
                  path=""
                  depth={0}
                  dirs={dirs}
                  expanded={expanded}
                  selected={selected}
                  changed={changed}
                  onToggle={toggle}
                  onOpen={(path) => path !== selected && guarded(() => open(path))}
                  edit={
                    canEdit
                      ? {
                          naming,
                          onSubmit: (name) => void submitNaming(name),
                          onCancel: () => setNaming(null),
                          onMenu: onRowMenu,
                          drag: drag.drag,
                          onDragStart: drag.start,
                          wrapClick: drag.click,
                        }
                      : { onMenu: onRowMenu }
                  }
                />
              ) : (
                <span className="px-2 py-1 text-xs text-ink-4">正在读目录…</span>
              )}
            </div>
            {changed.size > 0 && (
              <div className="flex items-center gap-1.5 border-t border-line-soft px-4 py-2.5">
                <span className="size-1.5 rounded-full bg-accent" />
                <span className="text-[11px] text-ink-4">这个会话里 agent 改过的文件</span>
              </div>
            )}
          </div>
          <Preview
            sessionId={sessionId}
            canEdit={canEdit}
            editRequest={editRequest}
            dirtyRef={dirty}
            guard={guarded}
            onSaved={setFile}
            onReload={() => selected && void loadFile(selected, true)}
            file={file}
            selected={selected}
            jump={jump}
            changed={!!selected && changed.has(selected)}
            onMention={mention}
            onReveal={
              cwd && selected && !app.remote ? () => revealItemInDir(`${cwd}/${selected}`).catch(store.reportError) : undefined
            }
            onOpenExternal={
              cwd && selected && !app.remote ? () => openPath(`${cwd}/${selected}`).catch(store.reportError) : undefined
            }
          />
        </div>
      )}

      {finding && sessionId && (
        <QuickOpen
          sessionId={sessionId}
          initialQuery={quickQuery}
          onClose={closeFinding}
          onOpen={(path) => {
            closeFinding();
            guarded(() => reveal(path));
          }}
          onMention={(path) => {
            closeFinding();
            mention(path);
          }}
        />
      )}

      {menu && (
        <ContextMenu
          x={menu.x}
          y={menu.y}
          entries={menuEntries(menu.entry)}
          onAction={(a) => runAction(a, menu.entry)}
          onClose={() => setMenu(null)}
        />
      )}
      {drag.drag && <DragGhost drag={drag.drag} />}
      {confirm && dirty.current && (
        <UnsavedConfirm
          name={dirty.current.name}
          onCancel={() => setConfirm(null)}
          onDiscard={() => {
            dirty.current?.discard();
            dirty.current = null;
            setConfirm(null);
            confirm.run();
          }}
          onSave={() => {
            const pending = dirty.current;
            setConfirm(null);
            void pending?.save().then((ok) => {
              if (!ok) return;
              dirty.current = null;
              confirm.run();
            });
          }}
        />
      )}
    </aside>
  );
}

/** 树上改文件用的那些（不能改时只有右键菜单）。 */
interface TreeEdit {
  naming?: Naming | null;
  onSubmit?: (name: string) => void;
  onCancel?: () => void;
  onMenu: (e: ReactMouseEvent, entry: Entry | null) => void;
  drag?: DragState | null;
  onDragStart?: (e: ReactPointerEvent, path: string) => void;
  wrapClick?: (run: () => void) => () => void;
}

function Tree({
  path,
  depth,
  dirs,
  expanded,
  selected,
  changed,
  onToggle,
  onOpen,
  edit,
}: {
  path: string;
  depth: number;
  dirs: Map<string, Dir>;
  expanded: Set<string>;
  selected: string | null;
  changed: Set<string>;
  onToggle: (path: string) => void;
  onOpen: (path: string) => void;
  edit: TreeEdit;
}) {
  const dir = dirs.get(path);
  const naming = edit.naming;
  // 在这个文件夹下新建：输入框排在最前面。
  const creating =
    naming && naming.kind !== "rename" && naming.dir === path ? (
      <NameInput
        key={`new-${naming.kind}`}
        depth={depth}
        kind={naming.kind}
        error={naming.error}
        onSubmit={edit.onSubmit!}
        onCancel={edit.onCancel!}
      />
    ) : null;
  if (!dir) return <span className="py-1 text-xs text-ink-4" style={{ paddingLeft: 8 + depth * 16 }}>…</span>;
  if (dir.entries.length === 0 && depth > 0)
    return (
      creating ?? (
        <span className="py-1 text-xs text-ink-5" style={{ paddingLeft: 30 + depth * 16 }}>
          空目录
        </span>
      )
    );
  const wrap = edit.wrapClick ?? ((run: () => void) => run);
  return (
    <>
      {creating}
      {dir.entries.map((e) => {
        const isDir = e.kind === "dir";
        const open = expanded.has(e.path);
        const active = selected === e.path;
        const renaming = naming?.kind === "rename" && naming.path === e.path;
        const dropHere = isDir && edit.drag?.target === e.path;
        const dragged = edit.drag?.from === e.path;
        return (
          <div key={e.path} className="flex flex-col gap-px">
            {renaming ? (
              <NameInput
                depth={depth}
                kind={isDir ? "dir" : "file"}
                initial={e.name}
                error={naming.error}
                onSubmit={edit.onSubmit!}
                onCancel={edit.onCancel!}
              />
            ) : (
              <button
                type="button"
                title={e.path}
                data-tree-dir={isDir ? e.path : parentOf(e.path)}
                data-tree-folder={isDir ? e.path : undefined}
                onClick={wrap(() => (isDir ? onToggle(e.path) : onOpen(e.path)))}
                onContextMenu={(ev) => edit.onMenu(ev, e)}
                onPointerDown={edit.onDragStart && ((ev) => edit.onDragStart!(ev, e.path))}
                className={`motion-colors flex h-7 shrink-0 items-center gap-1.5 rounded-md pr-2 text-left ${
                  dropHere ? "bg-[#0a84ff1f] ring-1 ring-inset ring-accent" : active ? "bg-selected" : "hover:bg-hover"
                } ${dragged ? "opacity-50" : ""}`}
                style={{ paddingLeft: 8 + depth * 16 }}
              >
                {isDir ? (
                  open ? (
                    <ChevronDown size={14} className="shrink-0 text-ink-4" />
                  ) : (
                    <ChevronRight size={14} className="shrink-0 text-ink-4" />
                  )
                ) : (
                  <span className="size-3.5 shrink-0" />
                )}
                {isDir ? <FolderIcon open={open} /> : <TypeIcon name={e.name} />}
                <span className={`min-w-0 flex-1 truncate text-[13px] text-ink ${active ? "font-medium" : ""}`}>{e.name}</span>
                {changed.has(e.path) && <span className="size-1.5 shrink-0 rounded-full bg-accent" />}
              </button>
            )}
            {isDir && open && (
              <Tree
                path={e.path}
                depth={depth + 1}
                dirs={dirs}
                expanded={expanded}
                selected={selected}
                changed={changed}
                onToggle={onToggle}
                onOpen={onOpen}
                edit={edit}
              />
            )}
          </div>
        );
      })}
      {dir.truncated && (
        <span className="py-1 text-xs text-ink-4" style={{ paddingLeft: 30 + depth * 16 }}>
          只列了前 5000 项
        </span>
      )}
    </>
  );
}

/** 代码视图的行高 / 上内边距（和下面 leading-[21px]、py-3 对应）：跳行时按它算位置。 */
const LINE_PX = 21;
const CODE_TOP_PX = 12;

/** 正在编辑：打开时的内容和修改时间（保存时比对），有没有改，撞没撞上别人的修改。 */
interface Editing {
  /** text = 文本编辑器；viewer = 在预览器里改（表格），保存交给预览器。 */
  kind: "text" | "viewer";
  initial: string;
  base?: number;
  /** 取编辑器里的全文（第一次改动时拿到）。 */
  read: (() => string) | null;
  dirty: boolean;
  /** stale = 保存被拦（期间被改过）；changed = 编辑时发现文件被改了。 */
  conflict: "stale" | "changed" | null;
  saving: boolean;
}

function Preview({
  sessionId,
  canEdit,
  editRequest,
  dirtyRef,
  guard,
  onSaved,
  onReload,
  file,
  selected,
  jump,
  changed,
  onMention,
  onReveal,
  onOpenExternal,
}: {
  sessionId: string;
  canEdit: boolean;
  /** 新建完文件：对上了就直接进编辑。 */
  editRequest: { path: string; seq: number } | null;
  /** 有没保存的修改时，宿主拿它问「要不要保存」。 */
  dirtyRef: MutableRefObject<Dirty>;
  guard: (run: () => void) => void;
  onSaved: (file: FileView) => void;
  onReload: () => void;
  file: FileView | { kind: "loading" } | { kind: "error"; message: string } | null;
  selected: string | null;
  /** 要跳到的行：和正在看的文件对上才生效。 */
  jump: { path: string; line: number; seq: number } | null;
  changed: boolean;
  onMention: (path: string) => void;
  onReveal?: () => void;
  /** 用系统默认应用打开（只在连本机 Dock 时有：远程的文件不在这台机器上）。 */
  onOpenExternal?: () => void;
}) {
  const [mode, setMode] = useState<"source" | "preview">("source");
  const codeRef = useRef<HTMLDivElement>(null);
  const { codeTheme } = usePrefs();
  const text = file?.kind === "text" ? file.text : "";
  const lines = file?.kind === "text" ? text.replace(/\n$/, "").split("\n") : [];
  const jumpLine = jump && jump.path === selected && file?.kind === "text" ? Math.min(jump.line, lines.length) : null;
  const type = fileType(selected?.slice(selected.lastIndexOf("/") + 1) ?? "");
  const previewKind = selected ? previewable(selected) : null;
  // 表格 / 文档：功能目录登记的预览器（features/file-preview）。
  const previewer = selected ? filePreviewFor(selected.slice(selected.lastIndexOf("/") + 1)) : undefined;
  /** 预览器报的底栏文字（行数、已读多少）。 */
  const [status, setStatus] = useState<string | null>(null);
  // 太大的文件不高亮：几千行的 token 一次画出来会卡。
  const tooBig = text.length > HIGHLIGHT_MAX_BYTES || lines.length > HIGHLIGHT_MAX_LINES;
  const hl = useHighlight(file?.kind === "text" && !tooBig && type.lang !== "text" ? text : "", type.lang, codeTheme);
  const tokens = hl && hl.code === text && text ? hl : null;
  const editor = fileEditorEntry();
  const editable = canEdit && !!editor && file?.kind === "text" && !file.truncated;
  const [edit, setEdit] = useState<Editing | null>(null);
  const editRef = useRef(edit);
  editRef.current = edit;
  // 换文件：能预览的（Markdown / SVG / 表格 / 文档）默认看预览；要跳行的看源码（跳行那边会切）。
  useLayoutEffect(() => {
    setMode(previewKind || previewer ? "preview" : "source");
    setStatus(null);
    setEdit(null);
  }, [selected]);
  // 打开文本文件就把编辑器的代码取来，点「编辑」不用等。
  useEffect(() => {
    if (editable) editor?.preload();
  }, [editable, editor]);

  // ---- 在预览器里改（表格） ----
  /** 预览器能改、正在看预览：「编辑」改的是表格，不是源码。 */
  const tableMode =
    canEdit && !!previewer?.editable && (file?.kind === "text" || file?.kind === "binary") && (previewer.binary || mode === "preview");
  /** 预览器说能不能改（undefined = 还没说）。 */
  const [viewerBlock, setViewerBlock] = useState<string | null | undefined>(undefined);
  const viewerSave = useRef<((force: boolean) => Promise<SavedFile>) | null>(null);
  /** 换了它预览器就重来（丢掉没保存的改动、读新内容）；自己存的不算。 */
  const [viewerNonce, setViewerNonce] = useState(0);
  const ownSave = useRef<string | null>(null);
  const metaKey = file && "size" in file ? `${file.size}:${file.modifiedMs ?? ""}` : "";
  const lastMeta = useRef(metaKey);
  useEffect(() => {
    if (metaKey === lastMeta.current) return;
    lastMeta.current = metaKey;
    if (ownSave.current === metaKey) return;
    // 文件被别人改了：没在改（或没改过）就重新读；改过的由下面的冲突提示让人选。
    if (editRef.current?.kind === "viewer" && editRef.current.dirty) return;
    setViewerNonce((n) => n + 1);
  }, [metaKey]);
  useLayoutEffect(() => {
    setViewerBlock(undefined);
    viewerSave.current = null;
    ownSave.current = null;
  }, [selected]);
  const viewerEdit = useMemo(
    () => ({
      onDirty: (dirty: boolean) => setEdit((e) => (e && e.kind === "viewer" && e.dirty !== dirty ? { ...e, dirty } : e)),
      bind: (save: ((force: boolean) => Promise<SavedFile>) | null) => (viewerSave.current = save),
      onEditable: (reason: string | null) => setViewerBlock(reason),
    }),
    [],
  );

  const startEdit = () => {
    if (tableMode) {
      if (viewerBlock !== null) return;
      setEdit({ kind: "viewer", initial: "", base: file && "size" in file ? file.modifiedMs : undefined, read: null, dirty: false, conflict: null, saving: false });
      return;
    }
    if (file?.kind !== "text") return;
    setEdit({ kind: "text", initial: file.text, base: file.modifiedMs, read: null, dirty: false, conflict: null, saving: false });
  };
  // 新建完的文件直接进编辑（每个请求只认一次）。
  const handledRequest = useRef(0);
  useEffect(() => {
    if (!editRequest || editRequest.seq === handledRequest.current) return;
    if (editRequest.path !== selected || file?.kind !== "text") return;
    handledRequest.current = editRequest.seq;
    startEdit();
  }, [editRequest?.seq, selected, file]);

  // 编辑时文件被改了（agent 写了它）：没改过就换成新内容，改过就提示，让人选。
  useEffect(() => {
    const cur = editRef.current;
    if (!cur || cur.saving || !file || !("size" in file) || file.modifiedMs === cur.base) return;
    if (cur.kind === "viewer") {
      if (ownSave.current === metaKey) return;
      if (!cur.dirty) setEdit({ ...cur, base: file.modifiedMs });
      else if (!cur.conflict) setEdit({ ...cur, conflict: "changed" });
      return;
    }
    if (file.kind !== "text") return;
    if (!cur.dirty) setEdit({ ...cur, initial: file.text, base: file.modifiedMs, read: null });
    else if (!cur.conflict) setEdit({ ...cur, conflict: "changed" });
  }, [file]);

  const save = async (force = false): Promise<boolean> => {
    const cur = editRef.current;
    if (!cur || !selected) return false;
    if (cur.kind === "viewer") {
      const run = viewerSave.current;
      if (!run || !file || !("size" in file)) return false;
      setEdit((e) => e && { ...e, saving: true });
      try {
        const saved = await run(force);
        ownSave.current = `${saved.size}:${saved.modifiedMs ?? ""}`;
        setEdit((e) => e && { ...e, base: saved.modifiedMs, conflict: null, saving: false });
        onSaved({ ...file, size: saved.size, modifiedMs: saved.modifiedMs } as FileView);
        // 宿主那份正文（源码视图用）也换成新的；修改时间是自己存的，预览器不重来。
        onReload();
        return true;
      } catch (err) {
        setEdit((e) => e && { ...e, saving: false, conflict: isConflict(err) ? "stale" : e.conflict });
        if (!isConflict(err)) store.reportError(new Error(editError(err)));
        return false;
      }
    }
    const text = cur.read ? cur.read() : cur.initial;
    setEdit((e) => e && { ...e, saving: true });
    try {
      const saved = await saveFile(sessionId, selected, text, force ? undefined : cur.base);
      // 保存期间又敲了字：还算没保存。
      const now = editRef.current?.read?.();
      setEdit(
        (e) =>
          e && { ...e, base: saved.modifiedMs, dirty: now !== undefined && now !== text, conflict: null, saving: false },
      );
      onSaved({ kind: "text", path: saved.path, size: saved.size, modifiedMs: saved.modifiedMs, text, truncated: false });
      return true;
    } catch (err) {
      setEdit((e) => e && { ...e, saving: false, conflict: isConflict(err) ? "stale" : e.conflict });
      if (!isConflict(err)) store.reportError(new Error(editError(err)));
      return false;
    }
  };
  // 宿主问「要不要保存」时用。
  useEffect(() => {
    dirtyRef.current =
      edit?.dirty && selected
        ? {
            name: baseName(selected),
            save: () => save(),
            // 表格的改动在预览器里：重来一遍才丢得掉。文本的改动随编辑状态一起没了。
            discard: () => edit.kind === "viewer" && setViewerNonce((n) => n + 1),
          }
        : null;
  });
  // ⌘S：表格的格子不是编辑器，键落在面板上；编辑器自己处理过的（defaultPrevented）不再存一遍。
  useEffect(() => {
    if (!edit) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.defaultPrevented || !(e.metaKey || e.ctrlKey) || e.key.toLowerCase() !== "s") return;
      e.preventDefault();
      void save();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [!!edit]);
  const discard = () => {
    dirtyRef.current = null;
    if (editRef.current?.kind === "viewer") setViewerNonce((n) => n + 1);
    setEdit(null);
    onReload();
  };
  // 跳行：切回源码视图，那一行滚到上三分之一处，底色标出来。
  useLayoutEffect(() => {
    if (!jumpLine) return;
    if (mode !== "source") return setMode("source");
    const el = codeRef.current;
    if (el) el.scrollTop = Math.max(0, CODE_TOP_PX + (jumpLine - 1) * LINE_PX - el.clientHeight / 3);
  }, [jumpLine, jump?.seq, mode]);
  if (!selected || !file)
    return <Empty icon={FileText} title="选一个文件" text="左边点文件看内容，或按 ⌘P 按名字找。" />;
  const slash = selected.lastIndexOf("/");
  const dir = slash >= 0 ? selected.slice(0, slash) : "";
  const name = selected.slice(slash + 1);
  // 文本格式（Markdown / SVG / CSV）才有「源码 / 预览」切换；二进制格式只有预览。
  const canPreview = (!!previewKind || (!!previewer && !previewer.binary)) && file.kind === "text";
  // 预览器：二进制格式总是它；CSV 选了预览，或者宿主读不出文本（GBK 之类）时也交给它按编码读。
  const viewer =
    previewer && (file.kind === "text" || file.kind === "binary") && (previewer.binary || file.kind === "binary" || mode === "preview")
      ? previewer
      : null;
  const bg = tokens?.bg || "#ffffff";
  const conflictBar = edit?.conflict ? (
    <div className="enter-fade flex shrink-0 items-center gap-2 border-b border-[#ffd60a66] bg-[#fff8e1] px-4 py-2 text-xs text-[#7a5c00]">
      <span className="min-w-0 flex-1">
        {edit.conflict === "stale"
          ? "没保存：文件在你打开之后被改过（可能是 agent）。"
          : "这个文件刚被改过（可能是 agent），你的修改还没保存。"}
      </span>
      <button type="button" onClick={discard} className="shrink-0 rounded-md px-2 py-1 hover:bg-[#ffd60a33]">
        放弃我的修改
      </button>
      <button
        type="button"
        onClick={() => void save(true)}
        className="shrink-0 rounded-md bg-[#7a5c00] px-2 py-1 font-medium text-white hover:bg-[#5e4700]"
      >
        覆盖保存
      </button>
    </div>
  ) : null;

  return (
    <div className="@container flex min-w-0 flex-1 flex-col bg-white">
      <div className="flex h-11 shrink-0 items-center gap-2 border-b border-line-soft px-4">
        <TypeIcon name={name} />
        {/* 窄的时候先让目录让位，文件名最后才截断。 */}
        {dir && (
          <span className="hidden min-w-0 shrink-[2] truncate text-[13px] text-ink-4 @[560px]:inline">
            {dir.split("/").join(" / ")} /
          </span>
        )}
        <span className="min-w-0 shrink truncate text-[13px] font-semibold text-ink" title={selected}>
          {name}
        </span>
        {changed && (
          <span className="flex h-5 shrink-0 items-center rounded-full bg-[#eaf3ff] px-2 text-[11px] font-medium text-[#0a6fd6]">
            Agent 改过
          </span>
        )}
        <div className="flex-1" />
        {edit && (
          <>
            <span
              className={`flex shrink-0 items-center gap-1.5 text-[11.5px] transition-colors duration-[var(--dur-fast)] ${
                edit.dirty ? "text-ink-3" : "text-ink-5"
              }`}
            >
              <span className={`size-1.5 rounded-full ${edit.dirty ? "bg-[#ff9f0a]" : "bg-[#34c759]"}`} />
              {edit.saving ? "正在保存…" : edit.dirty ? "未保存" : "已保存"}
            </span>
            <button
              type="button"
              onClick={() => guard(() => setEdit(null))}
              className="motion-colors flex h-7 shrink-0 items-center rounded-[7px] px-2.5 text-xs text-ink-2 hover:bg-hover"
            >
              完成
            </button>
            <button
              type="button"
              disabled={!edit.dirty || edit.saving}
              onClick={() => void save()}
              title="保存（⌘S）"
              className="motion-colors flex h-7 shrink-0 items-center gap-1.5 rounded-[7px] bg-accent px-2.5 text-xs font-medium text-white hover:bg-[#0a74e0] disabled:bg-fill disabled:text-ink-4"
            >
              保存
              <span className="font-mono text-[10.5px] opacity-70">⌘S</span>
            </button>
            <div className="h-4 w-px shrink-0 bg-line-strong" />
          </>
        )}
        {!edit && (tableMode ? canEditFiles() : editable) && (
          <button
            type="button"
            onClick={startEdit}
            disabled={tableMode && viewerBlock !== null}
            title={
              tableMode && viewerBlock
                ? viewerBlock
                : tableMode
                  ? "在面板里改表格（⌘S 保存，⌘Z 撤销）"
                  : "在面板里改这个文件（⌘S 保存）"
            }
            className="motion-colors flex h-7 shrink-0 items-center gap-1 rounded-[7px] border border-line-strong px-2 hover:bg-hover disabled:cursor-default disabled:opacity-45 disabled:hover:bg-transparent"
          >
            <Pencil size={13} className="text-ink-2" />
            <span className="text-xs font-medium text-ink">编辑</span>
          </button>
        )}
        {!edit && canPreview && (
          <div className="flex shrink-0 items-center rounded-lg bg-fill p-0.5">
            {(["source", "preview"] as const).map((m) => (
              <button
                key={m}
                type="button"
                onClick={() => setMode(m)}
                className={`motion-colors rounded-md px-2.5 py-0.5 text-xs ${
                  mode === m ? "bg-white font-medium text-ink shadow-[0px_1px_2px_rgba(0,0,0,0.08)]" : "text-ink-3"
                }`}
              >
                {m === "source" ? "源码" : "预览"}
              </button>
            ))}
          </div>
        )}
        {!edit && (canPreview || editable || tableMode) && <div className="h-4 w-px shrink-0 bg-line-strong" />}
        <button
          type="button"
          onClick={() => onMention(selected)}
          title="把路径插到输入框里（不发送）"
          className="motion-colors flex h-7 shrink-0 items-center gap-1 rounded-[7px] border border-line-strong px-2 hover:bg-hover"
        >
          <AtSign size={14} className="text-ink-2" />
          <span className="hidden text-xs font-medium text-ink @[480px]:inline">插入输入框</span>
        </button>
        <IconButton
          icon={Copy}
          label="复制路径"
          size={14}
          onClick={() => void navigator.clipboard.writeText(selected)}
          className="size-7 text-ink-4"
        />
        {onOpenExternal && (
          <IconButton icon={AppWindow} label="用默认应用打开" size={14} onClick={onOpenExternal} className="size-7 text-ink-4" />
        )}
        {onReveal && <IconButton icon={SquareArrowOutUpRight} label="在访达中显示" size={14} onClick={onReveal} className="size-7 text-ink-4" />}
      </div>

      {file.kind === "loading" ? (
        <div className="flex flex-1 items-center justify-center text-xs text-ink-4">正在读…</div>
      ) : file.kind === "error" ? (
        <Empty icon={FileText} title="读不了这个文件" text={file.message} />
      ) : edit?.kind === "text" && editor ? (
        <div className="flex min-h-0 flex-1 flex-col">
          {edit.conflict && conflictBar}
          <editor.Editor
            path={selected}
            initial={edit.initial}
            line={jumpLine}
            onChange={(read) => setEdit((e) => (e && !e.dirty ? { ...e, dirty: true, read } : e))}
            onSave={() => void save()}
          />
        </div>
      ) : file.kind === "image" ? (
        <div className="flex min-h-0 flex-1 items-center justify-center bg-[#f2f2f4] p-6">
          <img
            src={`data:${file.mime};base64,${file.data}`}
            alt={name}
            className="enter-fade max-h-full max-w-full rounded-lg object-contain shadow-[0px_8px_24px_rgba(0,0,0,0.12)]"
          />
        </div>
      ) : viewer && (file.kind === "text" || file.kind === "binary") ? (
        <div className="flex min-h-0 flex-1 flex-col">
          {edit?.kind === "viewer" && edit.conflict && conflictBar}
          <viewer.Viewer
            // 文件被别人改了（或放弃了修改）才重来；自己存的、agent 改了别的文件时宿主的静默重读不打断。
            key={`${file.path}:${viewerNonce}`}
            edit={previewer?.editable ? { active: edit?.kind === "viewer", ...viewerEdit } : undefined}
            threadId={sessionId}
            file={{
              path: file.path,
              size: file.size,
              modifiedMs: file.modifiedMs,
              ...(file.kind === "text" ? { text: file.text, truncated: file.truncated } : {}),
            }}
            onStatus={setStatus}
          />
        </div>
      ) : file.kind === "binary" ? (
        <Empty
          icon={Archive}
          title="不能预览这种文件"
          text={`${name} 是二进制文件（${formatSize(file.size)}）。${
            onOpenExternal ? "可以用默认应用打开，或者" : "可以"
          }插入输入框，让 agent 用工具读它。`}
          action={onOpenExternal && { label: "用默认应用打开", run: onOpenExternal }}
        />
      ) : canPreview && mode === "preview" && previewKind === "markdown" ? (
        <div className="thin-scroll min-h-0 flex-1 overflow-y-auto px-8 py-6 [--chat-text:14.5px]">
          <div className="mx-auto max-w-[760px]">
            <Markdown text={file.text} />
          </div>
        </div>
      ) : canPreview && mode === "preview" && previewKind === "svg" ? (
        <div className="flex min-h-0 flex-1 items-center justify-center bg-[#f2f2f4] bg-[conic-gradient(#e8e8ec_25%,transparent_0_50%,#e8e8ec_0_75%,transparent_0)] bg-[length:16px_16px] p-6">
          <img
            src={`data:image/svg+xml;charset=utf-8,${encodeURIComponent(file.text)}`}
            alt={name}
            className="enter-fade max-h-full max-w-full object-contain"
          />
        </div>
      ) : (
        <div
          ref={codeRef}
          className="thin-scroll relative flex min-h-0 flex-1 overflow-auto py-3 font-mono text-[12.5px] leading-[21px] transition-colors duration-[var(--dur-base)]"
          style={{ background: bg }}
        >
          {jumpLine && (
            <div
              key={jump?.seq}
              className="enter-fade pointer-events-none absolute inset-x-0 bg-[#0a84ff17]"
              style={{ top: CODE_TOP_PX + (jumpLine - 1) * LINE_PX, height: LINE_PX }}
            />
          )}
          <pre
            className="sticky left-0 z-[1] w-12 shrink-0 select-none pr-3 text-right text-[12px] text-ink-5"
            style={{ background: bg }}
          >
            {lines.map((_, i) => i + 1).join("\n")}
          </pre>
          <pre className="min-w-0 flex-1 whitespace-pre pr-6 text-ink">
            {tokens ? <Tokens lines={tokens.lines} tail="" /> : file.text}
          </pre>
        </div>
      )}

      {file.kind !== "loading" && file.kind !== "error" && (
        <footer className="flex h-[30px] shrink-0 items-center gap-2 border-t border-line-soft px-4 text-[11px] text-ink-4">
          <span className="truncate">
            {viewer && status
              ? status
              : file.kind === "text"
              ? `${type.label} · ${lines.length} 行 · ${formatSize(file.size)}${file.truncated ? " · 只显示前 512 KB" : ""}${
                  tooBig && type.lang !== "text" ? " · 文件太大，没有高亮" : ""
                }`
              : file.kind === "image"
                ? `${file.mime.replace("image/", "").toUpperCase()} · ${formatSize(file.size)}`
                : formatSize(file.size)}
          </span>
          <span className="flex-1" />
          {file.modifiedMs ? (
            <span>
              改于 {ago(file.modifiedMs)} · {edit ? "编辑中" : "只读"}
            </span>
          ) : (
            <span>{edit ? "编辑中" : "只读"}</span>
          )}
        </footer>
      )}
    </div>
  );
}

/** ⌘P：按名字模糊找。↑↓ 选，↵ 打开，⌘↵ 插入输入框，Esc 关。 */
function QuickOpen({
  sessionId,
  initialQuery = "",
  onClose,
  onOpen,
  onMention,
}: {
  sessionId: string;
  /** 预先填好的查询（对话流里点的路径没找准时，填文件名）。 */
  initialQuery?: string;
  onClose: () => void;
  onOpen: (path: string) => void;
  onMention: (path: string) => void;
}) {
  const [query, setQuery] = useState(initialQuery);
  const [paths, setPaths] = useState<string[]>([]);
  const [pick, setPick] = useState(0);

  useEffect(() => {
    if (!query.trim()) {
      setPaths([]);
      return;
    }
    let alive = true;
    const t = window.setTimeout(() => {
      dock
        .call<{ paths: string[] }>("fs/find", { threadId: sessionId, query, limit: 30 })
        .then((r) => alive && (setPaths(r.paths), setPick(0)))
        .catch(() => alive && setPaths([]));
    }, 120);
    return () => {
      alive = false;
      window.clearTimeout(t);
    };
  }, [query, sessionId]);

  return (
    <div className="enter-fade absolute inset-x-0 bottom-0 top-11 z-20 bg-black/[0.04]" onMouseDown={onClose}>
      <div
        className="glass-popover pop-in absolute left-1/2 top-6 flex w-[min(480px,calc(100%-32px))] -translate-x-1/2 flex-col rounded-xl border border-line-strong bg-white shadow-[0px_16px_40px_rgba(0,0,0,0.14)]"
        onMouseDown={(e) => e.stopPropagation()}
      >
        <div className="flex h-11 items-center gap-2 border-b border-line-soft px-3.5">
          <Search size={16} className="shrink-0 text-ink-4" />
          <input
            autoFocus
            value={query}
            placeholder="按名字找文件"
            spellCheck={false}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => {
              if (imeComposing(e)) return;
              if (e.key === "Escape") onClose();
              else if (e.key === "ArrowDown") {
                e.preventDefault();
                setPick((p) => Math.min(p + 1, paths.length - 1));
              } else if (e.key === "ArrowUp") {
                e.preventDefault();
                setPick((p) => Math.max(p - 1, 0));
              } else if (e.key === "Enter" && paths[pick]) {
                e.preventDefault();
                if (e.metaKey) onMention(paths[pick]);
                else onOpen(paths[pick]);
              }
            }}
            className="min-w-0 flex-1 bg-transparent text-[14px] text-ink outline-none placeholder:text-ink-5"
          />
          <span className="font-mono text-[11px] text-ink-4">esc</span>
        </div>
        {paths.length > 0 && (
          <div className="thin-scroll flex max-h-[320px] flex-col gap-0.5 overflow-y-auto p-1.5">
            {paths.map((p, i) => {
              const name = p.slice(p.lastIndexOf("/") + 1);
              return (
                <button
                  key={p}
                  type="button"
                  onMouseEnter={() => setPick(i)}
                  onClick={() => onOpen(p)}
                  className={`flex h-9 shrink-0 items-center gap-2.5 rounded-lg px-2.5 text-left ${i === pick ? "bg-[#eaf3ff]" : ""}`}
                >
                  <TypeIcon name={name} />
                  <span className="shrink-0 text-[13px] font-medium text-ink">{name}</span>
                  <span className="min-w-0 flex-1 truncate text-xs text-ink-4">{p}</span>
                  {i === pick && <span className="font-mono text-[11px] text-ink-4">↵</span>}
                </button>
              );
            })}
          </div>
        )}
        {query.trim() && paths.length === 0 && <div className="px-4 py-3 text-xs text-ink-4">没找到</div>}
        <div className="flex h-8 items-center border-t border-line-soft px-3.5">
          <span className="text-[11px] text-ink-4">↑↓ 选择 · ↵ 打开 · ⌘↵ 插入输入框</span>
        </div>
      </div>
    </div>
  );
}

function Empty({
  icon: Icon,
  title,
  text,
  action,
}: {
  icon: typeof Folder;
  title: string;
  text: string;
  action?: { label: string; run: () => void };
}) {
  return (
    <div className="enter-fade flex min-w-0 flex-1 flex-col items-center justify-center gap-3 bg-pane px-10">
      <div className="flex size-12 items-center justify-center rounded-full border border-line-strong bg-white">
        <Icon size={20} className="text-ink-3" />
      </div>
      <span className="text-[15px] font-semibold text-ink">{title}</span>
      <span className="max-w-[340px] text-center text-[13px] leading-[1.55] text-ink-3">{text}</span>
      {action && (
        <button type="button" onClick={action.run} className="text-[13px] font-medium text-[#0a6fd6] hover:underline">
          {action.label}
        </button>
      )}
    </div>
  );
}

/** 这个会话里 agent 用写文件工具改过（且成功）的文件，换算成相对工作区根的路径；
 * `writes` 是这些写的工具项 id（每写一次多一条）。 */
function agentChanges(thread: ThreadState, root: string | null): { paths: Set<string>; writes: string[] } {
  const out = new Set<string>();
  const writes: string[] = [];
  for (const turn of thread.turns) {
    for (const item of turn.items) {
      if (item.kind !== "tool" || item.status !== "completed") continue;
      const keys = WRITE_TOOLS[item.name];
      if (!keys) continue;
      const raw = keys.map((k) => item.arguments[k]).find((v): v is string => typeof v === "string" && !!v.trim());
      if (!raw) continue;
      let path = raw.trim().replace(/^\.\//, "");
      if (path.startsWith("/")) {
        if (!root || !path.startsWith(`${root}/`)) continue;
        path = path.slice(root.length + 1);
      }
      out.add(path);
      writes.push(item.id);
    }
  }
  return { paths: out, writes };
}

/** 宿主自己能预览的文件：Markdown 排版、SVG 看图（表格 / 文档走登记的预览器）。 */
function previewable(path: string): "markdown" | "svg" | null {
  const ext = path.slice(path.lastIndexOf(".") + 1).toLowerCase();
  if (ext === "md" || ext === "markdown" || ext === "mdx") return "markdown";
  if (ext === "svg") return "svg";
  return null;
}

const HIGHLIGHT_MAX_BYTES = 300 * 1024;
const HIGHLIGHT_MAX_LINES = 6000;

function TypeIcon({ name }: { name: string }) {
  const t = fileType(name);
  const Icon = t.icon;
  return <Icon size={14} className="shrink-0" style={{ color: t.color }} />;
}

function FolderIcon({ open }: { open: boolean }) {
  const Icon = open ? FolderOpen : Folder;
  return <Icon size={14} className="shrink-0" style={{ color: FOLDER_COLOR }} fill={`${FOLDER_COLOR}26`} />;
}

function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

function ago(ms: number): string {
  const s = Math.max(0, (Date.now() - ms) / 1000);
  if (s < 60) return "刚刚";
  if (s < 3600) return `${Math.floor(s / 60)} 分钟前`;
  if (s < 86400) return `${Math.floor(s / 3600)} 小时前`;
  return new Date(ms).toLocaleDateString();
}

function tildify(path: string): string {
  const m = /^\/Users\/[^/]+/.exec(path) ?? /^\/home\/[^/]+/.exec(path);
  return m ? `~${path.slice(m[0].length)}` : path;
}

function message(e: unknown): string {
  const text = e instanceof Error ? e.message : String(e);
  return text.replace(/^fs\/\w+: /, "");
}

/**
 * 对话流里写的路径在会话目录里对应哪个文件：原样能读就是它；否则按文件名找，
 * 结尾对得上的（模型写的是从子目录起的路径）或同名的只有一个就是它。找不准返回 `null`。
 */
async function locate(threadId: string, path: string): Promise<string | null> {
  try {
    await dock.call("fs/read", { threadId, path, maxBytes: 1 });
    return path;
  } catch {
    // 不在这儿（或是目录）：按名字找。
  }
  const name = path.slice(path.lastIndexOf("/") + 1);
  try {
    const { paths } = await dock.call<{ paths: string[] }>("fs/find", { threadId, query: name, limit: 50 });
    const tail = paths.filter((p) => p === path || p.endsWith(`/${path}`));
    if (tail.length === 1) return tail[0];
    const named = paths.filter((p) => p.slice(p.lastIndexOf("/") + 1) === name);
    if (tail.length === 0 && named.length === 1) return named[0];
  } catch {
    // 找不了就交给快速打开。
  }
  return null;
}
