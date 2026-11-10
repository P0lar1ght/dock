// 记忆 / 浏览器与联网两页共用：读白名单键（config/get），即改即存（config/set）。

import { useState } from "react";
import { configGet, configSet, type ConfigValues } from "../../dock/settingsApi";
import { autosave, useLoad } from "./ui";

export function useConfigKeys() {
  const cfg = useLoad(configGet);
  const [values, setValues] = useState<Record<string, unknown> | null>(null);
  const current = values ?? cfg.data?.values ?? {};

  /** 写一个键；`null` = 删掉回默认。成功后用 Dock 回的整份值刷新。 */
  const write = async (key: string, value: unknown) => {
    // 先乐观地改界面，失败再拉回来。
    setValues({ ...current, [key]: value });
    const r = await autosave(() => configSet(key, value));
    if (r) setValues(r.values);
    else setValues(null);
    return r;
  };

  return {
    loading: cfg.loading && !cfg.data,
    error: cfg.error,
    data: cfg.data as ConfigValues | undefined,
    get: <T>(key: string): T | null => (current[key] ?? null) as T | null,
    write,
  };
}
