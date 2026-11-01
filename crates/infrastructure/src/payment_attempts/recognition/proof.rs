//! Original source-event custody and complete bounded evidence under one claim.
use super::*;

pub(super) async fn context(
    tx: &mut Transaction<'_, Postgres>,
    attempt: &str,
    claim: bool,
) -> Result<(FrozenAttempt, i64), PaymentStoreError> {
    let (frozen, started) = super::super::evidence::context(tx, attempt, claim).await?;
    let complete: bool = sqlx::query_scalar(
        r#"SELECT EXISTS(SELECT 1 FROM payment_attempts a
        JOIN checkout_orders o ON o.id=a.order_id
        JOIN payment_dispatch_starts d ON d.attempt_id=a.id
        WHERE a.id=$1 AND EXISTS(SELECT 1 FROM commerce_outbox e
          WHERE e.aggregate_id=o.id AND e.kind='checkout.order_created.v1'
          AND e.created_at=o.created_at
          AND e.payload=jsonb_build_object('schema_version',1,'order_id',o.id))
        AND EXISTS(SELECT 1 FROM commerce_outbox e WHERE e.aggregate_id=o.id
          AND e.kind='payment.attempt_prepared.v1' AND e.created_at=a.created_at
          AND e.payload=jsonb_build_object('schema_version',1,'order_id',o.id,'attempt_id',a.id))
        AND EXISTS(SELECT 1 FROM commerce_outbox e WHERE e.aggregate_id=o.id
          AND e.kind='payment.reconciliation_required.v1' AND e.created_at=d.started_at
          AND e.payload=jsonb_build_object('schema_version',1,'order_id',o.id,
            'attempt_id',a.id,'dispatch_id',d.id)))"#,
    )
    .bind(attempt)
    .fetch_one(&mut **tx)
    .await?;
    if !complete {
        return Err(PaymentStoreError::InvalidStoredData);
    }
    Ok((frozen, started))
}

pub(super) async fn watermark(
    tx: &mut Transaction<'_, Postgres>,
    frozen: &FrozenAttempt,
    started: i64,
) -> Result<i64, PaymentStoreError> {
    let observations =
        super::super::observations::rows::prefix(tx, frozen, started, i64::MAX).await?;
    commerce_domain::payment_attempts::identities::summarize(
        frozen.snapshot(),
        started,
        &observations,
    )
    .map_err(|_| PaymentStoreError::InvalidStoredData)?;
    Ok(observations
        .last()
        .map_or(0, |observation| observation.revision))
}

pub(super) async fn ownership(
    tx: &mut Transaction<'_, Postgres>,
    frozen: &FrozenAttempt,
    resolution: &super::super::identities::Resolution,
) -> Result<(), PaymentStoreError> {
    let valid: bool = sqlx::query_scalar(
        r#"SELECT NOT EXISTS(
      SELECT 1 FROM unnest($3::text[]) provider WHERE NOT EXISTS(
        SELECT 1 FROM payment_provider_identity_claims c
        JOIN payment_identity_resolutions initial ON initial.id=c.initial_resolution_id
        WHERE c.profile_id=$1 AND c.provider_id=provider AND c.attempt_id=$2
          AND initial.attempt_id=$2 AND initial.revision<=$4 AND initial.through_observation<=$5
          AND initial.summary->'provider_ids' ? provider
          AND EXISTS(SELECT 1 FROM commerce_outbox e WHERE e.aggregate_id=$6
            AND e.kind='payment.identity_resolved.v1' AND e.created_at=initial.recorded_at
            AND e.payload=jsonb_build_object('schema_version',1,'order_id',$6::text,
              'attempt_id',$2::text,'resolution_id',initial.id))))"#,
    )
    .bind(&frozen.snapshot().binding.profile.id)
    .bind(&frozen.snapshot().binding.id)
    .bind(&resolution.summary.provider_ids)
    .bind(resolution.revision)
    .bind(resolution.through_observation)
    .bind(&frozen.snapshot().binding.order_id)
    .fetch_one(&mut **tx)
    .await?;
    if !valid {
        return Err(PaymentStoreError::InvalidStoredData);
    }
    Ok(())
}
