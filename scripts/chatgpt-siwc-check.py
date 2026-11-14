#!/usr/bin/env python3
"""ChatGPT 订阅通道（SIWC）实测清单。

设计稿里的验证清单，每项**只发一次**短请求（回复一个词就够），不重复跑。
分两段：

    A 段（不需要账号）：授权端点、官方公钥、两条推理路由是否在。
    B 段（需要 access token）：模型列表、最小推理、工具调用、reasoning 档位。

用法：

    # A 段：随便什么时候都能跑
    python3 scripts/chatgpt-siwc-check.py

    # B 段：先 /auth login chatgpt 拿到凭据，再（token 从**环境变量**读）
    DOCK_CHATGPT_ACCESS_TOKEN=... python3 scripts/chatgpt-siwc-check.py

    # 或者让它自己从凭据目录里读（不用把 token 写进命令行）
    python3 scripts/chatgpt-siwc-check.py --from-credentials

**token 不要走命令行参数**：命令行会进 shell 历史，也会出现在 `ps` 的输出里
（同机器上别的用户看得见）。环境变量只留在本进程的环境里。

从凭据目录取 token（放进环境变量，**不要打印到屏幕**）：
    export DOCK_CHATGPT_ACCESS_TOKEN="$(python3 - <<'EOF'
    import json, pathlib
    p = pathlib.Path.home() / ".dock/credentials/chatgpt"
    for f in p.glob("*.json"):
        print(json.loads(f.read_text())["access_token"])
    EOF
    )"

每项的请求 id（x-request-id）都会打出来——官方文档说凭它找支持。

请求一律带 `stream: true`：**官方那条通道只按流式收**，`stream: false` 的
结果没有参考价值（多半直接被拒）。
"""

import argparse
import json
import os
import sys
import urllib.error
import urllib.request

ISSUER = "https://auth.openai.com"
DISCOVERY = f"{ISSUER}/.well-known/openid-configuration"
API = "https://api.openai.com/v1"

OK, BAD, SKIP = "✓", "✗", "—"


def request(url, method="GET", body=None, token=None, timeout=30):
    """发一次请求。返回 (status, headers, 解析后的 body)。不重试。"""
    data = None
    headers = {"accept": "application/json"}
    if body is not None:
        data = json.dumps(body).encode()
        headers["content-type"] = "application/json"
    if token:
        headers["authorization"] = f"Bearer {token}"
    req = urllib.request.Request(url, data=data, headers=headers, method=method)
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            raw = resp.read()
            return resp.status, dict(resp.headers), _parse(raw)
    except urllib.error.HTTPError as e:
        raw = e.read()
        return e.code, dict(e.headers), _parse(raw)
    except urllib.error.URLError as e:
        return 0, {}, {"error": {"message": f"连不上：{e.reason}"}}


def _parse(raw):
    """解析响应体。SSE（`stream: true`）也要能读。"""
    text = raw.decode("utf-8", "replace")
    if "data:" in text and "response." in text:
        return _parse_sse(text)
    try:
        return json.loads(text)
    except ValueError:
        return {"_raw": text[:400]}


def _parse_sse(text):
    """把 SSE 收成一份跟非流式响应同形的 JSON。

    增量文本拼到 `output_text`，最后一条 `response.completed` 的 response
    放到 `response`——B 段那些检查读的就是这两个字段。
    """
    out = {"output_text": "", "response": {}, "events": []}
    for line in text.splitlines():
        if not line.startswith("data:"):
            continue
        data = line[5:].strip()
        if not data or data == "[DONE]":
            continue
        try:
            ev = json.loads(data)
        except ValueError:
            continue
        kind = ev.get("type", "")
        out["events"].append(kind)
        if kind == "response.output_text.delta":
            out["output_text"] += ev.get("delta", "")
        elif kind in ("response.completed", "response.incomplete", "response.failed"):
            out["response"] = ev.get("response", {}) or {}
            if kind == "response.failed":
                out["error"] = out["response"].get("error", {})
    return out


def token_from_credentials():
    """从凭据目录里取 access token。

    取**最近落盘**的那一份（mtime 最新的 json）。不打印——只进程序内部。
    """
    import glob
    import pathlib

    d = pathlib.Path.home() / ".dock/credentials/chatgpt"
    files = [pathlib.Path(f) for f in glob.glob(str(d / "*.json"))]
    if not files:
        return None
    newest = max(files, key=lambda p: p.stat().st_mtime)
    try:
        return json.loads(newest.read_text()).get("access_token")
    except (OSError, ValueError):
        return None


