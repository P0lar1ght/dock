# 会话压缩

> 属于 [TOOLS.md](../../TOOLS.md) 的一部分。改这一块只需要读本文件。

## `compact`

- **ctx**：`"compact"`
- **模型工具**：—

Grok 会话压缩。`install_app` 在 `llm` 之后挂。手动 `/compact [说明]`；上下文达到窗口 85% 时 `maybe_auto`（loop live-lookup，工具轮次结束后、下次采样前）。摘要 prompt / 清洗 / 阈值从 grok-build `xai-grok-compaction` 拷来。成功后 **滚动区保留原对话**（Grok pager 也不擦 scrollback），只把 sampler 历史换成摘要前缀；占用数字按模型历史计。占用 overlay 是 TUI live-look `"context"`，不是模型工具

### 压缩后模型历史长什么样

只有三段：**首条真实用户消息 + 最后一条用户消息**（`<user_query>` 包裹）+ **一条装摘要的 `SystemReminder`**。**一条 assistant 轮次都不留**，工具链也不留。与 Codex 的 `build_compacted_history`（只留 user 消息 + 摘要）一致，也与 Grok `CompactionStateContext::for_compaction()` 清空 `recent_messages` 的意图一致 —— 那段注释写的正是同一个问题：保留下来的尾巴会「重新暗示模型去重读同样的文件，却几乎腾不出空间」。

- 早先照抄 Grok 的 `extract_messages_since_last_user`，把「最后一条用户消息之后」的轮次整段保留（含推理）。那是压缩压不动的根因：`maybe_auto` 在每个采样步之前跑，触发时「最后一条用户消息之后」正好是这一轮刚跑完的整条工具链，而推理是占用的大头 —— 折叠了旧历史却把它原样留下，占用几乎不降。
- 当时保留它的理由是 DeepSeek thinking 模式：请求带 `tools` 时，之前每一轮的 `reasoning_content` 都必须回传，缺了 400。但那条约束只对**请求里还在的** assistant 消息成立（官方文档：不带 `tools` 时 reasoning_content 被忽略、也不计入上下文）。整段丢掉之后不存在缺推理的旧轮次，所以不触发。
- 代价是摘要成了模型唯一的依据，所以**摘要请求保留每条工具结果的前 8KB**（`SUMMARY_TOOL_OUTPUT_HEAD_BYTES`，按行边界截断并标「已截断」），且只留最近 40 条（更早的换成一行占位）。原先工具结果整个丢掉，模型会照着摘要重调一遍刚跑过的 `read_file` / `grep` —— Grok 的 `for_compaction` 注释把这件事点名为压缩后的头号失败模式。推理仍然清空。
- **摘要请求要放得进窗口**。自动压缩在 85% 触发，摘要请求带着这些工具输出可以接近原历史的大小，只剩 15% 给摘要的输出上限。所以先按估算收到窗口的 70%（`SUMMARY_INPUT_BUDGET_PERCENT`）：放不下就把带的工具输出条数减半，直到放下；一条不带还放不下就照发。上游仍报「上下文超限」时，下一次再少带一半，不原样重发（原样重发结论一样，整次压缩失败后自动压缩被抑制到下一条用户消息，长工具循环会撞上真正的超限）。限流、断网等别的错误照原样重试。
- **工具调用的实参也一起给**（`SUMMARY_TOOL_ARGS_HEAD_BYTES` = 2KB，原样 JSON）。摘要 prompt 明写要「列出跑过哪些工具及其实参」，而压缩后模型历史里一条工具轮次都没有 —— 不给实参，摘要模型只能编，编出来的那份就是继承者唯一的依据。实参按行边界截断，`{}` / 空实参写成 `args: (无)`。
- **工具结果用 `SystemReminder` 承载，不用 `User`**。三条 wire（chat / messages / responses）都按「请求里第 i 个 `User`」取图片槽，而图片槽按**真实用户消息**排；工具结果若是 `User`，后面每条真实用户消息的图都被顶歪，最后一张还会落到摘要 prompt 那条 `User` 上。摘要本来也不需要图（`compact_session` 传空的图片表），换成 `SystemReminder` 就绕开了整套下标对齐。
- 摘要 prompt 为此补了一段硬约束：明说「继承者拿不到任何 assistant 轮次与工具输出，用户消息也只留首条和最近一条」，要求第 6 段尽量原样记下中间的用户消息（纠正、约束只剩这一处），要求第 8 段列出**已经跑过哪些工具、各自得到什么结论**、声明哪些验证已经做过，免得压缩后重复劳动。
- 那几条提示都还在，只是不再是模型历史的一部分：完整的原对话在滚动区，被摘要掉的原文在 `compaction/segment_NNN.md`。

### 摘要写漏了怎么办：指针，不是整份历史

摘要末尾还挂一段 `<transcript_location>`（抄 Grok `format_transcript_location`），告诉模型**这次压缩的原文在哪**：

