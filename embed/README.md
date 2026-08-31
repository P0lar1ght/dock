# dock-embed

宿主页用一份脚本注入 Dock 宠物和 Chat。目录：`embed/`。构建产物是 `dist/dock-embed.js`。

这份 SDK 参考 PolarVigil 的浏览器注入面（Origin 配对、JSON-RPC、Thread/Turn），协议改成 Dock 实际投影的 `dock.1`：同一进程里的当前会话，而不是 PolarVigil Runtime 的十二项硬能力。

```html
<script
  src="/vendor/dock/dock-embed.js"
  data-dock-auto
  data-application="example-app"
  data-gateway-url="http://127.0.0.1:18991"
  data-skin="dudu">
</script>
```

```bash
cd embed
npm install
npm run build
```

本地预览注入页：打开 `examples/inject/index.html`（先 `npm run build`）。

| 属性 | 含义 |
| --- | --- |
| `data-dock-auto` | 自动挂载 `<dock-agent>` |
| `data-application` | 稳定 Application ID |
| `data-gateway-url` | 回环 Gateway，默认 `http://127.0.0.1:18991` |
| `data-skin` | 内置皮肤，默认 `dudu` |
| `data-theme` | `auto` / `light` / `dark` |
| `data-auto-connect` | `false` 时由宿主调用 `connect()` |

公开事件：`dock:ready`、`dock:state`、`dock:toggle`、`dock:error`。首次连接会创建配对请求，**请在 Dock 终端确认**（`/pair` 或弹出 overlay），浏览器轮询直到 `approved`，再 `POST /v1/pairing/exchanges` 拿 ticket。没有 `dock pair` CLI。
