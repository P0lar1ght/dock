# 会话压缩

> 属于 [TOOLS.md](../../TOOLS.md) 的一部分。改这一块只需要读本文件。

## `compact`

- **ctx**：`"compact"`
- **模型工具**：—

Grok 会话压缩。`install_app` 在 `llm` 之后挂。手动 `/compact [说明]`；上下文达到窗口 85% 时 `maybe_auto`（loop live-lookup，工具轮次结束后、下次采样前）。摘要 prompt / 清洗 / 阈值从 grok-build `xai-grok-compaction` 拷来。成功后 **滚动区保留原对话**（Grok pager 也不擦 scrollback），只把 sampler 历史换成摘要前缀；占用数字按模型历史计。占用 overlay 是 TUI live-look `"context"`，不是模型工具

### 占用怎么算

判定、压缩前记忆 flush 门槛、顶栏 / `/context` / 网关快照共用一个数：
`context_usage::context_tokens_used`。做法同 Grok / Codex。

- 上一次采样上游报了用量：以它的 prompt + completion 为锚点。
  - completion 含推理，下一次请求会原样回放。
  - 锚点之后追加的工具结果、提醒按估算补上。
- 没报用量，或刚压缩 / 换会话 / 清空：system + 工具表 + 图片 + 模型历史全靠估算。
  - 推理照算：有 `reasoning_items` 按原件（明文与密文取大），否则按 `reasoning` 文本。
- 压缩、`/resume` 换会话、清空都作废锚点，下一次采样再落新的。
  - 否则压缩后还按压缩前的数判「压了还超」，整轮被抑制（#154）。

### 压缩进展

`Sessions::compaction()` 是正在进行（或最近一次）的压缩，不落盘。

- 字段：发起方（auto / manual）、阶段、第几次尝试、上次失败原因、摘要已输出 token、
  压缩前后占用、耗时、结果（running / completed / failed / cancelled）。
- 阶段：`memory` 整理记忆（flush 到门槛才有）→ `summary` 生成摘要 → `apply` 替换历史。
- 每次变化发 `session/compaction`（`PageCompaction { page, progress }`）。
  - 摘要流式计数最多 250ms 一条；没变的不发。
- TUI 状态行每帧读它；网关投影成 dock.1 `context/compacted`（见 `cordis-gateway/README.md`）。
- 换会话、清空时清掉。

成功压缩后会在会话目录写入 `compaction/segment_NNN.md` 与 `INDEX.md`（供事后查阅被摘要掉的原文）。
