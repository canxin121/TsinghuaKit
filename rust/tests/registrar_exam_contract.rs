use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    thread::{self, JoinHandle},
};

#[path = "../src/registrar_exam.rs"]
mod registrar_exam;

use registrar_exam::{
    RegistrarExamCourseQuery, RegistrarExamError, RegistrarExamHttpResponse,
    RegistrarExamPageProfile, RegistrarExamRecordSource, RegistrarExamStage, RegistrarExamTerm,
    RegistrarVerifiedExamPageProfile, examination_lookup_is_configured, parse_exam_course_jsonp,
    parse_exam_course_response, parse_exam_page_html, parse_exam_page_response,
    parse_verified_exam_page_html, parse_verified_exam_page_response,
};
use reqwest::{StatusCode, Url};
use tsinghua_kit::registrar_client::registrar_exam as live_exam;
use tsinghua_kit::{CampusHttpTransport, RegistrarClient, RegistrarClientConfig};

const BASE_URL: &str = "https://registrar.example.test";

const EXAM_HTML: &str = r#"
<html><body>
<table class="biaodan_table">
  <tr><th>开课系</th><th>课程号</th><th>课序号</th><th>课程名</th><th>课程分类</th><th>教师</th><th>人数</th><th>考试日期</th><th>考场</th></tr>
  <tr><td>计算机系</td><td>30240043</td><td>01</td><td>程序设计基础</td><td>本科生</td><td>教师甲</td><td>30</td><td>06.22一 下午</td><td>六教6A201</td></tr>
</table>
</body></html>
"#;

const EXAM_HEADER_ONLY_HTML: &str = r#"
<html><body><table class="biaodan_table">
  <tr><th>开课系</th><th>课程号</th><th>课序号</th><th>课程名</th><th>课程分类</th><th>教师</th><th>人数</th><th>考试日期</th><th>考场</th></tr>
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

    fn with_header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_owned(), value.to_owned()));
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
        self.join.join().expect("fixture server")
    }
}

