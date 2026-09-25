//! Read-only academic-grade profile and parser for the Registrar service.
//!
//! This module deliberately stops at a request plan and a typed HTML parser.
//! It does not own the ALL_ZHJW ticket exchange, cookies, authentication, or
//! HTTP transport.  A later integration can attach [`RegistrarGradesRequest`]
//! to the existing Registrar session without exposing those credentials to a
//! caller that only needs grade data.
//!
//! The profile is based on the locally audited Registrar report route:
//! `/cj.cjCjbAll.do`.  Undergraduate and graduate reports use different `m`
//! values and different table column offsets. Reference getCheerioText uses
//! raw DOM child positions (including whitespace), whereas this parser extracts
//! td/th elements. Each supported element layout is verified by its header.

#[path = "registrar_grade_grid.rs"]
mod grade_grid;

use crate::protocol::AcademicStage;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// The read-only Registrar report endpoint observed in the local reference
/// audit.  The base host is intentionally owned by the existing Registrar
/// client; this module exposes only a path.
pub const GRADES_PATH: &str = "/cj.cjCjbAll.do";

const REPORT_TYPE_PARAMETER: &str = "cjdlx";
const REPORT_TYPE_VALUE: &str = "zw";

/// Undergraduate report selector used by the Registrar's `flag=diN` query
/// parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UndergraduateReportKind {
    /// The first-degree report (`flag=di1`).
    FirstDegree,
    /// The second-degree report (`flag=di2`).
    SecondDegree,
    /// The minor report (`flag=di3`).
    Minor,
}

impl UndergraduateReportKind {
    const fn wire_flag(self) -> &'static str {
        match self {
            Self::FirstDegree => "di1",
            Self::SecondDegree => "di2",
            Self::Minor => "di3",
        }
    }
}

/// Errors caused by combining a stage with an incompatible report selector.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RegistrarGradesProfileError {
    #[error("an undergraduate profile must select a report kind")]
    MissingUndergraduateReportKind,

    #[error("a graduate profile cannot select an undergraduate report kind")]
    GraduateReportKind,
}

/// Stable, transport-free profile for one Registrar grade report.
///
/// Use [`Self::undergraduate`] or [`Self::graduate`] to construct a valid
/// profile.  The fields remain private so a future integration can keep the
/// stage and query shape consistent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistrarGradesProfile {
    stage: AcademicStage,
    undergraduate_report: Option<UndergraduateReportKind>,
}

impl RegistrarGradesProfile {
    /// Creates a first-degree, second-degree, or minor undergraduate profile.
    pub const fn undergraduate(report: UndergraduateReportKind) -> Self {
        Self {
            stage: AcademicStage::Undergraduate,
            undergraduate_report: Some(report),
        }
    }

    /// Creates the graduate report profile.
    pub const fn graduate() -> Self {
        Self {
            stage: AcademicStage::Graduate,
            undergraduate_report: None,
        }
    }

    /// Creates a profile from stable domain values and validates their
    /// combination.  This is useful when the values came from serialized
    /// configuration rather than one of the constructors above.
    pub const fn for_stage(
        stage: AcademicStage,
        undergraduate_report: Option<UndergraduateReportKind>,
    ) -> Result<Self, RegistrarGradesProfileError> {
        match (stage, undergraduate_report) {
            (AcademicStage::Undergraduate, Some(report)) => Ok(Self {
                stage,
                undergraduate_report: Some(report),
            }),
            (AcademicStage::Undergraduate, None) => {
                Err(RegistrarGradesProfileError::MissingUndergraduateReportKind)
            }
            (AcademicStage::Graduate, Some(_)) => {
                Err(RegistrarGradesProfileError::GraduateReportKind)
            }
            (AcademicStage::Graduate, None) => Ok(Self {
                stage,
                undergraduate_report: None,
            }),
        }
    }

    pub const fn stage(self) -> AcademicStage {
        self.stage
    }

    pub const fn undergraduate_report(self) -> Option<UndergraduateReportKind> {
        self.undergraduate_report
    }

    const fn method(self) -> &'static str {
        match self.stage {
            AcademicStage::Undergraduate => "bks_cjdcx",
            AcademicStage::Graduate => "yjs_cjdcx",
        }
    }

    /// Grade permission is distinct from the teaching-calendar menu. This
    /// selector is used only if the actual report requires its own handoff.
    pub(crate) const fn roaming_selector(self) -> &'static str {
        match self.stage {
            AcademicStage::Undergraduate => "B7EF0ADF9406335AD7905B30CD7B49B1",
            AcademicStage::Graduate => "E35232808C08C8C5F199F13BF6B7F5D0",
        }
    }

    fn validate(self) -> Result<(), RegistrarGradesProfileError> {
        match (self.stage, self.undergraduate_report) {
            (AcademicStage::Undergraduate, Some(_)) | (AcademicStage::Graduate, None) => Ok(()),
            (AcademicStage::Undergraduate, None) => {
                Err(RegistrarGradesProfileError::MissingUndergraduateReportKind)
            }
            (AcademicStage::Graduate, Some(_)) => {
                Err(RegistrarGradesProfileError::GraduateReportKind)
            }
        }
    }

    /// Builds the deterministic GET plan for this report.
    ///
    /// The plan contains only the endpoint path and non-secret query values.
    /// The caller must execute it with the existing authenticated Registrar
    /// Cookie session.
    pub fn request(self) -> Result<RegistrarGradesRequest, RegistrarGradesProfileError> {
        self.validate()?;

        let mut query = vec![
            RegistrarQueryParameter {
                name: "m".to_owned(),
                value: self.method().to_owned(),
            },
            RegistrarQueryParameter {
                name: REPORT_TYPE_PARAMETER.to_owned(),
                value: REPORT_TYPE_VALUE.to_owned(),
            },
        ];
        if let Some(report) = self.undergraduate_report {
            query.push(RegistrarQueryParameter {
                name: "flag".to_owned(),
                value: report.wire_flag().to_owned(),
            });
        }

        Ok(RegistrarGradesRequest {
            method: RegistrarRequestMethod::Get,
            path: GRADES_PATH.to_owned(),
            query,
            authentication: RegistrarGradesAuthentication::ExistingRegistrarSession,
        })
    }

    /// Parses one response using the stage-specific table layout.
    pub fn parse_html(self, body: &str) -> Result<RegistrarGradeReport, RegistrarGradesParseError> {
        self.validate()?;
        parse_grades_html(self, body)
    }
}

/// HTTP method exposed by a read-only Registrar request plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum RegistrarRequestMethod {
    Get,
}

/// Authentication boundary for the grades request.
///
/// The value is intentionally a capability marker.  It does not contain the
/// ticket, Cookie, CSRF token, or any other session material.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegistrarGradesAuthentication {
    ExistingRegistrarSession,
}

/// One deterministic query parameter in a Registrar request plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistrarQueryParameter {
    pub name: String,
    pub value: String,
}

/// A read-only request plan ready for later use by the existing Registrar
/// transport.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistrarGradesRequest {
    pub method: RegistrarRequestMethod,
    pub path: String,
    pub query: Vec<RegistrarQueryParameter>,
    pub authentication: RegistrarGradesAuthentication,
}

/// A normalized report returned by [`RegistrarGradesProfile::parse_html`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RegistrarGradeReport {
    pub stage: AcademicStage,
    pub undergraduate_report: Option<UndergraduateReportKind>,
    pub courses: Vec<RegistrarCourseGrade>,
}

/// Stable course-grade DTO independent of the Registrar table layout.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RegistrarCourseGrade {
    pub course_name: String,
    pub credit: f64,
    pub grade: String,
    pub grade_point: Option<f64>,
    pub semester: String,
}

/// The field represented by a required table cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistrarGradeField {
    CourseName,
    Credit,
    Grade,
    GradePoint,
    Semester,
}

