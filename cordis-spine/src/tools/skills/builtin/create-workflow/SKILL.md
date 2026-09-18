---
name: create-workflow
description: 创作一条 dock 工作流：写 Rhai 编排脚本（子代理、阶段、有界并行扇出、验证面板），用 workflow 工具冒烟一条路径，存成命名工作流，再问用户要不要真跑一次。本文件同时是工作流脚本的完整 Rhai 参考——脚本形状、host API、方言规则、dock 与 grok 的差异。用户说「写一条工作流 / 把这套流程自动化 / 编排多个子代理」或敲 `/create-workflow` 时用它。
metadata:
  short-description: "创作一条多代理工作流"
---

# 创作工作流

工作流是确定性的 Rhai 脚本，用 `agent()`、`parallel()`、`phase()`、`complete()` 编排子代理，由 `workflow` 工具跑成一次后台运行。本文件既是创作流程，也是完整语言参考——`workflow` 工具的描述指的就是这里，参考部分对任何脚本都适用，不管它是不是按这个流程写出来的。

跟用户说这些 `.rhai` 文件时叫它「工作流」，别叫「Rhai」。

## 流程

1. **问清意图**：要做什么、哪一步并行扇出、什么需要验证、最终产物是什么（一份报告？一个结构化结果？）、一次运行大概能接受起多少个子代理。
2. **定名字和落点**（和技能同一套约定）：
   - 项目级：`<仓库根>/.dock/workflows/<name>.rhai` —— 跟着这个仓库走，可以和同事共享（在 git 仓库里默认选它）。
   - 用户级：`~/.dock/workflows/<name>.rhai` —— 所有项目都能用。
   - 名字用小写字母、数字、连字符（如 `review-changes`）。不能盖掉内置的 `deep-research`。
3. **写脚本**。从下面的例子起手，照着后面的参考章节写。形状是：`let meta` 头（纯字面量）→ schema 常量 → 一个阶段一段。子代理 prompt 要写成祈使句且自足（见「踩过的坑」）。
4. **冒烟一条路径**。用 `workflow` 工具发 `{ script: "<rhai>", validate_only: true }` 加有代表性的 `args`，改到元数据、编译、那一条 canned-host 路径都过。**这不等于每个分支和真实工具都验过**——见「迭代」。
5. **存盘**到选好的路径，目录不存在就建。存完它就能用 `/<name>` 或 `/workflow <name> ...` 启动。注意 `/workflow` 是**运行看板**，不是已存脚本的目录；名字要等第一次运行起来才会出现在那儿。
6. **问用户要不要真跑一次**，带有代表性的参数。它在后台跑，用户在 `/workflow` 里看进度。用户不跑就停在这儿，并说清楚只做了路径冒烟。
7. **汇报**：文件路径、冒烟输出和它的局限、怎么运行、一次运行最多起多少子代理。

## 例子：扇出 → 对抗验证 → 结构化结果

```rhai
let meta = #{
    name: "review-changes",
    description: "分维度评审一份改动，再逐条对抗验证",
    phases: [
        #{ title: "Review", detail: "一个维度一个评审员" },
        #{ title: "Verify", detail: "一条发现一个质疑者" },
    ],
};

let findings_schema = #{
    "type": "object", "required": ["findings"],
    "properties": #{
        "findings": #{ "type": "array", "maxItems": 8, "items": #{
            "type": "object", "required": ["file", "issue"],
            "properties": #{
                "file": #{ "type": "string" },
                "issue": #{ "type": "string" },
            },
        }},
    },
};
let verdict_schema = #{
    "type": "object", "required": ["real", "reason", "evidence"],
    "properties": #{
        "real": #{ "type": "boolean" },
        "reason": #{ "type": "string" },
        "evidence": #{ "type": "string" },
    },
};

// 先护住 `args` 本身：没传参数时 `args.target` 是在 unit 上取字段，直接抛错，
// 那样下面这句 pause 永远走不到。
let target = if args == () { () } else { args.target };
if target == () { pause("verification", "请传 args.target：要评审的 diff、分支或路径。"); }

phase("Review");
let dimensions = ["正确性缺陷", "错误处理缺口", "性能问题"];
let jobs = [];
for d in dimensions {
    jobs.push(#{
        prompt: "评审 " + target + " 的" + d + "。用只读工具（read_file、grep、git diff）"
            + "看真实代码，不要凭记忆回答。最多报 8 条具体发现，格式 {file, issue}；"
            + "只有在你确实读过代码之后，空列表才是有效答案。",
        label: "review:" + d,
        capability_mode: "read-only",
        output_schema: findings_schema,
    });
}
let results = parallel(jobs);

let findings = [];
for r in results {
    if r != () && r.success && r.output.findings != () {
        for f in r.output.findings { findings.push(f); }
    }
}
if findings.len() == 0 { complete(#{ summary: "没有发现。", confirmed: [] }); }

phase("Verify");
let vjobs = [];
for f in findings {
    vjobs.push(#{
        prompt: "读已落地的代码，对抗性地验证这条评审发现：\"" + f.issue + "\"，位于 "
            + f.file + "。只有拿到你自己查证的具体证据才把 real 设成 true，否则默认 false。",
        label: "verify:" + f.file,
        capability_mode: "read-only",
        output_schema: verdict_schema,
    });
}
let verdicts = parallel(vjobs);

let confirmed = [];
let i = 0;
for v in verdicts {
    if v != () && v.success && v.output.real == true
        && v.output.evidence != () && v.output.evidence != "" {
        confirmed.push(findings[i]);
    }
    i += 1;
}
log(confirmed.len().to_string() + "/" + findings.len().to_string() + " 条发现通过了验证");
complete(#{ summary: confirmed.len().to_string() + " 条确认发现", confirmed: confirmed });
```

