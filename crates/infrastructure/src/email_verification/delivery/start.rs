//! Durable local one-start journal; it does not open or send the encrypted bearer.
//!
//! This checks ciphertext length and relational custody, not AEAD authenticity.
//! A same-length ciphertext modification is detected only by a later authenticated opening.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClaimOutcome {
    Started,
    AlreadyStarted,
}

#[derive(sqlx::FromRow)]
struct Source {
    created_at: i64,
    expires_at: i64,
    identity: Option<String>,
    mode: String,
    consumed_at: Option<i64>,
    consumed_xid_empty: bool,
    envelope_key: Option<String>,
    nonce_counter: Option<i64>,
    fingerprint: Option<Vec<u8>>,
}
#[derive(sqlx::FromRow)]
struct Started {
    key_id: String,
    nonce_counter: i64,
    key_fingerprint: Vec<u8>,
    started_at: i64,
    current_transaction: bool,
    valid_source: bool,
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
    bounded_now(&observed)?;
    let mut tx = begin_durable(pool).await?;
    let claimed: Option<Vec<u8>> = sqlx::query_scalar(
        "SELECT verifier FROM email_verification_challenges WHERE verifier=$1 FOR UPDATE",
    )
    .bind(verifier.as_slice())
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| VerificationError::Unavailable)?;
    if claimed.is_none() {
        return Err(VerificationError::Denied);
    }
    // READ COMMITTED obtains a fresh source snapshot after a possible lock wait.
    let source: Source = sqlx::query_as("SELECT c.created_at,c.expires_at,CASE WHEN octet_length(c.email_identity)<=254 THEN c.email_identity END AS identity,c.delivery_mode AS mode,c.consumed_at,c.consumed_xid IS NULL AS consumed_xid_empty,left(e.key_id,65) AS envelope_key,e.nonce_counter,CASE WHEN octet_length(k.key_fingerprint)=32 THEN k.key_fingerprint END AS fingerprint FROM email_verification_challenges c LEFT JOIN email_verification_delivery_envelopes e ON e.verifier=c.verifier LEFT JOIN email_verification_delivery_keys k ON k.key_id=e.key_id WHERE c.verifier=$1")
        .bind(verifier.as_slice()).fetch_one(&mut *tx).await.map_err(|_| VerificationError::Unavailable)?;
    if source.mode != "envelope" || source.consumed_at.is_some() || !source.consumed_xid_empty {
        return Err(VerificationError::Denied);
    }
    if source.envelope_key.as_deref() != Some(key.key_id.as_str()) {
        return Err(VerificationError::Denied);
    }
    let fingerprint = source
        .fingerprint
        .as_deref()
        .ok_or(VerificationError::Unavailable)?;
    if fingerprint != key.fingerprint.as_slice() {
        return Err(VerificationError::Denied);
    }
    let counter = source.nonce_counter.ok_or(VerificationError::Unavailable)?;
    let identity = source
        .identity
        .as_deref()
        .ok_or(VerificationError::Unavailable)?;
    if normalize_email(identity).as_deref() != Some(identity)
        || source.created_at < 0
        || source.created_at.checked_add(LIFETIME) != Some(source.expires_at)
        || source.expires_at > MAX_QUOTE_UNIX_SECONDS
        || !(1..=1_000_000).contains(&counter)
    {
        return Err(VerificationError::Unavailable);
    }
    let at = bounded_now(&observed)?;
    if at < source.created_at {
        return Err(VerificationError::Unavailable);
    }
    if at >= source.expires_at {
        return Err(VerificationError::Denied);
    }
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
        check_live(&source, bounded_now(&observed)?)?;
        tx.commit()
            .await
            .map_err(|_| VerificationError::Unavailable)?;
        return Ok(ClaimOutcome::AlreadyStarted);
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
    require_durable(&mut tx).await?;
    check_live(&source, bounded_now(&observed)?)?;
    tx.commit()
        .await
        .map_err(|_| VerificationError::Unavailable)?;
    Ok(ClaimOutcome::Started)
}

async fn read_start(
    tx: &mut Transaction<'_, Postgres>,
    verifier: &[u8; 32],
) -> Result<Option<Started>, VerificationError> {
    sqlx::query_as("SELECT left(s.key_id,65) AS key_id,s.nonce_counter,CASE WHEN octet_length(s.key_fingerprint)=32 THEN s.key_fingerprint END AS key_fingerprint,s.started_at,COALESCE(s.transaction_id=pg_current_xact_id_if_assigned(),false) AS current_transaction,COALESCE(email_verification_delivery_start_source(s),false) AS valid_source FROM email_verification_delivery_starts s WHERE s.verifier=$1")
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
fn check_live(source: &Source, at: i64) -> Result<(), VerificationError> {
    if at < source.created_at {
        return Err(VerificationError::Unavailable);
    }
    if at >= source.expires_at {
        return Err(VerificationError::Denied);
    }
    Ok(())
}
