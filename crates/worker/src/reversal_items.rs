//! Explicit local/test item supervision; provider identifiers remain private.
use commerce_application::request_context::{Clock, Identifiers};
use commerce_infrastructure::{
    mollie::{MollieError, ReconciliationClient, TestCredential},
    payment_attempts::reversal_item_supervision::{self, EnrollIntent},
    runtime::{SecureIdentifiers, SystemClock},
};
use sqlx::PgPool;
use std::io::{Error, Result};

#[derive(PartialEq, Eq)]
pub(super) struct Enrollment {
    attempt: String,
    observation: String,
    page: String,
    ordinal: i32,
    effect: String,
}
impl Enrollment {
    pub(super) fn parse(
        attempt: &str,
        observation: &str,
        page: &str,
        ordinal: &str,
        effect: &str,
    ) -> Result<Self> {
        let ordinal = ordinal
            .parse::<i32>()
            .ok()
            .filter(|v| (1..=10).contains(v))
            .filter(|v| v.to_string() == ordinal)
            .ok_or_else(|| Error::other("unsupported worker command"))?;
        if !super::reversal_pages::identifier(attempt)
            || !super::reversal_pages::identifier(observation)
            || !super::reversal_pages::identifier(page)
            || effect.is_empty()
            || effect.len() > 128
            || !effect
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            return Err(Error::other("unsupported worker command"));
        }
        Ok(Self {
            attempt: attempt.into(),
            observation: observation.into(),
            page: page.into(),
            ordinal,
            effect: effect.into(),
        })
    }
}
pub(super) struct Settings {
    pub(super) key: String,
    pub(super) profile: String,
    pub(super) batch: Option<String>,
}

pub(super) async fn run(pool: &PgPool, intent: Option<Enrollment>) -> Result<()> {
    let key = std::env::var("MOLLIE_TEST_API_KEY")
        .map_err(|_| Error::other("test payment configuration required"))?;
    let profile = std::env::var("MOLLIE_TEST_PROFILE_ID")
        .map_err(|_| Error::other("test payment configuration required"))?;
    let batch = match std::env::var("REVERSAL_ITEM_BATCH") {
        Ok(value) => Some(value),
        Err(std::env::VarError::NotPresent) => None,
        Err(_) => return Err(Error::other("invalid reversal item batch")),
    };
    configured_run(
        pool,
        &SystemClock,
        &SecureIdentifiers,
        Settings {
            key,
            profile,
            batch,
        },
        intent,
        ReconciliationClient::new,
    )
    .await
}
pub(super) async fn configured_run<F>(
    pool: &PgPool,
    clock: &dyn Clock,
    ids: &dyn Identifiers,
    settings: Settings,
    intent: Option<Enrollment>,
    factory: F,
) -> Result<()>
where
    F: FnOnce(TestCredential) -> std::result::Result<ReconciliationClient, MollieError>,
{
    let (credential, limit) = super::reversal_pages::capture(
        &settings.key,
        &settings.profile,
        settings.batch.as_deref(),
    )?;
    drop(settings);
    let client = factory(credential)
        .map_err(|_| Error::other("test payment transport unavailable"))?
        .verify_profile()
        .await
        .map_err(|_| Error::other("test payment profile verification failed"))?;
    if let Some(intent) = intent {
        let job = reversal_item_supervision::enroll(
            pool,
            clock,
            ids,
            &client,
            EnrollIntent {
                attempt_id: &intent.attempt,
                observation_id: &intent.observation,
                page_id: &intent.page,
                ordinal: intent.ordinal,
                enrollment_key: &intent.effect,
            },
        )
        .await
        .map_err(|_| Error::other("reversal item enrollment unavailable"))?;
        tracing::info!(code = "reversal_item_enrolled", job_id = job.id);
    } else {
        let batch = reversal_item_supervision::run_batch(pool, clock, ids, &client, limit)
            .await
            .map_err(|_| Error::other("reversal item reconciliation unavailable"))?;
        tracing::info!(code = "reversal_item_batch", processed = batch.processed);
    }
    Ok(())
}
pub(super) async fn inspect(pool: &PgPool, id: &str) -> Result<()> {
    let job = reversal_item_supervision::read_job(pool, id)
        .await
        .map_err(|_| Error::other("reversal item inspection unavailable"))?
        .ok_or_else(|| Error::other("reversal item not found"))?;
    let late_observation_available = reversal_item_supervision::read_late_receipt(pool, id)
        .await
        .map_err(|_| Error::other("reversal item inspection unavailable"))?
        .is_some();
    tracing::info!(
        code = "reversal_item_state",
        late_observation_available,
        state = job.state,
        generation = job.generation,
        failures = job.failures,
        reason = job.review_reason.as_deref().unwrap_or("none")
    );
    Ok(())
}
