//! 沙箱里的命令拿不到的环境变量。
//!
//! 默认表抄 Grok（`EnvPolicy::default_excludes`）：名字里带 KEY / SECRET / TOKEN / PASSWORD /
//! CREDENTIAL 的、`*_PAT`、动态库注入；再加上几个借沙箱外进程办事的入口（ssh-agent、
//! 会话总线、askpass）。Dock 进程自己的模型密钥常在环境变量里，不去掉就等于交给每条命令。

use std::ffi::OsString;

const SCRUB: &[&str] = &[
    "*KEY*",
    "*SECRET*",
    "*TOKEN*",
    "*PASSWORD*",
    "*PASSWD*",
    "*CREDENTIAL*",
    "*_PAT",
    "*ASKPASS*",
    "LD_PRELOAD",
    "LD_LIBRARY_PATH",
    "DYLD_*",
    "SSH_AUTH_SOCK",
    "DBUS_SESSION_BUS_ADDRESS",
];

/// `names` 里要去掉的那些；`keep` 里点名的留下（精确匹配，不分大小写）。
pub(crate) fn scrubbed(
    names: impl IntoIterator<Item = OsString>,
    keep: &[String],
) -> Vec<OsString> {
    names
        .into_iter()
        .filter(|name| {
            let Some(name) = name.to_str() else {
                return false;
            };
            !keep.iter().any(|k| k.eq_ignore_ascii_case(name))
                && SCRUB.iter().any(|glob| glob_match(glob, name))
        })
        .collect()
}

/// 只认 `*` 的通配，ASCII 不分大小写。
fn glob_match(pattern: &str, name: &str) -> bool {
    let pattern = pattern.to_ascii_uppercase();
    let name = name.to_ascii_uppercase();
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == name;
    }
    let mut rest = name.as_str();
    let first = parts[0];
    if !rest.starts_with(first) {
        return false;
    }
    rest = &rest[first.len()..];
    let last = parts[parts.len() - 1];
    for mid in &parts[1..parts.len() - 1] {
        match rest.find(mid) {
            Some(at) => rest = &rest[at + mid.len()..],
            None => return false,
        }
    }
    rest.ends_with(last)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    #[test]
    fn scrubs_secrets_and_keeps_the_rest() {
        let out = scrubbed(
            names(&[
                "PATH",
                "HOME",
                "OPENAI_API_KEY",
                "GITHUB_TOKEN",
                "AWS_SECRET_ACCESS_KEY",
                "DB_PASSWORD",
                "GITHUB_PAT",
                "DYLD_INSERT_LIBRARIES",
                "SSH_AUTH_SOCK",
                "LANG",
            ]),
            &[],
        );
        assert_eq!(
            out,
            names(&[
                "OPENAI_API_KEY",
                "GITHUB_TOKEN",
                "AWS_SECRET_ACCESS_KEY",
                "DB_PASSWORD",
                "GITHUB_PAT",
                "DYLD_INSERT_LIBRARIES",
                "SSH_AUTH_SOCK",
            ])
        );
    }

    #[test]
    fn keep_overrides_the_table() {
        let out = scrubbed(
            names(&["OPENAI_API_KEY", "HF_TOKEN"]),
            &["openai_api_key".into()],
        );
        assert_eq!(out, names(&["HF_TOKEN"]));
    }

    #[test]
    fn glob_handles_prefix_suffix_and_middle() {
        assert!(glob_match("*KEY*", "api_key_x"));
        assert!(glob_match("DYLD_*", "DYLD_LIBRARY_PATH"));
        assert!(glob_match("*_PAT", "GH_PAT"));
        assert!(!glob_match("*_PAT", "PATH"));
        assert!(!glob_match("LD_PRELOAD", "LD_PRELOADX"));
    }
}
