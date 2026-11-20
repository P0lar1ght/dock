// 最小的插件：一颗不带参数的工具。
// 拷到 .dock/plugins/demo/source.rhai
// → cordis_plugin { action: "reload", id: "demo" }
// → use_tool { tool_name: "demo_ping", tool_input: {} }
#{
    inject: ["tools"],
    apply: |host| {
        host.register_tool(#{
            name: "demo_ping",
            description: "Return pong",
            parameters: #{ type: "object", properties: #{} },
            execute: |args| { "pong" }
        });
    }
}
