//! 沙箱的策略规则：按出网目的地（主机）或命令匹配，命中了放行、询问或阻断。按顺序第一条
//! 命中的算；都不中就放行（默认放行：沙箱一开不上锁，要限制时再加规则）。
//!
//! 规则从哪来（配置文件、这一页临时授权的）由调用方决定，这里只管匹配。

/// 命中之后怎么办。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Allow,
    Ask,
    Deny,
}

/// 一条规则：`host` 或 `command` 二选一（都是 `*` 通配）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    pub host: Option<String>,
    pub command: Option<String>,
    pub action: Action,
}

/// `host` 命中的第一条规则（大小写不敏感；`*.example.com` 也匹配 `example.com` 本身）。
pub fn for_host<'a>(rules: &'a [Rule], host: &str) -> Option<&'a Rule> {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    rules.iter().find(|r| {
        r.host.as_deref().is_some_and(|p| {
            let p = p.trim().to_ascii_lowercase();
            glob(&p, &host) || p.strip_prefix("*.").is_some_and(|bare| bare == host)
        })
    })
}

/// `command` 命中的第一条规则（整条命令匹配，首尾空白不算）。
pub fn for_command<'a>(rules: &'a [Rule], command: &str) -> Option<&'a Rule> {
    let command = command.trim();
    rules.iter().find(|r| {
        r.command
            .as_deref()
            .is_some_and(|p| glob(p.trim(), command))
    })
}

/// `*` 匹配任意一段（含空），其余逐字。
pub fn glob(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        if pi < p.len() && p[pi] == '*' {
            star = Some((pi, ti));
            pi += 1;
        } else if pi < p.len() && p[pi] == t[ti] {
            pi += 1;
            ti += 1;
        } else if let Some((sp, st)) = star {
            pi = sp + 1;
            ti = st + 1;
            star = Some((sp, st + 1));
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|c| *c == '*')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(p: &str, action: Action) -> Rule {
        Rule {
            host: Some(p.into()),
            command: None,
            action,
        }
    }
    fn cmd(p: &str, action: Action) -> Rule {
        Rule {
            host: None,
            command: Some(p.into()),
            action,
        }
    }

    #[test]
    fn globs() {
        assert!(glob("*", ""));
        assert!(glob("git push*", "git push --force origin main"));
        assert!(!glob("git push*", "git pull"));
        assert!(glob("*rm -rf*", "cd x && rm -rf build"));
        assert!(glob("a*b*c", "aXXbYYc"));
        assert!(!glob("a*b*c", "aXXbYY"));
    }

    #[test]
    fn first_matching_rule_wins_and_default_is_none() {
        let rules = vec![
            host("api.corp.example", Action::Allow),
            host("*.corp.example", Action::Deny),
            cmd("git push*", Action::Ask),
        ];
        assert_eq!(
            for_host(&rules, "API.corp.example.").unwrap().action,
            Action::Allow
        );
        assert_eq!(
            for_host(&rules, "git.corp.example").unwrap().action,
            Action::Deny
        );
        assert_eq!(
            for_host(&rules, "corp.example").unwrap().action,
            Action::Deny
        );
        assert!(for_host(&rules, "github.com").is_none(), "都不中就是放行");
        assert_eq!(
            for_command(&rules, "  git push origin x ").unwrap().action,
            Action::Ask
        );
        assert!(for_command(&rules, "git status").is_none());
    }
}
