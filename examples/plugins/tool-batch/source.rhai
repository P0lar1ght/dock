// tool-batch：注册模型工具 `tool_batch`，一次调用按顺序跑一串工具（内置 / MCP / 其它插件），
// 中间结果存在插件里的变量表，不进模型上下文；最后只回 `return` 点名的几项（截断到 max_chars）。
//
// 只用 host.call_tool；不联网（不碰 http_request）、不读写文件（不碰 host.read_bytes）、不用 eval。
// 宿主的限制和绕法见同目录 HOST_GAPS.md。
//
// 注意：Rhai 的 `fn` 看不见外层变量和顶层常量，所以上限写成函数、host 当参数传进去。

fn tb_max_steps() { 50 }
fn tb_budget_secs() { 120.0 }
fn tb_default_max_chars() { 4000 }

// 任意值 → JSON 文本。宿主没给通用的 to_json（Rhai 只有 Map.to_json），包一层再剥掉。
fn tb_json(v) {
    let s = #{ v: v }.to_json();
    s.sub_string(5, s.len() - 6)
}

// 工具结果文本 → 值：像 JSON 对象 / 数组就解析，否则原样当字符串。
// Rhai 的 parse_json 只认对象（顶层必须是 `{`），数组包一层再取出来。
fn tb_parse(text) {
    let t = text;
    t.trim();
    if t.starts_with("{") {
        try { return parse_json(t); } catch (e) { }
    } else if t.starts_with("[") {
        try { return parse_json("{\"v\":" + t + "}").v; } catch (e) { }
    }
    text
}

