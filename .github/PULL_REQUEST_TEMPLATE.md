<!-- 标题格式与 commit 一致：feat(tui): … / fix(mcp): … -->

## 改了什么

<!-- 一到三句，能对应 diff。 -->

## 为什么

<!-- 关联 issue：Closes #12。或写触发场景。 -->

## 怎么验证

<!-- 贴真实命令与结果摘要；没跑的部分写「未验证」。 -->

```bash
cargo test -p cordis-spine -p cordis-tui -p cordis-app -p cordis-gateway
```

## 影响面

- [ ] 模型工具面变了 → 已同步 `TOOLS.md`（改工具表必须附 `cargo test -p cordis-spine --test round -- install_app_registers`）
- [ ] 斜杠 / 快捷键 / overlay 变了 → 已同步 `CLI.md`
- [ ] public API、`dock.1` 协议或 `config.toml` 键变了 → 已写迁移方式
- [ ] 落盘格式变了（`meta.json` / `chat_history.jsonl`）→ 已写兼容处理
- [ ] 新依赖 / 新 workspace 成员 → 已在 issue 对齐

## 自查

- [ ] 没有密钥、`.env`、`.dock/` 私人配置、真实会话数据
- [ ] 没有删测试、弱化断言、加长超时来换绿灯
- [ ] 没有夹带无关格式化或顺手重构（格式与 lint 由 CI 的 `cargo fmt --check` / `clippy -D warnings` 把关）
- [ ] 回归测试能打到原缺陷
