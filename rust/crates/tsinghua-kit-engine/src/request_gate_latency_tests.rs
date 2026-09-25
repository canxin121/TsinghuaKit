//! No campus requests. These exercise the actual gate, not the loopback bypass.
use super::*;
use reqwest::header::HeaderValue;
use std::sync::Arc;

#[tokio::test]
async fn backend_repair_captcha_and_srun_mutating_gets_are_exclusive_but_status_remains_bounded_read()
 {
    use crate::transport::CampusHttpTransport;
    let gate = RequestGate::new(Duration::ZERO);
    for url in [
        "https://example.invalid/site/captcha?refresh=1",
        "https://example.invalid/https/fixture-mapping/site/captcha?_=fixture",
        "https://example.invalid/cgi-bin/get_challenge?username=fixture",
        "https://example.invalid/cgi-bin/srun_portal?action=login",
        "https://example.invalid/cgi-bin/srun_portal?action=logout",
    ] {
        let request = reqwest::Request::new(reqwest::Method::GET, Url::parse(url).unwrap());
        let exclusive = CampusHttpTransport::dispatch_requires_exclusivity(&request);
        assert!(exclusive);
        let held = gate.acquire_for(exclusive).await.0;
        assert_eq!(gate.slots.available_permits(), 0);
        drop(held);
    }
    let status = reqwest::Request::new(
        reqwest::Method::GET,
        Url::parse("https://example.invalid/cgi-bin/rad_user_info").unwrap(),
    );
    assert!(!CampusHttpTransport::dispatch_requires_exclusivity(&status));
    let held = gate.acquire_for(false).await.0;
    assert_eq!(gate.slots.available_permits(), 3);
    drop(held);
}

#[tokio::test]
async fn backend_repair_latency_default_76_dispatches_have_no_synthetic_sleep() {
    let gate = RequestGate::new(configured_request_gap(None));
    let mut sleeps = Duration::ZERO;
    tokio::time::timeout(Duration::from_millis(500), async {
        for _ in 0..76 {
            let (permit, _, wait) = gate.acquire_for(false).await;
            sleeps += wait;
            drop(permit);
        }
    })
    .await
    .expect("normal reads must not wait 2 or 3 seconds per dispatch");
    assert_eq!(sleeps, Duration::ZERO);
}

#[tokio::test]
async fn backend_repair_latency_read_permits_overlap_but_fifth_read_waits() {
    let gate = RequestGate::new(Duration::ZERO);
    let mut permits = Vec::new();
    for _ in 0..MAX_READ_DISPATCHES {
        permits.push(
            tokio::time::timeout(Duration::from_millis(100), gate.acquire_for(false))
                .await
                .unwrap()
                .0,
        );
    }
    assert!(
        tokio::time::timeout(Duration::from_millis(15), gate.acquire_for(false))
            .await
            .is_err()
    );
    permits.pop();
    let next = tokio::time::timeout(Duration::from_millis(100), gate.acquire_for(false))
        .await
        .unwrap();
    assert_eq!(next.2, Duration::ZERO);
}

#[tokio::test]
async fn backend_repair_latency_auth_is_exclusive_and_not_starved_by_later_reads() {
    let gate = Arc::new(RequestGate::new(Duration::ZERO));
    let first = gate.acquire_for(false).await.0;
    let order = Arc::new(std::sync::Mutex::new(Vec::new()));
    let auth = {
        let gate = gate.clone();
        let order = order.clone();
        tokio::spawn(async move {
            let _permit = gate.acquire_for(true).await.0;
            order.lock().unwrap().push("auth");
            tokio::time::sleep(Duration::from_millis(10)).await;
        })
    };
    tokio::task::yield_now().await;
    let read = {
        let gate = gate.clone();
        let order = order.clone();
        tokio::spawn(async move {
            let _permit = gate.acquire_for(false).await.0;
            order.lock().unwrap().push("read");
        })
    };
    tokio::task::yield_now().await;
    assert!(order.lock().unwrap().is_empty());
    drop(first);
    auth.await.unwrap();
    read.await.unwrap();
    assert_eq!(*order.lock().unwrap(), ["auth", "read"]);
}

