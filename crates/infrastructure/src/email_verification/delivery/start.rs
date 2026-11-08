//! Durable local one-start journal; it does not open or send the encrypted bearer.
//!
//! This checks ciphertext length and relational custody, not AEAD authenticity.
//! A same-length ciphertext modification is detected only by a later authenticated opening.
use super::*;
mod source;
pub(super) use source::Source;
use source::lock_source;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClaimOutcome {
    Started,
    AlreadyStarted,
}

#[derive(sqlx::FromRow)]
struct Started {
    key_id: String,
    nonce_counter: i64,
    key_fingerprint: Vec<u8>,
    started_at: i64,
    current_transaction: bool,
    valid_source: bool,
    transaction_id: Option<String>,
}

/// Only the transaction that created the durable start can construct this value.
pub(super) struct Permit {
    pub(super) verifier: [u8; 32],
    source: Source,
    started_at: i64,
    transaction_id: String,
}
pub(super) enum InternalClaim {
    New(Box<Permit>),
    AlreadyStarted,
}

/// Commit the one local start while the challenge is still live and locked.
/// The result grants no transport, decryption, mailbox or verification authority.
pub async fn claim(
    pool: &PgPool,
    verifier: &[u8; 32],
    clock: &dyn Clock,
    key: &VerificationDeliveryKey,
) -> Result<ClaimOutcome, VerificationError> {
    let observed = ObservedClock::new(clock, 0);
    match claim_with_permit(pool, verifier, &observed, key).await? {
        InternalClaim::New(_) => Ok(ClaimOutcome::Started),
        InternalClaim::AlreadyStarted => Ok(ClaimOutcome::AlreadyStarted),
    }
}

pub(super) async fn claim_with_permit(
    pool: &PgPool,
    verifier: &[u8; 32],
    observed: &dyn Clock,
    key: &VerificationDeliveryKey,
) -> Result<InternalClaim, VerificationError> {
    bounded_now(observed)?;
    let mut tx = begin_durable(pool).await?;
    let source = lock_source(&mut tx, verifier).await?;
    let counter = source.validate(key)?;
    let at = bounded_now(observed)?;
    check_live(&source, at)?;
    // Reuse the native exact source predicate without creating a start yet.
    let source_valid: bool = sqlx::query_scalar("SELECT email_verification_delivery_start_source(ROW($1::bytea,$2::text,$3::bigint,$4::bytea,$5::bigint,pg_current_xact_id())::email_verification_delivery_starts)")
        .bind(verifier.as_slice()).bind(&key.key_id).bind(counter).bind(key.fingerprint.as_slice()).bind(at)
        .fetch_one(&mut *tx).await.map_err(|_| VerificationError::Unavailable)?;
    if !source_valid {
        return Err(VerificationError::Unavailable);
    }
    if let Some(previous) = read_start(&mut tx, verifier).await? {
        validate_start(&previous, key, counter, &source, at)?;
        require_durable(&mut tx).await?;
        check_live(&source, bounded_now(observed)?)?;
        tx.commit()
            .await
            .map_err(|_| VerificationError::Unavailable)?;
        return Ok(InternalClaim::AlreadyStarted);
    }
    sqlx::query("INSERT INTO email_verification_delivery_starts(verifier,key_id,nonce_counter,key_fingerprint,started_at) VALUES($1,$2,$3,$4,$5)")
        .bind(verifier.as_slice()).bind(&key.key_id).bind(counter).bind(key.fingerprint.as_slice()).bind(at)
        .execute(&mut *tx).await.map_err(|_| VerificationError::Unavailable)?;
    sqlx::query("SET CONSTRAINTS require_email_verification_delivery_start IMMEDIATE")
        .execute(&mut *tx)
        .await
        .map_err(|_| VerificationError::Unavailable)?;
    let saved = read_start(&mut tx, verifier)
        .await?
        .ok_or(VerificationError::Unavailable)?;
    validate_start(&saved, key, counter, &source, at)?;
    if saved.started_at != at || !saved.current_transaction {
        return Err(VerificationError::Unavailable);
    }
    let transaction_id = saved.transaction_id.ok_or(VerificationError::Unavailable)?;
    require_durable(&mut tx).await?;
    check_live(&source, bounded_now(observed)?)?;
    tx.commit()
        .await
        .map_err(|_| VerificationError::Unavailable)?;
    Ok(InternalClaim::New(Box::new(Permit {
        verifier: *verifier,
        source,
        started_at: at,
        transaction_id,
    })))
}

/// Re-lock and validate the exact prior start and encrypted source before opening.
pub(super) async fn source_for_open(
    tx: &mut Transaction<'_, Postgres>,
    permit: &Permit,
    key: &VerificationDeliveryKey,
    clock: &dyn Clock,
) -> Result<Source, VerificationError> {
    let source = lock_source(tx, &permit.verifier).await?;
    if source.is_consumed() {
        return Err(VerificationError::Denied);
    }
    let at = bounded_now(clock)?;
    check_live(&source, at)?;
    if source != permit.source {
        return Err(VerificationError::Unavailable);
    }
    let saved = read_start(tx, &permit.verifier)
        .await?
        .ok_or(VerificationError::Unavailable)?;
    let counter = source.nonce_counter.ok_or(VerificationError::Unavailable)?;
    validate_start(&saved, key, counter, &source, at)?;
    if saved.started_at != permit.started_at
        || saved.transaction_id.as_deref() != Some(permit.transaction_id.as_str())
    {
        return Err(VerificationError::Unavailable);
    }
    Ok(source)
}

async fn read_start(
    tx: &mut Transaction<'_, Postgres>,
    verifier: &[u8; 32],
) -> Result<Option<Started>, VerificationError> {
    sqlx::query_as("SELECT left(s.key_id,65) AS key_id,s.nonce_counter,CASE WHEN octet_length(s.key_fingerprint)=32 THEN s.key_fingerprint END AS key_fingerprint,s.started_at,COALESCE(s.transaction_id=pg_current_xact_id_if_assigned(),false) AS current_transaction,COALESCE(email_verification_delivery_start_source(s),false) AS valid_source,left(s.transaction_id::text,21) AS transaction_id FROM email_verification_delivery_starts s WHERE s.verifier=$1")
        .bind(verifier.as_slice()).fetch_optional(&mut **tx).await.map_err(|_| VerificationError::Unavailable)
}
fn validate_start(
    start: &Started,
    key: &VerificationDeliveryKey,
    counter: i64,
    source: &Source,
    at: i64,
) -> Result<(), VerificationError> {
    if (
        start.key_id.as_str(),
        start.nonce_counter,
        start.key_fingerprint.as_slice(),
    ) != (key.key_id.as_str(), counter, key.fingerprint.as_slice())
        || !(source.created_at..source.expires_at).contains(&start.started_at)
        || start.started_at > at
        || !start.valid_source
    {
        return Err(VerificationError::Unavailable);
    }
    Ok(())
}
pub(super) fn check_live(source: &Source, at: i64) -> Result<(), VerificationError> {
    if at < source.created_at {
        return Err(VerificationError::Unavailable);
    }
    if at >= source.expires_at {
        return Err(VerificationError::Denied);
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../tests/support/email_verification_start_unit.rs"]
mod tests;
