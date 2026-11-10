// 权限请求的 `summary`（Dock spine 的 permission_summary）：首行是要放行的东西，
// 命令工具（bash / monitor）是 `$ 完整命令`；有 `description` 时另起一行。

/** 拆成首行（要放行的东西）和说明（命令工具的 `description`，可能没有）。 */
export function permissionParts(summary: string): { main: string; note: string } {
  const [main = "", ...rest] = summary.split("\n");
  return { main, note: rest.join(" ").trim() };
}
