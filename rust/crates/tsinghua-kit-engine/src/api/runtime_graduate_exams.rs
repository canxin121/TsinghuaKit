//! Reference: thu-info-lib schedule.ts getPrimary/getSchedule, JXRL_YJS_PREFIX
//! and models/schedule/schedule.ts parseJSON. Only the server's category is
//! evidence of an exam; no undergraduate endpoint or title heuristic is used.
use super::*;
use crate::ServiceError;
use crate::registrar_client::RegistrarEvent;
use chrono::{Datelike, NaiveTime, Timelike};

pub(super) fn cache_service(stage: AcademicStage) -> &'static str {
    match stage {
        AcademicStage::Undergraduate => REGISTRAR_EXAMS_CACHE_SERVICE,
        AcademicStage::Graduate => "registrar-graduate-calendar-exams-v1",
    }
}

fn exam_category(category: &str) -> bool {
    matches!(
        category.trim().to_ascii_lowercase().as_str(),
        "考试" | "期中考试" | "期末考试" | "补考" | "研究生考试" | "exam" | "examination"
    )
}

fn invalid() -> ServiceError {
    ServiceError::Adapter {
        message: "研究生考试教学日历的结构或学期范围未确认".into(),
    }
}

pub(super) async fn read_report(
    source: &CampusLiveDataSource,
    stage: AcademicStage,
    window: Option<(NaiveDate, NaiveDate)>,
    semester: &str,
) -> Result<CampusExamReportDto, ServiceError> {
    if source.config().academic_stage != stage {
        return Err(invalid());
    }
    if stage == AcademicStage::Undergraduate {
        return source.list_undergraduate_exams().await.map(map_exam_report);
    }
    if source.config().semester != semester {
        return Err(invalid());
    }
    let (first, last) = window
        .filter(|(a, b)| a <= b && (*b - *a).num_days() <= 370)
        .ok_or_else(invalid)?;
    let range = campus_date_range_between(first, last).map_err(|_| invalid())?;
    let events = source.list_graduate_calendar_events(range).await?;
    let mut rows = Vec::new();
    let mut seen = HashSet::new();
    for event in events {
        // A missing classification cannot prove either an exam or its absence.
        let category = event
            .category
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(invalid)?;
        if !exam_category(category) {
            continue;
        }
        let row = map_event(event)?;
        if seen.insert((
            row.course_name.clone(),
            row.course_code.clone(),
            row.schedule_raw.clone(),
            row.location.clone(),
        )) {
            rows.push(row);
        }
    }
    rows.sort_by(|a, b| {
        (&a.schedule_raw, &a.course_name, &a.location).cmp(&(
            &b.schedule_raw,
            &b.course_name,
            &b.location,
        ))
    });
    Ok(CampusExamReportDto {
        stage: "graduate".into(),
        exam_count: rows.len().try_into().map_err(|_| invalid())?,
        exams: rows,
        generated_at: None,
        source: None,
        status: None,
        error: None,
    })
}

fn weekday(date: NaiveDate) -> &'static str {
    match date.weekday() {
        chrono::Weekday::Mon => "monday",
        chrono::Weekday::Tue => "tuesday",
        chrono::Weekday::Wed => "wednesday",
        chrono::Weekday::Thu => "thursday",
        chrono::Weekday::Fri => "friday",
        chrono::Weekday::Sat => "saturday",
        chrono::Weekday::Sun => "sunday",
    }
}

fn map_event(event: RegistrarEvent) -> Result<CampusExamDto, ServiceError> {
    if event.ends_at <= event.starts_at
        || event.ends_at.date() != event.starts_at.date()
        || event.starts_at.second() != 0
        || event.ends_at.second() != 0
        || event.starts_at.nanosecond() != 0
        || event.ends_at.nanosecond() != 0
    {
        return Err(invalid());
    }
    let date = event.starts_at.date();
    let session = format!(
        "{}–{}",
        event.starts_at.format("%H:%M"),
        event.ends_at.format("%H:%M")
    );
    let row = CampusExamDto {
        course_code: event.course_code.unwrap_or_default(),
        course_sequence: String::new(),
        course_name: event.title,
        exam_month: date.month() as u8,
        exam_day: date.day() as u8,
        exam_weekday: weekday(date).into(),
        schedule_raw: format!("{date} {session}"),
        exam_session: session,
        location: event.location.unwrap_or_default(),
        department: None,
        category: event.category,
        instructor: event.instructor,
        headcount: None,
    };
    if !valid_row(&row, date, date) {
        return Err(invalid());
    }
    Ok(row)
}

pub(super) fn cache_rows_are_valid(payload: &RegistrarExamsCachePayload) -> bool {
    let Some((first, last)) = payload.calendar_window else {
        return false;
    };
    if first > last || (last - first).num_days() > 370 {
        return false;
    }
    let mut seen = HashSet::new();
    payload.exams.iter().all(|row| {
        valid_row(row, first, last)
            && seen.insert((
                &row.course_name,
                &row.course_code,
                &row.schedule_raw,
                &row.location,
            ))
    })
}

fn valid_row(row: &CampusExamDto, first: NaiveDate, last: NaiveDate) -> bool {
    let Some((date_text, session)) = row.schedule_raw.split_once(' ') else {
        return false;
    };
    let Ok(date) = NaiveDate::parse_from_str(date_text, "%Y-%m-%d") else {
        return false;
    };
    let Some((start, end)) = session.split_once('–') else {
        return false;
    };
    let (Ok(start), Ok(end)) = (
        NaiveTime::parse_from_str(start, "%H:%M"),
        NaiveTime::parse_from_str(end, "%H:%M"),
    ) else {
        return false;
    };
    date >= first
        && date <= last
        && end > start
        && date.format("%Y-%m-%d").to_string() == date_text
        && format!("{}–{}", start.format("%H:%M"), end.format("%H:%M")) == session
        && row.exam_session == session
        && row.exam_weekday == weekday(date)
        && row.exam_month as u32 == date.month()
        && row.exam_day as u32 == date.day()
        && cache_context_text_is_safe(&row.course_name, false)
        && cache_context_text_is_safe(&row.course_code, true)
        && row.course_sequence.is_empty()
        && cache_context_text_is_safe(&row.location, true)
        && row.category.as_deref().is_some_and(exam_category)
        && row
            .instructor
            .as_deref()
            .is_none_or(|s| cache_context_text_is_safe(s, false))
        && row.department.is_none()
        && row.headcount.is_none()
}
