//! Current/following term dates from Learn's already proven session.
//! The server's calendar data stays in Rust until it is normalized to dates.

use super::*;
use crate::learn_client::{LearnClientError, LearnCurrentSemester};
use chrono::Datelike;

pub(super) async fn read(runtime: &mut CampusRuntime) -> Result<LearnTermCalendarDto, String> {
    runtime.allow_live_operation()?;
    let user = runtime.ensure_identity_user_for_live_read().await?;
    runtime.apply_reference_academic_stage(&user);
    if !runtime.service_session_is_proven(ServiceId::Learn) {
        runtime
            .ensure_academic_reader_session(ServiceId::Learn)
            .await?;
    }
    if !runtime.service_session_is_proven(ServiceId::Identity)
        || !runtime.service_session_is_proven(ServiceId::Learn)
        || runtime
            .coordinator
            .registry()
            .snapshot_for(ServiceId::Learn)
            .user
            .as_ref()
            != Some(&user)
    {
        return runtime.fail("学习平台服务会话未建立，请重新建立");
    }
    let source = runtime
        .learn_source
        .clone()
        .ok_or("学习平台服务会话未建立，请重新建立")?;
    if !Arc::ptr_eq(
        source.registrar().transport().cookie_jar(),
        runtime.identity.transport().cookie_jar(),
    ) {
        return runtime.fail("网络学堂账号会话未确认，请重新登录");
    }
    let csrf = runtime
        .coordinator
        .bound_csrf(ServiceId::Learn)
        .ok_or("学习平台 CSRF 未确认")?
        .clone();
    let calendar = match source
        .learn()
        .fetch_term_calendar_with_bound_csrf(source.registrar().transport(), &csrf)
        .await
    {
        Ok(calendar) => calendar,
        Err(LearnClientError::SessionExpired) => {
            runtime.invalidate_learn_session();
            return runtime.fail("学习平台服务会话已过期，请重新建立");
        }
        Err(error) => {
            return Err(runtime.record_business_failure(
                "learn",
                "learn_term_calendar",
                learn_failure_code(&error),
            ));
        }
    };
    let current = normalize_term(&calendar.current).ok_or_else(|| {
        runtime.record_business_failure("learn", "learn_term_calendar", "learn_response_format")
    })?;
    let upcoming = calendar
        .upcoming
        .iter()
        .map(normalize_term)
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| {
            runtime.record_business_failure("learn", "learn_term_calendar", "learn_response_format")
        })?;
    runtime.allow_live_operation()?;
    if !runtime.service_session_is_proven(ServiceId::Identity)
        || !runtime.service_session_is_proven(ServiceId::Learn)
        || runtime
            .coordinator
            .registry()
            .snapshot_for(ServiceId::Learn)
            .user
            .as_ref()
            != Some(&user)
    {
        return runtime.fail("网络学堂账号会话未确认，请重新登录");
    }
    runtime.last_error = None;
    runtime.persist_resume_state_after_live_read(&user, "learn");
    Ok(LearnTermCalendarDto {
        current,
        upcoming,
        generated_at: Utc::now().to_rfc3339(),
        source: "live".into(),
        status: "ready".into(),
    })
}

fn normalize_term(term: &LearnCurrentSemester) -> Option<LearnTermDto> {
    let start = parse_semester_boundary_date(&term.start_date)?;
    let end = parse_semester_boundary_date(&term.end_date)?;
    if start > end || (end - start).num_days() > 370 {
        return None;
    }
    // The reference calendar starts teaching weeks on Monday. A semester
    // beginning on a weekend starts with the following Monday.
    let weekday = start.weekday().num_days_from_monday();
    let delta = match weekday {
        5 => 2,
        6 => 1,
        other => -i64::from(other),
    };
    let first_day = start.checked_add_signed(chrono::Duration::days(delta))?;
    if first_day > end {
        return None;
    }
    let week_count = u32::try_from((end - first_day).num_days().div_euclid(7) + 1).ok()?;
    if !(1..=80).contains(&week_count) {
        return None;
    }
    Some(LearnTermDto {
        id: term.id.clone(),
        label: term.label.clone(),
        start_date: start.to_string(),
        end_date: end.to_string(),
        first_day: first_day.to_string(),
        week_count,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_repair_learn_term_calendar_aligns_teaching_weeks_and_rejects_bad_range() {
        let term = |start: &str, end: &str| LearnCurrentSemester {
            id: "2026-2027-1".into(),
            label: "秋季学期".into(),
            start_date: start.into(),
            end_date: end.into(),
        };
        assert_eq!(
            normalize_term(&term("2026-09-02", "2027-01-15"))
                .unwrap()
                .first_day,
            "2026-08-31"
        );
        assert_eq!(
            normalize_term(&term("2026-09-05", "2027-01-15"))
                .unwrap()
                .first_day,
            "2026-09-07"
        );
        assert!(normalize_term(&term("2027-01-15", "2026-09-02")).is_none());
    }
}
