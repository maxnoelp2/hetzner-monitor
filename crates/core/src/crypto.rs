//! Envelope encryption for secrets at rest (ADR-0005, `docs/design/security.md`).
//!
//! Each secret gets a random 256-bit data encryption key (DEK). The secret is sealed with the
//! DEK, the DEK is sealed with the instance master key. Both use AES-256-GCM with a random
//! 96-bit nonce prefixed to the ciphertext and the owning row's id as associated data, so a
//! ciphertext copied to another row fails to decrypt.

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use zeroize::{Zeroize, Zeroizing};

const KEY_LEN: usize = 32;
const NONCE_LEN: usize = 12;

/// Current master key version. Rotation (several versions) arrives in M5.
pub const CURRENT_KEY_VERSION: i32 = 1;

/// Errors from sealing or opening. Never contains key material or plaintext.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CryptoError {
    #[error("master key must be {KEY_LEN} bytes encoded as base64")]
    InvalidMasterKey,
    #[error("unsupported master key version {0}")]
    UnknownKeyVersion(i32),
    #[error("ciphertext is malformed")]
    Malformed,
    #[error("decryption failed (wrong key, wrong row or tampered data)")]
    DecryptFailed,
    #[error("system random number generator failed")]
    Random,
}

/// The instance master key. Zeroized on drop; never printed.
pub struct MasterKey {
    bytes: Zeroizing<[u8; KEY_LEN]>,
}

impl MasterKey {
    /// Parses a base64-encoded 32-byte key (surrounding whitespace is ignored).
    pub fn from_base64(encoded: &str) -> Result<Self, CryptoError> {
        let mut decoded = Zeroizing::new(
            STANDARD
                .decode(encoded.trim())
                .map_err(|_| CryptoError::InvalidMasterKey)?,
        );
        let bytes: [u8; KEY_LEN] = decoded
            .as_slice()
            .try_into()
            .map_err(|_| CryptoError::InvalidMasterKey)?;
        decoded.zeroize();
        Ok(Self {
            bytes: Zeroizing::new(bytes),
        })
    }

    /// Generates a new random master key and returns it base64-encoded.
    pub fn generate_base64() -> Result<Zeroizing<String>, CryptoError> {
        let key = random_bytes::<KEY_LEN>()?;
        Ok(Zeroizing::new(STANDARD.encode(key.as_slice())))
    }

    fn cipher(&self) -> Aes256Gcm {
        Aes256Gcm::new_from_slice(self.bytes.as_slice()).expect("key length is fixed")
    }
}

impl std::fmt::Debug for MasterKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MasterKey([REDACTED])")
    }
}

/// A sealed secret as stored in the database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sealed {
    /// `nonce || AES-256-GCM(DEK, secret, aad)`
    pub secret_ct: Vec<u8>,
    /// `nonce || AES-256-GCM(master, DEK, aad)`
    pub dek_ct: Vec<u8>,
    pub key_version: i32,
}

/// Seals `plaintext` for the row identified by `aad` (its UUID bytes).
pub fn seal(master: &MasterKey, aad: &[u8], plaintext: &[u8]) -> Result<Sealed, CryptoError> {
    let dek = random_bytes::<KEY_LEN>()?;
    let dek_cipher = Aes256Gcm::new_from_slice(dek.as_slice()).expect("key length is fixed");
    Ok(Sealed {
        secret_ct: encrypt(&dek_cipher, aad, plaintext)?,
        dek_ct: encrypt(&master.cipher(), aad, dek.as_slice())?,
        key_version: CURRENT_KEY_VERSION,
    })
}

/// Opens a sealed secret for the row identified by `aad`.
pub fn open(
    master: &MasterKey,
    aad: &[u8],
    sealed: &Sealed,
) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
    if sealed.key_version != CURRENT_KEY_VERSION {
        return Err(CryptoError::UnknownKeyVersion(sealed.key_version));
    }
    let dek = decrypt(&master.cipher(), aad, &sealed.dek_ct)?;
    let dek_cipher =
        Aes256Gcm::new_from_slice(dek.as_slice()).map_err(|_| CryptoError::Malformed)?;
    decrypt(&dek_cipher, aad, &sealed.secret_ct)
}

