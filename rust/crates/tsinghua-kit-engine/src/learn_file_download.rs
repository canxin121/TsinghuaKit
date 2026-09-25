//! Bounded, read-only Learn file downloads. The URL and upstream file ID are
//! never returned to Flutter or accepted from it.

use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

use reqwest::{
    StatusCode,
    header::{CONTENT_TYPE, LOCATION},
};
use thiserror::Error;
use uuid::Uuid;

use crate::{
    learn_client::LearnClient,
    protocol::{CourseRole, ServiceId},
    session::BoundCsrfToken,
    telemetry::timing::{BoundedStreamError, stream_bounded_to_writer},
    transport::CampusHttpTransport,
};

const STUDENT_PATH: &str = "/b/wlxt/kj/wlkc_kjxxb/student/downloadFile";
const TEACHER_PATH: &str = "/b/wlxt/kj/wlkc_kjxxb/teacher/downloadFile";
const MAX_BYTES: usize = 512 * 1024 * 1024;

#[derive(Debug, Error)]
pub(crate) enum LearnFileDownloadError {
    #[error("Learn file session expired")]
    Session,
    #[error("Learn file download target was not allowed")]
    Route,
    #[error("Learn file download network failed")]
    Network,
    #[error("Learn file download HTTP status {0}")]
    Http(u16),
    #[error("Learn file exceeded the bounded download limit")]
    TooLarge,
    #[error("Learn file download returned an unverified body")]
    Content,
    #[error("Learn file target already exists")]
    Exists,
    #[error("Learn file destination could not be written")]
    Storage,
}

pub(crate) async fn probe_download(
    learn: &LearnClient,
    transport: &CampusHttpTransport,
    csrf: &BoundCsrfToken,
    raw_file_id: &str,
) -> Result<usize, LearnFileDownloadError> {
    fetch_to_writer(learn, transport, csrf, raw_file_id, &mut io::sink()).await
}

pub(crate) async fn save_download(
    learn: &LearnClient,
    transport: &CampusHttpTransport,
    csrf: &BoundCsrfToken,
    raw_file_id: &str,
    output_path: &str,
) -> Result<usize, LearnFileDownloadError> {
    let destination = checked_destination(output_path)?;
    if destination.symlink_metadata().is_ok() {
        return Err(LearnFileDownloadError::Exists);
    }
    let temporary = destination
        .parent()
        .ok_or(LearnFileDownloadError::Storage)?
        .join(format!(".thyou-download-{}.part", Uuid::new_v4()));
    let guard = TemporaryFile {
        path: temporary.clone(),
    };
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temporary)
        .map_err(|_| LearnFileDownloadError::Storage)?;
    let bytes = fetch_to_writer(learn, transport, csrf, raw_file_id, &mut file).await?;
    file.sync_all()
        .map_err(|_| LearnFileDownloadError::Storage)?;
    drop(file);
    fs::hard_link(&temporary, &destination).map_err(|error| {
        if error.kind() == io::ErrorKind::AlreadyExists {
            LearnFileDownloadError::Exists
        } else {
            LearnFileDownloadError::Storage
        }
    })?;
    drop(guard);
    Ok(bytes)
}