fn read_request(stream: &mut TcpStream) -> CapturedRequest {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 1024];
    let header_end = loop {
        let count = stream.read(&mut buffer).expect("fixture request headers");
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

fn valid_query() -> RegistrarExamCourseQuery {
    RegistrarExamCourseQuery::new(
        "30240043",
        "0",
        RegistrarExamTerm::parse("2025-2026-2").expect("term"),
        "examCallback",
    )
    .expect("query")
}

fn client(base_url: &str) -> RegistrarClient {
    let transport = CampusHttpTransport::with_timeout(
        "THYou/registrar-exam-contract",
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
    .expect("client")
}

fn response(
    status: StatusCode,
    url: &str,
    content_type: &str,
    body: &str,
) -> RegistrarExamHttpResponse {
    RegistrarExamHttpResponse::new(
        status,
        Url::parse(url).expect("response URL"),
        Some(content_type.to_owned()),
        body,
    )
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
fn examination_capability_is_enabled_only_for_the_confirmed_undergraduate_page() {
    assert!(examination_lookup_is_configured());

    let undergraduate = RegistrarExamPageProfile::undergraduate();
    assert_eq!(undergraduate.stage(), RegistrarExamStage::Undergraduate);
    assert!(undergraduate.is_configured());

    assert!(RegistrarExamPageProfile::for_stage(RegistrarExamStage::Undergraduate).is_ok());
    assert!(matches!(
        RegistrarExamPageProfile::for_stage(RegistrarExamStage::Graduate),
        Err(RegistrarExamError::UnsupportedGraduateRoute)
    ));
}

#[test]
fn confirmed_page_and_course_requests_emit_the_exact_wire_contract() {
    let page_request = RegistrarExamPageProfile::undergraduate().request();
    assert_eq!(page_request.path, "/jxmh.do");
    assert_eq!(page_request.query[1], ("m", "bks_ksSearch"));
    assert_eq!(
        page_request
            .endpoint_url(BASE_URL)
            .expect("confirmed page endpoint")
            .as_str(),
        "https://registrar.example.test/jxmh.do?url=%2Fjxmh.do&m=bks_ksSearch"
    );
    assert!(matches!(
        page_request.endpoint_url("not a URL"),
        Err(RegistrarExamError::InvalidBaseUrl { .. })
    ));

    let query = valid_query();
    assert!(query.is_configured());
    let course_request = query.request();
    assert_eq!(course_request.method, reqwest::Method::GET);
    assert_eq!(course_request.path, "/jxmh.do");
    assert_eq!(
        course_request.query,
        vec![
            ("m".to_owned(), "bks_ksSearch".to_owned()),
            ("kch".to_owned(), "30240043".to_owned()),
            ("kxh".to_owned(), "0".to_owned()),
            ("p_xnxq".to_owned(), "2025-2026-2".to_owned()),
            ("jsoncallback".to_owned(), "examCallback".to_owned()),
        ]
    );
    assert_eq!(
        course_request
            .endpoint_url(BASE_URL)
            .expect("course endpoint")
            .as_str(),
        "https://registrar.example.test/jxmh.do?m=bks_ksSearch&kch=30240043&kxh=0&p_xnxq=2025-2026-2&jsoncallback=examCallback"
    );

    assert!(matches!(
        course_request.endpoint_url("not a URL"),
        Err(RegistrarExamError::InvalidBaseUrl { .. })
    ));

    let mut foreign_request = course_request.clone();
    foreign_request.path = "/foreign.do";
    assert!(matches!(
        foreign_request.endpoint_url(BASE_URL),
        Err(RegistrarExamError::InvalidRequest { .. })
    ));
}

#[test]
fn course_query_validation_is_strict_before_request_construction() {
    let term = RegistrarExamTerm::parse("2025-2026-2").expect("term");
    let query = RegistrarExamCourseQuery::new("30240043", "0", term, "examCallback")
        .expect("syntactically valid candidate query");
    assert_eq!(query.course_code(), "30240043");
    assert_eq!(query.course_sequence(), "0");
    assert_eq!(query.callback(), "examCallback");
    assert_eq!(query.term().to_string(), "2025-2026-2");
    assert!(query.is_configured());

    assert!(matches!(
        RegistrarExamCourseQuery::new("3024004", "0", term, "examCallback"),
        Err(RegistrarExamError::InvalidCourseCode)
    ));
    assert!(matches!(
        RegistrarExamCourseQuery::new("30240043", "0", term, "exam-callback"),
        Err(RegistrarExamError::InvalidCallback)
    ));
}

#[test]
fn confirmed_exam_html_success_is_promoted_only_after_the_page_contract() {
    let exam_html = r#"
      <table id="exam">
        <tr>
          <th>开课系</th><th>课程号</th><th>课序号</th><th>课程名</th>
          <th>课程分类</th><th>教师</th><th>人数</th><th>考试日期</th><th>考场</th>
        </tr>
        <tr>
          <td>计算机系</td><td>30240043</td><td>0</td><td>程序设计基础</td>
          <td>本科生</td><td>教师甲</td><td>30</td><td>06.22一 下午</td><td>六教6A201</td>
        </tr>
      </table>
    "#;

    let records = parse_exam_page_html(exam_html).expect("confirmed page table parses");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].course_code, "30240043");
    assert_eq!(records[0].course_sequence, "0");
    assert_eq!(records[0].location, "六教6A201");

    let records = parse_exam_page_response(
        BASE_URL,
        &response(
            StatusCode::OK,
            "https://registrar.example.test/jxmh.do?url=%2Fjxmh.do&m=bks_ksSearch",
            "text/html; charset=UTF-8",
            exam_html,
        ),
    )
    .expect("confirmed page response parses");
    assert_eq!(records.len(), 1);
}

#[test]
fn calendar_and_grade_tables_are_never_misclassified_as_exam_records() {
    let calendar_html = r#"
      <table>
        <tr><th>星期一</th><th>课程</th><th>类别</th></tr>
        <tr><td>一</td><td>程序设计基础</td><td>考试</td></tr>
      </table>
    "#;
    let grade_html = r#"
      <table cellspacing="1">
        <tr><th>序号</th><th>课程名</th><th>成绩</th></tr>
        <tr><td>1</td><td>程序设计基础</td><td>90</td></tr>
      </table>
    "#;

    for body in [calendar_html, grade_html] {
        assert!(matches!(
            parse_exam_page_html(body),
            Err(RegistrarExamError::MissingExamTable)
        ));
        assert!(matches!(
            parse_exam_page_response(
                BASE_URL,
                &response(
                    StatusCode::OK,
                    "https://registrar.example.test/jxmh_out.do?m=bks_jxrl_all",
                    "text/html",
                    body,
                ),
            ),
            Err(RegistrarExamError::UnexpectedPath)
        ));
    }
}

#[test]
fn course_responses_classify_login_business_origin_and_status_failures() {
    let query = valid_query();
    let request = query.request();
    let valid_url = "https://registrar.example.test/jxmh.do?m=bks_ksSearch&kch=30240043&kxh=0&p_xnxq=2025-2026-2&jsoncallback=examCallback";

    let cases = [
        (
            StatusCode::OK,
            valid_url,
            "application/javascript",
            r#"examCallback({"success":false,"message":"暂无考试安排"})"#,
            "business",
        ),
        (
            StatusCode::OK,
            valid_url,
            "application/javascript",
            r#"examCallback({"code":401,"message":"请先登录"})"#,
            "login",
        ),
        (
            StatusCode::OK,
            "https://foreign.example.test/jxmh.do?m=bks_ksSearch&kch=30240043&kxh=0&p_xnxq=2025-2026-2&jsoncallback=examCallback",
            "application/javascript",
            "examCallback([])",
            "origin",
        ),
        (
            StatusCode::NOT_FOUND,
            valid_url,
            "text/plain",
            "route moved",
            "status",
        ),
    ];

    for (status, url, content_type, body, kind) in cases {
        let captured = response(status, url, content_type, body);
        let error = parse_exam_course_response(BASE_URL, &request, &captured)
            .expect_err("invalid course response must fail closed");
        match kind {
            "business" => assert!(matches!(error, RegistrarExamError::BusinessFailure { .. })),
            "login" => assert_eq!(error, RegistrarExamError::AuthenticationRequired),
            "origin" => assert_eq!(error, RegistrarExamError::UnexpectedOrigin),
            "status" => assert_eq!(
                error,
                RegistrarExamError::HttpStatus {
                    status: StatusCode::NOT_FOUND.as_u16()
                }
            ),
            _ => unreachable!("unknown fixture kind"),
        }
    }

    let html = response(StatusCode::OK, valid_url, "text/html", "examCallback([])");
    assert_eq!(
        parse_exam_course_response(BASE_URL, &request, &html),
        Err(RegistrarExamError::JsonpHtmlContentType)
    );

    let unknown = response(
        StatusCode::OK,
        valid_url,
        "application/octet-stream",
        "examCallback([])",
    );
    assert_eq!(
        parse_exam_course_response(BASE_URL, &request, &unknown),
        Err(RegistrarExamError::InvalidJsonpContentType)
    );
}

#[test]
fn course_jsonp_accepts_records_and_empty_arrays_but_rejects_bad_shapes() {
    let query = valid_query();
    let request = query.request();
    let valid_body = r#"examCallback([{"kch":"30240043","kxh":"0","kcm":"程序设计基础","ksrq":"06.22一 下午","ksdd":"六教6A201","kkdw":"计算机系","jsxm":"教师甲","rs":"30"}])"#;

    let records =
        parse_exam_course_jsonp(valid_body, "examCallback", &request).expect("valid course JSONP");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].source, RegistrarExamRecordSource::CourseJsonp);
    assert_eq!(records[0].course_code, "30240043");
    assert_eq!(records[0].course_sequence, "0");
    assert_eq!(records[0].course_name, "程序设计基础");
    assert_eq!(records[0].location, "六教6A201");
    assert_eq!(records[0].department.as_deref(), Some("计算机系"));
    assert_eq!(records[0].instructor.as_deref(), Some("教师甲"));
    assert_eq!(records[0].headcount, Some(30));

    assert!(
        parse_exam_course_jsonp(" examCallback( [] ) ", "examCallback", &request)
            .expect("empty course JSONP")
            .is_empty()
    );

    let payloads = [
        r#"examCallback([{"kxh":"0","kcm":"程序设计基础","ksrq":"06.22一 下午","ksdd":"六教6A201"}])"#,
        r#"examCallback([{"kch":"30240043","kcm":"程序设计基础","ksrq":"06.22一 下午","ksdd":"六教6A201"}])"#,
        r#"examCallback([{"kch":"30240043","kxh":"0","ksrq":"06.22一 下午","ksdd":"六教6A201"}])"#,
        r#"examCallback([{"kch":"30240043","kxh":"0","kcm":"程序设计基础","ksdd":"六教6A201"}])"#,
        r#"examCallback([{"kch":"30240043","kxh":"0","kcm":"程序设计基础","ksrq":"06.22一 下午"}])"#,
        r#"examCallback([{"kch":"30240043","kxh":"0","kcm":"程序设计基础","ksrq":"06.22一 下午","ksdd":"六教6A201"},{"kch":"30240044","kxh":"0","kcm":"另一门","ksrq":"06.22一 下午","ksdd":"六教6A202"}])"#,
        r#"examCallback([{"kch":"30240044","kxh":"0","kcm":"另一门","ksrq":"06.22一 下午","ksdd":"六教6A202"}])"#,
        r#"examCallback([null])"#,
        r#"examCallback({"success":false,"message":"暂无考试安排"})"#,
        r#"examCallback({"data":)"#,
        "examCallback([]);examCallback([])",
    ];

    for body in payloads {
        assert!(parse_exam_course_jsonp(body, "examCallback", &request).is_err());
    }
}

