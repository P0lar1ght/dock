// 重新生成打包的供应商目录快照：拉 models.dev，只留设置页读的字段。
// 用法：node scripts/update-provider-catalog.mjs [api.json 路径]（不给路径就联网拉）。
// 字段白名单和 Dock 的 cordis-spine/src/llm/http/catalog.rs 保持一致。

import { readFile, writeFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";

const SOURCE = "https://models.dev/api.json";
const OUT = fileURLToPath(new URL("../src/dock/providerCatalog.snapshot.json", import.meta.url));
const PROVIDER_KEYS = ["id", "name", "api", "npm", "env", "doc"];
const MODEL_KEYS = ["id", "name", "reasoning", "tool_call", "attachment", "modalities", "limit", "cost", "release_date", "status"];

const pick = (obj, keys) => Object.fromEntries(keys.filter((k) => k in obj).map((k) => [k, obj[k]]));

const raw = process.argv[2]
  ? JSON.parse(await readFile(process.argv[2], "utf8"))
  : await fetch(SOURCE).then((r) => {
      if (!r.ok) throw new Error(`models.dev 回了 HTTP ${r.status}`);
      return r.json();
    });

const out = {};
for (const [pid, p] of Object.entries(raw)) {
  if (!p || typeof p !== "object") continue;
  const models = {};
  for (const [mid, m] of Object.entries(p.models ?? {})) {
    if (m && typeof m === "object") models[mid] = pick(m, MODEL_KEYS);
  }
  out[pid] = { ...pick(p, PROVIDER_KEYS), models };
}
if (Object.keys(out).length === 0) throw new Error("目录是空的");
await writeFile(OUT, JSON.stringify(out));
const count = Object.values(out).reduce((n, p) => n + Object.keys(p.models).length, 0);
console.log(`写好了 ${OUT}：${Object.keys(out).length} 家供应商，${count} 个模型`);
