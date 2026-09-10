# embed-sdk AGENTS.md

本目录是宿主页 JS SDK（npm 项目），规则与根 [AGENTS.md](../AGENTS.md) 相同之处不重复；这里是本包的差异与命令。冲突时以本文件为准。

## 命令

```bash
npm ci                 # 有 package-lock.json，用 ci 而不是 install
npm run build          # clean + build:types + build:bundle
npm run build:types    # tsc -b，只出 .d.ts
npm run build:bundle   # vite build + scripts/build-standalone.mjs
npm run dev:host       # 宿主页调试：127.0.0.1:19080（--strictPort）
npm run pack:skin      # scripts/package-dockskin.mjs 打包 pet skin
```

Node **>= 18**（`package.json` 的 `engines.node`）。

## 约定

- 包管理器是 **npm**，锁文件 `package-lock.json` 必须一起提交；不要在子目录里混用 pnpm / yarn。
- `dist/` 与 `node_modules/` 被 gitignore；产物不提交，靠 `npm run build` 现出。
- 协议是 Gateway 的 `dock.1`。新能力要先加 Gateway 投影，再改 SDK；**不要**为每个斜杠单独适配。
- SDK 只解析、采集（截图）、把 `{ kind }` 画出来。不要在这里另起 harness，也不要直连 TUI。
- 斜杠目录来自 `cordis_tui::slash_catalog()` + `"slash"` extras + `/screenshot*`，不要手抄命令表。
- 运行时依赖版本在 `package.json` 里写死（`lit`、`remark-*`、`vite` 等）；改版本属于「新依赖」，先问。
- 没有 lint / typecheck 脚本以外的检查：类型检查就是 `npm run build:types`。

## 边界

- 不在本目录放密钥、宿主页 token、真实用户数据。
- 改影响宿主页 API 的 `data-*` 属性或事件名时，同步 `README.md` 与根 `docs/ARCHITECTURE.md` 的相关段落。
- 不动 `~/.dock` 或项目 `.dock` 的运行时数据。
