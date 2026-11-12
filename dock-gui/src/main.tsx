import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import App from "./App";
import { ChatWindow } from "./components/ChatWindow";
import { PanelWindow } from "./components/PanelWindow";
import type { SidePanel } from "./components/ToolPanel";
import { panelFor } from "./features/registry";
import "./styles.css";
import "./features";
import { bootGateways } from "./dock/gateways";
import { applyAppearance } from "./dock/prefs";

// 强调色、界面字号（设置 › 外观）：第一帧之前就套上。
applyAppearance();
// 每台网关一份连接。要等 store 模块求值完再建，否则 DockStore 还没初始化。
bootGateways();

// `index.html?session=<id>` = 从工作台拖出来的独立聊天窗，只画那一个会话；
// 再带 `panel=<种类>` = 拖出来的独立面板窗。插件面板（`panel=plugin:<id>`）不属于会话，不带 session。
const query = new URLSearchParams(location.search);
const detached = query.get("session");
const panel = query.get("panel") as SidePanel | null;

/** 功能登记的面板（插件面板）：独立窗不需要会话。 */
const isFeaturePanel = (kind: string) => !!panelFor(kind);

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    {panel && (detached || isFeaturePanel(panel)) ? (
      <PanelWindow kind={panel} sessionId={detached} />
    ) : detached ? (
      <ChatWindow sessionId={detached} />
    ) : (
      <App />
    )}
  </StrictMode>,
);
