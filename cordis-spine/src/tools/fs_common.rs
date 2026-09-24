//! Shared path resolve + JSON arg field parsing for workspace file tools.

use std::path::PathBuf;

use serde_json::Value;

pub(crate) fn parse_args(raw: &str) -> Value {
    serde_json::from_str(raw).unwrap_or(Value::Null)
}

pub(crate) fn str_field(v: &Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(s) = v.get(*key).and_then(|x| x.as_str()) {
            if !s.is_empty() {
                return Some(s.to_string());
            }
        }
    }
    None
}

pub(crate) fn int_field(v: &Value, key: &str) -> Option<i64> {
    v.get(key).and_then(|x| {
        x.as_i64()
            .or_else(|| x.as_u64().map(|n| n as i64))
            .or_else(|| x.as_str().and_then(|s| s.parse().ok()))
    })
}

pub(crate) fn bool_field(v: &Value, key: &str) -> bool {
    v.get(key)
        .and_then(|x| {
            x.as_bool()
                .or_else(|| x.as_str().map(|s| s.eq_ignore_ascii_case("true")))
        })
        .unwrap_or(false)
}

/// 相对路径按**当前会话**的工作目录展开（[`crate::session::cwd::current_cwd`]），
/// 不按进程 cwd——同一进程里的两页可能在不同项目。
pub(crate) fn resolve(path: &str) -> PathBuf {
    let p = PathBuf::from(path);
    if p.is_absolute() {
        p
    } else {
        crate::session::cwd::current_cwd().join(p)
    }
}
