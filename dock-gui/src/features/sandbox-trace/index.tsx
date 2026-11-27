// 行为轨迹（Dock `sandbox/status` / `sandbox/trace`）：开了沙箱的页，agent 做过什么画成链路图和时间线。
// 只在这里往宿主登记，宿主不 import 本目录。

import { Waypoints } from "lucide-react";
import { ToolRow } from "../../components/ToolPanel";
import { registerPanel, registerPanelEntryGroup, registerPanelMenuItem, type PanelEntryGroupProps } from "../registry";
import { canTrace } from "./api";
import { TracePanel } from "./TracePanel";

// 右侧面板 `sandbox:trace`，跟着会话走。
registerPanel({ prefix: "sandbox", Panel: TracePanel, poppable: false, single: true, label: "行为轨迹", icon: Waypoints });
// 「再叠一页」菜单里列一行。
registerPanelMenuItem({ kind: "sandbox:trace", prefix: "sandbox", label: "行为轨迹", icon: Waypoints });
// 工具面板（右侧一个面板都没开时）里也有一行。老 Dock 不认 `sandbox/*` 就不列。
function TraceEntry({ onOpen }: PanelEntryGroupProps) {
  if (!canTrace()) return null;
  return (
    <>
      <div className="mx-1 my-1.5 h-px bg-line-soft" />
      <div className="px-1 pb-1 pt-1.5 text-[11px] font-medium text-ink-4">沙箱</div>
      <ToolRow icon={Waypoints} label="行为轨迹" title="这一页开了沙箱以后 agent 做过什么：链路图、时间线、HTTP 请求 / 响应" onClick={() => onOpen("sandbox:trace")} />
    </>
  );
}
registerPanelEntryGroup(TraceEntry);