impl std::fmt::Display for RegistrarGradeField {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::CourseName => "course name",
            Self::Credit => "credit",
            Self::Grade => "grade",
            Self::GradePoint => "grade point",
            Self::Semester => "semester",
        })
    }
}

/// Strict parser failures for an HTML grade report.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RegistrarGradesParseError {
    #[error(transparent)]
    InvalidProfile(#[from] RegistrarGradesProfileError),

    #[error("the Registrar grade response is empty")]
    EmptyResponse,

    #[error("the Registrar session appears to have expired")]
    SessionExpired,

    #[error("the Registrar grade table is missing")]
    MissingGradeTable,

    #[error("the Registrar grade response returned a business failure: {message}")]
    BusinessFailure { message: String },

    #[error("the Registrar grade table has no header row")]
    MissingHeaderRow,

    #[error("the Registrar grade table header at column {column} is not {expected:?}")]
    InvalidHeader {
        column: usize,
        expected: &'static str,
    },

    #[error("grade row {row} has {actual} cells; at least {expected} are required")]
    WrongColumnCount {
        row: usize,
        actual: usize,
        expected: usize,
    },

    #[error("grade row {row} has an empty {field}")]
    EmptyField {
        row: usize,
        field: RegistrarGradeField,
    },

    #[error("grade row {row} has an invalid {field}: {value}")]
    InvalidNumber {
        row: usize,
        field: RegistrarGradeField,
        value: String,
    },

    #[error("the Registrar grade table is malformed: {context}")]
    MalformedHtml { context: &'static str },
}

#[derive(Debug, Clone, Copy)]
struct GradeColumnLayout {
    course_code: usize,
    course_name: usize,
    credit: usize,
    grade: usize,
    grade_point: usize,
    semester: usize,
    width: usize,
    reference_semester_position: bool,
}

/// Parses the known Registrar HTML table into stable course-grade records.
///
/// The parser considers tables whose opening tag has `cellspacing="1"` (or an
/// equivalent unquoted/single-quoted value). A unique semantic header binds
/// every required field after bounded rowspan/colspan expansion. Title rows,
/// extra context columns and whitespace never change those field bindings.
pub fn parse_grades_html(
    profile: RegistrarGradesProfile,
    body: &str,
) -> Result<RegistrarGradeReport, RegistrarGradesParseError> {
    crate::telemetry::timing::measure_sync(crate::telemetry::timing::Phase::Decode, || {
        profile.validate()?;
        if body.len() > 8 * 1024 * 1024 {
            return Err(RegistrarGradesParseError::MalformedHtml {
                context: "grade response size limit",
            });
        }
        if body.trim().is_empty() {
            return Err(RegistrarGradesParseError::EmptyResponse);
        }

        // Login and timeout templates are often malformed, and some contain a
        // decorative table. Classify them before table discovery so a dead
        // session cannot be reported as a missing or malformed grade table.
        if looks_like_expired_session(body) {
            return Err(RegistrarGradesParseError::SessionExpired);
        }
        if let Some(message) = grade_business_failure_reason(body) {
            return Err(RegistrarGradesParseError::BusinessFailure { message });
        }

        let tables = find_grade_tables(body)
            .map_err(|context| RegistrarGradesParseError::MalformedHtml { context })?;

        let mut selected = None;
        let mut best_failure = None;
        let mut candidate_bytes = 0usize;
        for (index, table) in tables.into_iter().enumerate() {
            candidate_bytes = candidate_bytes.saturating_add(table.len());
            if candidate_bytes > 16 * 1024 * 1024 {
                return Err(RegistrarGradesParseError::MalformedHtml {
                    context: "grade response size limit",
                });
            }
            match grade_grid::read(table, profile.stage, index) {
                Ok(parsed) => {
                    if selected.is_some() {
                        return Err(RegistrarGradesParseError::MalformedHtml {
                            context: "multiple grade reports",
                        });
                    }
                    selected = Some(parsed);
                }
                Err(failure) => {
                    if best_failure
                        .as_ref()
                        .is_none_or(|old: &grade_grid::TableFailure| {
                            failure.confidence > old.confidence
                        })
                    {
                        best_failure = Some(failure);
                    }
                }
            }
        }
        let parsed = match selected {
            Some(parsed) => parsed,
            None => {
                return Err(best_failure
                    .map(|f| f.error)
                    .unwrap_or(RegistrarGradesParseError::MissingGradeTable));
            }
        };
        // A second schema-bearing malformed report is not harmless decoration.
        if best_failure.is_some_and(|failure| failure.confidence >= 5) {
            return Err(RegistrarGradesParseError::MalformedHtml {
                context: "ambiguous grade report containers",
            });
        }
        let layout = parsed.layout;
        let mut courses = Vec::with_capacity(parsed.rows.len());
        for (row_index, row) in parsed.rows {
            let course_name = required_text(
                row_index,
                RegistrarGradeField::CourseName,
                &row[layout.course_name],
            )?;
            let credit_text =
                required_text(row_index, RegistrarGradeField::Credit, &row[layout.credit])?;
            let credit = parse_number(row_index, RegistrarGradeField::Credit, &credit_text)?;
            if credit <= 0.0 {
                return Err(RegistrarGradesParseError::InvalidNumber {
                    row: row_index,
                    field: RegistrarGradeField::Credit,
                    value: credit_text,
                });
            }

            let grade = required_text(row_index, RegistrarGradeField::Grade, &row[layout.grade])?;
            let grade = normalize_grade(&grade);

            let grade_point_text = normalize_text(&row[layout.grade_point]);
            let grade_point =
                if grade_point_text.is_empty() || missing_grade_point_marker(&grade_point_text) {
                    None
                } else {
                    let parsed = parse_number(
                        row_index,
                        RegistrarGradeField::GradePoint,
                        &grade_point_text,
                    )?;
                    if !parsed.is_finite() || parsed < 0.0 {
                        return Err(RegistrarGradesParseError::InvalidNumber {
                            row: row_index,
                            field: RegistrarGradeField::GradePoint,
                            value: grade_point_text,
                        });
                    }
                    Some(parsed)
                };

            let semester = required_text(
                row_index,
                RegistrarGradeField::Semester,
                &row[layout.semester],
            )?;

            courses.push(RegistrarCourseGrade {
                course_name,
                credit,
                grade,
                grade_point,
                semester: normalize_semester(&semester),
            });
        }

        Ok(RegistrarGradeReport {
            stage: profile.stage,
            undergraduate_report: profile.undergraduate_report,
            courses,
        })
    })
}

fn missing_grade_point_marker(value: &str) -> bool {
    matches!(
        value.to_ascii_uppercase().as_str(),
        "-" | "—" | "–" | "－" | "N/A" | "NA" | "N.A."
    )
}

fn required_text(
    row: usize,
    field: RegistrarGradeField,
    value: &str,
) -> Result<String, RegistrarGradesParseError> {
    let value = normalize_text(value);
    if value.is_empty() {
        return Err(RegistrarGradesParseError::EmptyField { row, field });
    }
    Ok(value)
}

fn parse_number(
    row: usize,
    field: RegistrarGradeField,
    value: &str,
) -> Result<f64, RegistrarGradesParseError> {
    if value.is_empty()
        || !value.chars().any(|character| character.is_ascii_digit())
        || value
            .chars()
            .any(|character| !character.is_ascii_digit() && character != '.')
        || value.matches('.').count() > 1
    {
        tracing::warn!(
            target: "tsinghua_kit::api",
            event = "registrar_grade_structure",
            service = "registrar",
            business_stage = "registrar_grade_data",
            reason = "registrar_grade_number",
            grade_field = grade_field_key(field),
            grade_value_shape = grade_value_shape(value),
            row_index = row as u64,
        );
        return Err(RegistrarGradesParseError::InvalidNumber {
            row,
            field,
            value: value.to_owned(),
        });
    }

    let parsed = value
        .parse::<f64>()
        .ok()
        .filter(|number| number.is_finite())
        .ok_or_else(|| RegistrarGradesParseError::InvalidNumber {
            row,
            field,
            value: value.to_owned(),
        })?;
    Ok(parsed)
}

fn grade_field_key(field: RegistrarGradeField) -> &'static str {
    match field {
        RegistrarGradeField::CourseName => "course_name",
        RegistrarGradeField::Credit => "credit",
        RegistrarGradeField::Grade => "grade",
        RegistrarGradeField::GradePoint => "grade_point",
        RegistrarGradeField::Semester => "semester",
    }
}

