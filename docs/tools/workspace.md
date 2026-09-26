# 工作区七颗工具

> 属于 [TOOLS.md](../../TOOLS.md) 的一部分。改这一块只需要读本文件。

插件 `tools`（`workspace_tools`），ctx `"tools"`，工作区内建、不能被 register 盖掉。

`list_dir` `read_file` `grep` `search_replace` `bash`（别名 `run_terminal_cmd`）`glob` `write_file`

## 源码布局

七颗各自一目录，挂在 `cordis-spine/src/tools/` 顶层（与 `ask_user/`、`browser/` 同级），**没有** `tools/workspace/` 伞目录：

```
tools/read_file/   tools/bash/   tools/list_dir/   tools/glob/
tools/write_file/  tools/search_replace/   tools/grep/   # grep 薄包装，引擎在 cordis-base
tools/fs_common.rs          # resolve / parse_args / 字段解析
tools/workspace.rs          # 薄套件：specs / handles / execute_with（注册入口不变）
```

`workspace_tools` / `handles` / `specs` / `execute_with` 仍从 `tools::workspace` 导出，权限门 / 计划门 / preset 不受影响。


## 设计前提

**模型总是绕回 `bash`，很大程度上是因为内置工具真的更弱。** 所以先把工具修到值得用，再谈引导。引导只加厚了工具描述，**没有**往系统提示里加段（`system-prompt/assemble` 仍是 `ORDER_CORDIS/PERSONA/WORKFLOWS/SKILLS` 四个槽）。

七颗的描述都是 Grok 那种 usage notes（不是一行字），且逐条点名对应的 shell 命令——`read_file` 之于 `cat`、`grep` 之于 `grep`/`rg`、`glob` 之于 `find`、`list_dir` 之于 `ls`、`search_replace` 之于 `sed -i`——并说明为什么用工具更划算：不过权限门、计划模式下仍可用、会回报自己截断了多少。`bash` 描述里同一份清单写在**第一条**，用的是否定式（「文件检索别走 bash」），并点明 shell 的 `grep` 不读 `.gitignore`、会连 `target/` 一起读（同一趟仓库检索：工具几十 ms，bash 分钟级）。

### 为什么不学 codex 删掉 `grep`

codex 的模型侧文件面只有 `exec_command` + `apply_patch` + `view_image`，没有内容检索工具。它敢这么做有两个前提，dock 都不具备：

1. 它的 shell 强一档：PTY、session id + `write_stdin` 可续、`workdir`、`yield_time_ms` 10s 让出、按 token 计的 `max_output_tokens`。
2. 它没有 tool-level 的只读预设。

dock 的权限门 / 计划门 / preset allowlist **全是 tool-level 的**：`acp.rs` 的 `gated_builtin` 里有 `bash`、没有 `grep`；`presets/code/agents/explore.yml` 与 `plan.yml` 明确列 `grep`。搜索一旦归进 `bash`，只读场景里每搜一次都得先过只读命令判定（见下节），工具还是更顺。

**只读场景里的 bash。** 计划模式开着、子会话能力档位是 `read-only`、或当前角色预设标了 `read_only: true`（内置 `explore` / `plan`、`/btw` 旁问页）——这些都给 `bash`，但不走普通的计划门 / 权限门，走 `tools::read_only`：`cordis_base::read_only_shell::is_read_only_command` 判为只读的（白名单程序、无写盘重定向、无命令替换 / 后台、无 `find -delete` `git commit` `sed -i` 这类改动参数）直接跑、不问；其余一律 `Permissions::request_strict` 问用户——**自动批准和「以后都允许」都不算数**，「以后都拒绝」照旧生效。用户拒了，模型收到一句说明哪些能直接跑。判定宁小勿大：误判成要问只是多弹一次框。

## 各颗的行为

### `grep`

**进程内 ripgrep**（`src/grep.rs`）：`ignore` 走目录、`grep-regex` 编译、`grep-searcher` 扫文件，**不 spawn `rg`**。旧的 `Command::new("rg")` + 手搓 `grep_walk` 兜底已删——那条兜底不读 `.gitignore`、regex 方言也不同，等于「装没装 rg 结果不一样」。

- 参数面：`glob` `type` `-i` `-A` `-B` `-C` `output_mode`（`content`/`files_with_matches`/`count`）`head_limit` `multiline`
- `type` 用 `ignore` 自带的 ripgrep 默认类型表（219 条，含 `rust`/`py`/`ts`/`go`…）；未知类型名降级成不过滤并在结果里说明
- NUL 字节即判二进制并停搜；单行超 1000 字符按**字符**边界截断
- 整趟 20s 墙钟预算，到点带回已扫到的部分并说明
- 空结果回报搜索范围与已施加的过滤——模型才分得清「真没有」和「搜错地方」
- **超过 5MB 的文件整颗跳过并报数**（对齐 Grok `grep` 的 `--max-filesize 5M`）：一颗几百 MB 的日志能把 20s 墙钟吃光，结果却看起来像「搜完了」。`path` 显式点名一颗文件时不设这道闸——那时跳过等于答非所问

**为什么是「按批并发 + 把每颗文件的准备工作挪到命中路径上」**，而不是直接并行扫：

| 版本 | dock 仓库 | grok-build/crates | CPU 重正则 |
|---|---|---|---|
| 纯顺序（旧） | 31.1 ms | 98.6 ms | 124.2 ms |
| 只并行扫描 | 28.4 ms | 95.6 ms | 103.4 ms |
| 并行扫描 + 惰性显示名 + 并发 `stat` | **17.1 ms** | **58.1 ms** | **69.2 ms** |

