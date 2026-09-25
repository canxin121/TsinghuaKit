//! Curated read-only electricity values for the public SDK.
//!
//! The legacy source does not document the unit of its numeric values and
//! exposes several unlabeled payment-history columns. This facade preserves
//! the numeric values as returned and exposes only the validated timestamp,
//! amount, and status fields with established meaning.

use std::fmt;

use crate::error::{Error, ErrorCode, Service};

/// The current dorm-electricity remainder.
#[derive(Clone, PartialEq)]
pub struct ElectricityRemainder {
    remainder: f64,
    update_time: String,
}

impl ElectricityRemainder {
    pub(crate) fn from_runtime(
        value: crate::api::runtime::ElectricityRemainderDto,
    ) -> Result<Self, Error> {
        if !value.remainder.is_finite()
            || value.remainder.abs() > 1_000_000_000_000.0
            || !valid_legacy_timestamp(&value.update_time)
        {
            return Err(invalid_electricity_response());
        }
        Ok(Self {
            remainder: value.remainder,
            update_time: value.update_time,
        })
    }

    /// Returns the numeric value exactly as reported by the service.
    ///
    /// The source does not establish a unit, so the SDK does not label this
    /// value as currency, energy, or another measurement.
    pub fn value(&self) -> f64 {
        self.remainder
    }

    /// Returns the source's local-time label without converting its timezone.
    pub fn update_time(&self) -> &str {
        &self.update_time
    }
}

impl fmt::Debug for ElectricityRemainder {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ElectricityRemainder")
            .field("value_present", &true)
            .field("update_time_present", &true)
            .finish()
    }
}

/// One validated payment-history record.
#[derive(Clone, PartialEq)]
pub struct ElectricityPaymentRecord {
    occurred_at: String,
    amount: f64,
    status: String,
}

impl ElectricityPaymentRecord {
    fn from_runtime(
        value: crate::api::runtime::ElectricityPaymentRecordDto,
    ) -> Result<Self, Error> {
        if !valid_legacy_timestamp(&value.occurred_at)
            || !value.amount.is_finite()
            || value.amount.abs() > 1_000_000_000_000.0
            || !safe_label(&value.status, 128)
        {
            return Err(invalid_electricity_response());
        }
        Ok(Self {
            occurred_at: value.occurred_at,
            amount: value.amount,
            status: value.status,
        })
    }

    /// Returns the source time label without guessing a timezone.
    pub fn occurred_at(&self) -> &str {
        &self.occurred_at
    }

    /// Returns the numeric amount exactly as reported by the service.
    ///
    /// The source does not document a unit or scale for this field.
    pub fn amount(&self) -> f64 {
        self.amount
    }

    /// Returns the source's payment status label.
    pub fn status(&self) -> &str {
        &self.status
    }
}

impl fmt::Debug for ElectricityPaymentRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ElectricityPaymentRecord")
            .field("occurred_at_present", &true)
            .field("amount_present", &true)
            .field("status_present", &true)
            .finish()
    }
}

/// A complete payment-history response, including a structurally verified
/// empty response.
#[derive(Clone, PartialEq)]
pub struct ElectricityPaymentHistory {
    records: Vec<ElectricityPaymentRecord>,
}

impl ElectricityPaymentHistory {
    pub(crate) fn from_runtime(
        value: crate::api::runtime::ElectricityPaymentHistoryDto,
    ) -> Result<Self, Error> {
        if value.empty != value.records.is_empty() {
            return Err(invalid_electricity_response());
        }
        let records = value
            .records
            .into_iter()
            .map(ElectricityPaymentRecord::from_runtime)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { records })
    }

    /// Returns all payment-history records in the source's order.
    pub fn records(&self) -> &[ElectricityPaymentRecord] {
        &self.records
    }

    /// Returns whether the service confirmed that the history is empty.
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
}

impl fmt::Debug for ElectricityPaymentHistory {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ElectricityPaymentHistory")
            .field("record_count", &self.records.len())
            .finish()
    }
}

fn invalid_electricity_response() -> Error {
    Error::new(Service::Electricity, ErrorCode::InvalidResponse)
}

fn safe_label(value: &str, max_bytes: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_bytes
        && value.chars().all(|character| !character.is_control())
}