#[tokio::test]
async fn backend_repair_latency_cancelled_waiter_does_not_leak_capacity() {
    let gate = RequestGate::new(Duration::ZERO);
    let exclusive = gate.acquire_for(true).await.0;
    assert!(
        tokio::time::timeout(Duration::from_millis(10), gate.acquire_for(false))
            .await
            .is_err()
    );
    drop(exclusive);
    let permit = tokio::time::timeout(Duration::from_millis(100), gate.acquire_for(true))
        .await
        .unwrap();
    drop(permit);
    assert_eq!(gate.slots.available_permits(), MAX_READ_DISPATCHES as usize);
}

#[tokio::test]
async fn backend_repair_latency_server_retry_after_remains_mandatory_without_replay() {
    let gate = RequestGate::new(Duration::ZERO);
    let mut headers = HeaderMap::new();
    headers.insert(RETRY_AFTER, HeaderValue::from_static("1"));
    gate.observe_response(StatusCode::TOO_MANY_REQUESTS, &headers)
        .await;
    // A simultaneous success cannot erase the rate-limit instruction.
    gate.observe_response(StatusCode::OK, &HeaderMap::new())
        .await;
    let (_permit, _, waited) = gate.acquire_for(false).await;
    assert!(waited >= Duration::from_millis(900));
}

#[tokio::test]
async fn backend_repair_latency_waiter_rechecks_extended_cooldown_before_dispatch() {
    let gate = RequestGate::new(Duration::ZERO);
    *gate.next_dispatch.lock().await = Instant::now() + Duration::from_millis(30);
    let started = Instant::now();
    let waiting = gate.acquire_for(false);
    let extend = async {
        tokio::time::sleep(Duration::from_millis(10)).await;
        *gate.next_dispatch.lock().await = Instant::now() + Duration::from_millis(60);
    };
    let (permit, _) = tokio::join!(waiting, extend);
    assert!(started.elapsed() >= Duration::from_millis(60));
    assert!(permit.2 >= Duration::from_millis(50));
}

#[test]
fn backend_repair_latency_explicit_diagnostic_pacing_is_honored_not_forced() {
    assert_eq!(configured_request_gap(None), Duration::ZERO);
    assert_eq!(configured_request_gap(Some("0")), Duration::ZERO);
    assert_eq!(
        configured_request_gap(Some("250")),
        Duration::from_millis(250)
    );
    assert_eq!(configured_request_gap(Some("3000")), Duration::from_secs(3));
    assert_eq!(
        configured_request_gap(Some("999999")),
        Duration::from_secs(60)
    );
    assert_eq!(configured_request_gap(Some("invalid")), Duration::ZERO);
}

#[tokio::test]
async fn backend_repair_latency_explicit_spacing_still_paces_real_gate() {
    let gate = RequestGate::new(Duration::from_millis(25));
    let first = gate.acquire_for(false).await;
    drop(first);
    let (_permit, _, wait) = gate.acquire_for(false).await;
    assert!(wait >= Duration::from_millis(20));
}

#[test]
fn backend_repair_latency_auth_and_one_shot_navigation_keep_exclusive_dispatch() {
    use crate::transport::CampusHttpTransport;
    for (method, url) in [
        (reqwest::Method::POST, "https://example.invalid/api/read"),
        (
            reqwest::Method::GET,
            "https://example.invalid/entry?ticket=synthetic",
        ),
        (
            reqwest::Method::GET,
            "https://example.invalid/entry?code=synthetic",
        ),
        (
            reqwest::Method::GET,
            "https://example.invalid/thu-oauth/entry",
        ),
        (
            reqwest::Method::GET,
            "https://example.invalid/onlineAppRedirect",
        ),
    ] {
        let request = reqwest::Request::new(method, Url::parse(url).unwrap());
        assert!(CampusHttpTransport::dispatch_requires_exclusivity(&request));
    }
    let request = reqwest::Request::new(
        reqwest::Method::GET,
        Url::parse("https://example.invalid/courses?page=1").unwrap(),
    );
    assert!(!CampusHttpTransport::dispatch_requires_exclusivity(
        &request
    ));
}

#[tokio::test]
async fn backend_repair_latency_unadvertised_429_still_delays_and_success_never_resets_it() {
    let gate = RequestGate::new(Duration::ZERO);
    gate.observe_response(StatusCode::TOO_MANY_REQUESTS, &HeaderMap::new())
        .await;
    let first = *gate.next_dispatch.lock().await;
    assert!(first.duration_since(Instant::now()) >= Duration::from_secs(59));
    gate.observe_response(StatusCode::OK, &HeaderMap::new())
        .await;
    assert_eq!(*gate.next_dispatch.lock().await, first);
}
