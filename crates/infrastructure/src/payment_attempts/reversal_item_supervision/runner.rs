//! One committed start permits at most one paid-parent and exact-item request pair.
use super::{
    jobs::{self, Claim, Outcome, Work},
    *,
};
use crate::{mollie::MollieError, payment_attempts::reversals};
use commerce_application::request_context::ObservedClock;

#[derive(Default)]
pub struct Batch {
    pub processed: u16,
}

/// Run one explicitly requested, profile-scoped bounded pass.
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

// This noncloneable committed claim is consumed by the only provider invocation.
async fn advance(
    pool: &PgPool,
    clock: &dyn Clock,
    ids: &dyn Identifiers,
    client: &VerifiedReconciliationClient,
    claim: Claim,
) -> Result<(), RecoveryError> {
    match reversals::lookup(pool, &claim.job.attempt_id, &claim.operation_id).await {
        Ok(Some(_)) => return jobs::finish(pool, clock, &claim, Outcome::Observed).await,
        Err(_) => return jobs::finish(pool, clock, &claim, Outcome::Invalid).await,
        Ok(None) => {}
    }
    if jobs::at_capacity(pool, &claim).await? {
        return jobs::finish(pool, clock, &claim, Outcome::Capped).await;
    }
    match jobs::authorize(pool, clock, &claim).await {
        Ok(()) => {}
        Err(RecoveryError::InvalidStoredData) => {
            return jobs::finish(pool, clock, &claim, Outcome::Invalid).await;
        }
        Err(error) => return Err(error),
    }
    let result = match claim.job.kind {
        ReversalKind::Refund => {
            client
                .fetch_refund(&claim.attempt, &claim.job.payment_id, &claim.job.child_id)
                .await
        }
        ReversalKind::Chargeback => {
            client
                .fetch_chargeback(&claim.attempt, &claim.job.payment_id, &claim.job.child_id)
                .await
        }
    };
    let outcome = match result {
        Ok(observation) => {
            match reversals::record(pool, clock, ids, &claim.operation_id, &observation).await {
                Ok(_) => Outcome::Observed,
                Err(_) => jobs::observation_failure(pool, &claim).await?,
            }
        }
        Err(MollieError::Unavailable) => Outcome::Transient,
        Err(_) => Outcome::Invalid,
    };
    jobs::finish(pool, clock, &claim, outcome).await
}
