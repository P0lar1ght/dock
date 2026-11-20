// 规约守门：把 AGENTS.md 里的硬约束变成保证（模型想不想都会生效）。
// 拷到 .dock/plugins/guard/source.rhai → cordis_plugin { action: "reload", id: "guard" }。
// 按项目改下面三张表。
fn denied_command(cmd) {
    // 不可回滚、或者会动别人工作区的命令：直接拒。
    for bad in ["git reset --hard", "git push --force", "git push -f", "git stash", "rm -rf /"] {
        if cmd.contains(bad) { return bad; }
    }
    ()
}

fn asked_command(cmd) {
    // 能做，但要用户点头（自动批准模式下也问）。
    for risky in ["git push", "git commit", "npm publish", "cargo publish"] {
        if cmd.contains(risky) { return risky; }
    }
    ()
}

fn frozen_path(path) {
    // 冻结目录：只许读，不许改。
    for dir in ["vendor/", "third_party/"] {
        if path.starts_with(dir) || path.contains("/" + dir) { return dir; }
    }
    ()
}

#{
    inject: [],
    apply: |host| {
        host.on("tools/pre-execute", |c| {
            if c.name == "bash" {
                let cmd = c.args?.command ?? "";
                let bad = denied_command(cmd);
                if bad != () {
                    return #{ deny: "项目规约不许 `" + bad + "`：换一条能回滚的做法，或者请用户自己来。" };
                }
                let risky = asked_command(cmd);
                if risky != () {
                    return #{ ask: "`" + risky + "` 会改到仓库之外" };
                }
            }
            if c.name == "search_replace" || c.name == "write_file" {
                let path = c.args?.file_path ?? "";
                let dir = frozen_path(path);
                if dir != () {
                    return #{ deny: dir + " 是冻结副本，不许直接改。要改就把代码复制到自己的模块里。" };
                }
            }
            ()
        });
    }
}