#[tokio::test]
async fn course_jsonp_request_reuses_the_registrar_cookie_from_ticket_login() {
    let server = FixtureServer::start(vec![
        FixtureResponse::ok(
            "text/html; charset=UTF-8",
            "<html><body>registrar home</body></html>",
        )
        .with_header("Set-Cookie", "registrar-session=issued; Path=/"),
        FixtureResponse::ok(
            "application/javascript; charset=UTF-8",
            r#"examCallback([{"kch":"30240043","kxh":"0","kcm":"程序设计基础","ksrq":"06.22一 下午","ksdd":"六教6A201"}])"#,
        ),
    ]);
    let registrar = client(&server.base_url);

    registrar
        .establish_registrar_session("fixture-ticket")
        .await
        .expect("ticket login");
    let live_query = live_exam::RegistrarExamCourseQuery::new(
        "30240043",
        "0",
        live_exam::RegistrarExamTerm::parse("2025-2026-2").expect("term"),
        "examCallback",
    )
    .expect("live query");
    let records = registrar
        .fetch_exam_course(&live_query)
        .await
        .expect("course JSONP");
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].source,
        live_exam::RegistrarExamRecordSource::CourseJsonp
    );

    let requests = server.finish();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].method, "GET");
    assert_eq!(
        requests[0].target,
        "/j_acegi_login.do?url=%2F&ticket=fixture-ticket"
    );
    assert_eq!(requests[1].method, "GET");
    assert_eq!(
        requests[1].target,
        "/jxmh.do?m=bks_ksSearch&kch=30240043&kxh=0&p_xnxq=2025-2026-2&jsoncallback=examCallback"
    );
    assert!(
        requests[1]
            .header("cookie")
            .is_some_and(|value| value.contains("registrar-session=issued"))
    );
    assert!(requests[1].body.is_empty(), "JSONP GET must have no body");
}

