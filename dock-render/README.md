# dock-render

把模型输出画到界面上：Markdown、Mermaid 图、以后同类渲染都放这里。不是 Agent 循环，也不进 `cordis-tui` 本体。

目录已从 `cordis-render/` 改名；crate 名 **`cordis-markdown`** / **`xai-grok-mermaid`** 暂不变（directory rename only）。

| 子目录 | 干什么 | crate 名 |
|---|---|---|
| [`markdown/`](markdown/) | 终端 Markdown（流式、代码高亮、LaTeX） | `cordis-markdown` |
| [`mermaid/`](mermaid/) | Mermaid → PNG | `xai-grok-mermaid` |
| [`third_party/`](third_party/) | 冻结的 Mermaid 布局栈（dagre / graphlib / to-svg / ordered_hashmap） | 上游原名 |

追加新渲染能力时在本目录新建子 crate，由 `cordis-tui` path-dep 进来即可。
