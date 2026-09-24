//! Redacted end-to-end contracts for Learn and Registrar behind WebVPN paths.
//!
//! These tests use only loopback fixture servers.  The two service profiles
//! deliberately have different mapping prefixes so a leading slash cannot
//! silently erase the Learn or Registrar mapping directory.

use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use reqwest::{StatusCode, Url};
use tsinghua_kit::{
    AcademicStage, CalendarWindow, CampusHttpTransport, CampusLiveConfig, CampusLiveDataSource,
    CampusOverview, CampusOverviewResolver, CourseRole, JsonFileCache, LearnClient,
    LearnClientConfig, LearnClientError, LearnSessionError, LearnSessionOrchestrator,
    LearnTodoConfig, LearnTodoSource, RegistrarClient, RegistrarClientConfig, RegistrarClientError,
    RegistrarSessionOrchestrator, ServiceId, ServiceSessionState, SessionCoordinator, UserIdentity,
};
use uuid::Uuid;

const LEARN_MAPPING_PREFIX: &str = "/https/learn-fixture/";
const REGISTRAR_MAPPING_PREFIX: &str = "/http/registrar-fixture/";
const COOKIE_ROAMING_PATH: &str = "/f/j_spring_security_thauth_roaming_entry";
const NEW_STUDENT_COURSE_HOME_PATH: &str = "/f/wlxt/index/course/student/index";
const LEGACY_STUDENT_COURSE_HOME_PATH: &str = "/f/wlxt/index/course/student/";
const STUDENT_COURSE_PATH: &str =
    "/b/wlxt/kc/v_wlkc_xs_xkb_kcb_extend/student/loadCourseBySemesterId/2025-2026-2/zh";
const TICKET_EXCHANGE_PATH: &str = "/b/wlxt/common/auth/gnt";
const REGISTRAR_LOGIN_PATH: &str = "/j_acegi_login.do";
const REGISTRAR_CALENDAR_PATH: &str = "/jxmh_out.do";
const CALLBACK: &str = "fixtureCalendar";
const CSRF: &str = "learn-csrf-fixture";
const TICKET: &str = "registrar-ticket-fixture";
const E2E_SEMESTER: &str = "2026-2027-1";
const E2E_CSRF_HANDOFF: &str = "learn-csrf-handoff";
const E2E_CSRF_CURRENT: &str = "learn-csrf-current";
const E2E_CSRF_LIST: &str = "learn-csrf-semester-list";
const E2E_TICKET: &str = "registrar-ticket-e2e";
const RUNTIME_CALLBACK: &str = "thyouCalendar";
const CURRENT_SEMESTER_PATH: &str = "/b/kc/zhjw_v_code_xnxq/getCurrentAndNextSemester";
const SEMESTER_LIST_PATH: &str = "/b/wlxt/kc/v_wlkc_xs_xktjb_coassb/queryxnxq";

#[derive(Clone, Debug)]
struct FixtureResponse {
    status: StatusCode,
    content_type: Option<&'static str>,
    headers: Vec<(String, String)>,
    body: String,
}

impl FixtureResponse {
    fn new(status: StatusCode, content_type: Option<&'static str>, body: &str) -> Self {
        Self {
            status,
            content_type,
            headers: Vec::new(),
            body: body.to_owned(),
        }
    }

    fn html(body: &str) -> Self {
        Self::new(StatusCode::OK, Some("text/html; charset=UTF-8"), body)
    }

    fn json(body: &str) -> Self {
        Self::new(StatusCode::OK, Some("application/json"), body)
    }

    fn javascript(body: &str) -> Self {
        Self::new(
            StatusCode::OK,
            Some("application/javascript; charset=UTF-8"),
            body,
        )
    }

    fn redirect(location: &str) -> Self {
        Self::new(StatusCode::FOUND, None, "").with_header("Location", location)
    }

    fn with_header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_owned(), value.to_owned()));
        self
    }
}

#[derive(Clone, Debug)]
struct CapturedRequest {
    method: String,
    target: String,
    headers: Vec<(String, String)>,
    body: String,
}

