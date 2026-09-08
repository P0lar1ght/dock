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
}
