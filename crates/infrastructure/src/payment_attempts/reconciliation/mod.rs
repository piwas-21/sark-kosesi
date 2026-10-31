//! Bounded private GET-only recovery. Never dispatch, financial or stock authority.
use super::evidence::{context, limits, now};
use super::*;
use crate::mollie::{MollieError, VerifiedReconciliationClient};
use commerce_application::request_context::{Clock, Identifiers, ObservedClock};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
pub(crate) mod jobs;
pub(crate) mod steps;
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Search,
    Candidates,
    Poll,
    NeedsReview,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewReason {
    Unresolved,
    Ambiguous,
    PaidObserved,
    ScanLimit,
    PollLimit,
    StepLimit,
    CustodyInvalid,
    RetryExhausted,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Job {
    pub attempt_id: String,
    pub source_event_id: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub due_at: i64,
    pub generation: i64,
    pub lease_until: Option<i64>,
    pub phase: Phase,
    pub page_id: Option<String>,
    pub candidate_index: i32,
    pub operation: i64,
    pub polls: i32,
    pub failures: i32,
    pub review_reason: Option<ReviewReason>,
}
pub(crate) struct Claim {
    pub(crate) job: Job,
    pub(crate) until: i64,
}
#[derive(Debug, PartialEq, Eq)]
pub enum RecoveryError {
    InvalidRequest,
    ClockUnavailable,
    Storage,
    InvalidStoredData,
    LeaseLost,
}
impl From<PaymentStoreError> for RecoveryError {
    fn from(value: PaymentStoreError) -> Self {
        match value {
            PaymentStoreError::ClockUnavailable => Self::ClockUnavailable,
            PaymentStoreError::InvalidStoredData => Self::InvalidStoredData,
            _ => Self::Storage,
        }
    }
}
impl From<sqlx::Error> for RecoveryError {
    fn from(_: sqlx::Error) -> Self {
        Self::Storage
    }
}
#[derive(Default)]
pub struct Batch {
    pub processed: u16,
    pub needs_review: u16,
    pub retried: u16,
}
/// Explicit trusted supervisor entry. At most 25 leased steps, one GET per step.
/// Profile verification precedes this call; every provider call is outside DB TXs.
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
    let clock: &dyn Clock = &clock;
    jobs::enqueue(pool, clock, client.profile_id()).await?;
    let mut batch = Batch {
        needs_review: jobs::sweep(pool, clock, client.profile_id()).await?,
        ..Batch::default()
    };
    for _ in 0..limit {
        let Some(claim) = jobs::claim(pool, clock, client.profile_id()).await? else {
            break;
        };
        let mut next = claim.job.clone();
        match steps::advance(pool, clock, ids, client, &claim).await {
            Ok(progress) => next = progress,
            Err(MollieError::Unavailable) => {
                next.failures += 1;
                if next.failures >= 5 {
                    steps::review(&mut next, ReviewReason::RetryExhausted);
                } else {
                    batch.retried += 1;
                }
            }
            Err(_) => steps::review(&mut next, ReviewReason::CustodyInvalid),
        }
        jobs::finish(pool, clock, &claim, &next).await?;
        batch.processed += 1;
        if next.phase == Phase::NeedsReview {
            batch.needs_review += 1;
        }
    }
    batch.needs_review += jobs::sweep(pool, clock, client.profile_id()).await?;
    Ok(batch)
}
/// Private operational inspection, never customer data or payment authority.
pub async fn read_job(pool: &PgPool, attempt_id: &str) -> Result<Option<Job>, RecoveryError> {
    if !identifier(attempt_id) {
        return Err(RecoveryError::InvalidRequest);
    }
    let mut tx = pool.begin().await?;
    limits(&mut tx).await?;
    let result = jobs::load(&mut tx, attempt_id).await?;
    tx.rollback().await?;
    Ok(result)
}
