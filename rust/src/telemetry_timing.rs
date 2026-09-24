//! Numeric, monotonic timing only. A task-local collector prevents concurrent
//! callers from attributing another operation's requests to their own report.
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    future::Future,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

const MAX_SAMPLES: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    RuntimeQueue,
    GateQueue,
    RateLimit,
    ResponseHeaders,
    ResponseBody,
    Decode,
    CacheRead,
    CacheWrite,
    CredentialStore,
    AuthBootstrap,
    SessionRecovery,
    ServiceHandoff,
    UserInput,
}

pub(crate) fn measure_sync<T, E>(
    phase: Phase,
    work: impl FnOnce() -> Result<T, E>,
) -> Result<T, E> {
    let timer = PhaseTimer::new(phase);
    let result = work();
    timer.finish(result.is_ok());
    result
}
impl Phase {
    pub fn key(self) -> &'static str {
        match self {
            Self::RuntimeQueue => "runtime_queue",
            Self::GateQueue => "gate_queue",
            Self::RateLimit => "rate_limit",
            Self::ResponseHeaders => "response_headers",
            Self::ResponseBody => "response_body",
            Self::Decode => "decode",
            Self::CacheRead => "cache_read",
            Self::CacheWrite => "cache_write",
            Self::CredentialStore => "credential_store",
            Self::AuthBootstrap => "auth_bootstrap",
            Self::SessionRecovery => "session_recovery",
            Self::ServiceHandoff => "service_handoff",
            Self::UserInput => "user_input",
        }
    }
}

