// 开发用：不连 Dock，用假数据摆出组件的各个状态，给无头浏览器截图对照设计稿。
// 打开 http://localhost:1420/preview.html（`npm run tauri dev` 或 `npx vite` 起的服务）。
// 不进打包：vite build 只认 index.html。

import React, { useState } from "react";
import { createRoot } from "react-dom/client";
import { SplitDivider } from "../components/SplitDivider";
import type { PendingItem } from "dock-core";
import { Pending, type PendingActions } from "../components/Pending";
import type { AppState } from "../dock/store";
import { ConnectionBanner } from "../components/ConnectionBanner";
import { LaunchSplash } from "../components/LaunchSplash";
import { Sidebar, SessionRow, FloatingWindowControls, WindowControls, type SidebarProps } from "../components/Sidebar";
import { NotificationMenu } from "../components/NotificationMenu";
import { Transcript } from "../components/Transcript";
import { Composer } from "../components/Composer";
import { GoalContextPreview } from "./goalContext";
import { ToolCard } from "../components/ToolCard";
import { Queued } from "../components/Queued";
import { DeleteConfirm, RenameInput, SessionMenu } from "../components/SessionMenu";
import { PresetStudioPreview, isPresetPreview } from "./presetStudio";
import { SettingsPreview, isSettingsPreview } from "./settings";
import { NavPagePreview, isNavPagePreview } from "./navPages";
import { PluginViewsPreview } from "./pluginViews";
import { FilesLinkPreview } from "./filesLink";
import { ToolPanelPreview, isToolPanelPreview } from "./toolPanel";
import { ToolFixturePreview, isToolFixturePreview } from "./toolFixtures";
import "../styles.css";
import "../features";

const noop: PendingActions = { onPermission() {}, onAnswer() {}, onPlan() {}, onElicit() {}, onElicitOpen() {} };

const colors = {
  id: "q-color",
  question: "喜欢哪种颜色？",
  multiSelect: false,
  options: [
    { label: "红", description: "红色" },
    { label: "绿", description: "绿色" },
    { label: "蓝", description: "蓝色" },
  ],
};
const langs = {
  id: "q-lang",
  question: "你会哪些语言？",
  multiSelect: true,
  options: [
    { label: "Rust", description: "Rust" },
    { label: "TypeScript", description: "TypeScript" },
    { label: "Go", description: "Go" },
  ],
};

const longPlan = Array.from({ length: 6 }, (_, i) =>
  [`## 第 ${i + 1} 阶段`, "- 重构 `SessionStore`，按会话分片", "- 补上回归测试", "- 跑 `cargo test -p cordis-spine`"].join("\n"),
).join("\n\n");

const LONG_FIND =
  "cd dock-gui && find src src-tauri/src src-tauri/Cargo.toml wonder-ui -type f \\( -name '*.ts' -o -name '*.tsx' -o -name '*.rs' -o -name '*.json' -a ! -name 'package*.json' \\) ! -path '*/node_modules/*' ! -path '*/dist/*' ! -path '*/target/*' | wc -l";

/** `#bash-description`：bash 卡有 / 没有描述、收起 / 展开、运行中、后台。 */
function BashDescriptionPreview() {
  const described = { command: LONG_FIND, description: "Count source files excluding build output" };
  const cases: [string, Record<string, unknown>, string, "completed" | "running" | "failed", boolean][] = [
    ["described · collapsed", described, "412", "completed", false],
    ["described · open", described, "412", "completed", true],
    ["described · running", { command: "cargo test -p cordis-tui", description: "Run the TUI tests" }, "", "running", false],
    ["described · failed", { command: "cargo clippy -p cordis-tui -- -D warnings", description: "Lint the TUI crate" }, "exit 101\nerror: unused import", "failed", false],
    ["described · background", { command: "npm run dev", description: "Start the dev server", is_background: true }, "[Command moved to background]\n\njob_id: job-3", "completed", false],
    ["long description", { command: "git status -sb", description: "Show the working tree status including untracked files and the upstream tracking branch, then summarize" }, "## main", "completed", false],
    ["no description", { command: "git status -sb" }, "## main...origin/main", "completed", false],
  ];
  return (
    <div className="flex w-[760px] flex-col gap-4 bg-pane p-6">
      {cases.map(([name, args, out, status, open]) => (
        <section key={name} data-preview={name} className="flex flex-col gap-0.5">
          <div className="font-mono text-[11px] text-ink-4">{name}</div>
          <ToolCard tool="bash" args={args} output={out} status={status} startedAt={Date.now() - 2300} endedAt={status === "running" ? null : Date.now()} initialOpen={open} />
        </section>
      ))}
    </div>
  );
}

