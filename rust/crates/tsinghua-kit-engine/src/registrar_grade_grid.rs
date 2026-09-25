//! Structural grade-table reader. Column roles come only from a unique,
//! allowlisted semantic header inside the known report table, never from
//! guessed positions, grades or numeric-looking business values.
use super::{GradeColumnLayout, RegistrarGradesParseError as Error, normalize_text};
use crate::protocol::AcademicStage;
use scraper::{ElementRef, Html, Selector};
use sha1::{Digest, Sha1};

const MAX_ROWS: usize = 8192;
const MAX_COLUMNS: usize = 64;
const MAX_CELLS: usize = 262144;
const MAX_CELL_TEXT: usize = 8192;
const MAX_EXPANDED_TEXT: usize = 16 * 1024 * 1024;
const MAX_HEADER_ROWS: usize = 4;
const MAX_HEADER_PREFIX: usize = 16;

#[derive(Clone)]
struct Cell {
    text: String,
    origin: (usize, usize),
    colspan: usize,
}
struct Row {
    cells: Vec<Cell>,
    footer: bool,
}
#[derive(Clone)]
struct Pending {
    cell: Cell,
    remaining: usize,
}

pub(super) struct VerifiedTable {
    pub layout: GradeColumnLayout,
    pub rows: Vec<(usize, Vec<String>)>,
}
pub(super) struct TableFailure {
    pub error: Error,
    pub confidence: u32,
}
fn malformed(context: &'static str) -> Error {
    Error::MalformedHtml { context }
}
fn token(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_whitespace() && *c != '\u{a0}')
        .collect()
}

fn visible_text(element: ElementRef<'_>) -> String {
    let raw = element
        .descendants()
        .filter_map(|node| {
            let text = node.value().as_text()?;
            let hidden = node.ancestors().filter_map(ElementRef::wrap).any(|parent| {
                matches!(
                    parent.value().name(),
                    "script" | "style" | "template" | "noscript"
                ) || parent.value().attr("hidden").is_some()
            });
            (!hidden).then(|| text.to_string())
        })
        .collect::<Vec<_>>()
        .join(" ");
    normalize_text(&raw)
}
fn span(cell: ElementRef<'_>, attr: &str, max: usize) -> Result<usize, Error> {
    let Some(raw) = cell.value().attr(attr) else {
        return Ok(1);
    };
    let raw = raw.trim();
    if raw.is_empty() || raw.len() > 5 || !raw.bytes().all(|b| b.is_ascii_digit()) {
        return Err(malformed("invalid grade cell span"));
    }
    raw.parse::<usize>()
        .ok()
        .filter(|n| *n > 0 && *n <= max)
        .ok_or_else(|| malformed("invalid grade cell span"))
}
fn grid(body: &str) -> Result<Vec<Row>, Error> {
    let document = Html::parse_fragment(&format!("<table>{body}</table>"));
    let table_selector = Selector::parse("table").expect("fixed selector");
    let row_selector = Selector::parse("tr").expect("fixed selector");
    let table = document
        .select(&table_selector)
        .next()
        .ok_or_else(|| malformed("missing grade table container"))?;
    let mut rows = Vec::new();
    let mut pending: Vec<Option<Pending>> = vec![None; MAX_COLUMNS];
    let mut cell_budget = 0usize;
    let mut text_budget = 0usize;
    for tr in table.select(&row_selector).filter(|row| {
        row.ancestors()
            .filter_map(ElementRef::wrap)
            .find(|p| p.value().name() == "table")
            .is_some_and(|p| p.id() == table.id())
    }) {
        if rows.len() >= MAX_ROWS {
            return Err(malformed("grade table size limit"));
        }
        let index = rows.len();
        let footer = tr
            .ancestors()
            .filter_map(ElementRef::wrap)
            .any(|p| p.value().name() == "tfoot");
        let mut slots: Vec<Option<Cell>> = vec![None; MAX_COLUMNS];
        for column in 0..MAX_COLUMNS {
            if let Some(p) = pending[column].as_mut() {
                text_budget += p.cell.text.len();
                if text_budget > MAX_EXPANDED_TEXT {
                    return Err(malformed("grade table size limit"));
                }
                slots[column] = Some(p.cell.clone());
                p.remaining -= 1;
                if p.remaining == 0 {
                    pending[column] = None;
                }
            }
        }
        let mut cursor = 0;
        for (raw_index, cell) in tr
            .children()
            .filter_map(ElementRef::wrap)
            .filter(|c| matches!(c.value().name(), "td" | "th"))
            .enumerate()
        {
            while cursor < MAX_COLUMNS && slots[cursor].is_some() {
                cursor += 1;
            }
            let colspan = span(cell, "colspan", MAX_COLUMNS)?;
            let rowspan = span(cell, "rowspan", MAX_ROWS)?;
            if cursor + colspan > MAX_COLUMNS {
                return Err(malformed("grade table size limit"));
            }
            let text = visible_text(cell);
            if text.len() > MAX_CELL_TEXT {
                return Err(malformed("grade table size limit"));
            }
            text_budget += text.len() * colspan;
            if text_budget > MAX_EXPANDED_TEXT {
                return Err(malformed("grade table size limit"));
            }
            let value = Cell {
                text,
                origin: (index, raw_index),
                colspan,
            };
            for column in cursor..cursor + colspan {
                if slots[column].is_some() {
                    return Err(malformed("overlapping grade cell spans"));
                }
                slots[column] = Some(value.clone());
                if rowspan > 1 {
                    pending[column] = Some(Pending {
                        cell: value.clone(),
                        remaining: rowspan - 1,
                    });
                }
            }
            cursor += colspan;
        }
        let width = slots.iter().rposition(Option::is_some).map_or(0, |n| n + 1);
        cell_budget += width;
        if cell_budget > MAX_CELLS {
            return Err(malformed("grade table size limit"));
        }
        let cells = slots
            .into_iter()
            .take(width)
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| malformed("gaps in grade cell spans"))?;
        rows.push(Row { cells, footer });
    }
    if pending.iter().any(Option::is_some) {
        return Err(malformed("unterminated grade cell rowspan"));
    }
    Ok(rows)
}