fn grade_value_shape(value: &str) -> &'static str {
    if value.is_empty() {
        return "empty";
    }
    if value.chars().all(|character| character.is_ascii()) {
        let letters = value
            .chars()
            .any(|character| character.is_ascii_alphabetic());
        let digits = value.chars().any(|character| character.is_ascii_digit());
        let punctuation = value
            .chars()
            .any(|character| character.is_ascii_punctuation());
        let whitespace = value
            .chars()
            .any(|character| character.is_ascii_whitespace());
        return match (letters, digits, punctuation, whitespace) {
            (true, true, true, _) => "ascii_letters_digits_punctuation",
            (true, true, false, _) => "ascii_letters_digits",
            (true, false, true, _) => "ascii_letters_punctuation",
            (true, false, false, true) => "ascii_letters_space",
            (true, false, false, false) => "ascii_letters",
            (false, true, _, _) => "ascii_numeric_other",
            (false, false, true, _) => "ascii_punctuation",
            _ => "ascii_other",
        };
    }
    if value.chars().all(|character| !character.is_ascii()) {
        return "unicode";
    }
    "mixed_unicode"
}

fn normalize_grade(value: &str) -> String {
    let value = normalize_text(value);
    if value.is_ascii() {
        value.to_ascii_uppercase()
    } else {
        value
    }
}

fn normalize_semester(value: &str) -> String {
    let value =
        normalize_text(value).replace(['－', '‐', '‑', '‒', '–', '—', '﹘', '﹣', '－'], "-");
    if value.contains('-') {
        value
            .split('-')
            .map(str::trim)
            .collect::<Vec<_>>()
            .join("-")
    } else {
        value
    }
}

fn normalize_text(value: &str) -> String {
    let mut result = String::new();
    let mut pending_space = false;
    for character in value.chars() {
        if character.is_whitespace() || character == '\u{a0}' {
            pending_space = !result.is_empty();
            continue;
        }
        if pending_space {
            result.push(' ');
            pending_space = false;
        }
        result.push(character);
    }
    result.trim().to_owned()
}

fn looks_like_expired_session(body: &str) -> bool {
    // Share the client's explicit login evidence. WebVPN script references,
    // SSO course names and hidden JavaScript strings do not prove expiration.
    crate::registrar_client::looks_like_login_page(body)
}

fn grade_business_failure_reason(body: &str) -> Option<String> {
    let document = scraper::Html::parse_document(body);
    let lower = document
        .root_element()
        .descendants()
        .filter_map(|node| {
            let text = node.value().as_text()?;
            let hidden = node
                .ancestors()
                .filter_map(|parent| parent.value().as_element())
                .any(|element| {
                    matches!(element.name(), "script" | "style" | "template" | "noscript")
                        || element.attr("hidden").is_some()
                });
            (!hidden).then(|| text.to_string())
        })
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();
    [
        ("查询失败", "upstream grade query failed"),
        ("系统错误", "upstream grade system error"),
        ("系统异常", "upstream grade system exception"),
        ("无权访问", "upstream grade access denied"),
        ("没有权限", "upstream grade access denied"),
        ("service unavailable", "upstream grade service unavailable"),
        ("服务不可用", "upstream grade service unavailable"),
        ("暂时不可用", "upstream grade service unavailable"),
        ("网络失败", "upstream grade network failure"),
        ("请求失败", "upstream grade request failed"),
        ("internal server error", "upstream grade server error"),
        ("permission denied", "upstream grade access denied"),
        ("access denied", "upstream grade access denied"),
    ]
    .into_iter()
    .find_map(|(marker, message)| lower.contains(marker).then(|| message.to_owned()))
}

#[derive(Debug, Clone, Copy)]
struct TagSpan {
    start: usize,
    end: usize,
}

fn find_grade_tables(body: &str) -> Result<Vec<&str>, &'static str> {
    // Balance containers instead of pairing an outer layout table with the
    // first inner closing tag. Candidates keep their own rows; nested
    // layout tables cannot supply another table's required header fields.
    let mut tables = Vec::new();
    let mut stack: Vec<(usize, bool, bool)> = Vec::new();
    let mut cursor = 0;
    let mut seen = 0usize;
    while let Some(relative) = body[cursor..].find('<') {
        let start = cursor + relative;
        if body[start..].starts_with("<!--") {
            let end = body[start + 4..].find("-->").ok_or("unclosed comment")?;
            cursor = start + 4 + end + 3;
            continue;
        }
        let end = find_tag_end(body, start).ok_or("unclosed table")?;
        let tag = &body[start..end];
        let name = tag_name(tag).unwrap_or_default().to_ascii_lowercase();
        let closing = tag[1..].trim_start().starts_with('/');
        if !closing && matches!(name.as_str(), "script" | "style" | "template" | "noscript") {
            cursor = find_close_tag(body, &name, end)
                .ok_or("unclosed inert element")?
                .end;
            continue;
        }
        cursor = end;
        if name != "table" {
            continue;
        }
        if closing {
            let (begin, matched, _nested) = stack.pop().ok_or("unexpected table close")?;
            if matched {
                tables.push(&body[begin..start]);
            }
        } else {
            seen += 1;
            if seen > 256 || stack.len() >= 64 {
                return Err("table nesting limit");
            }
            if let Some(parent) = stack.last_mut() {
                parent.2 = true;
            }
            let matched = attribute_value(tag, "cellspacing")
                .is_some_and(|value| normalize_text(&value) == "1");
            stack.push((end, matched, false));
        }
    }
    if !stack.is_empty() {
        return Err("unclosed table");
    }
    Ok(tables)
}

fn find_open_tag(input: &str, wanted: &str, from: usize) -> Option<TagSpan> {
    let mut cursor = from;
    while cursor < input.len() {
        let relative = input[cursor..].find('<')?;
        let start = cursor + relative;
        let mut name_start = start + 1;
        while name_start < input.len() && input.as_bytes()[name_start].is_ascii_whitespace() {
            name_start += 1;
        }
        if name_start >= input.len() || matches!(input.as_bytes()[name_start], b'/' | b'!' | b'?') {
            cursor = start + 1;
            continue;
        }
        let name_end = ascii_name_end(input, name_start);
        if name_end > name_start
            && input[name_start..name_end].eq_ignore_ascii_case(wanted)
            && tag_name_boundary(input, name_end)
        {
            let end = find_tag_end(input, start)?;
            return Some(TagSpan { start, end });
        }
        cursor = start + 1;
    }
    None
}

fn find_close_tag(input: &str, wanted: &str, from: usize) -> Option<TagSpan> {
    let mut cursor = from;
    while cursor < input.len() {
        let relative = input[cursor..].find("</")?;
        let start = cursor + relative;
        let name_start = start + 2;
        let name_end = ascii_name_end(input, name_start);
        if name_end > name_start
            && input[name_start..name_end].eq_ignore_ascii_case(wanted)
            && tag_name_boundary(input, name_end)
        {
            let end = find_tag_end(input, start)?;
            return Some(TagSpan { start, end });
        }
        cursor = start + 2;
    }
    None
}

