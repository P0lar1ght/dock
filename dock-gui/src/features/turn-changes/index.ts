// 每轮改动（Dock `turn/changes/*`）：一轮结束后挂改动卡，右侧「审查」面板看 diff 和 code-review 批注。
// 只在这里往宿主登记，宿主不 import 本目录。

import { GitCompareArrows } from "lucide-react";
import { registerPanel, registerPanelMenuItem, registerTurnFooter } from "../registry";
import { ChangesPanel } from "./ChangesPanel";
import { TurnChangesCard } from "./TurnChangesCard";

registerTurnFooter(TurnChangesCard);
// 右侧面板 `changes:worktree`（工具面板里的「审查」）/ `changes:turn:<turnId>`（改动卡的「审查」），跟着会话走。
registerPanel({ prefix: "changes", Panel: ChangesPanel, poppable: false, single: true, label: "审查", icon: GitCompareArrows });
// 「再叠一页」菜单里也列一行。
registerPanelMenuItem({ kind: "changes:worktree", prefix: "changes", label: "审查", icon: GitCompareArrows });
