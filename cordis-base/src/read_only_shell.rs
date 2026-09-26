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
    if has_unsafe_syntax(command) {
        return false;
    }
    split_segments(command)
        .iter()
        .all(|seg| segment_is_read_only(seg))
}

/// 命令替换、进程替换、后台、往文件写的重定向。引号里的也算：宁可多问。
fn has_unsafe_syntax(command: &str) -> bool {
    if command.contains('`')
        || command.contains("$(")
        || command.contains("<(")
        || command.contains(">(")
    {
        return true;
    }
    let chars: Vec<char> = command.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        match c {
            // 单个 `&` 是后台；`&&` 是连接符，`>&` / `&>` 交给下面的重定向判断。
            '&' => {
                let prev = i.checked_sub(1).map(|j| chars[j]);
                let next = chars.get(i + 1).copied();
                if prev != Some('&') && next != Some('&') && prev != Some('>') && next != Some('>')
                {
                    return true;
                }
            }
            '>' if !harmless_redirect(&chars[i..]) => return true,
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

fn split_segments(command: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut chars = command.chars().peekable();
    // 引号里的 `|` `;` 不是分隔符（`jq '.a | length'`）。
    let mut quote: Option<char> = None;
    while let Some(c) = chars.next() {
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
            cur.push(c);
            continue;
        }
        match c {
            '\'' | '"' => {
                quote = Some(c);
                cur.push(c);
            }
            '|' | ';' | '\n' => {
                if c == '|' && chars.peek() == Some(&'|') {
                    chars.next();
                }
                out.push(std::mem::take(&mut cur));
            }
            '&' if chars.peek() == Some(&'&') => {
                chars.next();
                out.push(std::mem::take(&mut cur));
            }
            _ => cur.push(c),
        }
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
