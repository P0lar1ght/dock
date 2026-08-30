//! Interval token parser copied from grok `slash/commands/loop_cmd.rs`.

#![allow(dead_code)] // Grok-copied API kept for later wiring.

/// Split `/loop` args into an optional leading compact interval token and the prompt.
pub fn parse_loop_args(args: &str) -> (Option<&str>, &str) {
    let trimmed = args.trim();
    if let Some(space) = trimmed.find(char::is_whitespace) {
        let first = &trimmed[..space];
        let rest = trimmed[space..].trim_start();
        if is_interval_token(first) && !rest.is_empty() {
            return (Some(first), rest);
        }
    }
    (None, trimmed)
}

pub fn is_interval_token(s: &str) -> bool {
    if s.len() < 2 {
        return false;
    }
    let (digits, suffix) = s.split_at(s.len() - 1);
    matches!(suffix, "s" | "m" | "h" | "d")
        && digits.chars().all(|c| c.is_ascii_digit())
        && digits.parse::<u64>().is_ok_and(|n| n > 0)
}

pub fn interval_to_human(token: &str) -> String {
    let (digits, suffix) = token.split_at(token.len() - 1);
    let n: u64 = digits.parse().unwrap_or(0);
    match suffix {
        "s" => {
            if n <= 1 {
                "每 1 秒".into()
            } else {
                format!("每 {n} 秒")
            }
        }
        "m" => {
            if n == 1 {
                "每 1 分钟".into()
            } else {
                format!("每 {n} 分钟")
            }
        }
        "h" => {
            if n == 1 {
                "每 1 小时".into()
            } else {
                format!("每 {n} 小时")
            }
        }
        "d" => {
            if n == 1 {
                "每 1 天".into()
            } else {
                format!("每 {n} 天")
            }
        }
        _ => format!("每 {token}"),
    }
}

pub fn token_to_duration(token: &str) -> Option<std::time::Duration> {
    if !is_interval_token(token) {
        return None;
    }
    let (digits, suffix) = token.split_at(token.len() - 1);
    let n: u64 = digits.parse().ok()?;
    Some(match suffix {
        "s" => std::time::Duration::from_secs(n),
        "m" => std::time::Duration::from_secs(n * 60),
        "h" => std::time::Duration::from_secs(n * 3600),
        "d" => std::time::Duration::from_secs(n * 86400),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_interval_and_prompt() {
        let (tok, prompt) = parse_loop_args("5m check the build");
        assert_eq!(tok, Some("5m"));
        assert_eq!(prompt, "check the build");
    }

    #[test]
    fn no_interval_is_all_prompt() {
        let (tok, prompt) = parse_loop_args("just a prompt");
        assert!(tok.is_none());
        assert_eq!(prompt, "just a prompt");
    }
}
