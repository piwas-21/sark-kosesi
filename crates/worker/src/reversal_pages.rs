//! Explicit test-profile reversal-page entry; never emits provider identifiers or keys.
use commerce_application::request_context::{Clock, Identifiers};
use commerce_infrastructure::{
    mollie::{MollieError, ReconciliationClient, ReversalKind, TestCredential},
    payment_attempts::reversal_supervision::{self, EnrollIntent},
    runtime::{SecureIdentifiers, SystemClock},
};
use sqlx::PgPool;
use std::io::{Error, Result};

#[derive(PartialEq, Eq)]
pub(super) struct Enrollment {
    attempt: String,
    observation: String,
    kind: ReversalKind,
    effect: String,
}
pub(super) fn identifier(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
impl Enrollment {
    pub(super) fn parse(
        attempt: &str,
        observation: &str,
        kind: &str,
        effect: &str,
    ) -> Result<Self> {
        let kind = match kind {
            "refund" => ReversalKind::Refund,
            "chargeback" => ReversalKind::Chargeback,
            _ => return Err(Error::other("unsupported worker command")),
        };
        if !identifier(attempt)
            || !identifier(observation)
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
            kind,
            effect: effect.into(),
        })
    }
}
pub(super) fn capture(
    key: &str,
    profile: &str,
    batch: Option<&str>,
) -> Result<(TestCredential, u16)> {
    let limit = batch
        .unwrap_or("10")
        .parse::<u16>()
        .ok()
        .filter(|v| (1..=25).contains(v))
        .ok_or_else(|| Error::other("invalid reversal page batch"))?;
    let credential = TestCredential::capture(key, profile)
        .map_err(|_| Error::other("invalid test payment credential"))?;
    Ok((credential, limit))
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
    let batch = match std::env::var("REVERSAL_PAGE_BATCH") {
        Ok(value) => Some(value),
        Err(std::env::VarError::NotPresent) => None,
        Err(_) => return Err(Error::other("invalid reversal page batch")),
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
// Only the canonical production constructor is supplied by the environment entry.
// The private factory seam permits authenticated loopback fixtures, never an origin override.
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
    let (credential, limit) = capture(&settings.key, &settings.profile, settings.batch.as_deref())?;
    drop(settings);
    let client =
        factory(credential).map_err(|_| Error::other("test payment transport unavailable"))?;
    let client = client
        .verify_profile()
        .await
        .map_err(|_| Error::other("test payment profile verification failed"))?;
    if let Some(intent) = intent {
        let job = reversal_supervision::enroll(
            pool,
            clock,
            ids,
            &client,
            EnrollIntent {
                attempt_id: &intent.attempt,
                observation_id: &intent.observation,
                kind: intent.kind,
                enrollment_key: &intent.effect,
            },
        )
        .await
        .map_err(|_| Error::other("reversal scan enrollment unavailable"))?;
        tracing::info!(code = "reversal_scan_enrolled", job_id = job.id);
        return Ok(());
    }
    let batch = reversal_supervision::run_batch(pool, clock, ids, &client, limit)
        .await
        .map_err(|_| Error::other("reversal page reconciliation unavailable"))?;
    tracing::info!(code = "reversal_page_batch", processed = batch.processed);
    Ok(())
}
pub(super) async fn inspect(pool: &PgPool, id: &str) -> Result<()> {
    let inspection = reversal_supervision::inspect(pool, id)
        .await
        .map_err(|_| Error::other("reversal scan inspection unavailable"))?
        .ok_or_else(|| Error::other("reversal scan not found"))?;
    let job = inspection.job;
    let late_page = match inspection.late_page_present {
        None => "not_applicable",
        Some(false) => "absent",
        Some(true) => "present",
    };
    tracing::info!(
        code = "reversal_scan_state",
        state = job.state,
        generation = job.generation,
        failures = job.failures,
        reason = job.review_reason.as_deref().unwrap_or("none"),
        late_page
    );
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_test_configuration_is_redacted() {
        let key = format!("test_{}", "synthetic".repeat(4));
        assert_eq!(capture(&key, "pfl_SYNTHETIC", None).expect("default").1, 10);
        for (raw, expected) in [("1", 1), ("25", 25)] {
            assert_eq!(
                capture(&key, "pfl_SYNTHETIC", Some(raw))
                    .expect("boundary")
                    .1,
                expected
            );
        }
        for raw in ["0", "26", "invalid", "65536"] {
            assert!(capture(&key, "pfl_SYNTHETIC", Some(raw)).is_err());
        }
        for bad in ["live_DO_NOT_USE", "test_short"] {
            assert!(
                !capture(bad, "pfl_SYNTHETIC", None)
                    .err()
                    .expect("invalid key")
                    .to_string()
                    .contains(bad)
            );
        }
        assert!(capture(&key, "wrong", None).is_err());
    }
}
