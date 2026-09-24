//! Password wire encoding for the unified identity service.
//!
//! Two current independent clients use the same wire rule: encrypt the UTF-8
//! password with the page's SM2 public key, use the modern C1C3C2 layout, and
//! submit the uncompressed SEC1 ciphertext as lower-case hexadecimal.  The
//! ciphertext already contains the `04` SEC1 point prefix, so this module does
//! not prepend a second one.

use sm2::pke::{EncryptingKey, Mode};
use thiserror::Error;

/// Errors raised while turning a plaintext password into the identity wire
/// value.  The password and public-key material never enter the error value.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum Sm2PasswordError {
    #[error("the SM2 public key is empty")]
    EmptyPublicKey,

    #[error("the SM2 public key must contain 128 or 130 hexadecimal characters")]
    InvalidPublicKeyLength,

    #[error("the SM2 public key must use the uncompressed SEC1 prefix")]
    InvalidPublicKeyPrefix,

    #[error("the SM2 public key contains a non-hexadecimal character")]
    InvalidPublicKeyHex,

    #[error("the SM2 public key is not a valid uncompressed curve point")]
    InvalidPublicKeyPoint,

    #[error("SM2 password encryption failed")]
    EncryptionFailed,
}

/// Encrypts a UTF-8 password using the page-provided SM2 public key.
///
/// The returned string is the lower-case hexadecimal form expected by the
/// identity login form, including the single leading `04` point prefix.
pub fn encrypt_password(password: &str, public_key_hex: &str) -> Result<String, Sm2PasswordError> {
    let public_key = sec1_public_key(public_key_hex)?;
    let encrypting_key = EncryptingKey::from_sec1_bytes(&public_key)
        .map_err(|_| Sm2PasswordError::InvalidPublicKeyPoint)?;
    let ciphertext = encrypting_key
        .encrypt_cipher::<_, sm3::Sm3>(
            &mut sm2::elliptic_curve::common::getrandom::SysRng,
            password.as_bytes(),
        )
        .map_err(|_| Sm2PasswordError::EncryptionFailed)?
        .to_vec(Mode::C1C3C2, false)
        .map_err(|_| Sm2PasswordError::EncryptionFailed)?;

    Ok(hex_encode(&ciphertext))
}

fn sec1_public_key(public_key_hex: &str) -> Result<Vec<u8>, Sm2PasswordError> {
    let value = public_key_hex.trim();
    if value.is_empty() {
        return Err(Sm2PasswordError::EmptyPublicKey);
    }
    if value.len() != 128 && value.len() != 130 {
        return Err(Sm2PasswordError::InvalidPublicKeyLength);
    }

    let mut output = Vec::with_capacity(65);
    output.push(0x04);
    let bytes = if value.len() == 130 {
        let prefix = value.as_bytes();
        if prefix[0] != b'0' || prefix[1] != b'4' {
            return Err(Sm2PasswordError::InvalidPublicKeyPrefix);
        }
        &prefix[2..]
    } else {
        value.as_bytes()
    };
    for pair in bytes.chunks_exact(2) {
        let high = hex_digit(pair[0]).ok_or(Sm2PasswordError::InvalidPublicKeyHex)?;
        let low = hex_digit(pair[1]).ok_or(Sm2PasswordError::InvalidPublicKeyHex)?;
        output.push((high << 4) | low);
    }
    Ok(output)
}

fn hex_digit(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn hex_encode(value: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(value.len() * 2);
    for byte in value {
        output.push(HEX[usize::from(byte >> 4)] as char);
        output.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use sm2::{SecretKey, elliptic_curve::sec1::ToSec1Point, pke::DecryptingKey};

    #[test]
    fn encrypts_a_valid_page_key_with_a_single_sec1_prefix() {
        let public_key = reference_public_key();
        let encrypted =
            encrypt_password("test_password", &public_key).expect("reference public key encrypts");

        assert!(encrypted.starts_with("04"));
        assert_eq!(encrypted.len(), 2 * (65 + "test_password".len() + 32));
        assert!(encrypted.bytes().all(|byte| byte.is_ascii_hexdigit()));
    }

    #[test]
    fn accepts_the_page_key_with_its_existing_sec1_prefix() {
        let unprefixed = reference_public_key();
        let prefixed = format!("04{unprefixed}");

        let encrypted =
            encrypt_password("test_password", &prefixed).expect("prefixed key encrypts");

        assert!(encrypted.starts_with("04"));
        assert_eq!(encrypted.len(), 2 * (65 + "test_password".len() + 32));
    }

    #[test]
    fn rejects_a_130_character_key_with_a_compressed_or_invalid_prefix() {
        let unprefixed = reference_public_key();
        for prefix in ["02", "03", "05"] {
            let key = format!("{prefix}{unprefixed}");
            assert!(matches!(
                encrypt_password("password", &key),
                Err(Sm2PasswordError::InvalidPublicKeyPrefix)
            ));
        }
    }

    #[test]
    fn rejects_malformed_public_keys_without_exposing_the_key() {
        for key in ["00", &"z".repeat(128), &"0".repeat(126)] {
            let error = encrypt_password("password", key).expect_err("bad key");
            assert!(!format!("{error:?}").contains(key));
        }
    }

    #[test]
    fn ciphertext_round_trips_through_the_same_standard_implementation() {
        let secret = [
            0x3d, 0xd2, 0xa3, 0x67, 0x9b, 0xf6, 0xf1, 0xdf, 0xc3, 0xb4, 0x9d, 0x3e, 0x99, 0x11,
            0x47, 0x18, 0xe4, 0x8e, 0xec, 0x17, 0x0b, 0xe4, 0xe4, 0xd3, 0xa8, 0x20, 0x52, 0xda,
            0xb1, 0x9e, 0x8b, 0x50,
        ];
        let secret_key = SecretKey::from_slice(&secret).expect("secret key");
        let public_key = secret_key.public_key().to_sec1_point(false);
        let public_key_hex = hex_encode(&public_key.as_bytes()[1..]);
        let encrypted = encrypt_password("plaintext", &public_key_hex).expect("encrypts");
        let bytes = encrypted_bytes(&encrypted);
        let decrypting_key = DecryptingKey::new(secret_key);

        assert_eq!(
            decrypting_key.decrypt(&bytes).expect("decrypts"),
            b"plaintext"
        );
    }

    fn encrypted_bytes(value: &str) -> Vec<u8> {
        value
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| (hex_digit(pair[0]).unwrap() << 4) | hex_digit(pair[1]).unwrap())
            .collect()
    }

    fn reference_public_key() -> String {
        let secret = [
            0x3d, 0xd2, 0xa3, 0x67, 0x9b, 0xf6, 0xf1, 0xdf, 0xc3, 0xb4, 0x9d, 0x3e, 0x99, 0x11,
            0x47, 0x18, 0xe4, 0x8e, 0xec, 0x17, 0x0b, 0xe4, 0xe4, 0xd3, 0xa8, 0x20, 0x52, 0xda,
            0xb1, 0x9e, 0x8b, 0x50,
        ];
        let secret_key = SecretKey::from_slice(&secret).expect("secret key");
        let public_key = secret_key.public_key().to_sec1_point(false);
        hex_encode(&public_key.as_bytes()[1..])
    }
}