def completed(body):
    """流式响应里最终那条 `response.completed` 的内容。

    SSE 的形状是 `{"response": {...}}`；非流式（万一上游给了）就在顶层。
    """
    return body.get("response") or body


def request_id(headers):
    for name in ("x-request-id", "request-id", "cf-ray"):
        if headers.get(name):
            return headers[name]
    return "-"


def show(label, ok, detail="", rid="-"):
    mark = OK if ok else BAD
    line = f"  {mark} {label}"
    if detail:
        line += f" — {detail}"
    if rid and rid != "-":
        line += f"  [request-id: {rid}]"
    print(line)
    return ok


# ---------- A 段：不需要账号 ----------

def phase_a():
    print("A 段：公开端点（不需要账号）")

    status, _, disco = request(DISCOVERY)
    if status != 200:
        show("OIDC discovery 可达", False, f"HTTP {status}")
        return
    show("OIDC discovery 可达", True, disco.get("issuer", ""))

    checks = [
        ("issuer 与我们写的一致", disco.get("issuer") == ISSUER, disco.get("issuer")),
        ("授权端点", disco.get("authorization_endpoint") == f"{ISSUER}/api/accounts/authorize", disco.get("authorization_endpoint")),
        ("换码端点", disco.get("token_endpoint") == f"{ISSUER}/api/accounts/oauth/token", disco.get("token_endpoint")),
        ("撤销端点（存在即够）", bool(disco.get("revocation_endpoint")), disco.get("revocation_endpoint")),
        ("PKCE 只支持 S256", disco.get("code_challenge_methods_supported") == ["S256"], str(disco.get("code_challenge_methods_supported"))),
        ("ID token 只用 RS256 签", disco.get("id_token_signing_alg_values_supported") == ["RS256"], str(disco.get("id_token_signing_alg_values_supported"))),
        ("支持 refresh_token 授权类型", "refresh_token" in (disco.get("grant_types_supported") or []), str(disco.get("grant_types_supported"))),
        ("公开客户端（auth method none）", "none" in (disco.get("token_endpoint_auth_methods_supported") or []), str(disco.get("token_endpoint_auth_methods_supported"))),
    ]
    for label, ok, detail in checks:
        show(label, ok, detail)

    scopes = disco.get("scopes_supported") or []
    show(
        "订阅那两条 scope 不在 OIDC 列表里（按 SIWC 文档发即可）",
        True,
        f"OIDC 只列 {scopes}",
    )

    jwks_uri = disco.get("jwks_uri")
    if not jwks_uri:
        show("官方公钥（JWKS）可取", False, "discovery 里没有 jwks_uri")
    else:
        status, _, jwks = request(jwks_uri)
        keys = jwks.get("keys") or []
        rsa = [k for k in keys if k.get("kty") == "RSA"]
        rs256 = [k for k in rsa if k.get("alg") == "RS256"]
        show("官方公钥（JWKS）可取", status == 200, f"{len(keys)} 把钥匙")
        show("公钥是 RSA/RS256（我们只验这一种）", len(rs256) == len(rsa) and bool(rsa), f"{len(rs256)}/{len(rsa)}")

    # 两条推理路由：没 token 必须是 401 + 标准错误信封
    status, headers, body = request(f"{API}/models", token=None)
    ok = status == 401 and isinstance(body.get("error"), dict)
    show("GET /v1/models 路由存在（无 token → 401）", ok, f"HTTP {status}", request_id(headers))

    status, headers, body = request(
        f"{API}/responses",
        method="POST",
        body={
            "model": "gpt-5.6-terra",
            "instructions": "Reply with the single word: ok",
            "input": [{"type": "message", "role": "user", "content": "hi"}],
            "store": False,
            "stream": True,
        },
    )
    ok = status == 401 and isinstance(body.get("error"), dict)
    show("POST /v1/responses 路由存在（无 token → 401）", ok, f"HTTP {status}", request_id(headers))
    print()


# ---------- B 段：需要 access token ----------

