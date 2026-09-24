//! Rust-owned SRun login material.
//!
//! The current Tsinghua portal page calls `md5(password, token)`.  In the
//! page's md5 implementation this is HMAC-MD5 with the challenge token as
//! the key and the plaintext password as the message.  The same challenge
//! token is then used as the xencode key for the `info` field.  This module
//! keeps those operations next to the request client so a plaintext password
//! never has to cross the Flutter boundary or be stored in a generic request
//! DTO.
//!
//! The implementation is an independent Rust implementation of the wire
//! algorithm.  It follows the current public portal JavaScript profile:
//! `srun_bx1`, `n=200`, `type=1`, fixed JSON field order, lowercase hex, and
//! the SRun Base64 alphabet.  Older clients use different password digest
//! conventions; they remain explicit profile candidates in [`crate::tunet`]
//! and are not silently substituted here.

use std::fmt;

use hmac::{Hmac, Mac};
use md5::Md5;
use sha1::{Digest, Sha1};
use thiserror::Error;

const ENC_VER: &str = "srun_bx1";
const REQUEST_N: &str = "200";
const REQUEST_TYPE: &str = "1";
const PASSWORD_PREFIX: &str = "{MD5}";
const INFO_PREFIX: &str = "{SRBX1}";
const SRUN_BASE64_ALPHABET: &[u8; 64] =
    b"LVoJPiCN2R8G90yg+hmFHuacZ1OWMnrsSTXkYpUq/3dlbfKwv6xztjI7DeBE45QA";

type HmacMd5 = Hmac<Md5>;

/// Password digest profiles that have a concrete, independently verified
/// input order.  The current portal page uses the token-keyed profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SrunPasswordDigestScheme {
    /// HMAC-MD5(key = challenge token, message = plaintext password), hex.
    PortalHmacMd5,
}

/// Stable errors raised while constructing SRun login material.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SrunLoginError {
    #[error("SRun {field} must not be empty")]
    EmptyField { field: &'static str },

    #[error("SRun {field} contains a control character")]
    InvalidField { field: &'static str },

    #[error("SRun JSON field could not be encoded")]
    JsonEncode,
}

/// Derived SRun fields ready to add to the portal request.
///
/// Every field is derived from a password or challenge token.  The values are
/// available to the Rust request planner through accessors, while `Debug`
/// redacts all three so a request plan cannot accidentally print credentials
/// or challenge-derived material.
#[derive(Clone, PartialEq, Eq)]
pub struct SrunLoginMaterial {
    wire_password: String,
    info: String,
    checksum: String,
}

impl SrunLoginMaterial {
    pub fn wire_password(&self) -> &str {
        &self.wire_password
    }

    pub fn info(&self) -> &str {
        &self.info
    }

    pub fn checksum(&self) -> &str {
        &self.checksum
    }
}

impl fmt::Debug for SrunLoginMaterial {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SrunLoginMaterial")
            .field("wire_password", &"[redacted]")
            .field("info", &"[redacted]")
            .field("checksum", &"[redacted]")
            .finish()
    }
}

/// Build the password, `info`, and `chksum` values used by the current SRun
/// account login page.
pub fn build_srun_login_material(
    scheme: SrunPasswordDigestScheme,
    username: &str,
    password: &str,
    challenge_token: &str,
    ac_id: &str,
    ip: &str,
) -> Result<SrunLoginMaterial, SrunLoginError> {
    require_nonempty("username", username)?;
    require_nonempty("password", password)?;
    require_nonempty("challenge token", challenge_token)?;
    require_nonempty("ac_id", ac_id)?;
    require_nonempty("IP address", ip)?;

    let password_digest = match scheme {
        SrunPasswordDigestScheme::PortalHmacMd5 => {
            let mut hmac = HmacMd5::new_from_slice(challenge_token.as_bytes())
                .expect("HMAC accepts keys of every length");
            hmac.update(password.as_bytes());
            hex_lower(&hmac.finalize().into_bytes())
        }
    };

    let info_json = format_info_json(username, password, ip, ac_id)?;
    let encoded_info = srun_base64(&xencode(&info_json, challenge_token));
    let info = format!("{INFO_PREFIX}{encoded_info}");
    let checksum_input = format!(
        "{challenge_token}{username}{challenge_token}{password_digest}{challenge_token}{ac_id}{challenge_token}{ip}{challenge_token}{REQUEST_N}{challenge_token}{REQUEST_TYPE}{challenge_token}{info}"
    );
    let mut sha1 = Sha1::new();
    sha1.update(checksum_input.as_bytes());
    let checksum = hex_lower(&sha1.finalize());

    Ok(SrunLoginMaterial {
        wire_password: format!("{PASSWORD_PREFIX}{password_digest}"),
        info,
        checksum,
    })
}

