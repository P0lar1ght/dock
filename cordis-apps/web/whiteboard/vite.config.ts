import { defineConfig, type Plugin } from "vite";
import react from "@vitejs/plugin-react";
import { viteSingleFile } from "vite-plugin-singlefile";

// 单文件打包会把动态 import 全部内联：Excalidraw 的「文字转图表」要整个 mermaid（几 MB），
// 50 多种界面语言也全进来。白板里画图交给 Agent 的工具，这两样换成空模块，只留简体中文。
function trim(): Plugin {
  const STUB_MERMAID = "\0dock-stub-mermaid";
  const STUB_LOCALE = "\0dock-stub-locale";
  return {
    name: "dock-trim-excalidraw",
    enforce: "pre",
    resolveId(id, importer) {
      if (id === "@excalidraw/mermaid-to-excalidraw") return STUB_MERMAID;
      if (importer?.includes("@excalidraw/excalidraw") && /\/locales\//.test(id) && !/\/locales\/zh-CN-/.test(id)) return STUB_LOCALE;
      return null;
    },
    load(id) {
      if (id === STUB_MERMAID)
        return "export const parseMermaidToExcalidraw = async () => { throw new Error('Dock 白板不带 Mermaid 转换'); };";
      if (id === STUB_LOCALE) return "export default {};";
      return null;
    },
  };
}

// 单文件：MCP App 的界面是一份 HTML 资源（resources/read），没有同源服务器可取别的文件。
export default defineConfig({
  plugins: [trim(), react(), viteSingleFile()],
  define: { "process.env.IS_PREACT": JSON.stringify("false") },
  build: { outDir: "dist", assetsInlineLimit: Number.MAX_SAFE_INTEGER, chunkSizeWarningLimit: 8192 },
});
