//! Application-owned tracing, with a deny-by-default field boundary.
//! Never record arguments, URLs, headers, bodies, usernames or credentials.
use chrono::{SecondsFormat, Utc};
use serde_json::{Map, Value, json};
use std::{
    collections::{BTreeMap, VecDeque},
    fmt,
    fs::{self, File, OpenOptions},
    future::Future,
    io::{self, BufWriter, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Instant,
};
use tracing::{
    Dispatch, Event, Instrument, Level, Metadata, Subscriber,
    field::{Field, Visit},
};
use tracing_appender::non_blocking::{NonBlocking, NonBlockingBuilder, WorkerGuard};
use tracing_subscriber::{Layer, layer::Context, prelude::*, registry::LookupSpan};

const MODULES: &[&str] = &[
    "api",
    "auth",
    "http",
    "session",
    "security",
    "storage",
    "validation",
];
#[path = "telemetry_labels.rs"]
mod labels;
#[path = "telemetry_timing.rs"]
pub mod timing;
static IDS: AtomicU64 = AtomicU64::new(1);
static REQUESTS: AtomicU64 = AtomicU64::new(0);

/// A WebVPN site mapping consists of the public fixed IV followed by a hex
/// encoded host selector. It is not a URL or session ticket. The narrow form
/// is used only to diagnose a blocked INFO news redirect.
pub(crate) fn safe_webvpn_mapping_id(value: &str) -> bool {
    const IV: &str = "77726476706e69737468656265737421";
    value.starts_with(IV)
        && (64..=96).contains(&value.len())
        && value.len() % 2 == 0
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}
pub(crate) fn next_id() -> u64 {
    IDS.fetch_add(1, Ordering::Relaxed)
}

fn write_json_buffered(writer: &mut impl Write, value: &impl serde::Serialize) -> io::Result<()> {
    let mut buffered = BufWriter::with_capacity(64 * 1024, writer);
    serde_json::to_writer_pretty(&mut buffered, value)?;
    buffered.write_all(b"\n")?;
    buffered.flush()
}

#[cfg(test)]
#[path = "telemetry_request_audit_tests.rs"]
mod request_audit_tests;

pub(crate) fn diagnostic_reason(error: &str) -> &'static str {
    // Only copy an already source-owned fixed diagnostic code, never raw
    // URL/body text. The public UI message may carry this code in parentheses.
    if let Some(start) = error.find('（')
        && let Some(end) = error[start + '（'.len_utf8()..].find('）')
    {
        let code = &error[start + '（'.len_utf8()..start + '（'.len_utf8() + end];
        if let Some(known) = labels::REASONS.iter().copied().find(|known| *known == code) {
            return known;
        }
    }
    match error {
        "统一认证需要图形验证码，当前密码登录未完成" => {
            return "image_captcha_required";
        }
        "统一认证登录会话已失效，请重新发起登录" => {
            return "identity_session_invalid";
        }
        "统一认证未通过，具体原因待确认，请查看认证诊断" => {
            return "identity_login_rejected";
        }
        _ => {}
    }
    if error.contains("验证码错误") || error.contains("verification code was rejected") {
        return "verification_code_rejected";
    }
    if error.contains("密码错误") || error.contains("用户名或密码错误") {
        return "credentials_rejected";
    }
    labels::REASONS
        .iter()
        .copied()
        .find(|reason| *reason == error)
        .unwrap_or_else(|| crate::live_validation::error_reason(error))
}

