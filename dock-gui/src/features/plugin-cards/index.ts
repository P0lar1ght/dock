// 插件卡片（Dock `turn/cards/*`，Rhai `host.card`）：挂在一轮回答下面。只在这里往宿主登记。

import { registerTurnFooter } from "../registry";
import { PluginTurnCards } from "./PluginTurnCards";

registerTurnFooter(PluginTurnCards);
