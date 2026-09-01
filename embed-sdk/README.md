# dock-embed

宿主页用一份脚本注入 Dock 宠物和 Chat。目录：`embed-sdk/`。构建产物是 `dist/dock-embed.js`。

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
cd embed-sdk
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

公开事件：`dock:ready`、`dock:state`、`dock:toggle`、`dock:error`。`connect()` 在 Origin 未绑定时**不会当终态失败**：它创建配对请求并等到 Dock 终端批准（`/pair` 或弹出 overlay），再 `POST /v1/pairing/exchanges` 拿 ticket、挂上 live thread，然后才 `dock:ready`。没有 `dock pair` CLI。`openChat()` 会等同一条连接路径，所以 composer 在 ready 之后可以发消息。

斜杠补全来自 Gateway `slash/list`，发送走 `slash/execute`（接到同一套 spine / agent harness）。嵌入脚本只做前缀过滤和结果渲染；`/screenshot` 仍在浏览器里截图，再作为 Turn 交给 Gateway。
