//! Detached integrity evidence for the bounded local journal, not provider completeness.
use super::*;
use sha2::{Digest, Sha256};

/// Only the locally retained observations are counted; no absence or balance follows.
#[derive(Debug, PartialEq, Eq)]
pub struct SetInspection {
    pub observations: usize,
    pub digest: String,
}

/// Validate every stored receipt and reciprocal event under the original attempt claim.
pub async fn inspect_set(pool: &PgPool, attempt: &str) -> Result<SetInspection, PaymentStoreError> {
    if !identifier(attempt) {
        return Err(PaymentStoreError::InvalidRequest);
    }
    let mut tx = pool.begin().await?;
    limits(&mut tx).await?;
    let (frozen, started) = context(&mut tx, attempt, true).await?;
    let start_id: String =
        sqlx::query_scalar("SELECT left(id,33) FROM payment_dispatch_starts WHERE attempt_id=$1")
            .bind(attempt)
            .fetch_one(&mut *tx)
            .await?;
    if !identifier(&start_id) {
        return Err(PaymentStoreError::InvalidStoredData);
    }
    let entries: Vec<(String, String, Option<String>)> = sqlx::query_as("SELECT left(o.id,33),left(o.effect_key,129),left(e.payload->>'observation_sha256',65) FROM payment_reversal_observations o LEFT JOIN commerce_outbox e ON e.id=o.event_id WHERE o.attempt_id=$1 ORDER BY o.id COLLATE \"C\" LIMIT 129")
        .bind(attempt).fetch_all(&mut *tx).await?;
    if entries.len() > 128 {
        return Err(PaymentStoreError::InvalidStoredData);
    }
    // The order has one immutable attempt. Do not trust the event payload to
    // select its scope: even a malformed orphan event must be accounted for.
    let event_ids: Vec<String> = sqlx::query_scalar("SELECT left(id,33) FROM commerce_outbox WHERE aggregate_id=$1 AND kind='payment.reversal_observed.v1' ORDER BY id COLLATE \"C\" LIMIT 129")
        .bind(&frozen.snapshot().binding.order_id).fetch_all(&mut *tx).await?;
    if event_ids.len() != entries.len() {
        return Err(PaymentStoreError::InvalidStoredData);
    }
    let mut event_ids: std::collections::HashSet<_> = event_ids.into_iter().collect();
    let mut digest = Sha256::new();
    digest.update(b"piwas:reversal-observation-set:v1\0");
    let scope =
        serde_json::to_vec(frozen.snapshot()).map_err(|_| PaymentStoreError::InvalidStoredData)?;
    digest.update((scope.len() as u64).to_be_bytes());
    digest.update(scope);
    digest.update(start_id.as_bytes());
    digest.update(started.to_be_bytes());
    digest.update((entries.len() as u64).to_be_bytes());
    for (id, effect, event_digest) in &entries {
        let receipt = rows::load(&mut tx, &frozen, started, effect)
            .await?
            .ok_or(PaymentStoreError::InvalidStoredData)?;
        let event_digest = event_digest
            .as_deref()
            .ok_or(PaymentStoreError::InvalidStoredData)?;
        if receipt.id != *id
            || !event_ids.remove(&receipt.event_id)
            || event_digest.len() != 64
            || !event_digest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(PaymentStoreError::InvalidStoredData);
        }
        digest.update(event_digest.as_bytes());
    }
    let digest = digest
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    tx.rollback().await?;
    Ok(SetInspection {
        observations: entries.len(),
        digest,
    })
}
