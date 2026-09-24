//! Read-only Learn course-file metadata. Raw download IDs and URLs stay in
//! Rust; the bridge receives only opaque file selectors and safe labels.

use std::{collections::HashSet, fmt};

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

const STUDENT_PATH: &str = "/b/wlxt/kj/wlkc_kjxxb/student/kjxxbByWlkcidAndSizeForStudent";
const TEACHER_PATH: &str = "/b/wlxt/kj/v_kjxxb_wjwjb/teacher/queryByWlkcid";
const STUDENT_CATEGORY_PATH: &str = "/b/wlxt/kj/wlkc_kjflb/student/pageList";
const TEACHER_CATEGORY_PATH: &str = "/b/wlxt/kj/wlkc_kjflb/teacher/pageList";
const LIMIT: usize = 200;

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct LearnFileRecord {
    pub id: String,
    pub raw_file_id: String,
    pub title: String,
    pub description: Option<String>,
    pub size: Option<String>,
    pub uploaded_at: Option<String>,
    pub file_type: Option<String>,
    pub category_selector: Option<String>,
}

impl fmt::Debug for LearnFileRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LearnFileRecord")
            .field("id", &self.id)
            .field("title", &self.title)
            .field("category_selector", &self.category_selector)
            .finish_non_exhaustive()
    }
}