## 脚本形状

第一条语句必须是纯字面量的 meta map——不能有变量、函数调用、任何要算的东西：

```rhai
let meta = #{
    name: "find-flaky-tests",
    description: "找出不稳定的测试并给出修法",
    phases: [ #{ title: "Scan", detail: "翻 CI 日志" }, #{ title: "Fix" } ],
};
phase("Scan");
let r = agent("在 CI 日志里找重试标记，列出不稳定的测试。",
    #{ label: "scanner", capability_mode: "read-only" });
if r.success { complete(r.output); }
```

`meta.name` 用小写字母、数字、连字符。`meta.phases` 可选，但里面的 title 应当和正文的 `phase()` 调用对得上，`/workflow` 详情页左栏才不会错位（没有任何东西强制两者一致，打错字就只是把那一栏晾在那儿）。`when_to_use` 是可选字符串，列目录时显示。

## 方言

- Map 是 `#{ ... }`。unit `()` 是空值，`x != ()` 是「存在吗」的判断。
- map 里的 JSON-Schema 键要加引号，因为 `type` 是 Rhai 关键字：`#{ "type": "object", "required": [...], "properties": #{ ... } }`。
- 保留但没用上的标识符会以 `'X' is a reserved keyword` 失败：`shared`、`sync`、`async`、`await`、`spawn`、`go`、`thread`、`new`、`match`、`case`、`default`、`void`、`null`、`nil`、`exit`、`static`、`var`。改名（`shared` → `has_shared`）。
- 每个调用都阻塞。`parallel()` 是唯一的并发手段：它收一个 option map 数组（不收闭包），并且是一道屏障——最慢的那个不结束，后面一行都不跑。
- 长字符串（prompt）用 `+=` 一句句拼。单个 `+` 链表达式够长之后会触发 `Expression exceeds maximum complexity`（几百个 `+` 项就到了），拆开写。数字拼接要 `.to_string()`。
- `s[i]` 得到的是 `char`，在 `char` 上取字段会报 `getter is not registered for type 'char'`——通常说明你把字符串当成解析好的 JSON 在用。用 `type_of(x)` 确认，切片用 `s.sub_string(start, len)`。
- `s.trim()` 这类字符串方法是**原地改** `s` 并返回 `()`，不是返回新串。所以 `x.trim() != ""` 恒真，`"p" + x.trim()` 会把文本吃掉。单独一行 trim 完再用 `x`，或者写个helper：`fn trimmed(s) { if type_of(s) == "string" { s.trim(); s } else { "" } }`。
- 没有正则——需要的那几个字符串操作自己手写，或者干脆简化（比如改用下标标签）。
- 函数按值传参，所以要返回值而不是跨调用改对象。`const X = 5;` 可用；map 和数组仍可变。用普通 `for` 循环，别用 `.map` / `.filter`。

## Host API

