# vendor

冻结副本。不 path-dep `grok-build/`；升级时整棵替换后再改 path。包名保持上游 / Grok 原名。同类收进一个子目录，后续追加也往对应入口里放。

| 目录 | 干什么 |
|---|---|
| [`mermaid/`](mermaid/) | Warp Mermaid 布局栈，给 `cordis-render/mermaid` 用 |
| [`xai/`](xai/) | Grok 拷贝：工作流、模糊搜文件；`grok-tools` 只作对照源 |
