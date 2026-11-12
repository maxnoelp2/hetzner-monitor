//! Master-key rotation (ADR-0013): status, re-wrapping DEKs to the newest key version, and the
//! startup check that every stored version is configured.
//!
//! These are operator functions over **all tenants** (like the scheduler's claim): they never
//! read or return secrets, only re-wrap DEKs in place.

use std::collections::BTreeMap;

use hm_core::crypto::{self, Keyring, Sealed};
use sqlx::PgPool;
use uuid::Uuid;

/// Rows re-wrapped per transaction.
const BATCH: i64 = 100;

#[derive(Debug, thiserror::Error)]
pub enum KeysError {
    #[error(
        "the database holds secrets sealed with master key version(s) {0:?}, which are not configured; \
         add them to HM_MASTER_KEYS (format `1:<key>,2:<key>`)"
    )]
    MissingVersions(Vec<i32>),
    #[error(
        "a {table} row could not be re-wrapped ({error}); is the right key configured for its version?"
    )]
    Rewrap {
        table: &'static str,
        error: crypto::CryptoError,
    },
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

/// Number of sealed rows per table and key version.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Status {
    /// `(table, version) → rows`
    pub rows: BTreeMap<(&'static str, i32), i64>,
}

impl Status {
    /// All versions in use.
    pub fn versions(&self) -> impl Iterator<Item = i32> + '_ {
        let mut v: Vec<i32> = self.rows.keys().map(|(_, v)| *v).collect();
        v.sort_unstable();
        v.dedup();
        v.into_iter()
    }
}

/// Counts sealed rows per table and key version.
pub async fn status(db: &PgPool) -> Result<Status, sqlx::Error> {
    let rows: Vec<(String, i32, i64)> = sqlx::query_as(
        "SELECT 'hetzner_projects', key_version, count(*) FROM hetzner_projects GROUP BY key_version
         UNION ALL
         SELECT 'notification_channels', key_version, count(*) FROM notification_channels GROUP BY key_version",
    )
    .fetch_all(db)
    .await?;
    let mut status = Status::default();
    for (table, version, count) in rows {
        let table = if table == "hetzner_projects" {
            "hetzner_projects"
        } else {
            "notification_channels"
        };
        status.rows.insert((table, version), count);
    }
    Ok(status)
}

/// Fails if a stored key version is not in `keys` (called on startup).
pub async fn check_versions(db: &PgPool, keys: &Keyring) -> Result<(), KeysError> {
    let missing: Vec<i32> = status(db)
        .await?
        .versions()
        .filter(|v| !keys.contains(*v))
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(KeysError::MissingVersions(missing))
    }
}

/// Result of [`rotate`]: rows re-wrapped per table.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Rotated {
    pub projects: u64,
    pub channels: u64,
}

/// Re-wraps every DEK not on the current version. Idempotent; a row changed concurrently
/// (e.g. a channel config replaced) is skipped by the version guard and already current.
pub async fn rotate(db: &PgPool, keys: &Keyring) -> Result<Rotated, KeysError> {
    check_versions(db, keys).await?;
    Ok(Rotated {
        projects: rotate_projects(db, keys).await?,
        channels: rotate_channels(db, keys).await?,
    })
}

type Row = (Uuid, Vec<u8>, Vec<u8>, i32);

fn rewrap(
    keys: &Keyring,
    table: &'static str,
    (id, secret_ct, dek_ct, key_version): Row,
) -> Result<(Uuid, i32, Sealed), KeysError> {
    let sealed = Sealed {
        secret_ct,
        dek_ct,
        key_version,
    };
    let new = crypto::rewrap(keys, id.as_bytes(), &sealed)
        .map_err(|error| KeysError::Rewrap { table, error })?
        .expect("query selects only non-current rows");
    Ok((id, key_version, new))
}

async fn rotate_projects(db: &PgPool, keys: &Keyring) -> Result<u64, KeysError> {
    let current = keys.current_version();
    let mut done = 0;
    loop {
        let mut tx = db.begin().await?;
        let rows: Vec<Row> = sqlx::query_as(
            "SELECT id, token_ct, token_dek_ct, key_version FROM hetzner_projects
             WHERE key_version <> $1 ORDER BY id LIMIT $2 FOR UPDATE SKIP LOCKED",
        )
        .bind(current)
        .bind(BATCH)
        .fetch_all(&mut *tx)
        .await?;
        if rows.is_empty() {
            return Ok(done);
        }
        for row in rows {
            let (id, old, new) = rewrap(keys, "hetzner_projects", row)?;
            done += sqlx::query(
                "UPDATE hetzner_projects SET token_dek_ct = $3, key_version = $4
                 WHERE id = $1 AND key_version = $2",
            )
            .bind(id)
            .bind(old)
            .bind(&new.dek_ct)
            .bind(new.key_version)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        }
        tx.commit().await?;
    }
}

async fn rotate_channels(db: &PgPool, keys: &Keyring) -> Result<u64, KeysError> {
    let current = keys.current_version();
    let mut done = 0;
    loop {
        let mut tx = db.begin().await?;
        let rows: Vec<Row> = sqlx::query_as(
            "SELECT id, config_ct, config_dek_ct, key_version FROM notification_channels
             WHERE key_version <> $1 ORDER BY id LIMIT $2 FOR UPDATE SKIP LOCKED",
        )
        .bind(current)
        .bind(BATCH)
        .fetch_all(&mut *tx)
        .await?;
        if rows.is_empty() {
            return Ok(done);
        }
        for row in rows {
            let (id, old, new) = rewrap(keys, "notification_channels", row)?;
            done += sqlx::query(
                "UPDATE notification_channels SET config_dek_ct = $3, key_version = $4
                 WHERE id = $1 AND key_version = $2",
            )
            .bind(id)
            .bind(old)
            .bind(&new.dek_ct)
            .bind(new.key_version)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        }
        tx.commit().await?;
    }
}