#[test]
fn schedule_parser_uses_the_same_strict_grammar_as_the_verified_page() {
    let schedule = registrar_exam::RegistrarExamSchedule::parse("06.22一 下午")
        .expect("confirmed page schedule parses");
    assert_eq!(schedule.month, 6);
    assert_eq!(schedule.day, 22);
    assert_eq!(
        schedule.session,
        registrar_exam::RegistrarExamSession::Afternoon
    );
    assert!(matches!(
        registrar_exam::RegistrarExamSchedule::parse("6.22一 下午"),
        Err(RegistrarExamError::InvalidSchedule)
    ));
}

#[test]
fn response_debug_redacts_body_and_query() {
    let captured = RegistrarExamHttpResponse::new(
        StatusCode::OK,
        Url::parse("https://registrar.example.test/jxmh.do?token=PRIVATE_QUERY_VALUE")
            .expect("URL"),
        Some("text/html".to_owned()),
        "PRIVATE_RESPONSE_BODY",
    );
    let debug = format!("{captured:?}");
    assert!(!debug.contains("PRIVATE_QUERY_VALUE"));
    assert!(!debug.contains("PRIVATE_RESPONSE_BODY"));
    assert!(debug.contains("body_len"));
}

#[test]
fn verified_undergraduate_profile_emits_the_confirmed_get_query_and_no_body() {
    let profile = RegistrarVerifiedExamPageProfile::undergraduate();
    assert_eq!(profile.stage(), RegistrarExamStage::Undergraduate);
    assert!(profile.is_configured());

    let request = profile.request();
    assert_eq!(request.method, reqwest::Method::GET);
    assert_eq!(request.path, "/jxmh.do");
    assert_eq!(request.query, [("url", "/jxmh.do"), ("m", "bks_ksSearch")]);
    assert_eq!(
        request
            .endpoint_url(BASE_URL)
            .expect("confirmed endpoint")
            .as_str(),
        "https://registrar.example.test/jxmh.do?url=%2Fjxmh.do&m=bks_ksSearch"
    );
}