pub(crate) struct HttpTrace {
    id: u64,
    endpoint: &'static str,
    method: &'static str,
    start: Instant,
    dispatched_at: Option<Instant>,
    collector: Option<timing::Collector>,
    done: bool,
}
impl HttpTrace {
    pub(crate) fn begin(request: &reqwest::Request) -> Self {
        let trace = Self {
            id: next_id(),
            endpoint: endpoint(request.url()),
            method: method(request.method()),
            start: Instant::now(),
            dispatched_at: None,
            collector: timing::Collector::current(),
            done: false,
        };
        tracing::debug!(target:"tsinghua_kit::http",event="request_queued",request_id=trace.id,endpoint=trace.endpoint,method=trace.method,loopback=crate::request_gate::is_loopback(request.url()));
        trace
    }
    pub(crate) fn waited(&self, queue: std::time::Duration, rate: std::time::Duration) {
        if let Some(c) = &self.collector {
            c.phase(timing::Phase::GateQueue, queue);
            c.phase(timing::Phase::RateLimit, rate);
        }
        tracing::info!(target:"tsinghua_kit::http",event="request_wait_finished",request_id=self.id,
            endpoint=self.endpoint,gate_queue_us=timing::micros(queue),rate_limit_us=timing::micros(rate));
    }
    pub(crate) fn dispatched(&mut self) {
        self.dispatched_at = Some(Instant::now());
        if let Some(c) = &self.collector {
            c.update(|m| m.requests += 1);
        }
        REQUESTS.fetch_add(1, Ordering::Relaxed);
        tracing::trace!(target:"tsinghua_kit::http",event="request_dispatched",request_id=self.id,endpoint=self.endpoint,method=self.method,wait_ms=self.start.elapsed().as_millis() as u64);
    }
    pub(crate) fn finish(&mut self, result: &mut Result<reqwest::Response, reqwest::Error>) {
        self.done = true;
        let elapsed = self.start.elapsed();
        let duration_ms = elapsed.as_millis() as u64;
        let headers = self
            .dispatched_at
            .map_or(std::time::Duration::ZERO, |at| at.elapsed());
        if let Some(c) = &self.collector {
            c.phase(timing::Phase::ResponseHeaders, headers);
            c.update(|m| {
                if result.is_ok() {
                    m.responses += 1;
                } else {
                    m.transport_failures += 1;
                }
            });
        }
        tracing::info!(target:"tsinghua_kit::http",event="request_timing",request_id=self.id,
            endpoint=self.endpoint,method=self.method,headers_us=timing::micros(headers),
            duration_us=timing::micros(elapsed),outcome=if result.is_ok() {"success"} else {"failed"});
        match result {
            Ok(response) => {
                response.extensions_mut().insert(timing::ResponseStamp {
                    request_id: self.id,
                    endpoint: self.endpoint,
                    collector: self.collector.clone(),
                });
                let http_status = response.status().as_u16();
                let redirect_present = response.headers().contains_key(reqwest::header::LOCATION);
                let cookie_updated = response.headers().contains_key(reqwest::header::SET_COOKIE);
                let bytes = response.content_length().unwrap_or(0);
                if response.status().is_client_error() || response.status().is_server_error() {
                    tracing::warn!(target:"tsinghua_kit::http",event="response_headers",request_id=self.id,endpoint=self.endpoint,method=self.method,http_status,redirect_present,cookie_updated,bytes,duration_ms);
                } else {
                    tracing::debug!(target:"tsinghua_kit::http",event="response_headers",request_id=self.id,endpoint=self.endpoint,method=self.method,http_status,redirect_present,cookie_updated,bytes,duration_ms);
                }
            }
            Err(error) => {
                let reason = if error.is_timeout() {
                    "timeout"
                } else if error.is_connect() {
                    "connect"
                } else if error.is_body() {
                    "body_read"
                } else {
                    "transport"
                };
                tracing::warn!(target:"tsinghua_kit::http",event="request_failed",request_id=self.id,endpoint=self.endpoint,method=self.method,reason,duration_ms);
            }
        }
    }
}
impl Drop for HttpTrace {
    fn drop(&mut self) {
        if !self.done {
            if let Some(c) = &self.collector {
                c.update(|m| m.cancellations += 1);
            }
            tracing::warn!(target:"tsinghua_kit::http",event="request_cancelled",request_id=self.id,endpoint=self.endpoint,duration_ms=self.start.elapsed().as_millis() as u64);
        }
    }
}