/// The fixed request fields required by the current `srun_bx1` profile.
pub fn srun_request_n() -> &'static str {
    REQUEST_N
}

/// The fixed request type required by the current `srun_bx1` profile.
pub fn srun_request_type() -> &'static str {
    REQUEST_TYPE
}

fn require_nonempty(field: &'static str, value: &str) -> Result<(), SrunLoginError> {
    if value.is_empty() {
        Err(SrunLoginError::EmptyField { field })
    } else if value.chars().any(char::is_control) {
        Err(SrunLoginError::InvalidField { field })
    } else {
        Ok(())
    }
}

fn format_info_json(
    username: &str,
    password: &str,
    ip: &str,
    ac_id: &str,
) -> Result<String, SrunLoginError> {
    let username = serde_json::to_string(username).map_err(|_| SrunLoginError::JsonEncode)?;
    let password = serde_json::to_string(password).map_err(|_| SrunLoginError::JsonEncode)?;
    let ip = serde_json::to_string(ip).map_err(|_| SrunLoginError::JsonEncode)?;
    let ac_id = serde_json::to_string(ac_id).map_err(|_| SrunLoginError::JsonEncode)?;
    Ok(format!(
        r#"{{"username":{username},"password":{password},"ip":{ip},"acid":{ac_id},"enc_ver":"{ENC_VER}"}}"#
    ))
}

/// Encode a UTF-16 JavaScript string using the SRun xencode variant.
///
/// The portal implementation uses `charCodeAt` and packs four UTF-16 code
/// units into each 32-bit word.  Working with UTF-16 code units here keeps
/// non-ASCII input aligned with the browser implementation rather than with
/// a byte-oriented approximation.
fn xencode(value: &str, key: &str) -> Vec<u8> {
    if value.is_empty() {
        return Vec::new();
    }

    let mut values = pack_utf16(value, true);
    let mut key_values = pack_utf16(key, false);
    if key_values.len() < 4 {
        key_values.resize(4, 0);
    }

    let n = values.len() - 1;
    let mut z = values[n];
    let mut y;
    let rounds = 6 + 52 / (n + 1);
    let mut delta = 0_u32;

    for _ in 0..rounds {
        delta = delta.wrapping_add(0x9E37_79B9);
        let e = (delta >> 2) & 3;

        for p in 0..n {
            y = values[p + 1];
            let mut mix = (z >> 5) ^ (y << 2);
            mix = mix.wrapping_add((y >> 3) ^ (z << 4) ^ (delta ^ y));
            mix = mix.wrapping_add(key_values[(p & 3) ^ e as usize] ^ z);
            z = values[p].wrapping_add(mix);
            values[p] = z;
        }

        y = values[0];
        let p = n;
        let mut mix = (z >> 5) ^ (y << 2);
        mix = mix.wrapping_add((y >> 3) ^ (z << 4) ^ (delta ^ y));
        mix = mix.wrapping_add(key_values[(p & 3) ^ e as usize] ^ z);
        z = values[p].wrapping_add(mix);
        values[p] = z;
    }

    values.into_iter().flat_map(u32::to_le_bytes).collect()
}

