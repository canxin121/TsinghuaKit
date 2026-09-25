//! Fixed, read-only access to the university calendar published by the
//! campus-app service. Image bytes never cause Flutter to issue HTTP itself.

use reqwest::{StatusCode, Url, header::CONTENT_TYPE};

use crate::{
    captcha_image::jpeg_dimensions,
    telemetry::timing::{BoundedBodyError, read_bounded_bytes},
    transport::CampusHttpTransport,
};

pub const SCHOOL_CALENDAR_ORIGIN: &str = "https://app.cs.tsinghua.edu.cn/";
pub const SCHOOL_CALENDAR_YEAR_PATH: &str = "/Api/SchoolCalendarYear";
const MAX_YEAR_BODY: usize = 4096;
const MAX_IMAGE_BODY: usize = 12 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchoolCalendarImage {
    pub latest_year: u32,
    pub year: u32,
    pub semester: String,
    pub language: String,
    pub image_bytes: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchoolCalendarError {
    Config,
    Selection,
    Route,
    Http,
    Unavailable,
    Network,
    Limit,
    Format,
}

impl SchoolCalendarError {
    pub const fn diagnostic_code(self) -> &'static str {
        match self {
            Self::Config => "school_calendar_config",
            Self::Selection => "school_calendar_selection",
            Self::Route => "school_calendar_route",
            Self::Http => "school_calendar_http",
            Self::Unavailable => "school_calendar_unavailable",
            Self::Network => "school_calendar_network",
            Self::Limit => "school_calendar_limit",
            Self::Format => "school_calendar_format",
        }
    }
}

pub struct SchoolCalendarReader {
    origin: Url,
    transport: CampusHttpTransport,
}

impl SchoolCalendarReader {
    pub fn for_app_service(transport: CampusHttpTransport) -> Result<Self, SchoolCalendarError> {
        let origin = Url::parse(SCHOOL_CALENDAR_ORIGIN).map_err(|_| SchoolCalendarError::Config)?;
        Ok(Self { origin, transport })
    }

    #[cfg(test)]
    fn for_fixture(origin: Url, transport: CampusHttpTransport) -> Self {
        Self { origin, transport }
    }

    /// A requested year is constrained by the server's latest published year.
    /// `None` opens the latest year without trusting the client's clock.
    pub async fn read(
        &self,
        requested_year: Option<u32>,
        semester: &str,
        language: &str,
    ) -> Result<SchoolCalendarImage, SchoolCalendarError> {
        let semester_number = match semester {
            "autumn" => 1,
            "spring" => 2,
            _ => return Err(SchoolCalendarError::Selection),
        };
        if !matches!(language, "zh" | "en") {
            return Err(SchoolCalendarError::Selection);
        }
        if requested_year.is_some_and(|year| !(2000..=2100).contains(&year)) {
            return Err(SchoolCalendarError::Selection);
        }

        let year_bytes = self
            .fetch(SCHOOL_CALENDAR_YEAR_PATH, MAX_YEAR_BODY, false)
            .await?;
        let latest_year = parse_latest_year(&year_bytes)?;
        let year = requested_year.unwrap_or(latest_year);
        if year > latest_year {
            return Err(SchoolCalendarError::Selection);
        }
        let path = format!("/xiaoli/{language}/{year}-{semester_number}.jpg");
        let image_bytes = self.fetch(&path, MAX_IMAGE_BODY, true).await?;
        let (width, height) = jpeg_dimensions(&image_bytes).ok_or(SchoolCalendarError::Format)?;
        if !image_bytes.ends_with(&[0xff, 0xd9])
            || width == 0
            || height == 0
            || width > 12000
            || height > 12000
            || u64::from(width) * u64::from(height) > 80_000_000
        {
            return Err(SchoolCalendarError::Format);
        }
        Ok(SchoolCalendarImage {
            latest_year,
            year,
            semester: semester.to_owned(),
            language: language.to_owned(),
            image_bytes,
        })
    }

    async fn fetch(
        &self,
        path: &str,
        limit: usize,
        image: bool,
    ) -> Result<Vec<u8>, SchoolCalendarError> {
        let target = self
            .origin
            .join(path)
            .map_err(|_| SchoolCalendarError::Config)?;
        if target.scheme() != self.origin.scheme()
            || target.host_str() != self.origin.host_str()
            || target.port_or_known_default() != self.origin.port_or_known_default()
            || target.path() != path
            || target.query().is_some()
            || target.fragment().is_some()
        {
            return Err(SchoolCalendarError::Route);
        }
        // The shared transport gate still owns the HTTP request, but this
        // fixed public route must expose a 302 for rejection rather than
        // following a same-origin login or moved-image path.
        let request = self
            .transport
            .client()
            .get(target.clone())
            .build()
            .map_err(|_| SchoolCalendarError::Config)?;
        let response = self
            .transport
            .execute_once(self.transport.client(), request)
            .await
            .map_err(|_| SchoolCalendarError::Network)?;
        if response.url() != &target
            || response.status().is_redirection()
            || response.headers().contains_key(reqwest::header::LOCATION)
        {
            return Err(SchoolCalendarError::Route);
        }
        if response.status() == StatusCode::NOT_FOUND {
            return Err(SchoolCalendarError::Unavailable);
        }
        if response.status() != StatusCode::OK {
            return Err(SchoolCalendarError::Http);
        }
        let mime = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .split(';')
            .next()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        let allowed = if image {
            mime == "image/jpeg"
        } else {
            matches!(mime.as_str(), "application/json" | "text/plain")
        };
        if !allowed
            || response
                .content_length()
                .is_some_and(|length| length > limit as u64)
        {
            return Err(if allowed {
                SchoolCalendarError::Limit
            } else {
                SchoolCalendarError::Format
            });
        }
        read_bounded_bytes(response, limit)
            .await
            .map_err(|error| match error {
                BoundedBodyError::Request(_) => SchoolCalendarError::Network,
                BoundedBodyError::TooLarge => SchoolCalendarError::Limit,
            })
    }
}