- `agent(prompt)` / `agent(prompt, opts)` → `#{ agent_id, success, output, cancelled, tokens_used, duration_ms }`。`output` 是子代理的最终文本；设了 `output_schema`（一个 JSON Schema map）就是校验过的对象。
  - **生效的 opts**：`label`（详情页那一行的名字）、`phase`（归到哪个阶段；不写就继承脚本当前的 `phase()`）、`capability_mode`（`"read-only"` / `"read-write"` / `"execute"` / `"all"`，读写与执行互不包含）、`output_schema`、`agent_type`（角色名，见下）、`model`、`effort`、`max_output_tokens`。
  - `model` 必须是**本机 `config.toml` 模型目录里真有的 id**；认不出来的会退回父会话的模型并记一条 warn（不会让 run 失败）。不写就跟父会话走。
  - `agent_type` 取当前预设的角色名：`code` / `cordis` 预设是 `general-purpose`、`explore`、`plan`。不写就是 `general-purpose`。角色名写错会让这次 `agent()` 失败（`success: false`），详情页那一行显示原因。
  - **被忽略的 opts**：`isolation_worktree`、`fork_context`、`resume_from`。dock 的子代理没有独立 worktree，也不继承父会话上下文——prompt 必须自足。
  - 子代理级别的失败是数据（`success: false`）；基础设施失败才抛异常。验证类面板要**失败即不通过**，可选的建议类面板才可以失败即放过。
- `parallel([#{ prompt: "...", label: "..." }, ...])` → 按输入顺序返回结果，失败的槽位是 `()`，用之前先过滤。整个面板对 `agent_budget` 是**一次性**记账：会超预算就在任何一个孩子起跑前整批失败。
- `phase(title)` 把后面的子代理归到一组（详情页左栏）。标题超过 256 字节会被截断。
- `log(message)` 给用户发一行进度。快照只留最近 50 条、单条 4KB；任务条和 `/workflow` 显示最新一条。**不进模型上下文**。
- `complete(value)` 以成功收尾，`value` 成为运行结果（如 `complete(#{ path: p, report: text })`）。结果会摊成可读文本再交给用户和主线程，不用自己 JSON 编码。
- `budget()` → `#{ total, spent, reserved, remaining }`。`total` 是逻辑子代理调用的绝对上限（默认 128，可显式设 1–1024）。每个 `agent()` 和 `parallel()` 的每一项在起跑前都会 `spent += 1`；schema 纠错重试不算。`reserved` 恒为 0。
- `write_scratch_file(name, content)` → 返回**绝对路径**；`read_scratch_file(name)` 按同一个名字读回来。`name` 必须是单个相对路径组件（不能有 `/`、`.`、`..`），拒符号链接，配额是 64 个文件 / 单文件 10MB / 总量 64MB。报告写这儿，然后 `complete(#{ path: p, report: text })`，用户就能拿到文件路径。
- `fingerprint(text)` → 稳定哈希（做停滞检测用）。`json_encode(value)` → 确定性 JSON 文本，用来引用不可信的 prompt 数据。
- `args` 就是工具的 `args` 值，原样（没传就是 `()`）。优先用对象字段（`args.query`）。
- **工作流不能再起工作流**：子代理拿不到 `workflow` 工具（`depth > 0` 直接拒），不然每一层都会拿一份全新预算。把子流程的逻辑内联进来，或者拆成两条工作流。

### dock 里不可用的

调用了就抛异常，把整条 run 打成 `failed`：

- `timestamp()` / `sleep()` / `exit()` —— 时间和随机性不可用（确定性要求）。要时间戳就从 `args` 传进来。
- `git_diff_since(commit)` / `render_template(name, map)` —— dock 不提供模板表；仓库读写让子代理走 bash 权限门。

`telemetry_event(...)` 不抛异常，但被直接丢掉（dock 没有埋点通道），只留一条 debug 日志。

### pause / await_user 在 dock 是「带话收尾」

`pause(kind, message)` 和 `await_user(kind, message)` 都会让 run 以 `<kind>_paused` 状态**结束**，`message` 显示给用户。**dock 没有 resume**——引擎的 journal 不落盘，`workflow` 工具的 `resume` source 一律拒绝。所以：

- 把它们当成「缺前提，说清楚为什么停」的收尾，比如上面例子里缺 `args.target`。
- 不要指望用户「继续」下去，也别写「等用户回答再往下」的门——写了就是把 run 停在那儿再也回不来。需要用户参与就让 run 收尾，把问题写进 `complete()` 或 pause message，让用户带着新参数重跑。
- kind 取 `user`、`back_off`、`no_progress`、`verification`、`infra`（`verification` 也接受 `blocked`，`back_off` 接受 `backoff`）。
- `complete` / `await_user` / `pause` 都不能被 try/catch 捕获。

