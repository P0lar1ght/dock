//! Absolute wall-clock helpers for Rhai scripts.
//!
//! Rhai's built-in `timestamp()` is `std::time::Instant` (monotonic / relative) —
//! useless for API signing (SigV4, Aliyun, etc.). These return UTC epoch times via
//! `std::time::SystemTime` only (no chrono).
//!
//! Registered **only on the run engine** in [`super::rhai_host::apply_rhai`]
//! (same place as `rhai_http` / `rhai_codec`), not on define-time preflight.

use std::time::{SystemTime, UNIX_EPOCH};

use rhai::{Engine, EvalAltResult};

pub(crate) const BUILTINS: &[(&str, &str, &[&str])] = &[
    (
        "unix_time",
        "UTC seconds since Unix epoch (SystemTime). Runtime only. Prefer over timestamp() (Instant) for API signing.",
        &["unix_time() -> i64"],
    ),
    (
        "unix_time_ms",
        "UTC milliseconds since Unix epoch (SystemTime). Runtime only.",
        &["unix_time_ms() -> i64"],
    ),
];

fn err(message: impl Into<String>) -> Box<EvalAltResult> {
    Box::new(EvalAltResult::ErrorRuntime(
        message.into().into(),
        rhai::Position::NONE,
    ))
}

fn since_epoch() -> Result<std::time::Duration, Box<EvalAltResult>> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| err(format!("system clock before Unix epoch: {e}")))
}

fn unix_time() -> Result<i64, Box<EvalAltResult>> {
    let d = since_epoch()?;
    i64::try_from(d.as_secs()).map_err(|_| err("unix_time: seconds overflow i64"))
}

fn unix_time_ms() -> Result<i64, Box<EvalAltResult>> {
    let d = since_epoch()?;
    i64::try_from(d.as_millis()).map_err(|_| err("unix_time_ms: milliseconds overflow i64"))
}

/// Register wall-clock builtins on a run-time engine.
pub fn register(engine: &mut Engine) {
    engine.register_fn("unix_time", unix_time);
    engine.register_fn("unix_time_ms", unix_time_ms);
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
    fn unix_time_in_sane_range() {
        let e = eng();
        let t: i64 = e.eval("unix_time()").unwrap();
        assert!(t > 1_700_000_000, "too early (before ~2023): {t}");
        assert!(t < 2_100_000_000, "too late (after ~2036): {t}");
    }

    #[test]
    fn unix_time_ms_matches_seconds() {
        let e = eng();
        let secs: i64 = e.eval("unix_time()").unwrap();
        let ms: i64 = e.eval("unix_time_ms()").unwrap();
        let from_ms = ms / 1000;
        let delta = (from_ms - secs).abs();
        assert!(
            delta <= 2,
            "unix_time_ms()/1000 ({from_ms}) vs unix_time() ({secs}) delta={delta}"
        );
        assert!(ms > 1_700_000_000_000, "ms too early: {ms}");
        assert!(ms < 2_100_000_000_000, "ms too late: {ms}");
    }
}
