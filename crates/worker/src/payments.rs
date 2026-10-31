//! Explicit test-only GET supervisor; no environment key enters logs or DB custody.
use commerce_infrastructure::{
    mollie::{ReconciliationClient, TestCredential},
    payment_attempts::reconciliation::{Batch, run_batch},
    runtime::{SecureIdentifiers, SystemClock},
};
use sqlx::PgPool;
use std::io::{Error, Result};
fn required(name: &str) -> Result<String> {
    std::env::var(name).map_err(|_| Error::other("test payment configuration required"))
}
fn capture(key: &str, profile: &str, batch: Option<&str>) -> Result<(ReconciliationClient, u16)> {
    let batch = batch
        .unwrap_or("10")
        .parse::<u16>()
        .ok()
        .filter(|v| (1..=25).contains(v))
        .ok_or_else(|| Error::other("invalid payment reconciliation batch"))?;
    let key = TestCredential::capture(key, profile)
        .map_err(|_| Error::other("invalid test payment credential"))?;
    let client = ReconciliationClient::new(key)
        .map_err(|_| Error::other("test payment transport unavailable"))?;
    Ok((client, batch))
}
fn report(batch: &Batch) -> Result<()> {
    tracing::info!(
        code = "payment_reconciliation_batch",
        processed = batch.processed,
        needs_review = batch.needs_review,
        retried = batch.retried
    );
    if batch.needs_review > 0 {
        return Err(Error::other(
            "payment reconciliation requires operator review",
        ));
    }
    if batch.retried > 0 {
        return Err(Error::other("payment reconciliation requires retry"));
    }
    Ok(())
}
pub async fn run(pool: &PgPool) -> Result<()> {
    let key = required("MOLLIE_TEST_API_KEY")?;
    let profile = required("MOLLIE_TEST_PROFILE_ID")?;
    let batch = match std::env::var("PAYMENT_RECONCILIATION_BATCH") {
        Ok(v) => Some(v),
        Err(std::env::VarError::NotPresent) => None,
        Err(_) => return Err(Error::other("invalid payment reconciliation batch")),
    };
    let (client, limit) = capture(&key, &profile, batch.as_deref())?;
    drop(key);
    let client = client
        .verify_profile()
        .await
        .map_err(|_| Error::other("test payment profile verification failed"))?;
    let batch = run_batch(pool, &SystemClock, &SecureIdentifiers, &client, limit)
        .await
        .map_err(|_| Error::other("payment reconciliation unavailable"))?;
    report(&batch)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn settings_accept_only_bounded_test_credentials_and_redacted_outcomes() {
        let key = format!("test_{}", "synthetic".repeat(4));
        assert!(capture(&key, "pfl_SYNTHETIC", None).is_ok());
        assert_eq!(
            capture(&key, "pfl_SYNTHETIC", Some("25"))
                .expect("bounded")
                .1,
            25
        );
        for raw in ["0", "26", "invalid"] {
            assert!(capture(&key, "pfl_SYNTHETIC", Some(raw)).is_err());
        }
        for bad in ["live_unavailable", "test_short"] {
            assert!(
                !capture(bad, "pfl_SYNTHETIC", None)
                    .err()
                    .expect("reject")
                    .to_string()
                    .contains(bad)
            );
        }
        assert!(capture(&key, "wrong", None).is_err());
        assert!(report(&Batch::default()).is_ok());
        assert_eq!(
            report(&Batch {
                processed: 1,
                needs_review: 1,
                retried: 0
            })
            .expect_err("operator")
            .to_string(),
            "payment reconciliation requires operator review"
        );
        assert_eq!(
            report(&Batch {
                processed: 1,
                needs_review: 0,
                retried: 1
            })
            .expect_err("retry")
            .to_string(),
            "payment reconciliation requires retry"
        );
    }
}
