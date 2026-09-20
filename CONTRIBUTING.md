# 贡献

Dock 是 Rust workspace + 一个 JS SDK。这份文件讲人类怎么提改动；agent 的硬规则在 [AGENTS.md](AGENTS.md)，命令细节在 [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md)。

## 开始之前

1. 有 issue 的话先读 issue；没有就先把要改的行为写清楚（触发方式、期望、现状）。
2. 一个 PR 只做一件事。
3. 工具链与版本约束见 [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md)。
4. 大改动（新依赖、public API、数据模型、权限模型、跨包重命名）先开 issue 对齐，再写代码。

## 开发

```bash
cargo run -p cordis-app
cargo test -p cordis-spine -p cordis-tui -p cordis-app -p cordis-gateway
cargo test -p cordis-spine --test round -- install_app_registers
cargo fmt -p <改过的 crate>              # CI 会跑 cargo fmt --check，别对 vendor/ 跑
cargo clippy -p <你改的 crate> --all-targets
```

`embed-sdk/` 的改动：

```bash
cd embed-sdk && npm ci && npm run build
```

命令的完整清单与注意事项（存量 fmt / clippy 差异、测试范围选择）见 [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md)。

## 提交

- Conventional Commits：`feat(tui): …`、`fix(mcp): …`、`docs(agents): …`。scope 用 crate 或产品面短名。
- 一个 commit 只做一件事。只 stage 任务相关文件。
- 不提交密钥、`.env`、`.dock/` 下的私人配置、真实会话数据。
- 完整步骤见 [.agents/skills/git-commit/SKILL.md](.agents/skills/git-commit/SKILL.md)。

## PR

- 标题与 commit 同格式。
- 描述里写清：改了什么、为什么、怎么验证（贴命令与结果）。
- 关联 issue（`Closes #12`）。
- 改动如果影响模型工具面或人操作的面，同步 `TOOLS.md` / `CLI.md`。
- CI 会在 PR 上跑默认回归集合，见 [.github/workflows/ci.yml](.github/workflows/ci.yml)。
- 步骤见 [.agents/skills/create-pr/SKILL.md](.agents/skills/create-pr/SKILL.md)。

行为边界见 [AGENTS.md](AGENTS.md) 的 Boundaries；`vendor/` 另有冻结副本约束，见 [vendor/AGENTS.md](vendor/AGENTS.md)。

## 许可证

仓库根 `Cargo.toml` 与第一方 crate 声明 `license = "Apache-2.0"`，全文见 [LICENSE](LICENSE)，版权归属与第三方清单见 [NOTICE](NOTICE)。`dock-render/` 的两个第一方 crate 与 `dock-render/third_party/`、`vendor/` 下的上游拷贝沿用各自上游许可证（多为 Apache-2.0，`dock-render/third_party/mermaid-to-svg` 为 MIT；副本见同目录 `LICENSE`，以各 crate 的 `Cargo.toml` 为准）。提交贡献即表示你同意以对应许可证发布你的改动。
