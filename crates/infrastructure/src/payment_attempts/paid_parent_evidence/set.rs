//! A detached, point-in-time digest of bounded stored evidence; not provider truth.
use super::*;
use sha2::{Digest, Sha256};

pub struct SetInspection {
    pub operations: usize,
    pub pending: usize,
    pub complete: usize,
    pub digest: String,
}

#[derive(sqlx::FromRow)]
struct SetRow {
    valid: bool,
    complete: bool,
    digest: Option<String>,
}

/// Claim the attempt and inspect its current received recognition and entire
/// bounded operation set. Any later write makes this detached digest stale.
pub async fn inspect_set(pool: &PgPool, attempt: &str) -> Result<SetInspection, PaymentStoreError> {
    if !identifier(attempt) {
        return Err(PaymentStoreError::InvalidRequest);
    }
    let mut tx = pool.begin().await?;
    limits(&mut tx).await?;
    claim_attempt(&mut tx, attempt).await?;
    let (_, current) = recognition::current_in_transaction(&mut tx, attempt).await?;
    let current = current.ok_or(PaymentStoreError::Unavailable)?;
    if current.state != recognition::State::Received {
        return Err(PaymentStoreError::Unavailable);
    }
    // This full native proof is the prerequisite for the bounded historical
    // prefix checks below, all under the same claimed attempt and transaction.
    let source: Option<(bool, String)> = sqlx::query_as(
        "SELECT coalesce(payment_recognition_source(r,true),false),payment_recognition_digest(r) FROM payment_recognitions r WHERE r.id=$1 AND r.attempt_id=$2",
    )
    .bind(&current.id)
    .bind(attempt)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((true, source_digest)) = source else {
        return Err(PaymentStoreError::InvalidStoredData);
    };
    let rows = sqlx::query_as::<_, SetRow>(include_str!("set.sql"))
        .bind(attempt)
        .bind(current.through_observation)
        .fetch_all(&mut *tx)
        .await?;
    if rows.len() > 128 || rows.iter().any(|row| !row.valid || row.digest.is_none()) {
        return Err(PaymentStoreError::InvalidStoredData);
    }
    let mut hash = Sha256::new();
    hash.update(b"piwas:paid-parent-evidence-set:v1\0");
    hash.update(attempt.as_bytes());
    hash.update(current.id.as_bytes());
    hash.update(current.through_observation.to_be_bytes());
    hash.update(source_digest.as_bytes());
    hash.update((rows.len() as u64).to_be_bytes());
    let mut complete = 0;
    for row in &rows {
        complete += usize::from(row.complete);
        hash.update(
            row.digest
                .as_deref()
                .ok_or(PaymentStoreError::InvalidStoredData)?
                .as_bytes(),
        );
    }
    let digest = hash
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    tx.rollback().await?;
    Ok(SetInspection {
        operations: rows.len(),
        pending: rows.len() - complete,
        complete,
        digest,
    })
}
