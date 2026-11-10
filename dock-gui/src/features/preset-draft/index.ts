// 会话里起草的预设（Dock `propose_preset`）：工具卡下面挂草稿卡，用户在编辑器里审、
// 自己保存。只在这里往宿主登记，宿主不 import 本目录。

import { createElement } from "react";
import { calledTool } from "../../components/toolCall";
import { registerToolCard } from "../registry";
import { PresetDraftCard, rememberSavedDrafts } from "./PresetDraftCard";

rememberSavedDrafts();

const TOOL = "propose_preset";

registerToolCard({
  Attachment: ({ threadId, itemId, toolName, args, status }) => {
    const called = calledTool({ name: toolName, arguments: args });
    if (called.name !== TOOL || status === "failed" || status === "denied" || status === "cancelled") return null;
    return createElement(PresetDraftCard, { origin: `${threadId}:${itemId}`, args: called.args, running: status === "running" });
  },
  // 草稿卡是这一轮的产物：轮次结束后不折进过程。
  isArtifact: (name, args) => calledTool({ name, arguments: args }).name === TOOL,
});
