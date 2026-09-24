//! Cross-service contract checks against pinned reference behavior.
use crate::protocol::{CourseRole, CsrfToken, ServiceId};
use crate::reference_test_support::{FixtureServer, Reply};
use crate::transport::CampusHttpTransport;
use serde_json::{Value, json};

#[tokio::test]
async fn backend_repair_api_audit_webvpn_encoded_separator_same_mapping_remains_usable() {
    let mapping = "77726476706e69737468656265737421aaaa";
    let server = FixtureServer::new(vec![
        Reply {
            status: 302,
            headers: format!("Location: /https/{mapping}%2Fnext\r\n"),
            body: String::new(),
        },
        Reply::html("ok"),
    ]);
    let transport = CampusHttpTransport::new("THYou/api-audit").unwrap();
    assert_eq!(
        transport
            .get_text(&format!("{}https/{mapping}/first", server.base()))
            .await
            .unwrap(),
        "ok"
    );
    assert_eq!(server.requests().len(), 2);
}

#[test]
fn backend_repair_api_audit_announcements_empty_and_failed_envelopes_are_distinct() {
    use crate::learn_announcements::{LearnAnnouncementBucket, parse_announcement_list};
    assert!(
        parse_announcement_list(
            r#"{"result":"success","object":{"aaData":[]}}"#,
            "fixture-course",
            LearnAnnouncementBucket::Active
        )
        .unwrap()
        .is_empty()
    );
    assert!(
        parse_announcement_list(
            r#"{"result":"fail","object":{"aaData":[]}}"#,
            "fixture-course",
            LearnAnnouncementBucket::Active
        )
        .is_err()
    );
    assert!(
        parse_announcement_list(
            r#"{"result":"success","object":{}}"#,
            "fixture-course",
            LearnAnnouncementBucket::Active
        )
        .is_err()
    );
    let valid = r#"{"result":"success","object":{"resultsList":[{"ggid":"notice-1","bt":"A &amp; B","fbsj":"2026-09-17 10:00:00","ggnr":""}]}}"#;
    let result =
        parse_announcement_list(valid, "fixture-course", LearnAnnouncementBucket::Expired).unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].title, "A & B");
}

