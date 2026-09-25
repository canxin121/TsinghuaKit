//! Curated domain values for read-only campus-card queries.
//!
//! The card SSO, account serial, transaction selectors, and response DTOs stay
//! inside the engine. This module exposes only validated display data and a
//! bounded date/type selection.

use std::fmt;

use crate::error::{Error, ErrorCode, Service};

/// A fixed campus-card ledger filter. It cannot select an account or a
/// server-side transaction identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum CampusCardTransactionType {
    /// Include every supported ledger entry.
    Any,
    /// Include consumption entries.
    Consumption,
    /// Include recharge entries.
    Recharge,
    /// Include subsidy entries.
    Subsidy,
}

impl CampusCardTransactionType {
    fn engine_value(self) -> crate::campus_card_read::CampusCardTransactionType {
        match self {
            Self::Any => crate::campus_card_read::CampusCardTransactionType::Any,
            Self::Consumption => crate::campus_card_read::CampusCardTransactionType::Consumption,
            Self::Recharge => crate::campus_card_read::CampusCardTransactionType::Recharge,
            Self::Subsidy => crate::campus_card_read::CampusCardTransactionType::Subsidy,
        }
    }

    pub(crate) const fn wire_value(self) -> &'static str {
        match self {
            Self::Any => "any",
            Self::Consumption => "consumption",
            Self::Recharge => "recharge",
            Self::Subsidy => "subsidy",
        }
    }
}

/// A validated date/type window for a complete campus-card transaction read.
#[derive(Clone, PartialEq, Eq)]
pub struct CampusCardTransactionRange {
    start_date: String,
    end_date: String,
    transaction_type: CampusCardTransactionType,
}

impl CampusCardTransactionRange {
    /// Creates an inclusive range of at most 31 calendar days.
    pub fn new(
        start_date: &str,
        end_date: &str,
        transaction_type: CampusCardTransactionType,
    ) -> Result<Self, Error> {
        let query = crate::campus_card_read::CampusCardTransactionQuery::new(
            start_date,
            end_date,
            transaction_type.engine_value(),
            1,
            0,
        )
        .map_err(|_| Error::new(Service::CampusCard, ErrorCode::InvalidInput))?;
        Ok(Self {
            start_date: query.start_date().as_str().to_owned(),
            end_date: query.end_date().as_str().to_owned(),
            transaction_type,
        })
    }

    /// Returns the first included campus-local date.
    pub fn start_date(&self) -> &str {
        &self.start_date
    }

    /// Returns the last included campus-local date.
    pub fn end_date(&self) -> &str {
        &self.end_date
    }

    /// Returns the fixed transaction type filter.
    pub fn transaction_type(&self) -> CampusCardTransactionType {
        self.transaction_type
    }
}

impl fmt::Debug for CampusCardTransactionRange {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CampusCardTransactionRange")
            .field("start_date", &self.start_date)
            .field("end_date", &self.end_date)
            .field("transaction_type", &self.transaction_type)
            .finish()
    }
}

/// The validated, read-only campus-card account fields exposed to callers.
#[derive(Clone, PartialEq, Eq)]
pub struct CampusCardAccount {
    display_name: String,
    display_name_latin: Option<String>,
    department_name: String,
    department_name_latin: Option<String>,
    department_id: i64,
    gender: Option<String>,
    effective_at: String,
    valid_until: String,
    balance_cents: i64,
    card_status: String,
    last_transaction_at: String,
    daily_limit_cents: i64,
    one_time_limit_cents: i64,
}

impl CampusCardAccount {
    pub(crate) fn from_runtime(
        value: crate::campus_card_read::CampusCardAccount,
    ) -> Result<Self, Error> {
        let invalid = || Error::new(Service::CampusCard, ErrorCode::InvalidResponse);
        if !safe_text(&value.display_name, false)
            || !safe_optional_text(value.display_name_latin.as_deref())
            || !safe_text(&value.department_name, false)
            || !safe_optional_text(value.department_name_latin.as_deref())
            || !safe_optional_text(value.gender.as_deref())
            || !safe_text(&value.effective_at, false)
            || !safe_text(&value.valid_until, false)
            || !safe_text(&value.card_status, false)
            || !safe_text(&value.last_transaction_at, true)
            || value.department_id < 0
            || value.balance_cents < 0
            || value.daily_limit_cents < 0
            || value.one_time_limit_cents < 0
        {
            return Err(invalid());
        }
        Ok(Self {
            display_name: value.display_name,
            display_name_latin: value.display_name_latin,
            department_name: value.department_name,
            department_name_latin: value.department_name_latin,
            department_id: value.department_id,
            gender: value.gender,
            effective_at: value.effective_at,
            valid_until: value.valid_until,
            balance_cents: value.balance_cents,
            card_status: value.card_status,
            last_transaction_at: value.last_transaction_at,
            daily_limit_cents: value.daily_limit_cents,
            one_time_limit_cents: value.one_time_limit_cents,
        })
    }

