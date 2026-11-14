//! Web Push (RFC 8030) with message encryption (RFC 8291, `aes128gcm`) and VAPID (RFC 8292),
//! implemented on the `RustCrypto` crates (ADR-0017): the `web-push` crate needs OpenSSL.
//!
//! A [`Subscription`] is what the browser's `PushManager.subscribe()` returns: an endpoint URL
//! (a capability: whoever knows it can push to the device) and the keys `p256dh` and `auth`.
//! It is a secret and `Debug` never shows it.

use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes128Gcm, Nonce};
use base64::Engine as _;
use base64::alphabet::URL_SAFE;
use base64::engine::GeneralPurpose;
use base64::engine::general_purpose::NO_PAD_INDIFFERENT;
use hkdf::Hkdf;
use p256::ecdsa::signature::Signer;
use p256::ecdsa::{Signature, SigningKey};
use p256::elliptic_curve::sec1::ToSec1Point;
use p256::{PublicKey, SecretKey};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use zeroize::Zeroizing;

use crate::payload::{Event, Payload, Severity};

/// base64url without padding; decoding also accepts padding (browsers differ).
pub const B64: GeneralPurpose = GeneralPurpose::new(&URL_SAFE, NO_PAD_INDIFFERENT);

/// Record size announced in the `aes128gcm` header (one record per message).
const RECORD_SIZE: u32 = 4096;
/// Largest plaintext we send. Push services accept at least 4096 bytes of encrypted body
/// (RFC 8030 §7.2); the header and tag take 103 bytes.
pub const MAX_PLAINTEXT: usize = 3072;
/// How long a VAPID token is valid (RFC 8292 allows up to 24 h).
const VAPID_TTL_S: u64 = 12 * 3600;

/// Invalid key material or subscription. Never contains the values.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct PushError(pub String);

fn err(msg: &str) -> PushError {
    PushError(msg.to_owned())
}

/// The installation's VAPID key pair and its `sub` contact (`mailto:` or `https:` URL).
#[derive(Clone)]
pub struct Vapid {
    key: SigningKey,
    public: Vec<u8>,
    subject: String,
}

impl fmt::Debug for Vapid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Vapid")
            .field("public_key", &self.public_key())
            .field("subject", &self.subject)
            .finish_non_exhaustive()
    }
}

impl Vapid {
    /// A new random key pair.
    pub fn generate(subject: &str) -> Result<Self, PushError> {
        Self::from_private(&random_secret()?.to_bytes(), subject)
    }

    /// From the 32-byte private scalar (as stored, sealed, in the database).
    pub fn from_private(private: &[u8], subject: &str) -> Result<Self, PushError> {
        let secret = SecretKey::from_slice(private).map_err(|_| err("invalid VAPID key"))?;
        let public = uncompressed(&secret.public_key());
        Ok(Self {
            key: SigningKey::from(secret),
            public,
            subject: subject.to_owned(),
        })
    }

    /// The private scalar, for sealing it.
    pub fn private_bytes(&self) -> Zeroizing<Vec<u8>> {
        Zeroizing::new(self.key.to_bytes().to_vec())
    }

    /// The public key (uncompressed point, base64url): `applicationServerKey` in the browser.
    pub fn public_key(&self) -> String {
        B64.encode(&self.public)
    }

    /// `Authorization: vapid t=<JWT>, k=<public key>` for a request to `endpoint` (RFC 8292).
    pub fn authorization(&self, endpoint: &Url, now_unix: u64) -> String {
        let header = B64.encode(br#"{"typ":"JWT","alg":"ES256"}"#);
        let claims = serde_json::json!({
            "aud": endpoint.origin().ascii_serialization(),
            "exp": now_unix + VAPID_TTL_S,
            "sub": self.subject,
        });
        let claims = B64.encode(claims.to_string());
        let signing_input = format!("{header}.{claims}");
        let signature: Signature = self.key.sign(signing_input.as_bytes());
        format!(
            "vapid t={signing_input}.{}, k={}",
            B64.encode(signature.to_bytes()),
            self.public_key()
        )
    }
}

/// A push subscription of one browser. Stored sealed (it is a secret).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Subscription {
    pub endpoint: String,
    /// The browser's P-256 public key, base64url.
    pub p256dh: String,
    /// The 16-byte authentication secret, base64url.
    pub auth: String,
}

impl fmt::Debug for Subscription {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Subscription([REDACTED])")
    }
}