const items: [string, PendingItem][] = [
  // 摘要格式照 Dock spine 的 permission_summary：命令工具是 `$ 命令`，有描述另起一行。
  ["permission", { kind: "permission", id: "p", toolName: "bash", summary: "$ npm run build", agentId: null, decision: null, always: false }],
  [
    "permission described",
    {
      kind: "permission",
      id: "pd",
      toolName: "bash",
      summary: `$ ${LONG_FIND}\nCount source files excluding build output`,
      agentId: null,
      decision: null,
      always: false,
    },
  ],
  [
    "permission from subagent",
    { kind: "permission", id: "ps", toolName: "bash", summary: "$ cargo test -p nova-gateway\nRun gateway tests", agentId: "kid-1", decision: null, always: false },
  ],
  ["question single", { kind: "question", id: "q1", questions: [colors], resolved: false } as PendingItem],
  ["question two", { kind: "question", id: "q2", questions: [langs, colors], resolved: false } as PendingItem],
  [
    "plan short",
    {
      kind: "plan",
      id: "pl",
      path: ".dock/plans/refactor-store.md",
      body: "# 重构计划：优化状态管理与会话缓存\n\n- 抽象 SessionStore trait\n- 将粗粒度锁换成并发容器 `SessionCache::new()`\n- 执行门禁 `cargo test -p cordis-spine`",
      empty: false,
      decision: null,
    },
  ],
  ["plan long", { kind: "plan", id: "pl2", path: ".dock/plans/full-refactor.md", body: longPlan, empty: false, decision: null }],
  ["plan empty", { kind: "plan", id: "pl3", path: ".dock/plans/empty.md", body: "", empty: true, decision: null }],
  ["elicit", { kind: "elicit", id: "e", server: "linear", heading: "", message: "将在 Linear 创建 issue：修复登录页样式（团队 WEB）", resolved: false, url: null, urlDomain: null, urlWarning: null }],
];

const app = (patch: Partial<AppState>) => ({ connection: "open", reconnects: 0, exited: null, error: null, ...patch }) as AppState;
const rows: [string, Partial<Parameters<typeof SessionRow>[0]>][] = [
  ["重构 nova-gateway 会话缓存", { active: true, open: true }],
  ["优化缓存预热逻辑", { running: true }],
  ["修复登录页样式", { waiting: true }],
  ["修复登录页样式（选中）", { waiting: true, active: true }],
  ["更新文档和示例", { finished: "completed" }],
  ["只回复 hi", { finished: "failed" }],
  ["普通会话", {}],
];

const railApp = { ...app({}), waiting: new Set<string>(), finished: new Map(), sessions: [] } as unknown as AppState;
const railProps = {
  groups: [
    { id: "coding", label: "编码", projects: [] },
    { id: "minimal", label: "极简", projects: [] },
  ],
  presets: [
    { id: "coding", label: "编码", available: true },
    { id: "minimal", label: "极简", available: true },
  ],
  newChatPreset: "coding",
  activeSessionId: null,
  userName: "me",
  notifications: (rail: boolean) => <NotificationMenu app={railApp} onOpen={() => {}} rail={rail} />,
  onNewChat() {},
  onToggleSidebar() {},
} as unknown as SidebarProps;

