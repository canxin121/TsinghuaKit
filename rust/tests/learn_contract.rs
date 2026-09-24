use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use reqwest::StatusCode;

use tsinghua_kit::{
    CampusHttpTransport, CourseRole, CsrfToken, LearnAnnouncementError,
    LearnAnnouncementParseError, LearnAnnouncementSource, LearnClient, LearnClientConfig,
    LearnClientError, LearnCourseRecord, LearnError, LearnTodoConfig, LearnTodoSource,
    ServiceError, ServiceId, TodoFilter,
    campus_live::{CampusTodoFailureKind, CampusTodoSource},
    learn_announcements::{
        LearnAnnouncementBucket, LearnAnnouncementRequestMethod, STUDENT_ACTIVE_PATH,
        STUDENT_EXPIRED_PATH, TEACHER_ACTIVE_PATH, TEACHER_EXPIRED_PATH,
    },
    learn_client::{CourseJsonError, SemesterParseError, parse_current_semester},
    learn_todos::{HomeworkBucket, parse_homework_list},
    parse_announcement_list,
};

const STUDENT_COURSE_PATH: &str =
    "/b/wlxt/kc/v_wlkc_xs_xkb_kcb_extend/student/loadCourseBySemesterId/2025-2026-2/zh";
const TEACHER_COURSE_PATH_0: &str = "/b/kc/v_wlkc_kcb/queryAsorCoCourseList/2025-2026-2/0";
const TEACHER_COURSE_PATH_1: &str = "/b/kc/v_wlkc_kcb/queryAsorCoCourseList/2025-2026-2/1";
const CURRENT_SEMESTER_PATH: &str = "/b/kc/zhjw_v_code_xnxq/getCurrentAndNextSemester";
const COURSE_HOME_PATH: &str = "/f/wlxt/index/course/student/index";

#[derive(Clone, Debug)]
struct FixtureResponse {
    status: u16,
    reason: String,
    content_type: Option<String>,
    headers: Vec<(String, String)>,
    body: String,
}

impl FixtureResponse {
    fn json(body: impl Into<String>) -> Self {
        Self {
            status: 200,
            reason: "OK".to_owned(),
            content_type: Some("application/json".to_owned()),
            headers: Vec::new(),
            body: body.into(),
        }
    }

    fn html(body: impl Into<String>) -> Self {
        Self {
            status: 200,
            reason: "OK".to_owned(),
            content_type: Some("text/html; charset=UTF-8".to_owned()),
            headers: Vec::new(),
            body: body.into(),
        }
    }

    fn redirect(location: impl Into<String>) -> Self {
        Self {
            status: 302,
            reason: "Found".to_owned(),
            content_type: None,
            headers: vec![("Location".to_owned(), location.into())],
            body: String::new(),
        }
    }

    fn with_header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_owned(), value.to_owned()));
        self
    }
}

fn spawn_fixture<F>(request_count: usize, handler: F) -> (String, JoinHandle<Vec<String>>)
where
    F: Fn(usize, &str) -> FixtureResponse + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").expect("fixture listener");
    listener
        .set_nonblocking(true)
        .expect("fixture listener becomes nonblocking");
    let address = listener.local_addr().expect("fixture listener address");
    let base_url = format!("http://{address}/");
    let server = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut requests = Vec::new();
        while requests.len() < request_count && Instant::now() < deadline {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let request = read_http_request(&mut stream);
                    let response = handler(requests.len(), &request);
                    write_http_response(&mut stream, &response);
                    requests.push(request);
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("fixture accept failed: {error}"),
            }
        }
        requests
    });
    (base_url, server)
}

fn write_http_response(stream: &mut TcpStream, response: &FixtureResponse) {
    let mut headers = format!(
        "HTTP/1.1 {} {}\r\nContent-Length: {}\r\nConnection: close\r\n",
        response.status,
        response.reason,
        response.body.as_bytes().len()
    );
    if let Some(content_type) = &response.content_type {
        headers.push_str(&format!("Content-Type: {content_type}\r\n"));
    }
    for (name, value) in &response.headers {
        headers.push_str(&format!("{name}: {value}\r\n"));
    }
    headers.push_str("\r\n");
    stream
        .write_all(headers.as_bytes())
        .expect("fixture response headers write");
    stream
        .write_all(response.body.as_bytes())
        .expect("fixture response body write");
}