async fn fetch_to_writer<W: Write>(
    learn: &LearnClient,
    transport: &CampusHttpTransport,
    csrf: &BoundCsrfToken,
    raw_file_id: &str,
    writer: &mut W,
) -> Result<usize, LearnFileDownloadError> {
    if csrf.service() != ServiceId::Learn
        || raw_file_id.is_empty()
        || raw_file_id.len() > 256
        || raw_file_id.chars().any(char::is_control)
    {
        return Err(LearnFileDownloadError::Route);
    }
    let path = match learn.config().role() {
        CourseRole::Student => STUDENT_PATH,
        CourseRole::Teacher => TEACHER_PATH,
    };
    let mut url = learn
        .resolve_endpoint(path)
        .map_err(|_| LearnFileDownloadError::Route)?;
    url.query_pairs_mut()
        .append_pair("sfgk", "0")
        .append_pair("wjid", raw_file_id)
        .append_pair("_csrf", csrf.as_csrf_token().as_str());
    let request = transport
        .client()
        .get(url.clone())
        .build()
        .map_err(|_| LearnFileDownloadError::Route)?;
    let response = transport
        .execute_once(transport.client(), request)
        .await
        .map_err(|_| LearnFileDownloadError::Network)?;
    let status = response.status();
    if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
        return Err(LearnFileDownloadError::Session);
    }
    if response.url() != &url
        || response.headers().contains_key(LOCATION)
        || status.is_redirection()
    {
        return Err(LearnFileDownloadError::Route);
    }
    if status != StatusCode::OK {
        return Err(LearnFileDownloadError::Http(status.as_u16()));
    }
    if response
        .content_length()
        .is_some_and(|size| size > MAX_BYTES as u64)
    {
        return Err(LearnFileDownloadError::TooLarge);
    }
    if response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            let mime = value.to_ascii_lowercase();
            mime.starts_with("text/html") || mime.starts_with("application/json")
        })
    {
        return Err(LearnFileDownloadError::Session);
    }
    let receipt = stream_bounded_to_writer(response, writer, MAX_BYTES)
        .await
        .map_err(|error| match error {
            BoundedStreamError::Request(_) => LearnFileDownloadError::Network,
            BoundedStreamError::TooLarge => LearnFileDownloadError::TooLarge,
            BoundedStreamError::Io(_) => LearnFileDownloadError::Storage,
        })?;
    let prefix = receipt
        .prefix
        .strip_prefix(&[0xef, 0xbb, 0xbf])
        .unwrap_or(&receipt.prefix);
    let prefix = prefix
        .iter()
        .copied()
        .skip_while(u8::is_ascii_whitespace)
        .collect::<Vec<_>>();
    if receipt.bytes_written == 0
        || prefix.starts_with(b"<")
        || prefix.starts_with(b"{")
        || prefix.starts_with(b"[")
    {
        return Err(LearnFileDownloadError::Content);
    }
    Ok(receipt.bytes_written)
}

pub(crate) fn checked_destination(output_path: &str) -> Result<PathBuf, LearnFileDownloadError> {
    if output_path.is_empty() || output_path.chars().any(char::is_control) {
        return Err(LearnFileDownloadError::Route);
    }
    let path = Path::new(output_path);
    if !path.is_absolute() {
        return Err(LearnFileDownloadError::Route);
    }
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(LearnFileDownloadError::Route)?;
    if name.is_empty() || name == "." || name == ".." || name.len() > 255 {
        return Err(LearnFileDownloadError::Route);
    }
    let parent = path.parent().ok_or(LearnFileDownloadError::Route)?;
    let canonical = parent
        .canonicalize()
        .map_err(|_| LearnFileDownloadError::Storage)?;
    if !canonical.is_dir() {
        return Err(LearnFileDownloadError::Storage);
    }
    Ok(canonical.join(name))
}

struct TemporaryFile {
    path: PathBuf,
}

