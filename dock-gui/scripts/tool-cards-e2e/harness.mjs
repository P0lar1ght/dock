// 无头 Chrome 里跑完整 GUI（index.html），Tauri 换成假桥：dock_connection 指向隔离的 dock serve --remote。
import { chromium } from "/Users/polar/.npm/_npx/705bc6b22212b352/node_modules/playwright/index.mjs";
import { readFileSync } from "node:fs";

export const PROJ = "/tmp/dock-e2e/proj";
const TOKEN = readFileSync("/tmp/dock-e2e/token", "utf8").trim();

export async function openGui({ width = 1440, height = 900 } = {}) {
  const browser = await chromium.launch();
  const page = await browser.newPage({ viewport: { width, height }, deviceScaleFactor: 2 });
  const errors = [];
  page.on("pageerror", (e) => errors.push(String(e)));
  await page.addInitScript(({ token, proj }) => {
    let cb = 0;
    window.__TAURI_INTERNALS__ = {
      metadata: { currentWindow: { label: "main" }, currentWebview: { windowLabel: "main", label: "main" } },
      transformCallback: () => ++cb,
      unregisterCallback: () => {},
      convertFileSrc: (p) => p,
      invoke: async (cmd, args) => {
        if (cmd === "dock_connection") return { ws: "ws://127.0.0.1:18997/api/ws", token };
        if (cmd === "remote_list") return { active: "e2e", remotes: [{ id: "e2e", name: "e2e", url: "http://127.0.0.1:18997" }] };
        if (cmd === "plugin:dialog|open") return proj;
        if (cmd.startsWith("plugin:event|")) return 1;
        return null;
      },
    };
    window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => {} };
    localStorage.setItem(
      "dock-gui.prefs",
      JSON.stringify({ approval: "auto", projectDir: proj, restoreSession: false, notifyTurnDone: false, notifyNeedsInput: false }),
    );
  }, { token: TOKEN, proj: PROJ });
  await page.goto("http://localhost:1430/");
  return { browser, page, errors };
}