fn ascii_name_end(input: &str, start: usize) -> usize {
    let mut end = start;
    while end < input.len() {
        let character = input.as_bytes()[end];
        if character.is_ascii_alphanumeric()
            || character == b':'
            || character == b'_'
            || character == b'-'
        {
            end += 1;
        } else {
            break;
        }
    }
    end
}

fn tag_name_boundary(input: &str, position: usize) -> bool {
    position >= input.len()
        || input.as_bytes()[position].is_ascii_whitespace()
        || matches!(input.as_bytes()[position], b'>' | b'/')
}

fn find_tag_end(input: &str, start: usize) -> Option<usize> {
    let mut quote = None;
    for (offset, character) in input.as_bytes()[start..].iter().copied().enumerate() {
        match (quote, character) {
            (Some(expected), value) if value == expected => quote = None,
            (None, b'\'' | b'"') => quote = Some(character),
            (None, b'>') => return Some(start + offset + 1),
            _ => {}
        }
    }
    None
}

fn tag_name(tag: &str) -> Option<&str> {
    let mut start = 1;
    while start < tag.len() && tag.as_bytes()[start].is_ascii_whitespace() {
        start += 1;
    }
    if start < tag.len() && tag.as_bytes()[start] == b'/' {
        start += 1;
    }
    let end = ascii_name_end(tag, start);
    (end > start).then(|| &tag[start..end])
}

