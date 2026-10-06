// 会话轨迹：GUI 里是插件自带的 web 面板（panel.html，沙箱 iframe，经窄桥读会话事件）；
// 终端里没有 web，只画一行提示。插件本身不观察、不改 Agent 的任何行为。
#{
    inject: ["tui.slots"],
    apply: |host| {
        host.register_slot(#{
            id: "trajectory",
            title: "会话轨迹",
            render: || { "会话轨迹是 web 面板：在 GUI 的右侧面板「插件面板」里打开（可以弹出成独立窗）。" },
            web: "panel.html"
        });
    }
}
