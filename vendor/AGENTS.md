# vendor AGENTS.md

本目录全是**冻结副本**（上游 / Grok 源码）。规则与根 [AGENTS.md](../AGENTS.md) 相同之处不重复；这里是本目录的额外硬约束。冲突时以本文件为准。

## 硬约束

- **不为满足本仓需求就地改这里的代码。** 需要行为变化时：改用它的一等 crate，或在根 crate 里包一层适配。
- 升级走整棵替换：替换目录 → 跑 `cargo test -p cordis` 与相关 crate → 在提交信息里写清来源与版本。新依赖 / 换版本属于「先问」。
- 包名保持上游原名（`graphlib_rust`、`mermaid-to-svg` 之类），不要重命名成 `cordis-*`。
- 不要 `path-dep` 仓外的 `grok-build/`、`deepseek-harness/`、上游 JS `cordis/`；要源码就复制进这里。
- `vendor/xai/grok-tools` 只作对照源，**不是** workspace 成员；成员列表以根 `Cargo.toml` 为准。

## 现状

| 目录 | 内容 |
|---|---|
| `mermaid/` | Warp Mermaid 布局栈：`dagre_rust`、`graphlib_rust`、`mermaid-to-svg`、`ordered_hashmap`，给 `cordis-render/mermaid`（crate `xai-grok-mermaid`）用 |
| `xai/` | Grok 拷贝：`xai-workflow`、`xai-fuzzy-file-search`（workspace 成员）、`grok-tools`（仅对照源） |

## 许可证

沿用上游许可证，不要改成第一方的 `cordis-*` 那套：`xai/` 与 `mermaid/` 多数目录是 Apache-2.0（`mermaid-to-svg` 是 MIT）。声明在各 crate 的 `Cargo.toml`，Apache-2.0 副本放在对应目录的 `LICENSE`。替换或升级目录时保持这些文件与上游一致。

## 验证

```bash
cargo test -p xai-workflow
cargo test -p xai-fuzzy-file-search
cargo test -p cordis-markdown -p xai-grok-mermaid
```

`vendor/` 的 crate 测试失败时先判断是不是上游本身的问题：是就作为已知边界上报，不要就地打补丁藏过去。
