use super::super::reversal_pages::{self, ReversalPageReceipt};
use super::*;
use commerce_domain::payment_attempts::observations::ObservationSnapshot;

pub(super) async fn source(
    tx: &mut Transaction<'_, Postgres>,
    j: &Job,
    frozen: &FrozenAttempt,
    started: i64,
) -> Result<(), RecoveryError> {
    let dispatch_event: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM payment_dispatch_starts s JOIN payment_attempts a ON a.id=s.attempt_id JOIN commerce_outbox e ON e.aggregate_id=a.order_id WHERE s.attempt_id=$1 AND s.started_at=$2 AND e.kind='payment.reconciliation_required.v1' AND e.created_at=s.started_at AND e.payload=jsonb_build_object('schema_version',1,'order_id',a.order_id,'attempt_id',a.id,'dispatch_id',s.id))")
        .bind(&j.attempt_id).bind(started).fetch_one(&mut **tx).await?;
    if !dispatch_event {
        return Err(RecoveryError::InvalidStoredData);
    }
    let o = super::super::observations::rows::by_id(tx, frozen, started, &j.observation_id).await?;
    if !valid_source(j, frozen, &o) {
        return Err(RecoveryError::InvalidStoredData);
    }
    Ok(())
}
pub(super) async fn tip(
    tx: &mut Transaction<'_, Postgres>,
    j: &Job,
    frozen: &FrozenAttempt,
    started: i64,
) -> Result<(), RecoveryError> {
    if let Some(id) = &j.tip_page_id {
        let p = reversal_pages::rows::load(tx, frozen, started, None, Some(id))
            .await?
            .ok_or(RecoveryError::InvalidStoredData)?;
        if !valid_tip(j, &p) {
            return Err(RecoveryError::InvalidStoredData);
        }
    }
    Ok(())
}
pub(super) async fn exact_page(
    tx: &mut Transaction<'_, Postgres>,
    j: &Job,
    frozen: &FrozenAttempt,
    started: i64,
    op: &str,
) -> Result<Option<ReversalPageReceipt>, RecoveryError> {
    let p = reversal_pages::rows::load(tx, frozen, started, Some(op), None).await?;
    if let Some(p) = &p
        && !valid_page(j, p)
    {
        return Err(RecoveryError::InvalidStoredData);
    }
    Ok(p)
}
#[derive(sqlx::FromRow)]
pub(super) struct Operation {
    pub(super) id: String,
    pub(super) job_id: String,
    pub(super) generation: i32,
    pub(super) predecessor_id: Option<String>,
    pub(super) started_at: i64,
    pub(super) lease_until: i64,
    pub(super) outcome: Option<String>,
    pub(super) finished_at: Option<i64>,
    pub(super) page_id: Option<String>,
}
pub(super) async fn operation(
    tx: &mut Transaction<'_, Postgres>,
    j: &Job,
) -> Result<Operation, RecoveryError> {
    let op=sqlx::query_as::<_,Operation>("SELECT left(id,33) id,left(job_id,33) job_id,generation,left(predecessor_id,33) predecessor_id,started_at,lease_until,left(outcome,20) outcome,finished_at,left(page_id,33) page_id FROM reversal_scan_operations WHERE job_id=$1 AND generation=$2")
        .bind(&j.id).bind(j.generation).fetch_optional(&mut **tx).await?.ok_or(RecoveryError::InvalidStoredData)?;
    if !valid_operation(j, &op) {
        return Err(RecoveryError::InvalidStoredData);
    }
    Ok(op)
}
fn valid_source(j: &Job, frozen: &FrozenAttempt, o: &ObservationSnapshot) -> bool {
    o.intent.payment.status == PaymentStatus::Paid
        && o.intent.payment.id == j.payment_id
        && frozen.snapshot().binding.profile.id == j.profile_id
        && j.created_at >= o.recorded_at
}
fn valid_tip(j: &Job, p: &ReversalPageReceipt) -> bool {
    p.payment_id == j.payment_id
        && p.kind == j.kind
        && p.credential_fingerprint == j.credential_fingerprint
        && p.recorded_at >= j.created_at
        && p.recorded_at <= j.updated_at
}
fn valid_page(j: &Job, p: &ReversalPageReceipt) -> bool {
    p.previous_page_id == j.tip_page_id
        && p.payment_id == j.payment_id
        && p.kind == j.kind
        && p.credential_fingerprint == j.credential_fingerprint
        && p.recorded_at >= j.updated_at
}
fn valid_operation(j: &Job, op: &Operation) -> bool {
    identifier(&op.id)
        && op.job_id == j.id
        && op.generation == j.generation
        && op.predecessor_id == j.tip_page_id
        && op.started_at == j.updated_at
        && Some(op.lease_until) == j.lease_until
        && op.outcome.is_none()
        && op.finished_at.is_none()
        && op.page_id.is_none()
}
pub(super) async fn lock_job(
    tx: &mut Transaction<'_, Postgres>,
    id: &str,
) -> Result<Option<Job>, RecoveryError> {
    let locked: Option<String> =
        sqlx::query_scalar("SELECT id FROM reversal_scan_jobs WHERE id=$1 FOR UPDATE SKIP LOCKED")
            .bind(id)
            .fetch_optional(&mut **tx)
            .await?;
    if locked.is_none() {
        return Ok(None);
    }
    rows::load(tx, id).await
}

#[cfg(test)]
#[path = "../../../tests/support/reversal_supervision_validate.rs"]
mod tests;
