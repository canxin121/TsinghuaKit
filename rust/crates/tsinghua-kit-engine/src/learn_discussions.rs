//! Read-only course discussion topics through the proven Learn session.
//! Upstream topic and board IDs never cross the Flutter bridge.

use std::collections::HashSet;

use reqwest::{StatusCode, header::LOCATION};
use serde_json::{Map, Value};
use thiserror::Error;

use crate::{
    learn_client::{
        LearnClient, LearnClientError, LearnPageClassification, decode_learn_html,
        is_login_failure_message,
    },
    protocol::{CourseRole, ServiceId},
    session::BoundCsrfToken,
    transport::CampusHttpTransport,
};

const STUDENT_PATH: &str = "/b/wlxt/bbs/bbs_tltb/student/kctlList";
const TEACHER_PATH: &str = "/b/wlxt/bbs/bbs_tltb/teacher/kctlList";
const LIMIT: usize = 200;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LearnDiscussionRecord {
    pub selector: String,
    pub title: String,
    pub publisher_name: String,
    pub published_at: String,
    pub last_reply_at: Option<String>,
    pub reply_count: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LearnDiscussionRead {
    pub items: Vec<LearnDiscussionRecord>,
    pub complete: bool,
}

#[derive(Debug, Error)]
pub(crate) enum LearnDiscussionError {
    #[error("Learn discussion session expired")]
    Session,
    #[error("Learn discussion route or CSRF proof rejected")]
    Route,
    #[error("Learn discussion network request failed")]
    Network,
    #[error("Learn discussion HTTP status {0}")]
    Http(u16),
    #[error("Learn discussion response was not a verified list")]
    Response,
}

pub(crate) async fn read_course_discussions(
    learn: &LearnClient,
    transport: &CampusHttpTransport,
    csrf: &BoundCsrfToken,
    csrf_parameter: &str,
    course_id: &str,
) -> Result<LearnDiscussionRead, LearnDiscussionError> {
    if csrf.service() != ServiceId::Learn
        || csrf_parameter != "_csrf"
        || course_id.is_empty()
        || course_id.len() > 256
        || course_id.chars().any(char::is_control)
    {
        return Err(LearnDiscussionError::Route);
    }
    let path = match learn.config().role() {
        CourseRole::Student => STUDENT_PATH,
        CourseRole::Teacher => TEACHER_PATH,
    };
    let mut url = learn
        .resolve_endpoint(path)
        .map_err(|_| LearnDiscussionError::Route)?;
    url.query_pairs_mut()
        .append_pair("wlkcid", course_id)
        .append_pair("size", "200")
        .append_pair(csrf_parameter, csrf.as_csrf_token().as_str());
    let request = transport
        .client()
        .get(url.clone())
        .build()
        .map_err(|_| LearnDiscussionError::Route)?;
    let response = transport
        .execute_once(transport.client(), request)
        .await
        .map_err(|_| LearnDiscussionError::Network)?;
    let status = response.status();
    let final_url = response.url().clone();
    let redirect = response
        .headers()
        .get(LOCATION)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let body = crate::telemetry::timing::read_text(response)
        .await
        .map_err(|_| LearnDiscussionError::Network)?;
    let mapped = learn
        .map_html_response_with_redirect(status, Some(&final_url), redirect.as_deref(), &body)
        .map_err(|error| match error {
            LearnClientError::SessionExpired => LearnDiscussionError::Session,
            _ => LearnDiscussionError::Route,
        })?;
    if matches!(
        mapped.classification,
        LearnPageClassification::LoginExpired(_)
    ) {
        return Err(LearnDiscussionError::Session);
    }
    if final_url != url || redirect.is_some() {
        return Err(LearnDiscussionError::Route);
    }
    if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
        return Err(LearnDiscussionError::Session);
    }
    if status != StatusCode::OK {
        return Err(LearnDiscussionError::Http(status.as_u16()));
    }
    parse_discussion_list(&body, course_id)
}