#[test]
fn verified_exam_parser_accepts_the_cross_checked_nine_column_table_only() {
    let records = parse_verified_exam_page_html(EXAM_HTML).expect("verified exam table");
    assert_eq!(records.len(), 1);
    let record = &records[0];
    assert_eq!(record.course_code, "30240043");
    assert_eq!(record.course_sequence, "01");
    assert_eq!(record.course_name, "程序设计基础");
    assert_eq!(record.schedule.month, 6);
    assert_eq!(record.schedule.day, 22);
    assert_eq!(record.location, "六教6A201");
    assert_eq!(record.headcount, Some(30));

    let spaced_schedule = EXAM_HTML.replace("06.22一 下午", "06.22 一 下午");
    let spaced = parse_verified_exam_page_html(&spaced_schedule).expect("spaced schedule");
    assert_eq!(spaced[0].schedule.raw, "06.22 一 下午");

    let extra_cell = EXAM_HTML.replace(
        "<td>六教6A201</td>",
        "<td>六教6A201</td><td>unexpected</td>",
    );
    assert!(matches!(
        parse_verified_exam_page_html(&extra_cell),
        Err(RegistrarExamError::InvalidHtmlRow { .. })
    ));
}

#[test]
fn verified_exam_empty_and_business_failure_shapes_are_distinct() {
    let empty = parse_verified_exam_page_html(EXAM_HEADER_ONLY_HTML)
        .expect("header-only table is a legal empty result");
    assert!(empty.is_empty());

    let failure = parse_verified_exam_page_html(
        "<html><body><div class=error>考试查询失败：系统异常</div></body></html>",
    )
    .expect_err("explicit upstream failure must not become an empty list");
    assert!(matches!(
        failure,
        RegistrarExamError::BusinessFailure { .. }
    ));
}

#[test]
fn verified_exam_response_requires_exact_origin_path_query_status_and_html() {
    let request = RegistrarVerifiedExamPageProfile::undergraduate().request();
    let valid_url = "https://registrar.example.test/jxmh.do?url=%2Fjxmh.do&m=bks_ksSearch";
    let cases = [
        (
            response(
                StatusCode::OK,
                "https://foreign.example.test/jxmh.do?url=%2Fjxmh.do&m=bks_ksSearch",
                "text/html",
                EXAM_HTML,
            ),
            RegistrarExamError::UnexpectedOrigin,
        ),
        (
            response(
                StatusCode::OK,
                "https://registrar.example.test/moved?url=%2Fjxmh.do&m=bks_ksSearch",
                "text/html",
                EXAM_HTML,
            ),
            RegistrarExamError::UnexpectedPath,
        ),
        (
            response(
                StatusCode::OK,
                "https://registrar.example.test/jxmh.do?m=bks_ksSearch&url=%2Fjxmh.do",
                "text/html",
                EXAM_HTML,
            ),
            RegistrarExamError::MissingConfirmedQuery,
        ),
        (
            response(StatusCode::NO_CONTENT, valid_url, "text/html", EXAM_HTML),
            RegistrarExamError::HttpStatus {
                status: StatusCode::NO_CONTENT.as_u16(),
            },
        ),
        (
            response(StatusCode::OK, valid_url, "application/json", EXAM_HTML),
            RegistrarExamError::InvalidHtmlContentType,
        ),
        (
            response(
                StatusCode::OK,
                "https://registrar.example.test/timeout.jsp",
                "text/html",
                "<html>用户登陆超时</html>",
            ),
            RegistrarExamError::AuthenticationRequired,
        ),
    ];

    for (captured, expected) in cases {
        let error = parse_verified_exam_page_response(BASE_URL, &request, &captured)
            .expect_err("response must fail closed");
        assert_eq!(error, expected);
    }

    let mut malformed_request = request.clone();
    malformed_request.method = reqwest::Method::POST;
    assert!(matches!(
        parse_verified_exam_page_response(
            BASE_URL,
            &malformed_request,
            &response(StatusCode::OK, valid_url, "text/html", EXAM_HTML),
        ),
        Err(RegistrarExamError::InvalidRequest { .. })
    ));
}