fn attribute_value(tag: &str, wanted: &str) -> Option<String> {
    let mut cursor = 1;
    while cursor < tag.len() && tag.as_bytes()[cursor].is_ascii_whitespace() {
        cursor += 1;
    }
    cursor = ascii_name_end(tag, cursor);
    while cursor < tag.len() {
        while cursor < tag.len()
            && (tag.as_bytes()[cursor].is_ascii_whitespace()
                || matches!(tag.as_bytes()[cursor], b'/' | b'>'))
        {
            cursor += 1;
        }
        if cursor >= tag.len() || tag.as_bytes()[cursor] == b'>' {
            break;
        }
        let name_start = cursor;
        let name_end = ascii_name_end(tag, name_start);
        if name_end == name_start {
            cursor += 1;
            continue;
        }
        cursor = name_end;
        while cursor < tag.len() && tag.as_bytes()[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if cursor >= tag.len() || tag.as_bytes()[cursor] != b'=' {
            continue;
        }
        cursor += 1;
        while cursor < tag.len() && tag.as_bytes()[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        let value = if cursor < tag.len() && matches!(tag.as_bytes()[cursor], b'\'' | b'"') {
            let quote = tag.as_bytes()[cursor];
            cursor += 1;
            let value_start = cursor;
            while cursor < tag.len() && tag.as_bytes()[cursor] != quote {
                cursor += 1;
            }
            let value = tag[value_start..cursor].to_owned();
            if cursor < tag.len() {
                cursor += 1;
            }
            value
        } else {
            let value_start = cursor;
            while cursor < tag.len()
                && !tag.as_bytes()[cursor].is_ascii_whitespace()
                && tag.as_bytes()[cursor] != b'>'
            {
                cursor += 1;
            }
            tag[value_start..cursor].to_owned()
        };
        if tag[name_start..name_end].eq_ignore_ascii_case(wanted) {
            return Some(value);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::AcademicStage;

    const UNDERGRADUATE_FIXTURE: &str = r#"
        <html><body>
        <table id="table1" cellspacing="1">
          <tr>
            <th>序号</th><th>课程号</th><th>课程类别</th><th>课程名称</th>
            <th>属性</th><th>学分</th><th>学时</th><th>成绩</th>
            <th>备注</th><th>绩点</th><th>教师</th><th>学期</th>
          </tr>
          <tr>
            <td>1</td><td>30240512</td><td>必修</td><td><a>线性&nbsp;代数</a></td>
            <td>必修</td><td>4.0</td><td>64</td><td> a+ </td>
            <td></td><td>4.0</td><td>张老师</td><td>2024 - 秋</td>
          </tr>
          <tr>
            <td>2</td><td>30240513</td><td>选修</td><td>程序设计&lt;基础&gt;</td>
            <td>选修</td><td>2</td><td>32</td><td>通过</td>
            <td></td><td>—</td><td>李老师</td><td>2025-春</td>
          </tr>
        </table>
        </body></html>
    "#;

    const GRADUATE_FIXTURE: &str = r#"
        <table cellspacing='1'>
          <tr>
            <th>序号</th><th>课程号</th><th>课程类别</th><th>课程名称</th>
            <th>属性</th><th>学分</th><th>学时</th><th>考核方式</th>
            <th>备注</th><th>成绩</th><th>教师</th><th>绩点</th>
            <th>类别</th><th>学期</th>
          </tr>
          <tr>
            <td>1</td><td>70510012</td><td>学位课</td><td><span>量子力学</span></td>
            <td>必修</td><td>3</td><td>48</td><td>考试</td>
            <td></td><td>88</td><td>王老师</td><td>3.7</td>
            <td>学位</td><td>2024/2025-1</td>
          </tr>
        </table>
    "#;

    #[test]
    fn request_profiles_keep_stage_routes_and_query_order_explicit() {
        let undergraduate =
            RegistrarGradesProfile::undergraduate(UndergraduateReportKind::SecondDegree)
                .request()
                .expect("valid undergraduate request");
        assert_eq!(undergraduate.method, RegistrarRequestMethod::Get);
        assert_eq!(undergraduate.path, GRADES_PATH);
        assert_eq!(
            undergraduate.query,
            vec![
                RegistrarQueryParameter {
                    name: "m".to_owned(),
                    value: "bks_cjdcx".to_owned(),
                },
                RegistrarQueryParameter {
                    name: "cjdlx".to_owned(),
                    value: "zw".to_owned(),
                },
                RegistrarQueryParameter {
                    name: "flag".to_owned(),
                    value: "di2".to_owned(),
                },
            ]
        );

        let graduate = RegistrarGradesProfile::graduate()
            .request()
            .expect("valid graduate request");
        assert_eq!(
            graduate.query,
            vec![
                RegistrarQueryParameter {
                    name: "m".to_owned(),
                    value: "yjs_cjdcx".to_owned(),
                },
                RegistrarQueryParameter {
                    name: "cjdlx".to_owned(),
                    value: "zw".to_owned(),
                },
            ]
        );
        assert_eq!(
            graduate.authentication,
            RegistrarGradesAuthentication::ExistingRegistrarSession
        );
    }

    #[test]
    fn rejects_invalid_deserialized_profile_combinations() {
        assert_eq!(
            RegistrarGradesProfile::for_stage(AcademicStage::Undergraduate, None),
            Err(RegistrarGradesProfileError::MissingUndergraduateReportKind)
        );
        assert_eq!(
            RegistrarGradesProfile::for_stage(
                AcademicStage::Graduate,
                Some(UndergraduateReportKind::Minor),
            ),
            Err(RegistrarGradesProfileError::GraduateReportKind)
        );
    }

    #[test]
    fn parses_and_normalizes_undergraduate_rows() {
        let report = RegistrarGradesProfile::undergraduate(UndergraduateReportKind::FirstDegree)
            .parse_html(UNDERGRADUATE_FIXTURE)
            .expect("undergraduate fixture parses");
        assert_eq!(report.stage, AcademicStage::Undergraduate);
        assert_eq!(report.courses.len(), 2);
        assert_eq!(
            report.courses[0],
            RegistrarCourseGrade {
                course_name: "线性 代数".to_owned(),
                credit: 4.0,
                grade: "A+".to_owned(),
                grade_point: Some(4.0),
                semester: "2024-秋".to_owned(),
            }
        );
        assert_eq!(report.courses[1].grade, "通过");
        assert_eq!(report.courses[1].grade_point, None);
        assert_eq!(report.courses[1].course_name, "程序设计<基础>");
    }

    #[test]
    fn uses_graduate_column_offsets_and_allows_empty_grade_point() {
        let report = RegistrarGradesProfile::graduate()
            .parse_html(GRADUATE_FIXTURE)
            .expect("graduate fixture parses");
        assert_eq!(report.stage, AcademicStage::Graduate);
        assert_eq!(report.courses.len(), 1);
        assert_eq!(report.courses[0].course_name, "量子力学");
        assert_eq!(report.courses[0].grade, "88");
        assert_eq!(report.courses[0].grade_point, Some(3.7));
        assert_eq!(report.courses[0].semester, "2024/2025-1");
    }

    #[test]
    fn accepts_a_header_only_report_as_a_valid_empty_result() {
        let header_only = r#"
            <table cellspacing="1">
              <tr>
                <th>序号</th><th>课程号</th><th>课程类别</th><th>课程名称</th>
                <th>属性</th><th>学分</th><th>学时</th><th>成绩</th>
                <th>备注</th><th>绩点</th><th>教师</th><th>学期</th>
              </tr>
            </table>
        "#;
        let report = RegistrarGradesProfile::undergraduate(UndergraduateReportKind::FirstDegree)
            .parse_html(header_only)
            .expect("header-only report parses");
        assert!(report.courses.is_empty());
    }

    #[test]
    fn rejects_a_same_sized_non_grade_table_instead_of_returning_empty_data() {
        let unrelated = r#"
            <table cellspacing="1">
              <tr><th>a</th><th>b</th><th>c</th><th>d</th><th>e</th><th>f</th>
                <th>g</th><th>h</th><th>i</th><th>j</th><th>k</th><th>l</th></tr>
            </table>
        "#;
        assert!(matches!(
            RegistrarGradesProfile::undergraduate(UndergraduateReportKind::FirstDegree)
                .parse_html(unrelated),
            Err(RegistrarGradesParseError::InvalidHeader { column: 3, .. })
        ));
    }

    #[test]
    fn selects_the_matching_grade_table_after_an_unrelated_table() {
        let body = format!(
            r#"
                <table cellspacing="1">
                  <tr><th>layout</th><th>metadata</th></tr>
                </table>
                {UNDERGRADUATE_FIXTURE}
            "#
        );
        let report = RegistrarGradesProfile::undergraduate(UndergraduateReportKind::FirstDegree)
            .parse_html(&body)
            .expect("the matching grade table is not necessarily first");
        assert_eq!(report.courses.len(), 2);
        assert_eq!(report.courses[0].course_name, "线性 代数");
    }

    #[test]
    fn rejects_login_pages_malformed_tables_and_shifted_rows() {
        assert_eq!(
            RegistrarGradesProfile::graduate().parse_html(
                "<html><title>清华大学WebVPN</title><form>login password</form></html>"
            ),
            Err(RegistrarGradesParseError::SessionExpired)
        );
        assert_eq!(
            RegistrarGradesProfile::graduate().parse_html("<table cellspacing=\"1\"><tr><td>x"),
            Err(RegistrarGradesParseError::MalformedHtml {
                context: "unclosed table"
            })
        );

        let shifted = r#"
            <table cellspacing="1">
              <tr><th>序号</th><th>课程号</th><th>课程类别</th><th>课程名称</th><th>属性</th><th>学分</th><th>学时</th><th>成绩</th><th>备注</th><th>绩点</th><th>教师</th><th>学期</th></tr>
              <tr><td>1</td><td>2</td><td>3</td><td>name</td><td>5</td><td>not-a-number</td><td>7</td><td>A</td><td>9</td><td>4</td><td>11</td><td>2024-秋</td></tr>
            </table>
        "#;
        assert!(matches!(
            RegistrarGradesProfile::undergraduate(UndergraduateReportKind::FirstDegree)
                .parse_html(shifted),
            Err(RegistrarGradesParseError::InvalidNumber {
                field: RegistrarGradeField::Credit,
                ..
            })
        ));
    }

    #[test]
    fn rejects_non_numeric_and_empty_required_values() {
        let invalid = r#"
            <table cellspacing="1">
              <tr><th>序号</th><th>课程号</th><th>课程类别</th><th>课程名称</th><th>属性</th><th>学分</th><th>学时</th><th>成绩</th><th>备注</th><th>绩点</th><th>教师</th><th>学期</th></tr>
              <tr><td>1</td><td>2</td><td>3</td><td></td><td>5</td><td>2</td><td>7</td><td>A</td><td>9</td><td>4x</td><td>11</td><td>2024-秋</td></tr>
            </table>
        "#;
        assert_eq!(
            RegistrarGradesProfile::undergraduate(UndergraduateReportKind::FirstDegree)
                .parse_html(invalid),
            Err(RegistrarGradesParseError::EmptyField {
                row: 1,
                field: RegistrarGradeField::CourseName,
            })
        );
    }

    #[test]
    fn rejects_negative_grade_points_and_explicit_upstream_failures() {
        let negative_point = r#"
            <table cellspacing="1">
              <tr><th>序号</th><th>课程号</th><th>课程类别</th><th>课程名称</th><th>属性</th><th>学分</th><th>学时</th><th>成绩</th><th>备注</th><th>绩点</th><th>教师</th><th>学期</th></tr>
              <tr><td>1</td><td>2</td><td>3</td><td>课程</td><td>5</td><td>2</td><td>7</td><td>A</td><td>9</td><td>-1</td><td>11</td><td>2024-秋</td></tr>
            </table>
        "#;
        assert!(matches!(
            RegistrarGradesProfile::undergraduate(UndergraduateReportKind::FirstDegree)
                .parse_html(negative_point),
            Err(RegistrarGradesParseError::InvalidNumber {
                field: RegistrarGradeField::GradePoint,
                ..
            })
        ));

        assert!(matches!(
            RegistrarGradesProfile::graduate()
                .parse_html("<html><body><div>成绩查询失败：系统异常</div></body></html>"),
            Err(RegistrarGradesParseError::BusinessFailure { .. })
        ));
    }

    #[test]
    fn report_dto_round_trips_without_transport_fields() {
        let report = RegistrarGradesProfile::undergraduate(UndergraduateReportKind::FirstDegree)
            .parse_html(UNDERGRADUATE_FIXTURE)
            .expect("fixture parses");
        let encoded = serde_json::to_string(&report).expect("report serializes");
        assert!(!encoded.contains("ticket"));
        assert!(!encoded.contains("Cookie"));
        let decoded: RegistrarGradeReport = serde_json::from_str(&encoded).expect("report decodes");
        assert_eq!(decoded, report);
    }
}

#[cfg(test)]
mod run072714_tests {
    use super::*;

    fn reference_table(graduate: bool, whitespace: &str) -> String {
        // getCheerioText(row, 3/5/7/9/11) indexes raw child nodes.
        // With whitespace between tags these are td 1/2/3/4/5, NOT
        // the 4th/6th/8th/10th/12th HTML columns.
        let (headers, values): (Vec<&str>, Vec<&str>) = if graduate {
            (
                vec![
                    "课程号",
                    "课程名称",
                    "学分",
                    "考核方式",
                    "成绩",
                    "绩点",
                    "学期",
                ],
                vec![
                    "fixture-course",
                    "Fixture graduate course",
                    "3",
                    "考试",
                    "A",
                    "4.0",
                    "2026-2027-1",
                ],
            )
        } else {
            (
                vec!["课程号", "课程名称", "学分", "成绩", "绩点", "学期"],
                vec![
                    "fixture-course",
                    "Fixture undergraduate course",
                    "2",
                    "B+",
                    "3.3",
                    "2026-2027-1",
                ],
            )
        };
        let row = |cells: &[&str]| {
            format!(
                "<tr>{w}{cells}{w}</tr>",
                w = whitespace,
                cells = cells
                    .iter()
                    .map(|s| format!("<td>{s}</td>"))
                    .collect::<Vec<_>>()
                    .join(whitespace)
            )
        };
        format!(
            "<table id='table1' cellspacing='1'>{}{}</table>",
            row(&headers),
            row(&values)
        )
    }

    #[test]
    fn backend_repair_run072714_reference_dom_offsets_decode_actual_grade_cells() {
        for graduate in [false, true] {
            let profile = if graduate {
                RegistrarGradesProfile::graduate()
            } else {
                RegistrarGradesProfile::undergraduate(UndergraduateReportKind::FirstDegree)
            };
            for whitespace in ["\n    ", "", "\r\n\t"] {
                let report = profile
                    .parse_html(&reference_table(graduate, whitespace))
                    .expect("reference element-cell layout");
                assert_eq!(report.courses.len(), 1);
                let course = &report.courses[0];
                assert_eq!(course.credit, if graduate { 3.0 } else { 2.0 });
                assert_eq!(course.grade, if graduate { "A" } else { "B+" });
                assert_eq!(course.grade_point, Some(if graduate { 4.0 } else { 3.3 }));
                assert_eq!(course.semester, "2026-2027-1");
            }
        }
    }

    #[test]
    fn backend_repair_run072714_compact_grade_layout_still_rejects_wrong_schema() {
        let html = reference_table(true, "\n");
        assert!(
            RegistrarGradesProfile::undergraduate(UndergraduateReportKind::FirstDegree)
                .parse_html(&html)
                .is_err()
        );
        assert!(
            RegistrarGradesProfile::graduate()
                .parse_html(&html.replace("课程号", "账号"))
                .is_err()
        );
        assert!(
            RegistrarGradesProfile::graduate()
                .parse_html(&html.replace("<td>3</td>", "<td>unknown</td>"))
                .is_err()
        );
        assert!(
            RegistrarGradesProfile::graduate()
                .parse_html(&html.replace("<td>A</td>", "<td></td>"))
                .is_err()
        );
    }
    #[test]
    fn backend_repair_run072714_nested_grade_report_ignores_inert_webvpn_markup() {
        let table = reference_table(true, "\n");
        let html = format!(
            "<html><head><script src='/wengine-vpn/webvpn.js'></script><script>const warning='系统错误'; const tpl='<table cellspacing=1>ignored</table>';</script></head><body><table cellspacing='0'><tr><td>{table}</td></tr></table></body></html>"
        );
        let result = RegistrarGradesProfile::graduate()
            .parse_html(&html)
            .unwrap();
        assert_eq!(result.courses.len(), 1);
        assert_eq!(result.courses[0].grade, "A");
        assert!(
            RegistrarGradesProfile::graduate()
                .parse_html(&format!("<input type='password'>{html}"))
                .is_err()
        );
        assert!(
            RegistrarGradesProfile::graduate()
                .parse_html(&format!("<p>用户登陆超时，请先登录</p>{html}"))
                .is_err()
        );
        assert!(
            RegistrarGradesProfile::graduate()
                .parse_html(&format!("<p>没有权限</p>{html}"))
                .is_err()
        );
    }

    #[test]
    fn backend_repair_run072714_ambiguous_and_unclosed_grade_containers_fail_closed() {
        let table = reference_table(false, "\n");
        let profile = RegistrarGradesProfile::undergraduate(UndergraduateReportKind::FirstDegree);
        assert!(profile.parse_html(&format!("{table}{table}")).is_err());
        assert!(
            profile
                .parse_html(&format!("<table><tr><td>{table}"))
                .is_err()
        );
        assert!(
            profile
                .parse_html("<table cellspacing='1'><tr><td>x")
                .is_err()
        );
    }
}

#[cfg(test)]
mod run113847_tests {
    use super::*;
    fn table(header: &str, body: &str) -> String {
        format!("<table id='table1' cellspacing='1'>{header}{body}</table>")
    }
    const HEADER: &str = "<tr><th>课程号</th><th>课程名称</th><th>学分</th><th>考核方式</th><th>成绩</th><th>绩点</th><th>学期</th></tr>";
    const ROW: &str = "<tr><td>C001</td><td>Fixture</td><td>3</td><td>考试</td><td>A</td><td>4.0</td><td>2026-2027-1</td></tr>";
    #[test]
    fn backend_repair_run113847_grade_title_and_trailing_columns_do_not_shift_fields() {
        let header = HEADER.replace("</tr>", "<th>备注</th></tr>");
        let row = ROW.replace("</tr>", "<td>已审核</td></tr>");
        let html = table(
            &format!("<tr><th colspan='8'>研究生成绩单</th></tr>{header}"),
            &row,
        );
        let report = RegistrarGradesProfile::graduate().parse_html(&html).expect(
            "schema-bearing header follows a title, optional trailing column is not grade data",
        );
        assert_eq!(report.courses.len(), 1);
        assert_eq!(report.courses[0].grade, "A");
        assert_eq!(report.courses[0].grade_point, Some(4.0));
        assert_eq!(report.courses[0].semester, "2026-2027-1");
    }
    #[test]
    fn backend_repair_run113847_grade_multirow_header_expands_spans_before_binding_columns() {
        let header = "<tr><th rowspan='2'>课程号</th><th rowspan='2'>课程名称</th><th rowspan='2'>学分</th><th rowspan='2'>考核方式</th><th colspan='2'>考核结果</th><th rowspan='2'>学期</th></tr><tr><th>成绩</th><th>绩点</th></tr>";
        let report = RegistrarGradesProfile::graduate()
            .parse_html(&table(header, ROW))
            .expect("unambiguous expanded semantic header");
        assert_eq!(report.courses.len(), 1);
        assert_eq!(report.courses[0].credit, 3.0);
        assert_eq!(report.courses[0].grade, "A");
    }
    #[test]
    fn backend_repair_run113847_grade_column_binding_tolerates_leading_and_reordered_context() {
        let header = "<tr><th>序号</th><th>课程名称</th><th>课程号</th><th>课程学分</th><th>考核方式</th><th>备注</th><th>成绩</th><th>学期</th><th>绩点</th></tr>";
        let row = "<tr><td>1</td><td>Fixture</td><td>C001</td><td>3</td><td>考试</td><td>context</td><td>A</td><td>2026-2027-1</td><td>4.0</td></tr>";
        let report = RegistrarGradesProfile::graduate()
            .parse_html(&table(header, row))
            .unwrap();
        assert_eq!(report.courses[0].course_name, "Fixture");
        assert_eq!(report.courses[0].grade_point, Some(4.0));
        assert_eq!(report.courses[0].semester, "2026-2027-1");
    }
    #[test]
    fn backend_repair_run113847_grade_whitespace_labels_optional_closures_and_repeated_headers() {
        let header = HEADER
            .replace("课程号", "课 程 号")
            .replace("学分", "学&nbsp;分");
        let row = ROW.replace("</td>", "");
        let html = table(&header, &format!("{row}{header}{row}"));
        let report = RegistrarGradesProfile::graduate()
            .parse_html(&html)
            .unwrap();
        assert_eq!(report.courses.len(), 2);
    }
    #[test]
    fn backend_repair_run113847_grade_duplicate_or_missing_required_fields_are_rejected() {
        for header in [
            HEADER.replace("绩点", "成绩"),
            HEADER.replace("学期", "年份"),
            HEADER.replace("课程号", "身份编号"),
        ] {
            assert!(
                RegistrarGradesProfile::graduate()
                    .parse_html(&table(&header, ROW))
                    .is_err()
            );
        }
    }
    #[test]
    fn backend_repair_run113847_grade_short_rows_and_cross_field_colspans_do_not_fill_values() {
        let short = ROW.replace("<td>4.0</td>", "");
        let merged = ROW.replace("<td>A</td><td>4.0</td>", "<td colspan='2'>A</td>");
        for row in [short, merged] {
            assert!(
                RegistrarGradesProfile::graduate()
                    .parse_html(&table(HEADER, &row))
                    .is_err()
            );
        }
    }
    #[test]
    fn backend_repair_run113847_grade_explicit_empty_and_footer_are_not_bad_course_rows() {
        let empty = "<tr><td colspan='7'>暂无成绩记录</td></tr>";
        assert!(
            RegistrarGradesProfile::graduate()
                .parse_html(&table(HEADER, empty))
                .unwrap()
                .courses
                .is_empty()
        );
        assert!(
            RegistrarGradesProfile::graduate()
                .parse_html(&table(HEADER, &format!("{empty}{ROW}")))
                .is_err()
        );
        let summary = "<tfoot><tr><td colspan='2'>合计</td><td>3</td><td></td><td></td><td></td><td></td></tr></tfoot>";
        let result = RegistrarGradesProfile::graduate()
            .parse_html(&table(HEADER, &format!("{ROW}{summary}")))
            .unwrap();
        assert_eq!(result.courses.len(), 1);
        assert!(
            RegistrarGradesProfile::graduate()
                .parse_html(&table(HEADER, summary))
                .is_err()
        );
    }
    #[test]
    fn backend_repair_run113847_grade_rowspans_are_bounded_and_not_header_values() {
        let one = ROW.replace("<td>2026-2027-1</td>", "<td rowspan='2'>2026-2027-1</td>");
        let two = ROW
            .replace("<td>2026-2027-1</td>", "")
            .replace("C001", "C002");
        let result = RegistrarGradesProfile::graduate()
            .parse_html(&table(HEADER, &format!("{one}{two}")))
            .unwrap();
        assert_eq!(result.courses.len(), 2);
        assert_eq!(result.courses[1].semester, "2026-2027-1");
        for bad in ["0", "-1", "99999999", "invalid"] {
            let invalid =
                HEADER.replace("<th>学期</th>", &format!("<th rowspan='{bad}'>学期</th>"));
            assert!(
                RegistrarGradesProfile::graduate()
                    .parse_html(&table(&invalid, ROW))
                    .is_err()
            );
        }
    }
    #[test]
    fn backend_repair_run113847_grade_unrecognized_data_before_header_cannot_be_dropped() {
        assert!(
            RegistrarGradesProfile::graduate()
                .parse_html(&table(ROW, &format!("{HEADER}{ROW}")))
                .is_err()
        );
        let unknown = "<tr><td colspan='7'>unexpected record contents</td></tr>";
        assert!(
            RegistrarGradesProfile::graduate()
                .parse_html(&table(HEADER, &format!("{ROW}{unknown}")))
                .is_err()
        );
    }
    #[test]
    fn backend_repair_run113847_grade_valid_report_does_not_hide_another_malformed_report() {
        let valid = table(HEADER, ROW);
        let invalid = table(HEADER, &ROW.replace("<td>4.0</td>", ""));
        assert!(
            RegistrarGradesProfile::graduate()
                .parse_html(&format!("{valid}{invalid}"))
                .is_err()
        );
        assert!(
            RegistrarGradesProfile::graduate()
                .parse_html(&format!("{valid}{valid}"))
                .is_err()
        );
    }
    #[test]
    fn backend_repair_run113847_grade_context_column_matrix_preserves_semantic_values() {
        for extra in 0..12 {
            let mut headers = vec![
                "课程号".to_owned(),
                "课程名称".into(),
                "学分".into(),
                "考核方式".into(),
                "成绩".into(),
                "绩点".into(),
                "学期".into(),
            ];
            let mut values = vec![
                "C001".to_owned(),
                "Fixture".into(),
                "3".into(),
                "考试".into(),
                "A".into(),
                "4.0".into(),
                "2026-2027-1".into(),
            ];
            for n in 0..extra {
                let index = (n * 3) % (headers.len() + 1);
                headers.insert(index, format!("附加信息{n}"));
                values.insert(index, format!("context{n}"));
            }
            let tr = |cells: &[String]| {
                format!(
                    "<tr>{}</tr>",
                    cells
                        .iter()
                        .map(|s| format!("<td>{s}</td>"))
                        .collect::<String>()
                )
            };
            let caption = "<tr><th colspan='24'>研究生成绩单</th></tr>";
            let result = RegistrarGradesProfile::graduate()
                .parse_html(&table(&format!("{caption}{}", tr(&headers)), &tr(&values)))
                .unwrap();
            assert_eq!(result.courses.len(), 1);
            assert_eq!(result.courses[0].course_name, "Fixture");
            assert_eq!(result.courses[0].credit, 3.0);
            assert_eq!(result.courses[0].grade_point, Some(4.0));
        }
    }
    #[test]
    fn backend_repair_run113847_grade_shape_diagnostics_exclude_business_content() {
        let root = std::env::temp_dir().join(format!("grade-log-{}", uuid::Uuid::new_v4()));
        let mut session = crate::telemetry::LogSession::start(
            &root,
            crate::telemetry::LogConfig::parse("debug", false).unwrap(),
        )
        .unwrap();
        let html = table(
            HEADER,
            &ROW.replace("Fixture", "synthetic-private-business-value")
                .replace("<td>4.0</td>", ""),
        );
        tracing::dispatcher::with_default(&session.dispatch, || {
            assert!(
                RegistrarGradesProfile::graduate()
                    .parse_html(&html)
                    .is_err()
            );
        });
        session.flush();
        let mut fields = Vec::new();
        let mut output = String::new();
        for entry in std::fs::read_dir(&session.directory)
            .unwrap()
            .filter_map(Result::ok)
        {
            if entry.file_name().to_string_lossy().starts_with("events.") {
                let text = std::fs::read_to_string(entry.path()).unwrap();
                for line in text.lines() {
                    if let Ok(value) = serde_json::from_str::<serde_json::Value>(line) {
                        fields.push(value["fields"].clone());
                    }
                }
                output.push_str(&text);
            }
        }
        assert!(!output.contains("synthetic-private-business-value"));
        assert!(!output.contains("C001"));
        assert!(!output.contains("2026-2027-1"));
        assert!(
            fields
                .iter()
                .any(|f| f["event"] == "registrar_grade_structure"
                    && f["actual_columns"] == 6
                    && f["expected_columns"] == 7)
        );
        drop(session);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn backend_repair_run113847_grade_expansion_limits_precede_large_text_cloning() {
        let large = "x".repeat(8193);
        let row = ROW.replace("Fixture", &large);
        assert!(matches!(
            RegistrarGradesProfile::graduate().parse_html(&table(HEADER, &row)),
            Err(RegistrarGradesParseError::MalformedHtml { .. })
        ));
        let huge_span = HEADER.replace("<th>课程号</th>", "<th colspan='65'>课程号</th>");
        assert!(
            RegistrarGradesProfile::graduate()
                .parse_html(&table(&huge_span, ROW))
                .is_err()
        );
        assert!(
            RegistrarGradesProfile::graduate()
                .parse_html(&" ".repeat(8 * 1024 * 1024 + 1))
                .is_err()
        );
    }
    #[test]
    fn backend_repair_run113847_grade_valid_table_cannot_hide_malformed_spanned_report() {
        let valid = table(HEADER, ROW);
        let malformed = table(
            &HEADER.replace("<th>学期</th>", "<th rowspan='0'>学期</th>"),
            ROW,
        );
        assert!(
            RegistrarGradesProfile::graduate()
                .parse_html(&format!("{valid}{malformed}"))
                .is_err()
        );
        let duplicate = table(&HEADER.replace("绩点", "成绩"), ROW);
        assert!(
            RegistrarGradesProfile::graduate()
                .parse_html(&format!("{valid}{duplicate}"))
                .is_err()
        );
    }
    #[test]
    fn backend_repair_run113847_grade_data_cannot_complete_incomplete_header() {
        let incomplete = HEADER.replace("<th>学期</th>", "<th>未知字段</th>");
        let misleading = ROW.replace("2026-2027-1", "学期");
        for data in [misleading.clone(), format!("{misleading}{ROW}")] {
            assert!(
                RegistrarGradesProfile::graduate()
                    .parse_html(&table(&incomplete, &data))
                    .is_err(),
                "a course row must not be consumed as the missing header field"
            );
        }
    }

    #[test]
    fn backend_repair_run113847_header_range_keeps_merged_group_records() {
        let header = "<tr><td rowspan='2'>课程号</td><td rowspan='2'>课程名称</td><td rowspan='2'>学分</td><td rowspan='2'>考核方式</td><td colspan='2'>考核结果</td><td rowspan='2'>学期</td></tr><tr><td>成绩</td><td>绩点</td></tr>";
        for extra in 0..4 {
            let columns = (0..extra)
                .map(|i| format!("<th rowspan='2'>附加栏目{i}</th>"))
                .collect::<String>();
            let cells = (0..extra)
                .map(|i| format!("<td>附加内容{i}</td>"))
                .collect::<String>();
            let h = header.replacen("<tr>", &format!("<tr>{columns}"), 1);
            let one = ROW.replacen("<tr>", &format!("<tr>{cells}"), 1);
            let two = one.replace("C001", "C002");
            let body = format!(
                "<tr><th colspan='{}'>研究生成绩单</th></tr>{h}{one}{two}",
                7 + extra
            );
            let report = RegistrarGradesProfile::graduate()
                .parse_html(&table("", &body))
                .unwrap();
            assert_eq!(report.courses.len(), 2);
            assert!(report.courses.iter().all(|course| course.credit == 3.0
                && course.grade == "A"
                && course.grade_point == Some(4.0)));
        }
    }
}

#[cfg(test)]
mod run123904_tests {
    use super::*;
    fn report(term: &str, extra: &str) -> String {
        format!(
            "<table id='table1' cellspacing='1'><tr><th>课程号</th><th>课程名称</th><th>学分</th><th>考核方式</th><th>成绩</th><th>绩点</th><th>{term}</th>{extra}</tr><tr><td>C001</td><td>Synthetic course</td><td>3</td><td>考试</td><td>A</td><td>4.0</td><td>2026-2027-1</td><td>已审核</td></tr></table>"
        )
    }
    #[test]
    fn backend_repair_run123904_eight_column_grade_accepts_explicit_term_aliases() {
        for term in [
            "学年学期",
            "学年度学期",
            "选课学期",
            "课程学期",
            "开课学年学期",
            "选课学年学期",
            "修读学年学期",
            "修课学年学期",
            "学 年 学 期",
        ] {
            let parsed = RegistrarGradesProfile::graduate()
                .parse_html(&report(term, "<th>备注</th>"))
                .unwrap();
            assert_eq!(parsed.courses.len(), 1);
            assert_eq!(parsed.courses[0].semester, "2026-2027-1");
            assert_eq!(parsed.courses[0].grade, "A");
            assert_eq!(parsed.courses[0].credit, 3.0);
        }
    }
    #[test]
    fn backend_repair_run123904_term_aliases_cannot_turn_year_or_date_into_semester() {
        for term in ["学年", "年度", "开课时间", "日期", "未确认学期字段"] {
            assert!(
                RegistrarGradesProfile::graduate()
                    .parse_html(&report(term, "<th>备注</th>"))
                    .is_err()
            );
        }
        assert!(
            RegistrarGradesProfile::graduate()
                .parse_html(&report("学年学期", "<th>选课学期</th>"))
                .is_err()
        );
    }
    #[test]
    fn backend_repair_run123904_term_header_separators_do_not_change_semantics() {
        for label in [
            "学年、学期",
            "学年 / 学期",
            "学年度，学期",
            "开课学年、学期",
            "选课学年度/学期",
            "修读学年／学期",
        ] {
            let parsed = RegistrarGradesProfile::graduate()
                .parse_html(&report(label, "<th>备注</th>"))
                .unwrap();
            assert_eq!(parsed.courses[0].semester, "2026-2027-1");
        }
        for label in ["学年/日期", "修读年度", "选课日期"] {
            assert!(
                RegistrarGradesProfile::graduate()
                    .parse_html(&report(label, "<th>备注</th>"))
                    .is_err()
            );
        }
    }
}

#[cfg(test)]
mod run132925_tests {
    use super::*;

    #[test]
    fn backend_repair_run132925_reference_graduate_legacy_semester_position_is_bound() {
        let labels = [
            "课程号",
            "课程名称",
            "学分",
            "课程类别",
            "成绩",
            "绩点",
            "学段",
            "备注",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
        let layout = grade_grid::test_reference_graduate_layout(&labels)
            .expect("the audited graduate cell positions remain supported");
        assert_eq!(layout.course_code, 0);
        assert_eq!(layout.course_name, 1);
        assert_eq!(layout.credit, 2);
        assert_eq!(layout.grade, 4);
        assert_eq!(layout.grade_point, 5);
        assert_eq!(layout.semester, 6);
        assert_eq!(layout.width, 8);
    }

    #[test]
    fn backend_repair_run132925_reference_graduate_fallback_does_not_accept_shifted_roles() {
        let header = "<tr><th>课程号</th><th>课程名称</th><th>学分</th><th>课程类别</th><th>绩点</th><th>成绩</th><th>学段</th><th>备注</th></tr>";
        let row = "<tr><td>C001</td><td>Reference course</td><td>3</td><td>学位</td><td>4.0</td><td>A</td><td>2026-2027-1</td><td>已审核</td></tr>";
        let report = format!("<table id='table1' cellspacing='1'>{header}{row}</table>");
        assert!(
            RegistrarGradesProfile::graduate()
                .parse_html(&report)
                .is_err()
        );
    }

    #[test]
    fn backend_repair_run132925_reference_fallback_rejects_nonempty_unknown_semester_header() {
        for semester_header in ["学段", "开课时间", "日期", "未确认学期字段"] {
            let header = format!(
                "<tr><th>课程号</th><th>课程名称</th><th>学分</th><th>课程类别</th><th>成绩</th><th>绩点</th><th>{semester_header}</th><th>备注</th></tr>"
            );
            let row = "<tr><td>C001</td><td>Reference course</td><td>3</td><td>学位</td><td>A</td><td>4.0</td><td>2026-2027-1</td><td>已审核</td></tr>";
            let report = format!("<table id='table1' cellspacing='1'>{header}{row}</table>");
            assert!(
                RegistrarGradesProfile::graduate()
                    .parse_html(&report)
                    .is_err(),
                "an arbitrary non-empty semester label must not activate the positional fallback"
            );
        }
    }
}

#[cfg(test)]
mod run143518_tests {
    use super::*;

    #[test]
    fn backend_repair_run143518_grade_point_na_marker_is_an_explicit_missing_value() {
        let header = "<tr><th>课程号</th><th>课程名称</th><th>学分</th><th>考核方式</th><th>成绩</th><th>绩点</th><th>学期</th></tr>";
        for marker in ["N/A", "na", "N.A.", "—"] {
            let row = format!(
                "<tr><td>C001</td><td>Fixture</td><td>3</td><td>考试</td><td>A</td><td>{marker}</td><td>2026-2027-1</td></tr>"
            );
            let report = format!("<table id='table1' cellspacing='1'>{header}{row}</table>");
            let parsed = RegistrarGradesProfile::graduate()
                .parse_html(&report)
                .expect("explicit missing grade-point marker");
            assert_eq!(parsed.courses[0].grade_point, None);
        }
    }
}