fn encrypt(cipher: &Aes256Gcm, aad: &[u8], plaintext: &[u8]) -> Result<Vec<u8>, CryptoError> {
    let nonce_bytes = random_bytes::<NONCE_LEN>()?;
    let nonce = Nonce::try_from(nonce_bytes.as_slice()).map_err(|_| CryptoError::Malformed)?;
    let ciphertext = cipher
        .encrypt(
            &nonce,
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|_| CryptoError::Malformed)?;
    let mut out = Vec::with_capacity(NONCE_LEN + ciphertext.len());
    out.extend_from_slice(nonce_bytes.as_slice());
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

fn decrypt(cipher: &Aes256Gcm, aad: &[u8], data: &[u8]) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
    if data.len() <= NONCE_LEN {
        return Err(CryptoError::Malformed);
    }
    let (nonce_bytes, ciphertext) = data.split_at(NONCE_LEN);
    let nonce = Nonce::try_from(nonce_bytes).map_err(|_| CryptoError::Malformed)?;
    cipher
        .decrypt(
            &nonce,
            Payload {
                msg: ciphertext,
                aad,
            },
        )
        .map(Zeroizing::new)
        .map_err(|_| CryptoError::DecryptFailed)
}

/// Fills a fixed-size buffer from the OS random number generator.
pub fn random_bytes<const N: usize>() -> Result<Zeroizing<[u8; N]>, CryptoError> {
    let mut buf = Zeroizing::new([0_u8; N]);
    getrandom::fill(buf.as_mut_slice()).map_err(|_| CryptoError::Random)?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::{CryptoError, MasterKey, open, seal};

    fn key() -> MasterKey {
        MasterKey::from_base64(&MasterKey::generate_base64().unwrap()).unwrap()
    }

    #[test]
    fn round_trip() {
        let master = key();
        let sealed = seal(&master, b"row-1", b"hcloud-token-value").unwrap();
        assert_eq!(
            open(&master, b"row-1", &sealed).unwrap().as_slice(),
            b"hcloud-token-value"
        );
    }

    #[test]
    fn ciphertext_does_not_contain_plaintext_and_is_randomised() {
        let master = key();
        let a = seal(&master, b"row-1", b"hcloud-token-value").unwrap();
        let b = seal(&master, b"row-1", b"hcloud-token-value").unwrap();
        assert!(!a.secret_ct.windows(5).any(|w| w == b"token"));
        assert_ne!(a.secret_ct, b.secret_ct);
        assert_ne!(a.dek_ct, b.dek_ct);
    }

    #[test]
    fn wrong_row_wrong_key_and_tampering_fail() {
        let master = key();
        let sealed = seal(&master, b"row-1", b"secret").unwrap();
        assert_eq!(
            open(&master, b"row-2", &sealed).unwrap_err(),
            CryptoError::DecryptFailed
        );
        assert_eq!(
            open(&key(), b"row-1", &sealed).unwrap_err(),
            CryptoError::DecryptFailed
        );

        let mut tampered = sealed.clone();
        let last = tampered.secret_ct.len() - 1;
        tampered.secret_ct[last] ^= 0x01;
        assert_eq!(
            open(&master, b"row-1", &tampered).unwrap_err(),
            CryptoError::DecryptFailed
        );

        let mut truncated = sealed.clone();
        truncated.dek_ct.truncate(4);
        assert_eq!(
            open(&master, b"row-1", &truncated).unwrap_err(),
            CryptoError::Malformed
        );

        let mut future = sealed;
        future.key_version = 2;
        assert_eq!(
            open(&master, b"row-1", &future).unwrap_err(),
            CryptoError::UnknownKeyVersion(2)
        );
    }

    #[test]
    fn master_key_validation_and_redaction() {
        assert_eq!(
            MasterKey::from_base64("not base64!").unwrap_err(),
            CryptoError::InvalidMasterKey
        );
        assert_eq!(
            MasterKey::from_base64("c2hvcnQ=").unwrap_err(),
            CryptoError::InvalidMasterKey
        );
        let encoded = MasterKey::generate_base64().unwrap();
        let master = MasterKey::from_base64(&format!("  {}\n", encoded.as_str())).unwrap();
        assert_eq!(format!("{master:?}"), "MasterKey([REDACTED])");
    }
}
