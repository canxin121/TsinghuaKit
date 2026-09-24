//! Read-only student homework detail. Both fixed Learn routes must succeed;
//! submission, grading, download and preview endpoints are never called here.

use reqwest::{StatusCode, header::LOCATION};
use scraper::{Html, Selector};
use serde_json::Value;
use thiserror::Error;

use crate::{
    learn_client::{
        LearnClient, LearnClientError, LearnPageClassification, is_login_failure_message,
    },
    protocol::{CourseRole, ServiceId},
    session::BoundCsrfToken,
    transport::CampusHttpTransport,
};

const VIEW_PATH: &str = "/f/wlxt/kczy/zy/student/viewCj";
const DETAIL_PATH: &str = "/b/wlxt/kczy/zy/student/detail";
const MAX_TEXT: usize = 64 * 1024;
const MAX_BODY: usize = 2 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct HomeworkAttachment {
    pub kind: &'static str,
    pub name: String,
    pub size: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct HomeworkDetail {
    pub description: Option<String>,
    pub answer_content: Option<String>,
    pub submitted_content: Option<String>,
    pub attachments: Vec<HomeworkAttachment>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub(crate) enum HomeworkDetailError {
    #[error("Learn homework session expired")]
    Session,
    #[error("Learn homework route or selection rejected")]
    Route,
    #[error("Learn homework network request failed")]
    Network,
    #[error("Learn homework HTTP status {0}")]
    Http(u16),
    #[error("Learn homework response was not a verified detail")]
    Response,
}

pub(crate) async fn read_homework_detail(
    learn: &LearnClient,
    transport: &CampusHttpTransport,
    csrf: &BoundCsrfToken,
    csrf_parameter: &str,
    course_id: &str,
    student_id: &str,
    base_id: &str,
) -> Result<HomeworkDetail, HomeworkDetailError> {
    if learn.config().role() != CourseRole::Student
        || csrf.service() != ServiceId::Learn
        || csrf_parameter != "_csrf"
        || [course_id, student_id, base_id]
            .iter()
            .any(|value| invalid_id(value))
    {
        return Err(HomeworkDetailError::Route);
    }
    let mut view = learn
        .resolve_endpoint(VIEW_PATH)
        .map_err(|_| HomeworkDetailError::Route)?;
    view.query_pairs_mut()
        .append_pair("wlkcid", course_id)
        .append_pair("xszyid", student_id)
        .append_pair(csrf_parameter, csrf.as_csrf_token().as_str());
    let view_request = transport
        .client()
        .get(view)
        .build()
        .map_err(|_| HomeworkDetailError::Route)?;
    let view_html = fetch_exact(learn, transport, view_request).await?;
    let mut parsed = parse_homework_view(&view_html)?;

    let mut detail = learn
        .resolve_endpoint(DETAIL_PATH)
        .map_err(|_| HomeworkDetailError::Route)?;
    detail
        .query_pairs_mut()
        .append_pair(csrf_parameter, csrf.as_csrf_token().as_str());
    let detail_request = transport
        .client()
        .post(detail)
        .form(&[("id", base_id)])
        .build()
        .map_err(|_| HomeworkDetailError::Route)?;
    let detail_json = fetch_exact(learn, transport, detail_request).await?;
    // The JSON detail is the reference client's authoritative description.
    // An empty *present* msg is legitimate; a missing msg is protocol drift.
    parsed.description = parse_homework_description(&detail_json)?;
    Ok(parsed)
}

fn invalid_id(value: &str) -> bool {
    value.trim().is_empty() || value.len() > 256 || value.chars().any(char::is_control)
}

async fn fetch_exact(
    learn: &LearnClient,
    transport: &CampusHttpTransport,
    request: reqwest::Request,
) -> Result<String, HomeworkDetailError> {
    let expected = request.url().clone();
    let response = transport
        .execute_once(transport.client(), request)
        .await
        .map_err(|_| HomeworkDetailError::Network)?;
    let status = response.status();
    let final_url = response.url().clone();
    let redirect = response
        .headers()
        .get(LOCATION)
        .map(|value| value.to_str().map(str::to_owned))
        .transpose()
        .map_err(|_| HomeworkDetailError::Route)?;
    if response
        .content_length()
        .is_some_and(|size| size > MAX_BODY as u64)
    {
        return Err(HomeworkDetailError::Response);
    }
    let body = crate::telemetry::timing::read_text(response)
        .await
        .map_err(|_| HomeworkDetailError::Network)?;
    if body.len() > MAX_BODY {
        return Err(HomeworkDetailError::Response);
    }
    let mapped = learn
        .map_html_response_with_redirect(status, Some(&final_url), redirect.as_deref(), &body)
        .map_err(|error| match error {
            LearnClientError::SessionExpired => HomeworkDetailError::Session,
            _ => HomeworkDetailError::Route,
        })?;
    if matches!(
        mapped.classification,
        LearnPageClassification::LoginExpired(_)
    ) || matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN)
    {
        return Err(HomeworkDetailError::Session);
    }
    if final_url != expected || redirect.is_some() {
        return Err(HomeworkDetailError::Route);
    }
    if status != StatusCode::OK {
        return Err(HomeworkDetailError::Http(status.as_u16()));
    }
    Ok(body)
}

fn parse_homework_view(body: &str) -> Result<HomeworkDetail, HomeworkDetailError> {
    let document = Html::parse_document(body);
    let content = Selector::parse("div.list.calendar.clearfix > div.fl.right > div.c55")
        .map_err(|_| HomeworkDetailError::Response)?;
    let boxbox = Selector::parse("div.boxbox").map_err(|_| HomeworkDetailError::Response)?;
    let right = Selector::parse("div.right").map_err(|_| HomeworkDetailError::Response)?;
    let files =
        Selector::parse("div.list.fujian.clearfix").map_err(|_| HomeworkDetailError::Response)?;
    let link = Selector::parse("a").map_err(|_| HomeworkDetailError::Response)?;
    let size =
        Selector::parse(".fl > span[class^=color]").map_err(|_| HomeworkDetailError::Response)?;

    let blocks = document.select(&content).collect::<Vec<_>>();
    if blocks.is_empty() {
        return Err(HomeworkDetailError::Response);
    }
    let answer_content = blocks
        .get(1)
        .map(|block| clean_text(block.text()))
        .transpose()?
        .flatten();
    let submitted_content = document
        .select(&boxbox)
        .nth(1)
        .and_then(|box_node| box_node.select(&right).nth(2))
        .map(|node| clean_text(node.text()))
        .transpose()?
        .flatten();
    let mut attachments = Vec::new();
    for (index, section) in document.select(&files).enumerate() {
        let kind = match index {
            0 => "assignment",
            1 => "answer",
            2 => "submitted",
            3 => "grade",
            _ => return Err(HomeworkDetailError::Response),
        };
        if let Some(anchor) = section.select(&link).next() {
            let name = clean_text(anchor.text())?.ok_or(HomeworkDetailError::Response)?;
            let size = section
                .select(&size)
                .next()
                .map(|node| clean_text(node.text()))
                .transpose()?
                .flatten();
            attachments.push(HomeworkAttachment { kind, name, size });
        }
    }
    Ok(HomeworkDetail {
        description: None,
        answer_content,
        submitted_content,
        attachments,
    })
}

fn parse_homework_description(body: &str) -> Result<Option<String>, HomeworkDetailError> {
    let value: Value = serde_json::from_str(body).map_err(|_| HomeworkDetailError::Response)?;
    let root = value.as_object().ok_or(HomeworkDetailError::Response)?;
    if root.get("result").and_then(Value::as_str) != Some("success") {
        return Err(
            if root
                .get("msg")
                .and_then(Value::as_str)
                .is_some_and(is_login_failure_message)
            {
                HomeworkDetailError::Session
            } else {
                HomeworkDetailError::Response
            },
        );
    }
    let msg = root
        .get("msg")
        .and_then(Value::as_str)
        .ok_or(HomeworkDetailError::Response)?;
    if msg.len() > MAX_TEXT {
        return Err(HomeworkDetailError::Response);
    }
    let fragment = Html::parse_fragment(msg);
    clean_text(fragment.root_element().text())
}

fn clean_text<'a>(
    parts: impl Iterator<Item = &'a str>,
) -> Result<Option<String>, HomeworkDetailError> {
    let text = parts.collect::<Vec<_>>().join(" ");
    if text.len() > MAX_TEXT
        || text
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
    {
        return Err(HomeworkDetailError::Response);
    }
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    Ok((!normalized.is_empty()).then_some(normalized))
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
    async fn backend_repair_homework_detail_uses_only_fixed_read_routes() {
        let server = FixtureServer::new(vec![
            Reply::html(
                "<div class='list calendar clearfix'><div class='fl right'><div class='c55'>题目</div><div class='c55'>参考答案</div></div></div><div class='list fujian clearfix'><div class='ftitle'><a href='/file?fileId=private'>附件.txt</a></div><div class='fl'><span class='color'>2 MB</span></div></div>",
            ),
            Reply::json(r#"{"result":"success","msg":"<p>写一篇短文</p>"}"#),
        ]);
        let learn =
            LearnClient::new(LearnClientConfig::new(server.base(), CourseRole::Student).unwrap());
        let transport = CampusHttpTransport::new("fixture-homework").unwrap();
        let csrf = SessionRegistry::new()
            .bind_csrf(ServiceId::Learn, CsrfToken::new("fixture-csrf").unwrap());
        let read = read_homework_detail(
            &learn, &transport, &csrf, "_csrf", "course", "student", "base",
        )
        .await
        .unwrap();
        assert_eq!(read.description.as_deref(), Some("写一篇短文"));
        assert_eq!(read.answer_content.as_deref(), Some("参考答案"));
        assert_eq!(read.attachments[0].name, "附件.txt");
        assert!(!format!("{read:?}").contains("private"));
        let requests = server.requests();
        assert_eq!(requests.len(), 2);
        assert!(requests[0].starts_with("GET /f/wlxt/kczy/zy/student/viewCj?"));
        assert!(requests[0].contains("wlkcid=course&xszyid=student&_csrf=fixture-csrf"));
        assert!(requests[1].starts_with("POST /b/wlxt/kczy/zy/student/detail?_csrf=fixture-csrf"));
        assert!(requests[1].contains("id=base"));
    }

    #[test]
    fn backend_repair_homework_detail_rejects_unproven_or_broken_content() {
        assert_eq!(
            parse_homework_view("<html>login</html>"),
            Err(HomeworkDetailError::Response)
        );
        assert_eq!(
            parse_homework_description(r#"{"result":"success"}"#),
            Err(HomeworkDetailError::Response)
        );
        assert_eq!(
            parse_homework_description(r#"{"result":"failed","msg":"请重新登录"}"#),
            Err(HomeworkDetailError::Session)
        );
        assert_eq!(
            parse_homework_description("<html>login</html>"),
            Err(HomeworkDetailError::Response)
        );
    }

    #[tokio::test]
    async fn backend_repair_homework_detail_never_posts_after_login_page_or_foreign_csrf() {
        let server = FixtureServer::new(vec![Reply::html(
            r#"<form action="/do/off/ui/auth/login/check"><input name="i_user"></form>"#,
        )]);
        let learn =
            LearnClient::new(LearnClientConfig::new(server.base(), CourseRole::Student).unwrap());
        let transport = CampusHttpTransport::new("fixture-homework-login").unwrap();
        let registry = SessionRegistry::new();
        let foreign = registry.bind_csrf(ServiceId::Info, CsrfToken::new("foreign").unwrap());
        assert_eq!(
            read_homework_detail(&learn, &transport, &foreign, "_csrf", "c", "s", "b").await,
            Err(HomeworkDetailError::Route)
        );
        assert!(server.requests().is_empty());
        let csrf = registry.bind_csrf(ServiceId::Learn, CsrfToken::new("valid").unwrap());
        assert_eq!(
            read_homework_detail(&learn, &transport, &csrf, "_csrf", "c", "s", "b").await,
            Err(HomeworkDetailError::Session)
        );
        assert_eq!(server.requests().len(), 1);
    }
}
