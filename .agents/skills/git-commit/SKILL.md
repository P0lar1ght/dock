---
name: git-commit
description: >-
  Write a Dock commit: check the working tree, stage only task-relevant files,
  and produce one Conventional Commit with a crate/product scope. Use when the
  user asks to commit, amend, or split staged changes into commits.
user-invocable: true
---

# 提交改动

用户明确要求 commit 时才用这条流程。**不要**在没被要求时自行提交。

## 1. 看状态

```bash
git status -sb
git log --oneline -10        # 对齐现有 scope 与用词
git diff                     # 未 staged
git diff --staged            # 已 staged
```

工作区里有与本次任务无关的改动时，跳过它们，不要顺手清理、不要 `git stash`、不要 `reset --hard`。

## 2. 检查内容

- 没有密钥、`.env`、`.dock/` 下的私人配置、`~/.dock/mcp_credentials.json`、真实会话数据。
- 没有夹带格式化 / 重构产生的无关 diff。
- 工具面或人操作面变了 → `TOOLS.md` / `CLI.md` 已同步。
- 测试按改动范围跑过（`.agents` 外的默认集合见 `docs/DEVELOPMENT.md`）。

## 3. Stage

```bash
git add <只加本次任务的文件>
git diff --staged
```

需要拆成多个 commit 时按主题拆（一个 commit 一件事），不要用 `-a` 一把梭。

## 4. 写信息

Conventional Commits，scope 用 crate 或产品面短名，与 `git log` 里的历史一致：

```
feat(tui): /preset Roles 支持自定义 id 与显示名
fix(mcp): append non-image structuredContent into tool text
docs(agents): 重写根政策并拆出 docs/ARCHITECTURE.md
test(gateway): cover one-time ticket reuse
chore(deps): bump chromiumoxide
```

规则：

- 类型：`feat` `fix` `refactor` `perf` `test` `docs` `build` `ci` `chore`。
- 标题说清行为变化，不写「update code」「fix bug」。
- 用户可见文案是中文；标题与正文语言跟本仓库历史保持一致（中文或英文均可，但同一次提交别混）。
- 破坏性改动：`feat(tools)!: …`，并在正文写迁移方式。
- 不写「已验证」除非真的跑过；把实际命令与结果放进正文。

多行：

```bash
git commit -m "fix(gateway): 一次性 ticket 复用拒绝" -m "复现：POST /v1/pairing/exchanges 用同一 ticket 两次。第二次现在返回 4xx。"
```

## 5. 收尾

```bash
git status -sb
git show --stat HEAD
```

`push` 与开 PR 是独立动作，用户没要求就不做（开 PR 见 create-pr skill）。