pub(crate) fn suggested_filename(title: &str, file_type: Option<&str>) -> String {
    let mut name: String = title
        .chars()
        .map(|character| {
            if character.is_control()
                || matches!(
                    character,
                    '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'
                )
            {
                '_'
            } else {
                character
            }
        })
        .take(120)
        .collect();
    name = name.trim().trim_matches('.').trim().to_owned();
    if name.is_empty() {
        name = "课程资料".into();
    }
    if let Some(extension) = file_type.map(str::trim).filter(|value| {
        !value.is_empty()
            && value.len() <= 10
            && value
                .chars()
                .all(|character| character.is_ascii_alphanumeric())
    }) {
        let suffix = format!(".{extension}");
        if !name
            .to_ascii_lowercase()
            .ends_with(&suffix.to_ascii_lowercase())
        {
            name.push_str(&suffix);
        }
    }
    name
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LearnFileCategoryRecord {
    pub selector: String,
    pub title: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LearnFileRead {
    pub files: Vec<LearnFileRecord>,
    pub complete: bool,
}

#[derive(Debug, Error)]
pub(crate) enum LearnFileError {
    #[error("Learn course-file session expired")]
    Session,
    #[error("Learn course-file route or CSRF proof rejected")]
    Route,
    #[error("Learn course-file network request failed")]
    Network,
    #[error("Learn course-file HTTP status {0}")]
    Http(u16),
    #[error("Learn course-file response was not a verified list")]
    Response,
}

pub(crate) async fn read_course_files(
    learn: &LearnClient,
    transport: &CampusHttpTransport,
    csrf: &BoundCsrfToken,
    csrf_parameter: &str,
    course_id: &str,
) -> Result<LearnFileRead, LearnFileError> {
    if csrf.service() != ServiceId::Learn
        || csrf_parameter != "_csrf"
        || course_id.is_empty()
        || course_id.len() > 256
        || course_id.chars().any(char::is_control)
    {
        return Err(LearnFileError::Route);
    }
    let path = match learn.config().role() {
        CourseRole::Student => STUDENT_PATH,
        CourseRole::Teacher => TEACHER_PATH,
    };
    let mut url = learn
        .resolve_endpoint(path)
        .map_err(|_| LearnFileError::Route)?;
    url.query_pairs_mut()
        .append_pair("wlkcid", course_id)
        .append_pair("size", "200")
        .append_pair(csrf_parameter, csrf.as_csrf_token().as_str());
    let request = transport
        .client()
        .get(url.clone())
        .build()
        .map_err(|_| LearnFileError::Route)?;
    let response = transport
        .execute_once(transport.client(), request)
        .await
        .map_err(|_| LearnFileError::Network)?;
    let status = response.status();
    let final_url = response.url().clone();
    let redirect = response
        .headers()
        .get(LOCATION)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let body = crate::telemetry::timing::read_text(response)
        .await
        .map_err(|_| LearnFileError::Network)?;
    let mapped = learn
        .map_html_response_with_redirect(status, Some(&final_url), redirect.as_deref(), &body)
        .map_err(|error| match error {
            LearnClientError::SessionExpired => LearnFileError::Session,
            _ => LearnFileError::Route,
        })?;
    if matches!(
        mapped.classification,
        LearnPageClassification::LoginExpired(_)
    ) {
        return Err(LearnFileError::Session);
    }
    if final_url != url || redirect.is_some() {
        return Err(LearnFileError::Route);
    }
    if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
        return Err(LearnFileError::Session);
    }
    if status != StatusCode::OK {
        return Err(LearnFileError::Http(status.as_u16()));
    }
    parse_file_list(&body, course_id)
}

pub(crate) async fn read_file_categories(
    learn: &LearnClient,
    transport: &CampusHttpTransport,
    csrf: &BoundCsrfToken,
    csrf_parameter: &str,
    course_id: &str,
) -> Result<Vec<LearnFileCategoryRecord>, LearnFileError> {
    if csrf.service() != ServiceId::Learn
        || csrf_parameter != "_csrf"
        || course_id.is_empty()
        || course_id.len() > 256
        || course_id.chars().any(char::is_control)
    {
        return Err(LearnFileError::Route);
    }
    let path = match learn.config().role() {
        CourseRole::Student => STUDENT_CATEGORY_PATH,
        CourseRole::Teacher => TEACHER_CATEGORY_PATH,
    };
    let mut url = learn
        .resolve_endpoint(path)
        .map_err(|_| LearnFileError::Route)?;
    url.query_pairs_mut()
        .append_pair("wlkcid", course_id)
        .append_pair(csrf_parameter, csrf.as_csrf_token().as_str());
    let request = transport
        .client()
        .get(url.clone())
        .build()
        .map_err(|_| LearnFileError::Route)?;
    let response = transport
        .execute_once(transport.client(), request)
        .await
        .map_err(|_| LearnFileError::Network)?;
    let status = response.status();
    let final_url = response.url().clone();
    let redirect = response
        .headers()
        .get(LOCATION)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let body = crate::telemetry::timing::read_text(response)
        .await
        .map_err(|_| LearnFileError::Network)?;
    let mapped = learn
        .map_html_response_with_redirect(status, Some(&final_url), redirect.as_deref(), &body)
        .map_err(|error| match error {
            LearnClientError::SessionExpired => LearnFileError::Session,
            _ => LearnFileError::Route,
        })?;
    if matches!(
        mapped.classification,
        LearnPageClassification::LoginExpired(_)
    ) || matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN)
    {
        return Err(LearnFileError::Session);
    }
    if final_url != url || redirect.is_some() {
        return Err(LearnFileError::Route);
    }
    if status != StatusCode::OK {
        return Err(LearnFileError::Http(status.as_u16()));
    }
    parse_file_categories(&body, course_id)
}

pub(crate) fn parse_file_categories(
    body: &str,
    course_id: &str,
) -> Result<Vec<LearnFileCategoryRecord>, LearnFileError> {
    let value: Value = serde_json::from_str(body).map_err(|_| LearnFileError::Response)?;
    let root = value.as_object().ok_or(LearnFileError::Response)?;
    if root.get("result").and_then(Value::as_str) != Some("success") {
        let message = root
            .get("msg")
            .or_else(|| root.get("message"))
            .and_then(Value::as_str)
            .unwrap_or("");
        return Err(if is_login_failure_message(message) {
            LearnFileError::Session
        } else {
            LearnFileError::Response
        });
    }
    let object = root
        .get("object")
        .and_then(Value::as_object)
        .ok_or(LearnFileError::Response)?;
    let rows = object
        .get("rows")
        .and_then(Value::as_array)
        .ok_or(LearnFileError::Response)?;
    // A full bounded page cannot prove that the next page is absent. If the
    // deployment supplies an explicit total, it must agree with the rows.
    if rows.len() >= LIMIT {
        return Err(LearnFileError::Response);
    }
    for key in ["total", "recordsTotal", "iTotalRecords"] {
        if let Some(total) = object.get(key) {
            let total = total.as_u64().ok_or(LearnFileError::Response)?;
            if total != rows.len() as u64 {
                return Err(LearnFileError::Response);
            }
        }
    }
    let mut seen = HashSet::new();
    let mut categories = Vec::with_capacity(rows.len());
    for row in rows {
        let row = row.as_object().ok_or(LearnFileError::Response)?;
        let raw_id = required(row, "kjflid")?;
        let title = decode_learn_html(&required(row, "bt")?);
        if title.trim().is_empty() || title.len() > 1024 || title.chars().any(char::is_control) {
            return Err(LearnFileError::Response);
        }
        let selector = category_selector(course_id, &raw_id)?;
        if !seen.insert(selector.clone()) {
            return Err(LearnFileError::Response);
        }
        categories.push(LearnFileCategoryRecord { selector, title });
    }
    Ok(categories)
}