impl Drop for TemporaryFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
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
    async fn backend_repair_learn_file_download_uses_fixed_route_and_bound_csrf() {
        let server = FixtureServer::new(vec![Reply {
            status: 200,
            headers: "Content-Type: application/pdf\r\n".into(),
            body: "%PDF-1.7\nfixture-pdf".into(),
        }]);
        let learn =
            LearnClient::new(LearnClientConfig::new(server.base(), CourseRole::Student).unwrap());
        let transport = CampusHttpTransport::new("fixture-learn-file-download").unwrap();
        let csrf = SessionRegistry::new()
            .bind_csrf(ServiceId::Learn, CsrfToken::new("fixture-csrf").unwrap());
        let bytes = probe_download(&learn, &transport, &csrf, "private-file-id")
            .await
            .unwrap();
        assert_eq!(bytes, "%PDF-1.7\nfixture-pdf".len());
        let requests = server.requests();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].starts_with(&format!("GET {STUDENT_PATH}?")));
        assert!(requests[0].contains("sfgk=0"));
        assert!(requests[0].contains("wjid=private-file-id"));
        assert!(requests[0].contains("_csrf=fixture-csrf"));
    }

    #[tokio::test]
    async fn backend_repair_learn_file_download_rejects_login_and_foreign_csrf() {
        let server = FixtureServer::new(vec![Reply::html("<html>login</html>")]);
        let learn =
            LearnClient::new(LearnClientConfig::new(server.base(), CourseRole::Teacher).unwrap());
        let transport = CampusHttpTransport::new("fixture-learn-file-login").unwrap();
        let registry = SessionRegistry::new();
        let foreign = registry.bind_csrf(ServiceId::Info, CsrfToken::new("fixture-csrf").unwrap());
        assert!(matches!(
            probe_download(&learn, &transport, &foreign, "private-file-id").await,
            Err(LearnFileDownloadError::Route)
        ));
        assert!(server.requests().is_empty());
        let csrf = registry.bind_csrf(ServiceId::Learn, CsrfToken::new("fixture-csrf").unwrap());
        assert!(matches!(
            probe_download(&learn, &transport, &csrf, "private-file-id").await,
            Err(LearnFileDownloadError::Session)
        ));
    }

    #[tokio::test]
    async fn backend_repair_learn_file_download_saves_atomically_without_overwriting() {
        let server = FixtureServer::new(vec![Reply {
            status: 200,
            headers: "Content-Type: application/pdf\r\n".into(),
            body: "%PDF-1.7\nfixture".into(),
        }]);
        let learn =
            LearnClient::new(LearnClientConfig::new(server.base(), CourseRole::Student).unwrap());
        let transport = CampusHttpTransport::new("fixture-learn-file-save").unwrap();
        let csrf = SessionRegistry::new()
            .bind_csrf(ServiceId::Learn, CsrfToken::new("fixture-csrf").unwrap());
        let root = std::env::temp_dir().join(format!("thyou-file-fixture-{}", Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let destination = root.join("lecture.pdf");
        let path = destination.to_string_lossy().into_owned();
        let bytes = save_download(&learn, &transport, &csrf, "private-file-id", &path)
            .await
            .unwrap();
        assert_eq!(bytes, "%PDF-1.7\nfixture".len());
        assert_eq!(fs::read(&destination).unwrap(), b"%PDF-1.7\nfixture");
        assert!(matches!(
            save_download(&learn, &transport, &csrf, "private-file-id", &path).await,
            Err(LearnFileDownloadError::Exists)
        ));
        assert_eq!(server.requests().len(), 1);
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn backend_repair_learn_file_download_rejects_redirect_and_discards_html_payload() {
        let server = FixtureServer::new(vec![
            Reply {
                status: 302,
                headers: "Location: /login\r\n".into(),
                body: String::new(),
            },
            Reply {
                status: 200,
                headers: "Content-Type: application/octet-stream\r\n".into(),
                body: "\u{feff}<html>login</html>".into(),
            },
        ]);
        let learn =
            LearnClient::new(LearnClientConfig::new(server.base(), CourseRole::Student).unwrap());
        let transport = CampusHttpTransport::new("fixture-learn-file-reject").unwrap();
        let csrf = SessionRegistry::new()
            .bind_csrf(ServiceId::Learn, CsrfToken::new("fixture-csrf").unwrap());
        assert!(matches!(
            probe_download(&learn, &transport, &csrf, "private-file-id").await,
            Err(LearnFileDownloadError::Route)
        ));
        let root = std::env::temp_dir().join(format!("thyou-file-reject-{}", Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let destination = root.join("lecture.pdf");
        assert!(matches!(
            save_download(
                &learn,
                &transport,
                &csrf,
                "private-file-id",
                &destination.to_string_lossy(),
            )
            .await,
            Err(LearnFileDownloadError::Content)
        ));
        assert_eq!(server.requests().len(), 2);
        assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
        fs::remove_dir_all(root).unwrap();
    }
}
