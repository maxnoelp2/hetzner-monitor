//! Redaction wrapper for secret values (tokens, URLs with credentials, keys).
//!
//! `Secret<T>` never prints its content via `Debug` or `Display`. Access the value
//! explicitly with [`Secret::expose`] at the single place where it is needed.

use std::fmt;

/// A value that must never appear in logs, errors or API responses.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret<T>(T);

impl<T> Secret<T> {
    /// Wraps a secret value.
    pub const fn new(value: T) -> Self {
        Self(value)
    }

    /// Returns the secret value. Call this only where the value is actually used.
    pub const fn expose(&self) -> &T {
        &self.0
    }
}

impl<T> From<T> for Secret<T> {
    fn from(value: T) -> Self {
        Self::new(value)
    }
}

impl<T> fmt::Debug for Secret<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret([REDACTED])")
    }
}

impl<T> fmt::Display for Secret<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

#[cfg(test)]
mod tests {
    use super::Secret;

    #[test]
    fn debug_and_display_never_show_the_value() {
        let secret = Secret::new("postgres://user:hunter2@db/app".to_owned());
        assert_eq!(format!("{secret:?}"), "Secret([REDACTED])");
        assert_eq!(format!("{secret}"), "[REDACTED]");
        assert!(!format!("{secret:?}{secret}").contains("hunter2"));
    }

    #[test]
    fn expose_returns_the_value() {
        let secret = Secret::from(42);
        assert_eq!(*secret.expose(), 42);
    }
}