impl Subscription {
    /// Checks the keys and that the endpoint is an `https://` URL with a host (`http://` only
    /// with `allow_http`, for local test push services).
    pub fn validate(&self, allow_http: bool) -> Result<(), PushError> {
        let url = Url::parse(&self.endpoint).map_err(|_| err("`endpoint` is not a URL"))?;
        let scheme_ok = url.scheme() == "https" || (allow_http && url.scheme() == "http");
        if !scheme_ok || url.host_str().is_none() || self.endpoint.len() > 2048 {
            return Err(err("`endpoint` must be an https URL with a host"));
        }
        self.keys()?;
        Ok(())
    }

    /// SHA-256 of the endpoint, hex: identifies a subscription without storing the endpoint
    /// in plain text.
    pub fn endpoint_hash(&self) -> String {
        use std::fmt::Write as _;

        use sha2::Digest as _;
        Sha256::digest(self.endpoint.as_bytes()).iter().fold(
            String::with_capacity(64),
            |mut hex, b| {
                let _ = write!(hex, "{b:02x}");
                hex
            },
        )
    }

    /// The push service's host (shown in the API instead of the endpoint).
    pub fn host(&self) -> String {
        Url::parse(&self.endpoint)
            .ok()
            .and_then(|u| u.host_str().map(str::to_owned))
            .unwrap_or_default()
    }

    fn keys(&self) -> Result<(PublicKey, [u8; 16]), PushError> {
        let p256dh = B64
            .decode(self.p256dh.trim())
            .map_err(|_| err("`keys.p256dh` is not base64url"))?;
        let public = PublicKey::from_sec1_bytes(&p256dh)
            .map_err(|_| err("`keys.p256dh` is not a P-256 public key"))?;
        let auth: [u8; 16] = B64
            .decode(self.auth.trim())
            .ok()
            .and_then(|a| a.try_into().ok())
            .ok_or_else(|| err("`keys.auth` must be 16 bytes, base64url"))?;
        Ok((public, auth))
    }
}

/// Encrypts `plaintext` for `subscription` (RFC 8291) with a fresh key pair and salt.
pub fn encrypt(subscription: &Subscription, plaintext: &[u8]) -> Result<Vec<u8>, PushError> {
    let (ua_public, auth) = subscription.keys()?;
    let mut salt = [0u8; 16];
    getrandom::fill(&mut salt).map_err(|_| err("random number generator failed"))?;
    encrypt_with(&ua_public, &auth, plaintext, &random_secret()?, &salt)
}

fn encrypt_with(
    ua_public: &PublicKey,
    auth: &[u8; 16],
    plaintext: &[u8],
    as_secret: &SecretKey,
    salt: &[u8; 16],
) -> Result<Vec<u8>, PushError> {
    if plaintext.len() > MAX_PLAINTEXT {
        return Err(err("push message too large"));
    }
    let ua = uncompressed(ua_public);
    let as_public = uncompressed(&as_secret.public_key());
    let shared = as_secret.diffie_hellman(ua_public);

    // IKM = HKDF(auth_secret, ecdh_secret, "WebPush: info" || 0x00 || ua_public || as_public, 32)
    let mut key_info = Vec::with_capacity(14 + 130);
    key_info.extend_from_slice(b"WebPush: info\0");
    key_info.extend_from_slice(&ua);
    key_info.extend_from_slice(&as_public);
    let mut ikm = Zeroizing::new([0u8; 32]);
    Hkdf::<Sha256>::new(Some(auth), shared.raw_secret_bytes())
        .expand(&key_info, ikm.as_mut_slice())
        .map_err(|_| err("key derivation failed"))?;

    let prk = Hkdf::<Sha256>::new(Some(salt), ikm.as_slice());
    let mut cek = Zeroizing::new([0u8; 16]);
    let mut nonce = [0u8; 12];
    prk.expand(b"Content-Encoding: aes128gcm\0", cek.as_mut_slice())
        .and_then(|()| prk.expand(b"Content-Encoding: nonce\0", &mut nonce))
        .map_err(|_| err("key derivation failed"))?;

    // One record: plaintext || 0x02 (last-record delimiter), no further padding.
    let mut record = Zeroizing::new(Vec::with_capacity(plaintext.len() + 1));
    record.extend_from_slice(plaintext);
    record.push(0x02);
    let ciphertext = Aes128Gcm::new_from_slice(cek.as_slice())
        .map_err(|_| err("cipher setup failed"))?
        .encrypt(&Nonce::from(nonce), record.as_slice())
        .map_err(|_| err("encryption failed"))?;

    // Header: salt (16) || rs (u32 BE) || idlen (1) || keyid (as_public, 65).
    let mut out = Vec::with_capacity(21 + as_public.len() + ciphertext.len());
    out.extend_from_slice(salt);
    out.extend_from_slice(&RECORD_SIZE.to_be_bytes());
    out.push(u8::try_from(as_public.len()).expect("65-byte point"));
    out.extend_from_slice(&as_public);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

fn uncompressed(key: &PublicKey) -> Vec<u8> {
    key.to_sec1_point(false).as_bytes().to_vec()
}

fn random_secret() -> Result<SecretKey, PushError> {
    // A random 32-byte string is a valid scalar except with negligible probability; retry then.
    for _ in 0..4 {
        let mut bytes = Zeroizing::new([0u8; 32]);
        getrandom::fill(bytes.as_mut_slice()).map_err(|_| err("random number generator failed"))?;
        if let Ok(key) = SecretKey::from_slice(bytes.as_slice()) {
            return Ok(key);
        }
    }
    Err(err("could not generate a key"))
}

/// The JSON the service worker receives (`web/public/sw.js`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Message {
    pub title: String,
    pub body: String,
    /// Groups notifications of one alert: a newer one replaces the older.
    pub tag: String,
    /// Path to open on click (same origin as the UI), e.g. `/alerts/<id>`.
    pub url: String,
    pub event: Event,
    pub severity: Severity,
}

