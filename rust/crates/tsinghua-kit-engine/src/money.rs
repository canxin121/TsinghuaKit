//! Exact decimal money handling shared by the read domains.
//!
//! Several campus services render amounts as decimal strings or JSON numbers.
//! Converting one through `f64` would round it: a value the service wrote with
//! both of its decimals can come back one cent off, and the error compounds once
//! a caller sums a column.  The conversion here is arithmetic on the digit
//! string, so a representable amount survives as exact integer cents and an
//! amount that cannot be represented exactly is refused rather than rounded.
//!
//! This module reimplements the contract from public reference behavior.  It
//! does not copy source, fixtures, or assets.

/// The longest decimal token this module will look at.  Anything longer is a
/// mis-sliced fragment or a hostile value rather than a money amount.
const MAX_TOKEN_CHARS: usize = 32;

/// The largest exponent this module will accept, so a token cannot be used to
/// ask for an enormous power of ten.
const MAX_EXPONENT: i32 = 18;

/// Converts an exact decimal token into integer cents.
///
/// Returns `None` for anything that is not a plain decimal with at most two
/// significant fraction digits, so a value this module cannot represent exactly
/// is an error instead of a rounded number.
pub(crate) fn exact_cents(token: &str) -> Option<i64> {
    if token.is_empty() || token.len() > MAX_TOKEN_CHARS {
        return None;
    }
    let (negative, unsigned) = match token.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, token.strip_prefix('+').unwrap_or(token)),
    };
    let (mantissa, exponent) = match unsigned.find(['e', 'E']) {
        Some(index) => (
            &unsigned[..index],
            unsigned[index + 1..].parse::<i32>().ok()?,
        ),
        None => (unsigned, 0),
    };
    if !(-MAX_EXPONENT..=MAX_EXPONENT).contains(&exponent) {
        return None;
    }
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    if whole.is_empty()
        || !whole
            .bytes()
            .chain(fraction.bytes())
            .all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let digits: String = format!("{whole}{fraction}");
    let digits = digits.trim_start_matches('0');
    if digits.is_empty() {
        return Some(0);
    }
    // cents = digits * 10^(exponent - fraction.len() + 2)
    let shift = exponent - i32::try_from(fraction.len()).ok()? + 2;
    if shift.unsigned_abs() > 18 {
        return None;
    }
    let magnitude = digits.parse::<i128>().ok()?;
    let magnitude = if shift >= 0 {
        magnitude.checked_mul(10i128.checked_pow(u32::try_from(shift).ok()?)?)?
    } else {
        let divisor = 10i128.checked_pow(shift.unsigned_abs())?;
        if magnitude % divisor != 0 {
            return None;
        }
        magnitude / divisor
    };
    let magnitude = if negative { -magnitude } else { magnitude };
    i64::try_from(magnitude).ok()
}

/// Why a JSON amount could not be read as exact cents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AmountError {
    /// The value was present but is not an exact decimal, so reading it would
    /// mean rounding it.
    Inexact,
}

/// Converts a JSON value into exact integer cents.
///
/// A number is read from its own literal text rather than through `f64`, so a
/// JSON number the service wrote with two decimals keeps both of them.  A
/// missing key and an explicit `null` both mean "the service did not report
/// this amount" and yield `Ok(None)`; a value that is present but cannot be
/// represented exactly is an error rather than a rounded number.
pub(crate) fn json_cents(
    fields: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Result<Option<i64>, AmountError> {
    let Some(value) = fields.get(key) else {
        return Ok(None);
    };
    match value {
        serde_json::Value::Number(number) => exact_cents(&number.to_string())
            .map(Some)
            .ok_or(AmountError::Inexact),
        serde_json::Value::String(text) => exact_cents(text.trim())
            .map(Some)
            .ok_or(AmountError::Inexact),
        serde_json::Value::Null => Ok(None),
        _ => Err(AmountError::Inexact),
    }
}

#[cfg(test)]
mod tests {
    use super::{AmountError, exact_cents, json_cents};

    #[test]
    fn exact_cents_reads_integral_decimals_and_rejects_lossy_ones() {
        assert_eq!(exact_cents("0"), Some(0));
        assert_eq!(exact_cents("1"), Some(100));
        assert_eq!(exact_cents("1.5"), Some(150));
        assert_eq!(exact_cents("1.50"), Some(150));
        assert_eq!(exact_cents("-2.25"), Some(-225));
        assert_eq!(exact_cents("+3.00"), Some(300));
        assert_eq!(exact_cents("1e2"), Some(10_000));
        assert_eq!(exact_cents("1.005"), None);
        assert_eq!(exact_cents("abc"), None);
        assert_eq!(exact_cents(""), None);
        assert_eq!(exact_cents("1.2.3"), None);
        assert_eq!(exact_cents("0x10"), None);
    }

    #[test]
    fn json_cents_reads_numbers_from_their_own_literal_text() {
        let value: serde_json::Value = serde_json::json!({
            "a": 500.00,
            "b": "200.00",
            "c": null,
            "d": true,
        });
        let fields = value.as_object().expect("object");
        assert_eq!(json_cents(fields, "a"), Ok(Some(50_000)));
        assert_eq!(json_cents(fields, "b"), Ok(Some(20_000)));
        assert_eq!(json_cents(fields, "c"), Ok(None));
        assert_eq!(json_cents(fields, "d"), Err(AmountError::Inexact));
        assert_eq!(json_cents(fields, "missing"), Ok(None));

        let inexact: serde_json::Value = serde_json::json!({"e": "1.005"});
        assert_eq!(
            json_cents(inexact.as_object().expect("object"), "e"),
            Err(AmountError::Inexact)
        );
    }
}
