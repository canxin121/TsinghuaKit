//! Process-wide dispatch pacing for real campus requests.
//! Loopback fixtures are exempt; credentials and response bodies never enter
//! the gate. A 429 delays subsequent dispatches but never replays a request.

use chrono::{DateTime, Utc};
use reqwest::{
    StatusCode, Url,
    header::{HeaderMap, RETRY_AFTER},
};
use std::{
    net::IpAddr,
    sync::OnceLock,
    time::{Duration, Instant},
};
use tokio::sync::{Mutex, Semaphore, SemaphorePermit};

/// Normal user reads are not a load test: no unconditional multi-second gap.
/// Bound concurrent dispatches instead, and wait when the server asks us to.
const MIN_REQUEST_GAP: Duration = Duration::ZERO;
pub(crate) const MAX_READ_DISPATCHES: u32 = 4;
const DEFAULT_RATE_LIMIT_COOLDOWN: Duration = Duration::from_secs(60);

pub(crate) struct RequestGate {
    gap: Duration,
    next_dispatch: Mutex<Instant>,
    slots: Semaphore,
}

impl RequestGate {
    fn new(gap: Duration) -> Self {
        Self {
            gap,
            next_dispatch: Mutex::new(Instant::now()),
            slots: Semaphore::new(MAX_READ_DISPATCHES as usize),
        }
    }

    #[cfg(test)]
    pub(crate) async fn acquire(&self) -> SemaphorePermit<'_> {
        self.acquire_for(true).await.0
    }

    #[cfg(test)]
    pub(crate) async fn acquire_timed(&self) -> (SemaphorePermit<'_>, Duration, Duration) {
        self.acquire_for(false).await
    }

    /// Auth submissions and consuming navigation acquire every permit. Other
    /// requests may overlap up to the bounded limit. Permits are held through
    /// response headers, not a global mutex across an entire HTTP round trip.
    /// The fair semaphore prevents queued auth from starving behind new reads.
    pub(crate) async fn acquire_for(
        &self,
        exclusive: bool,
    ) -> (SemaphorePermit<'_>, Duration, Duration) {
        let queued = Instant::now();
        let permit = self
            .slots
            .acquire_many(if exclusive { MAX_READ_DISPATCHES } else { 1 })
            .await
            .expect("request gate is never closed");
        let mut queue_wait = queued.elapsed();
        let mut rate_wait = Duration::ZERO;
        loop {
            let locking = Instant::now();
            let mut next = self.next_dispatch.lock().await;
            queue_wait += locking.elapsed();
            let now = Instant::now();
            if let Some(delay) = next.checked_duration_since(now) {
                // Do not reserve a stale deadline: a concurrent 429 can extend
                // cooldown while this task sleeps. Recheck before dispatch.
                drop(next);
                tracing::trace!(target:"tsinghua_kit::http",event="rate_limit_wait",wait_ms=delay.as_millis() as u64);
                let started = Instant::now();
                tokio::time::sleep(delay).await;
                rate_wait += started.elapsed();
            } else {
                *next = now + self.gap;
                break;
            }
        }
        (permit, queue_wait, rate_wait)
    }

    fn observe(&self, next: &mut Instant, status: StatusCode, headers: &HeaderMap) {
        if let Some(delay) = retry_after(status, headers, Utc::now()) {
            tracing::warn!(target:"tsinghua_kit::http",event="rate_limit_cooldown",http_status=status.as_u16(),cooldown_ms=delay.as_millis().min(u64::MAX as u128) as u64);
            if let Some(deadline) = Instant::now().checked_add(delay) {
                *next = (*next).max(deadline);
            }
        }
    }

    pub(crate) async fn observe_response(&self, status: StatusCode, headers: &HeaderMap) {
        // Extend cooldown before releasing the dispatch permit. A successful
        // concurrent response must never erase a server-requested deadline.
        let mut next = self.next_dispatch.lock().await;
        self.observe(&mut next, status, headers);
    }
}

pub(crate) fn configured_request_gap(value: Option<&str>) -> Duration {
    value
        .and_then(|text| text.parse::<u64>().ok())
        .map(|ms| Duration::from_millis(ms.min(60_000)))
        .unwrap_or(MIN_REQUEST_GAP)
}

pub(crate) fn campus_request_gate() -> &'static RequestGate {
    static GATE: OnceLock<RequestGate> = OnceLock::new();
    GATE.get_or_init(|| {
        RequestGate::new(configured_request_gap(
            std::env::var("THYOU_REQUEST_INTERVAL_MS").ok().as_deref(),
        ))
    })
}