fn pack_utf16(value: &str, include_length: bool) -> Vec<u32> {
    let code_units: Vec<u16> = value.encode_utf16().collect();
    let mut packed = vec![0_u32; code_units.len().div_ceil(4)];
    for (index, code_unit) in code_units.iter().copied().enumerate() {
        packed[index / 4] |= u32::from(code_unit) << (8 * (index % 4));
    }
    if include_length {
        packed.push(code_units.len() as u32);
    }
    packed
}

fn srun_base64(bytes: &[u8]) -> String {
    let output_len = bytes.len().div_ceil(3) * 4;
    let mut output = String::with_capacity(output_len);

    for chunk in bytes.chunks(3) {
        let first = u32::from(chunk[0]);
        let second = u32::from(*chunk.get(1).unwrap_or(&0));
        let third = u32::from(*chunk.get(2).unwrap_or(&0));
        let combined = (first << 16) | (second << 8) | third;
        let present = chunk.len();

        output.push(SRUN_BASE64_ALPHABET[((combined >> 18) & 0x3f) as usize] as char);
        output.push(SRUN_BASE64_ALPHABET[((combined >> 12) & 0x3f) as usize] as char);
        if present >= 2 {
            output.push(SRUN_BASE64_ALPHABET[((combined >> 6) & 0x3f) as usize] as char);
        } else {
            output.push('=');
        }
        if present == 3 {
            output.push(SRUN_BASE64_ALPHABET[(combined & 0x3f) as usize] as char);
        } else {
            output.push('=');
        }
    }

    output
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portal_digest_uses_token_as_hmac_key() {
        let material = build_srun_login_material(
            SrunPasswordDigestScheme::PortalHmacMd5,
            "alice",
            "what do ya want for nothing?",
            "Jefe",
            "1",
            "192.0.2.10",
        )
        .expect("fixture material should build");

        assert!(material.wire_password().starts_with(PASSWORD_PREFIX));
        assert_eq!(
            &material.wire_password()[PASSWORD_PREFIX.len()..],
            "750c783e6ab0b503eaa86e310a5db738"
        );
        assert!(!format!("{material:?}").contains("750c783e6ab0b503eaa86e310a5db738"));
    }

    #[test]
    fn info_keeps_the_portal_field_order_and_srun_prefix() {
        let material = build_srun_login_material(
            SrunPasswordDigestScheme::PortalHmacMd5,
            "alice",
            "password",
            "challenge",
            "1",
            "192.0.2.10",
        )
        .expect("fixture material should build");

        assert!(material.info().starts_with(INFO_PREFIX));
        assert_eq!(material.info()[INFO_PREFIX.len()..].len() % 4, 0);
        assert_eq!(
            material.info(),
            "{SRBX1}Iroy5flC/aAZDkMZk4yHekK2vHFrf20xuucx0jrMldcpaZ9yCYkVUCLesjgECXpdLtLQjXLcFC899piDwP8++7zWsecYnUyK8w5TqSsinBsctl+cVo44SunRNc+Iqa9e"
        );
        assert_eq!(material.checksum().len(), 40);
        assert_eq!(
            material.checksum(),
            "504e50a97c5e986d48e29bc259ce3fd3130dc934"
        );
        assert!(
            material
                .checksum()
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        );
    }

    #[test]
    fn empty_secret_inputs_are_rejected_without_echoing_values() {
        let error = build_srun_login_material(
            SrunPasswordDigestScheme::PortalHmacMd5,
            "alice",
            "",
            "challenge",
            "1",
            "192.0.2.10",
        )
        .expect_err("empty password must be rejected");
        assert_eq!(error, SrunLoginError::EmptyField { field: "password" });
        assert!(!error.to_string().contains("challenge"));
    }

    #[test]
    fn incomplete_checksum_inputs_are_rejected() {
        for (ac_id, ip, field) in [("", "192.0.2.10", "ac_id"), ("1", "", "IP address")] {
            let error = build_srun_login_material(
                SrunPasswordDigestScheme::PortalHmacMd5,
                "alice",
                "password",
                "challenge",
                ac_id,
                ip,
            )
            .expect_err("SRun login material must include every checksum input");
            assert_eq!(error, SrunLoginError::EmptyField { field });
        }
    }
}