```xml
<transcript_location>
The full, unsummarized transcript of this conversation is saved at:
/Users/…/sessions/<cwd-key>/<id>/compaction/segment_000.md

The same conversation as line-delimited JSON (raw tool arguments,
reasoning, timestamps) is at:
/Users/…/sessions/<cwd-key>/<id>/chat_history.jsonl
Read them only when the summary above is missing something you need. Do not
read either file in full: both are large, and reading one would refill the
context this compaction just freed. Target the lookup instead — `grep -n` for
an identifier or an error string, or `tail` / `offset` + `limit` for the most
recent turns.
</transcript_location>
```

**两个路径分工不同，都要给**：

| 文件 | 里面有什么 | 什么时候查 |
|---|---|---|
| `compaction/segment_NNN.md` | 摘要 + 逐轮 verbatim（user / assistant 正文 / 工具结果） | 想知道「早先那轮说了什么、工具返回了什么」 |
| `chat_history.jsonl` | 逐行 JSON 的原始事件，含**工具调用的确切实参**与推理 | 想知道「当时那条 bash / read_file 到底传了什么参数」 |

段文件把实参换成了 `[Called tools: name]` 占位、推理整段丢掉 —— 它是一份可读的整理稿，不是无损副本。无损的只有原始 jsonl。

要点：

- **给的是路径，不是内容**。往上下文里塞回历史等于没压；给一条可寻址的路径，模型缺什么自己去 `read_file` / `grep` —— 和 `tool_output.rs`「截断即降级成可寻址」是同一个思路。
- **指令里明写不要通读**。两份文件都可能上百 KB 到 1 MB（本次实测段文件 767 KB / jsonl 1.3 MB），`sessions/…/chat_history.jsonl` 一条长会话能到好几 MB。整份读进来等于把刚腾出来的上下文一次性填回去，所以指针里点名 `grep -n`、`tail`、`offset` + `limit`。`read_file` 默认一次吐 1000 行，对这两份文件都不该走默认。
- 顺序是**先判定、再落盘、最后拼指针**：收紧检查用不带指针的替换体过（指针约 400 字节，对判定没有意义），压缩真被采纳之后才写 `segment_NNN.md`，然后把真实路径拼进摘要。落盘放在检查之前的话，每次「压不小」都会在 `compaction/` 下留一份没人引用的段文件和一行 INDEX 假记录。
- 落盘失败就**不写段文件路径**（fail-open）：失败只丢一条提示，压缩本身不受影响；模型也不会去找一个不存在的文件。`chat_history.jsonl` 是会话本体，一直存在，不受这次落盘成败影响。
- 因此压缩后的历史仍是三条，指针在摘要那段 `SystemReminder` 里，不额外占一条消息。
- 子代理的日志在父会话目录的 `subagents/<id>/` 下：段文件也落在那里，指针指向那里的 `chat_history.jsonl`（`Sessions::transcript_dir`）。
- 「文件在不在」和「模型知不知道它在」是两件事。以前只写段文件、不告诉模型，等于白写 —— 这是这次补上的部分。

### 占用怎么算

判定、压缩前记忆 flush 门槛、顶栏 / `/context` / 网关快照共用一个数：
`context_usage::context_tokens_used`。做法同 Grok / Codex。

- 上一次采样上游报了用量：以它的 prompt + completion 为锚点。
  - completion 含推理，下一次请求会原样回放。
  - 锚点之后追加的工具结果、提醒按估算补上。
- 没报用量，或刚压缩 / 换会话 / 清空：system + 工具表 + 图片 + 模型历史全靠估算。
  - 推理照算：有 `reasoning_items` 按原件（明文与密文取大），否则按 `reasoning` 文本。
  - 工具调用的参数只算一次（assistant 的 `tool_calls`）。工具结果发给上游的只有 content，
    原先在 `ToolExecute` 上再算一遍，写大文件的会话估算虚高近一半（#176）。
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

### 什么时候干脆不压

摘要生成完、替换之前还有一道收紧检查（抄 DSH `compaction-basic`）：**新历史的估算 token 必须小于被它取代的旧历史**，否则判失败（`压缩失败：摘要不比原文小（A → B token）`），历史一点不动。

压缩会改写历史前缀、作废上游 KV 缓存，下一次采样必然全量重算 —— 换回来一个不更小的历史是纯亏。最常撞上的是「刚压完又压」和「这一轮只跑了一两步」，那时摘要 + 首末用户消息本来就可能比原文长。判失败之后 `maybe_auto` 会抑制自动压缩，等到下一条用户消息。

这条判定**不进重试循环**：同一段历史、同一份摘要重算几次结论一样，`continue` 只会白烧 `max_attempts - 1` 次摘要调用（默认 3 次，每次都是真金白银）。所以它直接 `return Err`，其余「摘要为空 / 过短 / 调了工具 / 上游报错」这类可能偶发的原因才重试。

成功压缩后会在会话目录写入 `compaction/segment_NNN.md` 与 `INDEX.md`（供事后查阅被摘要掉的原文）。
