#!/usr/bin/env python3
"""把 Wonder `get_element_code` 的导出结果存进 wonder-ui/。

用法：
  wonder-save.py <结果.json> <id>=<目录>[:说明] ...

例：
  wonder-save.py out.txt act=chat/ComposerV2:输入框底栏真实控件

- 结果文件是 MCP 返回的 JSON（{"results":[{dataNodeId, code}]}）。
- 目录相对 wonder-ui/；写 <目录名>.jsx 和 README.md。
- 结果被截断（JSON 解析失败）会直接报错，不写半截。
"""
import json
import os
import re
import sys

ROOT = os.path.join(os.path.dirname(__file__), "..", "wonder-ui")


def label(code: str) -> str:
    m = re.search(r'data-node-label="([^"]+)"', code)
    return m.group(1) if m else ""


def main() -> None:
    if len(sys.argv) < 3:
        sys.exit(__doc__)
    results = json.load(open(sys.argv[1]))["results"]
    codes = {r["dataNodeId"]: r.get("code") for r in results}
    for spec in sys.argv[2:]:
        node, _, rest = spec.partition("=")
        path, _, note = rest.partition(":")
        code = codes.get(node)
        if not code:
            sys.exit(f"结果里没有 {node}")
        name = path.rstrip("/").split("/")[-1]
        out = os.path.join(ROOT, path)
        os.makedirs(out, exist_ok=True)
        with open(os.path.join(out, f"{name}.jsx"), "w") as f:
            f.write(code.rstrip() + "\n")
        with open(os.path.join(out, "README.md"), "w") as f:
            f.write(f"# {name}\n\n{note or label(code)}\n\n")
            f.write(f"- `{name}.jsx`：`{node}` {label(code)}\n")
        print(f"{node} -> {path}")


if __name__ == "__main__":
    main()
