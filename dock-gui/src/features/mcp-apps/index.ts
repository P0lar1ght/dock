// MCP Apps 宿主（规范 io.modelcontextprotocol/ui 2026-01-26）：
// - Gen UI：工具卡下面内联界面（一次调用一份）；
// - UI Plugin：常驻面板 `mcp-app:<server>:<uri>`，按项目一份，Agent 和用户在同一处编辑。
// 协议：cordis-gateway/README.md「MCP Apps」；只在这里往宿主登记，宿主不 import 本目录。

import { AppWindow } from "lucide-react";
import { registerPanel, registerPanelEntryGroup, registerSplitGroup, registerToolCard } from "../registry";
import { AppEntryGroup, AppPanel, AppSplitGroup } from "./AppPanel";
import { InlineApp } from "./InlineApp";
import { MCP_APP_PANEL } from "./routing";

// 界面在右侧面板，消息流里只留一行入口（见 InlineApp.tsx）。
// 不登记 isArtifact / keepMounted：这两项会让工具卡在轮次结束后被挪到这一轮末尾——
// 一轮里调多次白板就全堆在最后，看不出哪次调用发生在哪儿。留在原位（随过程折叠）更准。
registerToolCard({
  Attachment: InlineApp,
});
// 能拖出成独立窗（Rust open_panel_window 认 `mcp-app:` kind），窗跟着拖出时的会话（也就定了项目）；
// 主窗口和独立窗之间的调用路由见 routing.ts。
registerPanel({ prefix: MCP_APP_PANEL, Panel: AppPanel, poppable: true, perSession: true, label: (kind) => kind.split(":")[1] || "应用", icon: AppWindow });
registerSplitGroup(AppSplitGroup);
// 右侧工具面板里也列一遍带界面的 MCP 服务（分屏菜单之外的入口）。
registerPanelEntryGroup(AppEntryGroup);
