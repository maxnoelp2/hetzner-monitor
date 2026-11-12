//! Envelope encryption for secrets at rest (ADR-0005, `docs/design/security.md`).
//!
//! Each secret gets a random 256-bit data encryption key (DEK). The secret is sealed with the
//! DEK, the DEK is sealed with the instance master key. Both use AES-256-GCM with a random
//! 96-bit nonce prefixed to the ciphertext and the owning row's id as associated data, so a
//! ciphertext copied to another row fails to decrypt.
//!
//! Master keys are versioned ([`Keyring`]): new secrets are sealed with the newest version, every
//! configured version can open, and [`rewrap`] moves a DEK to the newest version without touching
//! the secret itself (key rotation, ADR-0013).

use std::collections::BTreeMap;

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use zeroize::{Zeroize, Zeroizing};

const KEY_LEN: usize = 32;
const NONCE_LEN: usize = 12;

/// Version a single `HM_MASTER_KEY` is stored under.
pub const DEFAULT_KEY_VERSION: i32 = 1;
/// Highest allowed master key version (fits a Postgres `integer` comfortably, keeps lists short).
pub const MAX_KEY_VERSION: i32 = 32767;

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
    #[error("a keyring needs at least one master key")]
    EmptyKeyring,
    #[error("master key version {0} is invalid or listed twice (allowed: 1–32767, each once)")]
    InvalidKeyVersion(i32),
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

/// The configured master keys by version. The highest version is the current one.
pub struct Keyring {
    keys: BTreeMap<i32, MasterKey>,
}

impl Keyring {
    /// A keyring with one key under [`DEFAULT_KEY_VERSION`].
    #[must_use]
    pub fn single(key: MasterKey) -> Self {
        Self {
            keys: BTreeMap::from([(DEFAULT_KEY_VERSION, key)]),
        }
    }

    /// Builds a keyring from `(version, key)` pairs. Versions must be unique and in range.
    pub fn new(keys: impl IntoIterator<Item = (i32, MasterKey)>) -> Result<Self, CryptoError> {
        let mut map = BTreeMap::new();
        for (version, key) in keys {
            if !(1..=MAX_KEY_VERSION).contains(&version) || map.insert(version, key).is_some() {
                return Err(CryptoError::InvalidKeyVersion(version));
            }
        }
        if map.is_empty() {
            return Err(CryptoError::EmptyKeyring);
        }
        Ok(Self { keys: map })
    }

    /// Parses `version:base64,version:base64,…` (whitespace around items is ignored).
    pub fn parse(list: &str) -> Result<Self, CryptoError> {
        let mut keys = Vec::new();
        for item in list.split(',').map(str::trim).filter(|i| !i.is_empty()) {
            let (version, encoded) = item.split_once(':').ok_or(CryptoError::InvalidMasterKey)?;
            let version: i32 = version
                .trim()
                .parse()
                .map_err(|_| CryptoError::InvalidKeyVersion(0))?;
            keys.push((version, MasterKey::from_base64(encoded)?));
        }
        Self::new(keys)
    }

    /// The newest version; new secrets are sealed with it.
    #[must_use]
    pub fn current_version(&self) -> i32 {
        *self
            .keys
            .keys()
            .next_back()
            .expect("keyring is never empty")
    }

    /// Whether `version` can be opened with this keyring.
    #[must_use]
    pub fn contains(&self, version: i32) -> bool {
        self.keys.contains_key(&version)
    }

    /// All configured versions, ascending.
    pub fn versions(&self) -> impl Iterator<Item = i32> + '_ {
        self.keys.keys().copied()
    }

    fn current(&self) -> (i32, &MasterKey) {
        let (v, k) = self
            .keys
            .iter()
            .next_back()
            .expect("keyring is never empty");
        (*v, k)
    }

    fn get(&self, version: i32) -> Result<&MasterKey, CryptoError> {
        self.keys
            .get(&version)
            .ok_or(CryptoError::UnknownKeyVersion(version))
    }
}

