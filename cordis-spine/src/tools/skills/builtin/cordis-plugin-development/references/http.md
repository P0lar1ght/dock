# HTTP、文件上传、编解码、正则、时间

脚本自带 HTTP 和 JSON，一颗插件就能把任意 HTTP 接口变成 Dock 工具。
本页函数**只在运行期有**：写在 `apply` 或 `execute` / 回调里，不要写在顶层
（define 期的预检会求值顶层，在那里发请求就绕过了权限门）。
完整例子：`$SKILL_DIR/examples/api-tool.rhai`。

## `http_request(#{ … })`

| 字段 | 说明 |
| --- | --- |
| `url` | 必填 |
| `method` | 默认 `"GET"` |
| `headers` | map；值里不能有换行 |
| `body` | `String` 或 `Blob`；与 `multipart` 互斥 |
| `multipart` | 数组 `#{ name, value? \| blob?, filename?, content_type? }`，按数组顺序发（OSS PostObject 要把文件放最后） |
| `timeout_secs` | 默认 30，最多 120 |

返回 `#{ status, ok, url, headers, body, body_blob }`：
`body` 是有损 UTF-8 字符串，`body_blob` 是原始字节（签名 / 转传用）。

```rhai
let resp = http_request(#{
    url: "https://api.github.com/repos/" + args.repo + "/issues/" + args.n,
    headers: #{ "Accept": "application/vnd.github+json", "User-Agent": "dock",
                "Authorization": "Bearer " + token }
});
if !resp.ok { return "HTTP " + resp.status + ": " + resp.body; }
let issue = parse_json(resp.body);
issue.title + "\n\n" + issue.body
```

### 规矩（分清是 bug 还是规则）

- **4xx / 5xx 不抛错**：`ok: false` 加状态码，自己查 `resp.ok`。
  只有网络失败、参数错、SSRF 拦截、权限被拒才抛。
- **3xx 原样返回，不跟随**：要下一跳就拿 `Location` 再请求一次（换 host 会再问权限）。
- **内网和回环被拦**（与 `web_fetch` 同一套 SSRF 规则，先解析 DNS）。
  除非配置里开了 `[toolset.web_fetch] allow_local`，否则连不了 `localhost`。
- **每个 host 第一次请求会问用户**，「总是允许」按 host 记住。
- **凭据放 `headers`**，`https://user:pw@host/` 会被拒。
  像 Telegram 那种把 token 放在 URL 路径里的，按 host 权限 + 密钥处理，没有单独的机制。
- 响应体 ≤ 4 MiB，≤ 32 个响应头。请求体两档上限：
  - 脚本自己拼的（字符串、`to_json`、`blob(…)`）≤ 1 MiB；
  - `host.read_bytes(path)` 读来的 ≤ 16 MiB。读完再改（追加、切片）就退回 1 MiB 档。
  - 超了直接抛错，不会悄悄截断。

## 上传文件：`host.read_bytes(path)`

- 工具参数里**只传路径**，在 `execute` 里读成 `Blob` 再交给 `body` / `multipart`。
  不要让模型生成或内嵌文件内容、base64、大段 CSV——这也是走 16 MiB 档的唯一路子。
- 路径规则同 `read_file`：相对 cwd、绝对路径或 `~/…`。一次一个文件，不能列目录、通配、写入。
- 每个路径第一次读会弹 `read_bytes {path}` 确认（只显示路径），按路径记住。
  这道确认就是控制，不是沙箱。
- 空文件可以；超过 16 MiB 抛错。

## JSON

- `parse_json(text)` → 可以下标访问的 map / 数组。
- `value.to_json()` → 字符串，用作 `body`。
- 取不存在的键得到 `()`，用 `x ?? 默认值` 兜底。

## 正则

`regex_is_match` / `regex_find` / `regex_captures` / `regex_replace`，基于 `regex` crate。
- 模式 ≤ 4 KiB，文本 ≤ 1 MiB，替换结果 ≤ 1 MiB，最多 10000 次替换。
- `regex_replace` 只支持 `$n` / `${n}` / `$$`，没有 fancy-regex（无回溯引用、无环视）。

## 编解码与签名（纯函数，无 I/O、无确认）

| 调用 | 说明 |
| --- | --- |
| `to_base64` / `from_base64` | 标准 Base64；输入 `String` 或 `Blob`，解码得 `Blob` |
| `to_base64url` / `from_base64url` | URL 安全，编码不带填充，解码两种都收 |
| `url_encode` / `url_decode` | 查询参数用，空格 → `%20` |
| `to_hex` / `from_hex` | 小写十六进制；奇数长度或非法字符报错 |
| `sha256` / `sha256_blob` | 十六进制 / 32 字节 `Blob` |
| `hmac_sha256(key, msg)` / `hmac_sha256_blob` | 同上 |

- 输入超过 1 MiB 抛错；例外是被**哈希**的那一侧（`sha256` 的输入、HMAC 的 message）
  可到 16 MiB，方便给整份上传文件签名。
- `Blob` 转字符串：`blob.as_string()`（UTF-8 时）。

Basic 认证：

```rhai
let auth = "Basic " + to_base64(args.user + ":" + args.pass);
```

## 时间

| 调用 | 返回 |
| --- | --- |
| `utc_now()` | `#{ secs, ms, date, rfc3339, year, month, day, hour, minute, second }` |
| `unix_time()` / `unix_time_ms()` | 纪元秒 / 毫秒 |
| `utc_date()` / `now_date()` | `YYYY-MM-DD` |

API 签名用 `utc_now()`，别用 `timestamp()`（那是单调时钟，不是墙上时间）。
