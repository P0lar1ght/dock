//! Rhai regex helpers over the existing `regex` crate (runtime engine only).
//!
//! Caps: pattern / haystack length, [`RegexBuilder::size_limit`], replace-output
//! length, replace count. `regex_replace` supports limited `$n` / `${n}` / `$$`
//! — **no** fancy-regex.

use regex::RegexBuilder;
use rhai::{Array, Dynamic, Engine, EvalAltResult, ImmutableString, Position};

const MAX_PATTERN_LEN: usize = 4 * 1024;
const MAX_HAYSTACK_LEN: usize = 1024 * 1024;
const MAX_REGEX_SIZE: usize = 1_048_576;
const MAX_REPLACE_OUTPUT: usize = 1024 * 1024;
const MAX_REPLACE_COUNT: usize = 10_000;

pub(crate) const BUILTINS: &[(&str, &str, &[&str])] = &[
    (
        "regex_is_match",
        "True if `pattern` matches anywhere in `text`. Runtime only. Pattern ≤4KiB; text ≤1MiB; compiled size capped.",
        &["regex_is_match(pattern: String, text: String) -> bool"],
    ),
    (
        "regex_find",
        "First match as String, or () if none. Runtime only.",
        &["regex_find(pattern: String, text: String) -> String | ()"],
    ),
    (
        "regex_captures",
        "First match as array [full, group1, …] (unmatched groups are ()). () if no match. Runtime only.",
        &["regex_captures(pattern: String, text: String) -> Array | ()"],
    ),
    (
        "regex_replace",
        "Replace matches. Replacement supports `$n` / `${n}` (n≤99) and `$$`. Caps: output ≤1MiB, ≤10000 replacements. No fancy-regex. Runtime only.",
        &["regex_replace(pattern: String, text: String, replacement: String) -> String"],
    ),
];

fn err(message: impl Into<String>) -> Box<EvalAltResult> {
    Box::new(EvalAltResult::ErrorRuntime(
        message.into().into(),
        Position::NONE,
    ))
}

fn check_pattern(pattern: &str) -> Result<(), Box<EvalAltResult>> {
    if pattern.is_empty() {
        return Err(err("regex: pattern must not be empty"));
    }
    if pattern.len() > MAX_PATTERN_LEN {
        return Err(err(format!(
            "regex: pattern exceeds {MAX_PATTERN_LEN} bytes (got {})",
            pattern.len()
        )));
    }
    Ok(())
}

fn check_haystack(text: &str) -> Result<(), Box<EvalAltResult>> {
    if text.len() > MAX_HAYSTACK_LEN {
        return Err(err(format!(
            "regex: text exceeds {MAX_HAYSTACK_LEN} bytes (got {})",
            text.len()
        )));
    }
    Ok(())
}

fn compile(pattern: &str) -> Result<regex::Regex, Box<EvalAltResult>> {
    check_pattern(pattern)?;
    RegexBuilder::new(pattern)
        .size_limit(MAX_REGEX_SIZE)
        .build()
        .map_err(|e| err(format!("regex: {e}")))
}