（release，best of 5，同一台机器，±10% 抖动。同树 `rg` 是 73 ms、`rg -j1` 121 ms；搜一个几十颗文件的小目录是 0.9 ms，走顺序路径。）

只把扫描并行掉只有 1.03–1.20x——说明瓶颈不在正则，而在**每颗文件都要付的串行开销**：`getcwd`（算相对显示名）、一次 String 分配、一次 `stat`。这三样现在都只在**命中**时才做，或者跟着扫描一起摊到多核上。遍历本身仍是顺序 + 路径排序（结果要可复现，`head_limit` 的「前 N 条」也只有顺序固定才有意义），并发只在「扫哪些文件」这一层，合并按原顺序进行，所以顺序路径与并发路径逐字同结果（`parallel_scan_matches_sequential_scan`）。

**批大小必须自适应**，否则上面那张表只对「要走完整棵树」的查询成立。一批是**扫完才合并**的，而 content 模式下额度提前填满是常态（默认 200 行，一颗热门文件就够）——批要是一上来就开到 `FILES_PER_WORKER × 核数`（14 核上是 448 颗），`grep "fn "` 得先白扫一整批才发现第 2 颗就够了。所以批从 16 颗起步，每批没填满额度就 ×4，直到满额。同一台机器、同一棵树（旧 = 纯顺序）：

| 查询（content，默认 head_limit） | 旧 | 固定 448 批 | 自适应批 |
|---|---|---|---|
| 稀有字面量（走完整棵树） | 29.7 ms | 17.7 ms | **16.4 ms** |
| `pub fn`（额度早满） | 2.2 ms | 7.8 ms | **1.8 ms** |
| `fn `（额度早满） | 1.6 ms | 7.7 ms | **1.9 ms** |
| `.`（第一颗文件就满） | 0.8 ms | 11.3 ms | **1.2 ms** |
| CPU 重正则（扫几百颗才满） | 16.6 ms | 8.6 ms | **13.1 ms** |

代价写明白：最后一行是自适应的取舍——放大系数调到 ×8 能把它压到 10.4 ms，但 `pub fn` / `fn ` 会涨到 2.8 ms（比旧实现还慢）。选 ×4 是因为它让**每一类查询都不慢于旧实现**，而不是拿常见查询去换少见查询。

`head_limit` 的额度由 `SearchAcc::absorb` 在**按序合并时**夹住，不能只靠传给单文件 `Collector` 的上限：那个上限是批开头取的，批内每颗命中文件都按它收，叠起来会冲破额度（head_limit=200 实收 300 行，页脚还照说 300）。回归用例是 `head_limit_holds_across_multiple_files`——单文件的用例盖不到，那时 `Collector` 自己的计数还管用。

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

**前台预算到点 = 转后台，不是 kill。** `Jobs::detach` 把 `foreground` 翻成 `false`，进程一点不动，返回已产出的输出 + `job_id`，模型用 `job` 工具接着收。

旧行为是 kill + 「需要跑完就用 `is_background: true` 重跑」，那是双重浪费：已经跑掉的几分钟扔了，重跑还要再花同样的时间，而且大概率**再超时一次**——`cargo build` 不会因为重跑就变快。命令已经过了权限门、进程还活着，留着它严格更优。

三处刻意的边界：

- 返回文案**不以 `Error:` 开头**。模型看到 `Error:` 的第一反应是重试或换路子，而这次它什么都没做错，只是命令比预算长。
- **取消（用户按 Esc）仍然是 kill**，不是转后台——那是明确要它停（`bash_cancel_still_kills`）。
- **没挂 `"jobs"` 服务时（单测、精简装配）退回 kill + 报错**：那时的任务表是本次调用临时起的本地表，随调用一起析构，发出去的 `job_id` 没人查得到。宁可诚实地失败，也不发空头支票（`bash_timeout_without_a_jobs_service_still_kills`）。

已知代价：命令不再被超时兜住，一条跑飞的命令会一直跑到会话结束（和 `is_background: true` 的任务同一处境，目前都没有后台兜底上限）。`kill_task` 是唯一的收口。

「取消带回已产出输出」不变。

### `read_file` / `write_file`

`read_file` 按 Grok 顺序分流：字节读入 → 图片（magic / 扩展名，压缩进 `ToolResult.images`）→ PDF（`pdf_oxide` + `rendering` 在 spine；**默认 `format=image`** 按页渲 JPEG→`UserImage`，与 Grok 一致；`format=text` 抽文本；`pages` 超 10 页必填，每呼最多 20 页，对齐 `MAX_TOOL_IMAGES=20`，50MB / 60s，DPI 150 / JPEG q85）→ PPTX（DrawingML，`--- Slide N ---` + notes）→ binary gate（对齐 Grok `BINARY_EXTENSIONS`，docx 等拒绝；pdf/pptx/已识别图片豁免）→ 文本分页。技能 markdown（`**/SKILL.md` 与 `skills/` 路径下的 Markdown）以及工程指令文件（`AGENTS.md` / `CLAUDE.md` 等）在 **token 上限**（25k，非字节）内整读，显式 `offset`/`limit` 仍走窗口。

`write_file` 行为未变。

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

`ignore` / `globset` 原本就随 `cordis-tui/fuzzy-file-search`（crate `xai-fuzzy-file-search`）进了二进制，这里提成 `cordis-spine` 的直接依赖（无新包）；`grep-searcher` / `grep-regex` / `grep-matcher` / `encoding_rs_io` 是 4 个新包，全是 ripgrep 家族。
