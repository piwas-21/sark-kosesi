//! Private bounded callback supervision; no public ingress or financial effects.
use super::super::{
    PaymentStoreError,
    evidence::{fresh_id, limits, now},
    identifier,
};
use super::lookup;
use crate::mollie::{MollieError, VerifiedReconciliationClient};
use commerce_application::request_context::{Clock, Identifiers, ObservedClock};
use commerce_domain::payment_attempts::callback_inbox::{
    CallbackJob, IntakeSnapshot, JobState, MAX_BATCH, MAX_GENERATIONS, ReviewReason,
};
use sqlx::{PgPool, Postgres, Transaction};
mod active;
pub(crate) mod drain;
pub(crate) mod jobs;
mod processing_source;
mod rows;
pub struct IntakeReceipt {
    pub intake_id: String,
}
#[derive(Default)]
pub struct CallbackBatch {
    pub processed: u16,
    pub drained: u16,
    pub needs_review: u64,
    pub retry_pending: u64,
}
/// Trusted configured profile and untrusted identifier only. No status/body stored.
/// Native global/profile quotas serialize admission with the atomic intake/event/job.
pub async fn admit_callback(
    pool: &PgPool,
    clock: &dyn Clock,
    ids: &dyn Identifiers,
    profile_id: &str,
    provider_id: &str,
) -> Result<IntakeReceipt, PaymentStoreError> {
    let initial = clock
        .unix_seconds()
        .map_err(|_| PaymentStoreError::ClockUnavailable)?;
    let clock = ObservedClock::new(clock, initial);
    let at = now(&clock)?;
    let id = fresh_id(ids)?;
    IntakeSnapshot {
        schema_version: 1,
        id: id.clone(),
        profile_id: profile_id.into(),
        provider_id: provider_id.into(),
        received_at: at,
    }
    .validate()
    .map_err(|_| PaymentStoreError::InvalidRequest)?;
    let event = fresh_id(ids)?;
    let mut tx = pool.begin().await?;
    limits(&mut tx).await?;
    sqlx::query(
        "INSERT INTO callback_intakes(id,profile_id,provider_id,received_at) VALUES($1,$2,$3,$4)",
    )
    .bind(&id)
    .bind(profile_id)
    .bind(provider_id)
    .bind(at)
    .execute(&mut *tx)
    .await?;
    sqlx::query("INSERT INTO callback_intake_events(id,intake_id,created_at,kind,payload) VALUES($1,$2,$3,'payment.callback_received.v1',jsonb_build_object('schema_version',1,'intake_id',$2::text,'profile_id',$4::text,'provider_id',$5::text,'received_at',$3::bigint))")
        .bind(&event).bind(&id).bind(at).bind(profile_id).bind(provider_id).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO callback_inbox_jobs(intake_id,source_event_id,created_at,updated_at,due_at) VALUES($1,$2,$3,$3,$3)")
        .bind(&id).bind(&event).bind(at).execute(&mut *tx).await?;
    sqlx::query(
        "INSERT INTO callback_inbox_active(intake_id,profile_id,created_at) VALUES($1,$2,$3)",
    )
    .bind(&id)
    .bind(profile_id)
    .bind(at)
    .execute(&mut *tx)
    .await?;
    now(&clock)?;
    tx.commit().await?;
    Ok(IntakeReceipt { intake_id: id })
}
pub async fn read_callback_job(
    pool: &PgPool,
    intake_id: &str,
) -> Result<Option<CallbackJob>, PaymentStoreError> {
    if !identifier(intake_id) {
        return Err(PaymentStoreError::InvalidRequest);
    }
    let mut tx = pool.begin().await?;
    limits(&mut tx).await?;
    let job = rows::load(&mut tx, intake_id).await?;
    tx.rollback().await?;
    Ok(job)
}
/// Each claimed generation performs at most one GET. Receipt processing is drained
/// independently of latest job state; a stale fetched PAID fact is never stranded.
pub async fn run_callback_batch(
    pool: &PgPool,
    clock: &dyn Clock,
    ids: &dyn Identifiers,
    client: &VerifiedReconciliationClient,
    limit: u16,
) -> Result<CallbackBatch, PaymentStoreError> {
    if !(1..=MAX_BATCH).contains(&limit) {
        return Err(PaymentStoreError::InvalidRequest);
    }
    let initial = clock
        .unix_seconds()
        .map_err(|_| PaymentStoreError::ClockUnavailable)?;
    let clock = ObservedClock::new(clock, initial);
    let mut batch = CallbackBatch {
        drained: drain::drain_receipts(pool, &clock, ids, client.profile_id(), limit).await?,
        ..CallbackBatch::default()
    };
    jobs::sweep(pool, &clock, client.profile_id()).await?;
    for _ in 0..limit {
        let Some(mut claim) = jobs::claim(pool, &clock, ids, client.profile_id()).await? else {
            break;
        };
        let outcome = match client.fetch_lookup(claim.take_permit()?).await {
            Ok(fetched) => lookup::record_lookup(pool, &clock, ids, &claim.operation_id, &fetched)
                .await
                .map_err(MollieError::Custody),
            Err(error) => Err(error),
        };
        let (state, reason) = match outcome {
            Ok(_) => {
                // This drain targets this operation even when an older backlog fills
                // the bounded independent scan; the next pass still drains others.
                match drain::process_operation(pool, &clock, ids, &claim.operation_id).await? {
                    drain::Disposition::Observed => (JobState::Done, None),
                    drain::Disposition::Unmatched => {
                        (JobState::NeedsReview, Some(ReviewReason::Unmatched))
                    }
                    drain::Disposition::PaidObserved => {
                        (JobState::NeedsReview, Some(ReviewReason::PaidObserved))
                    }
                    drain::Disposition::Ambiguous => {
                        (JobState::NeedsReview, Some(ReviewReason::Ambiguous))
                    }
                    drain::Disposition::CustodyInvalid => {
                        (JobState::NeedsReview, Some(ReviewReason::CustodyInvalid))
                    }
                }
            }
            Err(MollieError::Unavailable) => {
                if claim.job.generation >= MAX_GENERATIONS {
                    (JobState::NeedsReview, Some(ReviewReason::RetryExhausted))
                } else {
                    (JobState::Pending, None)
                }
            }
            Err(_) => (JobState::NeedsReview, Some(ReviewReason::CustodyInvalid)),
        };
        // Losing lease custody never deletes the already committed fetched receipt
        // or its independent disposition. A later pass can drain every generation.
        match jobs::finish(pool, &clock, &claim, state, reason).await {
            Ok(()) | Err(PaymentStoreError::Conflict) => {}
            Err(error) => return Err(error),
        }
        batch.processed += 1;
    }
    batch.drained += drain::drain_receipts(pool, &clock, ids, client.profile_id(), limit).await?;
    jobs::sweep(pool, &clock, client.profile_id()).await?;
    (batch.needs_review, batch.retry_pending) =
        jobs::outstanding(pool, client.profile_id()).await?;
    now(&clock)?;
    Ok(batch)
}
