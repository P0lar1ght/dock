// 给一个会话的对话流造「打开文件」：路径按会话目录换算，点了交给 App 开文件面板。

import { useMemo } from "react";
import { store } from "../../dock/store";
import { parsePath, relativeTo, type PathLinker } from "./pathLink";

/** `threadId` 为空（或不是名册里的会话）时路径不变成链接。 */
export function usePathLinker(threadId: string | null): PathLinker | null {
  return useMemo(
    () =>
      threadId
        ? {
            resolve: (code) => {
              const cwd = store.sessionCwd(threadId);
              const parsed = cwd ? parsePath(code) : null;
              const path = parsed && cwd ? relativeTo(parsed.path, cwd) : null;
              return path ? { path, line: parsed!.line } : null;
            },
            open: (target) => store.openFile(threadId, target),
          }
        : null,
    [threadId],
  );
}
