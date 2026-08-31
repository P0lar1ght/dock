# vendor/mermaid

Warp 的 Mermaid → SVG 布局栈。只给 `cordis-render/mermaid`（crate `xai-grok-mermaid`）用，不是独立产品。

| 子目录 | 干什么 | crate 名 | 在用 |
|---|---|---|---|
| [`mermaid-to-svg/`](mermaid-to-svg/) | 源码 → SVG | `mermaid-to-svg` | 是 |
| [`dagre_rust/`](dagre_rust/) | dagre 布局 | `dagre_rust` | 是（mermaid-to-svg） |
| [`graphlib_rust/`](graphlib_rust/) | 图算法 | `graphlib_rust` | 是（dagre / mermaid-to-svg） |
| [`ordered_hashmap/`](ordered_hashmap/) | 有序 map | `ordered_hashmap` | 是（dagre / graphlib） |
