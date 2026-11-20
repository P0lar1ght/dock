// 输出瘦身：去掉 bash 输出里的 ANSI 颜色码，超长的只留头尾。省 token，也让前缀缓存更稳。
// 拷到 .dock/plugins/trim-output/source.rhai → cordis_plugin { action: "reload", id: "trim-output" }。
const HEAD = 3000;
const TAIL = 2000;

#{
    inject: [],
    apply: |host| {
        host.on("tools/execute", |r| {
            if r.name != "bash" { return (); }
            let text = regex_replace("\u001b\\[[0-9;?]*[A-Za-z]", r.content, "");
            let n = text.len();
            if n <= HEAD + TAIL {
                return if text == r.content { () } else { text };
            }
            text.sub_string(0, HEAD)
                + "\n\n[trim-output：中间 " + (n - HEAD - TAIL) + " 个字符省略了；要看全文就把输出写进文件再读]\n\n"
                + text.sub_string(n - TAIL, TAIL)
        });
    }
}