## 确定性

控制流只能来自 `args` 和 host 返回值。并行 prompt 要按下标区分，不能靠随机。

一次运行同时在跑的子代理还有一道**并发上限**（默认 32，并按机器并行度收窄），比它宽的 `parallel()` 面板会排队——仍然是屏障，只是分批跑完。

## 迭代

改脚本就直接改磁盘上那份 `.rhai`，用 `validate_only: true` 冒烟，再按 `script_path` 或 `/<name>` 起一次新 run。**没有 resume**，所以每次都是完整跑一遍——把有副作用的步骤写成幂等的，或者先查状态再做。

`validate_only` 会校验元数据、编译整个脚本、并执行由你的 args 和 canned host 结果（`success: true` 加一个固定的小 output 对象）选中的**那一条**路径。它能抓到这条路径上的错误，但**不会**枚举分支、不碰真实工具、不证明每个子代理产出都合 schema、也不验证任何外部副作用。所以要用有代表性的 args、护住每一个可选的 agent 结果，冒烟过了之后再问用户要不要真跑。

## 好用的模式

- 扇出的工作清单用**最简单且恰好正确**的确定性方式产生——遍历文件、`args` 里给死列表——把子代理花在判断上（扫描、验证），不要花在「决定范围」上。如果清单只能由子代理发现，就把它的输出当成不可信数据，用纯 Rhai 对着不变式再过一遍（比如只保留以 `args.root` 开头的路径）再分片。
- 规划 → 并行扇出 → 汇总。
- 对抗验证：独立的质疑者，prompt 要求它们去证伪。缺失、失败、不可用的验证**不算赞成票**——接受一个结论前必须要到具体证据（内置 `deep-research` 的 verifier 就是这么写的）。
- 跑到干为止：不断起 finder，直到连续两轮没有新东西；每轮用 `fingerprint` 检测停滞。
- 投票面板：每项 N 个质疑者塞进一个扁平的 `parallel()`（项数 × 票数），靠下标算术（`i / VOTES`、`i % VOTES`）再分组。
- 失败策略按用途定。可选的建议可以失败即放过；作为证据闸门的面板必须失败即不通过——没有可用证据就是「未验证」。

## 踩过的坑（每条都真的发生过）

- **prompt 太短的子代理只会返回垃圾。** 一个冷启动的子代理被告知「数一下 TODO 注释」，可能一个工具都不调就回 `{"findings": []}`。prompt 必须命令它用工具（「用 grep / read_file」「回答前先读代码」），并写明什么样的空答案才算有效。
- **护住每一个 agent 产出**：`r != () && r.success && r.output.x != ()`。`parallel()` 里失败的槽位是 `()`；作为证据闸门时，要把它们算作「未验证」，而不是悄悄从分母里去掉。
- **meta 必须是纯字面量**——`let meta = #{...}` 里不能有变量或函数调用。`meta.phases` 的 title 和 `phase()` 调用保持一致，详情页左栏才对得上。
- **别把 `pause` 当成可恢复的门**（dock 没有 resume，见上）。
- **静默截断读起来像全覆盖。** 自己加上限（`MAX_*`）的时候，把丢掉了什么 `log()` 出来。
- **子代理不会替你守不变式，脚本才会。** 一个被告知「只看 crates/codegen 下面」的扫描员，仍然会把它发现步骤喂给它的任何东西报上来。运行依赖的每一条范围规则，都要写成对 agent 输出的纯 Rhai 检查（一个 filter，或者一个 assert 加 `log()` 记丢弃），不能只写在 prompt 里。

## 用户怎么管这条 run

- `/workflow`（或 `/workflow runs`）打开运行看板；Enter 进详情页——左栏是 `meta.phases` 的阶段，右栏是该阶段的子代理行加各自最新一条 `report`。
- `/workflow stop <name|run_id>`，或者在看板 / `/tasks` 里选中行按 `x`，取消一次在跑的 run 并收掉它的子代理。
- **没有 pause / resume**，底栏也不列这两个键。
- 用户认的是**会话内唯一的显示名**（`review-changes`、`review-changes-2`），run id 留在内部。

子代理在跑的过程中调 `report`，会立刻在滚动区出一张卡给用户看，但**不会**中途叫醒主模型；整条 run 的过程上报要等收尾时和结果一起交付。所以别为了「让主线程知道」而加额外的 agent。