impl CapturedRequest {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

struct FixtureServer {
    base_url: String,
    join: JoinHandle<Vec<CapturedRequest>>,
}

impl FixtureServer {
    fn start<F>(request_count: usize, handler: F) -> Self
    where
        F: Fn(usize, &CapturedRequest) -> FixtureResponse + Send + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").expect("fixture listener");
        listener
            .set_nonblocking(true)
            .expect("fixture listener becomes nonblocking");
        let address = listener.local_addr().expect("fixture address");
        let join = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut requests = Vec::with_capacity(request_count);
            while requests.len() < request_count && Instant::now() < deadline {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let request = read_request(&mut stream);
                        let response = handler(requests.len(), &request);
                        write_response(&mut stream, &response);
                        requests.push(request);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("fixture accept failed: {error}"),
                }
            }
            assert_eq!(
                requests.len(),
                request_count,
                "fixture did not receive the expected number of requests"
            );
            requests
        });

        Self {
            base_url: format!("http://{address}"),
            join,
        }
    }

    fn finish(self) -> Vec<CapturedRequest> {
        self.join.join().expect("fixture server joins")
    }
}

fn read_request(stream: &mut TcpStream) -> CapturedRequest {
    stream
        .set_nonblocking(false)
        .expect("fixture request socket becomes blocking");
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("fixture request timeout");

    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 4096];
    let header_end = loop {
        let count = stream.read(&mut buffer).expect("fixture request headers");
        assert!(count > 0, "client closed before fixture request headers");
        bytes.extend_from_slice(&buffer[..count]);
        if let Some(position) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break position + 4;
        }
    };

    let header_text = String::from_utf8_lossy(&bytes[..header_end]);
    let mut lines = header_text.split("\r\n");
    let request_line = lines.next().expect("fixture request line");
    let mut parts = request_line.splitn(3, ' ');
    let method = parts.next().expect("fixture method").to_owned();
    let target = parts.next().expect("fixture target").to_owned();
    let headers = lines
        .filter(|line| !line.is_empty())
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            Some((name.to_owned(), value.trim().to_owned()))
        })
        .collect::<Vec<_>>();
    let content_length = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.parse::<usize>().ok())
        .unwrap_or(0);
    while bytes.len() < header_end + content_length {
        let count = stream.read(&mut buffer).expect("fixture request body");
        assert!(count > 0, "client closed before fixture request body");
        bytes.extend_from_slice(&buffer[..count]);
    }

    CapturedRequest {
        method,
        target,
        headers,
        body: String::from_utf8_lossy(&bytes[header_end..header_end + content_length]).into_owned(),
    }
}

fn write_response(stream: &mut TcpStream, response: &FixtureResponse) {
    let reason = response.status.canonical_reason().unwrap_or("Fixture");
    let mut wire = format!(
        "HTTP/1.1 {} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n",
        response.status.as_u16(),
        response.body.len()
    );
    if let Some(content_type) = response.content_type {
        wire.push_str(&format!("Content-Type: {content_type}\r\n"));
    }
    for (name, value) in &response.headers {
        wire.push_str(&format!("{name}: {value}\r\n"));
    }
    wire.push_str("\r\n");
    stream
        .write_all(wire.as_bytes())
        .and_then(|_| stream.write_all(response.body.as_bytes()))
        .expect("fixture response");
}

fn transport() -> CampusHttpTransport {
    CampusHttpTransport::with_timeout("THYou/webvpn-mapped-contract", Duration::from_secs(5))
        .expect("fixture transport")
}

fn mapped_url(base_url: &str, prefix: &str) -> String {
    format!("{base_url}{prefix}")
}

fn learn_client(base_url: &str, course_home_path: &str) -> LearnClient {
    let config = LearnClientConfig::new(base_url, CourseRole::Student)
        .expect("Learn fixture config")
        .with_course_home_path(course_home_path)
        .expect("course home route");
    LearnClient::new(config)
}

fn registrar_client(
    learn_base_url: &str,
    registrar_base_url: &str,
    transport: CampusHttpTransport,
) -> RegistrarClient {
    RegistrarClient::from_transport(
        RegistrarClientConfig {
            learn_base_url: learn_base_url.to_owned(),
            registrar_base_url: registrar_base_url.to_owned(),
            ..RegistrarClientConfig::default()
        },
        transport,
    )
    .expect("Registrar fixture config")
}

fn add_identity_cookie(transport: &CampusHttpTransport, base_url: &str) {
    transport.cookie_jar().add_cookie_str(
        "identity-handoff=present; Path=/",
        &Url::parse(base_url).expect("fixture base URL"),
    );
}

fn assert_cookie(request: &CapturedRequest, marker: &str) {
    assert!(
        request
            .header("cookie")
            .is_some_and(|value| value.contains(marker)),
        "request did not reuse the fixture Cookie jar"
    );
}