// Keep labels finite and source-owned. Unknown context columns are ignored,
// but can never fill a missing required role or change a selected value.
fn role(value: &str) -> Option<usize> {
    if term_header_kind(value) != "unrecognized" {
        return Some(5);
    }
    match token(value).as_str() {
        "课程号" | "课程编号" | "课程代码" => Some(0),
        "课程名称" | "课程名" | "中文课程名称" => Some(1),
        "学分" | "课程学分" => Some(2),
        "成绩" | "总评成绩" | "课程成绩" | "总成绩" => Some(3),
        "绩点" | "成绩绩点" | "课程绩点" => Some(4),
        "学期" | "开课学期" | "修读学期" | "修课学期" | "学年学期" | "学年度学期" | "选课学期"
        | "课程学期" | "开课学年学期" | "选课学年学期" | "修读学年学期" | "修课学年学期" => {
            Some(5)
        }
        _ => None,
    }
}
fn term_header_kind(label: &str) -> &'static str {
    // Only punctuation separating YEAR and TERM is normalized here; a year
    // or date column alone never becomes a semester, and values are untouched.
    let label = token(label).replace(['、', '，', ',', '/', '／'], "");
    match label.as_str() {
        "学期" | "开课学期" | "修读学期" | "修课学期" => "standard_term",
        "学年学期" | "学年度学期" => "academic_year_term",
        "选课学期" | "选课学年学期" | "选课学年度学期" => "selection_term",
        "课程学期" => "course_term",
        "开课学年学期"
        | "修读学年学期"
        | "修课学年学期"
        | "开课学年度学期"
        | "修读学年度学期"
        | "修课学年度学期" => "study_year_term",
        _ => "unrecognized",
    }
}

// Keep live diagnostics source-owned and content-free.  This distinguishes a
// legacy year/term label from an unrelated trailing context column without
// writing the actual upstream header text to the log.
fn term_header_shape(label: &str) -> &'static str {
    let normalized = token(label).replace(['、', '，', ',', '/', '／'], "");
    match term_header_kind(label) {
        "unrecognized" if normalized == "学年" || normalized == "年度" => "year_only",
        "unrecognized" if normalized.contains("学期") => "term_with_context",
        "unrecognized" if normalized.contains("学年") || normalized.contains("年度") => {
            "year_with_context"
        }
        "unrecognized" => "other",
        "standard_term" => "standard_term",
        "academic_year_term" => "academic_year_term",
        "selection_term" => "selection_term",
        "course_term" => "course_term",
        "study_year_term" => "study_year_term",
        _ => "other",
    }
}