/** `#collapsed`：侧栏收起 / 展开来回切（和 App 同一套外壳写法：宽度过渡到 0 + 固定按钮）。 */
function CollapsedPreview() {
  const [collapsed, setCollapsed] = useState(true);
  const props = {
    ...railProps,
    expanded: new Set<string>(),
    runningIds: new Set<string>(),
    waitingIds: new Set<string>(),
    finished: new Map<string, { status: string }>(),
    onSearch() {},
    onToggle() {},
    onSetNewChatPreset() {},
    onToggleSidebar: () => setCollapsed((v) => !v),
  } as unknown as SidebarProps;
  return (
    <div className="flex h-[420px] w-[900px] bg-pane">
      <div
        inert={collapsed}
        aria-hidden={collapsed || undefined}
        className={`shrink-0 overflow-hidden transition-[width] duration-[var(--dur-slow)] ease-[var(--ease-out)] ${
          collapsed ? "w-0" : "w-[248px]"
        }`}
      >
        <Sidebar {...props} />
      </div>
      {collapsed && <FloatingWindowControls {...props} />}
      <div className="flex min-w-0 flex-1 flex-col">
        <div className="flex h-11 shrink-0 items-center gap-2.5 border-b border-line-soft px-4">
          {collapsed && (
            <div className="invisible ml-1 mr-1 shrink-0" aria-hidden>
              <WindowControls {...props} collapsed />
            </div>
          )}
          <span className="truncate text-sm font-semibold text-ink">新对话</span>
        </div>
      </div>
    </div>
  );
}

const sess = (id: string, title: string) => ({ id, title, projectId: "p", preset: "编码", presetId: "coding", cwd: "/tmp", open: false, updatedAt: 0 });
/** `#search` / `#search-empty`：侧栏搜索。 */
function SearchPreview({ query }: { query: string }) {
  setTimeout(() => {
    const el = document.querySelector<HTMLInputElement>("aside input");
    if (!el) return;
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(el, query);
    el.dispatchEvent(new Event("input", { bubbles: true }));
  }, 200);
  const props = {
    ...railProps,
    expanded: new Set(),
    runningIds: new Set(),
    waitingIds: new Set(),
    finished: new Map(),
    searching: true,
    onSearch() {},
    groups: [
      { id: "coding", label: "编码", projects: [{ id: "a", name: "shop-web", path: "/tmp", sessions: [sess("1", "重构 store 模块"), sess("2", "Pinia store 持久化"), sess("x", "修复登录页")] }] },
      { id: "minimal", label: "极简", projects: [{ id: "b", name: "notes", path: "/tmp", sessions: [sess("3", "Store 方案选型")] }] },
    ],
  } as unknown as SidebarProps;
  return (
    <div className="flex h-[420px] bg-pane">
      <Sidebar {...props} />
    </div>
  );
}

/** `#tree-long`：会话很多时的侧栏：树自己滚，底部的个人信息块贴住底。 */
function LongTreePreview() {
  const many = Array.from({ length: 40 }, (_, i) => sess(String(i), `第 ${i + 1} 个会话 · 名字长一点看看截断`));
  const props = {
    ...railProps,
    expanded: new Set(["a"]),
    runningIds: new Set(),
    waitingIds: new Set(),
    finished: new Map(),
    onToggle() {},
    onSearch() {},
    groups: [
      { id: "coding", label: "编码", projects: [{ id: "a", name: "shop-web", path: "/tmp", sessions: many }] },
    ],
  } as unknown as SidebarProps;
  // 外壳照 App 写（侧栏外面再套一层宽度容器）：不然 aside 直接当 flex 子项，
  // 高度被拉满，测不出「底部块不贴底 / 被长列表顶出视口」。
  return (
    <div className="flex h-[520px] w-[900px] bg-pane">
      <div className="shrink-0 overflow-hidden w-[248px]">
        <Sidebar {...props} />
      </div>
      <div className="flex min-w-0 flex-1 flex-col" />
    </div>
  );
}

