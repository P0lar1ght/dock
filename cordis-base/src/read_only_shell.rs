//! 只读场景（计划模式、只读子代理、旁问页）里的 bash：这条命令是不是**只读**的。
//!
//! 判定是保守的：拿不准就算「不是只读」，交给用户批准。能放行的只有这些：
//! - 按 `|` `&&` `||` `;` 换行拆成段，**每一段**的程序都在白名单里；
//! - 没有输出重定向（`>` `>>`，`2>&1` 和 `>/dev/null` 这类不落盘的除外）、
//!   没有命令替换（`` ` `` `$(`）、没有进程替换 `<(` `>(`、没有 `&` 后台；
//! - 程序本身能改东西的参数不在（`find -delete`、`git branch -D`、`sed -i`……）。
//!
//! 误判的代价不对称：把只读的判成要问，只是多弹一次框；把会改东西的判成只读，
//! 只读场景就漏了。所以白名单宁小勿大。

/// 这条 bash 命令能不能在只读场景里不问用户直接跑。
pub fn is_read_only_command(command: &str) -> bool {
    let command = command.trim();
    if command.is_empty() {
        return false;
    }
    // 引号没配上对：shell 会报错，但拿不准它怎么切，按不是只读。
    let Some(lexed) = lex(command) else {
        return false;
    };
    if has_unsafe_syntax(command, &lexed) {
        return false;
    }
    split_segments(&lexed)
        .iter()
        .all(|seg| segment_is_read_only(seg))
}

/// 一个字符在 shell 眼里处在什么位置。
#[derive(Clone, Copy, PartialEq)]
enum At {
    /// 不在引号里、也不是被转义的：`|` `;` `>` `&` 在这里才有语法意义。
    Bare,
    /// 引号里，或者被反斜杠转义了：只是字面字符。
    Literal,
}

/// 按 shell 的引号与转义规则给每个字符标位置。单引号里没有转义；双引号里和引号外
/// `\x` 让 `x` 成为字面字符。引号没闭合回 `None`。
///
/// 以前只认引号不认转义：`echo \"a | rm -rf x \"` 里转义过的 `"` 被当成开引号，
/// 后面的 `| rm` 就被当成引号里的字面字符，整条判成只读——shell 实际会跑 `rm`。
fn lex(command: &str) -> Option<Vec<(char, At)>> {
    let mut out = Vec::with_capacity(command.len());
    let mut chars = command.chars();
    let mut quote: Option<char> = None;
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some('\''), '\'') => {
                quote = None;
                out.push((c, At::Literal));
            }
            (Some('\''), _) => out.push((c, At::Literal)),
            (Some('"'), '"') => {
                quote = None;
                out.push((c, At::Literal));
            }
            (Some(_), '\\') | (None, '\\') => {
                out.push((c, At::Literal));
                if let Some(next) = chars.next() {
                    out.push((next, At::Literal));
                }
            }
            (Some(_), _) => out.push((c, At::Literal)),
            (None, '\'' | '"') => {
                quote = Some(c);
                out.push((c, At::Literal));
            }
            (None, _) => out.push((c, At::Bare)),
        }
    }
    quote.is_none().then_some(out)
}

