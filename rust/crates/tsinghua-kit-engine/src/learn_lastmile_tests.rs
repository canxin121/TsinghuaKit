use super::*;
use crate::reference_test_support::{FixtureServer, Reply};
#[tokio::test]
async fn backend_repair_lastmile_learn_404_uses_reference_directory_route_once() {
    let server = FixtureServer::new(vec![
        Reply {
            status: 404,
            headers: String::new(),
            body: String::new(),
        },
        Reply::html("<html><meta name='_csrf' content='fixture-csrf'></html>"),
    ]);
    let mut client =
        LearnClient::new(LearnClientConfig::new(server.base(), CourseRole::Student).unwrap());
    let transport = CampusHttpTransport::new("THYou/lastmile").unwrap();
    let response = client
        .execute_reference_course_home(&transport)
        .await
        .unwrap();
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(
        client.config.course_home_path,
        "/f/wlxt/index/course/student/"
    );
    assert_eq!(server.requests().len(), 2);
    assert!(server.requests()[0].starts_with("GET /f/wlxt/index/course/student/index "));
    assert!(server.requests()[1].starts_with("GET /f/wlxt/index/course/student/ "));
    assert!(client.parse_csrf(response.body()).is_ok());
}
#[tokio::test]
async fn backend_repair_lastmile_learn_auth_failure_does_not_try_other_routes() {
    for status in [401, 403, 429, 500] {
        let server = FixtureServer::new(vec![Reply {
            status,
            headers: String::new(),
            body: "fixture failure".into(),
        }]);
        let mut client =
            LearnClient::new(LearnClientConfig::new(server.base(), CourseRole::Student).unwrap());
        let response = client
            .execute_reference_course_home(&CampusHttpTransport::new("THYou/lastmile").unwrap())
            .await
            .unwrap();
        assert_eq!(response.status.as_u16(), status);
        assert_eq!(server.requests().len(), 1);
    }
}

#[tokio::test]
async fn backend_repair_lastmile_learn_server_redirect_to_documented_home_is_not_replayed() {
    let server = FixtureServer::new(vec![
        Reply {
            status: 302,
            headers: "Location: /f/wlxt/index/course/student/\r\n".into(),
            body: String::new(),
        },
        Reply::html("<meta name='_csrf' content='fixture'>"),
    ]);
    let mut client =
        LearnClient::new(LearnClientConfig::new(server.base(), CourseRole::Student).unwrap());
    let response = client
        .execute_reference_course_home(&CampusHttpTransport::new("THYou/home-redirect").unwrap())
        .await
        .unwrap();
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(
        client.config.course_home_path,
        "/f/wlxt/index/course/student/"
    );
    assert_eq!(server.requests().len(), 2);
}
#[tokio::test]
async fn backend_repair_lastmile_learn_missing_both_homes_stops_after_two_gets() {
    let server = FixtureServer::new(vec![
        Reply {
            status: 404,
            headers: String::new(),
            body: String::new(),
        },
        Reply {
            status: 404,
            headers: String::new(),
            body: String::new(),
        },
    ]);
    let mut client =
        LearnClient::new(LearnClientConfig::new(server.base(), CourseRole::Student).unwrap());
    let response = client
        .execute_reference_course_home(&CampusHttpTransport::new("THYou/home-missing").unwrap())
        .await
        .unwrap();
    assert_eq!(response.status, StatusCode::NOT_FOUND);
    assert_eq!(server.requests().len(), 2);
}
#[tokio::test]
async fn backend_repair_lastmile_learn_student_cannot_accept_teacher_home_or_wrong_mapping() {
    for target in ["/f/wlxt/index/course/teacher/", "/f/unrelated"] {
        let server = FixtureServer::new(vec![
            Reply {
                status: 302,
                headers: format!("Location: {target}\r\n"),
                body: String::new(),
            },
            Reply::html("<meta name='_csrf' content='fixture'>"),
        ]);
        let mut client =
            LearnClient::new(LearnClientConfig::new(server.base(), CourseRole::Student).unwrap());
        assert!(
            client
                .execute_reference_course_home(
                    &CampusHttpTransport::new("THYou/home-role").unwrap()
                )
                .await
                .is_err()
        );
        assert_eq!(server.requests().len(), 2);
    }
}