// call_tool 只回 content、丢了 is_error，只能照着宿主的兜底规则（tool_output_looks_failed）
// 和几条已知的拒绝 / 拦截文案猜。
fn tb_failed(text) {
    let t = text;
    t.trim();
    for p in [
        "Error", "error", "exit ", "已中断。", "权限被拒绝", "计划模式已阻止", "只读模式：",
        "MCP 工具未启用或已关闭", "当前 Agent 预设未包含此工具"
    ] {
        if t.starts_with(p) { return true; }
    }
    false
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
        out += if type_of(r.value) == "string" { r.value } else { tb_json(r.value) };
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
    let s = if type_of(v) == "string" { v } else { tb_json(v) };
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

// 先整体校验形状，一步都不跑；有错直接回，不会跑一半。
fn tb_check(args) {
    if type_of(args) != "map" { return "arguments must be a JSON object { steps: [...] }"; }
    if !("steps" in args) || type_of(args.steps) != "array" { return "steps must be an array"; }
    let n = args.steps.len();
    if n == 0 { return "steps is empty"; }
    if n > tb_max_steps() {
        return "too many steps: " + n + " > " + tb_max_steps() + " (split the batch)";
    }
    for i in 0..n {
        let st = args.steps[i];
        if type_of(st) != "map" { return "step " + i + ": must be an object"; }
        if !("tool" in st) || type_of(st.tool) != "string" || st.tool == "" {
            return "step " + i + ": missing tool (full tool name, e.g. read_file or mcp_cua-driver__click)";
        }
        if st.tool == "tool_batch" { return "step " + i + ": tool_batch cannot call itself"; }
        if "on_error" in st && st.on_error != () && st.on_error != "stop" && st.on_error != "continue" {
            return "step " + i + ": on_error must be \"stop\" or \"continue\"";
        }
        if "save_as" in st && st.save_as != () {
            if type_of(st.save_as) != "string" || st.save_as == "" || st.save_as.contains(".") {
                return "step " + i + ": save_as must be a non-empty name without dots";
            }
        }
        if "expect" in st && st.expect != () && type_of(st.expect) != "string" {
            return "step " + i + ": expect must be a string";
        }
    }
    if "return" in args && args["return"] != () {
        if type_of(args["return"]) != "array" { return "return must be an array of variable names"; }
    }
    ""
}

fn tb_run(host, args) {
    let started = timestamp();
    let out = #{ ok: false, completed: 0, failed_step: (), error: (), results: #{}, trace: [], stopped: false, elapsed_ms: 0 };
    let bad = tb_check(args);
    if bad != "" { return tb_bad(out, bad); }

    let max = tb_default_max_chars();
    if "max_chars" in args && args.max_chars != () {
        let m = args.max_chars;
        if type_of(m) == "f64" { m = m.to_int(); }
        if type_of(m) != "i64" || m < 1 { return tb_bad(out, "max_chars must be a positive integer"); }
        max = m;
    }

    let steps = args.steps;
    let n = steps.len();
    let vars = #{};
    let last_key = ();
    let stopped = false;
    for i in 0..n {
        let st = steps[i];
        if started.elapsed > tb_budget_secs() {
            out.failed_step = i;
            out.error = "time budget " + tb_budget_secs() + "s used up before step " + i;
            stopped = true;
            break;
        }
        let on_error = if "on_error" in st && st.on_error != () { st.on_error } else { "stop" };
        let t0 = timestamp();
        let raw_args = if "args" in st && st.args != () { st.args } else { #{} };
        let sub = tb_subst(raw_args, vars);
        if !sub.ok {
            // 引用错是批次本身写错了：不管 on_error，一律停。
            out.trace.push("#" + i + " " + st.tool + " fail 0ms (bad reference)");
            out.failed_step = i;
            out.error = "step " + i + " (" + st.tool + "): " + sub.err;
            stopped = true;
            break;
        }
        let text = "";
        let failed = false;
        let reason = "";
        try {
            text = host.call_tool(st.tool, sub.value);
        } catch (e) {
            failed = true;
            reason = "call_tool threw: " + e;
            text = "Error: " + e;
        }
        if !failed && tb_failed(text) {
            failed = true;
            reason = tb_first_line(text, 300);
        }
        if !failed && "expect" in st && st.expect != () && !text.contains(st.expect) {
            failed = true;
            reason = "expect not found: \"" + st.expect + "\" (got: " + tb_first_line(text, 120) + ")";
        }
        let ms = (t0.elapsed * 1000.0).to_int();
        let value = tb_parse(text);
        let key = "step" + i;
        vars[key] = value;
        if "save_as" in st && st.save_as != () {
            key = st.save_as;
            vars[key] = value;
        }
        last_key = key;
        out.trace.push("#" + i + " " + st.tool + " " + (if failed { "fail" } else { "ok" }) + " " + ms + "ms" + (if failed { " — " + reason } else { "" }));
        if failed {
            if out.failed_step == () {
                out.failed_step = i;
                out.error = "step " + i + " (" + st.tool + "): " + reason;
            }
            if on_error == "stop" {
                stopped = true;
                break;
            }
        } else {
            out.completed += 1;
        }
    }

    let names = if "return" in args && args["return"] != () { args["return"] } else if last_key != () { [last_key] } else { [] };
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
    out.ok = out.failed_step == () && out.completed == n;
    out.elapsed_ms = (started.elapsed * 1000.0).to_int();
    out.stopped = stopped;
    out
}

#{
    inject: ["tools"],
    apply: |host| {
        host.register_tool(#{
            name: "tool_batch",
            description: "按顺序批量调用多个工具（Dock 内置、MCP 如 mcp_cua-driver__*、其它插件工具），一次调用顶十几轮。中间结果留在批里不进上下文，只回 `return` 点名的变量（默认最后一步），每项截断到 max_chars（默认 4000）。\n" +
                "用法：steps[i] = { tool: 工具全名, args: {...}, save_as?: 变量名, expect?: 结果必须包含的子串, on_error?: \"stop\"(默认)|\"continue\" }。args 里的字符串可写 ${var} / ${var.a.b} / ${var.items.0}（数组下标，-1 是最后一个）引用前面步骤的结果；整串只有一个引用时代入原值（对象 / 数组 / 数字原样），否则拼成文本。每步结果也自动存为 step0、step1…。结果文本是 JSON 对象 / 数组就解析后存，否则存字符串。\n" +
                "什么时候用：顺序已知的一串操作（定位→点击→输入→确认），或者一次做多个只读查询再汇总，尤其是跨工具组合（cua + 文件 + bash + 浏览器）。纯 cua 动作序列直接用 cua-driver 的 run_steps（带 wait_for/expect/observe，更快）；要分支 / 循环 / 按结果决定下一步，用 cua-driver 的 run_script 或分几次调用。\n" +
                "限制：串行执行，不并发；拿不到截图 / 图片，也拿不到工具的 is_error 标记（按 Error/权限被拒绝 等开头判失败），确认界面状态请用 get_window_state（query + include_screenshot:false）的文本或 verify_state 配 expect；每个需审批的工具（bash、cua 等）仍会逐次弹审批；最多 50 步、总 120 秒（只在步与步之间检查，正在跑的一步不会被打断）；引用写错一律停；不能嵌套调用 tool_batch。返回 JSON：{ ok, completed, failed_step, error, results, trace, elapsed_ms, stopped }；中途停下时第一行以 Error: 开头。",
            parameters: #{
                type: "object",
                properties: #{
                    steps: #{
                        type: "array",
                        description: "按顺序执行的步骤，最多 50 步",
                        items: #{
                            type: "object",
                            properties: #{
                                tool: #{ type: "string", description: "工具全名，如 read_file、bash、mcp_cua-driver__get_window_state" },
                                args: #{ type: "object", description: "工具参数；字符串里可用 ${var} / ${var.path}" },
                                save_as: #{ type: "string", description: "把结果存成这个变量名（不能含点）" },
                                expect: #{ type: "string", description: "结果文本必须包含的子串，不含则算失败" },
                                on_error: #{ type: "string", "enum": ["stop", "continue"], description: "失败时停（默认）还是继续" }
                            },
                            required: ["tool"]
                        }
                    },
                    "return": #{ type: "array", items: #{ type: "string" }, description: "要返回的变量名或路径（如 win.title）；省略则只回最后一步" },
                    max_chars: #{ type: "integer", description: "每项返回内容的截断上限，默认 4000" }
                },
                required: ["steps"]
            },
            execute: |args| {
                let out = tb_run(host, args);
                let json = out.to_json();
                if out.stopped || (out.error != () && out.trace.len() == 0) {
                    "Error: tool_batch " + (if out.failed_step != () { "stopped at step " + out.failed_step } else { "rejected" }) + ": " + out.error + "\n" + json
                } else {
                    json
                }
            }
        });
    }
}
