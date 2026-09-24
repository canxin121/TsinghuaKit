use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    thread::{self, JoinHandle},
};

use chrono::NaiveDate;
use reqwest::{Method, StatusCode, Url};
use tsinghua_kit::{
    AcademicStage, CalendarWindow, CampusHttpTransport, RegistrarClient, RegistrarClientConfig,
    RegistrarClientError, RegistrarGradesProfile, RegistrarSessionError,
    RegistrarSessionOrchestrator, ServiceId, ServiceSessionState, SessionCoordinator,
    UndergraduateReportKind, UserIdentity,
};
use tsinghua_kit::{
    protocol::CsrfToken,
    registrar_client::registrar_exam::{
        RegistrarExamError, RegistrarExamHttpResponse, RegistrarExamRecordSource,
        RegistrarExamStage, RegistrarVerifiedExamPageProfile, parse_verified_exam_page_html,
        parse_verified_exam_page_response,
    },
    registrar_client::{decode_calendar_jsonp, parse_all_zhjw_ticket_response},
};

const EXAM_HTML: &str = r#"
<html><body>
<table id="exam">
  <tr><th>开课系</th><th>课程号</th><th>课序号</th><th>课程名</th><th>课程分类</th><th>教师</th><th>人数</th><th>考试日期</th><th>考场</th></tr>
  <tr><td>计算机系</td><td>30240043</td><td>01</td><td>程序设计基础</td><td>本科生</td><td>教师甲</td><td>30</td><td>06.22一 下午</td><td>六教6A201</td></tr>
</table>
</body></html>
"#;

const EXAM_HEADER_ONLY_HTML: &str = r#"
<html><body><table id="exam">
  <tr><th>开课系</th><th>课程号</th><th>课序号</th><th>课程名</th><th>课程分类</th><th>教师</th><th>人数</th><th>考试日期</th><th>考场</th></tr>
</table></body></html>
"#;

const GRADE_HTML: &str = r#"
<html><body><table cellspacing="1">
  <tr><th>序号</th><th>课程号</th><th>课序号</th><th>课程名称</th><th>课程性质</th><th>学分</th><th>考试方式</th><th>成绩</th><th>补考</th><th>绩点</th><th>备注</th><th>学期</th></tr>
  <tr><td>1</td><td>30240043</td><td>01</td><td>程序设计基础</td><td>必修</td><td>3</td><td>正考</td><td>90</td><td></td><td>4.0</td><td></td><td>2025-2026-1</td></tr>
</table></body></html>
"#;

const GRADE_HEADER_ONLY_HTML: &str = r#"
<html><body><table cellspacing="1">
  <tr><th>序号</th><th>课程号</th><th>课序号</th><th>课程名称</th><th>课程性质</th><th>学分</th><th>考试方式</th><th>成绩</th><th>补考</th><th>绩点</th><th>备注</th><th>学期</th></tr>
</table></body></html>
"#;

#[derive(Debug, Clone)]
struct FixtureResponse {
    status: StatusCode,
    content_type: Option<&'static str>,
    headers: Vec<(String, String)>,
    body: String,
}

impl FixtureResponse {
    fn ok(content_type: &'static str, body: impl Into<String>) -> Self {
        Self {
            status: StatusCode::OK,
            content_type: Some(content_type),
            headers: Vec::new(),
            body: body.into(),
        }
    }

    fn redirect(location: &str) -> Self {
        Self {
            status: StatusCode::FOUND,
            content_type: None,
            headers: vec![("Location".to_owned(), location.to_owned())],
            body: String::new(),
        }
    }

    fn with_header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_owned(), value.to_owned()));
        self
    }

    fn with_status(mut self, status: StatusCode) -> Self {
        self.status = status;
        self
    }
}

