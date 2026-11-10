// 预览 `#files-link`：回复里行内代码的文件路径点了在右边文件面板打开（跳行、按名字找、同名弹快速打开）。
// 不连 Dock：名册塞一个假会话，dock.call 换成假的 fs/*。

import { useEffect, useState } from "react";
import { dock } from "../dock/client";
import { store } from "../dock/store";
import { Transcript } from "../components/Transcript";
import { FilesPanel } from "../components/FilesPanel";
import type { PathTarget } from "../components/markdown/pathLink";

const CWD = "/Users/p/proj";
const APP_TS = [
  'import { useEffect, useState } from "react";',
  "",
  "/** 拉一页会话历史。 */",
  "export async function loadPage(cursor: number | null): Promise<Page> {",
  '  const rows = await db.query("SELECT * FROM msgs WHERE id < ?", cursor ?? Infinity);',
  "  return { rows, next: rows.at(-1)?.id ?? null };",
  "}",
  "",
  ...Array.from({ length: 72 }, (_, i) => `const line${i + 9} = ${i + 9}; // 占位`),
].join("\n");

const FILES: Record<string, string> = {
  "src/app.ts": APP_TS,
  "src/styles.css": ".card {\n  display: flex;\n  color: #1c1c1e; /* 正文 */\n}\n",
  "README.md": "# 示例项目\n\n把会话历史改成**懒加载**。\n\n- [x] 单测\n- [ ] 迁移脚本\n\n```ts\nconst page = await loadPage(null);\n```\n",
  "Cargo.toml": '[package]\nname = "demo"\nversion = "0.1.0"\n\n[dependencies]\nserde = { version = "1", features = ["derive"] }\n',
  "package.json": '{\n  "name": "demo",\n  "private": true,\n  "scripts": { "dev": "vite" }\n}\n',
  "scripts/run.sh": '#!/bin/sh\nset -e\necho "building $1"\ncargo build -p cordis-app\n',
  "data/usage.csv": 'date,model,tokens,cost\n2026-10-01,deepseek-flash,120345,0.42\n2026-10-02,deepseek-flash,98012,0.31\n2026-10-03,"grok, fast",5021,0.02\n',
  "assets/logo.svg": '<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64"><circle cx="32" cy="32" r="28" fill="#0a84ff"/></svg>',
  "cordis-tui/src/history.rs": "/// 一页历史。\npub fn load_page(cursor: Option<u64>) -> Vec<Row> {\n    let mut rows = Vec::new();\n    rows\n}\n",
  "a/mod.rs": "// a\n",
  "b/mod.rs": "// b\n",
};

function mockFs() {
  (dock as unknown as { call: (m: string, p?: Record<string, unknown>) => Promise<unknown> }).call = async (method, params = {}) => {
    const path = String(params.path ?? "");
    if (method === "fs/read") {
      if (!(path in FILES)) throw new Error(`没有这个路径：${path}`);
      return { path, size: FILES[path].length, kind: "text", text: FILES[path], truncated: false, modifiedMs: Date.now() };
    }
    if (method === "fs/find") {
      const q = String(params.query);
      return { paths: Object.keys(FILES).filter((p) => p.includes(q)) };
    }
    if (method === "fs/list") {
      const prefix = path ? `${path}/` : "";
      const names = new Map<string, "dir" | "file">();
      for (const p of Object.keys(FILES).filter((p) => p.startsWith(prefix))) {
        const rest = p.slice(prefix.length);
        const slash = rest.indexOf("/");
        names.set(slash < 0 ? rest : rest.slice(0, slash), slash < 0 ? "file" : "dir");
      }
      const entries = [...names].map(([name, kind]) => ({ name, path: prefix + name, kind }));
      return { root: CWD, path, entries, truncated: false };
    }
    throw new Error(`预览里没有 ${method}`);
  };
  const s = store as unknown as { app: Record<string, unknown>; emit: () => void };
  s.app = {
    ...s.app,
    connection: "open",
    sessions: [{ id: "s1", projectId: "p", title: "预览", preset: "编码", presetId: null, cwd: CWD, open: true, updatedAt: 0 }],
  };
  s.emit();
}

const REPLY = [
  "改了 `src/app.ts:42`，说明在 `README.md`；`history.rs` 只写了文件名，`mod.rs` 有两个同名。",
  "绝对路径 `/Users/p/proj/src/app.ts` 能开，目录外的 `/etc/hosts` 和 `self.db`、`console.log` 不是链接。",
].join("\n\n");

export function FilesLinkPreview() {
  const [ready, setReady] = useState(false);
  const [target, setTarget] = useState<(PathTarget & { seq: number }) | null>(null);
  useEffect(() => {
    mockFs();
    setReady(true);
    return store.onOpenFile((_, t) => setTarget((p) => ({ ...t, seq: (p?.seq ?? 0) + 1 })));
  }, []);
  const [finding, setFinding] = useState(false);
  if (!ready) return null;
  const thread = {
    seq: 1,
    compaction: null,
    turns: [
      {
        id: "t1",
        status: "completed",
        error: null,
        startedAt: 0,
        endedAt: 1000,
        items: [
          { kind: "user", id: "u1", text: "改完了吗", at: Date.now(), attachments: [] },
          { kind: "text", id: "a1", text: REPLY },
        ],
      },
    ],
  };
  return (
    <section data-preview="files-link" className="flex h-[640px] w-[1280px] bg-pane">
      <div className="flex w-[560px] flex-col">
        <Transcript thread={thread as never} sessionKey="s1" />
      </div>
      <FilesPanel
        sessionId="s1"
        cwd={CWD}
        target={target}
        onTargetHandled={() => setTarget(null)}
        finding={finding}
        onFinding={setFinding}
        onClose={() => {}}
      />
    </section>
  );
}