impl Message {
    pub fn from_payload(payload: &Payload) -> Self {
        let url = if payload.event == Event::Test {
            "/settings".to_owned()
        } else {
            format!("/alerts/{}", payload.alert.id)
        };
        let mut body = payload.alert.message.clone();
        if payload.event != Event::Test {
            body.push_str("\nProject: ");
            body.push_str(&payload.project.name);
        }
        Self {
            title: payload.title(),
            body,
            tag: format!("hm-alert-{}", payload.alert.id),
            url,
            event: payload.event,
            severity: payload.alert.severity,
        }
    }

    /// Serialised, shortening the body (then the title) to stay within [`MAX_PLAINTEXT`].
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut message = self.clone();
        loop {
            let bytes = serde_json::to_vec(&message).unwrap_or_default();
            if bytes.len() <= MAX_PLAINTEXT {
                return bytes;
            }
            let excess = bytes.len() - MAX_PLAINTEXT + 3;
            let field = if message.body.len() > 3 {
                &mut message.body
            } else {
                &mut message.title
            };
            let mut keep = field.len().saturating_sub(excess);
            while !field.is_char_boundary(keep) {
                keep -= 1;
            }
            field.truncate(keep);
            field.push('…');
        }
    }
}

/// `Urgency` header (RFC 8030 §5.3) by event and severity.
pub const fn urgency(payload: &Payload) -> &'static str {
    match (payload.event, payload.alert.severity) {
        (Event::Test, _) | (_, Severity::Critical) => "high",
        (Event::Resolved, _) | (_, Severity::Warning) => "normal",
        (_, Severity::Info) => "low",
    }
}

/// `TTL` header in seconds: how long the push service keeps an undelivered message.
pub const fn ttl(payload: &Payload) -> u32 {
    match (payload.event, payload.alert.severity) {
        (Event::Test, _) => 600,
        (_, Severity::Critical) => 86_400,
        (_, Severity::Warning) => 43_200,
        (_, Severity::Info) => 3600,
    }
}

