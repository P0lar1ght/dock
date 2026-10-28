// 把一个 HTTP API 包成工具：设置卡放密钥，缺密钥时返回提示而不是抛错。
// 用户填密钥：GUI 设置页本插件的卡片，或终端 `/cordis set <插件 id> github <token>`。
#{
    inject: ["tools", "plugin.settings"],
    apply: |host| {
        host.register_settings(#{ title: "GitHub", fields: [
            #{ key: "api", type: "string", label: "API 地址", "default": "https://api.github.com" },
            #{ key: "github", type: "secret", label: "Token" }
        ] });
        host.register_tool(#{
            name: "gh_issue",
            description: "Read one GitHub issue: title and body",
            parameters: #{
                type: "object",
                properties: #{
                    repo: #{ type: "string", description: "owner/name" },
                    n: #{ type: "integer", description: "issue number" }
                },
                required: ["repo", "n"]
            },
            execute: |args| {
                // http_request / host.secret 只在运行期有，所以都写在 execute 里。
                let token = ();
                try { token = host.secret("github"); } catch (e) { }
                if token == () {
                    return "未配置 GitHub Token：请在设置页「GitHub」卡片里填写，"
                        + "或在终端执行 /cordis set <插件 id> github <token>";
                }
                let resp = http_request(#{
                    url: host.setting("api") + "/repos/" + args.repo + "/issues/" + args.n,
                    headers: #{
                        "Accept": "application/vnd.github+json",
                        "User-Agent": "dock",
                        "Authorization": "Bearer " + token
                    }
                });
                // 4xx / 5xx 不抛错，自己看 ok。
                if !resp.ok { return "HTTP " + resp.status + ": " + resp.body; }
                let issue = parse_json(resp.body);
                issue.title + "\n\n" + (issue.body ?? "")
            }
        });
    }
}
