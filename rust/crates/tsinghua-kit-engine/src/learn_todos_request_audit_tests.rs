use super::*;
use crate::transport::CampusHttpTransport;
use crate::{
    learn_client::LearnClientConfig,
    protocol::{CourseRole, CsrfToken},
    reference_test_support::{FixtureServer, Reply},
    session::SessionRegistry,
};

fn courses() -> Vec<LearnCourseRecord> {
    (0..7)
        .map(|index| LearnCourseRecord {
            course_id: Some(format!("{}", 7001 + index)),
            course_code: Some(format!("C{index}")),
            name: Some(format!("fixture {index}")),
            instructor: None,
            class_name: None,
            semester: Some("2026-2027-1".into()),
            unknown_fields: Default::default(),
        })
        .collect()
}

fn source(server: &FixtureServer) -> LearnTodoSource {
    let learn =
        LearnClient::new(LearnClientConfig::new(server.base(), CourseRole::Student).unwrap());
    let mut source = LearnTodoSource::new(
        learn,
        CampusHttpTransport::new("fixture-audit").unwrap(),
        LearnTodoConfig::new("2026-2027-1"),
    )
    .unwrap();
    let registry = SessionRegistry::new();
    source
        .with_csrf(
            registry.bind_csrf(ServiceId::Learn, CsrfToken::new("fixture-csrf").unwrap()),
            "_csrf",
        )
        .unwrap();
    source
}

fn empty() -> Reply {
    Reply::json(r#"{"result":"success","object":{"aaData":[]}}"#)
}

#[tokio::test]
async fn backend_repair_request_audit_all_seven_courses_request_each_bucket_once() {
    let server = FixtureServer::new((0..21).map(|_| empty()).collect());
    let source = source(&server);
    let (_, metrics) = crate::telemetry::timing::capture(async {
        let result = source
            .list_todos_for_courses(
                courses(),
                TodoFilter {
                    include_completed: true,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert!(result.items.is_empty() && result.failures.is_empty());
    })
    .await;
    let requests = server.requests();
    assert_eq!(requests.len(), 21);
    assert_eq!(metrics.requests, 21);
    for bucket in ["zyListWj", "zyListYjwg", "zyListYpg"] {
        assert_eq!(
            requests
                .iter()
                .filter(|request| request.lines().next().unwrap().contains(bucket))
                .count(),
            7
        );
    }
    for course in courses() {
        let id = course.id().unwrap();
        assert_eq!(
            requests
                .iter()
                .filter(|request| request.contains(&format!("\"value\":\"{id}\"")))
                .count(),
            3
        );
    }
    assert!(
        !requests
            .iter()
            .any(|request| request.contains("loadCourseBySemesterId"))
    );
}

#[tokio::test]
async fn backend_repair_request_audit_expired_shared_session_stops_remaining_course_fanout() {
    let server = FixtureServer::new(vec![Reply {
        status: 401,
        headers: "Content-Type: application/json\r\n".into(),
        body: "{}".into(),
    }]);
    let result = source(&server)
        .list_todos_for_courses(courses(), TodoFilter::default())
        .await;
    assert!(matches!(
        result,
        Err(ServiceError::SessionExpired {
            service: ServiceId::Learn
        })
    ));
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn backend_repair_request_audit_one_business_failure_is_reported_without_skipping_other_courses()
 {
    let server = FixtureServer::new(
        (0..21)
            .map(|index| {
                if index == 5 {
                    Reply::json(r#"{"result":"error","msg":"fixture business failure"}"#)
                } else {
                    empty()
                }
            })
            .collect(),
    );
    let result = source(&server)
        .list_todos_for_courses(courses(), TodoFilter::default())
        .await
        .unwrap();
    assert_eq!(result.failures.len(), 1);
    assert_eq!(server.requests().len(), 21);
}