def phase_b(token):
    print("B 段：需要 access token 的项")

    status, headers, body = request(f"{API}/models", token=token)
    rid = request_id(headers)
    if status != 200:
        show("GET /v1/models", False, f"HTTP {status} {body.get('error', {}).get('message', '')}", rid)
        return
    models = body.get("models") or []
    listed = [m for m in models if m.get("visibility") in (None, "list")]
    show("GET /v1/models", True, f"{len(models)} 个模型，其中 {len(listed)} 个 visibility=list", rid)
    for m in listed[:12]:
        print(f"      · {m.get('slug')}  ({m.get('display_name') or '-'})")
    if not listed:
        print("      （没有 visibility=list 的模型；Free 账号可能就是这样）")

    slug = None
    for m in listed:
        if m.get("slug"):
            slug = m["slug"]
            break
    if not slug:
        show("后续几项（需要模型 slug）", SKIP == OK, "没有可用模型")
        print()
        return

    # 最小请求：只带 instructions + 一条 user
    status, headers, body = request(
        f"{API}/responses",
        method="POST",
        body={
            "model": slug,
            "instructions": "Reply with the single word: ok",
            "input": [{"type": "message", "role": "user", "content": "hi"}],
            "store": False,
            "stream": True,
        },
        token=token,
    )
    rid = request_id(headers)
    detail = ""
    ok = status == 200
    if ok:
        text = "".join(
            part.get("text", "")
            for item in (completed(body).get("output") or [])
            if item.get("type") == "message"
            for part in (item.get("content") or [])
        )
        detail = f"回复：{text.strip()[:40]!r}"
        show("最小请求（instructions + 一条 user）", True, detail, rid)
        usage = completed(body).get("usage") or {}
        show("usage 字段回来了", bool(usage), json.dumps(usage, ensure_ascii=False)[:120], rid)
    else:
        show("最小请求（instructions + 一条 user）", False, f"HTTP {status} {json.dumps(body, ensure_ascii=False)[:200]}", rid)
        print()
        return

    # 工具调用：一个 namespace 里的 function
    status, headers, body = request(
        f"{API}/responses",
        method="POST",
        body={
            "model": slug,
            "instructions": "Call the get_weather tool for Beijing. Do not answer directly.",
            "input": [{"type": "message", "role": "user", "content": "weather in Beijing?"}],
            "tools": [
                {
                    "type": "namespace",
                    "name": "tools",
                    "tools": [
                        {
                            "type": "function",
                            "name": "get_weather",
                            "description": "Get the weather of a city",
                            "parameters": {
                                "type": "object",
                                "properties": {"city": {"type": "string"}},
                                "required": ["city"],
                            },
                        }
                    ],
                }
            ],
            "store": False,
            "stream": True,
        },
        token=token,
    )
    rid = request_id(headers)
    kinds = [i.get("type") for i in (completed(body).get("output") or [])] if status == 200 else []
    show(
        "工具调用（namespace 里的 function）",
        status == 200 and "function_call" in kinds,
        f"HTTP {status} output={kinds}" if status == 200 else f"HTTP {status}",
        rid,
    )

    # reasoning 档位：文档没提，实测一下收不收
    status, headers, body = request(
        f"{API}/responses",
        method="POST",
        body={
            "model": slug,
            "instructions": "Reply with the single word: ok",
            "input": [{"type": "message", "role": "user", "content": "hi"}],
            "reasoning": {"effort": "low", "summary": "concise"},
            "store": False,
            "stream": True,
        },
        token=token,
    )
    rid = request_id(headers)
    show(
        "reasoning effort/summary 收不收",
        status == 200,
        f"HTTP {status}" if status != 200 else "收了",
        rid,
    )
    print()


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument(
        "--from-credentials",
        action="store_true",
        help="从 ~/.dock/credentials/chatgpt/ 读 access token（不走命令行传 token）",
    )
    ap.add_argument("--skip-a", action="store_true", help="跳过 A 段")
    args = ap.parse_args()

    if not args.skip_a:
        phase_a()
    token = os.environ.get("DOCK_CHATGPT_ACCESS_TOKEN")
    if args.from_credentials and not token:
        token = token_from_credentials()
    if token:
        phase_b(token)
    else:
        print("B 段跳过（没有 token）。登录后拿 token 再跑一次。")
        print("  Dock 里：/auth login chatgpt")
        print("  然后：export DOCK_CHATGPT_ACCESS_TOKEN=… 或加 --from-credentials")
    return 0


if __name__ == "__main__":
    sys.exit(main())