fn expand_replacement(
    template: &str,
    caps: &regex::Captures<'_>,
) -> Result<String, Box<EvalAltResult>> {
    let mut out = String::with_capacity(template.len());
    let bytes = template.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // Copy the run up to the next `$` as a &str slice. Walking byte-by-byte
        // and pushing `b as char` would decode each UTF-8 byte as Latin-1 and
        // shred any non-ASCII replacement (`密钥` → `å¯é¥`).
        let Some(offset) = bytes[i..].iter().position(|b| *b == b'$') else {
            out.push_str(&template[i..]);
            break;
        };
        if offset > 0 {
            out.push_str(&template[i..i + offset]);
            i += offset;
            check_output(&out)?;
            continue;
        }
        // `$` is ASCII, so the byte after it is always a char boundary.
        i += 1;
        if i >= bytes.len() {
            out.push('$');
            break;
        }
        match bytes[i] {
            b'$' => {
                out.push('$');
                i += 1;
            }
            b'{' => {
                i += 1;
                let start = i;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                if i == start || i >= bytes.len() || bytes[i] != b'}' {
                    out.push('$');
                    out.push('{');
                    i = start;
                    check_output(&out)?;
                    continue;
                }
                let idx: usize = template[start..i]
                    .parse()
                    .map_err(|_| err("regex_replace: bad group index"))?;
                if idx > 99 {
                    return Err(err("regex_replace: group index exceeds 99"));
                }
                if let Some(m) = caps.get(idx) {
                    out.push_str(m.as_str());
                }
                i += 1;
            }
            b'0'..=b'9' => {
                let start = i;
                i += 1;
                if i < bytes.len() && bytes[i].is_ascii_digit() {
                    let two: usize = template[start..i + 1].parse().unwrap();
                    if two <= 99 && (caps.get(two).is_some() || two < caps.len()) {
                        if let Some(m) = caps.get(two) {
                            out.push_str(m.as_str());
                        }
                        i += 1;
                        check_output(&out)?;
                        continue;
                    }
                }
                let one = (bytes[start] - b'0') as usize;
                if let Some(m) = caps.get(one) {
                    out.push_str(m.as_str());
                }
            }
            // Not a group reference: emit a literal `$` and let the next pass
            // copy the following run as text (it may be multi-byte).
            _ => out.push('$'),
        }
        check_output(&out)?;
    }
    Ok(out)
}

fn check_output(out: &str) -> Result<(), Box<EvalAltResult>> {
    if out.len() > MAX_REPLACE_OUTPUT {
        return Err(err(format!(
            "regex_replace: output exceeds {MAX_REPLACE_OUTPUT} bytes"
        )));
    }
    Ok(())
}