fn first_term_candidate_shape(labels: &[String]) -> &'static str {
    let Some(grade_point) = labels.iter().position(|label| role(label) == Some(4)) else {
        return "none";
    };
    labels
        .iter()
        .skip(grade_point + 1)
        .map(|label| term_header_shape(label))
        .find(|shape| *shape != "other")
        .unwrap_or("other")
}

fn unknown_header_position_mask(labels: &[String]) -> u64 {
    labels
        .iter()
        .enumerate()
        .filter(|(_, label)| role(label).is_none())
        .fold(0, |bits, (column, _)| bits | (1u64 << column))
}

fn role_position_mask(labels: &[String]) -> u64 {
    labels
        .iter()
        .enumerate()
        .filter_map(|(column, label)| role(label).map(|field| (column, field)))
        .fold(0, |bits, (column, field)| {
            bits | (1u64 << (field * 8 + column))
        })
}

fn header_row_digest(row: &Row) -> String {
    let mut digest = Sha1::new();
    digest.update(b"thyou-grade-header-row-v1\0");
    for cell in &row.cells {
        digest.update(token(&cell.text).as_bytes());
        digest.update([0]);
        digest.update((cell.colspan as u64).to_le_bytes());
        digest.update([0xff]);
    }
    digest
        .finalize()
        .iter()
        .fold(String::with_capacity(40), |mut output, byte| {
            use std::fmt::Write;

            let _ = write!(output, "{byte:02x}");
            output
        })
}

fn mask(labels: &[String]) -> u64 {
    labels
        .iter()
        .filter_map(|s| role(s))
        .fold(0, |bits, r| bits | (1 << r))
}
fn schema(labels: &[String], stage: AcademicStage) -> Result<GradeColumnLayout, Error> {
    let mut found: [Option<usize>; 6] = [None; 6];
    for (column, label) in labels.iter().enumerate() {
        if let Some(r) = role(label) {
            if found[r].replace(column).is_some() {
                return Err(malformed("ambiguous grade header"));
            }
        }
    }
    let required = ["课程号", "课程名称", "学分", "成绩", "绩点", "学期"];
    for (r, name) in required.iter().enumerate() {
        if found[r].is_none() {
            return Err(Error::InvalidHeader {
                column: r,
                expected: *name,
            });
        }
    }
    // Stage comes from the configured, proven endpoint, not a numeric value.
    // Preserve the known incompatible compact graduate-only header guard.
    if stage == AcademicStage::Undergraduate && labels.iter().any(|s| token(s) == "考核方式") {
        return Err(malformed("grade header stage mismatch"));
    }
    Ok(GradeColumnLayout {
        course_code: found[0].unwrap(),
        course_name: found[1].unwrap(),
        credit: found[2].unwrap(),
        grade: found[3].unwrap(),
        grade_point: found[4].unwrap(),
        semester: found[5].unwrap(),
        width: labels.len(),
        reference_semester_position: false,
    })
}

const REFERENCE_GRADUATE_LEGACY_HEADER_DIGEST: &str = "eaf2da57ee5dfc93f3ce3907dcd1d052bba207dc";

/// The audited graduate report has one legacy deployment variant whose
/// semester header is not a semantic label, while the Reference still binds
/// the value to the seventh logical cell. Keep this compatibility branch
/// narrower than a positional guess: it is only valid for the graduate route,
/// exactly eight cells, the other five roles at their observed Reference
/// positions, and the exact observed header-row fingerprint. The remaining
/// cells are bounded context positions; an arbitrary non-empty label cannot
/// become a semester merely because a legacy page happens to have eight
/// columns.
fn reference_graduate_fallback_state(labels: &[String], stage: AcademicStage) -> &'static str {
    if stage != AcademicStage::Graduate || labels.len() != 8 {
        return "stage_or_width";
    }
    let required_positions = [(0, 0), (1, 1), (2, 2), (4, 3), (5, 4)];
    if required_positions
        .into_iter()
        .any(|(column, expected)| role(&labels[column]) != Some(expected))
    {
        return "role_positions";
    }
    if [3, 6, 7]
        .into_iter()
        .any(|column| role(&labels[column]).is_some())
    {
        return "context_role";
    }
    if labels[6].is_empty() {
        return "semester_header_shape";
    }
    // The caller checks the row fingerprint before using this schema. This
    // branch remains label-independent here so it can also classify the
    // candidate for diagnostics and data-row repeated-header detection.
    "eligible"
}

