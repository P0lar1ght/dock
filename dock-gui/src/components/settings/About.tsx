// 关于 Dock：GUI 在后台拉起的 Dock 进程（版本、协议、二进制路径、配置文件）。
// 连远程 Dock 时：没有本机二进制，显示连接地址；路径在远端，不给「在访达中显示」。

import { useEffect, useState, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { configStatus } from "../../dock/settingsApi";
import { useApp } from "../../dock/store";
import { remoteWhere } from "../../dock/remote";
import { Card, Page, useLoad } from "./ui";

function PathValue({ path, local = true }: { path: string; local?: boolean }) {
  return (
    <span className="flex items-center gap-2">
      <span className="min-w-0 break-all font-mono text-xs text-ink-2">{path}</span>
      {local && (
        <button
          type="button"
          onClick={() => revealItemInDir(path).catch(() => {})}
          className="motion-colors shrink-0 rounded-md px-2 py-0.5 text-[11.5px] font-medium text-accent hover:bg-[#0a84ff14]"
        >
          在访达中显示
        </button>
      )}
    </span>
  );
}

export function AboutPane({ version }: { version?: string }) {
  const remote = useApp().remote;
  const [path, setPath] = useState<string | null>(null);
  const status = useLoad(configStatus);
  useEffect(() => {
    if (!remote) invoke<string>("dock_path").then(setPath, () => setPath(null));
  }, [remote]);
  const config = status.data;
  const rows: [string, ReactNode][] = [
    ["Dock 版本", version ? `v${version}` : "—"],
    ["协议", "dock.1"],
    remote
      ? ["远程地址", <span className="break-all font-mono text-xs text-ink-2">{remoteWhere(remote)}</span>]
      : ["Dock 路径", path ? <PathValue path={path} /> : "—"],
    [
      "配置文件",
      config ? config.exists ? <PathValue path={config.path} local={!remote} /> : `${config.path}（还没有）` : "—",
    ],
  ];
  return (
    <Page title="关于 Dock" subtitle={remote ? `远程 Dock「${remote.name}」` : "GUI 在后台拉起的 Dock 进程"}>
      <Card className="py-1">
        {rows.map(([k, v]) => (
          <div key={k} className="flex items-center gap-6 border-b border-[#f0f0f3] px-4 py-3 last:border-b-0">
            <span className="w-24 shrink-0 text-[13px] text-ink-4">{k}</span>
            <span className="min-w-0 flex-1 text-[13px] text-ink">{v}</span>
          </div>
        ))}
      </Card>
    </Page>
  );
}