/// Seconds since the Unix epoch.
pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use p256::ecdsa::VerifyingKey;
    use p256::ecdsa::signature::Verifier;

    use super::*;
    use crate::payload::tests::sample;

    fn b64(s: &str) -> Vec<u8> {
        B64.decode(s.replace([' ', '\n'], "")).unwrap()
    }

    /// RFC 8291, Section 5 and Appendix A.
    #[test]
    fn rfc8291_test_vector() {
        let plaintext = b64("V2hlbiBJIGdyb3cgdXAsIEkgd2FudCB0byBiZSBhIHdhdGVybWVsb24");
        let as_secret =
            SecretKey::from_slice(&b64("yfWPiYE-n46HLnH0KqZOF1fJJU3MYrct3AELtAQ-oRw")).unwrap();
        let ua_public = PublicKey::from_sec1_bytes(&b64(
            "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4",
        ))
        .unwrap();
        let salt: [u8; 16] = b64("DGv6ra1nlYgDCS1FRnbzlw").try_into().unwrap();
        let auth: [u8; 16] = b64("BTBZMqHH6r4Tts7J_aSIgg").try_into().unwrap();
        let out = encrypt_with(&ua_public, &auth, &plaintext, &as_secret, &salt).unwrap();
        let expected = b64(
            "DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27ml\
             mlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A_yl95bQpu6cVPT\
             pK4Mqgkf1CXztLVBSt2Ks3oZwbuwXPXLWyouBWLVWGNWQexSgSxsj_Qulcy4a-fN",
        );
        assert_eq!(B64.encode(&out), B64.encode(&expected));
    }

    #[test]
    fn subscription_validation_never_echoes_values() {
        let good = Subscription {
            endpoint: "https://push.example.net/send/abc".to_owned(),
            p256dh: "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4".to_owned(),
            auth: "BTBZMqHH6r4Tts7J_aSIgg==".to_owned(),
        };
        good.validate(false).unwrap();
        assert_eq!(good.host(), "push.example.net");
        assert_eq!(good.endpoint_hash().len(), 64);
        assert_eq!(format!("{good:?}"), "Subscription([REDACTED])");

        let http = Subscription {
            endpoint: "http://127.0.0.1:9/sekrit".to_owned(),
            ..good.clone()
        };
        assert!(http.validate(false).is_err());
        http.validate(true).unwrap();
        for bad in [
            Subscription {
                endpoint: "not a url sekrit".to_owned(),
                ..good.clone()
            },
            Subscription {
                p256dh: "c2Vrcml0".to_owned(),
                ..good.clone()
            },
            Subscription {
                auth: "c2Vrcml0".to_owned(),
                ..good.clone()
            },
        ] {
            let e = bad.validate(false).unwrap_err();
            assert!(!e.0.contains("sekrit") && !e.0.contains("c2Vy"), "{e}");
        }
    }

    #[test]
    fn vapid_token_is_a_valid_es256_jwt() {
        let vapid = Vapid::generate("mailto:ops@example.com").unwrap();
        let again = Vapid::from_private(&vapid.private_bytes(), "mailto:ops@example.com").unwrap();
        assert_eq!(vapid.public_key(), again.public_key());
        assert_eq!(B64.decode(vapid.public_key()).unwrap().len(), 65);

        let endpoint = Url::parse("https://push.example.net:8443/send/abc").unwrap();
        let header = vapid.authorization(&endpoint, 1_000);
        let rest = header.strip_prefix("vapid t=").unwrap();
        let (token, k) = rest.split_once(", k=").unwrap();
        assert_eq!(k, vapid.public_key());
        let mut parts = token.split('.');
        let (h, c, s) = (
            parts.next().unwrap(),
            parts.next().unwrap(),
            parts.next().unwrap(),
        );
        let claims: serde_json::Value = serde_json::from_slice(&B64.decode(c).unwrap()).unwrap();
        assert_eq!(claims["aud"], "https://push.example.net:8443");
        assert_eq!(claims["exp"], 1_000 + VAPID_TTL_S);
        assert_eq!(claims["sub"], "mailto:ops@example.com");
        let key = VerifyingKey::from_sec1_bytes(&B64.decode(k).unwrap()).unwrap();
        let sig = Signature::from_slice(&B64.decode(s).unwrap()).unwrap();
        key.verify(format!("{h}.{c}").as_bytes(), &sig).unwrap();
    }

    #[test]
    fn message_links_to_the_alert_and_stays_small() {
        let p = sample(Event::Fired);
        let m = Message::from_payload(&p);
        assert_eq!(m.url, "/alerts/a1");
        assert_eq!(m.tag, "hm-alert-a1");
        assert_eq!(m.body, "server web-1 (1) is off\nProject: prod");
        assert_eq!(urgency(&p), "high");
        assert_eq!(ttl(&p), 86_400);

        let mut long = p.clone();
        long.alert.message = "ä".repeat(5000);
        let bytes = Message::from_payload(&long).to_bytes();
        assert!(bytes.len() <= MAX_PLAINTEXT);
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(v["body"].as_str().unwrap().ends_with('…'));
    }

    #[test]
    fn encrypt_rejects_oversized_messages() {
        let sub = Subscription {
            endpoint: "https://push.example.net/x".to_owned(),
            p256dh: "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4".to_owned(),
            auth: "BTBZMqHH6r4Tts7J_aSIgg".to_owned(),
        };
        assert_eq!(encrypt(&sub, &[0; 100]).unwrap().len(), 86 + 101 + 16);
        assert!(encrypt(&sub, &[0; MAX_PLAINTEXT + 1]).is_err());
    }
}