pub fn micros(duration: Duration) -> u64 {
    duration.as_micros().min(u64::MAX as u128) as u64
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Distribution {
    pub count: u64,
    pub total_us: u64,
    pub min_us: u64,
    pub max_us: u64,
    pub mean_us: u64,
    pub p50_us: u64,
    pub p95_us: u64,
    pub p99_us: u64,
    pub samples_recorded: usize,
    pub samples_truncated: bool,
}

#[derive(Default)]
struct Samples {
    count: u64,
    total: u64,
    min: u64,
    max: u64,
    values: Vec<u64>,
}
impl Samples {
    fn add(&mut self, value: u64) {
        if self.count == 0 {
            self.min = value;
        }
        self.count = self.count.saturating_add(1);
        self.total = self.total.saturating_add(value);
        self.min = self.min.min(value);
        self.max = self.max.max(value);
        if self.values.len() < MAX_SAMPLES {
            self.values.push(value);
        }
    }
    fn snapshot(&self) -> Distribution {
        let mut values = self.values.clone();
        values.sort_unstable();
        let quantile = |percent: usize| -> u64 {
            if values.is_empty() {
                return 0;
            }
            values[(values.len() * percent).div_ceil(100).saturating_sub(1)]
        };
        Distribution {
            count: self.count,
            total_us: self.total,
            min_us: self.min,
            max_us: self.max,
            mean_us: self.total.checked_div(self.count).unwrap_or(0),
            p50_us: quantile(50),
            p95_us: quantile(95),
            p99_us: quantile(99),
            samples_recorded: values.len(),
            samples_truncated: self.count > values.len() as u64,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TimingSnapshot {
    pub requests: u64,
    pub responses: u64,
    pub transport_failures: u64,
    pub body_reads: u64,
    pub body_failures: u64,
    pub cancellations: u64,
    pub decoded_text_bytes: u64,
    pub binary_body_bytes: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub cache_failures: u64,
    pub phases: BTreeMap<Phase, Distribution>,
}
impl TimingSnapshot {
    pub fn phase_us(&self, phase: Phase) -> u64 {
        self.phases.get(&phase).map_or(0, |d| d.total_us)
    }
}

#[derive(Default)]
struct Metrics {
    snapshot: TimingSnapshot,
    samples: BTreeMap<Phase, Samples>,
}
#[derive(Clone, Default)]
pub(crate) struct Collector(Arc<Mutex<Metrics>>);
tokio::task_local! { static CURRENT: Collector; }

impl Collector {
    pub(crate) fn current() -> Option<Self> {
        CURRENT.try_with(Clone::clone).ok()
    }
    pub(crate) fn phase(&self, phase: Phase, duration: Duration) {
        if let Ok(mut metrics) = self.0.lock() {
            metrics
                .samples
                .entry(phase)
                .or_default()
                .add(micros(duration));
        }
    }
    pub(crate) fn update(&self, update: impl FnOnce(&mut TimingSnapshot)) {
        if let Ok(mut metrics) = self.0.lock() {
            update(&mut metrics.snapshot);
        }
    }
    pub(crate) fn snapshot(&self) -> TimingSnapshot {
        self.0
            .lock()
            .map(|metrics| {
                let mut snapshot = metrics.snapshot.clone();
                snapshot.phases = metrics
                    .samples
                    .iter()
                    .map(|(p, s)| (*p, s.snapshot()))
                    .collect();
                snapshot
            })
            .unwrap_or_default()
    }
    pub(crate) async fn scope<T>(&self, future: impl Future<Output = T>) -> T {
        CURRENT.scope(self.clone(), future).await
    }
}

pub(crate) async fn capture<T>(future: impl Future<Output = T>) -> (T, TimingSnapshot) {
    let collector = Collector::default();
    let value = collector.scope(future).await;
    (value, collector.snapshot())
}

/// RAII completion also records cancelled futures/unwinding, without ever
/// formatting an input, returned value, error message, URL or account path.
pub struct PhaseTimer {
    phase: Phase,
    started: Instant,
    collector: Option<Collector>,
    outcome: &'static str,
}
impl PhaseTimer {
    pub fn new(phase: Phase) -> Self {
        Self {
            phase,
            started: Instant::now(),
            collector: Collector::current(),
            outcome: "cancelled",
        }
    }
    pub fn finish(mut self, success: bool) {
        self.outcome = if success { "success" } else { "failed" };
    }
}
impl Drop for PhaseTimer {
    fn drop(&mut self) {
        let elapsed = self.started.elapsed();
        if let Some(collector) = &self.collector {
            collector.phase(self.phase, elapsed);
        }
        tracing::info!(target:"tsinghua_kit::api", event="phase_finished", phase=self.phase.key(),
            duration_us=micros(elapsed), duration_ms=elapsed.as_millis() as u64, outcome=self.outcome);
    }
}
pub(crate) async fn phase<T, E>(
    phase: Phase,
    future: impl Future<Output = Result<T, E>>,
) -> Result<T, E> {
    let timer = PhaseTimer::new(phase);
    let result = future.await;
    timer.finish(result.is_ok());
    result
}

pub(crate) fn cache_result(hit: bool, failed: bool) {
    if let Some(collector) = Collector::current() {
        collector.update(|m| {
            if failed {
                m.cache_failures += 1;
            } else if hit {
                m.cache_hits += 1;
            } else {
                m.cache_misses += 1;
            }
        });
    }
}

/// Stored in reqwest extensions: no URL/body/credential, just attribution.
#[derive(Clone)]
pub(crate) struct ResponseStamp {
    pub request_id: u64,
    pub endpoint: &'static str,
    pub collector: Option<Collector>,
}

pub(crate) async fn read_text(response: reqwest::Response) -> Result<String, reqwest::Error> {
    let trace = BodyTrace::new(&response, true);
    let result = response.text().await; // Keep reqwest charset semantics.
    trace.finish(result.as_ref().map(|s| s.len()).ok(), result.is_ok());
    result
}
pub(crate) async fn read_bytes(response: reqwest::Response) -> Result<Vec<u8>, reqwest::Error> {
    let trace = BodyTrace::new(&response, false);
    let result = response.bytes().await.map(|bytes| bytes.to_vec());
    trace.finish(result.as_ref().map(|s| s.len()).ok(), result.is_ok());
    result
}

#[derive(Debug)]
pub(crate) enum BoundedBodyError {
    Request(reqwest::Error),
    TooLarge,
}

#[derive(Debug)]
pub(crate) enum BoundedStreamError {
    Request(reqwest::Error),
    TooLarge,
    Io(std::io::Error),
}

pub(crate) struct BoundedStreamReceipt {
    pub bytes_written: usize,
    pub prefix: Vec<u8>,
}

/// Stream a verified binary response to a caller-owned writer. Keeping only
/// a short prefix permits login-page rejection without holding a course file
/// in memory; all body bytes still appear in the shared phase accounting.
pub(crate) async fn stream_bounded_to_writer<W: std::io::Write>(
    mut response: reqwest::Response,
    writer: &mut W,
    limit: usize,
) -> Result<BoundedStreamReceipt, BoundedStreamError> {
    let trace = BodyTrace::new(&response, false);
    let mut bytes_written = 0usize;
    let mut prefix = Vec::with_capacity(1024);
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                if chunk.len() > limit.saturating_sub(bytes_written) {
                    trace.finish(None, false);
                    return Err(BoundedStreamError::TooLarge);
                }
                if prefix.len() < 1024 {
                    prefix.extend_from_slice(&chunk[..chunk.len().min(1024 - prefix.len())]);
                }
                if let Err(error) = writer.write_all(&chunk) {
                    trace.finish(None, false);
                    return Err(BoundedStreamError::Io(error));
                }
                bytes_written += chunk.len();
            }
            Ok(None) => {
                trace.finish(Some(bytes_written), true);
                return Ok(BoundedStreamReceipt {
                    bytes_written,
                    prefix,
                });
            }
            Err(error) => {
                trace.finish(None, false);
                return Err(BoundedStreamError::Request(error));
            }
        }
    }
}

/// Read an image or other binary response without trusting Content-Length.
/// A server that omits or lies about the header cannot make the Runtime hold
/// an unbounded body, and the usual phase accounting still sees the read.
pub(crate) async fn read_bounded_bytes(
    mut response: reqwest::Response,
    limit: usize,
) -> Result<Vec<u8>, BoundedBodyError> {
    let trace = BodyTrace::new(&response, false);
    let mut bytes = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                if chunk.len() > limit.saturating_sub(bytes.len()) {
                    trace.finish(None, false);
                    return Err(BoundedBodyError::TooLarge);
                }
                bytes.extend_from_slice(&chunk);
            }
            Ok(None) => {
                trace.finish(Some(bytes.len()), true);
                return Ok(bytes);
            }
            Err(error) => {
                trace.finish(None, false);
                return Err(BoundedBodyError::Request(error));
            }
        }
    }
}
struct BodyTrace {
    stamp: Option<ResponseStamp>,
    start: Instant,
    text: bool,
    done: bool,
}
impl BodyTrace {
    fn new(response: &reqwest::Response, text: bool) -> Self {
        Self {
            stamp: response.extensions().get::<ResponseStamp>().cloned(),
            start: Instant::now(),
            text,
            done: false,
        }
    }
    fn finish(mut self, bytes: Option<usize>, success: bool) {
        self.done = true;
        record_body(
            self.stamp.clone(),
            self.start.elapsed(),
            bytes,
            self.text,
            if success { "success" } else { "failed" },
        );
    }
}
impl Drop for BodyTrace {
    fn drop(&mut self) {
        if !self.done {
            record_body(
                self.stamp.clone(),
                self.start.elapsed(),
                None,
                self.text,
                "cancelled",
            );
        }
    }
}
fn record_body(
    stamp: Option<ResponseStamp>,
    duration: Duration,
    bytes: Option<usize>,
    text: bool,
    outcome: &'static str,
) {
    let collector = stamp
        .as_ref()
        .and_then(|s| s.collector.clone())
        .or_else(Collector::current);
    if let Some(collector) = collector {
        collector.phase(Phase::ResponseBody, duration);
        collector.update(|m| {
            m.body_reads += 1;
            if outcome == "failed" {
                m.body_failures += 1;
            }
            if outcome == "cancelled" {
                m.cancellations += 1;
            }
            if text {
                m.decoded_text_bytes = m
                    .decoded_text_bytes
                    .saturating_add(bytes.unwrap_or(0) as u64);
            } else {
                m.binary_body_bytes = m
                    .binary_body_bytes
                    .saturating_add(bytes.unwrap_or(0) as u64);
            }
        });
    }
    tracing::info!(target:"tsinghua_kit::http", event="body_finished",
        request_id=stamp.as_ref().map_or(0, |s| s.request_id),
        endpoint=stamp.as_ref().map_or("unknown", |s| s.endpoint),
        duration_us=micros(duration), duration_ms=duration.as_millis() as u64,
        decoded_text_bytes=if text { bytes.unwrap_or(0) as u64 } else { 0 },
        binary_body_bytes=if text { 0 } else { bytes.unwrap_or(0) as u64 },
        outcome);
}

#[cfg(test)]
#[path = "telemetry_timing_tests.rs"]
mod tests;
