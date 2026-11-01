//! Explicit local/test GET-only callback supervisor; aggregate redacted output.
use commerce_application::request_context::{Clock, Identifiers};
use commerce_infrastructure::{
    mollie::{MollieError, ReconciliationClient, TestCredential},
    payment_attempts::callbacks::inbox::{CallbackBatch, run_callback_batch},
    runtime::{SecureIdentifiers, SystemClock},
};
use sqlx::PgPool;
use std::io::{Error, Result};
fn capture(key: &str, profile: &str, batch: Option<&str>) -> Result<(TestCredential, u16)> {
    let batch = batch
        .unwrap_or("10")
        .parse::<u16>()
        .ok()
        .filter(|v| (1..=25).contains(v))
        .ok_or_else(|| Error::other("invalid callback reconciliation batch"))?;
    let key = TestCredential::capture(key, profile)
        .map_err(|_| Error::other("invalid test payment credential"))?;
    Ok((key, batch))
}
/// The event and command outcome share one closed, non-sensitive count snapshot.
struct LogSummary {
    code: &'static str,
    processed: u16,
    drained: u16,
    needs_review: u64,
    retry_pending: u64,
}
impl From<&CallbackBatch> for LogSummary {
    fn from(batch: &CallbackBatch) -> Self {
        Self {
            code: "callback_reconciliation_batch",
            processed: batch.processed,
            drained: batch.drained,
            needs_review: batch.needs_review,
            retry_pending: batch.retry_pending,
        }
    }
}
fn report(batch: &CallbackBatch) -> Result<()> {
    let LogSummary {
        code,
        processed,
        drained,
        needs_review,
        retry_pending,
    } = batch.into();
    tracing::info!(code, processed, drained, needs_review, retry_pending);
    if needs_review > 0 {
        return Err(Error::other(
            "callback reconciliation requires operator review",
        ));
    }
    if retry_pending > 0 {
        return Err(Error::other("callback reconciliation requires retry"));
    }
    Ok(())
}
pub(super) struct Settings {
    pub(super) key: String,
    pub(super) profile: String,
    pub(super) batch: Option<String>,
}
pub async fn run(pool: &PgPool) -> Result<()> {
    let batch = match std::env::var("CALLBACK_RECONCILIATION_BATCH") {
        Ok(v) => Some(v),
        Err(std::env::VarError::NotPresent) => None,
        Err(_) => return Err(Error::other("invalid callback reconciliation batch")),
    };
    let key = std::env::var("MOLLIE_TEST_API_KEY")
        .map_err(|_| Error::other("test payment configuration required"))?;
    let profile = std::env::var("MOLLIE_TEST_PROFILE_ID")
        .map_err(|_| Error::other("test payment configuration required"))?;
    configured_run(
        pool,
        &SystemClock,
        &SecureIdentifiers,
        Settings {
            key,
            profile,
            batch,
        },
        ReconciliationClient::new,
    )
    .await
}
// A private composition seam: production always supplies the fixed-origin test
// transport above. Unit fixtures supply their existing loopback client factory;
// no environment option or public endpoint can override the provider origin.
pub(super) async fn configured_run<F>(
    pool: &PgPool,
    clock: &dyn Clock,
    ids: &dyn Identifiers,
    settings: Settings,
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
    let batch = run_callback_batch(pool, clock, ids, &client, limit)
        .await
        .map_err(|_| Error::other("callback reconciliation unavailable"))?;
    report(&batch)
}
#[cfg(test)]
#[path = "../tests/support/callback_worker.rs"]
mod tests;
