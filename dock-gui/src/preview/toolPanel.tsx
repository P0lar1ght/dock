// 预览 `#tool-panel`：右侧工具面板（快捷面板列表）里挂着插件面板和 MCP 应用的那两组。
// 不连 Dock：`surface/list`、`mcp/app/tools` 换成假的。

import { dock } from "../dock/client";
import { ToolPanel } from "../components/ToolPanel";

const SURFACES = [
  { id: "activity", title: "Agent 活动" },
  { id: "trajectory", title: "会话轨迹" },
];

const APP_TOOLS = [
  { server: "whiteboard", name: "board", resourceUri: "ui://whiteboard/board.html" },
  { server: "sungods", name: "app", resourceUri: "ui://sungods/app" },
];

export const isToolPanelPreview = () => location.hash === "#tool-panel";

function mock() {
  const d = dock as unknown as {
    call: (m: string, p?: Record<string, unknown>) => Promise<unknown>;
    connect: () => Promise<void>;
    capabilities: Record<string, boolean>;
  };
  d.connect = async () => {};
  d.capabilities = { ...d.capabilities, mcpApps: true };
  d.call = async (method) => {
    if (method === "surface/list") return { surfaces: SURFACES };
    if (method === "mcp/app/tools") return { tools: APP_TOOLS };
    throw new Error(`预览里没有 ${method}`);
  };
}

// 换 fake 要赶在入口组拉数据之前：模块加载时就换（只有这个预览页换）。
if (isToolPanelPreview()) mock();

export function ToolPanelPreview() {
  return (
    <div className="flex min-h-screen items-start justify-end bg-pane p-6">
      <ToolPanel
        terminalOpen={false}
        onToggleTerminal={() => {}}
        onToggleBrowser={() => {}}
        onToggleFiles={() => {}}
        onToggleDesktop={() => {}}
        onToggleCanvas={() => {}}
        onToggleSubagents={() => {}}
        subagentsRunning={2}
        onToggleSideChat={() => {}}
        onToggleReview={() => {}}
        onOpenFeature={(kind) => console.log("打开面板", kind)}
      />
    </div>
  );
}