fn parse_latest_year(body: &[u8]) -> Result<u32, SchoolCalendarError> {
    let value: serde_json::Value =
        serde_json::from_slice(body).map_err(|_| SchoolCalendarError::Format)?;
    let year = value
        .as_object()
        .and_then(|object| object.get("year"))
        .and_then(serde_json::Value::as_u64)
        .and_then(|year| u32::try_from(year).ok())
        .ok_or(SchoolCalendarError::Format)?;
    if !(2000..=2100).contains(&year) {
        return Err(SchoolCalendarError::Format);
    }
    Ok(year)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
        time::{Duration, Instant},
    };

    fn serve(replies: Vec<(u16, &'static str, Vec<u8>)>) -> (Url, thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        listener.set_nonblocking(true).unwrap();
        let worker = thread::spawn(move || {
            let mut requests = Vec::new();
            let deadline = Instant::now() + Duration::from_secs(5);
            for (status, headers, body) in replies {
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(Instant::now() < deadline, "fixture request timed out");
                            thread::sleep(Duration::from_millis(2));
                        }
                        Err(error) => panic!("fixture accept: {error}"),
                    }
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut received = Vec::new();
                loop {
                    let mut buffer = [0_u8; 2048];
                    let count = stream.read(&mut buffer).unwrap();
                    if count == 0 {
                        break;
                    }
                    received.extend_from_slice(&buffer[..count]);
                    if received.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                        break;
                    }
                }
                requests.push(
                    String::from_utf8_lossy(&received)
                        .lines()
                        .next()
                        .unwrap_or_default()
                        .to_owned(),
                );
                write!(
                    stream,
                    "HTTP/1.1 {status} Fixture\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .unwrap();
                stream.write_all(&body).unwrap();
            }
            requests
        });
        (url, worker)
    }

    #[test]
    fn backend_repair_school_calendar_requires_a_real_bounded_year() {
        assert_eq!(parse_latest_year(br#"{"year":2026}"#), Ok(2026));
        for body in [
            br#"{"year":"2026"}"#.as_slice(),
            br#"{"year":1999}"#,
            br#"{"year":2026.5}"#,
            br#"{"message":"success"}"#,
            b"<html>login</html>",
        ] {
            assert_eq!(parse_latest_year(body), Err(SchoolCalendarError::Format));
        }
    }

    #[tokio::test]
    async fn backend_repair_school_calendar_rejects_unapproved_choices_before_http() {
        let transport = CampusHttpTransport::new("THYou/calendar-fixture").unwrap();
        let reader = SchoolCalendarReader::for_fixture(
            Url::parse("http://127.0.0.1:1/").unwrap(),
            transport,
        );
        assert_eq!(
            reader.read(None, "summer", "zh").await,
            Err(SchoolCalendarError::Selection)
        );
        assert_eq!(
            reader.read(Some(2026), "spring", "other").await,
            Err(SchoolCalendarError::Selection)
        );
    }

    #[tokio::test]
    async fn backend_repair_school_calendar_reads_fixed_year_and_jpeg_paths_without_redirects() {
        let jpeg = include_bytes!("fixtures/school_calendar.jpg").to_vec();
        let (origin, worker) = serve(vec![
            (
                200,
                "Content-Type: application/json\r\n",
                br#"{"year":2026}"#.to_vec(),
            ),
            (200, "Content-Type: image/jpeg\r\n", jpeg.clone()),
        ]);
        let transport = CampusHttpTransport::new("THYou/calendar-fixture").unwrap();
        let reader = SchoolCalendarReader::for_fixture(origin, transport);
        let image = reader.read(None, "autumn", "zh").await.unwrap();
        assert_eq!(image.latest_year, 2026);
        assert_eq!(image.year, 2026);
        assert_eq!(image.image_bytes, jpeg);
        assert_eq!(
            worker.join().unwrap(),
            [
                "GET /Api/SchoolCalendarYear HTTP/1.1",
                "GET /xiaoli/zh/2026-1.jpg HTTP/1.1",
            ]
        );

        let (origin, worker) = serve(vec![(
            302,
            "Location: /login\r\nContent-Type: text/html\r\n",
            Vec::new(),
        )]);
        let transport = CampusHttpTransport::new("THYou/calendar-fixture").unwrap();
        let reader = SchoolCalendarReader::for_fixture(origin, transport);
        assert_eq!(
            reader.read(None, "spring", "en").await,
            Err(SchoolCalendarError::Route)
        );
        assert_eq!(worker.join().unwrap().len(), 1);
    }
}
