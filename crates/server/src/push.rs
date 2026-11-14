//! Web Push setup (ADR-0017): the installation's VAPID key pair, generated on first use and
//! stored sealed in `vapid_keys` (AAD = row id), and the `sub` contact from the configuration.

use std::sync::{Arc, OnceLock};

use hm_core::crypto::{self, Keyring, Sealed};
use hm_notify::push::Vapid;
use sqlx::PgPool;
use uuid::Uuid;

/// Web Push state shared by the API and the notifier. Cheap to clone.
#[derive(Clone, Debug, Default)]
pub struct PushState {
    /// `HM_VAPID_SUBJECT` or an `https://` `HM_BASE_URL`; `None` = Web Push unavailable.
    subject: Option<Arc<str>>,
    vapid: Arc<OnceLock<Arc<Vapid>>>,
}

#[derive(Debug, thiserror::Error)]
pub enum PushSetupError {
    #[error("web push is not configured (set HM_VAPID_SUBJECT or an https:// HM_BASE_URL)")]
    Disabled,
    #[error("the stored VAPID key cannot be opened ({0})")]
    Crypto(#[from] crypto::CryptoError),
    #[error("the stored VAPID key is invalid")]
    InvalidKey,
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

impl PushState {
    pub fn new(subject: Option<&str>) -> Self {
        Self {
            subject: subject.map(Arc::from),
            vapid: Arc::default(),
        }
    }

    /// Whether Web Push can be used.
    pub const fn enabled(&self) -> bool {
        self.subject.is_some()
    }

    /// The VAPID key pair: loaded (or created) once, then cached.
    pub async fn vapid(&self, db: &PgPool, keys: &Keyring) -> Result<Arc<Vapid>, PushSetupError> {
        let subject = self.subject.as_deref().ok_or(PushSetupError::Disabled)?;
        if let Some(v) = self.vapid.get() {
            return Ok(v.clone());
        }
        let vapid = Arc::new(load_or_create(db, keys, subject).await?);
        // Concurrent first calls load the same row; keep whichever was stored first.
        Ok(self.vapid.get_or_init(|| vapid).clone())
    }
}

type Row = (Uuid, Vec<u8>, Vec<u8>, i32);

async fn load_or_create(
    db: &PgPool,
    keys: &Keyring,
    subject: &str,
) -> Result<Vapid, PushSetupError> {
    if let Some(row) = load(db).await? {
        return open(keys, row, subject);
    }
    let fresh = Vapid::generate(subject).map_err(|_| PushSetupError::InvalidKey)?;
    let id = Uuid::new_v4();
    let sealed = crypto::seal(keys, id.as_bytes(), &fresh.private_bytes())?;
    // Another instance may have won the race: then its row is used.
    let inserted = sqlx::query(
        "INSERT INTO vapid_keys (id, private_ct, private_dek_ct, key_version, public_key)
         VALUES ($1, $2, $3, $4, $5) ON CONFLICT (singleton) DO NOTHING",
    )
    .bind(id)
    .bind(&sealed.secret_ct)
    .bind(&sealed.dek_ct)
    .bind(sealed.key_version)
    .bind(fresh.public_key())
    .execute(db)
    .await?
    .rows_affected();
    if inserted == 1 {
        tracing::info!("VAPID key pair for Web Push created");
        return Ok(fresh);
    }
    let row = load(db).await?.ok_or(PushSetupError::InvalidKey)?;
    open(keys, row, subject)
}

async fn load(db: &PgPool) -> Result<Option<Row>, sqlx::Error> {
    sqlx::query_as("SELECT id, private_ct, private_dek_ct, key_version FROM vapid_keys")
        .fetch_optional(db)
        .await
}

fn open(
    keys: &Keyring,
    (id, secret_ct, dek_ct, key_version): Row,
    subject: &str,
) -> Result<Vapid, PushSetupError> {
    let private = crypto::open(
        keys,
        id.as_bytes(),
        &Sealed {
            secret_ct,
            dek_ct,
            key_version,
        },
    )?;
    Vapid::from_private(&private, subject).map_err(|_| PushSetupError::InvalidKey)
}
