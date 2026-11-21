// 完成标准：这一轮改了 Rust 文件却还没跑过 cargo test，就不许收尾——提醒模型去跑，
// 同时在这一轮下面挂一张卡片，用户也能一键让它去跑。
// 拷到 .dock/plugins/done-check/source.rhai → cordis_plugin { action: "reload", id: "done-check" }。
#{
    inject: [],
    apply: |host| {
        // 每页一份：改过哪些 .rs、之后有没有跑过测试。
        let pages = #{};

        host.on("agent/pre-step", |s| {
            // 新的一条用户消息：重新记账。
            pages[s.identity] = #{ edited: [], tested: false };
            ()
        });

        host.on("tools/execute", |r| {
            if !r.main || r.is_error { return (); }
            let page = pages[r.identity] ?? #{ edited: [], tested: false };
            if r.name == "search_replace" || r.name == "write_file" {
                // search_replace 的路径参数叫 file_path，write_file 的叫 target_file。
                let path = r.args?.file_path ?? r.args?.target_file ?? "";
                if path.ends_with(".rs") && !page.edited.contains(path) {
                    page.edited.push(path);
                    page.tested = false;
                }
            }
            if r.name == "bash" && (r.args?.command ?? "").contains("cargo test") {
                page.tested = true;
            }
            pages[r.identity] = page;
            ()
        });

        host.on("agent/turn-end", |e| {
            let page = pages[e.identity];
            if !e.main || page == () || page.tested || page.edited.is_empty() || e.rounds > 0 {
                return ();
            }
            let files = page.edited.reduce(|s, v| if s == () { v } else { s + "、" + v });
            // 卡片是锦上添花：宿主没有卡片位（比如没开 GUI 的测试树）也不能把提醒一起吞掉。
            try { host.card(#{
                id: "tests",
                title: "改了 Rust 文件，还没跑测试",
                text: files,
                tone: "warning",
                actions: [#{ id: "run", label: "跑测试" }],
                on_action: |a| {
                    host.followup("用户点了「跑测试」：跑一下刚改过的 crate 的 cargo test，把结果告诉用户。");
                    "已经让 agent 去跑了"
                }
            }); } catch (err) { }
            "这一轮改了 " + files + "，还没跑过 cargo test。先跑改到的那个 crate 的测试，通过了再收尾；跑不了就说明原因。"
        });
    }
}