fn read_http_request(stream: &mut TcpStream) -> String {
    // `spawn_fixture` uses a non-blocking listener so its accept loop can
    // enforce a deadline.  On some platforms accepted sockets inherit that
    // mode; restore blocking reads before applying the request timeout so an
    // otherwise valid client request is not mistaken for a protocol failure.
    stream
        .set_nonblocking(false)
        .expect("fixture request socket becomes blocking");
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("fixture request timeout");
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 4096];
    let mut expected_len = None;
    loop {
        let read = stream.read(&mut chunk).expect("fixture request read");
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..read]);
        if expected_len.is_none() {
            if let Some(header_end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&bytes[..header_end]);
                expected_len = headers.lines().find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                });
            }
        }
        if let Some(expected_len) = expected_len {
            let header_end = bytes
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .expect("fixture request headers");
            if bytes.len() >= header_end + 4 + expected_len {
                break;
            }
        } else if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

fn request_header<'a>(request: &'a str, name: &str) -> Option<&'a str> {
    request.lines().find_map(|line| {
        let (header_name, value) = line.split_once(':')?;
        header_name
            .eq_ignore_ascii_case(name)
            .then_some(value.trim())
    })
}

fn assert_ao_data_part(request: &str, expected_course_id: &str) {
    let content_type =
        request_header(request, "content-type").expect("multipart request has a content type");
    assert!(content_type.starts_with("multipart/form-data;"));
    let boundary = content_type
        .split(';')
        .find_map(|parameter| {
            let (name, value) = parameter.trim().split_once('=')?;
            name.eq_ignore_ascii_case("boundary")
                .then_some(value.trim_matches('"'))
        })
        .expect("multipart content type has a boundary");
    assert!(!boundary.is_empty());

    let body = request
        .split_once("\r\n\r\n")
        .map(|(_, body)| body)
        .expect("HTTP request has a body separator");
    let delimiter = format!("--{boundary}");
    assert!(body.starts_with(&format!("{delimiter}\r\n")));
    assert!(body.ends_with(&format!("\r\n{delimiter}--\r\n")));
    let part = body
        .split(&delimiter)
        .find(|part| part.contains("name=\"aoData\""))
        .expect("multipart request contains the aoData part");
    let value = part
        .split_once("\r\n\r\n")
        .map(|(_, value)| value.trim())
        .expect("aoData part has a value");
    let ao_data: serde_json::Value = serde_json::from_str(value).expect("aoData is JSON");
    assert_eq!(
        ao_data,
        serde_json::json!([{"name":"wlkcid","value":expected_course_id}])
    );
    assert!(value.find("page").is_none());
    assert!(value.find("start").is_none());
    assert!(value.find("length").is_none());
}

fn learn_csrf(value: &str) -> tsinghua_kit::BoundCsrfToken {
    let registry = tsinghua_kit::SessionRegistry::new();
    registry.bind_csrf(
        ServiceId::Learn,
        CsrfToken::new(value).expect("fixture CSRF token"),
    )
}

fn transport() -> CampusHttpTransport {
    CampusHttpTransport::with_timeout("THYou/learn-contract", Duration::from_secs(5))
        .expect("fixture transport")
}

fn student_client(base_url: &str) -> LearnClient {
    LearnClient::new(
        LearnClientConfig::new(base_url, CourseRole::Student).expect("student Learn config"),
    )
}

fn teacher_client(base_url: &str) -> LearnClient {
    LearnClient::new(
        LearnClientConfig::new(base_url, CourseRole::Teacher).expect("teacher Learn config"),
    )
}

#[test]
fn current_semester_requires_the_observed_success_envelope() {
    let current = parse_current_semester(
        r#"{"message":"success","result":{"id":"2025-2026-2","kssj":"2026-02-23 00:00:00","jssj":"2026-07-12 00:00:00","xnxq":"2025-2026-2"}}"#,
    )
    .expect("verified current semester fixture");
    assert_eq!(current.id, "2025-2026-2");

    let numeric_status = r#"{"resultCode":"0","result":{"id":"2025-2026-2","kssj":"2026-02-23","jssj":"2026-07-12","xnxq":"2025-2026-2"}}"#;
    assert!(matches!(
        parse_current_semester(numeric_status),
        Err(SemesterParseError::CurrentFailure { .. })
    ));
    assert!(matches!(
        parse_current_semester("{"),
        Err(SemesterParseError::Decode(_))
    ));
}

