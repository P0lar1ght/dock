// 选文件夹框。新对话页和「+ 创建项目」共用。
// 本机 Dock：系统原生的框（tauri-plugin-dialog）；远程 Dock：应用内的框，经 `fs/dirs` 看远端。

import { open } from "@tauri-apps/plugin-dialog";
import { store } from "../dock/store";
import { pickRemoteDirectory } from "./RemoteDirectoryPicker";

/**
 * 弹框选一个目录。选了回绝对路径，取消回 `null`；打不开（浏览器预览里没有 Tauri）
 * 就抛错，调用方退回手填。`from` 是打开时先定位到的目录。
 */
export async function pickDirectory(from?: string): Promise<string | null> {
  if (store.getApp().remote) return pickRemoteDirectory(from);
  const dir = await open({
    directory: true,
    multiple: false,
    title: "选择项目目录",
    defaultPath: from?.startsWith("/") ? from : undefined,
  });
  return typeof dir === "string" ? dir : null;
}
