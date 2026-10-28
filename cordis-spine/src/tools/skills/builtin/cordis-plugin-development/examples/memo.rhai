// 状态 + 两颗工具 + 斜杠 + 面板。
// store 被所有闭包共享；只活在这次 Run 里，update / stop / 重启都会清掉。
#{
    inject: ["tools", "slash", "tui.slots"],
    apply: |host| {
        let store = #{ text: "" };
        // 斜杠放最前：保留名会在这里先失败，后面什么都还没登记。
        host.register_slash(#{
            command: "memo",
            kind: "slot",
            text: "memo",
            description: "打开便签"
        });
        host.register_tool(#{
            name: "memo_get",
            description: "Read the memo text",
            parameters: #{ type: "object", properties: #{} },
            execute: |args| { if store.text == "" { "（空）" } else { store.text } }
        });
        host.register_tool(#{
            name: "memo_set",
            description: "Replace the memo text",
            parameters: #{
                type: "object",
                properties: #{ text: #{ type: "string" } },
                required: ["text"]
            },
            execute: |args| {
                store.text = args.text;
                host.slot_changed("memo");
                "已保存 " + store.text.len() + " 字"
            }
        });
        host.register_slot(#{
            id: "memo",
            title: "便签",
            hud: true,
            render: || { if store.text == "" { "（空）" } else { store.text } },
            actions: [#{ id: "clear", label: "清空" }],
            on_key: |key| {
                if key == "esc" { return "close"; }
                if key == "clear" { store.text = ""; host.slot_changed("memo"); }
                ()
            }
        });
    }
}
