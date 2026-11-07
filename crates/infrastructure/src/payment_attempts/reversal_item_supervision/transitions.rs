use super::super::reversals::ReversalReceipt;
use super::*;
pub(super) async fn transition(
    tx: &mut Transaction<'_, Postgres>,
    clock: &dyn Clock,
    j: &Job,
    op: &str,
    at: i64,
    result: (&str, Option<&ReversalReceipt>, bool),
) -> Result<(), RecoveryError> {
    let (outcome, receipt, recovering) = result;
    if receipt.is_some_and(|r| r.recorded_at > at) {
        return Err(RecoveryError::ClockUnavailable);
    }
    let failures = if outcome == "transient" {
        j.failures + 1
    } else {
        j.failures
    };
    let reason = match outcome {
        "uncertain" => Some("uncertain"),
        "invalid" => Some("invalid"),
        "capped" => Some("observation_limit"),
        "transient" if failures >= 3 => Some("retry_exhausted"),
        _ => None,
    };
    let state = if outcome == "observed" {
        "observed"
    } else if reason.is_some() {
        "review"
    } else {
        "pending"
    };
    let due = if state == "pending" {
        at.checked_add(60)
            .filter(|v| *v <= 9_007_199_254_740_991)
            .ok_or(RecoveryError::ClockUnavailable)?
    } else {
        at
    };
    let receipt_id = receipt.map(|r| r.id.as_str());
    let count=sqlx::query("UPDATE reversal_item_operations SET outcome=$1,finished_at=$2,receipt_id=$3 WHERE id=$4 AND job_id=$5 AND outcome IS NULL")
        .bind(outcome).bind(at).bind(receipt_id).bind(op).bind(&j.id).execute(&mut **tx).await?.rows_affected();
    if count != 1 {
        return Err(RecoveryError::InvalidStoredData);
    }
    let count=sqlx::query("UPDATE reversal_item_jobs SET state=$1,review_reason=$2,receipt_id=$3,failures=$4,updated_at=$5,due_at=$6,lease_until=NULL WHERE id=$7 AND state='leased' AND generation=$8 AND lease_until=$9")
        .bind(state).bind(reason).bind(receipt_id).bind(failures).bind(at).bind(due).bind(&j.id).bind(j.generation).bind(j.lease_until).execute(&mut **tx).await?.rows_affected();
    if count != 1 {
        return Err(RecoveryError::LeaseLost);
    }
    let stored:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM reversal_item_operations WHERE id=$1 AND job_id=$2 AND generation=$3 AND started_at=$4 AND lease_until=$5 AND outcome=$6 AND finished_at=$7 AND receipt_id IS NOT DISTINCT FROM $8 AND start_xid IS NOT NULL AND finish_xid=pg_current_xact_id() AND start_xid<>finish_xid)")
        .bind(op).bind(&j.id).bind(j.generation).bind(j.updated_at).bind(j.lease_until).bind(outcome).bind(at).bind(receipt_id).fetch_one(&mut **tx).await?;
    let mut expected = j.clone();
    expected.state = state.into();
    expected.review_reason = reason.map(str::to_owned);
    expected.receipt_id = receipt_id.map(str::to_owned);
    expected.failures = failures;
    expected.updated_at = at;
    expected.due_at = due;
    expected.lease_until = None;
    if !stored || rows::load(tx, &j.id).await? != Some(expected) {
        return Err(RecoveryError::InvalidStoredData);
    }
    let final_at = now(clock)?;
    if final_at < at || (!recovering && j.lease_until.is_some_and(|v| final_at >= v)) {
        return Err(RecoveryError::LeaseLost);
    }
    Ok(())
}
