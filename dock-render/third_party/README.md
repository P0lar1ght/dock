# dock-render/third_party

Warp 的 Mermaid → SVG 布局栈。只给 `dock-render/mermaid`（crate `xai-grok-mermaid`）用，不是独立产品。

从 `vendor/mermaid/` 迁入；升级时对照 `grok-build` `third_party/`（整棵替换或只换 `mermaid-to-svg`），保持各 crate `LICENSE`/`LICENCE`，Cargo 继续用 Dock 的 path pin（**不要** `workspace = true`）。

| 子目录 | 干什么 | crate 名 | 在用 |
|---|---|---|---|
| [`mermaid-to-svg/`](mermaid-to-svg/) | 源码 → SVG | `mermaid-to-svg` | 是 |
| [`dagre_rust/`](dagre_rust/) | dagre 布局 | `dagre_rust` | 是（mermaid-to-svg） |
| [`graphlib_rust/`](graphlib_rust/) | 图算法 | `graphlib_rust` | 是（dagre / mermaid-to-svg） |
| [`ordered_hashmap/`](ordered_hashmap/) | 有序 map | `ordered_hashmap` | 是（dagre / graphlib） |

```text
xai-grok-mermaid
  └── mermaid-to-svg          (MIT)
        ├── dagre_rust        (Apache-2.0)
        │     ├── graphlib_rust
        │     └── ordered_hashmap
        └── graphlib_rust     (Apache-2.0)
              └── ordered_hashmap
```

本地补丁与升级清单写在各 crate `Cargo.toml` 头部；`mermaid-to-svg` 含 flowchart `:::class` / `classDef` / `&` grouping（对齐 grok-build tip）。