/** `#tree`：侧栏只列当前预设（编码）的项目；点品牌行下拉切到「极简」，树跟着换。 */
function TreePreview() {
  const [preset, setPreset] = useState("coding");
  const props = {
    ...railProps,
    newChatPreset: preset,
    onSetNewChatPreset: setPreset,
    expanded: new Set(["a"]),
    runningIds: new Set(),
    waitingIds: new Set(),
    finished: new Map(),
    onToggle() {},
    onSearch() {},
    groups: [
      { id: "coding", label: "编码", projects: [{ id: "a", name: "shop-web", path: "/tmp", sessions: [sess("1", "重构 store 模块"), sess("2", "Pinia store 持久化")] }] },
      { id: "minimal", label: "极简", projects: [{ id: "b", name: "notes", path: "/tmp", sessions: [sess("3", "Store 方案选型")] }] },
      { id: "", label: "未记录预设", projects: [{ id: "c", name: "old", path: "/tmp", sessions: [sess("4", "旧会话")] }] },
    ],
  } as unknown as SidebarProps;
  return (
    <div className="flex h-[520px] bg-pane">
      <Sidebar {...props} />
    </div>
  );
}

const readOut = ["1→use std::fmt;", ...Array.from({ length: 8 }, (_, i) => `line ${i + 2}`), "10→line 10", "line 11"].join("\n") +
  "\n\n[… truncated: showed lines 1-11 (11 of 40). 29 lines remain — call read_file again with offset=12 and a limit, or raise limit.]";
const fixtures: [string, Record<string, unknown>, string][] = [
  ["read_file", { target_file: "src/lib.rs" }, readOut],
  ["read_file", { target_file: "src/lib.rs", offset: 35 }, "line 35\nline 36\n40→line 40"],
  ["grep", { pattern: "login", output_mode: "files_with_matches" }, "README.md\nsrc/login.ts"],
  ["grep", { pattern: "beta", "-C": 1 }, "src/a.rs-1-fn alpha() {}\nsrc/a.rs:2:fn beta() {}\nsrc/a.rs-3-let x = ALPHA;"],
  [
    "todo_write",
    {},
    "- [completed] 1: 确认 login 请求方法改动\n- [in_progress] 2: 验证 notes.md 已写入\n- [pending] 3: 跑测试\n\n1/3 done (1 in_progress, 1 pending).\nNothing is in_progress.",
  ],
  [
    "ask_user_question",
    { questions: [{ question: "喜欢哪种颜色？" }] },
    'User has answered your questions: "喜欢哪种颜色？"="绿". You can now continue with the user\'s answers in mind.',
  ],
  ["web_fetch", { url: "https://example.com" }, "HTTP 200 https://example.com/\n\nExample Domain\n\n# Example Domain"],
];

/** `#tools`：各工具按真实输出格式展开。 */
function ToolsPreview() {
  return (
    <section data-preview="tools" className="flex w-[900px] flex-col gap-1 bg-pane p-6">
      {fixtures.map(([tool, args, out], i) => (
        <ToolCard key={i} tool={tool} args={args} output={out} status="completed" initialOpen />
      ))}
    </section>
  );
}

/** `#split`：分屏竖线，脚本模拟拖到 70%（夹在上限）再读比例。 */
function SplitPreview() {
  const [r, setR] = React.useState(0.55);
  React.useEffect(() => {
    const t = setTimeout(() => {
      const d = document.querySelector<HTMLElement>("[role=separator]")!;
      const box = d.parentElement!.getBoundingClientRect();
      d.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true, clientX: box.left + box.width * 0.55 }));
      window.dispatchEvent(new PointerEvent("pointermove", { clientX: box.left + box.width * 0.9 }));
      window.dispatchEvent(new PointerEvent("pointerup", {}));
    }, 200);
    return () => clearTimeout(t);
  }, []);
  return (
    <div className="flex h-40 w-[900px]" data-ratio={r.toFixed(2)}>
      <div style={{ flex: `${r} 1 0%` }} className="bg-fill" />
      <SplitDivider onRatio={setR} />
      <div style={{ flex: `${1 - r} 1 0%` }} className="bg-white" />
    </div>
  );
}