pub(crate) fn register(engine: &mut Engine) {
    engine.register_fn(
        "regex_is_match",
        |pattern: ImmutableString, text: ImmutableString| -> Result<bool, Box<EvalAltResult>> {
            check_haystack(&text)?;
            Ok(compile(&pattern)?.is_match(&text))
        },
    );
    engine.register_fn(
        "regex_find",
        |pattern: ImmutableString, text: ImmutableString| -> Result<Dynamic, Box<EvalAltResult>> {
            check_haystack(&text)?;
            Ok(match compile(&pattern)?.find(&text) {
                Some(m) => Dynamic::from(m.as_str().to_string()),
                None => Dynamic::UNIT,
            })
        },
    );
    engine.register_fn(
        "regex_captures",
        |pattern: ImmutableString, text: ImmutableString| -> Result<Dynamic, Box<EvalAltResult>> {
            check_haystack(&text)?;
            let Some(caps) = compile(&pattern)?.captures(&text) else {
                return Ok(Dynamic::UNIT);
            };
            let mut arr = Array::with_capacity(caps.len());
            for idx in 0..caps.len() {
                match caps.get(idx) {
                    Some(m) => arr.push(Dynamic::from(m.as_str().to_string())),
                    None => arr.push(Dynamic::UNIT),
                }
            }
            Ok(Dynamic::from(arr))
        },
    );
    engine.register_fn(
        "regex_replace",
        |pattern: ImmutableString,
         text: ImmutableString,
         replacement: ImmutableString|
         -> Result<String, Box<EvalAltResult>> {
            check_haystack(&text)?;
            if replacement.len() > MAX_HAYSTACK_LEN {
                return Err(err(format!(
                    "regex_replace: replacement exceeds {MAX_HAYSTACK_LEN} bytes"
                )));
            }
            let re = compile(&pattern)?;
            let mut out = String::new();
            let mut last = 0usize;
            let mut count = 0usize;
            for caps in re.captures_iter(&text) {
                count += 1;
                if count > MAX_REPLACE_COUNT {
                    return Err(err(format!(
                        "regex_replace: exceeded {MAX_REPLACE_COUNT} replacements"
                    )));
                }
                let m = caps.get(0).expect("full match");
                out.push_str(&text[last..m.start()]);
                out.push_str(&expand_replacement(&replacement, &caps)?);
                check_output(&out)?;
                last = m.end();
            }
            out.push_str(&text[last..]);
            check_output(&out)?;
            Ok(out)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eng() -> Engine {
        let mut e = Engine::new();
        register(&mut e);
        e
    }

    #[test]
    fn is_match_find_captures_replace() {
        let e = eng();
        assert!(e
            .eval::<bool>(r#"regex_is_match("\\d+", "ab12cd")"#)
            .unwrap());
        assert!(!e.eval::<bool>(r#"regex_is_match("\\d+", "abcd")"#).unwrap());
        assert_eq!(
            e.eval::<String>(r#"regex_find("\\d+", "ab12cd")"#).unwrap(),
            "12"
        );
        let miss: Dynamic = e.eval(r#"regex_find("\\d+", "abcd")"#).unwrap();
        assert!(miss.is_unit());
        let caps: Array = e
            .eval(r#"regex_captures("(\\w+)@(\\w+)", "x a@b y")"#)
            .unwrap();
        assert_eq!(caps.len(), 3);
        assert_eq!(caps[0].clone().into_string().unwrap(), "a@b");
        assert_eq!(
            e.eval::<String>(r#"regex_replace("(\\w+)=(\\w+)", "a=1 b=2", "$2:$1")"#)
                .unwrap(),
            "1:a 2:b"
        );
        assert_eq!(
            e.eval::<String>(r#"regex_replace("x", "x", "$$")"#)
                .unwrap(),
            "$"
        );
        assert_eq!(
            e.eval::<String>(r#"regex_replace("(a)(b)", "ab", "${2}-${1}")"#)
                .unwrap(),
            "b-a"
        );
    }

    #[test]
    fn rejects_empty_pattern() {
        let e = eng();
        assert!(e.eval::<bool>(r#"regex_is_match("", "a")"#).is_err());
    }

    /// Regression: the replacement template used to be walked byte-by-byte with
    /// `b as char`, turning every multi-byte char into Latin-1 mojibake. Masking
    /// with a Chinese literal is the headline use, so it must survive verbatim.
    #[test]
    fn non_ascii_replacement_survives() {
        let e = eng();
        assert_eq!(
            e.eval::<String>(r#"regex_replace("token", "my token here", "密钥")"#)
                .unwrap(),
            "my 密钥 here"
        );
        assert_eq!(
            e.eval::<String>(r#"regex_replace("(\\w+)", "abc", "【$1】")"#)
                .unwrap(),
            "【abc】"
        );
        // A `$` that is not a group reference stays literal, and the multi-byte
        // run right after it is copied as text rather than re-decoded.
        assert_eq!(
            e.eval::<String>(r#"regex_replace("x", "x", "$中$$元")"#)
                .unwrap(),
            "$中$元"
        );
        // Non-ASCII in the haystack and in captured groups round-trips too.
        assert_eq!(
            e.eval::<String>(r#"regex_replace("(\\S+)=(\\S+)", "名字=张三", "$2←$1")"#)
                .unwrap(),
            "张三←名字"
        );
    }

    #[test]
    fn trailing_dollar_and_unclosed_brace_stay_literal() {
        let e = eng();
        assert_eq!(
            e.eval::<String>(r#"regex_replace("x", "x", "a$")"#)
                .unwrap(),
            "a$"
        );
        assert_eq!(
            e.eval::<String>(r#"regex_replace("x", "x", "${}")"#)
                .unwrap(),
            "${}"
        );
        assert_eq!(
            e.eval::<String>(r#"regex_replace("x", "x", "${1")"#)
                .unwrap(),
            "${1"
        );
    }
}
