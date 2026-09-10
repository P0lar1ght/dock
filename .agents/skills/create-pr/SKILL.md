---
name: create-pr
description: >-
  Open a Dock pull request: verify the branch and tests, push, fill the PR
  template with changes, verification evidence, and issue links. Use when the
  user asks to push a branch or open/update a pull request.
user-invocable: true
---

# 开 PR

用户明确要求 push / 开 PR 时才用。开 PR 前必须有已提交的改动（没提交先走 git-commit skill）。

## 1. 前提检查

```bash
git status -sb              # 有无未提交改动；不在 main 上直接提交
git log --oneline origin/main..HEAD
git diff --stat origin/main...HEAD
```

- 分支只包含本次任务的 commit，无夹带的无关改动。
- 验证跑过，并留下可复制的证据：命令 + 结果摘要。
- 影响模型工具面 / 斜杠 / overlay / 协议 / 配置键 → `TOOLS.md`、`CLI.md`、`docs/ARCHITECTURE.md` 已同步。

## 2. 跑验证

按改动范围选，不要为小改跑全量：

```bash
cargo test -p cordis-spine -p cordis-tui -p cordis-app -p cordis-gateway
cargo test -p cordis-spine --test round -- install_app_registers   # 改工具表时必跑
cargo clippy -p <改动的 crate> --all-targets
```

`embed-sdk/` 改动：

```bash
cd embed-sdk && npm ci && npm run build
```

失败就修或说明，不要用重试、加长超时、弱断言藏起来。

## 3. Push

```bash
git push -u origin <branch>
```

不要 force push 到已推送并被 review 的分支；不要在没被要求时动 `main`、发 tag 或发版。

## 4. 写 PR

用 `.github/PULL_REQUEST_TEMPLATE.md` 的结构：

- **改了什么** — 一到三句，能对应 diff。
- **为什么** — 关联 issue 或触发场景。
- **怎么验证** — 贴真实命令与结果；没跑的部分明确写「未验证」。
- **影响面** — 工具面 / 斜杠 / 协议 / 配置 / 落盘格式有没有变，变了写迁移方式。
- **自查** — 没夹带密钥 / 私人配置 / 真实数据；没删或弱化测试；没顺手重构。

标题与 commit 同格式（Conventional Commits）。issue 用 `Closes #12` 关联。

## 5. 之后

- 不擅自 merge、不擅自关 issue。
- Review 提出改动时：新 commit 追加，不要 force push 覆盖已被 review 的历史。
- 大改动（新依赖、public API、数据模型、权限模型）先在对齐阶段讨论过再开 PR。
