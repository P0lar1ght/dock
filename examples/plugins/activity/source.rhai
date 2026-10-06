// Agent 活动：实时看 Agent 在做什么。适合拖成独立窗口挂在副屏上（插件面板可以弹出、置顶）。
// - 面板：当前状态、本轮第几步、工具用量表、最近活动；按钮：暂停 / 继续记录、清零。
// - 状态项：输入框工具栏上「运行中 · 第 3 步」/「空闲 · 工具 12 次」，点它打开面板。
// - 设置卡：最近活动保留几条、要不要显示状态项。
// 只观察（session/event、agent/step-start、agent/turn-end 都回 () = 不插话），不改 Agent 的行为。
#{
    inject: ["tools", "tui.slots"],
    apply: |host| {
        // 闭包捕获的变量是共享的：所有回调改的是同一份。
        let state = #{
            recording: true,
            running: false,
            step: 0,
            turns: 0,
            total: 0,
            tools: #{},
            recent: [],
            turn_started: (),
            last_turn_secs: ()
        };

        host.register_settings(#{
            title: "Agent 活动",
            fields: [
                #{ key: "recent_limit", type: "number", label: "最近活动保留条数", min: 5, max: 50, "default": 12,
                   description: "面板「最近活动」里最多列几条，5–50" },
                #{ key: "show_status", type: "boolean", label: "在输入框显示状态项", "default": true,
                   description: "关掉后只在面板里看" }
            ]
        });

        let clip = |s, n| {
            if s.len() > n { s.sub_string(0, n) + "…" } else { s }
        };

        let refresh = |host, state| {
            let show = host.setting("show_status");
            if show == false {
                host.clear_status("activity");
            } else if state.running {
                host.set_status(#{ id: "activity", text: `运行中 · 第 ${state.step} 步`, tone: "accent",
                    tooltip: "Agent 正在工作，点开看活动", surface: "activity" });
            } else if !state.recording {
                host.set_status(#{ id: "activity", text: "记录已暂停", tone: "muted", surface: "activity" });
            } else {
                host.set_status(#{ id: "activity", text: `空闲 · 工具 ${state.total} 次`,
                    tooltip: `已完成 ${state.turns} 轮`, surface: "activity" });
            }
            host.slot_changed("activity");
        };

        host.on("agent/step-start", |s| {
            if !state.recording || !s.main { return (); }
            if !state.running {
                state.running = true;
                state.turn_started = timestamp();
            }
            state.step = s.step + 1;
            refresh.call(host, state);
            ()
        });

        host.on("agent/turn-end", |e| {
            if !state.recording || !e.main { return (); }
            state.running = false;
            state.turns += 1;
            if state.turn_started != () {
                state.last_turn_secs = state.turn_started.elapsed;
            }
            refresh.call(host, state);
            ()
        });

        host.on("session/event", |line| {
            if !state.recording { return; }
            let parts = line.split("\t");
            let kind = parts[0];
            let text = if parts.len() > 1 { let t = parts[1]; t.trim(); t } else { "" };
            // 只留看得懂的几种：你说的、回复、工具、本轮结束。空行、「tools:…」预告（下一行就是工具本身）不要。
            let row = switch kind {
                "user" => #{ title: clip.call(text, 60), badge: #{ text: "你", tone: "default" } },
                "assistant" => if text == "" || text.starts_with("tools:") { () } else {
                    #{ title: clip.call(text, 60), badge: #{ text: "回复", tone: "success" } }
                },
                "tool" => #{ title: text, badge: #{ text: "工具", tone: "accent" } },
                "turn-end" => #{ title: "本轮结束", subtitle: `第 ${state.turns} 轮 · ${state.step} 步`, badge: #{ text: "完成", tone: "muted" } },
                _ => ()
            };
            if kind == "tool" {
                state.total += 1;
                state.tools[text] = if text in state.tools { state.tools[text] + 1 } else { 1 };
            }
            if row == () || row.title == "" {
                if kind == "tool" { refresh.call(host, state); }
                return;
            }
            // 同一句连着来两次（流式收尾时会重复一次）只记一条。
            if state.recent.len() > 0 && state.recent[0].title == row.title && state.recent[0].badge.text == row.badge.text { return; }
            state.recent.insert(0, row);
            let limit = host.setting("recent_limit");
            let limit = if limit == () { 12 } else if type_of(limit) == "f64" { limit.to_int() } else { limit };
            while state.recent.len() > limit { state.recent.pop(); }
            // 面板和状态项一起刷：工具次数两边要对得上。
            refresh.call(host, state);
        });

        host.register_slot(#{
            id: "activity",
            title: "Agent 活动",
            render: || {
                `状态：${if state.running { "运行中" } else { "空闲" }}  工具 ${state.total} 次  完成 ${state.turns} 轮`
            },
            on_key: |key| {
                if key == "toggle" {
                    state.recording = !state.recording;
                    if !state.recording { state.running = false; }
                } else if key == "reset" {
                    state.tools = #{};
                    state.total = 0;
                    state.turns = 0;
                    state.recent = [];
                    state.last_turn_secs = ();
                }
                refresh.call(host, state);
                ()
            },
            view: || {
                let status = if !state.recording {
                    #{ label: "状态", value: "记录已暂停", tone: "muted" }
                } else if state.running {
                    #{ label: "状态", value: `运行中 · 第 ${state.step} 步`, tone: "accent" }
                } else {
                    #{ label: "状态", value: "空闲", tone: "success" }
                };
                let last = if state.last_turn_secs == () { "—" } else { `${state.last_turn_secs.to_int()} 秒` };
                let rows = [];
                for name in state.tools.keys() { rows.push([name, `${state.tools[name]}`]); }
                rows.sort(|a, b| parse_int(b[1]) - parse_int(a[1]));
                let tools = if rows.is_empty() {
                    #{ type: "empty", title: "还没有工具调用" }
                } else {
                    #{ type: "table", columns: ["工具", "次数"], rows: rows }
                };
                let recent = if state.recent.is_empty() {
                    #{ type: "text", text: "开始对话后，这里会实时列出 Agent 的每一步。", tone: "muted", size: "s" }
                } else {
                    #{ type: "list", items: state.recent }
                };
                #{ type: "stack", children: [
                    #{ type: "kv", items: [
                        status,
                        #{ label: "完成轮次", value: `${state.turns}` },
                        #{ label: "工具调用", value: `${state.total} 次` },
                        #{ label: "上一轮用时", value: last }
                    ] },
                    #{ type: "section", title: "工具用量", children: [tools] },
                    #{ type: "section", title: "最近活动", children: [recent] },
                    #{ type: "row", children: [
                        #{ type: "button", label: "清零", action: "reset" },
                        #{ type: "button", label: if state.recording { "暂停记录" } else { "继续记录" },
                           action: "toggle", style: "primary" }
                    ] }
                ] }
            }
        });

        refresh.call(host, state);
    }
}