fn reference_graduate_schema(
    labels: &[String],
    stage: AcademicStage,
    reference_header_matches: bool,
) -> Option<GradeColumnLayout> {
    if !reference_header_matches || reference_graduate_fallback_state(labels, stage) != "eligible" {
        return None;
    }
    Some(GradeColumnLayout {
        course_code: 0,
        course_name: 1,
        credit: 2,
        grade: 4,
        grade_point: 5,
        semester: 6,
        width: 8,
        reference_semester_position: true,
    })
}

fn schema_with_reference_fallback(
    labels: &[String],
    stage: AcademicStage,
    reference_header_matches: bool,
) -> Result<GradeColumnLayout, Error> {
    match schema(labels, stage) {
        Ok(layout) => Ok(layout),
        Err(error) => {
            reference_graduate_schema(labels, stage, reference_header_matches).ok_or(error)
        }
    }
}

#[cfg(test)]
pub(super) fn test_reference_graduate_layout(labels: &[String]) -> Option<GradeColumnLayout> {
    reference_graduate_schema(labels, AcademicStage::Graduate, true)
}
fn columns(layout: GradeColumnLayout) -> [usize; 6] {
    [
        layout.course_code,
        layout.course_name,
        layout.credit,
        layout.grade,
        layout.grade_point,
        layout.semester,
    ]
}
/// Combining header levels may reuse labels, but cannot turn a course row
/// into the missing label of an incomplete header. Every ordinary cell in a
/// required column must itself be the matching header; merged group headings
/// are allowed because their children resolve the distinct semantic fields.
fn header_range_is_structural(
    rows: &[Row],
    first: usize,
    end: usize,
    layout: GradeColumnLayout,
) -> bool {
    rows[first..=end].iter().all(|row| {
        row.cells.len() == layout.width
            && columns(layout)
                .iter()
                .enumerate()
                .all(|(expected, &column)| {
                    let cell = &row.cells[column];
                    cell.text.is_empty()
                        || cell.colspan > 1
                        || (layout.reference_semester_position && column == layout.semester)
                        || role(&cell.text) == Some(expected)
                })
    })
}

fn combine(rows: &[Row], start: usize, end: usize) -> (Vec<String>, Vec<usize>) {
    let width = rows[end].cells.len();
    let mut labels = vec![String::new(); width];
    let mut origins = vec![end; width];
    for row in rows.iter().take(end + 1).skip(start) {
        if row.cells.len() != width {
            continue;
        }
        for (column, cell) in row.cells.iter().enumerate() {
            if role(&cell.text).is_some()
                || (role(&labels[column]).is_none() && !cell.text.is_empty())
            {
                labels[column] = cell.text.clone();
                origins[column] = cell.origin.0;
            }
        }
    }
    (labels, origins)
}
fn single_merged(row: &Row, width: usize) -> Option<&str> {
    let first = row.cells.first()?;
    (row.cells.len() == width
        && first.colspan == width
        && row.cells.iter().all(|c| c.origin == first.origin))
    .then_some(first.text.as_str())
}
fn blank(row: &Row) -> bool {
    row.cells.iter().all(|cell| cell.text.is_empty())
}
fn preamble(row: &Row, width: usize) -> bool {
    if blank(row)
        || single_merged(row, width).is_some()
        || row.cells.first().is_some_and(|first| {
            first.colspan == row.cells.len()
                && row.cells.iter().all(|cell| cell.origin == first.origin)
        })
    {
        return true;
    }
    // Explicit student/report metadata is not course data. Do not log it.
    row.cells
        .iter()
        .filter(|c| {
            ["学号", "姓名", "院系", "专业", "年级"]
                .iter()
                .any(|name| token(&c.text).starts_with(name))
        })
        .count()
        >= 2
}
fn empty_label(value: &str) -> bool {
    matches!(
        token(value).as_str(),
        "暂无数据"
            | "暂无成绩"
            | "暂无成绩记录"
            | "无成绩记录"
            | "没有成绩记录"
            | "没有符合条件的记录"
            | "未查询到成绩"
    )
}
fn summary_label(value: &str) -> bool {
    let value = token(value);
    let prefix = ["合计", "总计", "总学分", "平均绩点", "平均学分绩点"]
        .iter()
        .any(|s| value.starts_with(s));
    prefix
        && value.len() <= 256
        && value.chars().all(|c| {
            c.is_ascii_digit()
                || ".：:，,()（）/%+-".contains(c)
                || "合计总学分平均绩点课程门数".contains(c)
        })
}
fn summary(row: &Row) -> bool {
    let Some(first) = row.cells.first() else {
        return false;
    };
    if !(row.footer || first.colspan > 1) || !summary_label(&first.text) {
        return false;
    }
    row.cells.iter().all(|cell| {
        cell.text.is_empty()
            || summary_label(&cell.text)
            || token(&cell.text)
                .chars()
                .all(|c| c.is_ascii_digit() || ".：:，,()（）/%+-".contains(c))
    })
}

