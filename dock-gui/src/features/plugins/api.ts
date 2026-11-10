// 插件给界面的东西（dock.1，见 docs/PLUGIN-VIEWS.md）：面板 `surface/*`、状态项 `status/list`、
// 工具卡视图 `tool/views` / `tool/view`、设置卡 `plugin/settings/*`。外形由 dock-core 解析。
// 除了 `surface/list`、`tool/views`，都只给受信 ticket（GUI 有）。
// 变动走连接级推送，不用订阅线程：收到就重拉对应那一份。

import {
  parsePluginPush,
  parsePluginSettings,
  parsePluginSettingsList,
  parsePluginSettingsPush,
  parsePluginSettingsSet,
  parseStatusItems,
  parseSurface,
  parseSurfaceAction,
  parseSurfaceList,
  parseSurfaceWeb,
  parseToolView,
  parseToolViews,
  type PluginPush,
} from "dock-core";
import { dock } from "../../dock/client";

export const surfaceList = () => dock.call("surface/list").then(parseSurfaceList);

export const surfaceGet = (id: string) =>
  dock.call<{ surface: unknown }>("surface/get", { id }).then((r) => parseSurface(r.surface));

export const surfaceAction = (id: string, action: string) => dock.call("surface/action", { id, action }).then(parseSurfaceAction);

export const surfaceWeb = (id: string) => dock.call("surface/web", { id }).then(parseSurfaceWeb);

export const statusList = () => dock.call("status/list").then(parseStatusItems);

export const toolViewNames = () => dock.call("tool/views").then(parseToolViews);

export const toolView = (threadId: string, itemId: string) => dock.call("tool/view", { threadId, itemId }).then(parseToolView);

export const pluginSettingsList = () => dock.call("plugin/settings/list").then(parsePluginSettingsList);

export const pluginSettingsGet = (pluginId: string) => dock.call("plugin/settings/get", { pluginId }).then(parsePluginSettings);

export const pluginSettingsSet = (pluginId: string, values: Record<string, unknown>) =>
  dock.call("plugin/settings/set", { pluginId, values }).then(parsePluginSettingsSet);

/** 面板 / 状态项 / 工具卡视图变了。返回取消函数。 */
export function onPluginPush(fn: (push: PluginPush) => void): () => void {
  return dock.onNotification((n) => {
    const push = parsePluginPush(n.method, n.params);
    if (push) fn(push);
  });
}

/** 某颗插件的设置卡变了（登记、卸下、别处改了值）。 */
export function onPluginSettingsChanged(fn: (pluginId: string) => void): () => void {
  return dock.onNotification((n) => {
    const push = parsePluginSettingsPush(n.method, n.params);
    if (push) fn(push.pluginId);
  });
}