#[derive(Debug, Clone)]
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
    fn start(responses: Vec<FixtureResponse>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("fixture listener");
        let address = listener.local_addr().expect("fixture address");
        let join = thread::spawn(move || {
            let mut requests = Vec::with_capacity(responses.len());
            for response in responses {
                let (mut stream, _) = listener.accept().expect("fixture request");
                requests.push(read_request(&mut stream));
                write_response(&mut stream, &response);
            }
            requests
        });

        Self {
            base_url: format!("http://{address}"),
            join,
        }
    }

    fn finish(self) -> Vec<CapturedRequest> {
        self.join.join().expect("fixture server").to_vec()
    }
}

fn read_request(stream: &mut TcpStream) -> CapturedRequest {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 1024];
    let header_end = loop {
        let count = stream.read(&mut buffer).expect("fixture request bytes");
        assert!(count > 0, "fixture client closed before request headers");
        bytes.extend_from_slice(&buffer[..count]);
        if let Some(position) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break position + 4;
        }
    };

    let header_text = String::from_utf8_lossy(&bytes[..header_end]);
    let mut lines = header_text.split("\r\n");
    let request_line = lines.next().expect("fixture request line");
    let mut request_parts = request_line.splitn(3, ' ');
    let method = request_parts.next().expect("fixture method").to_owned();
    let target = request_parts.next().expect("fixture target").to_owned();
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
        assert!(count > 0, "fixture client closed before request body");
        bytes.extend_from_slice(&buffer[..count]);
    }

    CapturedRequest {
        method,
        target,
        headers,
        body: String::from_utf8_lossy(&bytes[header_end..header_end + content_length]).into_owned(),
    }
}

fn write_response(stream: &mut TcpStream, fixture: &FixtureResponse) {
    let reason = fixture.status.canonical_reason().unwrap_or("Fixture");
    let mut response = format!(
        "HTTP/1.1 {} {reason}\r\nConnection: close\r\nContent-Length: {}\r\n",
        fixture.status.as_u16(),
        fixture.body.len()
    );
    if let Some(content_type) = fixture.content_type {
        response.push_str(&format!("Content-Type: {content_type}\r\n"));
    }
    for (name, value) in &fixture.headers {
        response.push_str(&format!("{name}: {value}\r\n"));
    }
    response.push_str("\r\n");
    stream
        .write_all(response.as_bytes())
        .and_then(|_| stream.write_all(fixture.body.as_bytes()))
        .expect("fixture response");
}

fn client(base_url: &str) -> RegistrarClient {
    let transport = CampusHttpTransport::with_timeout(
        "THYou/registrar-contract-test",
        std::time::Duration::from_secs(5),
    )
    .expect("transport");
    RegistrarClient::from_transport(
        RegistrarClientConfig {
            learn_base_url: base_url.to_owned(),
            registrar_base_url: base_url.to_owned(),
            ..RegistrarClientConfig::default()
        },
        transport,
    )
    .expect("registrar client")
}

fn add_fixture_cookie(client: &RegistrarClient, base_url: &str) {
    let url = Url::parse(base_url).expect("fixture URL");
    client
        .transport()
        .cookie_jar()
        .add_cookie_str("registrar-session=present; Path=/", &url);
}

fn date(day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 9, day).expect("fixture date")
}

fn window(day: u32) -> CalendarWindow {
    CalendarWindow::new(date(day), date(day)).expect("fixture window")
}

fn exam_response(
    status: StatusCode,
    url: &str,
    content_type: Option<&str>,
    body: &str,
) -> RegistrarExamHttpResponse {
    RegistrarExamHttpResponse::new(
        status,
        Url::parse(url).expect("exam response URL"),
        content_type.map(str::to_owned),
        body,
    )
}

#[test]
fn verified_exam_profile_has_the_independently_confirmed_request_shape() {
    let profile = RegistrarVerifiedExamPageProfile::undergraduate();
    let request = profile.request();
    assert_eq!(profile.stage(), RegistrarExamStage::Undergraduate);
    assert!(profile.is_configured());
    assert_eq!(request.method, Method::GET);
    assert_eq!(request.path, "/jxmh.do");
    assert_eq!(request.query, [("url", "/jxmh.do"), ("m", "bks_ksSearch")]);

    let endpoint = request
        .endpoint_url("https://registrar.example.test")
        .expect("verified endpoint");
    assert_eq!(endpoint.path(), "/jxmh.do");
    assert_eq!(endpoint.query(), Some("url=%2Fjxmh.do&m=bks_ksSearch"));
    assert!(matches!(
        RegistrarVerifiedExamPageProfile::for_stage(RegistrarExamStage::Graduate),
        Err(RegistrarExamError::UnsupportedGraduateRoute)
    ));
}