/** `#menu`：更多菜单、改名、删除确认。 */
function Menu() {
  setTimeout(() => document.querySelector<HTMLButtonElement>('button[aria-label="更多"]')?.click(), 200);
  return (
    <div className="flex h-[400px] w-[900px] flex-col gap-4 bg-pane p-4">
      <div className="flex h-11 items-center gap-2.5 border-b border-line-soft">
        <RenameInput title="修复登录页样式" onSave={() => {}} onCancel={() => {}} />
        <span className="text-[11px] text-ink-4">按 Enter 保存，Esc 取消</span>
        <span className="ml-auto">
          <SessionMenu sessionId="s1" cwd="/tmp" onRename={() => {}} onDelete={() => {}} />
        </span>
      </div>
      <DeleteConfirm onConfirm={() => {}} onCancel={() => {}} />
    </div>
  );
}

/** `#shell`：侧栏会话行、连接横条、载入骨架。 */
function Shell() {
  return (
    <div className="flex w-[900px] flex-col gap-6 bg-pane py-6">
      <section data-preview="sidebar" className="mx-4 flex w-[228px] flex-col gap-px rounded-[10px] border border-line-strong bg-sidebar p-2">
        {rows.map(([title, p]) => (
          <SessionRow key={title} title={title} active={false} running={false} waiting={false} open={false} onClick={() => {}} {...p} />
        ))}
      </section>
      <section className="flex flex-col gap-2">
        <ConnectionBanner app={app({ connection: "connecting" })} onRestart={() => {}} />
        <ConnectionBanner app={app({ connection: "closed", reconnects: 2 })} onRestart={() => {}} />
        <ConnectionBanner app={app({ exited: "exit status: 1" })} onRestart={() => {}} />
      </section>
      <section data-preview="tools" className="flex flex-col gap-1 px-4">
        <ToolCard tool="read_file" args={{ target_file: "src/app.ts" }} status="running" startedAt={Date.now() - 400} />
        <ToolCard tool="read_file" args={{ target_file: "src/app.ts" }} output="Error: 文件不存在：src/app.ts" status="failed" startedAt={0} endedAt={100} initialOpen />
        <ToolCard tool="read_file" args={{ target_file: "src/app.ts" }} output="已中断。" status="cancelled" startedAt={0} endedAt={100} initialOpen />
      </section>
      <Queued
        threadId="preview"
        items={[
          {
            id: "steer-1",
            message: "先别动测试，改实现",
            kind: "steer",
            images: [
              { digest: "a".repeat(64), mimeType: "image/png", width: 1280, height: 800 },
              { digest: "b".repeat(64), mimeType: "image/png", width: 640, height: 640 },
            ],
          },
          { id: "1", message: "顺便把 README 里的启动命令也更新一下", kind: "queue", images: [] },
        ]}
        canSteer
        onSteer={() => {}}
        onCancel={() => {}}
      />
      <section className="flex h-[420px] flex-col border-y border-line">
        <Transcript thread={{ turns: [] } as never} sessionKey="x" loading />
      </section>
      <section data-preview="failed" className="flex h-[300px] flex-col border-y border-line">
        <Transcript
          sessionKey="f"
          onRetry={(t) => document.body.setAttribute("data-retried", t)}
          thread={
            {
              turns: [
                {
                  id: "t1",
                  status: "failed",
                  error: "模型「atria-dawn」不在模型目录里：检查模型名，或在 config.toml 里加上 [model.\"atria-dawn\"] 并配好 api_base_url。",
                  startedAt: 0,
                  endedAt: 1000,
                  items: [{ kind: "user", id: "u", text: "只回复 hi", at: Date.now(), attachments: [] }],
                },
              ],
            } as never
          }
        />
      </section>
    </div>
  );
}