fn assert_get(request: &CapturedRequest, target: &str) {
    assert_eq!(request.method, "GET");
    assert_eq!(request.target, target);
}

fn assert_post(request: &CapturedRequest, target: &str) {
    assert_eq!(request.method, "POST");
    assert_eq!(request.target, target);
}

fn date() -> chrono::NaiveDate {
    chrono::NaiveDate::from_ymd_opt(2026, 9, 11).expect("fixture date")
}

fn calendar_window() -> CalendarWindow {
    CalendarWindow::new(date(), date()).expect("fixture calendar window")
}

fn successful_calendar_body() -> &'static str {
    r#"fixtureCalendar([{"grrlID":"event-fixture","nr":"课程甲","nq":"2026-09-11","kssj":"08:00","jssj":"09:35","dd":"地点甲","fl":"必修"}])"#
}

fn temporary_overview_cache() -> JsonFileCache<CampusOverview> {
    let path = std::env::temp_dir().join(format!(
        "thyou-webvpn-overview-{}-{}.json",
        std::process::id(),
        Uuid::new_v4()
    ));
    JsonFileCache::new(
        &path,
        CampusOverviewResolver::CACHE_SCHEMA_VERSION,
        "overview",
    )
}

#[tokio::test]
async fn mapped_learn_and_registrar_handoff_keeps_both_prefixes_and_new_course_home() {
    let expected_course_home =
        mapped_target_from_prefix(LEARN_MAPPING_PREFIX, NEW_STUDENT_COURSE_HOME_PATH);
    let expected_course_list = mapped_target_from_prefix(LEARN_MAPPING_PREFIX, STUDENT_COURSE_PATH);
    let expected_ticket_exchange =
        mapped_target_from_prefix(LEARN_MAPPING_PREFIX, TICKET_EXCHANGE_PATH);
    let expected_registrar_login =
        mapped_target_from_prefix(REGISTRAR_MAPPING_PREFIX, REGISTRAR_LOGIN_PATH);
    let expected_calendar =
        mapped_target_from_prefix(REGISTRAR_MAPPING_PREFIX, REGISTRAR_CALENDAR_PATH);

    let server = FixtureServer::start(6, move |index, request| match index {
        0 => {
            assert_get(
                request,
                &mapped_target_from_prefix(LEARN_MAPPING_PREFIX, COOKIE_ROAMING_PATH),
            );
            assert_cookie(request, "identity-handoff=present");
            FixtureResponse::html("<html><body>Learn handoff</body></html>")
                .with_header("Set-Cookie", "learn-session=issued; Path=/")
        }
        1 => {
            assert_get(request, &expected_course_home);
            assert_cookie(request, "learn-session=issued");
            FixtureResponse::html(&format!(
                "<html><head><meta name=\"_csrf\" content=\"{CSRF}\"></head></html>"
            ))
        }
        2 => {
            assert_get(request, &format!("{expected_course_list}?_csrf={CSRF}"));
            assert_cookie(request, "learn-session=issued");
            FixtureResponse::json(&format!(
                r#"{{"message":"success","resultList":[{{"wlkcid":"course-fixture","kch":"30240001","kcm":"课程甲","semester":"2025-2026-2"}}]}}"#
            ))
        }
        3 => {
            assert_post(request, &format!("{expected_ticket_exchange}?_csrf={CSRF}"));
            assert_eq!(request.body, "appId=ALL_ZHJW");
            assert_cookie(request, "learn-session=issued");
            FixtureResponse::new(StatusCode::OK, Some("text/plain"), &format!("\"{TICKET}\""))
        }
        4 => {
            assert_get(
                request,
                &format!("{expected_registrar_login}?url=%2F&ticket={TICKET}"),
            );
            assert_cookie(request, "learn-session=issued");
            FixtureResponse::html("<html><body>Registrar home</body></html>")
                .with_header("Set-Cookie", "registrar-session=issued; Path=/")
        }
        5 => {
            assert_get(
                request,
                &format!(
                    "{expected_calendar}?m=bks_jxrl_all&p_start_date=20260911&p_end_date=20260911&jsoncallback={CALLBACK}"
                ),
            );
            assert_cookie(request, "registrar-session=issued");
            FixtureResponse::javascript(successful_calendar_body())
        }
        _ => unreachable!("fixture request count bounds the handler"),
    });

    let learn_base_url = mapped_url(&server.base_url, LEARN_MAPPING_PREFIX);
    let registrar_base_url = mapped_url(&server.base_url, REGISTRAR_MAPPING_PREFIX);
    let shared_transport = transport();
    add_identity_cookie(&shared_transport, &learn_base_url);

    let mut learn_session = LearnSessionOrchestrator::with_coordinator(
        learn_client(&learn_base_url, NEW_STUDENT_COURSE_HOME_PATH),
        shared_transport.clone(),
        SessionCoordinator::new(),
    );
    let user = UserIdentity {
        username: "fixture-student".to_owned(),
        display_name: Some("脱敏同学".to_owned()),
    };
    let learn_result = learn_session
        .establish_cookie_backed(user.clone())
        .await
        .expect("mapped Learn handoff");
    assert_eq!(
        learn_result.snapshot.state,
        ServiceSessionState::Authenticated
    );
    let learn_csrf = learn_session
        .coordinator()
        .bound_csrf(ServiceId::Learn)
        .expect("Learn CSRF proof");

    let courses = learn_session
        .learn()
        .fetch_course_records(
            learn_session.transport(),
            &learn_csrf,
            "2025-2026-2",
            Some("zh"),
            None,
        )
        .await
        .expect("mapped course list");
    assert_eq!(courses.len(), 1);
    assert_eq!(courses[0].id(), Some("course-fixture"));

    let registrar = registrar_client(
        &learn_base_url,
        &registrar_base_url,
        learn_session.transport().clone(),
    );
    let registrar_session = RegistrarSessionOrchestrator::new(registrar);
    let registrar_result = registrar_session
        .establish(
            learn_session.coordinator_mut(),
            &learn_csrf,
            user,
            AcademicStage::Undergraduate,
            calendar_window(),
            CALLBACK,
        )
        .await
        .expect("mapped Registrar handoff");
    assert_eq!(
        registrar_result.snapshot.state,
        ServiceSessionState::Authenticated
    );
    assert_eq!(
        learn_session
            .coordinator()
            .registry()
            .snapshot_for(ServiceId::Learn)
            .state,
        ServiceSessionState::Authenticated
    );
    assert_eq!(
        learn_session
            .coordinator()
            .registry()
            .snapshot_for(ServiceId::Registrar)
            .state,
        ServiceSessionState::Authenticated
    );

    let requests = server.finish();
    assert_eq!(requests.len(), 6);
}