pub(crate) fn parse_discussion_list(
    body: &str,
    course_id: &str,
) -> Result<LearnDiscussionRead, LearnDiscussionError> {
    let value: Value = serde_json::from_str(body).map_err(|_| LearnDiscussionError::Response)?;
    let root = value.as_object().ok_or(LearnDiscussionError::Response)?;
    if root.get("result").and_then(Value::as_str) != Some("success") {
        let message = root
            .get("msg")
            .or_else(|| root.get("message"))
            .and_then(Value::as_str)
            .unwrap_or("");
        return Err(if is_login_failure_message(message) {
            LearnDiscussionError::Session
        } else {
            LearnDiscussionError::Response
        });
    }
    let object = root
        .get("object")
        .and_then(Value::as_object)
        .ok_or(LearnDiscussionError::Response)?;
    let rows = object
        .get("resultsList")
        .and_then(Value::as_array)
        .ok_or(LearnDiscussionError::Response)?;
    if rows.len() > LIMIT {
        return Err(LearnDiscussionError::Response);
    }
    let mut seen = HashSet::new();
    let mut items = Vec::with_capacity(rows.len());
    for row in rows {
        let row = row.as_object().ok_or(LearnDiscussionError::Response)?;
        let id = required(row, "id")?;
        let board_id = required(row, "bqid")?;
        if required(row, "wlkcid")? != course_id {
            return Err(LearnDiscussionError::Response);
        }
        let title = decode_learn_html(&required(row, "bt")?);
        if title.trim().is_empty() || title.len() > 1024 || title.chars().any(char::is_control) {
            return Err(LearnDiscussionError::Response);
        }
        let publisher_name = decode_learn_html(&required(row, "fbrxm")?);
        if publisher_name.trim().is_empty()
            || publisher_name.len() > 256
            || publisher_name.chars().any(char::is_control)
        {
            return Err(LearnDiscussionError::Response);
        }
        let published_at = required(row, "fbsj")?;
        let last_reply_at = optional(row, "zhhfsj")?;
        let reply_count = match row.get("hfcs") {
            Some(Value::Number(value)) => {
                value.as_u64().and_then(|value| u32::try_from(value).ok())
            }
            Some(Value::String(value)) => value.parse::<u32>().ok(),
            _ => None,
        }
        .filter(|count| *count <= 1_000_000)
        .ok_or(LearnDiscussionError::Response)?;
        let selector = crate::learn_todos::stable_uuid(
            "learn-discussion",
            &serde_json::to_string(&(course_id, id, board_id))
                .map_err(|_| LearnDiscussionError::Response)?,
        )
        .to_string();
        if !seen.insert(selector.clone()) {
            return Err(LearnDiscussionError::Response);
        }
        items.push(LearnDiscussionRecord {
            selector,
            title,
            publisher_name,
            published_at,
            last_reply_at,
            reply_count,
        });
    }
    Ok(LearnDiscussionRead {
        complete: items.len() < LIMIT,
        items,
    })
}

fn required(row: &Map<String, Value>, key: &str) -> Result<String, LearnDiscussionError> {
    optional(row, key)?
        .filter(|value| !value.is_empty())
        .ok_or(LearnDiscussionError::Response)
}