/** `#composer`：内容短时输入框浮起（wonder-ui/chat/ComposerFloating）；内容长时钉在底部。 */
function ComposerPreview({ long }: { long: boolean }) {
  const turns = Array.from({ length: long ? 8 : 1 }, (_, i) => ({
    id: `t${i}`,
    status: "completed",
    startedAt: 0,
    endedAt: 1000,
    items: [
      { kind: "user", id: `u${i}`, text: "把会话历史改成懒加载", at: Date.now(), attachments: [] },
      { kind: "text", id: `a${i}`, text: "已加载项目上下文。接下来会检查 cache 与 history 相关模块。" },
    ],
  }));
  return (
    <section data-preview={long ? "composer-long" : "composer-short"} className="flex h-[480px] w-[900px] flex-col bg-pane">
      <Transcript thread={{ turns } as never} sessionKey="c" />
      <Composer
        threadId="preview"
        settings={{ approval: "ask", model: "Grok", modelId: "grok", models: [{ id: "grok", label: "Grok" }], reasoning: "", reasoningOptions: [], plan: false, goal: null, context: null }}
        running={false}
        stopping={false}
        onSend={async () => true}
        onStop={() => {}}
        onSetModel={() => {}}
        onSetApproval={() => {}}
        onSetReasoning={() => {}}
        onSetPlan={() => {}}
        loadSlash={async () => []}
        onSlash={async () => null}
        onStartGoal={async () => true}
        loadContext={async () => Promise.reject(new Error("预览里没有 Dock"))}
        onCompact={() => {}}
      />
    </section>
  );
}

/**
 * `#transcript-stream`：最后一轮在思考，每 30ms 进一段思考增量（和真流式一样每次换新的 thread）。
 * 用来测流式时往上滚不被拽回底部。
 */
function StreamingPreview({ text = false }: { text?: boolean }) {
  const [chunks, setChunks] = React.useState(0);
  React.useEffect(() => {
    const t = window.setInterval(() => setChunks((n) => n + 1), 30);
    return () => window.clearInterval(t);
  }, []);
  const thread = React.useMemo(() => {
    const done = Array.from({ length: 12 }, (_, i) => ({
      id: `t${i}`,
      status: "completed",
      error: null,
      startedAt: 0,
      endedAt: 1000,
      items: [
        { kind: "user", id: `u${i}`, text: "把会话历史改成懒加载", at: Date.now(), attachments: [] },
        { kind: "text", id: `a${i}`, text: "已加载项目上下文。接下来会检查 cache 与 history 相关模块。\n\n".repeat(3) },
      ],
    }));
    const live = {
      id: "t-live",
      status: "running",
      error: null,
      startedAt: Date.now(),
      endedAt: null,
      items: [
        { kind: "user", id: "u-live", text: "继续", at: Date.now(), attachments: [] },
        text
          ? { kind: "text", id: "a-live", text: `## 结论\n\n${"订阅只在翻转时换新，".repeat(chunks + 1)}` }
          : { kind: "reasoning", id: "r-live", text: "先看看 store 的订阅. ".repeat(chunks + 1), startedAt: Date.now(), endedAt: null },
      ],
    };
    return { seq: chunks, turns: [...done, live], compaction: null };
  }, [chunks]);
  return (
    <section data-preview="transcript-stream" className="flex h-[480px] w-[900px] flex-col bg-pane">
      <Transcript thread={thread as never} sessionKey="s" />
    </section>
  );
}