#[tokio::test]
async fn authenticated_handoff_loads_a_non_empty_live_overview_with_latest_learn_csrf() {
    let expected_course_home =
        mapped_target_from_prefix(LEARN_MAPPING_PREFIX, NEW_STUDENT_COURSE_HOME_PATH);
    let expected_current = mapped_target_from_prefix(LEARN_MAPPING_PREFIX, CURRENT_SEMESTER_PATH);
    let expected_semester_list =
        mapped_target_from_prefix(LEARN_MAPPING_PREFIX, SEMESTER_LIST_PATH);
    let expected_course_list = mapped_target_from_prefix(
        LEARN_MAPPING_PREFIX,
        &format!(
            "/b/wlxt/kc/v_wlkc_xs_xkb_kcb_extend/student/loadCourseBySemesterId/{E2E_SEMESTER}/zh"
        ),
    );
    let expected_ticket_exchange =
        mapped_target_from_prefix(LEARN_MAPPING_PREFIX, TICKET_EXCHANGE_PATH);
    let expected_registrar_login =
        mapped_target_from_prefix(REGISTRAR_MAPPING_PREFIX, REGISTRAR_LOGIN_PATH);
    let expected_calendar =
        mapped_target_from_prefix(REGISTRAR_MAPPING_PREFIX, REGISTRAR_CALENDAR_PATH);
    let expected_calendar_query =
        "m=bks_jxrl_all&p_start_date=20260911&p_end_date=20260911&jsoncallback=thyouCalendar";

    let server = FixtureServer::start(15, move |index, request| match index {
        0 => {
            assert_get(
                request,
                &mapped_target_from_prefix(LEARN_MAPPING_PREFIX, COOKIE_ROAMING_PATH),
            );
            assert_cookie(request, "identity-handoff=present");
            FixtureResponse::html("<html><body>Learn handoff</body></html>")
                .with_header("Set-Cookie", "learn-session=issued; Path=/")
        }
        1 => {
            assert_get(request, &expected_course_home);
            assert_cookie(request, "learn-session=issued");
            FixtureResponse::html(&format!(
                "<html><head><meta name=\"_csrf\" content=\"{E2E_CSRF_HANDOFF}\"></head></html>"
            ))
        }
        2 => {
            assert_get(request, &expected_course_home);
            assert_cookie(request, "learn-session=issued");
            FixtureResponse::html(&format!(
                "<html><head><meta name=\"_csrf\" content=\"{E2E_CSRF_CURRENT}\"></head></html>"
            ))
        }
        3 => {
            assert_get(
                request,
                &format!("{expected_current}?_csrf={E2E_CSRF_CURRENT}"),
            );
            assert_cookie(request, "learn-session=issued");
            FixtureResponse::json(&format!(
                r#"{{"message":"success","result":{{"id":"{E2E_SEMESTER}","kssj":"2026-09-01 00:00:00","jssj":"2027-01-15 00:00:00","xnxq":"{E2E_SEMESTER}"}}}}"#
            ))
        }
        4 => {
            assert_get(request, &expected_course_home);
            assert_cookie(request, "learn-session=issued");
            FixtureResponse::html(&format!(
                "<html><head><meta name=\"_csrf\" content=\"{E2E_CSRF_LIST}\"></head></html>"
            ))
        }
        5 => {
            assert_get(
                request,
                &format!("{expected_semester_list}?_csrf={E2E_CSRF_LIST}"),
            );
            assert_cookie(request, "learn-session=issued");
            FixtureResponse::json(&format!(r#"["{E2E_SEMESTER}","2025-2026-2"]"#))
        }
        6 => {
            assert_post(
                request,
                &format!("{expected_ticket_exchange}?_csrf={E2E_CSRF_LIST}"),
            );
            assert_eq!(request.body, "appId=ALL_ZHJW");
            assert_cookie(request, "learn-session=issued");
            FixtureResponse::new(
                StatusCode::OK,
                Some("text/plain"),
                &format!("\"{E2E_TICKET}\""),
            )
        }
        7 => {
            assert_get(
                request,
                &format!("{expected_registrar_login}?url=%2F&ticket={E2E_TICKET}"),
            );
            assert_cookie(request, "learn-session=issued");
            FixtureResponse::html("<html><body>Registrar home</body></html>")
                .with_header("Set-Cookie", "registrar-session=issued; Path=/")
        }
        8 => {
            assert_get(
                request,
                &format!("{expected_calendar}?{expected_calendar_query}"),
            );
            assert_cookie(request, "registrar-session=issued");
            FixtureResponse::javascript(
                r#"thyouCalendar([{"grrlID":"event-e2e","nr":"课程甲","nq":"2026-09-11","kssj":"08:00","jssj":"09:35","dd":"地点甲","fl":"必修","kcdm":"30240001","jsxm":"教师甲"}])"#,
            )
        }
        9 => {
            assert_get(
                request,
                &format!("{expected_course_list}?_csrf={E2E_CSRF_LIST}"),
            );
            assert_cookie(request, "learn-session=issued");
            assert_cookie(request, "registrar-session=issued");
            FixtureResponse::json(&format!(
                r#"{{"message":"success","resultList":[{{"wlkcid":"course-e2e","kch":"30240001","kcm":"课程甲","jsxm":"教师甲","semester":"{E2E_SEMESTER}"}}]}}"#
            ))
        }
        10 => {
            assert_get(
                request,
                &format!("{expected_calendar}?{expected_calendar_query}"),
            );
            assert_cookie(request, "registrar-session=issued");
            FixtureResponse::javascript(
                r#"thyouCalendar([{"grrlID":"event-e2e","nr":"课程甲","nq":"2026-09-11","kssj":"08:00","jssj":"09:35","dd":"地点甲","fl":"必修","kcdm":"30240001","jsxm":"教师甲"}])"#,
            )
        }
        11 => {
            assert_get(
                request,
                &format!("{expected_course_list}?_csrf={E2E_CSRF_LIST}"),
            );
            assert_cookie(request, "learn-session=issued");
            assert_cookie(request, "registrar-session=issued");
            FixtureResponse::json(&format!(
                r#"{{"message":"success","resultList":[{{"wlkcid":"course-e2e","kch":"30240001","kcm":"课程甲","jsxm":"教师甲","semester":"{E2E_SEMESTER}"}}]}}"#
            ))
        }
        12..=14 => {
            let homework_path = match index {
                12 => "/b/wlxt/kczy/zy/student/zyListWj",
                13 => "/b/wlxt/kczy/zy/student/zyListYjwg",
                14 => "/b/wlxt/kczy/zy/student/zyListYpg",
                _ => unreachable!("homework fixture index"),
            };
            assert_post(
                request,
                &format!(
                    "{}?_csrf={E2E_CSRF_LIST}",
                    mapped_target_from_prefix(LEARN_MAPPING_PREFIX, homework_path)
                ),
            );
            assert_cookie(request, "learn-session=issued");
            assert_cookie(request, "registrar-session=issued");
            assert!(request.body.contains("name=\"aoData\""));
            assert!(request.body.contains("course-e2e"));
            FixtureResponse::json(r#"{"result":"success","object":{"aaData":[]}}"#)
        }
        _ => unreachable!("fixture request count bounds the handler"),
    });

    let learn_base_url = mapped_url(&server.base_url, LEARN_MAPPING_PREFIX);
    let registrar_base_url = mapped_url(&server.base_url, REGISTRAR_MAPPING_PREFIX);
    let shared_transport = transport();
    add_identity_cookie(&shared_transport, &learn_base_url);
    let user = UserIdentity {
        username: "fixture-student".to_owned(),
        display_name: Some("脱敏同学".to_owned()),
    };

    // The identity session is already authenticated at this point. The rest of
    // the fixture proves that the same Cookie jar can establish both academic
    // services and that the final semester-list CSRF is carried downstream.
    let mut coordinator = SessionCoordinator::new();
    coordinator
        .begin_authentication(ServiceId::Identity)
        .expect("identity authentication begins");
    coordinator
        .mark_authenticated(ServiceId::Identity, user.clone(), None, None, None)
        .expect("identity authentication is proven");

    let mut learn_session = LearnSessionOrchestrator::with_coordinator(
        learn_client(&learn_base_url, NEW_STUDENT_COURSE_HOME_PATH),
        shared_transport.clone(),
        coordinator,
    );
    let learn_result = learn_session
        .establish_cookie_backed(user.clone())
        .await
        .expect("Learn cookie handoff");
    assert_eq!(
        learn_result.snapshot.state,
        ServiceSessionState::Authenticated
    );

    let discovery = learn_session
        .discover_semesters(user.clone())
        .await
        .expect("live semester discovery");
    assert_eq!(discovery.current.id, E2E_SEMESTER);
    assert_eq!(discovery.semester_ids[0], E2E_SEMESTER);
    assert_eq!(
        discovery.final_learn_csrf.as_csrf_token().as_str(),
        E2E_CSRF_LIST
    );
    assert_eq!(
        learn_session
            .coordinator()
            .bound_csrf(ServiceId::Learn)
            .expect("final Learn CSRF")
            .as_csrf_token()
            .as_str(),
        E2E_CSRF_LIST
    );

    let registrar = registrar_client(
        &learn_base_url,
        &registrar_base_url,
        learn_session.transport().clone(),
    );
    let registrar_session = RegistrarSessionOrchestrator::new(registrar);
    registrar_session
        .establish(
            learn_session.coordinator_mut(),
            &discovery.final_learn_csrf,
            user,
            AcademicStage::Undergraduate,
            calendar_window(),
            RUNTIME_CALLBACK,
        )
        .await
        .expect("Registrar handoff and calendar proof");
    let registrar = registrar_session.into_registrar();

    let mut todo_source = LearnTodoSource::new(
        learn_session.learn().clone(),
        learn_session.transport().clone(),
        LearnTodoConfig::new(E2E_SEMESTER).with_language("zh"),
    )
    .expect("live Learn todo source");
    let final_learn_csrf = discovery.final_learn_csrf.clone();
    todo_source
        .with_csrf(final_learn_csrf.clone(), "_csrf")
        .expect("bind final Learn CSRF to todo source");

    let mut source = CampusLiveDataSource::new(
        learn_session.learn().clone(),
        registrar,
        CampusLiveConfig::new(E2E_SEMESTER, AcademicStage::Undergraduate),
        std::sync::Arc::new(todo_source),
    )
    .expect("live campus source");
    source
        .with_learn_csrf(final_learn_csrf, "_csrf")
        .expect("bind final Learn CSRF to campus source");

    let resolver = CampusOverviewResolver::with_cache(
        std::sync::Arc::new(source),
        temporary_overview_cache(),
        Duration::from_secs(60),
    );
    let result = resolver.load(date()).await;
    assert_eq!(result.source, "live");
    assert_eq!(result.status, "ready");
    assert!(result.error.is_none());
    assert!(result.section_errors.is_none());
    let overview = result.overview.expect("non-empty live overview");
    assert_eq!(overview.date, "2026-09-11");
    assert_eq!(overview.semester.as_deref(), Some(E2E_SEMESTER));
    assert_eq!(overview.course_count, 1);
    assert_eq!(overview.today_schedule.len(), 1);
    let schedule = &overview.today_schedule[0];
    assert_eq!(schedule.title, "课程甲");
    assert_eq!(schedule.kind, "course");
    assert_eq!(schedule.starts_at, "2026-09-11T00:00:00+00:00");
    assert_eq!(
        schedule.ends_at.as_deref(),
        Some("2026-09-11T01:35:00+00:00")
    );
    assert_eq!(schedule.location.as_deref(), Some("地点甲"));
    resolver.cache().clear().expect("fixture cache clears");

    assert_eq!(server.finish().len(), 15);
}

#[tokio::test]
async fn learn_course_home_mapping_loss_cannot_promote_the_session() {
    let server = FixtureServer::start(3, |index, request| match index {
        0 => {
            assert_eq!(
                request.target,
                mapped_target_from_prefix(LEARN_MAPPING_PREFIX, COOKIE_ROAMING_PATH)
            );
            FixtureResponse::html("<html><body>handoff</body></html>")
        }
        1 => {
            assert_eq!(
                request.target,
                mapped_target_from_prefix(LEARN_MAPPING_PREFIX, NEW_STUDENT_COURSE_HOME_PATH)
            );
            FixtureResponse::redirect(NEW_STUDENT_COURSE_HOME_PATH)
        }
        2 => {
            assert_eq!(request.target, NEW_STUDENT_COURSE_HOME_PATH);
            FixtureResponse::html(&format!("<meta name=\"_csrf\" content=\"{CSRF}\">"))
        }
        _ => unreachable!("fixture request count bounds the handler"),
    });
    let learn_base_url = mapped_url(&server.base_url, LEARN_MAPPING_PREFIX);
    let transport = transport();
    add_identity_cookie(&transport, &learn_base_url);
    let mut orchestrator = LearnSessionOrchestrator::new(
        learn_client(&learn_base_url, NEW_STUDENT_COURSE_HOME_PATH),
        transport,
    );

    let error = orchestrator
        .establish_cookie_backed(UserIdentity {
            username: "fixture-student".to_owned(),
            display_name: None,
        })
        .await
        .expect_err("a root redirect must not prove the mapped Learn session");
    assert!(matches!(
        error,
        LearnSessionError::Client(LearnClientError::UnexpectedOrigin)
    ));
    assert_eq!(
        orchestrator
            .coordinator()
            .registry()
            .snapshot_for(ServiceId::Learn)
            .state,
        ServiceSessionState::Anonymous
    );
    assert_eq!(server.finish().len(), 3);
}

#[tokio::test]
async fn learn_legacy_home_redirect_to_new_index_is_a_route_drift() {
    let server = FixtureServer::start(2, |index, request| match index {
        0 => {
            assert_eq!(
                request.target,
                mapped_target_from_prefix(LEARN_MAPPING_PREFIX, LEGACY_STUDENT_COURSE_HOME_PATH)
            );
            FixtureResponse::redirect(&mapped_target_from_prefix(
                LEARN_MAPPING_PREFIX,
                NEW_STUDENT_COURSE_HOME_PATH,
            ))
        }
        1 => {
            assert_eq!(
                request.target,
                mapped_target_from_prefix(LEARN_MAPPING_PREFIX, NEW_STUDENT_COURSE_HOME_PATH)
            );
            FixtureResponse::html(&format!("<meta name=\"_csrf\" content=\"{CSRF}\">"))
        }
        _ => unreachable!("fixture request count bounds the handler"),
    });
    let learn_base_url = mapped_url(&server.base_url, LEARN_MAPPING_PREFIX);
    let client = learn_client(&learn_base_url, LEGACY_STUDENT_COURSE_HOME_PATH);
    let error = client
        .execute_course_home(&transport())
        .await
        .expect_err("legacy home redirect must be visible as route drift");
    assert!(matches!(error, LearnClientError::UnexpectedPath));
    assert_eq!(server.finish().len(), 2);
}

#[tokio::test]
async fn mapped_registrar_mapping_loss_cannot_become_a_valid_empty_calendar() {
    let server = FixtureServer::start(2, |index, request| match index {
        0 => {
            assert_eq!(
                request.target,
                mapped_calendar_target(REGISTRAR_MAPPING_PREFIX)
            );
            FixtureResponse::redirect(REGISTRAR_CALENDAR_PATH)
        }
        1 => {
            assert_eq!(request.target, REGISTRAR_CALENDAR_PATH);
            FixtureResponse::javascript("fixtureCalendar([])")
        }
        _ => unreachable!("fixture request count bounds the handler"),
    });
    let registrar_base_url = mapped_url(&server.base_url, REGISTRAR_MAPPING_PREFIX);
    let registrar = registrar_client(&registrar_base_url, &registrar_base_url, transport());
    let error = registrar
        .fetch_calendar_window(AcademicStage::Undergraduate, calendar_window(), CALLBACK)
        .await
        .expect_err("mapping loss must not become callback([]) success");
    assert!(matches!(error, RegistrarClientError::UnexpectedOrigin));
    assert_eq!(server.finish().len(), 2);
}

#[tokio::test]
async fn mapped_registrar_redirect_to_another_route_is_not_calendar_success() {
    let server = FixtureServer::start(2, |index, request| match index {
        0 => {
            assert_eq!(
                request.target,
                mapped_calendar_target(REGISTRAR_MAPPING_PREFIX)
            );
            FixtureResponse::redirect("/http/registrar-fixture/unrelated.do")
        }
        1 => {
            assert_eq!(request.target, "/http/registrar-fixture/unrelated.do");
            FixtureResponse::javascript(successful_calendar_body())
        }
        _ => unreachable!("fixture request count bounds the handler"),
    });
    let registrar_base_url = mapped_url(&server.base_url, REGISTRAR_MAPPING_PREFIX);
    let registrar = registrar_client(&registrar_base_url, &registrar_base_url, transport());
    let error = registrar
        .fetch_calendar_window(AcademicStage::Undergraduate, calendar_window(), CALLBACK)
        .await
        .expect_err("mapped sibling route must not be parsed as calendar data");
    assert!(matches!(
        error,
        RegistrarClientError::InvalidConfig { message }
            if message == "calendar response ended outside the requested route"
    ));
    assert_eq!(server.finish().len(), 2);
}

#[tokio::test]
async fn mapped_registrar_http_200_login_page_is_expired_before_jsonp_decode() {
    let server = FixtureServer::start(1, |_index, request| {
        assert_eq!(
            request.target,
            mapped_calendar_target(REGISTRAR_MAPPING_PREFIX)
        );
        FixtureResponse::html(
            "<html><form action=\"/j_acegi_login.do\"><input name=\"password\"></form></html>",
        )
    });
    let registrar_base_url = mapped_url(&server.base_url, REGISTRAR_MAPPING_PREFIX);
    let registrar = registrar_client(&registrar_base_url, &registrar_base_url, transport());
    let error = registrar
        .fetch_calendar_window(AcademicStage::Undergraduate, calendar_window(), CALLBACK)
        .await
        .expect_err("HTTP 200 login HTML must not reach JSONP parsing");
    assert!(matches!(error, RegistrarClientError::LoginExpired { .. }));
    assert_eq!(server.finish().len(), 1);
}

#[tokio::test]
async fn mapped_registrar_jsonp_login_envelope_is_expired_before_empty_success() {
    let server = FixtureServer::start(1, |_index, request| {
        assert_eq!(
            request.target,
            mapped_calendar_target(REGISTRAR_MAPPING_PREFIX)
        );
        FixtureResponse::javascript(r#"fixtureCalendar({"status":401,"message":"请先登录"})"#)
    });
    let registrar_base_url = mapped_url(&server.base_url, REGISTRAR_MAPPING_PREFIX);
    let registrar = registrar_client(&registrar_base_url, &registrar_base_url, transport());
    let error = registrar
        .fetch_calendar_window(AcademicStage::Undergraduate, calendar_window(), CALLBACK)
        .await
        .expect_err("JSONP authentication envelope must not become empty data");
    assert!(matches!(error, RegistrarClientError::LoginExpired { .. }));
    assert_eq!(server.finish().len(), 1);
}

fn mapped_target_from_prefix(prefix: &str, route: &str) -> String {
    format!("{}{}", prefix.trim_end_matches('/'), route)
}

fn mapped_calendar_target(prefix: &str) -> String {
    format!(
        "{}{}?m=bks_jxrl_all&p_start_date=20260911&p_end_date=20260911&jsoncallback={CALLBACK}",
        prefix.trim_end_matches('/'),
        REGISTRAR_CALENDAR_PATH
    )
}
