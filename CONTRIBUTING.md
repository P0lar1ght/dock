# 贡献

Dock 是 Rust workspace + 一个 JS SDK。这份文件讲人类怎么提改动；agent 的硬规则在 [AGENTS.md](AGENTS.md)，命令细节在 [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md)。

## 开始之前

1. 有 issue 的话先读 issue；没有就先把要改的行为写清楚（触发方式、期望、现状）。
2. `git status -sb` 确认工作区状态，别把无关改动混进分支。
3. 检查你的工具链：rustc **1.88+**（README 声明，未实测；版本说明见 [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md)）、Node **>= 18**（`embed-sdk/package.json` 的 `engines.node`）。
4. 大改动（新依赖、public API、数据模型、权限模型、跨包重命名）先开 issue 对齐，再写代码。

## 开发

```bash
cargo run -p cordis-app
cargo test -p cordis-spine -p cordis-tui -p cordis-app -p cordis-gateway
cargo test -p cordis-spine --test round -- install_app_registers
cargo clippy -p <你改的 crate> --all-targets
rustfmt --edition 2021 <你改过的文件>
```

`embed-sdk/` 的改动：

```bash
cd embed-sdk && npm ci && npm run build
```

注意：仓库存量 clippy warning 与 fmt diff 都存在，只格式化你改过的文件；不要全仓 `cargo fmt --all` 或 `clippy --fix`，那会淹没 review。

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
- 步骤见 [.agents/skills/create-pr/SKILL.md](.agents/skills/create-pr/SKILL.md)。

## 不要做的事

- 改仓外的 `grok-build/`、`deepseek-harness/`、上游 JS `cordis/`；也不要 path-dep 它们。
- 就地改 `vendor/` 里的冻结副本（要改先走 upstream 同步，见 [vendor/README.md](vendor/README.md)）。
- 删测试、弱化断言、加长超时来换绿灯。
- 扩大 scope 顺手重构。
- 在 PR 里夹带格式化的无关文件。

## 许可证

仓库根 `Cargo.toml` 声明 `license = "MIT"`。仓库当前**没有** `LICENSE` 文件（TODO）。提交贡献即表示你同意以该许可证发布你的改动。
