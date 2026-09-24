//! Request profile and date-window planning for the academic registrar service.
//!
//! The registrar session is reached through a separate `ALL_ZHJW` ticket obtained
//! from the learning platform. This module only plans the stage-specific calendar
//! requests; authentication and JSONP decoding stay in their respective adapters.

use chrono::{Duration, NaiveDate};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::protocol::AcademicStage;

const CALENDAR_CHUNK_DAYS: i64 = 28;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RegistrarError {
    #[error("calendar range start must not be after end")]
    InvalidDateRange,

    #[error("calendar callback must not be empty")]
    EmptyCallback,

    #[error("calendar callback is not a JavaScript identifier path")]
    InvalidCallback,

    #[error("calendar range exceeded the representable date range")]
    DateOverflow,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalendarWindow {
    /// First date included in the registrar request.
    pub start: NaiveDate,
    /// Last date included in the registrar request.
    pub end: NaiveDate,
}

impl CalendarWindow {
    pub fn new(start: NaiveDate, end: NaiveDate) -> Result<Self, RegistrarError> {
        if start > end {
            return Err(RegistrarError::InvalidDateRange);
        }

        Ok(Self { start, end })
    }

    /// Validates a window after construction.  The fields are public for the
    /// serialized request DTO, so callers can also construct a value without
    /// going through [`Self::new`].
    pub fn validate(&self) -> Result<(), RegistrarError> {
        if self.start > self.end {
            return Err(RegistrarError::InvalidDateRange);
        }
        Ok(())
    }

    pub fn day_count(&self) -> i64 {
        (self.end - self.start).num_days() + 1
    }
}

/// Splits an inclusive date range into registrar-safe windows of at most 28 days.
pub fn split_calendar_range(
    start: NaiveDate,
    end: NaiveDate,
) -> Result<Vec<CalendarWindow>, RegistrarError> {
    if start > end {
        return Err(RegistrarError::InvalidDateRange);
    }

    let mut windows = Vec::new();
    let mut cursor = start;
    while cursor <= end {
        // Calculate the remaining distance first.  Adding 27 days before
        // clamping to `end` can overflow even when the requested one-day
        // window itself is representable (for example at `NaiveDate::MAX`).
        let remaining_days = (end - cursor).num_days();
        let chunk_days = remaining_days.min(CALENDAR_CHUNK_DAYS - 1);
        let candidate = cursor
            .checked_add_signed(Duration::days(chunk_days))
            .ok_or(RegistrarError::DateOverflow)?;
        let window_end = candidate;
        windows.push(CalendarWindow::new(cursor, window_end)?);
        if window_end == end {
            break;
        }
        // `window_end < end` proves that the successor is representable, but
        // retain the explicit error for defensive handling of chrono changes.
        cursor = window_end.succ_opt().ok_or(RegistrarError::DateOverflow)?;
    }

    Ok(windows)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegistrarProfile {
    pub stage: AcademicStage,
}

impl RegistrarProfile {
    pub const fn new(stage: AcademicStage) -> Self {
        Self { stage }
    }

    pub const fn calendar_path(self) -> &'static str {
        match self.stage {
            AcademicStage::Undergraduate => "/jxmh_out.do",
            AcademicStage::Graduate => "/jxmh_out.do",
        }
    }

    pub const fn calendar_method(self) -> &'static str {
        match self.stage {
            AcademicStage::Undergraduate => "bks_jxrl_all",
            AcademicStage::Graduate => "yjs_jxrl_all",
        }
    }

    pub fn calendar_request(
        self,
        window: CalendarWindow,
        callback: &str,
    ) -> Result<RegistrarCalendarRequest, RegistrarError> {
        window.validate()?;
        if callback.trim().is_empty() {
            return Err(RegistrarError::EmptyCallback);
        }
        if !valid_jsonp_callback(callback) {
            return Err(RegistrarError::InvalidCallback);
        }

        Ok(RegistrarCalendarRequest {
            path: self.calendar_path(),
            method: self.calendar_method(),
            start_date: window.start.format("%Y%m%d").to_string(),
            end_date: window.end.format("%Y%m%d").to_string(),
            callback: callback.to_owned(),
        })
    }
}

fn valid_jsonp_callback(callback: &str) -> bool {
    callback.split('.').all(|segment| {
        let mut characters = segment.chars();
        let Some(first) = characters.next() else {
            return false;
        };
        if !(first == '_' || first == '$' || first.is_ascii_alphabetic()) {
            return false;
        }
        characters.all(|character| {
            character == '_' || character == '$' || character.is_ascii_alphanumeric()
        })
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistrarCalendarRequest {
    pub path: &'static str,
    pub method: &'static str,
    pub start_date: String,
    pub end_date: String,
    pub callback: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 9, day).expect("valid test date")
    }

    #[test]
    fn splits_calendar_ranges_into_at_most_28_day_windows() {
        let windows = split_calendar_range(date(1), NaiveDate::from_ymd_opt(2026, 10, 15).unwrap())
            .expect("range splits");

        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].day_count(), 28);
        assert_eq!(windows[1].day_count(), 17);
        assert_eq!(windows[0].end.succ_opt().unwrap(), windows[1].start);
    }

    #[test]
    fn builds_stage_specific_jsonp_request_parameters() {
        let profile = RegistrarProfile::new(AcademicStage::Graduate);
        let request = profile
            .calendar_request(
                CalendarWindow::new(date(1), date(5)).unwrap(),
                "thyouCallback",
            )
            .expect("request builds");

        assert_eq!(request.path, "/jxmh_out.do");
        assert_eq!(request.method, "yjs_jxrl_all");
        assert_eq!(request.start_date, "20260901");
        assert_eq!(request.end_date, "20260905");
        assert_eq!(request.callback, "thyouCallback");
    }

    #[test]
    fn rejects_empty_ranges_and_callbacks() {
        assert_eq!(split_calendar_range(date(5), date(5)).unwrap().len(), 1);
        assert!(matches!(
            split_calendar_range(date(6), date(5)),
            Err(RegistrarError::InvalidDateRange)
        ));

        let profile = RegistrarProfile::new(AcademicStage::Undergraduate);
        assert!(matches!(
            profile.calendar_request(CalendarWindow::new(date(1), date(2)).unwrap(), "  "),
            Err(RegistrarError::EmptyCallback)
        ));
        assert!(matches!(
            profile.calendar_request(
                CalendarWindow::new(date(1), date(2)).unwrap(),
                "callback;invalid",
            ),
            Err(RegistrarError::InvalidCallback)
        ));
    }
}
