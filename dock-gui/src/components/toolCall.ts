// 对话流里的工具项实际调的是哪颗工具。
//
// MCP 工具（浏览器、cua-driver）和按需工具平时经 `use_tool { tool_name, tool_input }` 调，
// 预设把它们设成常驻（resident_tools）时才以本名出现。面板按本名认，两种都要接住。

export function calledTool(item: { name: string; arguments: Record<string, unknown> }): {
  name: string;
  args: Record<string, unknown>;
} {
  if (item.name === "use_tool" && typeof item.arguments.tool_name === "string") {
    const input = item.arguments.tool_input;
    return {
      name: item.arguments.tool_name,
      args: input && typeof input === "object" && !Array.isArray(input) ? (input as Record<string, unknown>) : {},
    };
  }
  return { name: item.name, args: item.arguments };
}
