# 技能

> 属于 [TOOLS.md](../../TOOLS.md) 的一部分。改这一块只需要读本文件。

## `skills`

- **ctx**：`"skills"`
- **模型工具**：—

发现 `SKILL.md`（`$DOCK_HOME/bundled/skills/` 内置 < `{cwd}/skills/` < `~/.dock/skills/` < `{cwd}/.agents/skills/` < `{cwd}/.dock/skills/`，后者同名覆盖；内置是编译期嵌入、启动物化到 bundled 缓存的 dock 自述技能，通用技能不进二进制）。frontmatter：`name`（出厂技能须合法且与目录同名；运行时非法或缺省即回退目录名，仍非法则丢弃该技能）`description`（出厂技能必填 ≤1024；运行时缺省取正文首行、再用 name）`license` `when-to-use` `paths` `user-invocable`（缺省 true）`disable-model-invocation`（缺省 false）。`license` 只在 `/skills` overlay 的路径行展示。向 `"context"` 登记 listing 段（窗口 token ×4 ×**3%**，不要把全文塞进系统提示；预算与渲染规则与工作流共用 `src/listing.rs`）。两道门（`listing::wants_listing`）：**只有主会话拿这一段**，子代理要在 `agents/<type>.yml` 写 `listings: true` 才带（内置只有 `general-purpose` 打开）；且 **header 点名的 `skill` 必须对本会话可见**（在当前预设 allowlist 里且已注册），否则整段不发——`warden` 主代理没有 `skill`，给了也只是诱导一次必然被 allowlist 挡回的调用。`agent/pre-step` 把用户气泡 `/name args` 注入 `SystemReminder`。带 `paths:` 的技能渐进披露：匹配文件被触碰前不进 listing，`tools/execute` 触碰后激活并 `SystemReminder` 通告（listing 首帧冻结不回写，与 Grok 同款）。`tools/execute` 路径靠近 skills 目录时中途发现。`user-invocable` 技能登记成 slash extras（不可盖 `RESERVED_SLASH`）；另登记 `/skills` overlay。fail-open

## `tool-skills`

- **ctx**：→ `"tools"`（live `"skills"`）
- **模型工具**：`skill`

按需读 `SKILL.md` 正文（去 frontmatter）+ `$ARGUMENTS` / `$SKILL_DIR`。参数 `name` 必填、`args` 可选。返回 skill 信封和同目录最多约 10 个附属文件名。`disable-model-invocation` 的技能不进 listing / 本工具，斜杠仍可用。带 `paths:` 的技能激活前本工具也拒载（提示 gated on paths），斜杠不受限。没 skill 目录时工具仍注册。`code` / `cordis` 允许名单含 `skill`。**`register`（进 sampler）**——系统提示的技能 listing 直接点名这个工具，不能再让模型先 `search_tool` 绕一圈
