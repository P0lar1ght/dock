// 沙箱 iframe 里用的库要一个自带依赖的脚本（iframe 不同源，加载不了应用里的 ES 模块）。
// 把它们预先打成 IIFE 放进 src/features/file-preview/vendor/，预览器用 ?raw 内联。
// 升级 @jvmr/pptx-to-html 之后重跑：node scripts/bundle-frame-libs.mjs
import { build } from "vite";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("..", import.meta.url));
const vendor = `${root}src/features/file-preview/vendor`;
await build({
  root,
  configFile: false,
  logLevel: "warn",
  // 库模式不替换 process.env.NODE_ENV（echarts 里有），浏览器里没有 process。
  define: { "process.env.NODE_ENV": JSON.stringify("production") },
  build: {
    outDir: vendor,
    emptyOutDir: false,
    minify: true,
    lib: { entry: `${vendor}/pptx-entry.js`, formats: ["iife"], name: "pptxFrame", fileName: () => "pptx-frame.js" },
  },
});
console.log("wrote", `${vendor}/pptx-frame.js`);