    /// Returns the account holder's display name.
    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    /// Returns the optional Latin display name.
    pub fn display_name_latin(&self) -> Option<&str> {
        self.display_name_latin.as_deref()
    }

    /// Returns the department display name.
    pub fn department_name(&self) -> &str {
        &self.department_name
    }

    /// Returns the optional Latin department display name.
    pub fn department_name_latin(&self) -> Option<&str> {
        self.department_name_latin.as_deref()
    }

    /// Returns the department's numeric display identifier.
    pub fn department_id(&self) -> i64 {
        self.department_id
    }

    /// Returns the optional source gender label.
    pub fn gender(&self) -> Option<&str> {
        self.gender.as_deref()
    }

    /// Returns the source effective-time label without guessing its timezone.
    pub fn effective_at(&self) -> &str {
        &self.effective_at
    }

    /// Returns the source validity-time label without guessing its timezone.
    pub fn valid_until(&self) -> &str {
        &self.valid_until
    }

    /// Returns the current account balance in fen.
    pub fn balance_cents(&self) -> i64 {
        self.balance_cents
    }

    /// Returns the source card-status label.
    pub fn card_status(&self) -> &str {
        &self.card_status
    }

    /// Returns the last-transaction display label when available.
    pub fn last_transaction_at(&self) -> &str {
        &self.last_transaction_at
    }

    /// Returns the daily limit in fen.
    pub fn daily_limit_cents(&self) -> i64 {
        self.daily_limit_cents
    }

    /// Returns the per-transaction limit in fen.
    pub fn one_time_limit_cents(&self) -> i64 {
        self.one_time_limit_cents
    }
}

impl fmt::Debug for CampusCardAccount {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CampusCardAccount")
            .field("display_name_present", &true)
            .field("department_name_present", &true)
            .field("has_balance", &true)
            .field("card_status_present", &true)
            .finish()
    }
}

/// One validated transaction row. The upstream transaction ID is deliberately
/// omitted because no public operation accepts it as a selector.
#[derive(Clone, PartialEq, Eq)]
pub struct CampusCardTransaction {
    summary: String,
    occurred_at: String,
    post_balance_cents: i64,
    amount_cents: i64,
    merchant_address: String,
    merchant_name: Option<String>,
    transaction_name: String,
}

impl CampusCardTransaction {
    pub(crate) fn from_runtime(
        value: crate::campus_card_read::CampusCardTransaction,
        range: &CampusCardTransactionRange,
    ) -> Result<Self, Error> {
        let invalid = || Error::new(Service::CampusCard, ErrorCode::InvalidResponse);
        let Some(date) = crate::campus_card_read::transaction_local_date(&value.occurred_at) else {
            return Err(invalid());
        };
        let start = chrono::NaiveDate::parse_from_str(&range.start_date, "%Y-%m-%d")
            .map_err(|_| invalid())?;
        let end = chrono::NaiveDate::parse_from_str(&range.end_date, "%Y-%m-%d")
            .map_err(|_| invalid())?;
        if date < start
            || date > end
            || !safe_text(&value.summary, false)
            || !safe_text(&value.occurred_at, false)
            || !safe_text(&value.merchant_address, true)
            || !safe_optional_text(value.merchant_name.as_deref())
            || !safe_text(&value.transaction_name, false)
            || value.post_balance_cents < 0
        {
            return Err(invalid());
        }
        Ok(Self {
            summary: value.summary,
            occurred_at: value.occurred_at,
            post_balance_cents: value.post_balance_cents,
            amount_cents: value.amount_cents,
            merchant_address: value.merchant_address,
            merchant_name: value.merchant_name,
            transaction_name: value.transaction_name,
        })
    }

    /// Returns the source transaction summary.
    pub fn summary(&self) -> &str {
        &self.summary
    }

    /// Returns the source timestamp label. Naive values are campus wall time.
    pub fn occurred_at(&self) -> &str {
        &self.occurred_at
    }

    /// Returns the post-transaction balance in fen.
    pub fn post_balance_cents(&self) -> i64 {
        self.post_balance_cents
    }

    /// Returns the signed ledger delta in fen.
    pub fn amount_cents(&self) -> i64 {
        self.amount_cents
    }

    /// Returns the merchant address, which may be empty for non-merchant rows.
    pub fn merchant_address(&self) -> &str {
        &self.merchant_address
    }

    /// Returns the optional merchant name.
    pub fn merchant_name(&self) -> Option<&str> {
        self.merchant_name.as_deref()
    }

    /// Returns the source transaction-type display label.
    pub fn transaction_name(&self) -> &str {
        &self.transaction_name
    }
}

impl fmt::Debug for CampusCardTransaction {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CampusCardTransaction")
            .field("summary_present", &true)
            .field("timestamp_present", &true)
            .field("merchant_present", &(!self.merchant_name.is_none()))
            .finish()
    }
}

