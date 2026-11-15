//! Atomic provenance for the fresh one-use create/fetch pair only.
use super::*;
use sqlx::{Postgres, Transaction};

pub(super) async fn insert(
    tx: &mut Transaction<'_, Postgres>,
    observation: &ObservationSnapshot,
    event_id: &str,
) -> Result<(), PaymentStoreError> {
    let inserted = sqlx::query("INSERT INTO payment_initial_dispatch_provenance(attempt_id,start_id,observation_id,observation_event_id) SELECT $1,id,$2,$3 FROM payment_dispatch_starts WHERE attempt_id=$1")
        .bind(&observation.attempt_id).bind(&observation.id).bind(event_id)
        .execute(&mut **tx).await?;
    if inserted.rows_affected() != 1 {
        return Err(PaymentStoreError::InvalidStoredData);
    }
    sqlx::query("SET CONSTRAINTS require_payment_initial_dispatch_provenance IMMEDIATE")
        .execute(&mut **tx)
        .await?;
    let exact: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM payment_initial_dispatch_provenance p WHERE p.attempt_id=$1 AND p.observation_id=$2 AND p.observation_event_id=$3 AND p.transaction_id=pg_current_xact_id() AND payment_initial_dispatch_provenance_exact(p))")
        .bind(&observation.attempt_id).bind(&observation.id).bind(event_id)
        .fetch_one(&mut **tx).await?;
    if !exact {
        return Err(PaymentStoreError::InvalidStoredData);
    }
    Ok(())
}
