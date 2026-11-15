//! Atomic provenance for the fresh one-use create/fetch pair only.
use super::*;
use sqlx::{Postgres, Transaction};

/// Revalidate the exact provenance receipt and its authenticated stored source.
/// Absence is unproven, not a provider absence or authority to dispatch.
pub(crate) async fn read_source(
    tx: &mut Transaction<'_, Postgres>,
    frozen: &FrozenAttempt,
    started_at: i64,
) -> Result<Option<ObservationSnapshot>, PaymentStoreError> {
    let row: Option<(String, bool)> = sqlx::query_as("SELECT pg_catalog.left(p.observation_id,33),public.payment_initial_dispatch_provenance_exact(p) FROM public.payment_initial_dispatch_provenance p WHERE p.attempt_id OPERATOR(pg_catalog.=) $1")
        .bind(&frozen.snapshot().binding.id).fetch_optional(&mut **tx).await?;
    let Some((observation_id, exact)) = row else {
        return Ok(None);
    };
    if !exact || !identifier(&observation_id) {
        return Err(PaymentStoreError::InvalidStoredData);
    }
    let observation = rows::by_id(tx, frozen, started_at, &observation_id)
        .await
        .map_err(|_| PaymentStoreError::InvalidStoredData)?;
    Ok(Some(observation))
}

pub(super) async fn insert(
    tx: &mut Transaction<'_, Postgres>,
    observation: &ObservationSnapshot,
    event_id: &str,
    frozen: &FrozenAttempt,
    started_at: i64,
) -> Result<(), PaymentStoreError> {
    if rows::by_id(tx, frozen, started_at, &observation.id).await? != *observation {
        return Err(PaymentStoreError::InvalidStoredData);
    }
    let inserted = sqlx::query("INSERT INTO public.payment_initial_dispatch_provenance(attempt_id,start_id,observation_id,observation_event_id,observation_sha256) SELECT $1,s.id,$2,$3,public.payment_initial_observation_digest(o) FROM public.payment_dispatch_starts s JOIN public.payment_observations o ON o.id OPERATOR(pg_catalog.=) $2 AND o.attempt_id OPERATOR(pg_catalog.=) s.attempt_id WHERE s.attempt_id OPERATOR(pg_catalog.=) $1")
        .bind(&observation.attempt_id).bind(&observation.id).bind(event_id)
        .execute(&mut **tx).await?;
    if inserted.rows_affected() != 1 {
        return Err(PaymentStoreError::InvalidStoredData);
    }
    sqlx::query("SET CONSTRAINTS public.require_payment_initial_dispatch_provenance IMMEDIATE")
        .execute(&mut **tx)
        .await?;
    let exact: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM public.payment_initial_dispatch_provenance p WHERE p.attempt_id OPERATOR(pg_catalog.=) $1 AND p.observation_id OPERATOR(pg_catalog.=) $2 AND p.observation_event_id OPERATOR(pg_catalog.=) $3 AND p.transaction_id OPERATOR(pg_catalog.=) pg_catalog.pg_current_xact_id() AND public.payment_initial_dispatch_provenance_exact(p))")
        .bind(&observation.attempt_id).bind(&observation.id).bind(event_id)
        .fetch_one(&mut **tx).await?;
    if !exact || read_source(tx, frozen, started_at).await?.as_ref() != Some(observation) {
        return Err(PaymentStoreError::InvalidStoredData);
    }
    Ok(())
}
