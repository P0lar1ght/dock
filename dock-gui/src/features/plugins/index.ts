// 插件给界面的东西（docs/PLUGIN-VIEWS.md）：状态项、面板、工具卡视图、设置卡。
// 设计：wonder-ui/plugins/（第七轮）。只在这里往宿主登记，宿主不 import 本目录。

import { Puzzle } from "lucide-react";
import { registerComposerItem, registerPanel, registerPanelEntryGroup, registerPluginRow, registerSplitGroup, registerToolCard } from "../registry";
import { PLUGIN_PANEL } from "./panelKind";
import { PluginEntryGroup, PluginPanel, PluginSplitGroup } from "./PluginPanel";
import { PluginSettingsRow } from "./SettingsCard";
import { StatusItems } from "./StatusItems";
import { ToolViewBody, useToolIcon } from "./ToolViews";

registerComposerItem(StatusItems);
registerToolCard({ useIcon: useToolIcon, Body: ToolViewBody });
registerPluginRow(PluginSettingsRow);
// 插件面板能拖出 / 开新窗口：Rust 侧 open_panel_window 认 `plugin:<id>`，全局一扇（不跟会话）。
registerPanel({ prefix: PLUGIN_PANEL, Panel: PluginPanel, poppable: true, label: (kind) => kind.slice(kind.indexOf(":") + 1), icon: Puzzle });
registerSplitGroup(PluginSplitGroup);
// 右侧工具面板里也列一遍挂着的插件面板（分屏菜单之外的入口）。
registerPanelEntryGroup(PluginEntryGroup);
