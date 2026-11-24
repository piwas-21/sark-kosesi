//! Detached integrity evidence for the bounded local journal, not provider completeness.
use super::super::evidence::{context_with_start, started};
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
    let (frozen, start_id, started) = context_with_start(&mut tx, attempt, true).await?;
    let (evidence, _) = inspect(&mut tx, &frozen, &start_id, started, None).await?;
    tx.rollback().await?;
    Ok(evidence)
}

/// Inspect all children under an existing attempt claim and require the exact paid parent.
pub(crate) async fn inspect_claimed(
    tx: &mut Transaction<'_, Postgres>,
    frozen: &FrozenAttempt,
    exact_parent: &str,
) -> Result<(SetInspection, i64), PaymentStoreError> {
    let (start_id, started_at) = started(tx, frozen).await?;
    inspect(tx, frozen, &start_id, started_at, Some(exact_parent)).await
}

async fn inspect(
    tx: &mut Transaction<'_, Postgres>,
    frozen: &FrozenAttempt,
    start_id: &str,
    started: i64,
    exact_parent: Option<&str>,
) -> Result<(SetInspection, i64), PaymentStoreError> {
    let attempt = &frozen.snapshot().binding.id;
    let effects: Vec<String> = sqlx::query_scalar("SELECT left(effect_key,129) FROM payment_reversal_observations WHERE attempt_id=$1 ORDER BY id COLLATE \"C\" LIMIT 129")
        .bind(attempt).fetch_all(&mut **tx).await?;
    if effects.len() > 128 {
        return Err(PaymentStoreError::InvalidStoredData);
    }
    // The order has one immutable attempt. Do not trust the event payload to
    // select its scope: even a malformed orphan event must be accounted for.
    // Each verified journal row binds a distinct event. Equal counts then
    // exclude orphans, while the journal rows determine digest order.
    let events: i64 = sqlx::query_scalar("SELECT count(*) FROM (SELECT 1 FROM commerce_outbox WHERE aggregate_id=$1 AND kind='payment.reversal_observed.v1' LIMIT 129) AS scoped")
        .bind(&frozen.snapshot().binding.order_id).fetch_one(&mut **tx).await?;
    if events != effects.len() as i64 {
        return Err(PaymentStoreError::InvalidStoredData);
    }
    let mut digest = Sha256::new();
    digest.update(b"piwas:reversal-observation-set:v1\0");
    let scope =
        serde_json::to_vec(frozen.snapshot()).map_err(|_| PaymentStoreError::InvalidStoredData)?;
    digest.update((scope.len() as u64).to_be_bytes());
    digest.update(scope);
    digest.update(start_id.as_bytes());
    digest.update(started.to_be_bytes());
    digest.update((effects.len() as u64).to_be_bytes());
    let mut recorded_at = started;
    for effect in &effects {
        let (row, event_digest) = rows::load_with_digest(tx, frozen, started, effect)
            .await?
            .ok_or(PaymentStoreError::InvalidStoredData)?;
        if exact_parent.is_some_and(|payment_id| row.payment_id != payment_id) {
            return Err(PaymentStoreError::Conflict);
        }
        recorded_at = recorded_at.max(row.recorded_at);
        digest.update(event_digest.as_bytes());
    }
    let digest = digest
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    Ok((
        SetInspection {
            observations: effects.len(),
            digest,
        },
        recorded_at,
    ))
}