pub(crate) fn service_name(service: crate::protocol::ServiceId) -> &'static str {
    use crate::protocol::ServiceId::*;
    match service {
        Identity => "identity",
        Learn => "learn",
        Registrar => "registrar",
        Info => "info",
        Usereg => "usereg",
        Library => "library",
        CampusCard => "campus_card",
    }
}
pub(crate) fn state_name(state: crate::protocol::ServiceSessionState) -> &'static str {
    use crate::protocol::ServiceSessionState::*;
    match state {
        Anonymous => "anonymous",
        Authenticating => "authenticating",
        RequiresSecondFactor => "requires_second_factor",
        Authenticated => "authenticated",
        Expired => "expired",
    }
}
pub fn request_count() -> u64 {
    REQUESTS.load(Ordering::Relaxed)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Verbosity {
    Off,
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}
impl Verbosity {
    pub fn parse(value: &str) -> Result<Self, &'static str> {
        match value.to_ascii_lowercase().as_str() {
            "off" => Ok(Self::Off),
            "error" => Ok(Self::Error),
            "warn" => Ok(Self::Warn),
            "info" => Ok(Self::Info),
            "debug" => Ok(Self::Debug),
            "trace" => Ok(Self::Trace),
            _ => Err("invalid_log_level"),
        }
    }
    fn permits(self, level: &Level) -> bool {
        let level = match *level {
            Level::ERROR => Self::Error,
            Level::WARN => Self::Warn,
            Level::INFO => Self::Info,
            Level::DEBUG => Self::Debug,
            Level::TRACE => Self::Trace,
        };
        self >= level
    }
}

#[derive(Clone, Debug)]
pub struct LogConfig {
    pub level: Verbosity,
    pub modules: BTreeMap<String, Verbosity>,
    pub console: bool,
    pub max_file_bytes: u64,
    pub retained_files: usize,
}
impl Default for LogConfig {
    fn default() -> Self {
        Self {
            level: Verbosity::Info,
            modules: BTreeMap::new(),
            console: false,
            max_file_bytes: 4 * 1024 * 1024,
            retained_files: 4,
        }
    }
}
impl LogConfig {
    /// Examples: `info`, `debug,http=trace,auth=debug`. No dependency targets
    /// or arbitrary RUST_LOG directives can expose HTTP library internals.
    pub fn parse(spec: &str, console: bool) -> Result<Self, &'static str> {
        let mut config = Self {
            console,
            ..Default::default()
        };
        if spec.is_empty() || spec.len() > 512 {
            return Err("invalid_log_filter");
        }
        for (i, part) in spec.split(',').enumerate() {
            if let Some((module, level)) = part.split_once('=') {
                if !MODULES.contains(&module) {
                    return Err("unknown_log_module");
                }
                config
                    .modules
                    .insert(module.into(), Verbosity::parse(level)?);
            } else if i == 0 {
                config.level = Verbosity::parse(part)?;
            } else {
                return Err("invalid_log_filter");
            }
        }
        Ok(config)
    }
    fn enabled(&self, meta: &Metadata<'_>) -> bool {
        let Some(module) = meta.target().strip_prefix("tsinghua_kit::") else {
            return false;
        };
        MODULES.contains(&module)
            && (meta.is_span()
                || self
                    .modules
                    .get(module)
                    .copied()
                    .unwrap_or(self.level)
                    .permits(meta.level()))
    }
}

