// 构建产物放进 crate：`include_str!("../../assets/whiteboard.html")`。
import { copyFileSync, statSync } from "node:fs";
const to = new URL("../../../assets/whiteboard.html", import.meta.url);
copyFileSync(new URL("../dist/index.html", import.meta.url), to);
console.log(`whiteboard.html ${(statSync(to).size / 1024 / 1024).toFixed(2)} MB`);
