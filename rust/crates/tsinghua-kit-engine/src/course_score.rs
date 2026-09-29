//! One course result looked up by course number.
//!
//! The service hall's own preset query answers with a single course's name,
//! credit, and grade.  It is implemented here rather than inside the task-list
//! client because its input is a caller-supplied course number: the number has
//! to be validated before any request, and the account's own student id — the
//! service's other query parameter — has to be derived from the proven
//! identity rather than accepted from a caller.
//!
//! This reimplements the contract from public reference behavior.  It does not
//! copy source, fixtures, or assets.

use serde_json::{Map, Value};

use crate::thos::ThosError;

/// A course number is a short ASCII identifier; this bound is what keeps
/// caller text from becoming an arbitrary value in a request body.
const MAX_COURSE_ID: usize = 32;

/// Everything that can stop one course-score lookup.
///
/// The transport variants are carried as the service hall's own error, so a
/// caller can still tell a closed session from a refused redirect; the local
/// variants say which argument was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CourseScoreError {
    /// The service hall request failed or was refused.
    Transport(ThosError),
    /// The caller's course number is not one this client will send.
    InvalidInput,
    /// The account's login id is not an all-digit student id.
    NotAStudentId,
    /// The answer was not the expected object.
    Response,
}

impl From<ThosError> for CourseScoreError {
    fn from(error: ThosError) -> Self {
        Self::Transport(error)
    }
}

/// One course result read from the service hall's preset query.
///
/// No account-identifying value is part of this type: the student id the query
/// needs exists only inside the request body the client builds.
#[derive(Clone, PartialEq)]
pub struct CourseScore {
    name: String,
    credit: Option<f64>,
    grade: String,
    /// True when the service reported nothing for this course at all.
    empty: bool,
}

/// A grade is personal academic data, so `Debug` prints only the presence and
/// shape of each field and never the values themselves.
impl std::fmt::Debug for CourseScore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CourseScore")
            .field("name_present", &!self.name.is_empty())
            .field("name_bytes", &self.name.len())
            .field("credit", &self.credit)
            .field("grade_present", &!self.grade.is_empty())
            .field("empty", &self.empty)
            .finish()
    }
}

impl CourseScore {
    /// Builds one result from the parsed fields.
    ///
    /// `empty` is derived here rather than accepted, so a caller can never
    /// label a result empty while carrying values, or the reverse.
    pub(crate) fn from_parts(name: String, credit: Option<f64>, grade: String) -> Self {
        let empty = name.is_empty() && grade.is_empty() && credit.is_none();
        Self {
            name,
            credit,
            grade,
            empty,
        }
    }

    /// Returns the course's own name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the credit value the service printed, when it printed one.
    pub fn credit(&self) -> Option<f64> {
        self.credit
    }

    /// Returns the grade exactly as the service rendered it.
    pub fn grade(&self) -> &str {
        &self.grade
    }

    /// Returns true when the service reported no result at all.
    ///
    /// The service answers a course number it holds no result for with a body
    /// whose three fields are all absent.  That is the service's own statement
    /// about this account and course, so a caller can distinguish it from a
    /// response that failed to parse.
    pub fn is_empty(&self) -> bool {
        self.empty
    }
}

/// The parse/validation failure of one course-score answer.
///
/// It carries no body text and no course number, so it can be logged as a
/// reason code without exposing either.
/// Validates one caller-supplied course number before it can enter a body.
///
/// A course number is a short ASCII identifier, so anything else is refused
/// here: caller text must not become an arbitrary value in a request.
pub(crate) fn course_id_for_request(course_id: &str) -> Result<&str, CourseScoreError> {
    if course_id.is_empty()
        || course_id.len() > MAX_COURSE_ID
        || !course_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err(CourseScoreError::InvalidInput);
    }
    Ok(course_id)
}

/// Validates the bound account's login id before it enters a body.
///
/// This value comes from the proven identity rather than from a caller, and it
/// is checked with the same rule the reference client applies to a login id:
/// digits only.  A username that is not a student id must not be sent as one.
pub(crate) fn student_id_for_request(student_id: &str) -> Result<&str, CourseScoreError> {
    if student_id.is_empty()
        || student_id.len() > MAX_COURSE_ID
        || !student_id.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(CourseScoreError::NotAStudentId);
    }
    Ok(student_id)
}

/// Parses the preset query's answer into one course result.
///
/// A missing field is not an error: it is how the service reports that it holds
/// nothing for this course, which [`CourseScore::is_empty`] then exposes.  A
/// present-but-malformed field is an error, so a broken answer can never become
/// an empty one.
pub(crate) fn parse_course_score(value: &Value) -> Result<CourseScore, CourseScoreError> {
    let object = value.as_object().ok_or(CourseScoreError::Response)?;
    let name = text(object, "KCMC")?;
    let grade = text(object, "DJZCJ")?;
    let credit = match object.get("XF") {
        None | Some(Value::Null) => None,
        Some(Value::Number(number)) => {
            let value = number.as_f64().ok_or(CourseScoreError::Response)?;
            if !is_plausible_credit(value) {
                return Err(CourseScoreError::Response);
            }
            Some(value)
        }
        Some(Value::String(raw)) => {
            let raw = raw.trim();
            if raw.is_empty() {
                None
            } else {
                let value: f64 = raw.parse().map_err(|_| CourseScoreError::Response)?;
                if !is_plausible_credit(value) {
                    return Err(CourseScoreError::Response);
                }
                Some(value)
            }
        }
        Some(_) => return Err(CourseScoreError::Response),
    };
    Ok(CourseScore::from_parts(name, credit, grade))
}

/// A course credit is a small non-negative number.  Anything else is a
/// malformed answer rather than a credit this client will pass on.
fn is_plausible_credit(value: f64) -> bool {
    value.is_finite() && (0.0..=100.0).contains(&value)
}

/// Reads one bounded text field, treating an absent value as empty.
fn text(object: &Map<String, Value>, key: &str) -> Result<String, CourseScoreError> {
    let value = match object.get(key) {
        None | Some(Value::Null) => return Ok(String::new()),
        Some(Value::String(raw)) => raw.trim().to_owned(),
        Some(Value::Number(number)) => number.to_string(),
        Some(_) => return Err(CourseScoreError::Response),
    };
    if value.len() > 8192 || value.chars().any(char::is_control) {
        return Err(CourseScoreError::Response);
    }
    Ok(value)
}