/// A complete, bounded transaction collection for one selected range.
#[derive(Clone, PartialEq, Eq)]
pub struct CampusCardTransactions {
    range: CampusCardTransactionRange,
    items: Vec<CampusCardTransaction>,
}

impl CampusCardTransactions {
    pub(crate) fn from_runtime(
        value: crate::api::runtime::CampusCardTransactionsResultDto,
        range: CampusCardTransactionRange,
    ) -> Result<Self, Error> {
        let expected_type = match range.transaction_type {
            CampusCardTransactionType::Any => "any",
            CampusCardTransactionType::Consumption => "consumption",
            CampusCardTransactionType::Recharge => "recharge",
            CampusCardTransactionType::Subsidy => "subsidy",
        };
        if value.start_date != range.start_date
            || value.end_date != range.end_date
            || value.transaction_type != expected_type
            || value.transactions.len() > 5_000
        {
            return Err(Error::new(Service::CampusCard, ErrorCode::InvalidResponse));
        }
        let mut identifiers = std::collections::HashSet::new();
        let items = value
            .transactions
            .into_iter()
            .map(|transaction| {
                if !identifiers.insert(transaction.transaction_id.clone()) {
                    return Err(Error::new(Service::CampusCard, ErrorCode::InvalidResponse));
                }
                CampusCardTransaction::from_runtime(transaction, &range)
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { range, items })
    }

    /// Returns the date and transaction-type query that produced this list.
    pub fn range(&self) -> &CampusCardTransactionRange {
        &self.range
    }

    /// Returns every transaction in the complete bounded date range.
    pub fn items(&self) -> &[CampusCardTransaction] {
        &self.items
    }
}

impl fmt::Debug for CampusCardTransactions {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CampusCardTransactions")
            .field("range", &self.range)
            .field("item_count", &self.items.len())
            .finish()
    }
}

/// A one-shot campus-card service password supplied after the Rust runtime
/// explicitly asks for it. It is neither an Auth account nor persisted.
pub struct CampusCardPasswordRequest {
    password: Option<zeroize::Zeroizing<String>>,
}

/// A service-level interaction that the current Client may need before a
/// campus-card read can continue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CampusCardInteraction {
    /// The card service requested its target-specific password once.
    PasswordRequired,
}

impl CampusCardPasswordRequest {
    /// Creates a one-shot service-password submission.
    pub fn new(password: impl Into<String>) -> Self {
        Self {
            password: Some(zeroize::Zeroizing::new(password.into())),
        }
    }

    pub(crate) fn take_password(&mut self) -> zeroize::Zeroizing<String> {
        self.password
            .take()
            .unwrap_or_else(|| zeroize::Zeroizing::new(String::new()))
    }

    pub(crate) fn is_valid(&self) -> bool {
        self.password
            .as_ref()
            .is_some_and(|password| !password.is_empty() && password.len() <= 4096)
    }
}

impl fmt::Debug for CampusCardPasswordRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CampusCardPasswordRequest")
            .field(
                "password_present",
                &self
                    .password
                    .as_ref()
                    .is_some_and(|password| !password.is_empty()),
            )
            .finish()
    }
}

impl Drop for CampusCardPasswordRequest {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.password.zeroize();
    }
}

fn safe_optional_text(value: Option<&str>) -> bool {
    value.is_none_or(|value| safe_text(value, false))
}

fn safe_text(value: &str, empty: bool) -> bool {
    (empty || !value.trim().is_empty())
        && value.chars().count() <= 4096
        && !value.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transaction_range_rejects_reversed_invalid_and_oversized_windows() {
        for (start, end) in [
            ("2026-09-02", "2026-09-01"),
            ("2026-02-30", "2026-03-01"),
            ("2026-09-01", "2026-10-02"),
        ] {
            let error = CampusCardTransactionRange::new(start, end, CampusCardTransactionType::Any)
                .unwrap_err();
            assert_eq!(error.code(), ErrorCode::InvalidInput);
        }
    }

    #[test]
    fn transaction_range_normalizes_only_validated_wire_dates() {
        let range = CampusCardTransactionRange::new(
            "2026-09-01",
            "2026-09-30",
            CampusCardTransactionType::Consumption,
        )
        .unwrap();

        assert_eq!(range.start_date(), "2026-09-01");
        assert_eq!(range.end_date(), "2026-09-30");
        assert_eq!(
            range.transaction_type(),
            CampusCardTransactionType::Consumption
        );
    }

    #[test]
    fn password_request_debug_redacts_the_secret_and_zeroize_is_owned() {
        let request = CampusCardPasswordRequest::new("card-service-secret");
        let debug = format!("{request:?}");

        assert!(debug.contains("password_present: true"));
        assert!(!debug.contains("card-service-secret"));
    }
}