fn valid_legacy_timestamp(value: &str) -> bool {
    if !safe_label(value, 64) {
        return false;
    }
    let mut pieces = value.split_whitespace();
    let date = pieces.next().unwrap_or_default();
    let time = pieces.next().unwrap_or_default();
    if pieces.next().is_some() {
        return false;
    }

    let separator = if date.contains('/') { '/' } else { '-' };
    let date_fields = date.split(separator).collect::<Vec<_>>();
    let [year, month, day] = date_fields.as_slice() else {
        return false;
    };
    let (Ok(year), Ok(month), Ok(day)) = (
        year.parse::<i32>(),
        month.parse::<u32>(),
        day.parse::<u32>(),
    ) else {
        return false;
    };
    if chrono::NaiveDate::from_ymd_opt(year, month, day).is_none() {
        return false;
    }

    let time_fields = time.split(':').collect::<Vec<_>>();
    if !(time_fields.len() == 2 || time_fields.len() == 3) {
        return false;
    }
    let Ok(hour) = time_fields[0].parse::<u32>() else {
        return false;
    };
    let Ok(minute) = time_fields[1].parse::<u32>() else {
        return false;
    };
    let second = match time_fields.get(2) {
        Some(value) => match value.parse::<u32>() {
            Ok(second) => second,
            Err(_) => return false,
        },
        None => 0,
    };
    hour < 24 && minute < 60 && second < 60
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_refactor_electricity_remainder_preserves_the_unscaled_source_value() {
        let remainder =
            ElectricityRemainder::from_runtime(crate::api::runtime::ElectricityRemainderDto {
                remainder: 123.45,
                update_time: "2026-09-25 08:09:10".to_owned(),
            })
            .unwrap();

        assert_eq!(remainder.value(), 123.45);
        assert_eq!(remainder.update_time(), "2026-09-25 08:09:10");
        let rendered = format!("{remainder:?}");
        assert!(!rendered.contains("123.45"));
        assert!(!rendered.contains("2026-09-25"));
    }

    #[test]
    fn backend_refactor_electricity_public_models_omit_unlabeled_columns_and_redact_debug() {
        let history = ElectricityPaymentHistory::from_runtime(
            crate::api::runtime::ElectricityPaymentHistoryDto {
                records: vec![crate::api::runtime::ElectricityPaymentRecordDto {
                    source_columns: vec![
                        "unlabeled-private-value".to_owned(),
                        "42".to_owned(),
                        "2026-09-25 08:09:10".to_owned(),
                        "unlabeled-selector".to_owned(),
                        "12.5".to_owned(),
                        "paid".to_owned(),
                    ],
                    sequence: Some(42),
                    occurred_at: "2026-09-25 08:09:10".to_owned(),
                    amount: 12.5,
                    status: "paid".to_owned(),
                }],
                empty: false,
            },
        )
        .unwrap();

        assert_eq!(history.records().len(), 1);
        assert_eq!(history.records()[0].amount(), 12.5);
        assert_eq!(history.records()[0].status(), "paid");
        let rendered = format!("{history:?} {:?}", history.records()[0]);
        assert!(!rendered.contains("unlabeled-private-value"));
        assert!(!rendered.contains("unlabeled-selector"));
        assert!(!rendered.contains("2026-09-25"));
        assert!(!rendered.contains("12.5"));
    }

    #[test]
    fn backend_refactor_electricity_accepts_a_verified_empty_history() {
        let history = ElectricityPaymentHistory::from_runtime(
            crate::api::runtime::ElectricityPaymentHistoryDto {
                records: Vec::new(),
                empty: true,
            },
        )
        .unwrap();
        assert!(history.is_empty());

        let inconsistent = ElectricityPaymentHistory::from_runtime(
            crate::api::runtime::ElectricityPaymentHistoryDto {
                records: Vec::new(),
                empty: false,
            },
        )
        .unwrap_err();
        assert_eq!(inconsistent.code(), ErrorCode::InvalidResponse);
    }

    #[test]
    fn backend_refactor_electricity_public_models_reject_unverified_fields() {
        let invalid = ElectricityPaymentHistory::from_runtime(
            crate::api::runtime::ElectricityPaymentHistoryDto {
                records: vec![crate::api::runtime::ElectricityPaymentRecordDto {
                    source_columns: vec![],
                    sequence: None,
                    occurred_at: "not-a-timestamp".to_owned(),
                    amount: f64::NAN,
                    status: "paid".to_owned(),
                }],
                empty: false,
            },
        )
        .unwrap_err();
        assert_eq!(invalid.service(), Service::Electricity);
        assert_eq!(invalid.code(), ErrorCode::InvalidResponse);
    }
}
