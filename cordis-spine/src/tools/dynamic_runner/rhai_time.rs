//! Absolute wall-clock helpers for Rhai scripts.
//!
//! Rhai's built-in `timestamp()` is `std::time::Instant` (monotonic / relative) —
//! useless for API signing (SigV4, Aliyun, etc.). These return UTC epoch / civil
//! times via `std::time::SystemTime` only (no chrono).
//!
//! Registered **only on the run engine** in [`super::rhai_host::apply_rhai`]
//! (same place as `rhai_http` / `rhai_codec`), not on define-time preflight.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rhai::{Dynamic, Engine, EvalAltResult, Map};

pub(crate) const BUILTINS: &[(&str, &str, &[&str])] = &[
    (
        "utc_now",
        "One SystemTime snapshot as a map: secs/ms (epoch), date (YYYY-MM-DD), rfc3339, year/month/day/hour/minute/second. Runtime only.",
        &["utc_now() -> Map"],
    ),
    (
        "unix_time",
        "UTC seconds since Unix epoch (SystemTime). Runtime only. Prefer over timestamp() (Instant) for API signing. Thin wrapper over utc_now().secs.",
        &["unix_time() -> i64"],
    ),
    (
        "unix_time_ms",
        "UTC milliseconds since Unix epoch (SystemTime). Runtime only. Thin wrapper over utc_now().ms.",
        &["unix_time_ms() -> i64"],
    ),
    (
        "utc_date",
        "UTC civil date YYYY-MM-DD (zero-padded). Runtime only. Sugar for utc_now().date.",
        &["utc_date() -> String"],
    ),
    (
        "now_date",
        "Alias of utc_date(): UTC civil date YYYY-MM-DD. Runtime only.",
        &["now_date() -> String"],
    ),
];

fn err(message: impl Into<String>) -> Box<EvalAltResult> {
    Box::new(EvalAltResult::ErrorRuntime(
        message.into().into(),
        rhai::Position::NONE,
    ))
}

/// UTC civil + clock parts derived from one duration since the Unix epoch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UtcParts {
    pub secs: i64,
    pub ms: i64,
    pub year: i64,
    pub month: i64,
    pub day: i64,
    pub hour: i64,
    pub minute: i64,
    pub second: i64,
}

impl UtcParts {
    pub fn date(&self) -> String {
        format!("{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }

    pub fn rfc3339(&self) -> String {
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
            self.year, self.month, self.day, self.hour, self.minute, self.second
        )
    }

    fn to_map(&self) -> Map {
        let mut m = Map::new();
        m.insert("secs".into(), Dynamic::from(self.secs));
        m.insert("ms".into(), Dynamic::from(self.ms));
        m.insert("date".into(), Dynamic::from(self.date()));
        m.insert("rfc3339".into(), Dynamic::from(self.rfc3339()));
        m.insert("year".into(), Dynamic::from(self.year));
        m.insert("month".into(), Dynamic::from(self.month));
        m.insert("day".into(), Dynamic::from(self.day));
        m.insert("hour".into(), Dynamic::from(self.hour));
        m.insert("minute".into(), Dynamic::from(self.minute));
        m.insert("second".into(), Dynamic::from(self.second));
        m
    }
}

/// Days since Unix epoch → `(year, month, day)` UTC.
/// Howard Hinnant `civil_from_days` (public domain / CC0).
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let mut y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    if m <= 2 {
        y += 1;
    }
    (y, m as i64, d as i64)
}

/// Testable breakdown from whole UTC seconds since epoch.
#[cfg(test)]
pub(crate) fn from_secs(secs: u64) -> UtcParts {
    from_duration(Duration::from_secs(secs))
}

fn from_duration(d: Duration) -> UtcParts {
    // Duration since UNIX_EPOCH is always non-negative here.
    let secs_u = d.as_secs();
    let ms_u = d.as_millis();
    // Clamp to i64 range for Rhai; real wall clocks stay well inside.
    let secs = i64::try_from(secs_u).unwrap_or(i64::MAX);
    let ms = i64::try_from(ms_u).unwrap_or(i64::MAX);

    let days = (secs_u / 86_400) as i64;
    let sod = (secs_u % 86_400) as i64;
    let hour = sod / 3600;
    let minute = (sod % 3600) / 60;
    let second = sod % 60;
    let (year, month, day) = civil_from_days(days);

    UtcParts {
        secs,
        ms,
        year,
        month,
        day,
        hour,
        minute,
        second,
    }
}

fn since_epoch() -> Result<Duration, Box<EvalAltResult>> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| err(format!("system clock before Unix epoch: {e}")))
}

fn snapshot() -> Result<UtcParts, Box<EvalAltResult>> {
    Ok(from_duration(since_epoch()?))
}

