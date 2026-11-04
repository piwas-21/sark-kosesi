use super::super::reversal_pages::ReversalPageReceipt;
use super::super::reversals::{self, ReversalReceipt};
use super::*;
use crate::mollie::ReversalPhase;
use commerce_domain::payment_attempts::{PaymentStatus, observations::ObservationSnapshot};

pub(super) async fn source(
    tx: &mut Transaction<'_, Postgres>,
    j: &Job,
    frozen: &FrozenAttempt,
    started: i64,
) -> Result<(), RecoveryError> {
    let dispatch:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM payment_dispatch_starts s JOIN payment_attempts a ON a.id=s.attempt_id JOIN commerce_outbox e ON e.aggregate_id=a.order_id WHERE s.attempt_id=$1 AND s.started_at=$2 AND e.kind='payment.reconciliation_required.v1' AND e.created_at=s.started_at AND e.payload=jsonb_build_object('schema_version',1,'order_id',a.order_id,'attempt_id',a.id,'dispatch_id',s.id))")
        .bind(&j.attempt_id).bind(started).fetch_one(&mut **tx).await?;
    let observation =
        super::super::observations::rows::by_id(tx, frozen, started, &j.observation_id).await?;
    let page =
        super::super::reversal_pages::rows::load(tx, frozen, started, None, Some(&j.page_id))
            .await?
            .ok_or(RecoveryError::InvalidStoredData)?;
    if !dispatch || !valid_source(j, frozen, &observation, &page) {
        return Err(RecoveryError::InvalidStoredData);
    }
    Ok(())
}
fn valid_source(
    j: &Job,
    frozen: &FrozenAttempt,
    observation: &ObservationSnapshot,
    page: &ReversalPageReceipt,
) -> bool {
    let candidate = j
        .ordinal
        .checked_sub(1)
        .and_then(|ordinal| usize::try_from(ordinal).ok())
        .and_then(|i| page.candidates.get(i));
    observation.intent.payment.status == PaymentStatus::Paid
        && observation.intent.payment.id == j.payment_id
        && frozen.snapshot().binding.profile.id == j.profile_id
        && page.payment_id == j.payment_id
        && page.kind == j.kind
        && page.credential_fingerprint == j.credential_fingerprint
        && candidate == Some(&j.child_id)
        && j.created_at >= observation.recorded_at
        && j.created_at >= page.recorded_at
}
fn matches(j: &Job, r: &ReversalReceipt, started: i64) -> bool {
    let chargeback = matches!(
        r.phase,
        ReversalPhase::ChargebackReversalUnknown
            | ReversalPhase::ChargebackReceived
            | ReversalPhase::ChargebackReversed
    );
    r.attempt_id == j.attempt_id
        && r.payment_id == j.payment_id
        && r.child_id == j.child_id
        && chargeback == (j.kind == ReversalKind::Chargeback)
        && r.credential_fingerprint == j.credential_fingerprint
        && r.recorded_at >= started
}
pub(super) async fn exact_receipt(
    tx: &mut Transaction<'_, Postgres>,
    j: &Job,
    frozen: &FrozenAttempt,
    started: i64,
    op: &str,
    op_started: i64,
) -> Result<Option<ReversalReceipt>, RecoveryError> {
    let result = reversals::rows::load(tx, frozen, started, op).await?;
    if result.as_ref().is_some_and(|r| !matches(j, r, op_started)) {
        return Err(RecoveryError::InvalidStoredData);
    }
    Ok(result)
}
pub(super) async fn receipt(
    tx: &mut Transaction<'_, Postgres>,
    j: &Job,
    frozen: &FrozenAttempt,
    started: i64,
) -> Result<(), RecoveryError> {
    if let Some(id) = &j.receipt_id {
        let op:Option<(String,i64,i64)>=sqlx::query_as("SELECT id,started_at,finished_at FROM reversal_item_operations WHERE job_id=$1 AND generation=$2 AND outcome='observed' AND receipt_id=$3 AND start_xid IS NOT NULL AND finish_xid IS NOT NULL AND start_xid<>finish_xid")
            .bind(&j.id).bind(j.generation).bind(id).fetch_optional(&mut **tx).await?;
        let (effect, at, finished) = op.ok_or(RecoveryError::InvalidStoredData)?;
        let r = exact_receipt(tx, j, frozen, started, &effect, at)
            .await?
            .ok_or(RecoveryError::InvalidStoredData)?;
        if r.id != *id || r.recorded_at > finished || finished != j.updated_at {
            return Err(RecoveryError::InvalidStoredData);
        }
    }
    Ok(())
}

pub(super) async fn late_receipt(
    tx: &mut Transaction<'_, Postgres>,
    j: &Job,
    frozen: &FrozenAttempt,
    started: i64,
) -> Result<Option<ReversalReceipt>, RecoveryError> {
    let op: Option<(String, i64)> = sqlx::query_as("SELECT left(id,33),started_at FROM reversal_item_operations WHERE job_id=$1 AND generation=$2 AND outcome='uncertain' AND finished_at=$3 AND started_at>=$4 AND lease_until=started_at+60 AND finished_at>=lease_until AND receipt_id IS NULL AND start_xid IS NOT NULL AND finish_xid IS NOT NULL AND start_xid<>finish_xid")
        .bind(&j.id).bind(j.generation).bind(j.updated_at).bind(j.created_at).fetch_optional(&mut **tx).await?;
    let (effect, at) = op.ok_or(RecoveryError::InvalidStoredData)?;
    if !identifier(&effect) {
        return Err(RecoveryError::InvalidStoredData);
    }
    exact_receipt(tx, j, frozen, started, &effect, at).await
}
#[derive(sqlx::FromRow)]
pub(super) struct Operation {
    pub(super) id: String,
    job_id: String,
    generation: i32,
    started_at: i64,
    lease_until: i64,
    outcome: Option<String>,
    finished_at: Option<i64>,
    receipt_id: Option<String>,
}
pub(super) async fn operation(
    tx: &mut Transaction<'_, Postgres>,
    j: &Job,
) -> Result<Operation, RecoveryError> {
    let op=sqlx::query_as::<_,Operation>("SELECT left(id,33) id,left(job_id,33) job_id,generation,started_at,lease_until,left(outcome,20) outcome,finished_at,left(receipt_id,33) receipt_id FROM reversal_item_operations WHERE job_id=$1 AND generation=$2")
        .bind(&j.id).bind(j.generation).fetch_optional(&mut **tx).await?.ok_or(RecoveryError::InvalidStoredData)?;
    if !valid_operation(j, &op) {
        return Err(RecoveryError::InvalidStoredData);
    }
    Ok(op)
}
fn valid_operation(j: &Job, op: &Operation) -> bool {
    identifier(&op.id)
        && op.job_id == j.id
        && op.generation == j.generation
        && op.started_at == j.updated_at
        && Some(op.lease_until) == j.lease_until
        && op.outcome.is_none()
        && op.finished_at.is_none()
        && op.receipt_id.is_none()
}
pub(super) async fn lock_job(
    tx: &mut Transaction<'_, Postgres>,
    id: &str,
) -> Result<Option<Job>, RecoveryError> {
    let locked: Option<String> =
        sqlx::query_scalar("SELECT id FROM reversal_item_jobs WHERE id=$1 FOR UPDATE SKIP LOCKED")
            .bind(id)
            .fetch_optional(&mut **tx)
            .await?;
    if locked.is_none() {
        return Ok(None);
    }
    rows::load(tx, id).await
}

#[cfg(test)]
#[path = "../../../tests/support/reversal_item_supervision_validate.rs"]
mod tests;