/// Only new private directories/files owned by this workflow are created.
/// Existing symlink destinations are rejected, never followed or chmod'd.
pub fn private_dir(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
            return Err(io::Error::other("unsafe_output_directory"));
        }
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let mut builder = fs::DirBuilder::new();
            builder.recursive(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            builder.create(path)?;
        }
        Err(e) => return Err(e),
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if fs::metadata(path)?.permissions().mode() & 0o077 != 0 {
            return Err(io::Error::other("output_directory_not_private"));
        }
    }
    Ok(())
}
pub fn new_private_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).read(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}
pub fn write_private_json(path: &Path, value: &impl serde::Serialize) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("invalid_report_path"))?;
    private_dir(parent)?;
    if fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink() || !m.is_file()) {
        return Err(io::Error::other("unsafe_report_path"));
    }
    let tmp = parent.join(format!(".report-{}.tmp", uuid::Uuid::new_v4().simple()));
    let result = (|| {
        let mut file = new_private_file(&tmp)?;
        // serde emits many tiny writes for pretty JSON. Buffer them without
        // weakening the sync-before-rename checkpoint or error cleanup.
        write_json_buffered(&mut file, value)?;
        file.sync_all()?;
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

struct RollingWriter {
    directory: PathBuf,
    stem: &'static str,
    max: u64,
    keep: usize,
    index: u64,
    size: u64,
    file: File,
    paths: VecDeque<PathBuf>,
    failed: Arc<AtomicBool>,
}
impl RollingWriter {
    fn new(
        directory: &Path,
        stem: &'static str,
        config: &LogConfig,
        failed: Arc<AtomicBool>,
    ) -> io::Result<Self> {
        let path = directory.join(format!("{stem}.000001.jsonl"));
        let file = new_private_file(&path)?;
        Ok(Self {
            directory: directory.into(),
            stem,
            max: config.max_file_bytes,
            keep: config.retained_files,
            index: 1,
            size: 0,
            file,
            paths: VecDeque::from([path]),
            failed,
        })
    }
    fn append(&mut self, bytes: &[u8]) -> io::Result<()> {
        if self.size > 0 && self.size.saturating_add(bytes.len() as u64) > self.max {
            self.file.flush()?;
            self.index += 1;
            let path = self
                .directory
                .join(format!("{}.{:06}.jsonl", self.stem, self.index));
            self.file = new_private_file(&path)?;
            self.paths.push_back(path);
            self.size = 0;
            while self.paths.len() > self.keep {
                if let Some(old) = self.paths.pop_front() {
                    fs::remove_file(old)?;
                }
            }
        }
        self.file.write_all(bytes)?;
        self.file.flush()?;
        self.size += bytes.len() as u64;
        Ok(())
    }
}
impl Write for RollingWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if let Err(e) = self.append(bytes) {
            self.failed.store(true, Ordering::Relaxed);
            return Err(e);
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

#[derive(Default, Clone)]
struct SafeFields {
    values: Map<String, Value>,
    redacted: usize,
}
fn numeric_field(name: &str) -> bool {
    matches!(
        name,
        "operation_id"
            | "phase_id"
            | "duration_us"
            | "queue_wait_us"
            | "dependency_wait_us"
            | "execution_us"
            | "user_input_us"
            | "body_read_us"
            | "decode_us"
            | "cache_read_us"
            | "cache_write_us"
            | "auth_bootstrap_us"
            | "service_handoff_us"
            | "session_recovery_us"
            | "cache_hits"
            | "cache_misses"
            | "headers_us"
            | "gate_queue_us"
            | "rate_limit_us"
            | "decoded_text_bytes"
            | "binary_body_bytes"
            | "configured_concurrency"
            | "runtime_parallelism"
            | "queue_depth"
            | "request_id"
            | "duration_ms"
            | "wait_ms"
            | "http_status"
            | "hop"
            | "bytes"
            | "case_index"
            | "total"
            | "count"
            | "attempt"
            | "request_count"
            | "cooldown_ms"
            | "table_index"
            | "row_count"
            | "row_index"
            | "header_row"
            | "actual_columns"
            | "expected_columns"
            | "header_fields"
            | "header_mask"
            | "header_unknown_mask"
            | "header_role_position_mask"
    )
}
fn text_field(name: &str) -> bool {
    matches!(
        name,
        "event"
            | "thos_target"
            | "news_target"
            | "news_fragment"
            | "news_query"
            | "news_mapping_id"
            | "operation"
            | "service"
            | "outcome"
            | "reason"
            | "method"
            | "from"
            | "to"
            | "phase"
            | "endpoint"
            | "category"
            | "case"
            | "evidence_source"
            | "evidence_marker"
            | "submission_encoding"
            | "submission_route"
            | "response_encoding"
            | "status_signal"
            | "observed_state"
            | "bound_state"
            | "data_field"
            | "business_stage"
            | "reuse_result"
            | "trust_result"
            | "route_class"
            | "route_decision"
            | "header_term_kind"
            | "header_term_shape"
            | "reference_fallback_state"
            | "header_failure_stage"
            | "grade_field"
            | "grade_value_shape"
            | "bucket"
            | "parse_reason"
            | "parse_field"
            | "body_shape"
            | "date_shape"
            | "date_width"
    )
}
fn safe_token(value: &str) -> bool {
    value.len() <= 96
        && !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}
impl Visit for SafeFields {
    fn record_str(&mut self, field: &Field, value: &str) {
        // Operations/cases are static source keys. Free text cannot pass this
        // boundary; message/debug fields are never formatted or persisted.
        if text_field(field.name()) && safe_token(value) && labels::allowed(field.name(), value) {
            self.values.insert(field.name().into(), json!(value));
        } else {
            self.redacted += 1;
        }
    }
    fn record_u64(&mut self, field: &Field, value: u64) {
        if numeric_field(field.name()) {
            self.values.insert(field.name().into(), json!(value));
        } else {
            self.redacted += 1;
        }
    }
    fn record_i64(&mut self, field: &Field, value: i64) {
        if numeric_field(field.name()) {
            self.values.insert(field.name().into(), json!(value));
        } else {
            self.redacted += 1;
        }
    }
    fn record_bool(&mut self, field: &Field, value: bool) {
        if matches!(
            field.name(),
            "redirect_present"
                | "cookie_updated"
                | "csrf_present"
                | "ticket_present"
                | "loopback"
                | "persisted"
                | "login_form_present"
                | "factor_present"
                | "handoff_proven"
                | "verified_flow"
                | "address_field_present"
                | "expected_ip_present"
                | "online_proven"
                | "offline_proven"
        ) {
            self.values.insert(field.name().into(), json!(value));
        } else {
            self.redacted += 1;
        }
    }
    fn record_debug(&mut self, _: &Field, _: &dyn fmt::Debug) {
        self.redacted += 1;
    }
}

struct SafeLayer {
    origin: Instant,
    config: LogConfig,
    run_id: String,
    events: NonBlocking,
    errors: NonBlocking,
    sequence: AtomicU64,
    failed: Arc<AtomicBool>,
}
impl<S: Subscriber + for<'a> LookupSpan<'a>> Layer<S> for SafeLayer {
    fn enabled(&self, meta: &Metadata<'_>, _: Context<'_, S>) -> bool {
        self.config.enabled(meta)
    }
    fn on_new_span(
        &self,
        attrs: &tracing::span::Attributes<'_>,
        id: &tracing::span::Id,
        ctx: Context<'_, S>,
    ) {
        let mut fields = SafeFields::default();
        attrs.record(&mut fields);
        if let Some(span) = ctx.span(id) {
            span.extensions_mut().insert(fields);
        }
    }
    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        if !self.config.enabled(event.metadata()) {
            return;
        }
        let mut fields = SafeFields::default();
        event.record(&mut fields);
        let mut spans = Vec::new();
        if let Some(scope) = ctx.event_scope(event) {
            for span in scope.from_root() {
                if let Some(safe) = span.extensions().get::<SafeFields>() {
                    spans.push(Value::Object(safe.values.clone()));
                }
            }
        }
        let module = event
            .metadata()
            .target()
            .strip_prefix("tsinghua_kit::")
            .unwrap_or("api");
        let value = json!({"schema":1,"elapsed_us":timing::micros(self.origin.elapsed()),"version":env!("CARGO_PKG_VERSION"),"timestamp":Utc::now().to_rfc3339_opts(SecondsFormat::Millis,true),"run_id":self.run_id,"sequence":self.sequence.fetch_add(1,Ordering::Relaxed),"pid":std::process::id(),"level":event.metadata().level().as_str(),"module":module,"fields":fields.values,"redacted_fields":fields.redacted,"spans":spans});
        if let Ok(mut bytes) = serde_json::to_vec(&value) {
            bytes.push(b'\n');
            let _ = self.events.clone().write_all(&bytes);
            if *event.metadata().level() <= Level::WARN {
                let _ = self.errors.clone().write_all(&bytes);
            }
        }
        if self.config.console && *event.metadata().level() <= Level::INFO {
            let f = &value["fields"];
            let _ = writeln!(
                io::stderr(),
                "[{}][{}] {} {} {} {}ms",
                event.metadata().level(),
                module,
                f["operation"].as_str().or(f["case"].as_str()).unwrap_or(""),
                f["event"].as_str().unwrap_or(""),
                f["reason"].as_str().or(f["outcome"].as_str()).unwrap_or(""),
                f["duration_ms"].as_u64().unwrap_or(0)
            );
        }
        if self.failed.load(Ordering::Relaxed) {
            let _ = writeln!(
                io::stderr(),
                "[ERROR][storage] log_write_failed; detailed logs may be incomplete"
            );
        }
    }
}

/// Keep this guard alive until the CLI exits. Dropping flushes the queues;
/// the App entry point intentionally retains its guard for process lifetime.
pub struct LogSession {
    pub directory: PathBuf,
    pub run_id: String,
    pub dispatch: Dispatch,
    guards: Option<(WorkerGuard, WorkerGuard)>,
    failed: Arc<AtomicBool>,
    _lock: File,
}
impl LogSession {
    pub fn start(root: &Path, config: LogConfig) -> io::Result<Self> {
        if !(1024..=64 * 1024 * 1024).contains(&config.max_file_bytes)
            || !(1..=16).contains(&config.retained_files)
        {
            return Err(io::Error::other("invalid_rotation_config"));
        }
        private_dir(root)?;
        let run_id = uuid::Uuid::new_v4().simple().to_string();
        let directory = root.join(format!("session-{run_id}"));
        private_dir(&directory)?;
        let lock = new_private_file(&directory.join(".active"))?;
        fs2::FileExt::try_lock_exclusive(&lock)?;
        prune_sessions(root, &directory, 5)?;
        let failed = Arc::new(AtomicBool::new(false));
        let (events, eg) = NonBlockingBuilder::default()
            .buffered_lines_limit(4096)
            .lossy(false)
            .thread_name("thyou-log-events")
            .finish(RollingWriter::new(
                &directory,
                "events",
                &config,
                failed.clone(),
            )?);
        let (errors, rg) = NonBlockingBuilder::default()
            .buffered_lines_limit(1024)
            .lossy(false)
            .thread_name("thyou-log-errors")
            .finish(RollingWriter::new(
                &directory,
                "errors",
                &config,
                failed.clone(),
            )?);
        let layer = SafeLayer {
            origin: Instant::now(),
            config,
            run_id: run_id.clone(),
            events,
            errors,
            sequence: AtomicU64::new(1),
            failed: failed.clone(),
        };
        let dispatch = Dispatch::new(tracing_subscriber::registry().with(layer));
        Ok(Self {
            directory,
            run_id,
            dispatch,
            guards: Some((eg, rg)),
            failed,
            _lock: lock,
        })
    }
    pub fn healthy(&self) -> bool {
        !self.failed.load(Ordering::Relaxed)
    }
    pub fn flush(&mut self) {
        self.guards.take();
    }
}

fn prune_sessions(root: &Path, current: &Path, keep: usize) -> io::Result<()> {
    let mut directories = fs::read_dir(root)?
        .filter_map(Result::ok)
        .filter(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            name.strip_prefix("session-")
                .is_some_and(|s| s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit()))
                && entry
                    .file_type()
                    .is_ok_and(|t| t.is_dir() && !t.is_symlink())
        })
        .collect::<Vec<_>>();
    directories.sort_by_key(|d| d.metadata().and_then(|m| m.modified()).ok());
    let count = directories.len().saturating_sub(keep);
    for entry in directories.into_iter().take(count) {
        if entry.path() == current {
            continue;
        }
        let path = entry.path().join(".active");
        if fs::symlink_metadata(&path).is_ok_and(|m| m.is_file() && !m.file_type().is_symlink())
            && let Ok(lock) = OpenOptions::new().read(true).write(true).open(path)
            && fs2::FileExt::try_lock_exclusive(&lock).is_ok()
        {
            fs::remove_dir_all(entry.path())?;
        }
    }
    Ok(())
}

