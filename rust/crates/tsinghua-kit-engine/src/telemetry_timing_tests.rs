use super::*;
use crate::{
    reference_test_support::{FixtureServer, Reply},
    transport::CampusHttpTransport,
};

#[test]
fn backend_repair_perf_percentiles_are_nearest_rank_and_bounded() {
    let mut samples = Samples::default();
    for value in 1..=100 {
        samples.add(value);
    }
    let result = samples.snapshot();
    assert_eq!((result.p50_us, result.p95_us, result.p99_us), (50, 95, 99));
    assert_eq!(
        (result.count, result.total_us, result.min_us, result.max_us),
        (100, 5050, 1, 100)
    );
    for value in 101..=5000 {
        samples.add(value);
    }
    let result = samples.snapshot();
    assert_eq!(result.samples_recorded, MAX_SAMPLES);
    assert!(result.samples_truncated);
    assert_eq!(result.count, 5000);
    assert_eq!(result.max_us, 5000);
}

#[tokio::test]
async fn backend_repair_perf_concurrent_collectors_do_not_mix_requests_or_phases() {
    let server_a = FixtureServer::new(vec![Reply::html("one")]);
    let server_b = FixtureServer::new(vec![Reply::html("second"), Reply::html("third")]);
    let a = async {
        let transport = CampusHttpTransport::new("fixture-timing").unwrap();
        transport.get_text(server_a.base()).await.unwrap();
        measure_sync(Phase::CacheRead, || Ok::<_, ()>(())).unwrap();
    };
    let b = async {
        let transport = CampusHttpTransport::new("fixture-timing").unwrap();
        transport.get_text(server_b.base()).await.unwrap();
        tokio::task::yield_now().await;
        transport.get_text(server_b.base()).await.unwrap();
    };
    let ((_, a), (_, b)) = tokio::join!(capture(a), capture(b));
    assert_eq!((a.requests, b.requests), (1, 2));
    assert_eq!((a.decoded_text_bytes, b.decoded_text_bytes), (3, 11));
    assert!(a.phases.contains_key(&Phase::CacheRead));
    assert!(!b.phases.contains_key(&Phase::CacheRead));
    assert!(Collector::current().is_none());
}

#[tokio::test]
async fn backend_repair_perf_body_time_is_measured_after_headers_not_as_server_latency() {
    use std::{
        io::{Read, Write},
        net::TcpListener,
    };
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let worker = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut buf = [0; 2048];
        let _ = stream.read(&mut buf).unwrap();
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\n")
            .unwrap();
        stream.flush().unwrap();
        std::thread::sleep(Duration::from_millis(60));
        stream.write_all(b"hello").unwrap();
    });
    let transport = CampusHttpTransport::new("fixture-body-timing").unwrap();
    let (result, timings) = capture(transport.get_text(&url)).await;
    assert_eq!(result.unwrap(), "hello");
    assert!(timings.phase_us(Phase::ResponseBody) >= 40_000);
    assert_eq!(timings.phases[&Phase::ResponseHeaders].count, 1);
    assert_eq!(timings.decoded_text_bytes, 5);
    worker.join().unwrap();
}

#[tokio::test]
async fn backend_repair_perf_cache_measurements_preserve_real_miss_hit_and_error() {
    let root = std::env::temp_dir().join(format!("thyou-perf-{}", uuid::Uuid::new_v4()));
    let cache = crate::cache::JsonFileCache::<serde_json::Value>::new(
        root.join("cache.json"),
        1,
        "fixture",
    );
    let (_, result) = capture(async {
        assert!(cache.read().unwrap().is_none());
        cache.write(serde_json::json!({"value":1})).unwrap();
        assert!(cache.read().unwrap().is_some());
        std::fs::write(cache.path(), "invalid").unwrap();
        assert!(cache.read().is_err());
    })
    .await;
    assert_eq!(
        (
            result.cache_misses,
            result.cache_hits,
            result.cache_failures
        ),
        (1, 1, 1)
    );
    assert_eq!(result.phases[&Phase::CacheRead].count, 3);
    assert_eq!(result.phases[&Phase::CacheWrite].count, 1);
    assert_eq!(result.requests, 0);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn backend_repair_perf_cancelled_phase_is_recorded_without_replaying_work() {
    let collector = Collector::default();
    collector
        .scope(async {
            let result = tokio::time::timeout(
                Duration::from_millis(15),
                phase(
                    Phase::SessionRecovery,
                    std::future::pending::<Result<(), ()>>(),
                ),
            )
            .await;
            assert!(result.is_err());
        })
        .await;
    let metrics = collector.snapshot();
    assert_eq!(metrics.phases[&Phase::SessionRecovery].count, 1);
    assert_eq!(metrics.requests, 0);
}

#[tokio::test]
async fn backend_repair_perf_detailed_logs_keep_timings_but_reject_credentials_and_urls() {
    use tracing::instrument::WithSubscriber;
    let root = std::env::temp_dir().join(format!("thyou-perf-log-{}", uuid::Uuid::new_v4()));
    let mut log =
        crate::telemetry::LogSession::start(&root, crate::telemetry::LogConfig::default()).unwrap();
    let server = FixtureServer::new(vec![Reply::html("PRIVATE_BODY_FIXTURE")]);
    let url = format!("{}?ticket=PRIVATE_TICKET_FIXTURE", server.base());
    async {
        let transport = CampusHttpTransport::new("fixture").unwrap();
        let _ = capture(transport.get_text(&url)).await;
        measure_sync(Phase::Decode, || Ok::<_, ()>(())).unwrap();
        tracing::info!(target:"tsinghua_kit::auth",event="phase_finished",phase="decode",
            duration_us=12_u64,password="PRIVATE_PASSWORD_FIXTURE",url=url.as_str());
    }
    .with_subscriber(log.dispatch.clone())
    .await;
    log.flush();
    let contents = std::fs::read_to_string(log.directory.join("events.000001.jsonl")).unwrap();
    assert!(contents.contains("duration_us"));
    assert!(contents.contains("headers_us"));
    assert!(contents.contains("elapsed_us"));
    assert!(contents.contains("body_finished"));
    for secret in [
        "PRIVATE_BODY_FIXTURE",
        "PRIVATE_TICKET_FIXTURE",
        "PRIVATE_PASSWORD_FIXTURE",
    ] {
        assert!(!contents.contains(secret));
    }
    drop(log);
    let _ = std::fs::remove_dir_all(root);
}
