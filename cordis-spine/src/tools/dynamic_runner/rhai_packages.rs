//! Rhai 生态里**只做计算**的包：`rhai-url`（解析 / 改写 URL）和 `rhai-rand`（随机数、
//! 打乱、抽样）。
//!
//! 只挂不碰外部的。`rhai-fs`、`rhai-http`、`rhai-dylib` 这类包会绕开 Dock 的门：文件写入
//! 不过权限门、不进每轮改动记录；HTTP 不按 host 确认、不走 SSRF / 代理策略；dylib 直接
//! 跑原生代码。要这些能力就 `host.call_tool` 调对应的工具（或接一个 MCP 服务器），权限照走。
//!
//! 和 [`super::rhai_codec`] 一样只挂在**运行期**引擎上，define 期的编译检查里没有它们。

use std::sync::{Arc, LazyLock};

use rhai::packages::Package;
use rhai::{Engine, EvalAltResult, Module, Position, INT};

/// `rand_alpha_numeric(n)` 的上限：和脚本字符串的上限（`sandboxed_engine` 的
/// `max_string_size`）一样。引擎是在函数**返回之后**才查字符串大小的，包里的实现会先
/// 分配 `n` 个字符——`rand_alpha_numeric(1_000_000_000_000)` 不拦就是先把进程撑爆。
pub const MAX_RAND_STRING: INT = 1024 * 1024;

// 包只建一次：每次运行插件都会新起一个引擎，模块按 `sync` 特性是 `Arc`，共享就好。
static URL: LazyLock<Arc<Module>> =
    LazyLock::new(|| rhai_url::UrlPackage::new().as_shared_module());
static RAND: LazyLock<Arc<Module>> =
    LazyLock::new(|| rhai_rand::RandomPackage::new().as_shared_module());

pub fn register(engine: &mut Engine) {
    engine.register_global_module(URL.clone());
    engine.register_global_module(RAND.clone());
    // 直接注册在引擎上的函数先于全局模块被找到，盖住包里同签名的那个。
    engine.register_fn("rand_alpha_numeric", rand_alpha_numeric);
}

fn rand_alpha_numeric(len: INT) -> Result<String, Box<EvalAltResult>> {
    use rand::distr::{Alphanumeric, SampleString};
    if !(1..=MAX_RAND_STRING).contains(&len) {
        return Err(Box::new(EvalAltResult::ErrorArithmetic(
            format!("rand_alpha_numeric length must be 1..={MAX_RAND_STRING}, got {len}"),
            Position::NONE,
        )));
    }
    Ok(Alphanumeric.sample_string(&mut rand::rng(), len as usize))
}

pub const BUILTINS: &[(&str, &str, &[&str])] = &[
    (
        "Url (rhai-url)",
        "Parse and edit a URL (the `url` crate). Properties read and write: href, scheme, domain, path, query, fragment / hash. Throws on an unparsable URL. Build query strings with it instead of concatenating url_encode pieces.",
        &[
            "let u = Url(\"https://api.example.com/v1/items?page=2\")",
            "u.domain; u.path; u.query_get(\"page\") -> String; u.query_gets(\"tag\") -> Array",
            "u.query_set(key, value); u.query_append(key, value); u.query_remove(key); u.query_clear()",
            "u.path = \"/v2/items\"; u.href -> String; u.to_string()",
        ],
    ),
    (
        "rand / rand_float / rand_bool / shuffle / sample (rhai-rand)",
        "Random numbers, shuffling and sampling (the `rand` crate, not for secrets). rand_alpha_numeric is capped at 1MiB.",
        &[
            "rand() -> i64; rand(1..=6) / rand(0..10) / rand(start, end) -> i64",
            "rand_float() -> f64 in [0, 1); rand_float(start, end) -> f64",
            "rand_bool() -> bool; rand_bool(probability: f64) -> bool",
            "rand_alpha_numeric(len: i64) -> String",
            "array.shuffle(); array.sample() -> item; array.sample(n) -> Array",
        ],
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> Engine {
        let mut e = super::super::rhai_host::sandboxed_engine(100_000);
        register(&mut e);
        e
    }

    #[test]
    fn urls_parse_and_edit() {
        let e = engine();
        let out: String = e
            .eval(
                r#"let u = Url("https://api.example.com/v1/items?page=2&tag=a&tag=b#top");
                   let tags = u.query_gets("tag");
                   u.query_set("page", "3");
                   u.query_remove("tag");
                   u.path = "/v2/items";
                   `${u.domain}|${tags.len()}|${u.href}`"#,
            )
            .unwrap();
        assert_eq!(
            out,
            "api.example.com|2|https://api.example.com/v2/items?page=3#top"
        );
        assert!(e.eval::<String>(r#"Url("not a url").href"#).is_err());
    }

    #[test]
    fn random_numbers_stay_in_range_and_arrays_shuffle() {
        let e = engine();
        let ok: bool = e
            .eval(
                r#"let all = true;
                   for i in 0..200 {
                       let d = rand(1..=6);
                       let f = rand_float();
                       if d < 1 || d > 6 || f < 0.0 || f >= 1.0 { all = false; }
                   }
                   let a = [1, 2, 3, 4, 5];
                   a.shuffle();
                   let picked = a.sample(2);
                   all && a.len() == 5 && picked.len() == 2 && rand_alpha_numeric(12).len() == 12"#,
            )
            .unwrap();
        assert!(ok);
    }

    /// 包里的 `rand_alpha_numeric` 先分配再交给引擎查大小；超上限的要在分配之前就拒。
    #[test]
    fn huge_random_strings_are_refused_before_allocating() {
        let e = engine();
        for len in ["1_000_000_000_000", "0", "-1"] {
            let err = e
                .eval::<String>(&format!("rand_alpha_numeric({len})"))
                .unwrap_err()
                .to_string();
            assert!(
                err.contains("rand_alpha_numeric length must be"),
                "{len}: {err}"
            );
        }
        let max = e
            .eval::<String>(&format!("rand_alpha_numeric({MAX_RAND_STRING})"))
            .unwrap();
        assert_eq!(max.len() as INT, MAX_RAND_STRING);
    }
}