#[test]
fn announcement_collection_fallback_and_flags_follow_the_observed_wire_types() {
    let records = parse_announcement_list(
        r#"{"result":"success","object":{"aaData":null,"resultsList":[{"ggid":"announcement-1","bt":"公告","fbsj":"2026-09-12","sfyd":"否","sfqd":"1","sfsc":"是"}]}}"#,
        "7001",
        tsinghua_kit::LearnAnnouncementBucket::Active,
    )
    .expect("null aaData falls back to resultsList");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].read, Some(false));
    assert_eq!(records[0].important, Some(true));
    assert_eq!(records[0].favorited, Some(true));

    for (field, value) in [("sfyd", "true"), ("sfsc", "0"), ("sfqd", "是")] {
        let body = format!(
            r#"{{"result":"success","object":{{"aaData":[{{"ggid":"announcement-1","bt":"公告","fbsj":"2026-09-12","{field}":{value}}}]}}}}"#,
            value = if field == "sfyd" {
                format!("\"{value}\"")
            } else if field == "sfsc" {
                value.to_owned()
            } else {
                format!("\"{value}\"")
            }
        );
        assert!(matches!(
            parse_announcement_list(&body, "7001", tsinghua_kit::LearnAnnouncementBucket::Active,),
            Err(LearnAnnouncementParseError::InvalidField { .. })
        ));
    }

    assert!(matches!(
        parse_announcement_list(
            r#"{"result":"success","object":{"aaData":{},"resultsList":[]}}"#,
            "7001",
            tsinghua_kit::LearnAnnouncementBucket::Active,
        ),
        Err(LearnAnnouncementParseError::InvalidCollection)
    ));
    assert!(matches!(
        parse_announcement_list(
            r#"{"result":"success","object":null}"#,
            "7001",
            LearnAnnouncementBucket::Active,
        ),
        Err(LearnAnnouncementParseError::MissingCollection)
    ));
}

#[test]
fn announcement_routes_are_role_and_bucket_specific() {
    for (role, bucket, expected_path) in [
        (
            CourseRole::Student,
            LearnAnnouncementBucket::Active,
            STUDENT_ACTIVE_PATH,
        ),
        (
            CourseRole::Student,
            LearnAnnouncementBucket::Expired,
            STUDENT_EXPIRED_PATH,
        ),
        (
            CourseRole::Teacher,
            LearnAnnouncementBucket::Active,
            TEACHER_ACTIVE_PATH,
        ),
        (
            CourseRole::Teacher,
            LearnAnnouncementBucket::Expired,
            TEACHER_EXPIRED_PATH,
        ),
    ] {
        let source = LearnAnnouncementSource::new(
            LearnClient::new(
                LearnClientConfig::new("https://learn.example.test/", role).expect("Learn config"),
            ),
            transport(),
        );
        let plan = source
            .request_plan("7001", bucket)
            .expect("announcement request plan");
        assert_eq!(plan.method, LearnAnnouncementRequestMethod::Post);
        assert_eq!(plan.path, expected_path);
        assert_eq!(plan.form.len(), 1);
        assert_eq!(plan.form[0].0, "aoData");
    }
}

#[test]
fn configured_learning_routes_are_rejected_before_url_join() {
    for path in [
        "//other.example/path",
        "/learn/../outside",
        "/learn/./outside",
        "/learn\\outside",
        "/learn%ZZ",
        "/learn/%2foutside",
        "/learn/%5coutside",
        "/learn/%2e%2e/outside",
        "/learn/%252e%252e/outside",
    ] {
        let result = LearnClientConfig::new("https://learn.example.test/", CourseRole::Student)
            .expect("base config")
            .with_course_home_path(path);
        assert!(
            matches!(
                result,
                Err(LearnClientError::Learn(LearnError::InvalidRoutePath))
            ),
            "unsafe configured route was accepted: {path}"
        );
    }

    let mut config = LearnClientConfig::new("https://learn.example.test/", CourseRole::Student)
        .expect("base config");
    config.course_home_path = "/learn/%252e%252e/outside".to_owned();
    let client = LearnClient::new(config);
    assert!(matches!(
        client.course_home_request_plan(),
        Err(LearnClientError::Learn(LearnError::InvalidRoutePath))
    ));
}

