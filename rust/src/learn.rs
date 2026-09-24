//! Role-aware request profiles for the learning platform.
//!
//! The learning platform exposes different course routes for students and teachers.
//! Keeping that choice in a typed profile prevents UI code and higher-level services
//! from assembling one route that happens to work for only one role.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::protocol::CourseRole;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum LearnError {
    #[error("semester must not be empty")]
    EmptySemester,

    #[error("semester must use the canonical YYYY-YYYY-1/2/3 identifier")]
    InvalidSemesterId,

    #[error("course index is required for teacher requests")]
    MissingTeacherCourseIndex,

    #[error("path segment contains a reserved separator")]
    InvalidPathSegment,

    #[error("learning route must be an absolute path without query or fragment")]
    InvalidRoutePath,

    #[error("roaming ticket must not be empty")]
    EmptyTicket,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LearnCourseRequest {
    pub path: String,
    pub semester: String,
    pub language: Option<String>,
    pub course_index: Option<String>,
    pub requires_csrf: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LearnRoamingRequest {
    pub path: String,
    pub ticket: String,
}

/// Ticket based roaming is the legacy Learn endpoint used after the identity
/// service returns a service ticket.  The current identity success page can
/// instead hand off through the cookie based `/f/` route; those are separate
/// profiles because putting a ticket on the latter route is not equivalent to
/// the legacy exchange.
pub const DEFAULT_ROAMING_ENTRY_PATH: &str = "/b/j_spring_security_thauth_roaming_entry";
pub const DEFAULT_COOKIE_ROAMING_ENTRY_PATH: &str = "/f/j_spring_security_thauth_roaming_entry";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LearnProfile {
    pub role: CourseRole,
}

impl LearnProfile {
    pub const fn new(role: CourseRole) -> Self {
        Self { role }
    }

    pub fn course_request(
        self,
        semester: &str,
        language: Option<&str>,
        course_index: Option<&str>,
    ) -> Result<LearnCourseRequest, LearnError> {
        validate_semester_id(semester)?;
        if let Some(language) = language {
            validate_segment(language, LearnError::InvalidPathSegment)?;
        }

        let path = match self.role {
            CourseRole::Student => {
                let language = language.unwrap_or("zh");
                format!(
                    "/b/wlxt/kc/v_wlkc_xs_xkb_kcb_extend/student/loadCourseBySemesterId/{semester}/{language}"
                )
            }
            CourseRole::Teacher => {
                let course_index = course_index.ok_or(LearnError::MissingTeacherCourseIndex)?;
                validate_segment(course_index, LearnError::InvalidPathSegment)?;
                format!("/b/kc/v_wlkc_kcb/queryAsorCoCourseList/{semester}/{course_index}")
            }
        };

        Ok(LearnCourseRequest {
            path,
            semester: semester.to_owned(),
            language: language.map(str::to_owned),
            course_index: course_index.map(str::to_owned),
            requires_csrf: true,
        })
    }

    /// Builds every course-list request used by the role profile.
    ///
    /// The student endpoint has one logical list and does not use a
    /// co-course index.  The teacher endpoint has two independently returned
    /// lists in the current public client: index `0` is the assistant-course
    /// view and index `1` is the co-taught-course view.  A caller may still
    /// provide one explicit teacher index when it is intentionally reading
    /// only one of those views.
    pub fn course_requests(
        self,
        semester: &str,
        language: Option<&str>,
        course_index: Option<&str>,
    ) -> Result<Vec<LearnCourseRequest>, LearnError> {
        match self.role {
            CourseRole::Student => Ok(vec![self.course_request(semester, language, None)?]),
            CourseRole::Teacher => {
                let indices = match course_index {
                    Some(index) => vec![index.to_owned()],
                    None => vec!["0".to_owned(), "1".to_owned()],
                };
                indices
                    .iter()
                    .map(|index| self.course_request(semester, language, Some(index)))
                    .collect()
            }
        }
    }

    pub fn roaming_entry_request(ticket: &str) -> Result<LearnRoamingRequest, LearnError> {
        Self::roaming_entry_request_at(DEFAULT_ROAMING_ENTRY_PATH, ticket)
    }

    pub fn roaming_entry_request_at(
        path: &str,
        ticket: &str,
    ) -> Result<LearnRoamingRequest, LearnError> {
        validate_route_path(path)?;
        if ticket.trim().is_empty() {
            return Err(LearnError::EmptyTicket);
        }

        Ok(LearnRoamingRequest {
            path: path.to_owned(),
            ticket: ticket.to_owned(),
        })
    }
}

/// Validates the identifier used by the current Learn course and semester
/// endpoints. Public implementations use the academic-year form
/// `YYYY-YYYY-1`, `YYYY-YYYY-2`, or `YYYY-YYYY-3`; the final digit represents
/// the autumn, spring, or summer term respectively.
pub fn validate_semester_id(value: &str) -> Result<(), LearnError> {
    if value.trim().is_empty() {
        return Err(LearnError::EmptySemester);
    }

    let bytes = value.as_bytes();
    let valid_shape = bytes.len() == 11
        && bytes[4] == b'-'
        && bytes[9] == b'-'
        && bytes[..4].iter().all(u8::is_ascii_digit)
        && bytes[5..9].iter().all(u8::is_ascii_digit)
        && matches!(bytes[10], b'1' | b'2' | b'3');
    if !valid_shape {
        return Err(LearnError::InvalidSemesterId);
    }

    let start_year = value[..4]
        .parse::<u16>()
        .map_err(|_| LearnError::InvalidSemesterId)?;
    let end_year = value[5..9]
        .parse::<u16>()
        .map_err(|_| LearnError::InvalidSemesterId)?;
    if end_year != start_year.saturating_add(1) {
        return Err(LearnError::InvalidSemesterId);
    }

    Ok(())
}

pub(crate) fn validate_route_path(path: &str) -> Result<(), LearnError> {
    if path.trim().is_empty()
        || !path.starts_with('/')
        || path.starts_with("//")
        || path.contains(['?', '#'])
        || path.contains('\\')
        || path
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
        || path.split('/').any(|segment| matches!(segment, "." | ".."))
        || invalid_percent_encoding(path)
        || path_contains_encoded_escape(path)
    {
        return Err(LearnError::InvalidRoutePath);
    }
    Ok(())
}

pub(crate) fn validate_segment(value: &str, empty_error: LearnError) -> Result<(), LearnError> {
    if value.trim().is_empty() {
        return Err(empty_error);
    }
    if matches!(value, "." | "..")
        || value.contains(['/', '\\', '?', '#'])
        || value
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
        || invalid_percent_encoding(value)
        || path_contains_encoded_escape(value)
    {
        return Err(LearnError::InvalidPathSegment);
    }
    Ok(())
}

fn invalid_percent_encoding(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.iter().enumerate().any(|(index, byte)| {
        *byte == b'%'
            && (index + 2 >= bytes.len()
                || !bytes[index + 1].is_ascii_hexdigit()
                || !bytes[index + 2].is_ascii_hexdigit())
    })
}

fn path_contains_encoded_escape(path: &str) -> bool {
    // Keep ordinary percent-encoded data valid, but reject direct and nested
    // encodings of separators, dots, or controls. `%252e%252e` therefore
    // fails after the second decoding pass.
    let mut candidate = path.to_owned();
    loop {
        if !candidate.as_bytes().contains(&b'%') {
            return false;
        }
        if invalid_percent_encoding(&candidate) {
            return false;
        }
        let bytes = candidate.as_bytes();
        if bytes.windows(3).any(|window| {
            window[0] == b'%'
                && hex_value(window[1])
                    .zip(hex_value(window[2]))
                    .is_some_and(|(high, low)| {
                        matches!((high << 4) | low, b'.' | b'/' | b'\\' | 0x00..=0x1f | 0x7f)
                    })
        }) {
            return true;
        }
        let Ok(decoded) = percent_decode(&candidate) else {
            return true;
        };
        if decoded == candidate {
            return false;
        }
        candidate = decoded;
    }
}

fn percent_decode(value: &str) -> Result<String, ()> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'%' {
            decoded.push(bytes[index]);
            index += 1;
            continue;
        }
        if index + 2 >= bytes.len() {
            return Err(());
        }
        let Some(high) = hex_value(bytes[index + 1]) else {
            return Err(());
        };
        let Some(low) = hex_value(bytes[index + 2]) else {
            return Err(());
        };
        decoded.push((high << 4) | low);
        index += 3;
    }
    String::from_utf8(decoded).map_err(|_| ())
}

