//! Session and CSRF tokens (ADR-0007, ADR-0013).
//!
//! The session token is 32 random bytes in base64url; only its SHA-256 hash is stored, so a
//! database dump cannot be replayed as a login. The CSRF token is derived from the session
//! token ([`csrf_token`]): nothing extra is stored, every tab of a session gets the same
//! value, and a page on another origin cannot compute it without reading the cookie.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

use crate::crypto::{CryptoError, random_bytes};
use crate::secret::Secret;

/// Name of the session cookie.
pub const COOKIE: &str = "hm_session";
/// Header carrying the CSRF token on state-changing cookie requests.
pub const CSRF_HEADER: &str = "x-csrf-token";

/// A freshly generated token and its hash.
pub struct NewToken {
    pub token: Secret<Zeroizing<String>>,
    pub hash: Vec<u8>,
}

/// Generates a random token.
pub fn generate() -> Result<NewToken, CryptoError> {
    let bytes = random_bytes::<32>()?;
    let token = Zeroizing::new(URL_SAFE_NO_PAD.encode(bytes.as_slice()));
    let hash = hash(&token);
    Ok(NewToken {
        token: Secret::new(token),
        hash,
    })
}

/// SHA-256 of a token.
#[must_use]
pub fn hash(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

/// The CSRF token of a session: `base64url(SHA-256("hm-csrf:" || session token))`.
#[must_use]
pub fn csrf_token(session_token: &str) -> Secret<String> {
    let mut h = Sha256::new();
    h.update(b"hm-csrf:");
    h.update(session_token.as_bytes());
    Secret::new(URL_SAFE_NO_PAD.encode(h.finalize()))
}

/// Constant-time check of a presented CSRF token against the session token.
#[must_use]
pub fn verify_csrf(presented: &str, session_token: &str) -> bool {
    let expected = csrf_token(session_token);
    presented
        .as_bytes()
        .ct_eq(expected.expose().as_bytes())
        .into()
}

/// Constant-time comparison of a presented token against a stored hash.
#[must_use]
pub fn verify(presented: &str, stored_hash: &[u8]) -> bool {
    hash(presented).ct_eq(stored_hash).into()
}

/// Whether a presented value has the shape of a token (43 base64url characters).
#[must_use]
pub fn well_formed(presented: &str) -> bool {
    presented.len() == 43
        && presented
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Finds the value of cookie `name` in a `Cookie` header (`a=1; b=2`).
#[must_use]
pub fn cookie_value<'a>(header: &'a str, name: &str) -> Option<&'a str> {
    header.split(';').find_map(|pair| {
        let (k, v) = pair.trim().split_once('=')?;
        (k.trim() == name).then(|| v.trim().trim_matches('"'))
    })
}

#[cfg(test)]
mod tests {
    use super::{cookie_value, csrf_token, generate, verify, verify_csrf, well_formed};

    #[test]
    fn tokens() {
        let t = generate().unwrap();
        let s = t.token.expose().as_str();
        assert!(well_formed(s));
        assert!(verify(s, &t.hash));
        assert!(!verify("other", &t.hash));
        assert!(!well_formed("short"));
        assert!(!well_formed(&format!("{}!", &s[..42])));
        assert_ne!(generate().unwrap().hash, t.hash);

        let csrf = csrf_token(s);
        assert!(well_formed(csrf.expose()));
        assert_ne!(csrf.expose(), s);
        assert!(verify_csrf(csrf.expose(), s));
        assert!(!verify_csrf(
            csrf.expose(),
            generate().unwrap().token.expose()
        ));
        assert!(!verify_csrf("", s));
    }

    #[test]
    fn cookies() {
        let h = "theme=dark; hm_session=abc_-1 ;other=x=y; empty";
        assert_eq!(cookie_value(h, "hm_session"), Some("abc_-1"));
        assert_eq!(cookie_value(h, "other"), Some("x=y"));
        assert_eq!(cookie_value(h, "missing"), None);
        assert_eq!(cookie_value("hm_session=\"q\"", "hm_session"), Some("q"));
        assert_eq!(cookie_value("xhm_session=1", "hm_session"), None);
    }
}
