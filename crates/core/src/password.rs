//! Password hashing with Argon2id (ADR-0007, ADR-0013).
//!
//! Hashes are PHC strings (`$argon2id$v=19$m=19456,t=2,p=1$…`) with the OWASP baseline
//! parameters. Hashing takes tens of milliseconds by design: call these functions from a
//! blocking task, not directly on the async runtime.

use argon2::{Algorithm, Argon2, Params, PasswordHasher, PasswordVerifier, Version};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use zeroize::Zeroizing;

use crate::crypto::{CryptoError, random_bytes};
use crate::secret::Secret;

/// Minimum password length (characters).
pub const MIN_LEN: usize = 12;
/// Maximum password length (characters); bounds hashing cost of hostile input.
pub const MAX_LEN: usize = 1024;

/// Errors from hashing. Never contains the password.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PasswordError {
    #[error("password must be {MIN_LEN}–{MAX_LEN} characters")]
    Length,
    #[error("password hashing failed")]
    Hash,
    #[error(transparent)]
    Crypto(#[from] CryptoError),
}

fn argon2() -> Argon2<'static> {
    let params = Params::new(
        Params::DEFAULT_M_COST,
        Params::DEFAULT_T_COST,
        Params::DEFAULT_P_COST,
        None,
    )
    .expect("default parameters are valid");
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
}

/// Checks the length policy.
pub fn validate(password: &str) -> Result<(), PasswordError> {
    let len = password.chars().count();
    if (MIN_LEN..=MAX_LEN).contains(&len) {
        Ok(())
    } else {
        Err(PasswordError::Length)
    }
}

/// Hashes a password (after [`validate`]) into a PHC string.
pub fn hash(password: &str) -> Result<String, PasswordError> {
    validate(password)?;
    let salt = random_bytes::<16>()?;
    argon2()
        .hash_password_with_salt(password.as_bytes(), salt.as_slice())
        .map(|h| h.to_string())
        .map_err(|_| PasswordError::Hash)
}

/// Verifies a password against a stored PHC string. Malformed hashes never verify.
#[must_use]
pub fn verify(password: &str, stored: &str) -> bool {
    if password.chars().count() > MAX_LEN {
        return false;
    }
    argon2()
        .verify_password(password.as_bytes(), stored)
        .is_ok()
}

/// Spends the same work as [`verify`] for a user that does not exist or has no password,
/// so response times do not reveal which e-mail addresses are registered.
pub fn verify_dummy(password: &str) {
    // A valid hash of a random value, computed once per process.
    static DUMMY: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    let stored = DUMMY.get_or_init(|| {
        let value = generate().map_or_else(|_| "x".repeat(MIN_LEN), |p| p.expose().to_string());
        hash(&value).unwrap_or_default()
    });
    let _ = verify(password, stored);
}

/// Generates a random password (24 characters base64url, 144 bits) to show once.
pub fn generate() -> Result<Secret<Zeroizing<String>>, CryptoError> {
    let bytes = random_bytes::<18>()?;
    Ok(Secret::new(Zeroizing::new(
        URL_SAFE_NO_PAD.encode(bytes.as_slice()),
    )))
}

#[cfg(test)]
mod tests {
    use super::{MAX_LEN, PasswordError, generate, hash, validate, verify, verify_dummy};

    #[test]
    fn hash_and_verify() {
        let stored = hash("correct horse battery").unwrap();
        assert!(stored.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"));
        assert!(!stored.contains("correct horse"));
        assert!(verify("correct horse battery", &stored));
        assert!(!verify("correct horse batterY", &stored));
        assert!(!verify("correct horse battery", "not a phc string"));
        assert_ne!(hash("correct horse battery").unwrap(), stored, "salted");
        verify_dummy("anything");
    }

    #[test]
    fn length_policy() {
        assert_eq!(validate("short").unwrap_err(), PasswordError::Length);
        assert_eq!(
            hash("x".repeat(MAX_LEN + 1).as_str()).unwrap_err(),
            PasswordError::Length
        );
        assert!(validate("twelve chars").is_ok());
        // Counted in characters, not bytes.
        assert!(validate("ääääääääääää").is_ok());
    }

    #[test]
    fn generated_passwords_are_valid_and_unique() {
        let a = generate().unwrap();
        let b = generate().unwrap();
        assert_eq!(a.expose().len(), 24);
        assert!(validate(a.expose()).is_ok());
        assert_ne!(a.expose().as_str(), b.expose().as_str());
    }
}