fn category_selector(course_id: &str, raw_id: &str) -> Result<String, LearnFileError> {
    Ok(crate::learn_todos::stable_uuid(
        "learn-file-category",
        &serde_json::to_string(&(course_id, raw_id)).map_err(|_| LearnFileError::Response)?,
    )
    .to_string())
}

pub(crate) fn parse_file_list(
    body: &str,
    course_id: &str,
) -> Result<LearnFileRead, LearnFileError> {
    let value: Value = serde_json::from_str(body).map_err(|_| LearnFileError::Response)?;
    let root = value.as_object().ok_or(LearnFileError::Response)?;
    if root.get("result").and_then(Value::as_str) != Some("success") {
        let message = root
            .get("msg")
            .or_else(|| root.get("message"))
            .and_then(Value::as_str)
            .unwrap_or("");
        return Err(if is_login_failure_message(message) {
            LearnFileError::Session
        } else {
            LearnFileError::Response
        });
    }
    let object = root.get("object").ok_or(LearnFileError::Response)?;
    let rows = if let Some(rows) = object.as_array() {
        rows
    } else {
        object
            .as_object()
            .and_then(|value| value.get("resultsList"))
            .and_then(Value::as_array)
            .ok_or(LearnFileError::Response)?
    };
    if rows.len() > LIMIT {
        return Err(LearnFileError::Response);
    }
    let mut seen = HashSet::new();
    let mut files = Vec::with_capacity(rows.len());
    for row in rows {
        let row = row.as_object().ok_or(LearnFileError::Response)?;
        let record_id = required(row, "kjxxid")?;
        let file_id = required(row, "wjid")?;
        let title = decode_learn_html(&required(row, "bt")?);
        if title.trim().is_empty() || title.len() > 1024 || title.chars().any(char::is_control) {
            return Err(LearnFileError::Response);
        }
        let id = crate::learn_todos::stable_uuid(
            "learn-file",
            &serde_json::to_string(&(course_id, &record_id, &file_id))
                .map_err(|_| LearnFileError::Response)?,
        )
        .to_string();
        if !seen.insert(id.clone()) {
            return Err(LearnFileError::Response);
        }
        files.push(LearnFileRecord {
            id,
            raw_file_id: file_id,
            title,
            description: optional_multiline(row, "ms")?
                .map(|value| {
                    let decoded = decode_learn_html(&value);
                    if decoded.chars().any(|character| {
                        character.is_control() && !matches!(character, '\n' | '\r' | '\t')
                    }) {
                        return Err(LearnFileError::Response);
                    }
                    Ok(decoded.split_whitespace().collect::<Vec<_>>().join(" "))
                })
                .transpose()?
                .filter(|value| !value.is_empty()),
            size: optional(row, "fileSize")?,
            uploaded_at: optional(row, "scsj")?,
            file_type: optional(row, "wjlx")?,
            category_selector: optional(row, "kjflid")?
                .map(|raw_id| category_selector(course_id, &raw_id))
                .transpose()?,
        });
    }
    Ok(LearnFileRead {
        complete: files.len() < LIMIT,
        files,
    })
}