fn unexpanded_header_hint(body: &str) -> u32 {
    let doc = Html::parse_fragment(&format!("<table>{body}</table>"));
    let tr = Selector::parse("tr").expect("fixed selector");
    let mut bits = 0u64;
    for row in doc.select(&tr).take(MAX_HEADER_PREFIX) {
        for cell in row
            .children()
            .filter_map(ElementRef::wrap)
            .filter(|c| matches!(c.value().name(), "td" | "th"))
        {
            if let Some(field) = role(&visible_text(cell)) {
                bits |= 1 << field;
            }
        }
    }
    bits.count_ones()
}

pub(super) fn read(
    body: &str,
    stage: AcademicStage,
    table_index: usize,
) -> Result<VerifiedTable, TableFailure> {
    let rows = grid(body).map_err(|error| TableFailure {
        error,
        confidence: unexpanded_header_hint(body).max(1),
    })?;
    let mut best = (0u32, Error::MissingHeaderRow);
    let mut best_mask = 0u64;
    let mut best_term_shape = "none";
    let mut best_unknown_position_mask = 0u64;
    let mut best_role_position_mask = 0u64;
    let mut best_reference_fallback_state = "none";
    let mut best_failure_stage = "schema";
    let mut selected = None;
    for end in 0..rows.len().min(MAX_HEADER_PREFIX) {
        for start in (end.saturating_sub(MAX_HEADER_ROWS - 1)..=end).rev() {
            let (labels, origins) = combine(&rows, start, end);
            let evidence = mask(&labels);
            let confidence = evidence.count_ones();
            let reference_header_matches = (start..=end).any(|row| {
                header_row_digest(&rows[row]) == REFERENCE_GRADUATE_LEGACY_HEADER_DIGEST
            });
            match schema_with_reference_fallback(&labels, stage, reference_header_matches) {
                Ok(layout) => {
                    let first = columns(layout)
                        .iter()
                        .map(|c| origins[*c])
                        .min()
                        .unwrap_or(end);
                    if !header_range_is_structural(&rows, first, end, layout) {
                        best = (confidence, malformed("course values inside grade header"));
                        best_mask = evidence;
                        best_term_shape = first_term_candidate_shape(&labels);
                        best_unknown_position_mask = unknown_header_position_mask(&labels);
                        best_role_position_mask = role_position_mask(&labels);
                        best_reference_fallback_state =
                            reference_graduate_fallback_state(&labels, stage);
                        best_failure_stage = "header_structure";
                        continue;
                    }
                    if rows
                        .iter()
                        .take(first)
                        .any(|row| !preamble(row, layout.width))
                    {
                        best = (
                            confidence,
                            malformed("unrecognized rows before grade header"),
                        );
                        best_term_shape = first_term_candidate_shape(&labels);
                        best_unknown_position_mask = unknown_header_position_mask(&labels);
                        best_role_position_mask = role_position_mask(&labels);
                        best_reference_fallback_state =
                            reference_graduate_fallback_state(&labels, stage);
                        best_failure_stage = "preamble";
                        continue;
                    }
                    if first > start {
                        // A preceding ordinary data row cannot be swallowed into a header.
                        if rows
                            .iter()
                            .take(first)
                            .skip(start)
                            .any(|row| !preamble(row, layout.width))
                        {
                            continue;
                        }
                    }
                    tracing::debug!(target:"tsinghua_kit::api",event="registrar_grade_structure",service="registrar",business_stage="registrar_grade_header",reason="verified",header_term_kind=term_header_kind(&labels[layout.semester]));
                    selected = Some((layout, end));
                    break;
                }
                Err(error) => {
                    if confidence >= best.0 {
                        best = (confidence, error);
                        best_mask = evidence;
                        best_term_shape = first_term_candidate_shape(&labels);
                        best_unknown_position_mask = unknown_header_position_mask(&labels);
                        best_role_position_mask = role_position_mask(&labels);
                        best_reference_fallback_state =
                            reference_graduate_fallback_state(&labels, stage);
                        best_failure_stage = "schema";
                    }
                }
            }
        }
        if selected.is_some() {
            break;
        }
    }
    let Some((layout, header_end)) = selected else {
        tracing::warn!(target:"tsinghua_kit::api",event="registrar_grade_structure",service="registrar",business_stage="registrar_grade_header",reason="registrar_grade_header_unrecognized",table_index=table_index as u64,row_count=rows.len() as u64,actual_columns=rows.iter().map(|r|r.cells.len()).max().unwrap_or(0) as u64,header_fields=best.0 as u64,header_mask=best_mask,header_term_shape=best_term_shape,header_unknown_mask=best_unknown_position_mask,header_role_position_mask=best_role_position_mask,reference_fallback_state=best_reference_fallback_state,header_failure_stage=best_failure_stage);
        return Err(TableFailure {
            error: best.1,
            confidence: best.0,
        });
    };
    tracing::debug!(target:"tsinghua_kit::api",event="registrar_grade_structure",service="registrar",business_stage="registrar_grade_header",reason="verified",table_index=table_index as u64,row_count=rows.len() as u64,header_row=header_end as u64,actual_columns=layout.width as u64,header_fields=6u64);
    let mut result = Vec::new();
    let mut explicit_empty = false;
    let mut summaries = 0;
    let outcome = (|| -> Result<(), Error> {
        for (index, row) in rows.iter().enumerate().skip(header_end + 1) {
            if blank(row) {
                continue;
            }
            let labels = row.cells.iter().map(|c| c.text.clone()).collect::<Vec<_>>();
            if schema_with_reference_fallback(
                &labels,
                stage,
                header_row_digest(row) == REFERENCE_GRADUATE_LEGACY_HEADER_DIGEST,
            )
            .is_ok_and(|candidate| {
                columns(candidate) == columns(layout) && candidate.width == layout.width
            }) {
                continue;
            }
            if let Some(text) = single_merged(row, layout.width) {
                if empty_label(text) {
                    if explicit_empty || !result.is_empty() {
                        return Err(malformed("contradictory empty grade report"));
                    }
                    explicit_empty = true;
                    continue;
                }
            }
            if summary(row) {
                summaries += 1;
                continue;
            }
            if explicit_empty {
                return Err(malformed("contradictory empty grade report"));
            }
            if row.cells.len() != layout.width {
                tracing::warn!(target:"tsinghua_kit::api",event="registrar_grade_structure",service="registrar",business_stage="registrar_grade_data",reason="registrar_grade_row_columns",table_index=table_index as u64,row_index=index as u64,actual_columns=row.cells.len() as u64,expected_columns=layout.width as u64);
                return Err(Error::WrongColumnCount {
                    row: index,
                    actual: row.cells.len(),
                    expected: layout.width,
                });
            }
            let mut origins = std::collections::HashSet::new();
            for column in columns(layout) {
                let cell = &row.cells[column];
                if cell.origin.0 <= header_end || !origins.insert(cell.origin) {
                    return Err(malformed("merged grade data fields"));
                }
            }
            if row.cells[layout.course_code].text.is_empty() {
                return Err(malformed("missing grade course identifier"));
            }
            result.push((index, labels));
        }
        if summaries > 0 && result.is_empty() && !explicit_empty {
            return Err(malformed("grade summary without records"));
        }
        Ok(())
    })();
    outcome.map_err(|error| TableFailure {
        error,
        confidence: 7,
    })?;
    Ok(VerifiedTable {
        layout,
        rows: result,
    })
}