#[tokio::test]
async fn current_semester_uses_course_home_csrf_and_real_get_route() {
    let (base_url, server) = spawn_fixture(2, |index, request| {
        if index == 0 {
            assert!(request.starts_with(&format!("GET {COURSE_HOME_PATH} HTTP/1.1\r\n")));
            return FixtureResponse::html(r#"<meta name="_csrf" content="semester-csrf">"#)
                .with_header("Set-Cookie", "learn-session=present; Path=/");
        }

        assert!(request.starts_with(&format!(
            "GET {CURRENT_SEMESTER_PATH}?_csrf=semester-csrf HTTP/1.1\r\n"
        )));
        assert!(
            request
                .lines()
                .any(|line| line.eq_ignore_ascii_case("cookie: learn-session=present"))
        );
        FixtureResponse::json(
            r#"{"message":"success","result":{"id":"2025-2026-2","kssj":"2026-02-23 00:00:00","jssj":"2026-07-12 00:00:00","xnxq":"2025-2026-2"}}"#,
        )
    });

    let client = student_client(&base_url);
    let current = client
        .fetch_current_semester(&transport())
        .await
        .expect("current semester request succeeds");
    assert_eq!(current.label, "2025-2026-2");
    let requests = server.join().expect("fixture server joins");
    assert_eq!(requests.len(), 2);
}

#[tokio::test]
async fn semester_list_filters_the_observed_null_item() {
    let (base_url, server) = spawn_fixture(2, |index, request| {
        if index == 0 {
            assert!(request.starts_with(&format!("GET {COURSE_HOME_PATH} HTTP/1.1\r\n")));
            return FixtureResponse::html(r#"<meta name="_csrf" content="semester-csrf">"#)
                .with_header("Set-Cookie", "learn-session=present; Path=/");
        }

        assert!(request.starts_with(&format!(
            "GET /b/wlxt/kc/v_wlkc_xs_xktjb_coassb/queryxnxq?_csrf=semester-csrf HTTP/1.1\r\n"
        )));
        FixtureResponse::json(r#"["2025-2026-2",null]"#)
    });

    let semesters = student_client(&base_url)
        .fetch_semester_ids(&transport())
        .await
        .expect("the current Learn implementation returns nullable semester rows");
    assert_eq!(semesters, vec!["2025-2026-2"]);
    assert_eq!(server.join().expect("fixture server joins").len(), 2);
}

#[tokio::test]
async fn teacher_course_read_fetches_both_public_indexes_and_deduplicates_ids() {
    let (base_url, server) = spawn_fixture(2, |index, request| {
        if index == 0 {
            assert!(request.starts_with(&format!(
                "GET {TEACHER_COURSE_PATH_0}?_csrf=teacher-csrf HTTP/1.1\r\n"
            )));
            return FixtureResponse::json(
                r#"{"message":"success","resultList":[{"wlkcid":"7001","kch":"30240001","kcm":"课程甲"}]}"#,
            );
        }

        assert!(request.starts_with(&format!(
            "GET {TEACHER_COURSE_PATH_1}?_csrf=teacher-csrf HTTP/1.1\r\n"
        )));
        FixtureResponse::json(
            r#"{"message":"success","resultList":[{"wlkcid":"7001","kch":"30240001","kcm":"课程甲重复"},{"wlkcid":"7002","kch":"30240002","kcm":"课程乙"}]}"#,
        )
    });

    let client = teacher_client(&base_url);
    let csrf = learn_csrf("teacher-csrf");
    let records = client
        .fetch_all_course_records(&transport(), &csrf, "2025-2026-2", None, None, "_csrf")
        .await
        .expect("both teacher course lists succeed");

    assert_eq!(records.len(), 2);
    assert_eq!(records[0].id(), Some("7001"));
    assert_eq!(records[0].title(), Some("课程甲"));
    assert_eq!(records[1].id(), Some("7002"));
    let requests = server.join().expect("fixture server joins");
    assert_eq!(requests.len(), 2);
}

#[tokio::test]
async fn announcements_accept_results_list_and_explicit_empty_expired_bucket() {
    let (base_url, server) = spawn_fixture(2, |index, request| {
        assert!(request.contains("_csrf=announcement-csrf"));
        assert_ao_data_part(request, "7001");
        if index == 0 {
            assert!(request.starts_with(&format!(
                "POST {STUDENT_ACTIVE_PATH}?_csrf=announcement-csrf HTTP/1.1\r\n"
            )));
            return FixtureResponse::json(
                r#"{"result":"success","object":{"resultsList":[{"ggid":"announcement-1","bt":"课程公告","fbsj":"2026-09-10 08:00:00"}]}}"#,
            )
            .with_header("Set-Cookie", "learn-session=present; Path=/");
        }

        assert!(
            request
                .lines()
                .any(|line| line.eq_ignore_ascii_case("cookie: learn-session=present"))
        );
        assert!(request.starts_with(&format!(
            "POST {STUDENT_EXPIRED_PATH}?_csrf=announcement-csrf HTTP/1.1\r\n"
        )));
        FixtureResponse::json(r#"{"result":"success","object":{"resultsList":[]}}"#)
    });

    let mut source = LearnAnnouncementSource::new(student_client(&base_url), transport());
    source
        .with_csrf(learn_csrf("announcement-csrf"))
        .expect("announcement CSRF binding");
    let announcements = source
        .list_course("7001")
        .await
        .expect("announcement fixture succeeds");

    assert_eq!(announcements.len(), 1);
    assert_eq!(announcements[0].announcement_id, "announcement-1");
    assert!(!announcements[0].expired);
    let requests = server.join().expect("fixture server joins");
    assert_eq!(requests.len(), 2);
}

#[tokio::test]
async fn announcement_json_login_failure_is_session_expired() {
    let (base_url, server) = spawn_fixture(1, |_index, request| {
        assert!(request.contains(STUDENT_ACTIVE_PATH));
        assert!(request.contains("_csrf=announcement-csrf"));
        FixtureResponse::json(r#"{"result":"error","msg":"未登录"}"#)
    });

    let mut source = LearnAnnouncementSource::new(student_client(&base_url), transport());
    source
        .with_csrf(learn_csrf("announcement-csrf"))
        .expect("announcement CSRF binding");
    let error = source
        .list_course("7001")
        .await
        .expect_err("login failure envelope");
    assert!(matches!(error, LearnAnnouncementError::SessionExpired));
    assert_eq!(server.join().expect("fixture server").len(), 1);
}

async fn assert_announcement_final_query_is_rejected(location: String) {
    let (base_url, server) = spawn_fixture(2, move |index, request| {
        if index == 0 {
            assert!(request.contains(STUDENT_ACTIVE_PATH));
            assert!(request.contains("_csrf=announcement-csrf"));
            return FixtureResponse::redirect(location.clone());
        }

        FixtureResponse::json(r#"{"result":"success","object":{"aaData":[]}}"#)
    });

    let mut source = LearnAnnouncementSource::new(student_client(&base_url), transport());
    source
        .with_csrf(learn_csrf("announcement-csrf"))
        .expect("announcement CSRF binding");
    let error = source
        .list_course("7001")
        .await
        .expect_err("redirect that changes CSRF query must fail closed");
    assert!(matches!(
        error,
        LearnAnnouncementError::InvalidRoute(message) if message.contains("CSRF")
    ));
    let requests = server.join().expect("fixture server joins");
    assert_eq!(requests.len(), 2);
}

#[tokio::test]
async fn announcements_reject_missing_or_tampered_final_csrf_query() {
    assert_announcement_final_query_is_rejected(STUDENT_ACTIVE_PATH.to_owned()).await;
    assert_announcement_final_query_is_rejected(format!("{STUDENT_ACTIVE_PATH}?_csrf=other-csrf"))
        .await;
}

fn partial_todo_response(index: usize, request: &str) -> FixtureResponse {
    if index == 0 {
        assert!(request.starts_with(&format!(
            "GET {STUDENT_COURSE_PATH}?_csrf=todo-csrf HTTP/1.1\r\n"
        )));
        return FixtureResponse::json(
            r#"{"message":"success","resultList":[{"wlkcid":"7001","kch":"30240001","kcm":"课程甲"}]}"#,
        );
    }

    assert!(request.contains("_csrf=todo-csrf"));
    assert_ao_data_part(request, "7001");
    if request.contains("zyListWj") {
        assert!(
            request
                .starts_with("POST /b/wlxt/kczy/zy/student/zyListWj?_csrf=todo-csrf HTTP/1.1\r\n")
        );
        return FixtureResponse::json(
            r#"{"result":"success","object":{"aaData":[{"xszyid":"pending-1","wlkcid":"7001","bt":"待提交","jzsj":"2026-09-20 23:59:59","fbsj":"2026-09-10 08:00:00"}]}}"#,
        );
    }
    if request.contains("zyListYjwg") {
        assert!(
            request.starts_with(
                "POST /b/wlxt/kczy/zy/student/zyListYjwg?_csrf=todo-csrf HTTP/1.1\r\n"
            )
        );
        return FixtureResponse::json(r#"{"result":"error","message":"provider unavailable"}"#);
    }
    assert!(
        request.starts_with("POST /b/wlxt/kczy/zy/student/zyListYpg?_csrf=todo-csrf HTTP/1.1\r\n")
    );
    FixtureResponse::json(
        r#"{"result":"success","object":{"aaData":[{"xszyid":"graded-1","wlkcid":"7001","bt":"已批改","jzsj":"2026-09-19 23:59:59","pysj":"2026-09-12 08:00:00","fbsj":"2026-09-09 08:00:00"}]}}"#,
    )
}

fn todo_source(base_url: &str) -> LearnTodoSource {
    let mut source = LearnTodoSource::new(
        student_client(base_url),
        transport(),
        LearnTodoConfig::new("2025-2026-2"),
    )
    .expect("todo source configuration");
    source
        .with_csrf(learn_csrf("todo-csrf"), "_csrf")
        .expect("todo CSRF binding");
    source
}

#[tokio::test]
async fn todo_report_keeps_verified_items_and_classifies_one_failed_provider() {
    let (base_url, server) = spawn_fixture(4, partial_todo_response);
    let source = todo_source(&base_url);
    let report = source
        .list_todos_report(TodoFilter {
            include_completed: true,
            ..TodoFilter::default()
        })
        .await
        .expect("partial todo report succeeds");

    assert_eq!(report.items.len(), 2);
    assert_eq!(report.failures.len(), 1);
    assert_eq!(report.failures[0].provider, "learn_homework_submitted");
    assert_eq!(
        report.failures[0].kind,
        CampusTodoFailureKind::BusinessFailure
    );
    assert!(report.failure_summary().is_some());
    let requests = server.join().expect("fixture server joins");
    assert_eq!(requests.len(), 4);
}

#[tokio::test]
async fn backend_repair_api_audit_todo_json_login_failure_preserves_typed_expiry_contract() {
    let (base_url, server) = spawn_fixture(2, |index, request| {
        if index == 0 {
            assert!(request.starts_with(&format!(
                "GET {STUDENT_COURSE_PATH}?_csrf=todo-csrf HTTP/1.1\r\n"
            )));
            return FixtureResponse::json(
                r#"{"message":"success","resultList":[{"wlkcid":"7001","kch":"30240001","kcm":"课程甲"}]}"#,
            );
        }

        assert!(request.contains("zyListWj"));
        assert!(request.contains("_csrf=todo-csrf"));
        FixtureResponse::json(r#"{"result":"error","message":"请先登录"}"#)
    });

    let source = todo_source(&base_url);
    let error = source
        .list_todos_report(TodoFilter::default())
        .await
        .expect_err("expired Learn session must stop the provider loop");
    assert!(matches!(
        error,
        ServiceError::SessionExpired {
            service: ServiceId::Learn
        }
    ));
    assert_eq!(server.join().expect("fixture server").len(), 2);
}

#[tokio::test]
async fn todo_source_uses_one_configured_csrf_parameter_for_course_and_all_buckets() {
    let (base_url, server) = spawn_fixture(4, |index, request| {
        assert!(request.contains("learn_token=csrf%2Bvalue"));
        assert!(!request.contains("%252B"));
        if index == 0 {
            assert!(request.starts_with(
                "GET /b/wlxt/kc/v_wlkc_xs_xkb_kcb_extend/student/loadCourseBySemesterId/2025-2026-2/zh?learn_token=csrf%2Bvalue HTTP/1.1\r\n"
            ));
            return FixtureResponse::json(
                r#"{"message":"success","resultList":[{"wlkcid":"7001","kch":"30240001","kcm":"课程甲"}]}"#,
            );
        }

        assert_ao_data_part(request, "7001");
        if index == 1 {
            assert!(request.starts_with(
                "POST /b/wlxt/kczy/zy/student/zyListWj?learn_token=csrf%2Bvalue HTTP/1.1\r\n"
            ));
        } else if index == 2 {
            assert!(request.starts_with(
                "POST /b/wlxt/kczy/zy/student/zyListYjwg?learn_token=csrf%2Bvalue HTTP/1.1\r\n"
            ));
        } else {
            assert!(request.starts_with(
                "POST /b/wlxt/kczy/zy/student/zyListYpg?learn_token=csrf%2Bvalue HTTP/1.1\r\n"
            ));
        }
        FixtureResponse::json(
            r#"{"result":"success","object":{"aaData":[{"xszyid":"assignment-1","wlkcid":"7001","bt":"作业","jzsj":"2026-09-20 23:59:59"}]}}"#,
        )
    });

    let mut source = LearnTodoSource::new(
        student_client(&base_url),
        transport(),
        LearnTodoConfig::new("2025-2026-2"),
    )
    .expect("todo source configuration");
    source
        .with_csrf(learn_csrf("csrf+value"), "learn_token")
        .expect("custom CSRF binding");
    let todos = source
        .list_todos(TodoFilter::default())
        .await
        .expect("custom CSRF parameter is accepted");
    assert_eq!(todos.len(), 1);
    assert_eq!(server.join().expect("fixture server").len(), 4);
}

#[test]
fn homework_success_requires_the_explicit_collection_envelope() {
    assert!(
        parse_homework_list(
            r#"{"result":"success","object":{"aaData":[]}}"#,
            HomeworkBucket::Pending,
        )
        .expect("explicit empty assignment list")
        .is_empty()
    );
    assert!(matches!(
        parse_homework_list(
            r#"{"result":"success","object":{}}"#,
            HomeworkBucket::Pending,
        ),
        Err(tsinghua_kit::LearnTodoParseError::InvalidCollection)
    ));
}

#[tokio::test]
async fn ordinary_todo_read_does_not_promote_partial_provider_data_to_success() {
    let (base_url, server) = spawn_fixture(4, partial_todo_response);
    let source = todo_source(&base_url);
    let error = source
        .list_todos(TodoFilter::default())
        .await
        .expect_err("ordinary todo read must report provider failure");
    assert!(matches!(
        error,
        ServiceError::Adapter { message }
            if message.contains("todo providers failed") && message.contains("business_failure")
    ));
    let requests = server.join().expect("fixture server joins");
    assert_eq!(requests.len(), 4);
}

async fn fetch_course_fixture(
    first_response: FixtureResponse,
    redirect_to: Option<String>,
) -> Result<Vec<LearnCourseRecord>, LearnClientError> {
    let request_count = if redirect_to.is_some() { 2 } else { 1 };
    let (base_url, server) = spawn_fixture(request_count, move |index, request| {
        if index == 0 {
            assert!(request.starts_with(&format!(
                "GET {STUDENT_COURSE_PATH}?_csrf=course-csrf HTTP/1.1\r\n"
            )));
            if let Some(location) = redirect_to.as_ref() {
                return FixtureResponse::redirect(location.clone());
            }
        }
        first_response.clone()
    });

    let client = student_client(&base_url);
    let csrf = learn_csrf("course-csrf");
    let result = client
        .fetch_course_records(&transport(), &csrf, "2025-2026-2", Some("zh"), None)
        .await;
    let requests = server.join().expect("fixture server joins");
    assert_eq!(requests.len(), request_count);
    result
}

#[tokio::test]
async fn course_read_rejects_login_html_malformed_json_and_route_drift_but_accepts_explicit_empty()
{
    let login = fetch_course_fixture(
        FixtureResponse::html(
            r#"<form action="/do/off/ui/auth/login/check"><input name="i_user"></form>"#,
        ),
        None,
    )
    .await;
    assert!(matches!(login, Err(LearnClientError::SessionExpired)));

    let malformed = fetch_course_fixture(FixtureResponse::json("{"), None).await;
    assert!(matches!(
        malformed,
        Err(LearnClientError::CourseJson(CourseJsonError::Decode(_)))
    ));

    let route_drift = fetch_course_fixture(
        FixtureResponse::json(r#"{"message":"success","resultList":[]}"#),
        Some("/moved".to_owned()),
    )
    .await;
    assert!(matches!(route_drift, Err(LearnClientError::UnexpectedPath)));

    let empty = fetch_course_fixture(
        FixtureResponse::json(r#"{"message":"success","resultList":[]}"#),
        None,
    )
    .await
    .expect("explicit successful empty course list");
    assert!(empty.is_empty());

    let semester_mismatch = fetch_course_fixture(
        FixtureResponse::json(
            r#"{"message":"success","resultList":[{"wlkcid":"7001","kcm":"课程","semester":"2025-2026-1"}]}"#,
        ),
        None,
    )
    .await;
    assert!(matches!(
        semester_mismatch,
        Err(LearnClientError::CourseJson(CourseJsonError::SemesterMismatch {
            index: Some(0),
            expected,
            actual,
        })) if expected == "2025-2026-2" && actual == "2025-2026-1"
    ));

    let malformed_semester = fetch_course_fixture(
        FixtureResponse::json(
            r#"{"message":"success","resultList":[{"wlkcid":"7001","kcm":"课程","semester":null}]}"#,
        ),
        None,
    )
    .await;
    assert!(matches!(
        malformed_semester,
        Err(LearnClientError::CourseJson(
            CourseJsonError::InvalidField {
                index: Some(0),
                field: "semester",
            }
        ))
    ));
}

#[test]
fn generic_username_password_login_forms_are_explicitly_expired() {
    let client = student_client("https://learn.example.test/");
    let response = client
        .map_html_response(
            StatusCode::OK,
            None,
            r#"<form action="/login"><input name="username"><input name="password" type="password"></form>"#,
        )
        .expect("generic login form classification");
    assert!(response.is_login_expired());
}

#[tokio::test]
async fn course_read_maps_a_json_login_envelope_to_session_expired() {
    let result = fetch_course_fixture(
        FixtureResponse::json(r#"{"message":"未登录","resultList":[]}"#),
        None,
    )
    .await;
    assert!(matches!(result, Err(LearnClientError::SessionExpired)));
}

#[tokio::test]
async fn semester_and_time_location_reads_map_json_login_envelopes_to_session_expired() {
    let (semester_base_url, semester_server) = spawn_fixture(2, |index, request| {
        if index == 0 {
            assert!(request.starts_with(&format!("GET {COURSE_HOME_PATH} HTTP/1.1\r\n")));
            return FixtureResponse::html(r#"<meta name="_csrf" content="semester-csrf">"#);
        }

        assert!(request.starts_with(&format!(
            "GET /b/wlxt/kc/v_wlkc_xs_xktjb_coassb/queryxnxq?_csrf=semester-csrf HTTP/1.1\r\n"
        )));
        FixtureResponse::json(r#"{"message":"需要登录","resultList":[]}"#)
    });
    let semester_error = student_client(&semester_base_url)
        .fetch_semester_ids(&transport())
        .await
        .expect_err("JSON semester login envelope");
    assert!(matches!(semester_error, LearnClientError::SessionExpired));
    assert_eq!(semester_server.join().expect("semester server").len(), 2);

    let (time_base_url, time_server) =
        spawn_fixture(1, |_index, request| {
            assert!(request.starts_with(
                "GET /b/kc/v_wlkc_xk_sjddb/detail?id=7001&_csrf=time-csrf HTTP/1.1\r\n"
            ));
            FixtureResponse::json(r#"{"message":"请重新登录"}"#)
        });
    let time_error = student_client(&time_base_url)
        .fetch_course_time_location(&transport(), &learn_csrf("time-csrf"), "7001")
        .await
        .expect_err("JSON time/location login envelope");
    assert!(matches!(time_error, LearnClientError::SessionExpired));
    assert_eq!(time_server.join().expect("time server").len(), 1);
}

#[tokio::test]
async fn course_read_classifies_a_stopped_external_identity_redirect_as_session_expiry() {
    let (base_url, server) = spawn_fixture(1, |_index, request| {
        assert!(request.starts_with(&format!(
            "GET {STUDENT_COURSE_PATH}?_csrf=course-csrf HTTP/1.1\r\n"
        )));
        FixtureResponse::redirect("https://id.example.test/do/off/ui/auth/login?service=learn")
    });

    let result = student_client(&base_url)
        .fetch_course_records(
            &transport(),
            &learn_csrf("course-csrf"),
            "2025-2026-2",
            Some("zh"),
            None,
        )
        .await;
    assert!(matches!(result, Err(LearnClientError::SessionExpired)));
    assert_eq!(server.join().expect("fixture server joins").len(), 1);
}

#[tokio::test]
async fn semester_read_classifies_a_same_origin_login_redirect_before_parsing() {
    let (base_url, server) = spawn_fixture(2, |index, request| {
        if index == 0 {
            assert!(request.starts_with(&format!("GET {COURSE_HOME_PATH} HTTP/1.1\r\n")));
            return FixtureResponse::redirect("/auth/login?service=learn");
        }

        assert!(request.starts_with("GET /auth/login?service=learn HTTP/1.1\r\n"));
        FixtureResponse::html(
            r#"<form action="/do/off/ui/auth/login/check"><input name="i_user"><input name="i_pass"></form>"#,
        )
    });

    let result = student_client(&base_url)
        .fetch_semester_ids(&transport())
        .await;
    assert!(matches!(result, Err(LearnClientError::SessionExpired)));
    assert_eq!(server.join().expect("fixture server joins").len(), 2);
}