fn optional(row: &Map<String, Value>, key: &str) -> Result<Option<String>, LearnDiscussionError> {
    let text = match row.get(key) {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::String(value)) => value.trim().to_owned(),
        Some(Value::Number(value)) => value.to_string(),
        _ => return Err(LearnDiscussionError::Response),
    };
    if text.len() > 8192 || text.chars().any(char::is_control) {
        return Err(LearnDiscussionError::Response);
    }
    Ok((!text.is_empty()).then_some(text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        learn_client::LearnClientConfig,
        protocol::CsrfToken,
        reference_test_support::{FixtureServer, Reply},
        session::SessionRegistry,
    };

    #[tokio::test]
    async fn backend_repair_learn_discussions_use_fixed_role_route_and_bound_csrf() {
        for (role, path) in [
            (CourseRole::Student, STUDENT_PATH),
            (CourseRole::Teacher, TEACHER_PATH),
        ] {
            let server = FixtureServer::new(vec![Reply::json(
                r#"{"result":"success","object":{"resultsList":[{"id":"private-topic","bqid":"private-board","wlkcid":"fixture-course","bt":"讨论 &amp; 交流","fbrxm":"合成教师","fbsj":"2026-09-24 08:00","hfcs":2}]}}"#,
            )]);
            let learn = LearnClient::new(LearnClientConfig::new(server.base(), role).unwrap());
            let transport = CampusHttpTransport::new("fixture-learn-discussions").unwrap();
            let csrf = SessionRegistry::new()
                .bind_csrf(ServiceId::Learn, CsrfToken::new("fixture-csrf").unwrap());
            let read =
                read_course_discussions(&learn, &transport, &csrf, "_csrf", "fixture-course")
                    .await
                    .unwrap();
            assert!(read.complete);
            assert_eq!(read.items.len(), 1);
            assert_eq!(read.items[0].title, "讨论 & 交流");
            assert_eq!(read.items[0].reply_count, 2);
            assert!(!format!("{read:?}").contains("private-topic"));
            let requests = server.requests();
            assert_eq!(requests.len(), 1);
            assert!(requests[0].starts_with(&format!("GET {path}?")));
            assert!(requests[0].contains("wlkcid=fixture-course"));
            assert!(requests[0].contains("size=200"));
            assert!(requests[0].contains("_csrf=fixture-csrf"));
        }
    }

    #[tokio::test]
    async fn backend_repair_learn_discussions_reject_foreign_course_and_unproved_empty() {
        let foreign = r#"{"result":"success","object":{"resultsList":[{"id":"topic","bqid":"board","wlkcid":"other-course","bt":"讨论","fbrxm":"合成教师","fbsj":"2026-09-24","hfcs":0}]}}"#;
        assert!(matches!(
            parse_discussion_list(foreign, "fixture-course"),
            Err(LearnDiscussionError::Response)
        ));
        assert!(matches!(
            parse_discussion_list(r#"{"result":"success","object":{}}"#, "fixture-course"),
            Err(LearnDiscussionError::Response)
        ));
        let empty = parse_discussion_list(
            r#"{"result":"success","object":{"resultsList":[]}}"#,
            "fixture-course",
        )
        .unwrap();
        assert!(empty.complete && empty.items.is_empty());
    }

    #[tokio::test]
    async fn backend_repair_learn_discussions_reject_redirect_login_and_foreign_csrf() {
        let server = FixtureServer::new(vec![
            Reply {
                status: 302,
                headers: "Location: https://example.invalid/external\r\n".into(),
                body: String::new(),
            },
            Reply::html("<html>login</html>"),
        ]);
        let learn =
            LearnClient::new(LearnClientConfig::new(server.base(), CourseRole::Student).unwrap());
        let transport = CampusHttpTransport::new("fixture-learn-discussions-reject").unwrap();
        let registry = SessionRegistry::new();
        let foreign = registry.bind_csrf(ServiceId::Info, CsrfToken::new("fixture-csrf").unwrap());
        assert!(matches!(
            read_course_discussions(&learn, &transport, &foreign, "_csrf", "fixture-course").await,
            Err(LearnDiscussionError::Route)
        ));
        assert!(server.requests().is_empty());
        let csrf = registry.bind_csrf(ServiceId::Learn, CsrfToken::new("fixture-csrf").unwrap());
        assert!(matches!(
            read_course_discussions(&learn, &transport, &csrf, "_csrf", "fixture-course").await,
            Err(LearnDiscussionError::Route)
        ));
        assert!(matches!(
            read_course_discussions(&learn, &transport, &csrf, "_csrf", "fixture-course").await,
            Err(LearnDiscussionError::Session | LearnDiscussionError::Response)
        ));
        assert_eq!(server.requests().len(), 2);
    }
}
