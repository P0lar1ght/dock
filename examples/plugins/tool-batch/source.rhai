// tool-batch：注册模型工具 `tool_batch`，一次调用跑一串工具（内置 / MCP / 其它插件），可以把互不
// 依赖的几颗放进 `parallel` 分组同时跑。中间结果存在插件里的变量表，不进模型上下文；最后只回
// `return` 点名的几项（截断到 max_chars）。
//
// 用到的宿主能力：host.call_tool_ex / call_tools_parallel（结构化结果、调用方身份、时限、停止）、
// host.tool_schema（兜底校验工具名和必填参数；schema 与 search_tool 同源）、host.eval_expr（when / transform，plugin.toml
// 里 allow_eval = true 才有）、host.cancelled。不联网、不读写文件。设计依据见 DESIGN.md。
//
// 注意：Rhai 的 `fn` 看不见外层变量和顶层常量，所以上限写成函数、host 当参数传进去。

fn tb_max_calls() { 50 }
fn tb_max_parallel() { 16 }
fn tb_budget_secs() { 120.0 }
fn tb_default_max_chars() { 4000 }
fn tb_max_timeout_ms() { 600000 }

// 工具结果文本 → 值：像 JSON 对象 / 数组就解析，否则原样当字符串。
fn tb_parse(text) {
    let t = text;
    t.trim();
    if t.starts_with("{") || t.starts_with("[") {
        try { return parse_json(t); } catch (e) { }
    }
    text
}

fn tb_first_line(text, max) {
    let t = text;
    t.trim();
    let i = t.index_of("\n");
    let line = if i >= 0 { t.sub_string(0, i) } else { t };
    if line.len() > max { line.sub_string(0, max) + "…" } else { line }
}

