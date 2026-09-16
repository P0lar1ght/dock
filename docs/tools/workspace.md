# 工作区七颗工具

> 属于 [TOOLS.md](../../TOOLS.md) 的一部分。改这一块只需要读本文件。

插件 `tools`（`workspace_tools`），ctx `"tools"`，工作区内建、不能被 register 盖掉。

`list_dir` `read_file` `grep` `search_replace` `bash`（别名 `run_terminal_cmd`）`glob` `write_file`

## 设计前提

**模型总是绕回 `bash`，很大程度上是因为内置工具真的更弱。** 所以先把工具修到值得用，再谈引导。引导只加厚了工具描述，**没有**往系统提示里加段（`system-prompt/assemble` 仍是 `ORDER_CORDIS/PERSONA/WORKFLOWS/SKILLS` 四个槽）。

七颗的描述都是 Grok 那种 usage notes（不是一行字），且逐条点名对应的 shell 命令——`read_file` 之于 `cat`、`grep` 之于 `grep`/`rg`、`glob` 之于 `find`、`list_dir` 之于 `ls`、`search_replace` 之于 `sed -i`——并说明为什么用工具更划算：不过权限门、计划模式下仍可用、会回报自己截断了多少。`bash` 描述里有同一份反向清单。

### 为什么不学 codex 删掉 `grep`

codex 的模型侧文件面只有 `exec_command` + `apply_patch` + `view_image`，没有内容检索工具。它敢这么做有两个前提，dock 都不具备：

1. 它的 shell 强一档：PTY、session id + `write_stdin` 可续、`workdir`、`yield_time_ms` 10s 让出、按 token 计的 `max_output_tokens`。
2. 它没有 tool-level 的只读预设。

dock 的权限门 / 计划门 / preset allowlist **全是 tool-level 的**：`acp.rs` 的 `gated_builtin` 里有 `bash`、没有 `grep`；`presets/code/agents/explore.yml` 与 `plan.yml` 明确列 `grep`、明确不含 `bash`。搜索一旦归进 `bash`，这两颗只读子代理和整个计划模式就都搜不了了。

## 各颗的行为

### `grep`

**进程内 ripgrep**（`src/grep.rs`）：`ignore` 走目录、`grep-regex` 编译、`grep-searcher` 扫文件，**不 spawn `rg`**。旧的 `Command::new("rg")` + 手搓 `grep_walk` 兜底已删——那条兜底不读 `.gitignore`、regex 方言也不同，等于「装没装 rg 结果不一样」。

- 参数面：`glob` `type` `-i` `-A` `-B` `-C` `output_mode`（`content`/`files_with_matches`/`count`）`head_limit` `multiline`
- `type` 用 `ignore` 自带的 ripgrep 默认类型表（219 条，含 `rust`/`py`/`ts`/`go`…）；未知类型名降级成不过滤并在结果里说明
- NUL 字节即判二进制并停搜；单行超 1000 字符按**字符**边界截断
- 整趟 20s 墙钟预算，到点带回已扫到的部分并说明
- 空结果回报搜索范围与已施加的过滤——模型才分得清「真没有」和「搜错地方」

### `glob`

换 `globset`（ripgrep 自己的 glob），修掉两个**反向**的错：旧的手搓 matcher 把 `*` 翻成 `.*`，导致 `**/*.rs` 漏掉根目录下所有文件（正则强制要有一个 `/`），而 `src/*.rs` 又会跨过 `/` 匹配进 `src/deep/b.rs`。

现在 `literal_separator(true)`：`*` 不跨 `/`、`**` 才跨，无 `/` 的模式匹配任意深度 basename。另外：**files-only**（旧实现在判 `is_dir` 前就把条目推进结果，目录会混进来）、按 mtime 新→旧排序、走 `ignore` 读 `.gitignore`、上限 200 条且截断出声。

### `list_dir`

走 `ignore::WalkBuilder`（`max_depth(1)`）读 `.gitignore`，`target/` / `node_modules/` 不再刷屏。超过 100 条转摘要：子目录仍逐条列，文件收成计数 + 扩展名分布 + 前 100 个文件名 + 「另有 N 个未列出」。

### `search_replace`

`replace_all: false` 且多处命中时**报错并回报命中数，文件一个字节不写**。旧实现 `replacen(..., 1)` 静默改第一处——模型以为改的是自己瞄准的那处，实际可能是另一处同名代码。对齐 Grok / DSH。

### `bash`

新增三个参数：

- `timeout_ms`——**只能收紧不能放宽**，上限仍是 `DOCK_BASH_FOREGROUND_MS` 的 5 分钟
- `workdir`——不存在直接报错，不让命令在意外目录里跑掉。注意命令内的相对路径此后相对 `workdir` 解析
- `description`——5–10 词，进 `JobSnapshot`

前台预算与「到点 / 取消都带回已产出输出」不变。

### `read_file` / `write_file`

行为未变，只加厚描述。

## 输出预算

统一走 `src/tool_output.rs`（从 `mcp/discover.rs` 提出来的共享层，`use_tool` 改用它、**文案与行为逐字不变**）。三层，缺一层就会丢信息：

1. **语义分页**——grep 数匹配行、`list_dir` 数条目，不按字节切
2. **回报总数**——`showing N of M`；额度填满就停时说 `at least`，**绝不谎报精确总数**
3. **溢出落盘** `$DOCK_HOME/tool-output/<call_id>.txt` 并在提示里回路径

有第三层，「截断」就不是丢信息，而是把信息从上下文降级成**可寻址**：模型能用 `read_file` / `grep` 把剩下的捞回来。

齐质列表**保头丢尾**——不做头尾各留一半的挖洞式截断，中间挖掉模型无从知道挖了什么。`bash` 那种「结论在尾巴上」的输出仍由 `jobs` 自己保头 4KB + 尾 16KB。

## 落盘文件的命名与清理

文件名是 `<进程前缀>-<清洗过的 call_id/job_id>.txt`。

**进程前缀不是装饰。** `Jobs::seq` 每进程从 1 重新计数，没有前缀时 `job-3.txt` 会跨重启复用，而旧会话历史里还写着那条路径——`/resume` 之后模型按路径读回来的会是**另一条命令的输出**。缺文件只是读不到，读到错的东西更糟。`call_id` 通常唯一，但 `stream_acc` 在 id 缺失时兜底成 `call-0`，同样会撞，所以前缀对所有落盘一视同仁。

清理由 `gc_spill_dir()` 在 `install_app` 开头跑一次：先删超过 **7 天**的，仍超 **512MB** 总量就从最旧的接着删。**只在启动时跑**——那一刻本进程还一个文件都没写，删不到正在用的；会话中途跑才有那个风险，所以不提供定时清理。全程 best-effort，读不动目录或删不掉文件都直接放过，绝不影响启动。

`tool-images/` 与 `screenshots/` **还没有清理**，同样无限增长。

## 依赖

`ignore` / `globset` 原本就随 `vendor/xai/fuzzy-file-search` 进了二进制，这里提成 `cordis-spine` 的直接依赖（无新包）；`grep-searcher` / `grep-regex` / `grep-matcher` / `encoding_rs_io` 是 4 个新包，全是 ripgrep 家族。
