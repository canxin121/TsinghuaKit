//! Cross-list consistency and verified-client contracts; synthetic data only.
use crate::{
    campus_live::CampusTodoSource,
    domain::TodoFilter,
    learn_announcements::LearnAnnouncementSource,
    learn_client::{LearnClient, LearnClientConfig, LearnCourseRecord},
    learn_todos::{LearnTodoConfig, LearnTodoSource},
    protocol::{CourseRole, CsrfToken, ServiceId},
    reference_test_support::{FixtureServer, Reply},
    session::SessionRegistry,
    transport::CampusHttpTransport,
    tunet_client::{TunetClient, TunetClientConfig},
};
use serde_json::{Value, json};

#[tokio::test]
async fn backend_repair_consistency_two_courses_unique_homework_keeps_course_associations() {
    let mut second = homework("h2", "Second course");
    second["wlkcid"] = json!("c2");
    let s = FixtureServer::new(vec![
        rows(vec![homework("h1", "First course")]),
        rows(vec![]),
        rows(vec![]),
        rows(vec![second]),
        rows(vec![]),
        rows(vec![]),
    ]);
    let courses=crate::learn_client::parse_course_records(r#"{"message":"success","resultList":[{"wlkcid":"c1","kch":"CODE1","kcm":"First"},{"wlkcid":"c2","kch":"CODE2","kcm":"Second"}]}"#).unwrap();
    let report = source(&s)
        .list_todos_for_courses(courses, TodoFilter::default())
        .await
        .unwrap();
    assert!(report.failures.is_empty());
    assert_eq!(report.items.len(), 2);
    assert_ne!(report.items[0].course_id, report.items[1].course_id);
    assert_eq!(s.requests().len(), 6);
}

#[tokio::test]
async fn backend_repair_consistency_homework_reused_id_across_courses_not_silently_dropped() {
    let mut second = homework("h1", "Second course");
    second["wlkcid"] = json!("c2");
    let s = FixtureServer::new(vec![
        rows(vec![homework("h1", "First course")]),
        rows(vec![]),
        rows(vec![]),
        rows(vec![second]),
        rows(vec![]),
        rows(vec![]),
    ]);
    let courses = crate::learn_client::parse_course_records(r#"{"message":"success","resultList":[{"wlkcid":"c1","kch":"CODE1","kcm":"First"},{"wlkcid":"c2","kch":"CODE2","kcm":"Second"}]}"#).unwrap();
    let report = source(&s)
        .list_todos_for_courses(courses, TodoFilter::default())
        .await
        .unwrap();
    assert!(!report.failures.is_empty());
    assert!(report.items.is_empty());
    assert_eq!(s.requests().len(), 6);
}

#[tokio::test]
async fn backend_repair_consistency_homework_valid_empty_buckets_remain_complete() {
    let s = FixtureServer::new(vec![rows(vec![]), rows(vec![]), rows(vec![])]);
    let report = source(&s)
        .list_todos_for_courses(course(), TodoFilter::default())
        .await
        .unwrap();
    assert!(report.failures.is_empty());
    assert!(report.items.is_empty());
    assert_eq!(s.requests().len(), 3);
}

#[tokio::test]
async fn backend_repair_consistency_homework_same_bucket_changed_deadline_is_conflict() {
    let first = homework("h1", "Fixture");
    let mut changed = first.clone();
    changed["jzsj"] = json!("2026-10-01 23:59:00");
    let s = FixtureServer::new(vec![rows(vec![first, changed]), rows(vec![]), rows(vec![])]);
    assert!(source(&s).list_course_homework("c1").await.is_err());
    assert_eq!(s.requests().len(), 3);
}

#[tokio::test]
async fn backend_repair_consistency_announcements_unique_active_and_expired_both_preserved() {
    let s = FixtureServer::new(vec![
        rows(vec![notice("n1", "Same title")]),
        rows(vec![notice("n2", "Same title")]),
    ]);
    let items = announcements(&s).list_course("c1").await.unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items.iter().filter(|n| n.expired).count(), 1);
    assert_eq!(s.requests().len(), 2);
}

#[tokio::test]
async fn backend_repair_consistency_network_account_mismatch_stops_before_retryable_unknown_state()
{
    let s = FixtureServer::new(vec![
        Reply::json(r#"{"error":"ok","ecode":0,"challenge":"synthetic-token"}"#),
        Reply::json(r#"{"error":"ok","ecode":0}"#),
        Reply::json(r#"{"error":"ok","user_name":"someone-else"}"#),
    ]);
    assert!(
        network_client(&s)
            .login_with_password_verified(
                "fixture-net",
                "synthetic-password",
                &[("ip", "192.0.2.10")]
            )
            .await
            .is_err()
    );
    assert_eq!(s.requests().len(), 3);
}

#[tokio::test]
async fn backend_repair_consistency_network_offline_blank_user_does_not_break_logout_proof() {
    let s = FixtureServer::new(vec![
        Reply::json(r#"{"error":"ok","ecode":0}"#),
        Reply::json(
            r#"{"error":"not_online_error","online_ip":"","online_device_total":0,"user_name":""}"#,
        ),
    ]);
    assert!(
        network_client(&s)
            .disconnect_verified("fixture-net", &[("ip", "192.0.2.10")])
            .await
            .unwrap()
            .is_offline_proven_for_ip("192.0.2.10")
    );
    assert_eq!(s.requests().len(), 2);
}

fn source(server: &FixtureServer) -> LearnTodoSource {
    let mut source = LearnTodoSource::new(
        LearnClient::new(LearnClientConfig::new(server.base(), CourseRole::Student).unwrap()),
        CampusHttpTransport::new("THYou/consistency-fixture").unwrap(),
        LearnTodoConfig::new("2026-2027-1"),
    )
    .unwrap();
    source
        .with_csrf(
            SessionRegistry::new()
                .bind_csrf(ServiceId::Learn, CsrfToken::new("synthetic-csrf").unwrap()),
            "_csrf",
        )
        .unwrap();
    source
}
fn course() -> Vec<LearnCourseRecord> {
    crate::learn_client::parse_course_records(
        r#"{"message":"success","resultList":[{"wlkcid":"c1","kch":"CODE1","kcm":"Fixture"}]}"#,
    )
    .unwrap()
}
fn homework(id: &str, title: &str) -> Value {
    json!({"xszyid":id,"zyid":format!("base-{id}"),"wlkcid":"c1","bt":title,"jzsj":"2026-09-30 23:59:00"})
}
fn rows(data: Vec<Value>) -> Reply {
    Reply::json(&json!({"result":"success","object":{"aaData":data}}).to_string())
}
fn announcements(server: &FixtureServer) -> LearnAnnouncementSource {
    let mut s = LearnAnnouncementSource::new(
        LearnClient::new(LearnClientConfig::new(server.base(), CourseRole::Student).unwrap()),
        CampusHttpTransport::new("THYou/consistency-fixture").unwrap(),
    );
    s.with_csrf(
        SessionRegistry::new()
            .bind_csrf(ServiceId::Learn, CsrfToken::new("synthetic-csrf").unwrap()),
    )
    .unwrap();
    s
}
fn notice(id: &str, title: &str) -> Value {
    json!({"ggid":id,"wlkcid":"c1","bt":title,"fbsj":"2026-09-17 12:00:00"})
}

#[tokio::test]
async fn backend_repair_consistency_homework_conflicting_buckets_not_returned_as_two_states() {
    let s = FixtureServer::new(vec![
        rows(vec![homework("h1", "Fixture")]),
        rows(vec![homework("h1", "Fixture")]),
        rows(vec![]),
    ]);
    assert!(source(&s).list_course_homework("c1").await.is_err());
    assert!(s.requests().len() <= 3);
}

#[tokio::test]
async fn backend_repair_consistency_homework_partial_report_removes_conflicts_and_marks_failure() {
    let s = FixtureServer::new(vec![
        rows(vec![
            homework("h1", "Old state"),
            homework("h2", "Consistent"),
        ]),
        rows(vec![homework("h1", "Changed state")]),
        rows(vec![]),
    ]);
    let report = source(&s)
        .list_todos_for_courses(
            course(),
            TodoFilter {
                include_completed: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(
        !report.failures.is_empty(),
        "never silently choose the first state"
    );
    assert_eq!(report.items.len(), 1);
    assert_eq!(report.items[0].title, "Consistent");
    assert_eq!(s.requests().len(), 3);
}

#[tokio::test]
async fn backend_repair_consistency_homework_identical_same_bucket_duplicates_collapse() {
    let row = homework("h1", "Fixture");
    let s = FixtureServer::new(vec![
        rows(vec![row.clone(), row]),
        rows(vec![]),
        rows(vec![]),
    ]);
    assert_eq!(
        source(&s).list_course_homework("c1").await.unwrap().len(),
        1
    );
    assert_eq!(s.requests().len(), 3);
}

#[tokio::test]
async fn backend_repair_consistency_homework_different_ids_remain_independent() {
    let s = FixtureServer::new(vec![
        rows(vec![homework("h1", "Same title")]),
        rows(vec![homework("h2", "Same title")]),
        rows(vec![]),
    ]);
    let report = source(&s)
        .list_todos_for_courses(
            course(),
            TodoFilter {
                include_completed: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(report.failures.is_empty());
    assert_eq!(report.items.len(), 2);
    assert_ne!(report.items[0].id, report.items[1].id);
}

#[tokio::test]
async fn backend_repair_consistency_announcements_conflicting_content_not_silently_first_wins() {
    let s = FixtureServer::new(vec![
        rows(vec![notice("n1", "Old")]),
        rows(vec![notice("n1", "New")]),
    ]);
    assert!(announcements(&s).list_course("c1").await.is_err());
    assert_eq!(s.requests().len(), 2);
}

#[tokio::test]
async fn backend_repair_consistency_announcements_identical_same_bucket_duplicates_collapse() {
    let row = notice("n1", "Fixture");
    let s = FixtureServer::new(vec![rows(vec![row.clone(), row]), rows(vec![])]);
    assert_eq!(announcements(&s).list_course("c1").await.unwrap().len(), 1);
    assert_eq!(s.requests().len(), 2);
}

fn network_client(server: &FixtureServer) -> TunetClient {
    let profile = crate::tunet::TunetProfile::current_with_overrides(
        crate::tunet::AuthFamily::Auth4,
        crate::tunet::TunetProfileOverrides {
            endpoint: Some(
                crate::tunet::HttpsEndpoint::http(
                    "127.0.0.1",
                    reqwest::Url::parse(server.base()).unwrap().port().unwrap(),
                )
                .unwrap(),
            ),
            ..Default::default()
        },
    )
    .unwrap();
    TunetClient::with_transport(
        TunetClientConfig::new(profile).unwrap(),
        CampusHttpTransport::new("THYou/consistency-fixture").unwrap(),
    )
    .unwrap()
}

#[tokio::test]
async fn backend_repair_consistency_network_verified_client_rejects_wrong_account_itself() {
    let s = FixtureServer::new(vec![
        Reply::json(r#"{"error":"ok","ecode":0,"challenge":"synthetic-token"}"#),
        Reply::json(r#"{"error":"ok","ecode":0}"#),
        Reply::json(r#"{"error":"ok","online_ip":"192.0.2.10","user_name":"someone-else"}"#),
    ]);
    assert!(
        network_client(&s)
            .login_with_password_verified(
                "fixture-net",
                "synthetic-password",
                &[("ip", "192.0.2.10")]
            )
            .await
            .is_err()
    );
    assert_eq!(
        s.requests().len(),
        3,
        "account mismatch is terminal, not another poll"
    );
}

#[tokio::test]
async fn backend_repair_consistency_network_verified_logout_rejects_other_account_proof() {
    let s = FixtureServer::new(vec![
        Reply::json(r#"{"error":"ok","ecode":0}"#),
        Reply::json(
            r#"{"error":"ok","online_ip":"","online_device_total":0,"user_name":"someone-else"}"#,
        ),
    ]);
    assert!(
        network_client(&s)
            .disconnect_verified("fixture-net", &[("ip", "192.0.2.10")])
            .await
            .is_err()
    );
    assert_eq!(s.requests().len(), 2);
}

#[tokio::test]
async fn backend_repair_consistency_network_verified_client_valid_account_still_succeeds() {
    let s = FixtureServer::new(vec![
        Reply::json(r#"{"error":"ok","ecode":0,"challenge":"synthetic-token"}"#),
        Reply::json(r#"{"error":"ok","ecode":0}"#),
        Reply::json(r#"{"error":"ok","online_ip":"192.0.2.10","user_name":"fixture-net"}"#),
    ]);
    assert!(
        network_client(&s)
            .login_with_password_verified(
                "fixture-net",
                "synthetic-password",
                &[("ip", "192.0.2.10")]
            )
            .await
            .unwrap()
            .is_online_proven()
    );
    assert_eq!(s.requests().len(), 3);
}