/** `#transcript-nav`：十几轮长短不一的对话，看左边的消息导航（悬停列表、点击跳转）。 */
function NavPreview() {
  const asks = [
    "你好",
    "你有哪些工具",
    "bash的长耗时任务怎么后台运行的",
    "把会话历史改成懒加载，首屏只读最近 50 条，老会话 meta.json 没有游标字段要兜底",
    "跑一下测试",
    "为什么 clippy 报了 needless_borrow",
    "改成按页订阅",
    "继续",
    "顺便把 README 的命令表也更新一下",
    "好的，提交吧",
    "[Image #1] 这个截图里的报错是什么意思",
    "最后总结一下今天的改动",
  ];
  const turns = asks.map((text, i) => ({
    id: `t${i + 1}`,
    status: i === asks.length - 1 ? "running" : "completed",
    error: null,
    startedAt: 0,
    endedAt: i === asks.length - 1 ? null : 9000,
    items: [
      { kind: "user", id: `u${i}`, text, at: Date.now() - (asks.length - i) * 7 * 60_000, attachments: [] },
      ...(i === asks.length - 1
        ? []
        : [{ kind: "text", id: `a${i}`, text: `## 第 ${i + 1} 轮\n\n先说结论：**已处理**。\n\n${"这一轮的回复正文，`task_output` 读增量。".repeat(4 + (i % 4) * 3)}` }]),
    ],
  }));
  return (
    <section data-preview="transcript-nav" className="flex h-[760px] w-[1100px] flex-col bg-pane">
      <Transcript thread={{ seq: 1, turns, compaction: null } as never} sessionKey="n" />
    </section>
  );
}

const SAMPLE_REPLY = [
  "## 改动概览",
  "把 `SessionStore` 改成**懒加载**，首屏只读最近 50 条。见 [设计文档](https://example.com)。",
  "1. 新增 `load_page(cursor)`",
  "2. 旧的 `load_all` 标记弃用",
  "   - 调用方：`cordis-tui/src/history.rs`、`cordis-gateway/src/thread.rs`",
  "- [x] 单测",
  "- [ ] 迁移脚本",
  "> 注意：老会话的 `meta.json` 没有游标字段，要兜底。",
  "```rust\npub fn load_page(&self, cursor: Option<u64>) -> Result<Page> {\n    let rows = self.db.query(\"SELECT * FROM msgs WHERE id < ?\", cursor)?; // 倒序\n    Ok(Page { rows, next: rows.last().map(|r| r.id) })\n}\n```",
  "| 场景 | 之前 | 之后 |\n|---|---|---|\n| 打开 2 万条会话 | 3.2s | 0.18s |\n| 内存 | 410MB | 36MB |",
  "复杂度从 $O(n)$ 降到 $O(\\log n)$，翻页代价：\\[ T(k) = \\sum_{i=1}^{k} \\frac{c}{2^i} < c \\]",
  "```mermaid\nflowchart LR\n  A[打开会话] --> B{有游标?}\n  B -- 有 --> C[load_page]\n  B -- 没有 --> D[读最近 50 条]\n```",
  "```diff\n- let all = store.load_all()?;\n+ let page = store.load_page(None)?;\n```",
  "---",
  "要我接着写迁移脚本吗？",
].join("\n\n");

/** `#transcript-reply`：一轮带常见 markdown（标题、列表、代码、表格、引用）的回复，看排版。 */
function ReplyPreview() {
  const turns = [
    {
      id: "t0",
      status: "completed",
      error: null,
      startedAt: 0,
      endedAt: 74000,
      items: [
        {
          kind: "user",
          id: "u0",
          text: ["## 需求：会话历史懒加载", "大会话打开太慢了。", ...Array.from({ length: 12 }, (_, i) => `- 第 ${i + 1} 条补充说明`)].join("\n"),
          at: Date.now(),
          attachments: [],
        },
        { kind: "reasoning", id: "r0", text: "先看 store。", startedAt: 0, endedAt: 9000 },
        { kind: "tool", id: "x0", name: "bash", arguments: { command: "cargo test -p cordis-spine" }, output: "test result: ok. 214 passed", status: "completed", startedAt: 0, endedAt: 31000 },
        { kind: "text", id: "a0", text: SAMPLE_REPLY },
      ],
    },
  ];
  return (
    <section data-preview="transcript-reply" className="flex h-[2400px] w-[1280px] flex-col bg-pane">
      <Transcript thread={{ seq: 1, turns, compaction: null } as never} sessionKey="r" />
    </section>
  );
}