/// 命令替换、进程替换（引号里也算：双引号里照样会展开，宁可多问）；引号外的后台 `&`
/// 和往文件写的重定向（引号里的 `>` 只是字面字符，比如 `grep -n "=>"`）。
fn has_unsafe_syntax(command: &str, lexed: &[(char, At)]) -> bool {
    if command.contains('`')
        || command.contains("$(")
        || command.contains("<(")
        || command.contains(">(")
    {
        return true;
    }
    let bare = |j: usize| {
        lexed
            .get(j)
            .filter(|(_, at)| *at == At::Bare)
            .map(|(c, _)| *c)
    };
    for (i, &(c, at)) in lexed.iter().enumerate() {
        if at != At::Bare {
            continue;
        }
        match c {
            // 单个 `&` 是后台；`&&` 是连接符，`>&` / `&>` 交给下面的重定向判断。
            '&' => {
                let prev = i.checked_sub(1).and_then(bare);
                let next = bare(i + 1);
                if prev != Some('&') && next != Some('&') && prev != Some('>') && next != Some('>')
                {
                    return true;
                }
            }
            '>' => {
                let rest: Vec<char> = lexed[i..].iter().map(|(c, _)| *c).collect();
                if !harmless_redirect(&rest) {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

/// `>` 开头的这一截是不是不落盘的重定向：`>&1` `>&2`（`2>&1`）或 `>/dev/null`。
fn harmless_redirect(rest: &[char]) -> bool {
    let tail: String = rest.iter().skip(1).collect();
    let tail = tail.trim_start_matches('>').trim_start();
    tail.starts_with("&1") || tail.starts_with("&2") || tail.starts_with("/dev/null")
}

/// 按引号外的 `|` `||` `&&` `;` 换行拆段（引号里、转义过的不算：`jq '.a | length'`）。
fn split_segments(lexed: &[(char, At)]) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut i = 0;
    while i < lexed.len() {
        let (c, at) = lexed[i];
        let next_bare = lexed
            .get(i + 1)
            .filter(|(_, a)| *a == At::Bare)
            .map(|(c, _)| *c);
        if at == At::Bare && matches!(c, '|' | ';' | '\n') {
            if c == '|' && next_bare == Some('|') {
                i += 1;
            }
            out.push(std::mem::take(&mut cur));
        } else if at == At::Bare && c == '&' && next_bare == Some('&') {
            i += 1;
            out.push(std::mem::take(&mut cur));
        } else {
            cur.push(c);
        }
        i += 1;
    }
    out.push(cur);
    out.into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// 粗切词：按空白拆，去掉包着的引号。只用来认程序名和参数，不求是个完整的 shell 解析器。
fn words(segment: &str) -> Vec<String> {
    segment
        .split_whitespace()
        .map(|w| w.trim_matches(|c| c == '\'' || c == '"').to_string())
        .collect()
}

/// 什么参数都安全的程序：只读文件、打印信息、在文本里找东西。
const PLAIN: &[&str] = &[
    "ls",
    "cat",
    "head",
    "tail",
    "wc",
    "grep",
    "egrep",
    "fgrep",
    "rg",
    "ag",
    "tree",
    "pwd",
    "echo",
    "printf",
    "which",
    "whereis",
    "type",
    "file",
    "stat",
    "du",
    "df",
    "basename",
    "dirname",
    "realpath",
    "readlink",
    "sort",
    "uniq",
    "cut",
    "tr",
    "diff",
    "cmp",
    "comm",
    "nl",
    "column",
    "jq",
    "date",
    "uname",
    "whoami",
    "id",
    "hostname",
    "ps",
    "true",
    "false",
    "test",
    "[",
    "md5",
    "md5sum",
    "shasum",
    "sha256sum",
    "cksum",
    "od",
    "hexdump",
    "xxd",
    "strings",
    "less",
    "more",
    "cd",
];

/// `git` 只读的子命令。`branch` / `tag` / `remote` / `config` / `stash` 要看参数。
const GIT_READ: &[&str] = &[
    "status",
    "log",
    "diff",
    "show",
    "blame",
    "rev-parse",
    "ls-files",
    "ls-tree",
    "grep",
    "describe",
    "shortlog",
    "reflog",
    "cat-file",
    "merge-base",
    "rev-list",
    "whatchanged",
    "name-rev",
    "count-objects",
    "for-each-ref",
    "show-ref",
    "help",
    "version",
    "--version",
];

fn segment_is_read_only(segment: &str) -> bool {
    let w = words(segment);
    let Some(prog) = w.first() else {
        return false;
    };
    // `FOO=1 cmd`：环境变量前缀能改变程序行为（比如 `GIT_DIR`），不放行。
    if prog.contains('=') {
        return false;
    }
    let prog = prog.rsplit('/').next().unwrap_or(prog);
    let args = &w[1..];
    let has = |flags: &[&str]| {
        args.iter().any(|a| {
            flags
                .iter()
                .any(|f| a == f || a.starts_with(&format!("{f}=")))
        })
    };
    match prog {
        p if PLAIN.contains(&p) => true,
        // `find` 能删、能对每个结果跑命令、能写文件。
        "find" => !has(&[
            "-delete", "-exec", "-execdir", "-ok", "-okdir", "-fprint", "-fprint0", "-fprintf",
            "-fls",
        ]),
        "fd" | "fdfind" => !has(&["-x", "--exec", "-X", "--exec-batch"]),
        // `sed` 只在不带 `-i` 且脚本里没有 `w` / `e` 命令时才算只读——判不准，只放 `-n` 打印。
        "sed" => {
            args.first().is_some_and(|a| a == "-n") && !args.iter().any(|a| a.starts_with("-i"))
        }
        "git" => git_is_read_only(args),
        "gh" => gh_is_read_only(args),
        "cargo" => matches!(
            args.first().map(String::as_str),
            Some("metadata" | "tree" | "--version" | "-V" | "version")
        ),
        "npm" | "pnpm" | "yarn" => matches!(
            args.first().map(String::as_str),
            Some("ls" | "list" | "view" | "info" | "why" | "--version" | "-v")
        ),
        "node" | "python" | "python3" | "rustc" | "go" | "java" | "ruby" => {
            matches!(args, [a] if a == "--version" || a == "-V" || a == "-v" || a == "version")
        }
        _ => false,
    }
}

/// `gh` 只认看 PR / 仓库 / issue 的那几个子命令（code-review 审 PR 时每个审查员都要跑
/// `gh pr diff`）。`gh api` 能发任意请求，`checkout` / `merge` / `comment` 之类会改东西，
/// 都不在里面；`--web` 会开浏览器，也不算。
fn gh_is_read_only(args: &[String]) -> bool {
    if args
        .iter()
        .any(|a| a == "--web" || a == "-w" || a.starts_with("--web="))
    {
        return false;
    }
    let sub: Vec<&str> = args
        .iter()
        .map(String::as_str)
        .filter(|a| !a.starts_with('-'))
        .take(2)
        .collect();
    matches!(
        sub.as_slice(),
        ["pr", "view" | "diff" | "list" | "checks" | "status"]
            | ["repo", "view"]
            | ["issue", "view" | "list"]
    ) || matches!(args.first().map(String::as_str), Some("--version"))
}

fn git_is_read_only(args: &[String]) -> bool {
    // 全局参数（`-C dir`、`--no-pager`）先跳过，找到子命令。
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-C" | "-c" => i += 2,
            a if a.starts_with('-') && !matches!(a, "--version") => i += 1,
            _ => break,
        }
    }
    // `-c key=value` 能临时改配置（比如 `core.pager`、别名），不放行。
    if args.iter().any(|a| a == "-c") {
        return false;
    }
    let Some(sub) = args.get(i).map(String::as_str) else {
        return false;
    };
    let rest = &args[i + 1..];
    let only_flags = |allowed: &[&str]| rest.iter().all(|a| allowed.contains(&a.as_str()));
    match sub {
        s if GIT_READ.contains(&s) => {
            // `git diff --output=file` 会写文件。
            !rest.iter().any(|a| a.starts_with("--output"))
        }
        "branch" => only_flags(&[
            "-a",
            "-r",
            "-v",
            "-vv",
            "--all",
            "--remotes",
            "--list",
            "--show-current",
            "--merged",
            "--no-merged",
            "--contains",
        ]),
        "tag" => rest.is_empty() || only_flags(&["-l", "--list", "-n"]),
        "remote" => rest.is_empty() || only_flags(&["-v", "--verbose"]),
        "stash" => matches!(rest.first().map(String::as_str), Some("list" | "show")),
        "config" => matches!(
            rest.first().map(String::as_str),
            Some("--get" | "--get-all" | "--list" | "-l" | "--get-regexp")
        ),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reading_and_searching_pass() {
        for cmd in [
            "ls -la",
            "cat src/main.rs | head -50",
            "rg -n 'fn main' --type rust",
            "grep -rn TODO src 2>/dev/null | wc -l",
            "find . -name '*.rs' -maxdepth 3",
            "git status -sb && git log --oneline -20",
            "git -C ../other diff HEAD~1 --stat",
            "git branch -a",
            "cargo metadata --format-version 1 | jq '.packages | length'",
            "node --version",
            "cd src && ls",
            "sed -n '10,20p' README.md",
        ] {
            assert!(is_read_only_command(cmd), "{cmd}");
        }
    }

    /// 回归：转义过的引号不是引号。以前 `echo \"a | rm -rf x \"` 被判成只读（`| rm` 被当成
    /// 引号里的字），shell 实际会跑 `rm`——只读子代理、计划模式里不问用户就能跑任意命令。
    #[test]
    fn escaped_quotes_do_not_hide_separators_or_redirects() {
        for cmd in [
            r#"echo \"a | rm -rf x \""#,
            r#"echo \'a ; touch pwned \'"#,
            r#"echo \"a && rm -rf x \""#,
            r#"echo \"> /tmp/pwned"#,
            // `\\` 是一个字面反斜杠，引号在它后面就闭合了，`| rm` 是真管道。
            r#"cat "a\\" | rm -rf x"#,
            r#"echo "unterminated | rm -rf x"#,
            r#"echo 'unterminated ; rm -rf x"#,
        ] {
            assert!(!is_read_only_command(cmd), "{cmd}");
        }
    }

    /// 引号里的 `>` `<` `&` `|` 只是字面字符：grep 的模式里常有 `=>`、泛型、`a|b`。
    #[test]
    fn quoted_operators_are_just_text() {
        for cmd in [
            r#"git show abc123:src/a.rs | grep -n "running\" =>\|persisted""#,
            r#"git show abc123:src/a.rs | grep -n "Arc<Mutex<Option<PathBuf>>>""#,
            "grep -n 'a > b' src/x.rs",
            r#"rg "fn \w+\(&self\)" src"#,
            "grep -rn 'a|b' .",
            r#"echo "x \" y" | head -1"#,
            // 双引号里转义的 `"` 不闭合引号：整串是 cat 的一个参数，没有管道。
            r#"cat "a\" | rm -rf x""#,
        ] {
            assert!(is_read_only_command(cmd), "{cmd}");
        }
        // 双引号里的命令替换照样会展开：仍然要问。
        assert!(!is_read_only_command(r#"echo "$(rm -rf x)""#));
        assert!(!is_read_only_command("echo \"`rm -rf x`\""));
    }

    /// code-review 审 PR：每个只读审查员都要跑 `gh pr diff`，不该每次都停下等批准。
    #[test]
    fn gh_reads_pass_but_anything_else_asks() {
        for cmd in [
            "gh pr diff 42",
            "gh pr view 42 --json title,body",
            "gh pr checks 42",
            "gh pr list --state open",
            "gh pr status",
            "gh repo view",
            "gh issue view 7",
            "gh issue list --label bug",
            "gh pr diff 42 --repo P0lar1ght/dock",
            "gh --version",
        ] {
            assert!(is_read_only_command(cmd), "{cmd}");
        }
        for cmd in [
            "gh pr merge 42",
            "gh pr comment 42 --body hi",
            "gh pr checkout 42",
            "gh pr review 42 --approve",
            "gh pr view 42 --web",
            "gh pr diff 42 -w",
            "gh api repos/x/y/pulls -f title=t",
            "gh api repos/x/y",
            "gh repo clone x/y",
            "gh issue create",
            "gh pr diff 42 > patch.diff",
            "gh auth token",
        ] {
            assert!(!is_read_only_command(cmd), "{cmd}");
        }
    }

    #[test]
    fn anything_that_can_write_or_run_arbitrary_code_asks() {
        for cmd in [
            "",
            "rm -rf target",
            "echo hi > notes.md",
            "cat a >> b",
            "ls; rm x",
            "ls && touch x",
            "find . -name '*.tmp' -delete",
            "find . -exec rm {} \\;",
            "git commit -m wip",
            "git branch -D old",
            "git checkout main",
            "git -c alias.st='!rm -rf .' st",
            "git diff --output=patch.diff",
            "git stash",
            "sed -i 's/a/b/' f",
            "sed 's/a/b/w out' f",
            "cargo build",
            "npm install",
            "python script.py",
            "cat $(which foo)",
            "echo `id`",
            "FOO=1 ls",
            "sleep 100 &",
            "diff <(ls a) <(ls b)",
            "bash -c 'ls'",
            "xargs rm < list",
        ] {
            assert!(!is_read_only_command(cmd), "{cmd}");
        }
    }
}
