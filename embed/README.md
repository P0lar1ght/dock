# dock-embed

宿主页用一份脚本注入 Dock 宠物和 Chat。目录：`embed/`。构建产物是 `dist/dock-embed.js`。

```html
<script
  src="/vendor/dock/dock-embed.js"
  data-dock-auto
  data-application="example-app"
  data-gateway-url="http://127.0.0.1:18990"
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
| `data-gateway-url` | 回环 Gateway，默认可省 |
| `data-skin` | 内置皮肤，默认 `dudu` |
| `data-theme` | `auto` / `light` / `dark` |
| `data-auto-connect` | `false` 时由宿主调用 `connect()` |

公开事件：`dock:ready`、`dock:state`、`dock:toggle`、`dock:error`。配对命令显示为 `dock pair <id> --url … --activate`。
