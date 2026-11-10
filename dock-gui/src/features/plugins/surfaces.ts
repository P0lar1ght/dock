// 挂着的插件面板（`surface/list`）：插件面板增删走连接级推送 `surface/changed`，收到就重拉。
// 分屏菜单和右侧工具面板共用这一份。

import { useEffect, useState } from "react";
import type { SurfaceSummary } from "dock-core";
import { onPluginPush, surfaceList } from "./api";

export function useSurfaces(): SurfaceSummary[] {
  const [surfaces, setSurfaces] = useState<SurfaceSummary[]>([]);
  useEffect(() => {
    let alive = true;
    const load = () => surfaceList().then((s) => alive && setSurfaces(s), () => {});
    load();
    const off = onPluginPush((p) => p.kind === "surface" && load());
    return () => {
      alive = false;
      off();
    };
  }, []);
  return surfaces;
}
