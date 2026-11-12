//! Private encrypted verification custody; no mail transport or recovery API.
use super::*;
use commerce_domain::checkout_quotes::MAX_QUOTE_UNIX_SECONDS;
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;
mod crypto;
pub mod opening;
pub mod outcome;
pub mod start;

const KEY_DOMAIN: &[u8] = b"piwas:verification-delivery:key-fingerprint:v1\0";

/// The dedicated AEAD key is supplied privately and never returned or logged.
pub struct VerificationDeliveryKey {
    key_id: String,
    bytes: Zeroizing<[u8; 32]>,
    fingerprint: [u8; 32],
}
impl VerificationDeliveryKey {
    pub fn capture(key_id: &str, bytes: [u8; 32]) -> Result<Self, VerificationError> {
        let bytes = Zeroizing::new(bytes);
        if key_id.is_empty()
            || key_id.len() > 64
            || !key_id.as_bytes()[0].is_ascii_alphanumeric()
            || !key_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            || bytes.iter().all(|b| *b == 0)
        {
            return Err(VerificationError::Unavailable);
        }
        let mut digest = Sha256::new();
        digest.update(KEY_DOMAIN);
        digest.update(bytes.as_slice());
        Ok(Self {
            key_id: key_id.into(),
            bytes,
            fingerprint: digest.finalize().into(),
        })
    }
}

/// This receipt contains the verifier and timing, never the bearer or ciphertext.
#[derive(PartialEq, Eq)]
pub struct DeliveryReceipt {
    pub verifier: [u8; 32],
    pub created_at: i64,
    pub expires_at: i64,
}

/// An immutable identifier cannot be registered to different material or reused under another ID.
pub async fn register_key(
    pool: &PgPool,
    clock: &dyn Clock,
    key: &VerificationDeliveryKey,
) -> Result<(), VerificationError> {
    let observed = ObservedClock::new(clock, 0);
    let at = bounded_now(&observed)?;
    let mut tx = begin(pool).await?;
    let inserted = sqlx::query("INSERT INTO email_verification_delivery_keys(key_id,key_fingerprint,registered_at) VALUES($1,$2,$3) ON CONFLICT (key_id) DO NOTHING")
        .bind(&key.key_id).bind(key.fingerprint.as_slice()).bind(at)
        .execute(&mut *tx).await.map_err(|_| VerificationError::Unavailable)?.rows_affected();
    let row: Option<(Vec<u8>, i64)> = sqlx::query_as("SELECT key_fingerprint,registered_at FROM email_verification_delivery_keys WHERE key_id=$1 FOR SHARE")
        .bind(&key.key_id).fetch_optional(&mut *tx).await.map_err(|_| VerificationError::Unavailable)?;
    let (fingerprint, registered_at) = row.ok_or(VerificationError::Unavailable)?;
    if fingerprint.as_slice() != key.fingerprint
        || registered_at < 0
        || (inserted == 1 && registered_at != at)
    {
        return Err(VerificationError::Unavailable);
    }
    let final_at = bounded_now(&observed)?;
    if final_at < registered_at {
        return Err(VerificationError::Unavailable);
    }
    tx.commit()
        .await
        .map_err(|_| VerificationError::Unavailable)
}

/// Issue one admitted challenge and its encrypted bearer in the same transaction.
pub async fn issue_for_delivery(
    pool: &PgPool,
    email: &str,
    clock: &dyn Clock,
    admission_key: &AdmissionKey,
    limits: AdmissionLimits,
    key: &VerificationDeliveryKey,
) -> Result<DeliveryReceipt, VerificationError> {
    if admission_key.matches_secret(&key.bytes) {
        return Err(VerificationError::Unavailable);
    }
    let observed = ObservedClock::new(clock, 0);
    admit_verification(
        pool,
        email,
        admission_key,
        limits,
        &observed,
        CredentialPurpose::EmailVerificationIssue,
    )
    .await?;
    let identity = normalize_email(email).ok_or(VerificationError::Denied)?;
    let token = fresh_token()?;
    let verifier = token.verifier();
    let reservation = reserve_nonce(pool, &observed, key, &verifier).await?;
    let created_at = bounded_now(&observed)?;
    let expires_at = created_at
        .checked_add(LIFETIME)
        .filter(|v| *v <= MAX_QUOTE_UNIX_SECONDS)
        .ok_or(VerificationError::Unavailable)?;
    let counter = reservation.counter;
    let context = crypto::Context {
        purpose: PURPOSE,
        identity: &identity,
        verifier: &verifier,
        created_at,
        expires_at,
        key_id: &key.key_id,
        counter,
        version: crypto::VERSION,
    };
    let ciphertext = crypto::seal(key, &context, &token, reservation)?;
    drop(token);
    let mut tx = begin_durable(pool).await?;
    sqlx::query("INSERT INTO email_verification_challenges(verifier,purpose,email_identity,created_at,expires_at,delivery_mode) VALUES($1,$2,$3,$4,$5,'envelope')")
        .bind(verifier.as_slice()).bind(PURPOSE).bind(&identity).bind(created_at).bind(expires_at)
        .execute(&mut *tx).await.map_err(|_| VerificationError::Unavailable)?;
    sqlx::query("INSERT INTO email_verification_delivery_envelopes(verifier,key_id,nonce_counter,envelope_version,ciphertext) VALUES($1,$2,$3,$4,$5)")
        .bind(verifier.as_slice()).bind(&key.key_id).bind(counter).bind(crypto::VERSION).bind(&ciphertext)
        .execute(&mut *tx).await.map_err(|_| VerificationError::Unavailable)?;
    sqlx::query("SET CONSTRAINTS require_email_verification_challenge_envelope,require_email_verification_envelope IMMEDIATE")
        .execute(&mut *tx).await.map_err(|_| VerificationError::Unavailable)?;
    let exact: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM email_verification_challenges c JOIN email_verification_delivery_envelopes e ON e.verifier=c.verifier WHERE c.verifier=$1 AND c.purpose=$2 AND c.email_identity=$3 AND c.created_at=$4 AND c.expires_at=$5 AND c.delivery_mode='envelope' AND c.consumed_at IS NULL AND c.consumed_xid IS NULL AND c.delivery_transaction_id=pg_current_xact_id() AND e.key_id=$6 AND e.nonce_counter=$7 AND e.envelope_version=$8 AND e.ciphertext=$9 AND octet_length(e.ciphertext)=48 AND e.transaction_id=pg_current_xact_id())")
        .bind(verifier.as_slice()).bind(PURPOSE).bind(&identity).bind(created_at).bind(expires_at).bind(&key.key_id).bind(counter).bind(crypto::VERSION).bind(&ciphertext)
        .fetch_one(&mut *tx).await.map_err(|_| VerificationError::Unavailable)?;
    if !exact {
        return Err(VerificationError::Unavailable);
    }
    require_durable(&mut tx).await?;
    let at = bounded_now(&observed)?;
    if at >= expires_at {
        return Err(VerificationError::Unavailable);
    }
    tx.commit()
        .await
        .map_err(|_| VerificationError::Unavailable)?;
    Ok(DeliveryReceipt {
        verifier,
        created_at,
        expires_at,
    })
}