// `name.a.b` / `name.items.0` → #{ ok, value } 或 #{ ok: false, err }。数组下标可以是负数（-1 = 最后一个）。
fn tb_resolve(path, vars) {
    let parts = path.split(".");
    let name = parts[0];
    name.trim();
    if name == "" { return #{ ok: false, err: "empty reference ${" + path + "}" }; }
    if !(name in vars) {
        return #{ ok: false, err: "unknown variable \"" + name + "\" in ${" + path + "} (not set by an earlier step)" };
    }
    let cur = vars[name];
    for i in 1..parts.len() {
        let k = parts[i];
        k.trim();
        let ty = type_of(cur);
        if ty == "map" {
            if !(k in cur) {
                return #{ ok: false, err: "${" + path + "}: field \"" + k + "\" not found" };
            }
            cur = cur[k];
        } else if ty == "array" {
            let idx = 0;
            try { idx = parse_int(k); } catch (e) {
                return #{ ok: false, err: "${" + path + "}: \"" + k + "\" is not an array index" };
            }
            if idx < 0 { idx += cur.len(); }
            if idx < 0 || idx >= cur.len() {
                return #{ ok: false, err: "${" + path + "}: index " + k + " out of range (len " + cur.len() + ")" };
            }
            cur = cur[idx];
        } else {
            return #{ ok: false, err: "${" + path + "}: cannot read \"" + k + "\" of a " + ty };
        }
    }
    #{ ok: true, value: cur }
}

// 字符串里的 ${...}：整串恰好是一个引用时换成原值（map / 数组 / 数字原样），否则按文本拼接
// （非字符串值拼成 JSON）。没有转义写法：字面量 "${" 没法表达。
fn tb_subst_str(s, vars) {
    if s.starts_with("${") && s.index_of("}") == s.len() - 1 && s.index_of("${", 2) < 0 {
        return tb_resolve(s.sub_string(2, s.len() - 3), vars);
    }
    let out = "";
    let rest = s;
    loop {
        let i = rest.index_of("${");
        if i < 0 { out += rest; break; }
        let j = rest.index_of("}", i);
        if j < 0 { out += rest; break; }
        out += rest.sub_string(0, i);
        let r = tb_resolve(rest.sub_string(i + 2, j - i - 2), vars);
        if !r.ok { return r; }
        out += if type_of(r.value) == "string" { r.value } else { to_json(r.value) };
        rest = rest.sub_string(j + 1);
    }
    #{ ok: true, value: out }
}

fn tb_subst(v, vars) {
    let ty = type_of(v);
    if ty == "string" { return tb_subst_str(v, vars); }
    if ty == "map" {
        let out = #{};
        for k in v.keys() {
            let r = tb_subst(v[k], vars);
            if !r.ok { return r; }
            out[k] = r.value;
        }
        return #{ ok: true, value: out };
    }
    if ty == "array" {
        let out = [];
        for x in v {
            let r = tb_subst(x, vars);
            if !r.ok { return r; }
            out.push(r.value);
        }
        return #{ ok: true, value: out };
    }
    #{ ok: true, value: v }
}

// 短的原样（map 仍是 map），超长的转成文本截断并注明砍掉多少。
fn tb_trunc(v, max) {
    let s = if type_of(v) == "string" { v } else { to_json(v) };
    if s.len() > max {
        s.sub_string(0, max) + "…[truncated " + (s.len() - max) + " chars]"
    } else {
        v
    }
}

fn tb_bad(out, msg) {
    out.error = msg;
    out
}

fn tb_has(m, k) { k in m && m[k] != () }

// 一个调用步骤（顶层或 parallel 分组里的一颗）的形状 + 工具名 + 必填参数。
fn tb_check_call(host, st, label, in_group) {
    if type_of(st) != "map" { return label + ": must be an object"; }
    if tb_has(st, "parallel") {
        return label + ": " + (if in_group { "parallel groups cannot nest" } else { "a parallel step has no tool" });
    }
    if !("tool" in st) || type_of(st.tool) != "string" || st.tool == "" {
        return label + ": missing tool (full tool name, e.g. read_file or mcp_cua-driver__click)";
    }
    if st.tool == "tool_batch" { return label + ": tool_batch cannot call itself"; }
    if st.tool == "use_tool" && tb_has(st, "args") && type_of(st.args) == "map"
        && tb_has(st.args, "tool_name") && st.args.tool_name == "tool_batch" {
        return label + ": tool_batch cannot call itself (not even via use_tool)";
    }
    let schema = host.tool_schema(st.tool);
    if schema == () {
        return label + ": unknown tool \"" + st.tool + "\" (not registered, or not callable here; MCP tools use their full name mcp_<server>__<tool>)";
    }
    let args = if tb_has(st, "args") { st.args } else { #{} };
    if type_of(args) != "map" { return label + ": args must be an object"; }
    let params = schema.parameters;
    if type_of(params) == "map" && tb_has(params, "required") && type_of(params.required) == "array" {
        for k in params.required {
            if !(k in args) {
                return label + ": " + st.tool + " needs argument \"" + k + "\"";
            }
        }
    }
    if tb_has(st, "on_error") && st.on_error != "stop" && st.on_error != "continue" {
        return label + ": on_error must be \"stop\" or \"continue\"";
    }
    if tb_has(st, "save_as") {
        if type_of(st.save_as) != "string" || st.save_as == "" || st.save_as.contains(".") {
            return label + ": save_as must be a non-empty name without dots";
        }
    }
    if tb_has(st, "expect") && type_of(st.expect) != "string" {
        return label + ": expect must be a string";
    }
    for k in ["when", "transform"] {
        if tb_has(st, k) && type_of(st[k]) != "string" {
            return label + ": " + k + " must be an expression string";
        }
    }
    if tb_has(st, "timeout_ms") {
        let t = st.timeout_ms;
        if type_of(t) == "f64" { t = t.to_int(); }
        if type_of(t) != "i64" || t < 1 || t > tb_max_timeout_ms() {
            return label + ": timeout_ms must be an integer 1.." + tb_max_timeout_ms();
        }
    }
    ""
}

// 先整体校验，一步都不跑；有错直接回，不会跑一半。
fn tb_check(host, args) {
    if type_of(args) != "map" { return "arguments must be a JSON object { steps: [...] }"; }
    if !("steps" in args) || type_of(args.steps) != "array" { return "steps must be an array"; }
    let n = args.steps.len();
    if n == 0 { return "steps is empty"; }
    let calls = 0;
    for i in 0..n {
        let st = args.steps[i];
        if type_of(st) == "map" && tb_has(st, "parallel") {
            if "tool" in st { return "step " + i + ": a step is either { tool, ... } or { parallel: [...] }, not both"; }
            if type_of(st.parallel) != "array" || st.parallel.len() == 0 {
                return "step " + i + ": parallel must be a non-empty array of calls";
            }
            if st.parallel.len() > tb_max_parallel() {
                return "step " + i + ": at most " + tb_max_parallel() + " calls in one parallel group";
            }
            if tb_has(st, "on_error") && st.on_error != "stop" && st.on_error != "continue" {
                return "step " + i + ": on_error must be \"stop\" or \"continue\"";
            }
            for j in 0..st.parallel.len() {
                let bad = tb_check_call(host, st.parallel[j], "step " + i + "." + j, true);
                if bad != "" { return bad; }
                calls += 1;
            }
        } else {
            let bad = tb_check_call(host, st, "step " + i, false);
            if bad != "" { return bad; }
            calls += 1;
        }
    }
    if calls > tb_max_calls() {
        return "too many calls: " + calls + " > " + tb_max_calls() + " (split the batch)";
    }
    if tb_has(args, "return") && type_of(args["return"]) != "array" {
        return "return must be an array of variable names";
    }
    ""
}

// when / transform 走 host.eval_expr；插件没开 allow_eval 时报错（不会默默跳过）。
fn tb_eval(host, expr, vars) {
    try {
        return #{ ok: true, value: host.eval_expr(expr, vars) };
    } catch (e) {
        return #{ ok: false, err: "" + e };
    }
}

// 跑之前：代入引用、算 when。→ #{ ok, skip, args, err }
fn tb_prepare(host, st, vars) {
    if tb_has(st, "when") {
        let w = tb_eval(host, st.when, vars);
        if !w.ok { return #{ ok: false, err: "when: " + w.err }; }
        if type_of(w.value) != "bool" { return #{ ok: false, err: "when must evaluate to true/false, got " + type_of(w.value) }; }
        if !w.value { return #{ ok: true, skip: true }; }
    }
    let raw = if tb_has(st, "args") { st.args } else { #{} };
    let sub = tb_subst(raw, vars);
    if !sub.ok { return #{ ok: false, err: sub.err }; }
    #{ ok: true, skip: false, args: sub.value }
}

// 跑完之后：is_error / expect / transform → #{ failed, reason, value }
fn tb_settle(host, st, r, vars) {
    let failed = r.is_error;
    let reason = if failed { tb_first_line(r.text, 300) } else { "" };
    if !failed && tb_has(st, "expect") && !r.text.contains(st.expect) {
        failed = true;
        reason = "expect not found: \"" + st.expect + "\" (got: " + tb_first_line(r.text, 120) + ")";
    }
    let value = tb_parse(r.text);
    if !failed && tb_has(st, "transform") {
        let scope = vars;
        scope.result = value;
        let t = tb_eval(host, st.transform, scope);
        if t.ok { value = t.value; } else { failed = true; reason = "transform: " + t.err; }
    }
    if r.images > 0 { reason += (if reason == "" { "" } else { "; " }) + r.images + " image(s) not kept"; }
    #{ failed: failed, reason: reason, value: value }
}

fn tb_int(v) { if type_of(v) == "f64" { v.to_int() } else { v } }

fn tb_opts(st) {
    if tb_has(st, "timeout_ms") { #{ timeout_ms: tb_int(st.timeout_ms) } } else { #{} }
}

fn tb_trace(label, tool, failed, ms, reason) {
    label + " " + tool + " " + (if failed { "fail" } else { "ok" }) + " " + ms + "ms" + (if reason != "" { " — " + reason } else { "" })
}

fn tb_run(host, args) {
    let started = timestamp();
    let out = #{ ok: false, completed: 0, skipped: 0, failed_step: (), error: (), results: #{}, trace: [], stopped: false, elapsed_ms: 0 };
    let bad = tb_check(host, args);
    if bad != "" { return tb_bad(out, bad); }

    let max = tb_default_max_chars();
    if tb_has(args, "max_chars") {
        let m = args.max_chars;
        if type_of(m) == "f64" { m = m.to_int(); }
        if type_of(m) != "i64" || m < 1 { return tb_bad(out, "max_chars must be a positive integer"); }
        max = m;
    }

    let steps = args.steps;
    let n = steps.len();
    let total = 0;
    let vars = #{};
    let last_key = ();
    let stopped = false;
    for i in 0..n {
        let st = steps[i];
        if host.cancelled() {
            out.failed_step = i;
            out.error = "cancelled by the user before step " + i;
            stopped = true;
            break;
        }
        if started.elapsed > tb_budget_secs() {
            out.failed_step = i;
            out.error = "time budget " + tb_budget_secs() + "s used up before step " + i;
            stopped = true;
            break;
        }
        let on_error = if tb_has(st, "on_error") { st.on_error } else { "stop" };
        let t0 = timestamp();
        let step_failed = false;

        if tb_has(st, "parallel") {
            let group = st.parallel;
            total += group.len();
            // 组里每颗都只能引用前面步骤的结果：先全部代入，再一起跑。
            let preps = [];
            let calls = [];
            let broken = "";
            for j in 0..group.len() {
                let p = tb_prepare(host, group[j], vars);
                if !p.ok { broken = "step " + i + "." + j + " (" + group[j].tool + "): " + p.err; break; }
                preps.push(p);
                if !p.skip {
                    let c = #{ tool: group[j].tool, args: p.args };
                    if tb_has(group[j], "timeout_ms") { c.timeout_ms = tb_int(group[j].timeout_ms); }
                    calls.push(c);
                }
            }
            if broken != "" {
                // 引用 / when 写错是批次本身写错了：不管 on_error，一律停。
                out.trace.push("#" + i + " parallel fail 0ms (bad step)");
                out.failed_step = i;
                out.error = broken;
                stopped = true;
                break;
            }
            let rs = host.call_tools_parallel(calls);
            let ms = (t0.elapsed * 1000.0).to_int();
            let values = [];
            let k = 0;
            for j in 0..group.len() {
                let m = group[j];
                if preps[j].skip {
                    out.skipped += 1;
                    values.push(());
                    out.trace.push("#" + i + "." + j + " " + m.tool + " skip (when false)");
                    continue;
                }
                let s = tb_settle(host, m, rs[k], vars);
                k += 1;
                values.push(s.value);
                if tb_has(m, "save_as") { vars[m.save_as] = s.value; }
                out.trace.push(tb_trace("#" + i + "." + j, m.tool, s.failed, ms, s.reason));
                if s.failed {
                    step_failed = true;
                    if out.failed_step == () {
                        out.failed_step = i;
                        out.error = "step " + i + "." + j + " (" + m.tool + "): " + s.reason;
                    }
                } else {
                    out.completed += 1;
                }
            }
            vars["step" + i] = values;
            last_key = "step" + i;
        } else {
            total += 1;
            let p = tb_prepare(host, st, vars);
            if !p.ok {
                out.trace.push("#" + i + " " + st.tool + " fail 0ms (bad step)");
                out.failed_step = i;
                out.error = "step " + i + " (" + st.tool + "): " + p.err;
                stopped = true;
                break;
            }
            if p.skip {
                out.skipped += 1;
                out.trace.push("#" + i + " " + st.tool + " skip (when false)");
                continue;
            }
            let r = host.call_tool_ex(st.tool, p.args, tb_opts(st));
            let s = tb_settle(host, st, r, vars);
            let ms = (t0.elapsed * 1000.0).to_int();
            let key = "step" + i;
            vars[key] = s.value;
            if tb_has(st, "save_as") {
                key = st.save_as;
                vars[key] = s.value;
            }
            last_key = key;
            out.trace.push(tb_trace("#" + i, st.tool, s.failed, ms, s.reason));
            if s.failed {
                step_failed = true;
                if out.failed_step == () {
                    out.failed_step = i;
                    out.error = "step " + i + " (" + st.tool + "): " + s.reason;
                }
            } else {
                out.completed += 1;
            }
        }
        if step_failed && on_error == "stop" {
            stopped = true;
            break;
        }
    }

    let names = if tb_has(args, "return") { args["return"] } else if last_key != () { [last_key] } else { [] };
    let unset = [];
    for name in names {
        let nm = if type_of(name) == "string" { name } else { name.to_string() };
        let r = tb_resolve(nm, vars);
        if r.ok {
            out.results[nm] = tb_trunc(r.value, max);
        } else {
            out.results[nm] = ();
            unset.push(r.err);
        }
    }
    if unset.len() > 0 { out.unset = unset; }
    out.ok = out.failed_step == () && out.completed + out.skipped == total;
    out.elapsed_ms = (started.elapsed * 1000.0).to_int();
    out.stopped = stopped;
    out
}

#{
    inject: ["tools"],
    apply: |host| {
        host.register_tool(#{
            name: "tool_batch",
            description: "一次调用跑多个工具（Dock 内置、MCP 如 mcp_cua-driver__*、其它插件工具），顶好几轮来回。中间结果留在批里不进上下文，只回 `return` 点名的变量（默认最后一步），每项截断到 max_chars（默认 4000）。\n" +
                "steps[i] 二选一：\n" +
                "- 调用：{ tool: 工具全名, args?: {...}, save_as?: 变量名, expect?: 结果必须包含的子串, when?: 表达式, transform?: 表达式, timeout_ms?: 毫秒, on_error?: \"stop\"(默认)|\"continue\" }\n" +
                "- 并行分组：{ parallel: [调用, ...], on_error? }：组里互不依赖的调用同时跑（最多 16 颗），整组跑完再往下；组结果存为数组 stepN（stepN.0、stepN.1…），各颗也可 save_as。\n" +
                "args 里的字符串可写 ${var} / ${var.a.b} / ${var.items.0}（-1 是最后一个）引用前面步骤的结果；整串只有一个引用时代入原值，否则拼成文本。每步结果也自动存为 step0、step1…；结果文本是 JSON 就解析后存。\n" +
                "when：Rhai 表达式，前面存的变量直接当名字用（如 \"win.count > 0\"），false 就跳过这步（不算失败）。transform：对这步结果求值后再存，结果叫 result（如 \"result.items.len()\"）。\n" +
                "先拿 schema：每颗要用的工具先 search_tool 查一下（上下文里已有的会回 schema_in_context，不必重查），按它的 input_schema 写 args，再拼 steps；批次自己的预校验只是兜底，代替不了 search_tool。\n" +
                "什么时候用：顺序已知的一串操作，或一次做多个只读查询再汇总（放进 parallel），尤其是跨工具组合。纯 cua 动作序列直接用 cua-driver 的 run_steps / run_script。只把互不依赖的调用放进 parallel；写文件、bash、桌面操作等有副作用的即使放进 parallel，宿主也会按顺序一颗颗跑。\n" +
                "规则：调用前整批先校验（工具名、必填参数、形状），有错一步都不跑；失败按工具自己的 is_error 判；需审批的工具仍逐次弹审批；最多 50 次调用、总 120 秒（步与步之间检查，单步用 timeout_ms 限时）；用户停止时立刻中止；引用 / when 写错一律停；图片不回传；不能嵌套 tool_batch（经 use_tool 也不行）；超时只说明没等到结果，那颗调用可能已经执行、副作用可能已经发生。返回 JSON：{ ok, completed, skipped, failed_step, error, results, trace, elapsed_ms, stopped }；中途停下或被拒时这次调用标记为失败。",
            parameters: #{
                type: "object",
                properties: #{
                    steps: #{
                        type: "array",
                        description: "按顺序执行的步骤；一步是一个调用，或 { parallel: [调用...] }。总共最多 50 次调用",
                        items: #{
                            type: "object",
                            properties: #{
                                tool: #{ type: "string", description: "工具全名，如 read_file、bash、mcp_cua-driver__get_window_state" },
                                args: #{ type: "object", description: "工具参数；字符串里可用 ${var} / ${var.path}" },
                                save_as: #{ type: "string", description: "把结果存成这个变量名（不能含点）" },
                                expect: #{ type: "string", description: "结果文本必须包含的子串，不含则算失败" },
                                when: #{ type: "string", description: "表达式，false 时跳过这步，如 \"hits.len() > 0\"" },
                                transform: #{ type: "string", description: "表达式，结果叫 result，求得的值代替结果存下，如 \"result.items[0].id\"" },
                                timeout_ms: #{ type: "integer", description: "这一步的时限（毫秒，最多 600000），到点算失败；含等审批的时间" },
                                on_error: #{ type: "string", "enum": ["stop", "continue"], description: "失败时停（默认）还是继续" },
                                parallel: #{ type: "array", items: #{ type: "object" }, description: "并行分组：一组互不依赖的调用（同上的字段，不含 parallel），同时跑" }
                            }
                        }
                    },
                    "return": #{ type: "array", items: #{ type: "string" }, description: "要返回的变量名或路径（如 win.title）；省略则只回最后一步" },
                    max_chars: #{ type: "integer", description: "每项返回内容的截断上限，默认 4000" }
                },
                required: ["steps"]
            },
            execute: |args| {
                let out = tb_run(host, args);
                #{ is_error: out.stopped || (out.error != () && out.trace.len() == 0), content: out.to_json() }
            }
        });
    }
}