#[test]
fn backend_repair_api_audit_info_failure_lists_are_not_successful_empty_responses() {
    use crate::info_news::{parse_news_detail, parse_news_list, parse_news_search};
    assert!(parse_news_list(r#"{"result":"fail","object":{"dataList":[]}}"#).is_err());
    assert!(parse_news_search(r#"{"success":false,"object":{"resultsList":[]}}"#).is_err());
    assert!(
        parse_news_detail(r#"{"result":"success","object":{"xxDto":{"bt":"Title","nr":""}}}"#)
            .is_err()
    );
    assert!(
        parse_news_list(r#"{"result":"success","object":{"dataList":[]}}"#)
            .unwrap()
            .into_page()
            .items
            .is_empty()
    );
}

#[test]
fn backend_repair_api_audit_info_search_unicode_and_filters_keep_exact_wire_shape() {
    use crate::info_news::{NewsHttpMethod, NewsProfile, NewsSearchInput};
    let profile = NewsProfile::standard();
    let plan = profile
        .search_request(
            &NewsSearchInput::new("清华 & A+B", 2)
                .with_channel_filter("教务通知")
                .with_exact_match(true),
        )
        .unwrap();
    assert_eq!(plan.method(), NewsHttpMethod::Post);
    assert_eq!(plan.form_parameters().len(), 1);
    assert_eq!(plan.form_parameters()[0].0, "esParamClass");
    let value: Value = serde_json::from_str(&plan.form_parameters()[0].1).unwrap();
    assert_eq!(
        value["params"],
        json!({"bt":"清华 & A+B","tag":"清华 & A+B","xxfl":"清华 & A+B"})
    );
    assert_eq!(value["filterParams"], json!({"lmmcgroup":"教务通知"}));
    assert_eq!(value["matchExact"], "是");
    assert_eq!(value["currentPage"], 2);
    assert_eq!(value["orderMap"]["sort"], "time");
    assert!(profile.list_request(0, 20, None, None).is_err());
    assert!(profile.detail_request("id&other=1").is_err());
}

#[test]
fn backend_repair_api_audit_registrar_grade_profiles_do_not_mix_academic_stages() {
    use crate::registrar_academic::{
        RegistrarGradesProfile as Profile, UndergraduateReportKind as Kind,
    };
    for (kind, flag) in [
        (Kind::FirstDegree, "di1"),
        (Kind::SecondDegree, "di2"),
        (Kind::Minor, "di3"),
    ] {
        let plan = Profile::undergraduate(kind).request().unwrap();
        assert_eq!(plan.path, "/cj.cjCjbAll.do");
        let query: std::collections::BTreeMap<_, _> =
            plan.query.into_iter().map(|q| (q.name, q.value)).collect();
        assert_eq!(query["m"], "bks_cjdcx");
        assert_eq!(query["flag"], flag);
        assert_eq!(query["cjdlx"], "zw");
    }
    let query = Profile::graduate().request().unwrap().query;
    assert!(
        query
            .iter()
            .any(|q| q.name == "m" && q.value == "yjs_cjdcx")
    );
    assert!(!query.iter().any(|q| q.name == "flag"));
    assert!(
        Profile::for_stage(crate::protocol::AcademicStage::Graduate, Some(Kind::Minor)).is_err()
    );
}

#[test]
fn backend_repair_api_audit_registrar_grades_empty_table_not_missing_or_malformed_data() {
    use crate::registrar_academic::{RegistrarGradesProfile, UndergraduateReportKind};
    let profile = RegistrarGradesProfile::undergraduate(UndergraduateReportKind::FirstDegree);
    let header = "<tr><th>序号</th><th>课程号</th><th>课序号</th><th>课程名称</th><th>课程性质</th><th>学分</th><th>考试方式</th><th>成绩</th><th>补考</th><th>绩点</th><th>备注</th><th>学期</th></tr>";
    assert!(
        profile
            .parse_html(&format!(
                "<html><table cellspacing=\"1\">{header}</table></html>"
            ))
            .unwrap()
            .courses
            .is_empty()
    );
    assert!(
        profile
            .parse_html("<html><body>service error</body></html>")
            .is_err()
    );
    let bad = "<tr><td>1</td><td>CODE</td><td>01</td><td>Fixture</td><td>必修</td><td>NaN</td><td>正考</td><td>90</td><td></td><td>4.0</td><td></td><td>2026-2027-1</td></tr>";
    assert!(
        profile
            .parse_html(&format!(
                "<html><table cellspacing=\"1\">{header}{bad}</table></html>"
            ))
            .is_err()
    );
}

#[test]
fn backend_repair_api_audit_calendar_leap_day_chunks_cover_range_once() {
    use crate::registrar::{RegistrarProfile, split_calendar_range};
    use chrono::NaiveDate;
    let start = NaiveDate::from_ymd_opt(2028, 2, 1).unwrap();
    let end = NaiveDate::from_ymd_opt(2028, 3, 5).unwrap();
    let windows = split_calendar_range(start, end).unwrap();
    assert_eq!(windows.iter().map(|w| w.day_count()).sum::<i64>(), 34);
    assert!(windows.iter().all(|w| w.day_count() <= 28));
    assert!(
        windows
            .windows(2)
            .all(|w| w[0].end.succ_opt() == Some(w[1].start))
    );
    let plan = RegistrarProfile::new(crate::protocol::AcademicStage::Graduate)
        .calendar_request(windows[1].clone(), "cb")
        .unwrap();
    assert_eq!(plan.start_date, "20280229");
    assert_eq!(plan.method, "yjs_jxrl_all");
    let record = crate::registrar_client::decode_calendar_jsonp(
        "cb([]);",
        "cb",
        crate::protocol::AcademicStage::Graduate,
        windows[1].clone(),
    )
    .unwrap();
    assert!(record.events.is_empty());
}

#[test]
fn backend_repair_api_audit_exam_header_empty_is_valid_but_graduate_route_is_not_invented() {
    use crate::registrar_client::registrar_exam::{
        RegistrarExamStage, RegistrarVerifiedExamPageProfile, parse_verified_exam_page_html,
    };
    let html = "<html><table><tr><th>开课系</th><th>课程号</th><th>课序号</th><th>课程名</th><th>课程分类</th><th>教师</th><th>人数</th><th>考试日期</th><th>考场</th></tr></table></html>";
    assert!(parse_verified_exam_page_html(html).unwrap().is_empty());
    assert!(parse_verified_exam_page_html("<html>service error</html>").is_err());
    assert!(RegistrarVerifiedExamPageProfile::for_stage(RegistrarExamStage::Graduate).is_err());
    let plan = RegistrarVerifiedExamPageProfile::undergraduate().request();
    assert_eq!(plan.method, reqwest::Method::GET);
    assert!(!plan.query.iter().any(|(key, _)| key.contains("date")));
}

#[test]
fn backend_repair_api_audit_classroom_gb2312_selectors_do_not_inject_query_fields() {
    let profile = crate::classroom_read::ClassroomReadProfile::new();
    let request = profile.weekly_state_request("一教&x=1", 3).unwrap();
    assert!(request.query.contains("%D2%BB%BD%CC"));
    assert!(request.query.contains("%26x%3D1"));
    assert!(!request.query.contains("&x="));
    assert!(profile.weekly_state_request("一教", 0).is_err());
    assert!(profile.weekly_state_request("bad\nvalue", 1).is_err());
    assert!(profile.weekly_state_request("🏫", 1).is_err());
}

#[tokio::test]
async fn backend_repair_api_audit_homework_aggregation_expiry_stops_after_first_bucket() {
    use crate::campus_live::CampusTodoSource;
    let server = FixtureServer::new(vec![
        Reply::json(
            r#"{"message":"success","resultList":[{"wlkcid":"fixture-course","kch":"CODE","kcm":"Fixture"}]}"#,
        ),
        Reply {
            status: 403,
            headers: String::new(),
            body: String::new(),
        },
    ]);
    let error = todo_source(&server)
        .list_todos(crate::domain::TodoFilter::default())
        .await
        .unwrap_err();
    assert!(error.is_session_expired(ServiceId::Learn));
    assert_eq!(server.requests().len(), 2);
}

#[tokio::test]
async fn backend_repair_api_audit_webvpn_nested_encoded_traversal_is_stopped_before_dispatch() {
    let mapping = "77726476706e69737468656265737421aaaa";
    for path in [
        format!("/https/{mapping}/%252e%252e/%252e%252e/other"),
        format!("/https/{mapping}%2F..%2Fother"),
    ] {
        let server = FixtureServer::new(vec![Reply {
            status: 307,
            headers: format!("Location: {path}\r\n"),
            body: String::new(),
        }]);
        let transport = CampusHttpTransport::new("THYou/api-audit").unwrap();
        let response = transport
            .send(
                transport
                    .client()
                    .post(format!("{}https/{mapping}/business", server.base()))
                    .body("synthetic"),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::TEMPORARY_REDIRECT);
        assert_eq!(server.requests().len(), 1);
    }
}

#[tokio::test]
async fn backend_repair_api_audit_webvpn_login_redirect_is_exposed_without_business_replay() {
    let mapping = "77726476706e69737468656265737421aaaa";
    let server = FixtureServer::new(vec![Reply {
        status: 302,
        headers: "Location: /login\r\n".into(),
        body: String::new(),
    }]);
    let transport = CampusHttpTransport::new("THYou/api-audit").unwrap();
    let response = transport
        .get_text_response(&format!("{}https/{mapping}/business", server.base()))
        .await
        .unwrap();
    assert_eq!(response.redirect_location.as_deref(), Some("/login"));
    assert_eq!(response.status, reqwest::StatusCode::FOUND);
    assert_eq!(server.requests().len(), 1);
}

fn todo_source(server: &FixtureServer) -> crate::learn_todos::LearnTodoSource {
    let learn = crate::learn_client::LearnClient::new(
        crate::learn_client::LearnClientConfig::new(server.base(), CourseRole::Student).unwrap(),
    );
    let mut source = crate::learn_todos::LearnTodoSource::new(
        learn,
        CampusHttpTransport::new("THYou/api-audit").unwrap(),
        crate::learn_todos::LearnTodoConfig::new("2026-2027-1"),
    )
    .unwrap();
    source
        .with_csrf(
            crate::session::SessionRegistry::new()
                .bind_csrf(ServiceId::Learn, CsrfToken::new("fixture-csrf").unwrap()),
            "_csrf",
        )
        .unwrap();
    source
}

#[tokio::test]
async fn backend_repair_api_audit_homework_expiry_keeps_typed_service_identity() {
    let server = FixtureServer::new(vec![Reply {
        status: 403,
        headers: String::new(),
        body: String::new(),
    }]);
    let error = todo_source(&server)
        .list_course_homework("fixture-course")
        .await
        .unwrap_err();
    assert!(error.is_session_expired(ServiceId::Learn));
    assert!(!error.is_session_expired(ServiceId::Registrar));
    assert_eq!(
        server.requests().len(),
        1,
        "stop after the first expired homework bucket"
    );
}

#[tokio::test]
async fn backend_repair_api_audit_homework_outage_not_misclassified_as_expiry() {
    let server = FixtureServer::new(vec![Reply {
        status: 503,
        headers: String::new(),
        body: String::new(),
    }]);
    let error = todo_source(&server)
        .list_course_homework("fixture-course")
        .await
        .unwrap_err();
    assert!(!error.is_session_expired(ServiceId::Learn));
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn backend_repair_api_audit_todo_course_discovery_expiry_keeps_typed_identity() {
    use crate::campus_live::CampusTodoSource;
    let server = FixtureServer::new(vec![Reply {
        status: 403,
        headers: String::new(),
        body: String::new(),
    }]);
    let error = todo_source(&server)
        .list_todos(crate::domain::TodoFilter::default())
        .await
        .unwrap_err();
    assert!(error.is_session_expired(ServiceId::Learn));
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn backend_repair_api_audit_library_wire_time_does_not_hide_invalid_seconds() {
    for time in [
        "2026-09-17 08:00:99",
        "2026-09-17T08:00:00garbage",
        "2026-09-17 08:00:00+bad",
    ] {
        let body = json!({"data":{"list":[{"id":1,"day":"2026-09-17","startTime":{"date":time},"endTime":{"date":"2026-09-17 21:00:00.000000"}}]}}).to_string();
        assert!(
            crate::library_read::parse_day_segments(&body).is_err(),
            "invalid complete timestamp must not become valid HH:MM"
        );
    }
}

#[test]
fn backend_repair_api_audit_library_php_datetime_and_count_consistency() {
    let body = json!({"data":{"list":[{"id":"7","day":"2026-09-17","startTime":{"date":"2026-09-17 08:00:00.000000"},"endTime":{"date":"2026-09-17 21:30:00.000000"}}]}}).to_string();
    let data = crate::library_read::parse_day_segments(&body).unwrap();
    assert_eq!(data.segments[0].start_time, "08:00");
    assert_eq!(data.segments[0].end_time, "21:30");
    assert!(
        crate::library_read::parse_area_tree(
            r#"{"data":{"list":[{"id":1,"name":"fixture","TotalCount":3,"UnavailableSpace":4}]}}"#
        )
        .is_err()
    );
}

#[test]
fn backend_repair_api_audit_library_failure_envelope_never_becomes_empty_data() {
    assert!(
        crate::library_read::parse_seat_availability(r#"{"success":false,"data":{"list":[]}}"#)
            .is_err()
    );
    assert!(
        crate::library_read::parse_seat_availability(r#"{"data":{"status":503,"list":[]}}"#)
            .is_err()
    );
    assert!(
        crate::library_read::parse_seat_availability(r#"{"data":{"list":[]}}"#)
            .unwrap()
            .seats
            .is_empty()
    );
    assert!(crate::library_read::parse_socket_status(r#"{"data":{"list":[]}}"#).is_err());
}

#[tokio::test]
async fn backend_repair_api_audit_electricity_server_timeout_is_not_login_expiry() {
    let server = FixtureServer::new(vec![Reply {
        status: 503,
        headers: "Content-Type: text/html\r\n".into(),
        body: "<html><body>请求超时，请稍后重试</body></html>".into(),
    }]);
    let adapter = crate::dorm_electricity_read::DormElectricityAdapter::try_with_transport(
        reqwest::Url::parse(server.base()).unwrap(),
        CampusHttpTransport::new("THYou/api-audit").unwrap(),
    )
    .unwrap();
    let error = adapter.read_remainder().await.unwrap_err();
    assert!(!error.is_session_expired());
    assert!(matches!(
        error,
        crate::dorm_electricity_read::DormElectricityAdapterError::HttpStatus {
            status: reqwest::StatusCode::SERVICE_UNAVAILABLE
        }
    ));
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn backend_repair_api_audit_electricity_explicit_expiry_and_malformed_data_remain_errors() {
    use crate::dorm_electricity_read::{
        DormElectricityParseError, parse_electricity_remainder_html,
    };
    assert!(matches!(
        parse_electricity_remainder_html("<html>会话已过期</html>"),
        Err(DormElectricityParseError::ExpiredPage)
    ));
    assert!(parse_electricity_remainder_html("<html><span id=\"Netweb_Home_electricity_DetailCtrl1_lblele\">NaN</span><span id=\"Netweb_Home_electricity_DetailCtrl1_lbltime\">2026-09-17 10:00:00</span></html>").is_err());
}

#[test]
fn backend_repair_api_audit_electricity_history_diagnostics_keep_parser_shape_stable() {
    use crate::dorm_electricity_read::{DormElectricityAdapterError, DormElectricityParseError};

    let cases = [
        (
            DormElectricityParseError::EmptyBody,
            "electricity_body_empty",
        ),
        (
            DormElectricityParseError::MissingHistoryTable,
            "electricity_history_table_missing",
        ),
        (
            DormElectricityParseError::InvalidHistoryCellCount { row: 3 },
            "electricity_history_row_shape",
        ),
        (
            DormElectricityParseError::InvalidHistoryTimestamp { row: 3 },
            "electricity_history_timestamp",
        ),
        (
            DormElectricityParseError::InvalidHistoryAmount { row: 3 },
            "electricity_history_amount",
        ),
        (
            DormElectricityParseError::EmptyHistoryStatus { row: 3 },
            "electricity_history_status",
        ),
    ];

    for (parse_error, expected) in cases {
        let error = DormElectricityAdapterError::Parse(parse_error);
        assert_eq!(error.diagnostic_code(), expected);
        assert!(!error.to_string().contains("synthetic"));
    }
}

#[test]
fn backend_repair_api_audit_electricity_history_ignores_one_unrelated_legacy_table() {
    use crate::dorm_electricity_read::{
        DormElectricityParseError, parse_electricity_payment_history_html,
    };

    let body = r#"<!doctype html><html>
        <table class="myTable"><tr><td>unrelated</td></tr></table>
        <div class="myTable"><table>
            <tr><th>header</th></tr>
            <tr><td></td><td>0</td><td>2026-09-01 01:02:03</td><td></td><td>10.00</td><td>已成功</td></tr>
            <tr><td>footer</td></tr>
        </table></div>
    </html>"#;
    let history = parse_electricity_payment_history_html(body).expect("history table");
    assert_eq!(history.len(), 1);
    assert_eq!(history.records[0].sequence, Some(0));

    let ambiguous = r#"<!doctype html><html>
        <table class="myTable"><tr><th>header</th></tr>
            <tr><td></td><td>0</td><td>2026-09-01 01:02:03</td><td></td><td>10.00</td><td>已成功</td></tr>
            <tr><td>footer</td></tr>
        </table>
        <table class="myTable"><tr><th>header</th></tr>
            <tr><td></td><td>1</td><td>2026-09-01 01:02:03</td><td></td><td>10.00</td><td>已成功</td></tr>
            <tr><td>footer</td></tr>
        </table>
    </html>"#;
    assert!(matches!(
        parse_electricity_payment_history_html(ambiguous),
        Err(DormElectricityParseError::DuplicateHistoryTable)
    ));
}

#[test]
fn backend_repair_followup_electricity_history_does_not_double_parse_nested_legacy_scope() {
    use crate::dorm_electricity_read::parse_electricity_payment_history_html;

    let body = r#"<!doctype html><html><div class="myTable">
        <tr><th>outer header</th></tr>
        <table class="myTable">
            <tr><td></td><td>0</td><td>2026-09-01 01:02:03</td><td></td><td>1.00</td><td>已成功</td></tr>
            <tr><td></td><td>1</td><td>2026-09-01 02:03:04</td><td></td><td>2.00</td><td>已成功</td></tr>
            <tr><td></td><td>2</td><td>2026-09-01 03:04:05</td><td></td><td>3.00</td><td>已成功</td></tr>
        </table>
        <tr><td></td><td>3</td><td>2026-09-01 04:05:06</td><td></td><td>4.00</td><td>已成功</td></tr>
        <tr><td>outer footer</td></tr>
    </div></html>"#;
    let history = parse_electricity_payment_history_html(body).expect("history table");
    assert_eq!(history.len(), 4);
    assert_eq!(history.records[0].sequence, Some(0));
    assert_eq!(history.records[3].sequence, Some(3));
}

#[test]
fn backend_repair_api_audit_info_handoff_diagnostics_identify_only_failure_field() {
    use crate::info::{InfoError, parse_online_app_redirect};

    let cases = [
        (
            r#"{"result":"error","message":"synthetic-private-result","object":{"roamingurl":"https://webvpn.example/target"}}"#,
            InfoError::ResultFailureWithTarget,
        ),
        (
            r#"{"success":false,"message":"synthetic-private-success","object":{"roamingurl":"https://webvpn.example/target"}}"#,
            InfoError::SuccessFlagFailure,
        ),
        (
            r#"{"error":"synthetic-private-error","object":{"roamingurl":"https://webvpn.example/target"}}"#,
            InfoError::ErrorFieldFailure,
        ),
        (
            r#"{"message":"permission denied: synthetic-private-message","object":{"roamingurl":"https://webvpn.example/target"}}"#,
            InfoError::MessageFailure,
        ),
    ];

    for (body, expected) in cases {
        let error = parse_online_app_redirect(body).unwrap_err();
        assert_eq!(error, expected);
        assert!(!error.to_string().contains("synthetic-private"));
    }
}

#[test]
fn backend_repair_api_audit_info_result_failure_keeps_target_presence_out_of_success_path() {
    use crate::info::{InfoError, parse_online_app_redirect};

    let with_target = parse_online_app_redirect(
        r#"{"result":"error","object":{"roamingurl":"https://webvpn.example/target"}}"#,
    )
    .unwrap_err();
    assert_eq!(with_target, InfoError::ResultFailureWithTarget);

    let without_target =
        parse_online_app_redirect(r#"{"result":"error","message":"synthetic-private-result"}"#)
            .unwrap_err();
    assert_eq!(without_target, InfoError::ResultFailure);
    assert!(!with_target.to_string().contains("https://"));
    assert!(!without_target.to_string().contains("synthetic-private"));
}

#[test]
fn backend_repair_followup_info_result_failure_keeps_envelope_shape_as_static_diagnostics() {
    use crate::info::{InfoError, parse_online_app_redirect};

    let with_object = parse_online_app_redirect(r#"{"result":"error","object":null}"#).unwrap_err();
    assert_eq!(with_object, InfoError::ResultFailureWithObject);

    let with_message =
        parse_online_app_redirect(r#"{"result":"error","message":"private-value"}"#).unwrap_err();
    assert_eq!(with_message, InfoError::ResultFailureWithMessage);
    assert!(!with_object.to_string().contains("private-value"));
    assert!(!with_message.to_string().contains("private-value"));
}

#[test]
fn backend_repair_followup_info_result_msg_without_target_is_classified_as_message_failure() {
    use crate::info::{InfoError, parse_online_app_redirect};

    let error =
        parse_online_app_redirect(r#"{"result":"error","msg":"permission denied"}"#).unwrap_err();
    assert_eq!(error, InfoError::ResultFailureWithMessage);
    assert!(!error.to_string().contains("permission denied"));
}

#[test]
fn backend_repair_followup_info_roaming_uses_reference_wire_headers() {
    use crate::info::InfoPortalProfile;
    use crate::info_client::InfoClient;
    use crate::info_client::InfoClientConfig;
    use crate::transport::CampusHttpTransport;
    use reqwest::header::{CONTENT_TYPE, USER_AGENT};

    let client = InfoClient::new(
        InfoClientConfig::new("https://info.example.test/", InfoPortalProfile::standard())
            .expect("INFO config"),
    )
    .expect("INFO client");
    let transport = CampusHttpTransport::new("THYou/audit").expect("transport");
    let plan = client
        .online_app_redirect_query_plan("registrar-selector", "csrf-value")
        .expect("roaming plan");
    let request = plan.build_request(&transport).expect("request");
    assert_eq!(
        request
            .headers()
            .get(USER_AGENT)
            .and_then(|value| value.to_str().ok()),
        Some(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/79.0.3945.88 Safari/537.36"
        )
    );
    assert_eq!(
        request
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("application/x-www-form-urlencoded")
    );
}

#[test]
fn backend_repair_followup_info_result_object_without_target_never_becomes_success() {
    use crate::info::{InfoError, parse_online_app_redirect};

    for body in [
        r#"{"result":"error","object":{}}"#,
        r#"{"result":"error","object":{"roamingurl":""}}"#,
        r#"{"result":"error","object":{"roamingurl":null}}"#,
    ] {
        assert_eq!(
            parse_online_app_redirect(body).unwrap_err(),
            InfoError::ResultFailureWithObjectNoTarget
        );
    }
}

#[test]
fn backend_repair_api_audit_registrar_jsonp_accepts_one_safe_statement_terminator() {
    for wire in ["cb([])", "cb([]);", " \u{feff}cb([1]); \n"] {
        assert!(crate::transport::parse_jsonp::<Value>(wire, "cb").is_ok());
    }
}

#[test]
fn backend_repair_api_audit_jsonp_never_executes_trailing_javascript() {
    for wire in [
        "cb([]);other()",
        "cb([]);;",
        "cb([])//comment",
        "other([]);",
        "cb(alert(1))",
        "cb([]);/*comment*/",
    ] {
        assert!(crate::transport::parse_jsonp::<Value>(wire, "cb").is_err());
    }
}

#[tokio::test]
async fn backend_repair_api_audit_webvpn_redirect_cannot_forward_body_to_other_mapping() {
    let mapping_a = "77726476706e69737468656265737421aaaa";
    let mapping_b = "77726476706e69737468656265737421bbbb";
    let server = FixtureServer::new(vec![Reply {
        status: 307,
        headers: format!("Location: /https/{mapping_b}/wrong\r\n"),
        body: String::new(),
    }]);
    let transport = CampusHttpTransport::new("THYou/api-audit").unwrap();
    let response = transport
        .send(
            transport
                .client()
                .post(format!("{}https/{mapping_a}/business", server.base()))
                .body("fixture-business-body"),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(
        server.requests().len(),
        1,
        "validate the mapping before the redirected request, not after"
    );
}

#[tokio::test]
async fn backend_repair_api_audit_webvpn_same_mapping_redirect_preserves_cookie_flow() {
    let mapping = "77726476706e69737468656265737421aaaa";
    let server = FixtureServer::new(vec![
        Reply {
            status: 302,
            headers: format!(
                "Location: /https/{mapping}/next\r\nSet-Cookie: fixture=ok; Path=/\r\n"
            ),
            body: String::new(),
        },
        Reply::html("done"),
    ]);
    let transport = CampusHttpTransport::new("THYou/api-audit").unwrap();
    assert_eq!(
        transport
            .get_text(&format!("{}https/{mapping}/first", server.base()))
            .await
            .unwrap(),
        "done"
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert!(
        requests[1]
            .to_ascii_lowercase()
            .contains("cookie: fixture=ok")
    );
}