pub(crate) fn request_gap_ms() -> u64 {
    campus_request_gate().gap.as_millis() as u64
}

pub(crate) fn is_loopback(url: &Url) -> bool {
    url.host_str().is_some_and(|host| {
        host.eq_ignore_ascii_case("localhost")
            || host
                .trim_matches(['[', ']'])
                .parse::<IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
    })
}

fn retry_after(status: StatusCode, headers: &HeaderMap, now: DateTime<Utc>) -> Option<Duration> {
    if !matches!(
        status,
        StatusCode::TOO_MANY_REQUESTS | StatusCode::SERVICE_UNAVAILABLE
    ) {
        return None;
    }
    let advertised = headers
        .get(RETRY_AFTER)
        .and_then(|s| s.to_str().ok())
        .and_then(|s| {
            s.trim()
                .parse::<u64>()
                .ok()
                .map(Duration::from_secs)
                .or_else(|| {
                    DateTime::parse_from_rfc2822(s.trim()).ok().map(|until| {
                        (until.with_timezone(&Utc) - now)
                            .to_std()
                            .unwrap_or_default()
                    })
                })
        });
    advertised.or_else(|| {
        (status == StatusCode::TOO_MANY_REQUESTS).then_some(DEFAULT_RATE_LIMIT_COOLDOWN)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::HeaderValue;

    #[tokio::test]
    async fn backend_repair_perf_gate_separates_lock_queue_from_throttle_sleep() {
        let gate = RequestGate::new(Duration::from_millis(100));
        let first = gate.acquire().await;
        let release = async {
            tokio::time::sleep(Duration::from_millis(15)).await;
            drop(first);
        };
        let waiting = gate.acquire_timed();
        let ((), (permit, queued, paced)) = tokio::join!(release, waiting);
        assert!(queued >= Duration::from_millis(10));
        assert!(paced >= Duration::from_millis(50));
        drop(permit);
    }

    #[test]
    fn backend_repair_429_defers_following_dispatches_without_replaying() {
        let gate = RequestGate::new(MIN_REQUEST_GAP);
        let mut next = Instant::now();
        gate.observe(&mut next, StatusCode::TOO_MANY_REQUESTS, &HeaderMap::new());
        assert!(next.duration_since(Instant::now()) >= Duration::from_secs(59));
        assert_eq!(MIN_REQUEST_GAP, Duration::ZERO);
    }

    #[tokio::test]
    async fn backend_repair_dispatches_observe_a_shared_minimum_gap() {
        let gate = RequestGate::new(Duration::from_millis(20));
        let started = Instant::now();
        drop(gate.acquire().await);
        drop(gate.acquire().await);
        assert!(started.elapsed() >= Duration::from_millis(20));
    }

    #[test]
    fn backend_repair_retry_after_supports_seconds_and_http_dates() {
        let now = DateTime::parse_from_rfc3339("2026-09-13T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let mut headers = HeaderMap::new();
        headers.insert(RETRY_AFTER, HeaderValue::from_static("120"));
        assert_eq!(
            retry_after(StatusCode::TOO_MANY_REQUESTS, &headers, now),
            Some(Duration::from_secs(120))
        );
        headers.insert(
            RETRY_AFTER,
            HeaderValue::from_static("Sun, 13 Sep 2026 12:01:30 GMT"),
        );
        assert_eq!(
            retry_after(StatusCode::SERVICE_UNAVAILABLE, &headers, now),
            Some(Duration::from_secs(90))
        );
        headers.clear();
        assert_eq!(
            retry_after(StatusCode::TOO_MANY_REQUESTS, &headers, now),
            Some(Duration::from_secs(60))
        );
        assert_eq!(
            retry_after(StatusCode::SERVICE_UNAVAILABLE, &headers, now),
            None
        );
        assert_eq!(retry_after(StatusCode::OK, &headers, now), None);
    }

    #[test]
    fn backend_repair_only_actual_loopback_fixtures_skip_pacing() {
        for endpoint in [
            "http://127.0.0.1:8080/",
            "http://[::1]/",
            "http://localhost/",
        ] {
            assert!(is_loopback(&Url::parse(endpoint).unwrap()));
        }
        for endpoint in [
            "https://id.tsinghua.edu.cn/",
            "https://webvpn.tsinghua.edu.cn/",
            "http://localhost.evil.invalid/",
        ] {
            assert!(!is_loopback(&Url::parse(endpoint).unwrap()));
        }
    }
}

#[cfg(test)]
#[path = "request_gate_latency_tests.rs"]
mod latency_tests;
