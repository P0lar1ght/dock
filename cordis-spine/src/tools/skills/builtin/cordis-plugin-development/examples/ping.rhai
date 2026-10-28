// 最小的插件：一颗不带参数的工具。
// cordis_define { plugin: { kind: "new", idPrefix: "demo" }, name: "Demo",
//                 purpose: "ping", factory: "rhai",
//                 source_path: ".dock/plugins/demo/source.rhai" }
// → cordis_run mode "run" → cordis_call { name: "demo_ping" }
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