static APP_LOGGER: OnceLock<Mutex<Option<LogSession>>> = OnceLock::new();
pub(crate) fn init_app_logging() {
    APP_LOGGER.get_or_init(|| {
        if cfg!(test) {
            return Mutex::new(None);
        }
        let root = std::env::var_os("THYOU_LOG_DIR")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(PathBuf::from).map(|home| {
                    if cfg!(target_os = "macos") {
                        home.join("Library/Logs/THYou")
                    } else {
                        home.join(".local/state/thyou/logs")
                    }
                })
            });
        let spec = std::env::var("THYOU_LOG").unwrap_or_else(|_| "info".into());
        let logger = root.and_then(|root| {
            LogConfig::parse(&spec, false)
                .ok()
                .and_then(|c| LogSession::start(&root, c).ok())
        });
        if let Some(log) = logger {
            if tracing::dispatcher::set_global_default(log.dispatch.clone()).is_ok() {
                return Mutex::new(Some(log));
            }
        }
        let _ = writeln!(
            io::stderr(),
            "[WARN][storage] backend_logger_unavailable; authentication behavior unchanged"
        );
        Mutex::new(None)
    });
}

pub(crate) fn observe<T, E: fmt::Display>(
    service: &'static str,
    operation: &'static str,
    future: impl Future<Output = Result<T, E>>,
) -> impl Future<Output = Result<T, E>> {
    // Box the operation before wrapping it in timing/task-local futures.
    // Boxing only the final observer still copies the complete operation
    // into each nested state machine during construction and polling. In
    // Debug, overview -> session recovery exhausted a default 2 MiB worker
    // stack even before the recovery gate could return without any I/O.
    let future = Box::pin(future);
    let operation_id = next_id();
    let span =
        tracing::info_span!(target:"tsinghua_kit::api","operation",operation,service,operation_id);
    Box::pin(async move {
        struct End { id:u64, start:Instant, done:bool }
        impl Drop for End {
            fn drop(&mut self) {
                if !self.done {
                    tracing::warn!(target:"tsinghua_kit::api",event="operation_finished",operation_id=self.id,
                        outcome="cancelled",duration_us=timing::micros(self.start.elapsed()),duration_ms=self.start.elapsed().as_millis() as u64);
                }
            }
        }
        let mut end=End { id:operation_id,start:Instant::now(),done:false };
        tracing::info!(target:"tsinghua_kit::api",event="operation_started",operation,service,operation_id);
        let result = if timing::Collector::current().is_some() {
            future.await
        } else {
            let (result, metrics) = timing::capture(future).await;
            tracing::info!(target:"tsinghua_kit::api",event="operation_timing",operation,service,operation_id,
                request_count=metrics.requests,
                gate_queue_us=metrics.phase_us(timing::Phase::GateQueue),
                rate_limit_us=metrics.phase_us(timing::Phase::RateLimit),
                headers_us=metrics.phase_us(timing::Phase::ResponseHeaders),
                body_read_us=metrics.phase_us(timing::Phase::ResponseBody),
                decode_us=metrics.phase_us(timing::Phase::Decode),
                cache_read_us=metrics.phase_us(timing::Phase::CacheRead),
                cache_write_us=metrics.phase_us(timing::Phase::CacheWrite),
                auth_bootstrap_us=metrics.phase_us(timing::Phase::AuthBootstrap),
                service_handoff_us=metrics.phase_us(timing::Phase::ServiceHandoff),
                session_recovery_us=metrics.phase_us(timing::Phase::SessionRecovery),
                decoded_text_bytes=metrics.decoded_text_bytes,binary_body_bytes=metrics.binary_body_bytes,
                cache_hits=metrics.cache_hits,cache_misses=metrics.cache_misses);
            result
        };
        end.done=true;
        let duration_us=timing::micros(end.start.elapsed());
        let duration_ms=duration_us/1000;
        match &result {
            Ok(_) => tracing::info!(target:"tsinghua_kit::api",event="operation_finished",operation,service,operation_id,outcome="success",duration_us,duration_ms),
            Err(error) => {
                let text=error.to_string();let reason=diagnostic_reason(&text);
                let category=crate::live_validation::error_category(&text);
                tracing::warn!(target:"tsinghua_kit::api",event="operation_finished",operation,service,operation_id,outcome="failed",reason,category,duration_us,duration_ms);
            }
        }
        result
    }.instrument(span))
}

