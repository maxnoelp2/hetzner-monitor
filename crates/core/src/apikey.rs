//! API key generation and verification (ADR-0007).
//!
//! Format: `hm_<prefix>_<secret>` — `prefix` is 12 lowercase alphanumerics used for lookup,
//! `secret` is 32 random bytes in base64url. Only the SHA-256 hash of the full key is stored.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

use crate::crypto::{CryptoError, random_bytes};
use crate::secret::Secret;

const PREFIX_LEN: usize = 12;
const ALPHABET: &[u8; 36] = b"abcdefghijklmnopqrstuvwxyz0123456789";

/// A freshly generated key. `full` is shown to the user exactly once.
pub struct NewApiKey {
    pub full: Secret<Zeroizing<String>>,
    pub prefix: String,
    pub hash: Vec<u8>,
}

/// Generates a new API key.
pub fn generate() -> Result<NewApiKey, CryptoError> {
    let prefix_bytes = random_bytes::<PREFIX_LEN>()?;
    let prefix: String = prefix_bytes
        .iter()
        .map(|b| char::from(ALPHABET[usize::from(*b) % ALPHABET.len()]))
        .collect();
    let secret = random_bytes::<32>()?;
    let full = Zeroizing::new(format!(
        "hm_{prefix}_{}",
        URL_SAFE_NO_PAD.encode(secret.as_slice())
    ));
    let hash = hash(&full);
    Ok(NewApiKey {
        full: Secret::new(full),
        prefix,
        hash,
    })
}

/// Extracts the lookup prefix from a presented key, or `None` if the format is wrong.
#[must_use]
pub fn parse_prefix(presented: &str) -> Option<&str> {
    let rest = presented.strip_prefix("hm_")?;
    let (prefix, secret) = rest.split_once('_')?;
    let valid = prefix.len() == PREFIX_LEN
        && prefix.bytes().all(|b| ALPHABET.contains(&b))
        && !secret.is_empty();
    valid.then_some(prefix)
}

/// SHA-256 of the full key.
#[must_use]
pub fn hash(full: &str) -> Vec<u8> {
    Sha256::digest(full.as_bytes()).to_vec()
}

/// Constant-time comparison of a presented key against a stored hash.
#[must_use]
pub fn verify(presented: &str, stored_hash: &[u8]) -> bool {
    hash(presented).ct_eq(stored_hash).into()
}

#[cfg(test)]
mod tests {
    use super::{generate, parse_prefix, verify};

    #[test]
    fn generated_key_parses_and_verifies() {
        let key = generate().unwrap();
        let full = key.full.expose().as_str();
        assert!(full.starts_with("hm_"));
        assert_eq!(parse_prefix(full), Some(key.prefix.as_str()));
        assert!(verify(full, &key.hash));
        assert!(!verify(&format!("{full}x"), &key.hash));
    }

    #[test]
    fn malformed_keys_have_no_prefix() {
        for bad in [
            "",
            "hm_",
            "hm_short_x",
            "xx_abcdefghijkl_secret",
            "hm_ABCDEFGHIJKL_secret",
            "hm_abcdefghijkl_",
        ] {
            assert_eq!(parse_prefix(bad), None, "{bad}");
        }
    }

    #[test]
    fn keys_are_unique() {
        assert_ne!(generate().unwrap().prefix, generate().unwrap().prefix);
    }
}