fn utc_now() -> Result<Map, Box<EvalAltResult>> {
    Ok(snapshot()?.to_map())
}

fn unix_time() -> Result<i64, Box<EvalAltResult>> {
    Ok(snapshot()?.secs)
}

fn unix_time_ms() -> Result<i64, Box<EvalAltResult>> {
    Ok(snapshot()?.ms)
}

fn utc_date() -> Result<String, Box<EvalAltResult>> {
    Ok(snapshot()?.date())
}

fn now_date() -> Result<String, Box<EvalAltResult>> {
    utc_date()
}

/// Register wall-clock builtins on a run-time engine.
pub fn register(engine: &mut Engine) {
    engine.register_fn("utc_now", utc_now);
    engine.register_fn("unix_time", unix_time);
    engine.register_fn("unix_time_ms", unix_time_ms);
    engine.register_fn("utc_date", utc_date);
    engine.register_fn("now_date", now_date);
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
    fn from_secs_epoch_day_zero() {
        let p = from_secs(0);
        assert_eq!(p.date(), "1970-01-01");
        assert_eq!(p.rfc3339(), "1970-01-01T00:00:00Z");
        assert_eq!((p.year, p.month, p.day), (1970, 1, 1));
        assert_eq!((p.hour, p.minute, p.second), (0, 0, 0));
        assert_eq!(p.secs, 0);
        assert_eq!(p.ms, 0);
    }

    #[test]
    fn from_secs_next_day() {
        let p = from_secs(86_400);
        assert_eq!(p.date(), "1970-01-02");
        assert_eq!(p.rfc3339(), "1970-01-02T00:00:00Z");
    }

    #[test]
    fn from_secs_leap_day() {
        // 2020-02-29 00:00:00 UTC
        let p = from_secs(1_582_934_400);
        assert_eq!(p.date(), "2020-02-29");
        assert_eq!((p.year, p.month, p.day), (2020, 2, 29));
        assert_eq!(p.rfc3339(), "2020-02-29T00:00:00Z");
    }

    #[test]
    fn from_secs_leap_day_afternoon() {
        // 2020-02-29 12:34:56 UTC
        let p = from_secs(1_582_934_400 + 12 * 3600 + 34 * 60 + 56);
        assert_eq!(p.date(), "2020-02-29");
        assert_eq!(p.rfc3339(), "2020-02-29T12:34:56Z");
        assert_eq!((p.hour, p.minute, p.second), (12, 34, 56));
    }

    #[test]
    fn zero_pads_month_and_day() {
        // 1970-03-05
        let p = from_secs(5_443_200);
        assert_eq!(p.date(), "1970-03-05");
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

    #[test]
    fn utc_now_map_fields_and_consistency() {
        let e = eng();
        let m: Map = e.eval("utc_now()").unwrap();
        let secs = m.get("secs").unwrap().as_int().unwrap();
        let ms = m.get("ms").unwrap().as_int().unwrap();
        let date = m.get("date").unwrap().clone().into_string().unwrap();
        let rfc = m.get("rfc3339").unwrap().clone().into_string().unwrap();
        let year = m.get("year").unwrap().as_int().unwrap();
        let month = m.get("month").unwrap().as_int().unwrap();
        let day = m.get("day").unwrap().as_int().unwrap();

        assert!(secs > 1_700_000_000 && secs < 2_100_000_000, "secs={secs}");
        assert!((ms / 1000 - secs).abs() <= 1, "ms={ms} secs={secs}");
        assert!(
            date.chars().count() == 10 && date.as_bytes()[4] == b'-' && date.as_bytes()[7] == b'-',
            "date shape: {date}"
        );
        assert!(rfc.starts_with(&date) && rfc.ends_with('Z'), "rfc={rfc}");
        let expected = from_secs(secs as u64);
        assert_eq!(date, expected.date());
        assert_eq!(
            (year, month, day),
            (expected.year, expected.month, expected.day)
        );
    }

    #[test]
    fn utc_date_and_now_date_match_utc_now() {
        let e = eng();
        let via_now: String = e.eval("utc_now().date").unwrap();
        let via_utc: String = e.eval("utc_date()").unwrap();
        let via_alias: String = e.eval("now_date()").unwrap();
        // Separate snapshots may cross midnight; shape + agreement of sugar aliases.
        assert_eq!(via_utc, via_alias);
        assert_eq!(via_utc.len(), 10);
        assert_eq!(via_now.len(), 10);
        // Same-day almost always; if flaky across midnight, still YYYY-MM-DD.
        assert_eq!(&via_utc[4..5], "-");
        assert_eq!(&via_utc[7..8], "-");
    }
}