#[tokio::test]
async fn ticket_login_and_exam_page_use_real_request_lines_and_shared_cookie_session() {
    let server = FixtureServer::start(vec![
        FixtureResponse::ok("text/plain", r#""fixture-ticket""#)
            .with_header("Set-Cookie", "registrar-session=issued; Path=/"),
        FixtureResponse::ok("text/html", "<html><body>registrar home</body></html>"),
        FixtureResponse::ok("text/html; charset=UTF-8", EXAM_HTML),
    ]);
    let registrar = client(&server.base_url);

    let raw_ticket = registrar
        .exchange_all_zhjw_ticket(Some("fixture-csrf"))
        .await
        .expect("ticket exchange");
    assert_eq!(
        parse_all_zhjw_ticket_response(&raw_ticket)
            .expect("ticket envelope")
            .as_str(),
        "fixture-ticket"
    );
    registrar
        .establish_registrar_session("fixture-ticket")
        .await
        .expect("registrar login");
    let records = registrar.fetch_exam_page().await.expect("exam page");

    assert_eq!(records.len(), 1);
    let record = &records[0];
    assert_eq!(record.source, RegistrarExamRecordSource::HtmlPage);
    assert_eq!(record.course_code, "30240043");
    assert_eq!(record.course_sequence, "01");
    assert_eq!(record.course_name, "程序设计基础");
    assert_eq!(record.schedule.month, 6);
    assert_eq!(record.schedule.day, 22);
    assert_eq!(record.location, "六教6A201");
    assert_eq!(record.headcount, Some(30));

    let requests = server.finish();
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[0].method, "POST");
    assert_eq!(
        requests[0].target,
        "/b/wlxt/common/auth/gnt?_csrf=fixture-csrf"
    );
    assert_eq!(requests[0].body, "appId=ALL_ZHJW");
    assert_eq!(requests[1].method, "GET");
    assert_eq!(
        requests[1].target,
        "/j_acegi_login.do?url=%2F&ticket=fixture-ticket"
    );
    assert!(
        requests[1]
            .header("cookie")
            .is_some_and(|value| value.contains("registrar-session=issued"))
    );
    assert_eq!(requests[2].method, "GET");
    assert_eq!(requests[2].target, "/jxmh.do?url=%2Fjxmh.do&m=bks_ksSearch");
    assert!(
        requests[2]
            .header("cookie")
            .is_some_and(|value| value.contains("registrar-session=issued"))
    );
}

#[tokio::test]
async fn ticket_exchange_rejects_a_redirect_that_drops_the_confirmed_query() {
    let server = FixtureServer::start(vec![
        FixtureResponse::redirect("/b/wlxt/common/auth/gnt"),
        FixtureResponse::ok("text/plain", r#""fixture-ticket""#),
    ]);
    let registrar = client(&server.base_url);

    let error = registrar
        .exchange_all_zhjw_ticket(Some("fixture-csrf"))
        .await
        .expect_err("ticket exchange must retain its confirmed CSRF query");
    assert!(matches!(
        error,
        RegistrarClientError::InvalidConfig { message }
            if message == "ticket exchange response did not retain the confirmed query"
    ));

    let requests = server.finish();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[0].target,
        "/b/wlxt/common/auth/gnt?_csrf=fixture-csrf"
    );
    assert_eq!(requests[1].target, "/b/wlxt/common/auth/gnt");
}

