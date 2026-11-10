import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
// @ts-expect-error type error without @types/node package
import process from "node:process";
const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(() => ({
  plugins: [react(), tailwindcss()],

  // 只在 Worker 里或第一次用到才 import 的依赖：开发服务器的依赖扫描不进 Worker（`new Worker(new URL(...))`），
  // 第一次用到时才发现、现场预构建，然后整页重载（GUI 退回开始页）。启动时就先构建好。
  optimizeDeps: {
    include: [
      "xlsx",
      "xlsx/dist/cpexcel.full.mjs",
      "pdfjs-dist",
      "codemirror",
      "@codemirror/state",
      "@codemirror/view",
      "@codemirror/commands",
      "@codemirror/language",
      "@codemirror/language-data",
    ],
  },

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 3. tell Vite to ignore watching `src-tauri`
      ignored: ["**/src-tauri/**"],
    },
    // dock-core 以源码形式软链进来（package.json 的 file:../dock-core），在项目根外面：
    // 不放行的话开发服务器拒绝读它，页面整个加载不出来。
    fs: {
      allow: [".", "../dock-core"],
    },
  },
}));
