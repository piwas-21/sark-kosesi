//! Explicit test-only GET supervisor; no environment key enters logs or DB custody.
use commerce_infrastructure::{
    mollie::{ReconciliationClient, TestCredential},
    payment_attempts::reconciliation::{Batch, Job, read_job, run_batch},
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
        retry_pending = batch.retry_pending
    );
    if batch.needs_review > 0 {
        return Err(Error::other(
            "payment reconciliation requires operator review",
        ));
    }
    if batch.retry_pending > 0 {
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

pub(super) async fn inspect(pool: &PgPool, attempt: &str) -> Result<()> {
    let job = read_job(pool, attempt)
        .await
        .map_err(|_| Error::other("payment recovery checkpoint unavailable"))?;
    if let Some(job) = job {
        report_checkpoint(&job);
    } else {
        tracing::info!(code = "payment_recovery_checkpoint", state = "absent");
    }
    Ok(())
}

fn report_checkpoint(job: &Job) {
    tracing::info!(
        code = "payment_recovery_checkpoint",
        state = "present",
        phase = ?job.phase,
        review_reason = ?job.review_reason,
        generation = job.generation,
        resume_revision = job.resume_revision,
        due_at = job.due_at,
        lease_until = ?job.lease_until,
        polls = job.polls,
        failures = job.failures,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use commerce_infrastructure::payment_attempts::reconciliation::{Phase, ReviewReason};
    use std::io::Write;
    use std::sync::{Arc, Mutex};

    struct LogBuffer(Arc<Mutex<Vec<u8>>>);
    impl Write for LogBuffer {
        fn write(&mut self, bytes: &[u8]) -> Result<usize> {
            self.0.lock().expect("checkpoint log").write(bytes)
        }

        fn flush(&mut self) -> Result<()> {
            Ok(())
        }
    }

    #[test]
    fn checkpoint_report_contains_only_closed_state_and_operational_counts() {
        let job = Job {
            attempt_id: "1".repeat(32),
            source_event_id: "2".repeat(32),
            page_id: Some("3".repeat(32)),
            created_at: 1500,
            updated_at: 1540,
            due_at: 1546,
            generation: 23,
            resume_revision: 2,
            lease_until: None,
            phase: Phase::NeedsReview,
            candidate_index: 1,
            operation: 22,
            polls: 3,
            failures: 5,
            review_reason: Some(ReviewReason::RetryExhausted),
        };
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let capture = Arc::clone(&buffer);
        let subscriber = tracing_subscriber::fmt()
            .json()
            .without_time()
            .with_writer(move || LogBuffer(Arc::clone(&capture)))
            .finish();
        tracing::subscriber::with_default(subscriber, || report_checkpoint(&job));
        let output = buffer.lock().expect("checkpoint log");
        let message = std::str::from_utf8(&output).expect("JSON log");
        for field in [
            r#""state":"present""#,
            r#""phase":"NeedsReview""#,
            r#""review_reason":"Some(RetryExhausted)""#,
            r#""generation":23"#,
            r#""resume_revision":2"#,
            r#""due_at":1546"#,
            r#""lease_until":"None""#,
            r#""polls":3"#,
            r#""failures":5"#,
        ] {
            assert!(message.contains(field), "missing checkpoint field {field}");
        }
        for private in [
            job.attempt_id.as_str(),
            job.source_event_id.as_str(),
            job.page_id.as_deref().expect("fixture page"),
        ] {
            assert!(
                !message.contains(private),
                "checkpoint exposed an identifier"
            );
        }
    }

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
                retry_pending: 0
            })
            .expect_err("operator")
            .to_string(),
            "payment reconciliation requires operator review"
        );
        assert_eq!(
            report(&Batch {
                processed: 1,
                needs_review: 0,
                retry_pending: 1
            })
            .expect_err("retry")
            .to_string(),
            "payment reconciliation requires retry"
        );
    }
}
