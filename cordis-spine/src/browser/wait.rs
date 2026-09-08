//! Bounded wait / retry helpers for CDP actions.

use std::future::Future;
use std::time::Duration;

/// Retry an async op until it succeeds or attempts/time are exhausted.
pub async fn retry_async<T, E, F, Fut>(
    attempts: u32,
    delay: Duration,
    mut op: F,
) -> Result<T, E>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    let attempts = attempts.max(1);
    let mut last_err = None;
    for i in 0..attempts {
        match op().await {
            Ok(v) => return Ok(v),
            Err(e) => {
                last_err = Some(e);
                if i + 1 < attempts {
                    tokio::time::sleep(delay).await;
                }
            }
        }
    }
    Err(last_err.expect("attempts >= 1"))
}

/// Cap a future with a timeout; map timeout to `on_timeout`.
pub async fn with_timeout<T, E, Fut, F>(
    timeout: Duration,
    fut: Fut,
    on_timeout: F,
) -> Result<T, E>
where
    Fut: Future<Output = Result<T, E>>,
    F: FnOnce() -> E,
{
    match tokio::time::timeout(timeout, fut).await {
        Ok(r) => r,
        Err(_) => Err(on_timeout()),
    }
}

/// Poll `check` until it returns `Ok(true)` or `timeout` elapses.
pub async fn poll_until<F, Fut>(
    timeout: Duration,
    interval: Duration,
    mut check: F,
) -> Result<(), String>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<bool, String>>,
{
    let start = tokio::time::Instant::now();
    let deadline = start + timeout;
    loop {
        if check().await? {
            return Ok(());
        }
        let now = tokio::time::Instant::now();
        if now >= deadline {
            return Err(format!(
                "wait timed out after {}ms",
                start.elapsed().as_millis()
            ));
        }
        let sleep_for = interval.min(deadline.saturating_duration_since(now));
        if sleep_for.is_zero() {
            return Err(format!(
                "wait timed out after {}ms",
                start.elapsed().as_millis()
            ));
        }
        tokio::time::sleep(sleep_for).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[tokio::test]
    async fn retry_eventually_succeeds() {
        let n = AtomicU32::new(0);
        let v = retry_async(5, Duration::from_millis(1), || {
            let c = n.fetch_add(1, Ordering::SeqCst);
            async move {
                if c < 2 {
                    Err("nope")
                } else {
                    Ok(42)
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(v, 42);
        assert_eq!(n.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn timeout_maps_error() {
        let err = with_timeout(
            Duration::from_millis(10),
            async {
                tokio::time::sleep(Duration::from_millis(50)).await;
                Ok::<_, &'static str>(())
            },
            || "timed out",
        )
        .await
        .unwrap_err();
        assert_eq!(err, "timed out");
    }

    #[tokio::test]
    async fn poll_until_succeeds_and_times_out() {
        let n = AtomicU32::new(0);
        poll_until(Duration::from_millis(200), Duration::from_millis(5), || {
            let c = n.fetch_add(1, Ordering::SeqCst);
            async move { Ok(c >= 2) }
        })
        .await
        .unwrap();

        let err = poll_until(Duration::from_millis(30), Duration::from_millis(5), || async {
            Ok(false)
        })
        .await
        .unwrap_err();
        assert!(err.contains("timed out"), "{err}");
    }
}
