use super::super::reversal_pages::{self, ReversalPageReceipt};
use super::*;

pub(super) fn review_reason(
    outcome: &str,
    failures: i32,
    page: Option<&ReversalPageReceipt>,
    page_count: i64,
    generation: i32,
) -> Option<&'static str> {
    match outcome {
        "uncertain" => Some("uncertain"),
        "invalid" => Some("invalid"),
        "capped" => Some("scan_limit"),
        "transient" if failures >= 3 => Some("retry_exhausted"),
        "page" if page.is_some_and(|p| p.next_cursor.is_none()) => Some("ended"),
        "page" if page.is_some_and(|p| p.page_number >= 32) || page_count >= 128 => {
            Some("scan_limit")
        }
        _ if generation >= 40 => Some("scan_limit"),
        _ => None,
    }
}

pub(super) async fn transition(
    tx: &mut Transaction<'_, Postgres>,
    clock: &dyn Clock,
    j: &Job,
    op: &str,
    at: i64,
    result: (&str, Option<&ReversalPageReceipt>, bool),
) -> Result<(), RecoveryError> {
    let (outcome, page, recovering) = result;
    let failures = if outcome == "transient" {
        j.failures + 1
    } else if outcome == "page" {
        0
    } else {
        j.failures
    };
    let page_count = if page.is_some() {
        reversal_pages::rows::history(tx, &j.attempt_id).await?.0
    } else {
        0
    };
    if page.is_some_and(|p| p.recorded_at > at) {
        return Err(RecoveryError::ClockUnavailable);
    }
    let reason = review_reason(outcome, failures, page, page_count, j.generation);
    let state = if reason.is_some() {
        "review"
    } else {
        "pending"
    };
    let due = if outcome == "transient" && reason.is_none() {
        at.checked_add(60)
            .filter(|v| *v <= 9_007_199_254_740_991)
            .ok_or(RecoveryError::ClockUnavailable)?
    } else {
        at
    };
    let tip = page.map(|p| p.id.as_str()).or(j.tip_page_id.as_deref());
    let count=sqlx::query("UPDATE reversal_scan_operations SET outcome=$1,finished_at=$2,page_id=$3 WHERE id=$4 AND job_id=$5 AND outcome IS NULL")
        .bind(outcome).bind(at).bind(page.map(|p|p.id.as_str())).bind(op).bind(&j.id).execute(&mut **tx).await?.rows_affected();
    if count != 1 {
        return Err(RecoveryError::InvalidStoredData);
    }
    let count=sqlx::query("UPDATE reversal_scan_jobs SET state=$1,review_reason=$2,tip_page_id=$3,failures=$4,updated_at=$5,due_at=$6,lease_until=NULL WHERE id=$7 AND state='leased' AND generation=$8 AND lease_until=$9")
        .bind(state).bind(reason).bind(tip).bind(failures).bind(at).bind(due).bind(&j.id).bind(j.generation).bind(j.lease_until).execute(&mut **tx).await?.rows_affected();
    if count != 1 {
        return Err(RecoveryError::LeaseLost);
    }
    let stored: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM reversal_scan_operations WHERE id=$1 AND job_id=$2 AND generation=$3 AND predecessor_id IS NOT DISTINCT FROM $4 AND started_at=$5 AND lease_until=$6 AND outcome=$7 AND finished_at=$8 AND page_id IS NOT DISTINCT FROM $9 AND start_xid IS NOT NULL AND finish_xid=pg_current_xact_id() AND start_xid<>finish_xid)")
        .bind(op).bind(&j.id).bind(j.generation).bind(&j.tip_page_id).bind(j.updated_at).bind(j.lease_until).bind(outcome).bind(at).bind(page.map(|p|p.id.as_str())).fetch_one(&mut **tx).await?;
    if !stored {
        return Err(RecoveryError::InvalidStoredData);
    }
    let after = rows::load(tx, &j.id)
        .await?
        .ok_or(RecoveryError::InvalidStoredData)?;
    let mut expected = j.clone();
    expected.state = state.into();
    expected.review_reason = reason.map(str::to_owned);
    expected.tip_page_id = tip.map(str::to_owned);
    expected.failures = failures;
    expected.updated_at = at;
    expected.due_at = due;
    expected.lease_until = None;
    if after != expected {
        return Err(RecoveryError::InvalidStoredData);
    }
    let final_at = now(clock)?;
    if final_at < at || (!recovering && j.lease_until.is_some_and(|v| final_at >= v)) {
        return Err(RecoveryError::LeaseLost);
    }
    Ok(())
}