pub(crate) fn endpoint(url: &reqwest::Url) -> &'static str {
    if crate::request_gate::is_loopback(url) {
        return "loopback";
    }
    match url.host_str().unwrap_or("") {
        "id.tsinghua.edu.cn" => "identity",
        "webvpn.tsinghua.edu.cn" => "webvpn",
        "oauth.tsinghua.edu.cn" => "oauth",
        "learn.tsinghua.edu.cn" => "learn",
        "zhjw.cic.tsinghua.edu.cn" => "registrar",
        "info.tsinghua.edu.cn" => "info",
        "card.tsinghua.edu.cn" => "campus_card",
        "usereg.tsinghua.edu.cn" => "usereg",
        "auth4.tsinghua.edu.cn" | "auth6.tsinghua.edu.cn" => "tunet",
        host if host.ends_with(".tsinghua.edu.cn") => "other_campus",
        _ => "external",
    }
}
pub(crate) fn method(method: &reqwest::Method) -> &'static str {
    match method.as_str() {
        "GET" => "get",
        "POST" => "post",
        "PUT" => "put",
        "DELETE" => "delete",
        "HEAD" => "head",
        "OPTIONS" => "options",
        "PATCH" => "patch",
        _ => "other",
    }
}

#[cfg(test)]
#[path = "telemetry_tests.rs"]
mod tests;
