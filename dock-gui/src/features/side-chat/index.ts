// 侧边聊天：右侧面板 `sidechat`（工具面板里的「侧边聊天」、⌥⌘S 打开）。
// 不能拖成独立窗口：它跟着主窗口里正在看的会话走。

import { MessageSquare } from "lucide-react";
import { registerPanel, registerPanelMenuItem } from "../registry";
import { SideChatPanel } from "./SideChatPanel";

// 宿主（App 的工具面板）按 `sidechat:current` 开它：跟着当前会话走。
registerPanel({ prefix: "sidechat", Panel: SideChatPanel, poppable: false, label: "侧边聊天", icon: MessageSquare });
// 「再叠一页」菜单里也列一行（没登记的话那里只有内置面板和插件 / MCP 应用）。
registerPanelMenuItem({ kind: "sidechat:current", prefix: "sidechat", label: "侧边聊天", icon: MessageSquare });
