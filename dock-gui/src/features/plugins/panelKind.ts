// 插件面板在右侧面板槽里的 kind：`plugin:<面板 id>`（面板 id 是 `"tui.slots"` 的 id）。

export const PLUGIN_PANEL = "plugin";

export const surfaceKind = (id: string) => `${PLUGIN_PANEL}:${id}`;

/** `plugin:<id>` → id；别的 kind 回 null。 */
export const surfaceIdOf = (kind: string) => (kind.startsWith(`${PLUGIN_PANEL}:`) ? kind.slice(PLUGIN_PANEL.length + 1) : null);
