//! One committed start permits at most one paid-parent/page request pair.
use super::{
    jobs::{self, Claim, Outcome, Work},
    *,
};
use crate::{mollie::MollieError, payment_attempts::reversal_pages};
use commerce_application::request_context::ObservedClock;

#[derive(Default)]
pub struct Batch {
    pub processed: u16,
}

/// Trusted explicit pass. Profile verification is separate; no scheduler is installed.
pub async fn run_batch(
    pool: &PgPool,
    clock: &dyn Clock,
    ids: &dyn Identifiers,
    client: &VerifiedReconciliationClient,
    limit: u16,
) -> Result<Batch, RecoveryError> {
    if !(1..=25).contains(&limit) {
        return Err(RecoveryError::InvalidRequest);
    }
    let initial = clock
        .unix_seconds()
        .map_err(|_| RecoveryError::ClockUnavailable)?;
    let clock = ObservedClock::new(clock, initial);
    let mut batch = Batch::default();
    for _ in 0..limit {
        let Some(work) = jobs::claim(
            pool,
            &clock,
            ids,
            client.profile_id(),
            client.credential_fingerprint(),
        )
        .await?
        else {
            break;
        };
        match work {
            Work::Claim(claim) => advance(pool, &clock, ids, client, *claim).await?,
            Work::Recovered => {}
        }
        batch.processed += 1;
    }
    Ok(batch)
}

// The noncloneable claim is moved into this private path. There is only one
// provider invocation, after exact-effect recovery and the final fence check.
async fn advance(
    pool: &PgPool,
    clock: &dyn Clock,
    ids: &dyn Identifiers,
    client: &VerifiedReconciliationClient,
    claim: Claim,
) -> Result<(), RecoveryError> {
    let existing = reversal_pages::lookup(pool, &claim.job.attempt_id, &claim.operation_id).await;
    match existing {
        Ok(Some(_)) => return jobs::finish(pool, clock, &claim, Outcome::Page).await,
        Err(_) => return jobs::finish(pool, clock, &claim, Outcome::Invalid).await,
        Ok(None) => {}
    }
    if jobs::at_capacity(pool, &claim).await? {
        return jobs::finish(pool, clock, &claim, Outcome::Capped).await;
    }
    let cursor = match &claim.job.tip_page_id {
        Some(tip) => match reversal_pages::resume(pool, &claim.job.attempt_id, tip).await {
            Ok(Some(cursor)) => Some(cursor),
            Ok(None) => {
                let outcome = jobs::page_failure(pool, &claim).await?;
                return jobs::finish(pool, clock, &claim, outcome).await;
            }
            Err(_) => return jobs::finish(pool, clock, &claim, Outcome::Invalid).await,
        },
        None => None,
    };
    match jobs::authorize(pool, clock, &claim).await {
        Ok(()) => {}
        Err(RecoveryError::InvalidStoredData) => {
            return jobs::finish(pool, clock, &claim, Outcome::Invalid).await;
        }
        Err(error) => return Err(error),
    }
    let result = client
        .reversal_page(
            &claim.attempt,
            &claim.job.payment_id,
            claim.job.kind,
            cursor.as_ref(),
        )
        .await;
    let outcome = match result {
        Ok(page) => match reversal_pages::record(
            pool,
            clock,
            ids,
            &claim.operation_id,
            claim.job.tip_page_id.as_deref(),
            &page,
        )
        .await
        {
            Ok(_) => Outcome::Page,
            Err(_) => jobs::page_failure(pool, &claim).await?,
        },
        Err(MollieError::Unavailable) => Outcome::Transient,
        Err(_) => Outcome::Invalid,
    };
    jobs::finish(pool, clock, &claim, outcome).await
}
