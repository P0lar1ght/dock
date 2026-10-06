# Agent 活动（示例插件）

实时看 Agent 在做什么。一颗 Rhai 磁盘插件，用到插件视图的三个扩展点
（`docs/PLUGIN-VIEWS.md`）：

- 插件面板「Agent 活动」：
  - 状态（空闲 / 运行中 · 第 N 步 / 记录已暂停）、完成轮次、工具调用次数、上一轮用时；
  - 「工具用量」表、「最近活动」列表（你说的、工具、回复、本轮结束）；
  - 底部按钮：清零、暂停记录 / 继续记录。
- 状态项：输入框工具栏上「运行中 · 第 3 步」/「空闲 · 工具 12 次」，点它打开面板。
- 设置卡：最近活动保留几条（5–50）、要不要显示状态项。

只观察（`session/event`、`agent/step-start`、`agent/turn-end` 都回 `()`），不改 Agent 的行为。
数据在内存里：Dock 重启就清零；统计的是所有会话的主 Agent（不分会话）。

适合在 GUI 里把面板弹出成独立窗、置顶，挂在副屏上看。

## 装上

用户级（所有项目都加载）：

```bash
mkdir -p ~/.dock/plugins
cp -R examples/plugins/activity ~/.dock/plugins/activity
```

只给某个项目：拷到 `<项目>/.dock/plugins/activity/`。

重启 Dock（或在设置 › 插件与技能里「立即加载」）。
终端里 `/cordis` 能看到它；面板在 GUI 右侧分屏菜单「插件面板」组里，TUI 里是 `slot` 浮层。

## 卸下

删掉那个目录，或在设置 › 插件与技能里关掉开关。

## 改它

`source.rhai` 里每个回调都有注释。Rhai 的闭包捕获变量是共享的：
`state` 被所有回调改的是同一份。Host API 见 `skills/cordis-plugin-development/SKILL.md`。
回归：`cargo test -p cordis-spine --test dynamic -- example_activity_plugin`。