fn required(row: &Map<String, Value>, key: &str) -> Result<String, LearnFileError> {
    optional(row, key)?
        .filter(|value| !value.is_empty())
        .ok_or(LearnFileError::Response)
}

fn optional(row: &Map<String, Value>, key: &str) -> Result<Option<String>, LearnFileError> {
    optional_with_policy(row, key, false)
}

fn optional_multiline(
    row: &Map<String, Value>,
    key: &str,
) -> Result<Option<String>, LearnFileError> {
    optional_with_policy(row, key, true)
}

fn optional_with_policy(
    row: &Map<String, Value>,
    key: &str,
    allow_multiline: bool,
) -> Result<Option<String>, LearnFileError> {
    let text = match row.get(key) {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::String(value)) => value.trim().to_owned(),
        Some(Value::Number(value)) => value.to_string(),
        _ => return Err(LearnFileError::Response),
    };
    if text.len() > 8192
        || text.chars().any(|character| {
            character.is_control() && !(allow_multiline && matches!(character, '\n' | '\r' | '\t'))
        })
    {
        return Err(LearnFileError::Response);
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
    async fn backend_repair_learn_files_use_fixed_role_route_and_shared_csrf() {
        for (role, path, body) in [
            (
                CourseRole::Student,
                STUDENT_PATH,
                r#"{"result":"success","object":[{"kjxxid":"one","wjid":"private-file","bt":"课程资料"}]}"#,
            ),
            (
                CourseRole::Teacher,
                TEACHER_PATH,
                r#"{"result":"success","object":{"resultsList":[{"kjxxid":"one","wjid":"private-file","bt":"课程资料"}]}}"#,
            ),
        ] {
            let server = FixtureServer::new(vec![Reply::json(body)]);
            let learn = LearnClient::new(LearnClientConfig::new(server.base(), role).unwrap());
            let transport = CampusHttpTransport::new("fixture-learn-files").unwrap();
            let csrf = SessionRegistry::new()
                .bind_csrf(ServiceId::Learn, CsrfToken::new("fixture-csrf").unwrap());
            let result = read_course_files(&learn, &transport, &csrf, "_csrf", "course-one")
                .await
                .unwrap();
            assert!(result.complete && result.files.len() == 1);
            let requests = server.requests();
            assert_eq!(requests.len(), 1);
            assert!(requests[0].starts_with(&format!("GET {path}?")));
            assert!(requests[0].contains("wlkcid=course-one"));
            assert!(requests[0].contains("size=200"));
            assert!(requests[0].contains("_csrf=fixture-csrf"));
        }
    }

    #[tokio::test]
    async fn backend_repair_learn_files_reject_foreign_csrf_and_login_html() {
        let server = FixtureServer::new(vec![Reply::html(
            r#"<form action="/do/off/ui/auth/login/check"><input name="i_user"></form>"#,
        )]);
        let learn =
            LearnClient::new(LearnClientConfig::new(server.base(), CourseRole::Student).unwrap());
        let transport = CampusHttpTransport::new("fixture-learn-files").unwrap();
        let registry = SessionRegistry::new();
        let foreign = registry.bind_csrf(ServiceId::Info, CsrfToken::new("fixture-csrf").unwrap());
        assert!(matches!(
            read_course_files(&learn, &transport, &foreign, "_csrf", "course-one").await,
            Err(LearnFileError::Route)
        ));
        assert!(server.requests().is_empty());
        let csrf = registry.bind_csrf(ServiceId::Learn, CsrfToken::new("fixture-csrf").unwrap());
        assert!(matches!(
            read_course_files(&learn, &transport, &csrf, "_csrf", "course-one").await,
            Err(LearnFileError::Session)
        ));
    }

    #[test]
    fn backend_repair_learn_files_require_success_and_explicit_collection() {
        let parsed = parse_file_list(r#"{"result":"success","object":[{"kjxxid":"one","wjid":"private-file","bt":"课程&amp;资料","fileSize":"2 MB","ms":"章节说明"}]}"#, "course-one").unwrap();
        assert!(parsed.complete);
        assert_eq!(parsed.files[0].title, "课程&资料");
        assert_eq!(parsed.files[0].description.as_deref(), Some("章节说明"));
        assert!(!format!("{:?}", parsed.files[0]).contains("private-file"));
        for body in [
            r#"{"result":"success","object":null}"#,
            r#"{"result":"success","object":{"resultsList":null}}"#,
            r#"{"result":"success","object":[{"kjxxid":"one","bt":"资料"}]}"#,
            r#"{"result":"error","object":[]}"#,
            "<html>login</html>",
        ] {
            assert!(parse_file_list(body, "course-one").is_err());
        }
        assert!(matches!(
            parse_file_list(r#"{"result":"error","message":"请重新登录"}"#, "course-one"),
            Err(LearnFileError::Session)
        ));
    }

    #[test]
    fn backend_repair_learn_files_limit_is_partial_and_duplicate_is_rejected() {
        let rows = (0..200)
            .map(|index| {
                serde_json::json!({
                    "kjxxid": format!("row-{index}"),
                    "wjid": format!("file-{index}"),
                    "bt": "资料",
                })
            })
            .collect::<Vec<_>>();
        let body = serde_json::json!({"result":"success","object":rows}).to_string();
        let read = parse_file_list(&body, "course-one").unwrap();
        assert_eq!(read.files.len(), 200);
        assert!(!read.complete);
        assert!(parse_file_list(
            r#"{"result":"success","object":[{"kjxxid":"one","wjid":"file","bt":"资料"},{"kjxxid":"one","wjid":"file","bt":"重复"}]}"#,
            "course-one"
        ).is_err());
        assert!(parse_file_list(
            r#"{"result":"success","object":[{"kjxxid":"one","wjid":"file","bt":"&#0;坏标题"}]}"#,
            "course-one"
        ).is_err());
    }

    #[test]
    fn backend_repair_learn_files_normalizes_multiline_description_without_control_leak() {
        let read = parse_file_list(
            "{\"result\":\"success\",\"object\":[{\"kjxxid\":\"one\",\"wjid\":\"file\",\"bt\":\"标题\",\"ms\":\"第一行\\n第二行\"}]}",
            "course-one",
        )
        .unwrap();
        assert_eq!(read.files[0].description.as_deref(), Some("第一行 第二行"));
        assert!(parse_file_list(
            r#"{"result":"success","object":[{"kjxxid":"one","wjid":"file","bt":"标题","ms":"&#0;隐藏控制字符"}]}"#,
            "course-one",
        ).is_err());
    }

    #[tokio::test]
    async fn backend_repair_learn_file_categories_use_fixed_role_routes_and_shared_csrf() {
        for (role, path) in [
            (CourseRole::Student, STUDENT_CATEGORY_PATH),
            (CourseRole::Teacher, TEACHER_CATEGORY_PATH),
        ] {
            let server = FixtureServer::new(vec![Reply::json(
                r#"{"result":"success","object":{"rows":[{"kjflid":"private-category","bt":"章节&amp;资料"}]}}"#,
            )]);
            let learn = LearnClient::new(LearnClientConfig::new(server.base(), role).unwrap());
            let transport = CampusHttpTransport::new("fixture-learn-categories").unwrap();
            let csrf = SessionRegistry::new()
                .bind_csrf(ServiceId::Learn, CsrfToken::new("fixture-csrf").unwrap());
            let categories = read_file_categories(&learn, &transport, &csrf, "_csrf", "course-one")
                .await
                .unwrap();
            assert_eq!(categories.len(), 1);
            assert_eq!(categories[0].title, "章节&资料");
            assert!(!format!("{categories:?}").contains("private-category"));
            let request = &server.requests()[0];
            assert!(request.starts_with(&format!("GET {path}?")));
            assert!(request.contains("wlkcid=course-one"));
            assert!(request.contains("_csrf=fixture-csrf"));
        }
    }

    #[tokio::test]
    async fn backend_repair_learn_file_categories_reject_foreign_csrf_and_login_page() {
        let server = FixtureServer::new(vec![Reply::html(
            r#"<form action="/do/off/ui/auth/login/check"><input name="i_user"></form>"#,
        )]);
        let learn =
            LearnClient::new(LearnClientConfig::new(server.base(), CourseRole::Student).unwrap());
        let transport = CampusHttpTransport::new("fixture-learn-categories-auth").unwrap();
        let registry = SessionRegistry::new();
        let foreign = registry.bind_csrf(ServiceId::Info, CsrfToken::new("fixture-csrf").unwrap());
        assert!(matches!(
            read_file_categories(&learn, &transport, &foreign, "_csrf", "course-one").await,
            Err(LearnFileError::Route)
        ));
        assert!(server.requests().is_empty());
        let csrf = registry.bind_csrf(ServiceId::Learn, CsrfToken::new("fixture-csrf").unwrap());
        assert!(matches!(
            read_file_categories(&learn, &transport, &csrf, "_csrf", "course-one").await,
            Err(LearnFileError::Session)
        ));
    }

    #[test]
    fn backend_repair_learn_file_categories_reject_invalid_or_incomplete_envelopes() {
        let course_id = "course-one";
        let categories = parse_file_categories(
            r#"{"result":"success","object":{"rows":[{"kjflid":"private-category","bt":"章节"}]}}"#,
            course_id,
        )
        .unwrap();
        let files = parse_file_list(
            r#"{"result":"success","object":[{"kjxxid":"one","wjid":"file","bt":"资料","kjflid":"private-category"}]}"#,
            course_id,
        )
        .unwrap();
        assert_eq!(
            files.files[0].category_selector.as_deref(),
            Some(categories[0].selector.as_str())
        );
        for body in [
            r#"{"result":"success","object":{}}"#,
            r#"{"result":"success","object":{"rows":null}}"#,
            r#"{"result":"success","object":{"rows":[{"kjflid":"x"}]}}"#,
            r#"{"result":"success","object":{"rows":[{"kjflid":"x","bt":"X"},{"kjflid":"x","bt":"duplicate"}]}}"#,
            r#"{"result":"error","object":{"rows":[]}}"#,
            "<html>login</html>",
        ] {
            assert!(parse_file_categories(body, course_id).is_err());
        }
        assert!(matches!(
            parse_file_categories(r#"{"result":"error","message":"请重新登录"}"#, course_id),
            Err(LearnFileError::Session)
        ));
    }

    #[test]
    fn backend_repair_learn_file_categories_reject_unproven_page_total() {
        let course_id = "course-one";
        assert!(
            parse_file_categories(
                r#"{"result":"success","object":{"total":2,"rows":[{"kjflid":"x","bt":"X"}]}}"#,
                course_id,
            )
            .is_err()
        );
        assert!(parse_file_categories(
            r#"{"result":"success","object":{"recordsTotal":"1","rows":[{"kjflid":"x","bt":"X"}]}}"#,
            course_id,
        )
        .is_err());
        let rows = (0..LIMIT)
            .map(|index| serde_json::json!({"kjflid":format!("id-{index}"),"bt":"资料"}))
            .collect::<Vec<_>>();
        let body = serde_json::json!({"result":"success","object":{"rows":rows}}).to_string();
        assert!(parse_file_categories(&body, course_id).is_err());
    }

    #[test]
    fn backend_repair_learn_file_download_name_is_safe_and_raw_id_stays_private() {
        assert_eq!(
            suggested_filename("../讲义/第一章", Some("PDF")),
            "_讲义_第一章.PDF"
        );
        assert_eq!(suggested_filename("讲义.pdf", Some("pdf")), "讲义.pdf");
        assert_eq!(suggested_filename("...", Some("exe/unsafe")), "课程资料");
        let list = parse_file_list(
            r#"{"result":"success","object":[{"kjxxid":"row","wjid":"secret-upstream-id","bt":"讲义"}]}"#,
            "course-one",
        )
        .unwrap();
        assert_eq!(list.files[0].raw_file_id, "secret-upstream-id");
        assert!(!format!("{:?}", list.files[0]).contains("secret-upstream-id"));
    }
}