impl std::fmt::Debug for Keyring {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Keyring")
            .field("versions", &self.keys.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
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

/// Seals `plaintext` for the row identified by `aad` (its UUID bytes) with the current key.
pub fn seal(keys: &Keyring, aad: &[u8], plaintext: &[u8]) -> Result<Sealed, CryptoError> {
    let (version, master) = keys.current();
    let dek = random_bytes::<KEY_LEN>()?;
    let dek_cipher = Aes256Gcm::new_from_slice(dek.as_slice()).expect("key length is fixed");
    Ok(Sealed {
        secret_ct: encrypt(&dek_cipher, aad, plaintext)?,
        dek_ct: encrypt(&master.cipher(), aad, dek.as_slice())?,
        key_version: version,
    })
}

/// Opens a sealed secret for the row identified by `aad` with the key of its version.
pub fn open(
    keys: &Keyring,
    aad: &[u8],
    sealed: &Sealed,
) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
    let dek = decrypt(&keys.get(sealed.key_version)?.cipher(), aad, &sealed.dek_ct)?;
    let dek_cipher =
        Aes256Gcm::new_from_slice(dek.as_slice()).map_err(|_| CryptoError::Malformed)?;
    decrypt(&dek_cipher, aad, &sealed.secret_ct)
}

/// Re-wraps the DEK of `sealed` with the current key. The secret ciphertext is unchanged and
/// never decrypted. Returns `None` if the row already uses the current version.
pub fn rewrap(keys: &Keyring, aad: &[u8], sealed: &Sealed) -> Result<Option<Sealed>, CryptoError> {
    let (version, master) = keys.current();
    if sealed.key_version == version {
        return Ok(None);
    }
    let dek = decrypt(&keys.get(sealed.key_version)?.cipher(), aad, &sealed.dek_ct)?;
    Ok(Some(Sealed {
        secret_ct: sealed.secret_ct.clone(),
        dek_ct: encrypt(&master.cipher(), aad, dek.as_slice())?,
        key_version: version,
    }))
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
    use super::{CryptoError, Keyring, MasterKey, open, rewrap, seal};

    fn master() -> MasterKey {
        MasterKey::from_base64(&MasterKey::generate_base64().unwrap()).unwrap()
    }

    fn key() -> Keyring {
        Keyring::single(master())
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

    #[test]
    fn keyring_parsing() {
        let a = MasterKey::generate_base64().unwrap();
        let b = MasterKey::generate_base64().unwrap();
        let ring = Keyring::parse(&format!(" 2:{} , 1:{}, ", b.as_str(), a.as_str())).unwrap();
        assert_eq!(ring.current_version(), 2);
        assert_eq!(ring.versions().collect::<Vec<_>>(), [1, 2]);
        assert!(!format!("{ring:?}").contains(a.as_str()));

        let dup = format!("1:{},1:{}", a.as_str(), b.as_str());
        assert_eq!(
            Keyring::parse(&dup).unwrap_err(),
            CryptoError::InvalidKeyVersion(1)
        );
        for bad in ["", " , "] {
            assert_eq!(Keyring::parse(bad).unwrap_err(), CryptoError::EmptyKeyring);
        }
        assert_eq!(
            Keyring::parse(a.as_str()).unwrap_err(),
            CryptoError::InvalidMasterKey
        );
        assert_eq!(
            Keyring::parse(&format!("0:{}", a.as_str())).unwrap_err(),
            CryptoError::InvalidKeyVersion(0)
        );
        assert_eq!(
            Keyring::parse(&format!("x:{}", a.as_str())).unwrap_err(),
            CryptoError::InvalidKeyVersion(0)
        );
        assert_eq!(
            Keyring::parse("1:c2hvcnQ=").unwrap_err(),
            CryptoError::InvalidMasterKey
        );
    }

    #[test]
    fn rotation_rewraps_only_the_dek() {
        let one = MasterKey::generate_base64().unwrap();
        let two = MasterKey::generate_base64().unwrap();
        let old = Keyring::parse(&format!("1:{}", one.as_str())).unwrap();
        let both = Keyring::parse(&format!("1:{},2:{}", one.as_str(), two.as_str())).unwrap();
        let new = Keyring::parse(&format!("2:{}", two.as_str())).unwrap();

        let sealed = seal(&old, b"row-1", b"secret").unwrap();
        assert_eq!(sealed.key_version, 1);
        // The new keyring still opens version 1 and seals new data with version 2.
        assert_eq!(
            open(&both, b"row-1", &sealed).unwrap().as_slice(),
            b"secret"
        );
        assert_eq!(seal(&both, b"row-2", b"x").unwrap().key_version, 2);

        let rotated = rewrap(&both, b"row-1", &sealed).unwrap().unwrap();
        assert_eq!(rotated.key_version, 2);
        assert_eq!(rotated.secret_ct, sealed.secret_ct);
        assert_ne!(rotated.dek_ct, sealed.dek_ct);
        assert_eq!(rewrap(&both, b"row-1", &rotated).unwrap(), None);
        // After rotation the old key can be dropped.
        assert_eq!(
            open(&new, b"row-1", &rotated).unwrap().as_slice(),
            b"secret"
        );
        assert_eq!(
            open(&new, b"row-1", &sealed).unwrap_err(),
            CryptoError::UnknownKeyVersion(1)
        );
        // Re-wrapping checks the AAD too.
        assert_eq!(
            rewrap(&both, b"row-2", &sealed).unwrap_err(),
            CryptoError::DecryptFailed
        );
    }
}