/// The committed reservation is move-only and can never be reused after failure.
struct NonceReservation {
    counter: i64,
    verifier: [u8; 32],
    key_id: String,
    fingerprint: [u8; 32],
}
async fn reserve_nonce(
    pool: &PgPool,
    clock: &dyn Clock,
    key: &VerificationDeliveryKey,
    verifier: &[u8; 32],
) -> Result<NonceReservation, VerificationError> {
    let mut tx = begin_durable(pool).await?;
    let stored: Option<Vec<u8>> = sqlx::query_scalar(
        "SELECT key_fingerprint FROM email_verification_delivery_keys WHERE key_id=$1 FOR UPDATE",
    )
    .bind(&key.key_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| VerificationError::Unavailable)?;
    if stored.as_deref() != Some(key.fingerprint.as_slice()) {
        return Err(VerificationError::Unavailable);
    }
    let at = bounded_now(clock)?;
    let counter: i64 = sqlx::query_scalar("INSERT INTO email_verification_delivery_nonces(key_id,verifier,reserved_at) VALUES($1,$2,$3) RETURNING nonce_counter")
        .bind(&key.key_id).bind(verifier.as_slice()).bind(at).fetch_one(&mut *tx).await.map_err(|_| VerificationError::Unavailable)?;
    let exact: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM email_verification_delivery_nonces WHERE key_id=$1 AND nonce_counter=$2 AND verifier=$3 AND reserved_at=$4 AND transaction_id=pg_current_xact_id())")
        .bind(&key.key_id).bind(counter).bind(verifier.as_slice()).bind(at).fetch_one(&mut *tx).await.map_err(|_| VerificationError::Unavailable)?;
    if !exact || counter <= 0 {
        return Err(VerificationError::Unavailable);
    }
    require_durable(&mut tx).await?;
    bounded_now(clock)?;
    tx.commit()
        .await
        .map_err(|_| VerificationError::Unavailable)?;
    Ok(NonceReservation {
        counter,
        verifier: *verifier,
        key_id: key.key_id.clone(),
        fingerprint: key.fingerprint,
    })
}
/// Force a local WAL flush for custody that survives process or host failure.
async fn begin_durable(pool: &PgPool) -> Result<Transaction<'_, Postgres>, VerificationError> {
    let mut tx = begin(pool).await?;
    sqlx::query("SET LOCAL synchronous_commit='on'")
        .execute(&mut *tx)
        .await
        .map_err(|_| VerificationError::Unavailable)?;
    Ok(tx)
}

async fn require_durable(tx: &mut Transaction<'_, Postgres>) -> Result<(), VerificationError> {
    let durable: bool = sqlx::query_scalar(
        "SELECT current_setting('fsync')='on' AND current_setting('synchronous_commit')='on'",
    )
    .fetch_one(&mut **tx)
    .await
    .map_err(|_| VerificationError::Unavailable)?;
    if !durable {
        return Err(VerificationError::Unavailable);
    }
    Ok(())
}

fn bounded_now(clock: &dyn Clock) -> Result<i64, VerificationError> {
    now(clock).and_then(|at| {
        if at <= MAX_QUOTE_UNIX_SECONDS {
            Ok(at)
        } else {
            Err(VerificationError::Unavailable)
        }
    })
}