#[tokio::test]
async fn ticket_exchange_rejects_a_redirect_that_changes_raw_query_encoding() {
    let server = FixtureServer::start(vec![
        FixtureResponse::redirect("/b/wlxt/common/auth/gnt?_csrf=csrf/value"),
        FixtureResponse::ok("text/plain", r#""fixture-ticket""#),
    ]);
    let registrar = client(&server.base_url);

    let error = registrar
        .exchange_all_zhjw_ticket(Some("csrf/value"))
        .await
        .expect_err("a redirect must retain the confirmed raw query encoding");
    assert!(matches!(
        error,
        RegistrarClientError::InvalidConfig { message }
            if message == "ticket exchange response did not retain the confirmed query"
    ));

    let requests = server.finish();
    assert_eq!(
        requests[0].target,
        "/b/wlxt/common/auth/gnt?_csrf=csrf%2Fvalue"
    );
    assert_eq!(
        requests[1].target,
        "/b/wlxt/common/auth/gnt?_csrf=csrf/value"
    );
}

#[tokio::test]
async fn registrar_reads_require_http_200_even_when_the_body_looks_valid() {
    let ticket_server = FixtureServer::start(vec![
        FixtureResponse::ok("text/plain", r#""fixture-ticket""#).with_status(StatusCode::CREATED),
    ]);
    let ticket_registrar = client(&ticket_server.base_url);
    let ticket_error = ticket_registrar
        .exchange_all_zhjw_ticket(None)
        .await
        .expect_err("a 201 ticket response must not become a ticket");
    assert!(matches!(
        ticket_error,
        RegistrarClientError::Transport(
            tsinghua_kit::TransportError::HttpStatus { status, .. }
        ) if status == StatusCode::CREATED
    ));
    ticket_server.finish();

    let login_server = FixtureServer::start(vec![
        FixtureResponse::ok("text/html", "<html><body>registrar home</body></html>")
            .with_status(StatusCode::CREATED),
    ]);
    let login_registrar = client(&login_server.base_url);
    let login_error = login_registrar
        .establish_registrar_session("fixture-ticket")
        .await
        .expect_err("a 201 handoff response must not establish a session");
    assert!(matches!(
        login_error,
        RegistrarClientError::Transport(
            tsinghua_kit::TransportError::HttpStatus { status, .. }
        ) if status == StatusCode::CREATED
    ));
    login_server.finish();

    let calendar_server = FixtureServer::start(vec![
        FixtureResponse::ok("application/javascript", "fixtureCb([])")
            .with_status(StatusCode::PARTIAL_CONTENT),
    ]);
    let calendar_registrar = client(&calendar_server.base_url);
    add_fixture_cookie(&calendar_registrar, &calendar_server.base_url);
    let calendar_error = calendar_registrar
        .fetch_calendar_window(AcademicStage::Undergraduate, window(11), "fixtureCb")
        .await
        .expect_err("a 206 calendar response must not become an empty result");
    assert!(matches!(
        calendar_error,
        RegistrarClientError::Transport(
            tsinghua_kit::TransportError::HttpStatus { status, .. }
        ) if status == StatusCode::PARTIAL_CONTENT
    ));
    calendar_server.finish();

    let grades_server = FixtureServer::start(vec![
        FixtureResponse::ok("text/html; charset=UTF-8", GRADE_HTML)
            .with_status(StatusCode::NO_CONTENT),
    ]);
    let grades_registrar = client(&grades_server.base_url);
    add_fixture_cookie(&grades_registrar, &grades_server.base_url);
    let grades_error = grades_registrar
        .fetch_grades(RegistrarGradesProfile::undergraduate(
            UndergraduateReportKind::FirstDegree,
        ))
        .await
        .expect_err("a 204 grade response must not become a grade report");
    assert!(matches!(
        grades_error,
        RegistrarClientError::Transport(
            tsinghua_kit::TransportError::HttpStatus { status, .. }
        ) if status == StatusCode::NO_CONTENT
    ));
    grades_server.finish();
}

#[tokio::test]
async fn verified_exam_header_only_response_is_a_real_empty_result() {
    let server = FixtureServer::start(vec![FixtureResponse::ok(
        "text/html; charset=UTF-8",
        EXAM_HEADER_ONLY_HTML,
    )]);
    let registrar = client(&server.base_url);
    add_fixture_cookie(&registrar, &server.base_url);

    let records = registrar
        .fetch_verified_exam_page(RegistrarExamStage::Undergraduate)
        .await
        .expect("header-only exam table is empty");
    assert!(records.is_empty());

    let requests = server.finish();
    assert_eq!(requests[0].method, "GET");
    assert_eq!(requests[0].target, "/jxmh.do?url=%2Fjxmh.do&m=bks_ksSearch");
    assert!(
        requests[0]
            .header("cookie")
            .is_some_and(|value| value.contains("registrar-session=present"))
    );
}

#[test]
fn verified_exam_parser_separates_origin_route_query_auth_status_type_and_malformed_rows() {
    let request = RegistrarVerifiedExamPageProfile::undergraduate().request();
    let valid_url = "https://registrar.example.test/jxmh.do?url=%2Fjxmh.do&m=bks_ksSearch";
    let cases = [
        (
            exam_response(
                StatusCode::OK,
                "https://foreign.example.test/jxmh.do?url=%2Fjxmh.do&m=bks_ksSearch",
                Some("text/html"),
                EXAM_HTML,
            ),
            RegistrarExamError::UnexpectedOrigin,
        ),
        (
            exam_response(
                StatusCode::OK,
                "https://registrar.example.test/moved?url=%2Fjxmh.do&m=bks_ksSearch",
                Some("text/html"),
                EXAM_HTML,
            ),
            RegistrarExamError::UnexpectedPath,
        ),
        (
            exam_response(
                StatusCode::OK,
                "https://registrar.example.test/jxmh.do",
                Some("text/html"),
                EXAM_HTML,
            ),
            RegistrarExamError::MissingConfirmedQuery,
        ),
        (
            exam_response(
                StatusCode::OK,
                "https://registrar.example.test/j_acegi_login.do?url=%2Fjxmh.do",
                Some("text/html"),
                "<html><body>请先登录</body></html>",
            ),
            RegistrarExamError::AuthenticationRequired,
        ),
        (
            exam_response(
                StatusCode::BAD_GATEWAY,
                valid_url,
                Some("text/html"),
                EXAM_HTML,
            ),
            RegistrarExamError::HttpStatus {
                status: StatusCode::BAD_GATEWAY.as_u16(),
            },
        ),
        (
            exam_response(
                StatusCode::OK,
                valid_url,
                Some("application/json"),
                EXAM_HTML,
            ),
            RegistrarExamError::InvalidHtmlContentType,
        ),
    ];

    for (response, expected) in cases {
        let error = parse_verified_exam_page_response(
            "https://registrar.example.test",
            &request,
            &response,
        )
        .expect_err("fixture boundary must fail closed");
        assert_eq!(error, expected);
    }

    assert!(matches!(
        parse_verified_exam_page_html("<table><tr><th>开课系</th>"),
        Err(RegistrarExamError::MalformedHtml { .. })
    ));
    let invalid_row = EXAM_HTML.replace("30240043", "not-a-course");
    assert!(matches!(
        parse_verified_exam_page_html(&invalid_row),
        Err(RegistrarExamError::InvalidHtmlRow { .. })
    ));
}

#[tokio::test]
async fn verified_exam_login_redirect_is_classified_as_expired_session() {
    let server = FixtureServer::start(vec![
        FixtureResponse::redirect("/j_acegi_login.do?url=%2Fjxmh.do"),
        FixtureResponse::ok("text/html", "<html><body>统一身份认证登录页</body></html>"),
    ]);
    let registrar = client(&server.base_url);

    let error = registrar
        .fetch_verified_exam_page(RegistrarExamStage::Undergraduate)
        .await
        .expect_err("login redirect must not parse as an empty exam list");
    assert!(matches!(
        error,
        RegistrarClientError::InvalidExamResponse {
            source: RegistrarExamError::AuthenticationRequired
        }
    ));
    let requests = server.finish();
    assert_eq!(requests[0].target, "/jxmh.do?url=%2Fjxmh.do&m=bks_ksSearch");
    assert_eq!(requests[1].target, "/j_acegi_login.do?url=%2Fjxmh.do");
}

#[tokio::test]
async fn grades_use_the_confirmed_route_query_cookie_and_allow_empty_tables() {
    let server = FixtureServer::start(vec![FixtureResponse::ok(
        "text/html; charset=UTF-8",
        GRADE_HTML,
    )]);
    let registrar = client(&server.base_url);
    add_fixture_cookie(&registrar, &server.base_url);

    let report = registrar
        .fetch_grades(RegistrarGradesProfile::undergraduate(
            UndergraduateReportKind::FirstDegree,
        ))
        .await
        .expect("grade report");
    assert_eq!(report.courses.len(), 1);
    assert_eq!(report.courses[0].course_name, "程序设计基础");
    assert_eq!(report.courses[0].credit, 3.0);
    assert_eq!(report.courses[0].grade, "90");
    assert_eq!(report.courses[0].grade_point, Some(4.0));
    assert_eq!(report.courses[0].semester, "2025-2026-1");

    let requests = server.finish();
    assert_eq!(requests[0].method, "GET");
    assert_eq!(
        requests[0].target,
        "/cj.cjCjbAll.do?m=bks_cjdcx&cjdlx=zw&flag=di1"
    );
    assert!(
        requests[0]
            .header("cookie")
            .is_some_and(|value| value.contains("registrar-session=present"))
    );

    let empty_server = FixtureServer::start(vec![FixtureResponse::ok(
        "text/html; charset=UTF-8",
        GRADE_HEADER_ONLY_HTML,
    )]);
    let empty_registrar = client(&empty_server.base_url);
    let empty_report = empty_registrar
        .fetch_grades(RegistrarGradesProfile::undergraduate(
            UndergraduateReportKind::FirstDegree,
        ))
        .await
        .expect("header-only grade table is empty");
    assert!(empty_report.courses.is_empty());
    empty_server.finish();
}

#[tokio::test]
async fn grade_login_redirect_and_wrong_content_type_are_not_business_successes() {
    let login_server = FixtureServer::start(vec![
        FixtureResponse::redirect("/j_acegi_login.do?url=%2F"),
        FixtureResponse::ok("text/html", "<html><body>请先登录</body></html>"),
    ]);
    let login_registrar = client(&login_server.base_url);
    let login_error = login_registrar
        .fetch_grades(RegistrarGradesProfile::graduate())
        .await
        .expect_err("grade login redirect");
    assert!(matches!(
        login_error,
        RegistrarClientError::LoginExpired { .. }
    ));
    login_server.finish();

    let type_server =
        FixtureServer::start(vec![FixtureResponse::ok("application/json", GRADE_HTML)]);
    let type_registrar = client(&type_server.base_url);
    let type_error = type_registrar
        .fetch_grades(RegistrarGradesProfile::graduate())
        .await
        .expect_err("wrong grade content type");
    assert!(matches!(
        type_error,
        RegistrarClientError::InvalidGradeContentType
    ));
    type_server.finish();
}

#[tokio::test]
async fn calendar_jsonp_uses_28_day_request_shape_and_preserves_empty_result() {
    let server = FixtureServer::start(vec![FixtureResponse::ok(
        "application/javascript; charset=UTF-8",
        r#"fixtureCb([{"grrlID":"fixture-event","nr":"高等数学","nq":"2026-09-11","kssj":"08:00","jssj":"09:35","dd":"六教6A013","fl":"必修"}])"#,
    )]);
    let registrar = client(&server.base_url);
    add_fixture_cookie(&registrar, &server.base_url);

    let record = registrar
        .fetch_calendar_window(AcademicStage::Undergraduate, window(11), "fixtureCb")
        .await
        .expect("calendar JSONP");
    assert_eq!(record.events.len(), 1);
    assert_eq!(record.events[0].id.as_deref(), Some("fixture-event"));
    assert_eq!(record.events[0].title, "高等数学");

    let requests = server.finish();
    assert_eq!(requests[0].method, "GET");
    assert_eq!(
        requests[0].target,
        "/jxmh_out.do?m=bks_jxrl_all&p_start_date=20260911&p_end_date=20260911&jsoncallback=fixtureCb"
    );
    assert!(
        requests[0]
            .header("cookie")
            .is_some_and(|value| value.contains("registrar-session=present"))
    );

    let empty_server = FixtureServer::start(vec![FixtureResponse::ok(
        "application/javascript",
        "fixtureCb([])",
    )]);
    let empty_registrar = client(&empty_server.base_url);
    let empty = empty_registrar
        .fetch_calendar_window(AcademicStage::Graduate, window(11), "fixtureCb")
        .await
        .expect("empty calendar JSONP");
    assert!(empty.events.is_empty());
    let empty_requests = empty_server.finish();
    assert_eq!(
        empty_requests[0].target,
        "/jxmh_out.do?m=yjs_jxrl_all&p_start_date=20260911&p_end_date=20260911&jsoncallback=fixtureCb"
    );
}

#[test]
fn calendar_jsonp_errors_are_classified_before_unsupported_shapes_become_success() {
    let current_window = window(11);
    let business_error = decode_calendar_jsonp(
        r#"fixtureCb({"success":false,"message":"教务服务暂时不可用"})"#,
        "fixtureCb",
        AcademicStage::Undergraduate,
        current_window.clone(),
    )
    .expect_err("explicit business failure");
    assert!(matches!(
        business_error,
        RegistrarClientError::InvalidBusinessPayload { .. }
    ));

    let login_error = decode_calendar_jsonp(
        r#"fixtureCb({"status":401,"message":"请先登录"})"#,
        "fixtureCb",
        AcademicStage::Undergraduate,
        current_window.clone(),
    )
    .expect_err("login envelope");
    assert!(matches!(
        login_error,
        RegistrarClientError::LoginExpired { .. }
    ));

    let unsupported = decode_calendar_jsonp(
        "fixtureCb({\"data\":[]})",
        "fixtureCb",
        AcademicStage::Undergraduate,
        current_window.clone(),
    )
    .expect_err("unconfirmed wrapper");
    assert!(matches!(
        unsupported,
        RegistrarClientError::InvalidBusinessPayload { .. }
    ));

    let malformed = decode_calendar_jsonp(
        "fixtureCb({\"data\":)",
        "fixtureCb",
        AcademicStage::Undergraduate,
        current_window,
    )
    .expect_err("malformed JSONP");
    assert!(matches!(
        malformed,
        RegistrarClientError::InvalidJsonp { .. }
    ));
}

#[tokio::test]
async fn registrar_session_proof_binds_the_learning_user_before_network_access() {
    let registrar = client("http://127.0.0.1:9");
    let orchestrator = RegistrarSessionOrchestrator::new(registrar);
    let mut coordinator = SessionCoordinator::new();
    let authenticated_user = UserIdentity {
        username: "student-a".to_owned(),
        display_name: Some("同学甲".to_owned()),
    };
    coordinator
        .begin_authentication(ServiceId::Learn)
        .expect("learn auth begins");
    let learn_csrf = coordinator.registry().bind_csrf(
        ServiceId::Learn,
        CsrfToken::new("fixture-learn-csrf").expect("fixture csrf"),
    );
    coordinator
        .mark_authenticated(
            ServiceId::Learn,
            authenticated_user,
            None,
            Some(learn_csrf.clone()),
            None,
        )
        .expect("learn auth completes");

    let error = orchestrator
        .establish(
            &mut coordinator,
            &learn_csrf,
            UserIdentity {
                username: "student-b".to_owned(),
                display_name: Some("同学乙".to_owned()),
            },
            AcademicStage::Undergraduate,
            window(11),
            "fixtureCb",
        )
        .await
        .expect_err("different users must not share the Learn ticket");
    assert!(matches!(error, RegistrarSessionError::UserBindingMismatch));
    assert_eq!(
        coordinator
            .registry()
            .snapshot_for(ServiceId::Registrar)
            .state,
        ServiceSessionState::Anonymous
    );
}