function Preview() {
  if (location.hash === "#transcript-reply") return <ReplyPreview />;
  if (location.hash === "#transcript-nav") return <NavPreview />;
  if (location.hash === "#files-link") return <FilesLinkPreview />;
  if (isToolPanelPreview()) return <ToolPanelPreview />;
  if (isToolFixturePreview()) return <ToolFixturePreview />;
  if (location.hash === "#transcript-stream") return <StreamingPreview />;
  // `#transcript-stream-text`：同上，流的是正文（看末尾光标）。
  if (location.hash === "#transcript-stream-text") return <StreamingPreview text />;
  if (isPresetPreview()) return <PresetStudioPreview />;
  if (isSettingsPreview()) return <SettingsPreview />;
  if (isNavPagePreview()) return <NavPagePreview />;
  if (location.hash === "#goal-context") return <GoalContextPreview />;
  if (location.hash === "#plugin-views") return <PluginViewsPreview />;
  if (location.hash === "#composer") return <ComposerPreview long={false} />;
  if (location.hash === "#composer-long") return <ComposerPreview long />;
  if (location.hash === "#shell") return <Shell />;
  if (location.hash === "#splash") return <SplashPreview />;
  if (location.hash === "#tree") return <TreePreview />;
  if (location.hash === "#tree-long") return <LongTreePreview />;
  if (location.hash === "#collapsed") return <CollapsedPreview />;
  if (location.hash === "#menu") return <Menu />;
  if (location.hash === "#split") return <SplitPreview />;
  if (location.hash === "#tools") return <ToolsPreview />;
  if (location.hash === "#bash-description") return <BashDescriptionPreview />;
  if (location.hash === "#search") return <SearchPreview query="store" />;
  if (location.hash === "#search-empty") return <SearchPreview query="foobar" />;
  // `#permission`：只摆权限条（主会话 / 子代理发的两种）。
  const shown = location.hash === "#permission" ? items.filter(([, item]) => item.kind === "permission") : items;
  return (
    <div className="flex w-[900px] flex-col gap-6 bg-pane py-6">
      {shown.map(([name, item]) => (
        <section key={name} data-preview={name} className="flex flex-col gap-1">
          <div className="px-4 font-mono text-[11px] text-ink-4">{name}</div>
          <Pending item={item} subagentRole={item.kind === "permission" && item.agentId ? "探索" : null} {...noop} />
        </section>
      ))}
    </div>
  );
}

createRoot(document.getElementById("root")!).render(<Preview />);

// 摆出选中态（默认页）：两道题那张勾上 Rust、Go，单选「绿」。
if (!location.hash) setTimeout(() => {
  const click = (section: string, label: string) =>
    [...document.querySelectorAll<HTMLButtonElement>(`[data-preview="${section}"] button`)]
      .find((b) => b.textContent?.includes(label))
      ?.click();
  click("question two", "Rust");
  setTimeout(() => click("question two", "Go"), 50);
  setTimeout(() => click("question two", "绿"), 100);
}, 300);

/** 启动画面：「重放」重新挂一次，「连上」演示离场。 */
function SplashPreview() {
  const [run, setRun] = useState(0);
  const [booted, setBooted] = useState(false);
  return (
    <section data-preview="splash" className="flex flex-col gap-2 px-4">
      <div className="flex gap-2 text-xs">
        <button type="button" className="rounded border px-2 py-1" onClick={() => (setBooted(false), setRun((n) => n + 1))}>
          重放
        </button>
        <button type="button" className="rounded border px-2 py-1" onClick={() => setBooted(true)}>
          连上
        </button>
      </div>
      <div key={run} className="relative h-[560px] w-[860px] overflow-hidden rounded-lg border border-line bg-window">
        <LaunchSplash app={app({ connection: "connecting", booted, fatal: null, remote: null })} />
      </div>
    </section>
  );
}
