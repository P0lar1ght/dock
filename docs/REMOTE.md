# 远程连接：`dock serve --remote`

把 Dock 放在一台常开的机器上（VPS、家里的服务器），
在别的机器上用桌面 GUI 或浏览器 UI 连它。

## 思路

- Dock **只绑回环**（默认 `127.0.0.1:18990`），自己不对公网开端口。
- TLS 与对外暴露交给前面的反向代理（Caddy / nginx），或者 Tailscale。
- 客户端用**设备令牌**鉴权（`dock device add`），不走本机那套 Origin 配对。
- 远程模式不挂配对与 ticket 的 HTTP 路由：公网上的人连配对申请都发不了。

```
客户端（GUI / 浏览器 UI）──wss──▶ 反向代理（TLS）──ws──▶ 127.0.0.1:18990 dock serve --remote
```

## 1. 签设备令牌

在跑 Dock 的那台机器上：

```bash
dock device add laptop       # 令牌只显示这一次，抄到客户端
dock device list             # 看有哪些设备、最后一次什么时候用
dock device revoke laptop    # 撤销：立刻失效，连着的连接几秒内断开
```

- 盘上只存令牌的 sha256：`$DOCK_HOME/devices.json`（unix 上 0600）。
- 丢了就撤销重签，没有「找回」。
- 一台设备一枚令牌，方便单独撤销。

## 2. 起常驻网关

```bash
dock serve --remote                      # 默认 127.0.0.1:18990
dock serve --remote --bind 127.0.0.1:9000
```

- 端口被占用就直接报错，不会像本机模式那样顺延（代理只认配好的端口）。
- 不看 stdin，跑到 SIGINT / SIGTERM 为止。
- 工作目录就是默认项目目录；会话按目录落在 `$DOCK_HOME/sessions/`。

systemd 示例（`/etc/systemd/system/dock.service`）：

```ini
[Unit]
Description=Dock remote gateway
After=network-online.target

[Service]
User=dock
WorkingDirectory=/home/dock/work
ExecStart=/usr/local/bin/dock serve --remote
Restart=on-failure

[Install]
WantedBy=multi-user.target
```

## 3. 前面挂 TLS

### Caddy（自动签证书）

```caddyfile
dock.example.com {
    reverse_proxy 127.0.0.1:18990
}
```

Caddy 会自己处理 WebSocket 升级。

### nginx

```nginx
server {
    listen 443 ssl;
    server_name dock.example.com;
    # ssl_certificate / ssl_certificate_key 照你的证书配

    location /api/ws {
        proxy_pass http://127.0.0.1:18990;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_read_timeout 1h;
    }
}
```

- `proxy_read_timeout` 要够长：会话一轮可能跑很久，中途没有消息。
- 建议在代理上再加限速（比如 nginx `limit_req`）。

### Tailscale（不开公网端口）

- 两台机器进同一个 tailnet。
- 用 `tailscale serve` 把 `127.0.0.1:18990` 暴露成 tailnet 内的 https 地址。

## 4. 客户端

### 桌面 GUI

- 设置 → 网关与设备 → 连接 →「+ 添加远程 Dock」：填名字、地址、令牌，可以先「测试连接」。
- 令牌只存进 macOS 钥匙串；点「使用」切过去，所有窗口重载。
- 连着远程时本机不拉起 Dock；本机终端、「在访达中显示」不出现（路径在远端）。
- 设置页改的是远端那台 Dock 的配置，本机配置不动。
- 新建对话选目录走应用内的框，经网关 `fs/dirs` 看远端目录；也可以直接填路径。
- 令牌被撤销 / 无效：会话区顶上红条，不再重连，点「连接设置」换一个。

### 浏览器 UI（SDK）

- 地址填 `https://dock.example.com`（浏览器 UI SDK 的 `gatewayUrl`）。
- 令牌填 `dock device add` 打印的那串（SDK 的 `deviceToken`）。
- SDK 在令牌模式下只接受 `https://`；`http://` 只允许回环（SSH 隧道、本机测试）。

## 安全要点

- 令牌等于这台 Dock 的全部能力（读写工作区、跑命令、看浏览器画面），当密码对待。
- 令牌也能改这台 Dock 的设置（模型、密钥、MCP、插件）、签 / 撤别的设备令牌。
- 怀疑令牌泄漏：撤掉它之后再看一遍 `dock device list`，不认识的设备一并撤销（泄漏的令牌可能已经给自己签了新的）。
- 同一条连接鉴权失败 5 次会被断开；全局限速交给代理。
- 令牌不绑 Origin：拿到令牌的任何客户端都能连，所以只在 https 上传。
- 网关日志（stderr）会记「设备 X 已连接」。