#[test]
fn registrar_https_timeout_downgrade_is_classified_as_authentication_expiry() {
    let request = RegistrarVerifiedExamPageProfile::undergraduate().request();
    let error = parse_verified_exam_page_response(
        "https://zhjw.cic.tsinghua.edu.cn",
        &request,
        &exam_response(
            StatusCode::OK,
            "http://zhjw.cic.tsinghua.edu.cn/timeout.jsp;jsessionid=synthetic",
            Some("text/html; charset=gbk"),
            "<html><body>用户登陆超时，请重新登录</body></html>",
        ),
    )
    .expect_err("the legacy Registrar timeout redirect is not an exam result");
    assert_eq!(error, RegistrarExamError::AuthenticationRequired);
}

#[tokio::test]
async fn verified_exam_page_reuses_the_registrar_cookie_from_ticket_login() {
    let server = FixtureServer::start(vec![
        FixtureResponse::ok(
            "text/html; charset=UTF-8",
            "<html><body>registrar home</body></html>",
        )
        .with_header("Set-Cookie", "registrar-session=issued; Path=/"),
        FixtureResponse::ok("text/html; charset=UTF-8", EXAM_HTML),
    ]);
    let transport = CampusHttpTransport::with_timeout(
        "THYou/registrar-exam-contract",
        std::time::Duration::from_secs(5),
    )
    .expect("transport");
    let client = RegistrarClient::from_transport(
        RegistrarClientConfig {
            learn_base_url: server.base_url.clone(),
            registrar_base_url: server.base_url.clone(),
            ..RegistrarClientConfig::default()
        },
        transport,
    )
    .expect("registrar client");

    client
        .establish_registrar_session("fixture-ticket")
        .await
        .expect("ticket login page");
    let records = client
        .fetch_verified_exam_page(live_exam::RegistrarExamStage::Undergraduate)
        .await
        .expect("verified exam page");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].course_code, "30240043");

    let requests = server.finish();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].method, "GET");
    assert_eq!(
        requests[0].target,
        "/j_acegi_login.do?url=%2F&ticket=fixture-ticket"
    );
    assert_eq!(requests[1].method, "GET");
    assert_eq!(requests[1].target, "/jxmh.do?url=%2Fjxmh.do&m=bks_ksSearch");
    assert!(
        requests[1]
            .header("cookie")
            .is_some_and(|value| value.contains("registrar-session=issued"))
    );
    assert!(
        requests[1].body.is_empty(),
        "confirmed GET has no request body"
    );
}

#[test]
fn verified_exam_route_has_no_graduate_or_calendar_fallback() {
    assert!(matches!(
        RegistrarVerifiedExamPageProfile::for_stage(RegistrarExamStage::Graduate),
        Err(RegistrarExamError::UnsupportedGraduateRoute)
    ));

    let calendar = r#"
      <table><tr><th>星期一</th><th>课程</th><th>类别</th></tr>
      <tr><td>一</td><td>程序设计基础</td><td>考试</td></tr></table>
    "#;
    let grades = r#"
      <table cellspacing="1"><tr><th>序号</th><th>课程名</th><th>成绩</th></tr>
      <tr><td>1</td><td>程序设计基础</td><td>90</td></tr></table>
    "#;
    for body in [calendar, grades] {
        assert!(matches!(
            parse_verified_exam_page_html(body),
            Err(RegistrarExamError::MissingExamTable)
        ));
    }
}
