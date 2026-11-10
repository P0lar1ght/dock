// 工作流 run（Dock `workflow`）：工具卡下面挂进度卡（阶段、子代理、结果），照子代理卡的样子。
// 只在这里往宿主登记，宿主不 import 本目录。

import { createElement } from "react";
import { MessageSquareWarning } from "lucide-react";
import { calledTool } from "../../components/toolCall";
import { registerPanel, registerToolCard } from "../registry";
import { ReviewPanel } from "./ReviewPanel";
import { WorkflowCard } from "./WorkflowCard";

// code-review 的批注：右侧面板 `review:<runId>`，跟着会话走（run 只在那个会话里）。
registerPanel({ prefix: "review", Panel: ReviewPanel, poppable: false, single: true, label: "审查批注", icon: MessageSquareWarning });

const TOOL = "workflow";

registerToolCard({
  Attachment: ({ threadId, itemId, toolName, args }) => {
    const called = calledTool({ name: toolName, arguments: args });
    if (called.name !== TOOL || called.args.validate_only === true) return null;
    return createElement(WorkflowCard, { threadId, toolCallId: itemId });
  },
  // run 是这一轮的产物，而且会跑到这一轮之后：轮次结束后不折进过程。
  isArtifact: (name, args) => {
    const called = calledTool({ name, arguments: args });
    return called.name === TOOL && called.args.validate_only !== true;
  },
});