fn hex_value(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn student_and_teacher_course_routes_are_kept_separate() {
        let student = LearnProfile::new(CourseRole::Student)
            .course_request("2025-2026-2", Some("en"), None)
            .expect("student request builds");
        assert_eq!(
            student.path,
            "/b/wlxt/kc/v_wlkc_xs_xkb_kcb_extend/student/loadCourseBySemesterId/2025-2026-2/en"
        );

        let teacher = LearnProfile::new(CourseRole::Teacher)
            .course_request("2025-2026-2", None, Some("7"))
            .expect("teacher request builds");
        assert_eq!(
            teacher.path,
            "/b/kc/v_wlkc_kcb/queryAsorCoCourseList/2025-2026-2/7"
        );
    }

    #[test]
    fn validates_role_specific_arguments_and_ticket() {
        let teacher = LearnProfile::new(CourseRole::Teacher);
        assert!(matches!(
            teacher.course_request("2025-2026-2", None, None),
            Err(LearnError::MissingTeacherCourseIndex)
        ));
        assert!(matches!(
            LearnProfile::new(CourseRole::Student).course_request(
                "2025-2026-2",
                Some("en/us"),
                None,
            ),
            Err(LearnError::InvalidPathSegment)
        ));
        assert!(matches!(
            LearnProfile::new(CourseRole::Student).course_request("2026-fall", None, None),
            Err(LearnError::InvalidSemesterId)
        ));
        assert!(matches!(
            LearnProfile::roaming_entry_request("  "),
            Err(LearnError::EmptyTicket)
        ));

        for value in [
            "en/us",
            "en\\us",
            "en%ZZ",
            "en%2fus",
            "en%5cus",
            "en%2eus",
            "en%252eus",
            ".",
            "..",
        ] {
            assert!(
                matches!(
                    LearnProfile::new(CourseRole::Student).course_request(
                        "2025-2026-2",
                        Some(value),
                        None,
                    ),
                    Err(LearnError::InvalidPathSegment)
                ),
                "unsafe language segment was accepted: {value}"
            );
        }
    }

    #[test]
    fn teacher_course_profile_includes_both_public_co_course_indexes() {
        let requests = LearnProfile::new(CourseRole::Teacher)
            .course_requests("2025-2026-2", Some("zh"), None)
            .expect("teacher requests build");
        assert_eq!(requests.len(), 2);
        assert!(requests[0].path.ends_with("/2025-2026-2/0"));
        assert!(requests[1].path.ends_with("/2025-2026-2/1"));

        let selected = LearnProfile::new(CourseRole::Teacher)
            .course_requests("2025-2026-2", Some("zh"), Some("1"))
            .expect("selected teacher request builds");
        assert_eq!(selected.len(), 1);
        assert!(selected[0].path.ends_with("/2025-2026-2/1"));
    }

    #[test]
    fn accepts_only_academic_year_semester_identifiers() {
        for value in ["2025-2026-1", "2025-2026-2", "2025-2026-3"] {
            validate_semester_id(value).expect("canonical semester id");
        }
        for value in [
            "2025-2025-1",
            "2025-2027-1",
            "2025-2026-0",
            "2025-2026-4",
            "2025-2026-01",
            "2026-fall",
        ] {
            assert!(matches!(
                validate_semester_id(value),
                Err(LearnError::InvalidSemesterId)
            ));
        }
    }

    #[test]
    fn rejects_unsafe_configured_routes() {
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
            assert!(
                matches!(validate_route_path(path), Err(LearnError::InvalidRoutePath)),
                "unsafe route was accepted: {path}"
            );
        }
    }
}
