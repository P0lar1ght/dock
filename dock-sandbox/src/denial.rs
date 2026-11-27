//! 一条沙箱里跑失败的命令，像不像是被沙箱拦下的。
//!
//! 没有确定的办法（macOS 上 Seatbelt 的拒绝在本机统一日志里查不到，Linux 的 EPERM 也
//! 不带来源），所以和 Codex（`is_likely_sandbox_denied`）一样看退出码和输出里的关键词。
//! 只用来给模型一句提示，误判的代价是多一句话。

/// 输出里出现就算像：权限类错误。
const DENIAL_WORDS: &[&str] = &[
    "operation not permitted",
    "permission denied",
    "read-only file system",
    "sandbox",
    "eperm",
    "eacces",
    "erofs",
];

/// 断网时再加上解析 / 连接失败。
const NETWORK_WORDS: &[&str] = &[
    "could not resolve",
    "name or service not known",
    "nodename nor servname",
    "temporary failure in name resolution",
    "network is unreachable",
    "failed to lookup address",
    "dns error",
    "getaddrinfo",
];

/// `exit_code`：`None` = 被信号结束。`network_off`：这条命令的策略断了网。
pub fn looks_like_denial(exit_code: Option<i32>, output: &str, network_off: bool) -> bool {
    // 0 是成功；127 是找不到命令，和沙箱无关。
    if matches!(exit_code, Some(0) | Some(127)) {
        return false;
    }
    let lower = output.to_lowercase();
    DENIAL_WORDS.iter().any(|w| lower.contains(w))
        || (network_off && NETWORK_WORDS.iter().any(|w| lower.contains(w)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_errors_look_like_denials() {
        assert!(looks_like_denial(
            Some(1),
            "touch: /etc/x: Operation not permitted",
            false
        ));
        assert!(looks_like_denial(
            Some(1),
            "sh: 1: cannot create /ws/.git/hooks/x: Read-only file system",
            false
        ));
    }

    #[test]
    fn success_and_missing_commands_do_not() {
        assert!(!looks_like_denial(Some(0), "permission denied", false));
        assert!(!looks_like_denial(Some(127), "permission denied", false));
        assert!(!looks_like_denial(
            Some(1),
            "error[E0308]: mismatched types",
            false
        ));
    }

    #[test]
    fn network_words_only_count_when_the_network_is_off() {
        let out = "curl: (6) Could not resolve host: example.com";
        assert!(!looks_like_denial(Some(6), out, false));
        assert!(looks_like_denial(Some(6), out, true));
    }
}
