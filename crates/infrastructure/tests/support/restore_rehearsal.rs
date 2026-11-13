//! Synthetic data for the owned, native PostgreSQL restore rehearsal only.
use crate::{
    MIGRATOR,
    customer_credential_admission::{AdmissionKey, AdmissionLimits},
    email_verification::delivery::{
        VerificationDeliveryKey, issue_for_delivery,
        opening::{OpenOutcome, claim_and_open},
        register_key,
        start::{ClaimOutcome, claim},
    },
};
use commerce_application::request_context::{Clock, ContextError};
use sha2::{Digest, Sha256};

struct FixtureClock(u64);
impl Clock for FixtureClock {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        Ok(self.0)
    }
}

async fn fixture_pool() -> sqlx::PgPool {
    let url = std::env::var("RESTORE_FIXTURE_DATABASE_URL").expect("owned rehearsal URL");
    let database = std::env::var("RESTORE_FIXTURE_DATABASE_NAME").expect("owned database name");
    assert!(database.starts_with("sark_restore_"));
    assert_eq!(database.len(), "sark_restore_".len() + 32);
    assert!(
        database["sark_restore_".len()..]
            .bytes()
            .all(|b| b.is_ascii_hexdigit())
    );
    assert!(url.starts_with("postgresql://p1_test:") && url.contains("@127.0.0.1:"));
    assert!(url.ends_with(&format!("/{database}")));
    let pool = crate::connect(&url)
        .await
        .expect("owned database connection");
    let actual: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .expect("owned database identity");
    assert_eq!(actual, database);
    pool
}

async fn synthetic_challenge(pool: &sqlx::PgPool, address: &str) -> ([u8; 32], i64) {
    let (raw, created): (Vec<u8>, i64) = sqlx::query_as(
        "SELECT verifier,created_at FROM email_verification_challenges WHERE email_identity=$1",
    )
    .bind(address)
    .fetch_one(pool)
    .await
    .expect("synthetic challenge");
    (raw.try_into().expect("bounded verifier"), created)
}

#[tokio::test]
#[ignore = "run only through scripts/rehearse_restore.py against its owned database"]
async fn seed_synthetic() {
    let pool = fixture_pool().await;
    MIGRATOR
        .run(&pool)
        .await
        .expect("embedded project migrations");
    // This path uses actual catalogue, stock, basket, quote and order APIs. The
    // false paid flag avoids the fixture's mock provider server entirely.
    let fixture = super::order_stock_cases::fixture::accepted(&pool, false, false).await;
    let key = VerificationDeliveryKey::capture("restore-synthetic", [41; 32]).expect("test key");
    let admission = AdmissionKey::new(1, [27; 32]).expect("test admission key");
    let limits = AdmissionLimits::new(1000, 100).expect("test limits");
    register_key(&pool, fixture.time.as_ref(), &key)
        .await
        .expect("register test key");
    issue_for_delivery(
        &pool,
        "restore@example.test",
        fixture.time.as_ref(),
        &admission,
        limits,
        &key,
    )
    .await
    .expect("synthetic encrypted envelope");
    let started = issue_for_delivery(
        &pool,
        "started-restore@example.test",
        fixture.time.as_ref(),
        &admission,
        limits,
        &key,
    )
    .await
    .expect("synthetic prestarted envelope");
    assert_eq!(
        claim(&pool, &started.verifier, fixture.time.as_ref(), &key).await,
        Ok(ClaimOutcome::Started)
    );
    pool.close().await;
}

#[tokio::test]
#[ignore = "run only through scripts/rehearse_restore.py against its restored owned database"]
async fn verify_restored_keys() {
    let pool = fixture_pool().await;
    let old =
        VerificationDeliveryKey::capture("restore-synthetic", [41; 32]).expect("old test key");
    let wrong =
        VerificationDeliveryKey::capture("restore-synthetic", [42; 32]).expect("wrong test key");
    let fresh = VerificationDeliveryKey::capture("restore-fresh-synthetic", [43; 32])
        .expect("fresh test key");
    let (verifier, created) = synthetic_challenge(&pool, "restore@example.test").await;
    let (started, started_at) = synthetic_challenge(&pool, "started-restore@example.test").await;
    assert_eq!(created, started_at);
    let clock = FixtureClock(u64::try_from(created).expect("synthetic clock"));
    let mut called = false;
    assert!(
        claim_and_open(&pool, &verifier, &clock, &wrong, |_, _| called = true)
            .await
            .is_err()
    );
    assert!(!called);
    assert_eq!(
        claim_and_open(&pool, &started, &clock, &old, |_, _| called = true).await,
        Ok(OpenOutcome::AlreadyStarted)
    );
    assert!(!called);
    assert_eq!(
        claim_and_open(&pool, &verifier, &clock, &old, |identity, bearer| {
            let digest: [u8; 32] = Sha256::new()
                .chain_update(b"piwas:email-verification:verifier:v1\0")
                .chain_update(bearer)
                .finalize()
                .into();
            called = identity == "restore@example.test" && digest == verifier;
        })
        .await,
        Ok(OpenOutcome::Opened)
    );
    assert!(called);
    register_key(&pool, &clock, &fresh)
        .await
        .expect("register fresh synthetic key");
    let admission = AdmissionKey::new(1, [27; 32]).expect("test admission key");
    let limits = AdmissionLimits::new(1000, 100).expect("test limits");
    let issued = issue_for_delivery(
        &pool,
        "fresh-restore@example.test",
        &clock,
        &admission,
        limits,
        &fresh,
    )
    .await
    .expect("fresh-key synthetic envelope");
    let mut fresh_called = false;
    assert!(
        claim_and_open(&pool, &issued.verifier, &clock, &old, |_, _| fresh_called =
            true)
        .await
        .is_err()
    );
    assert!(!fresh_called);
    assert_eq!(
        claim_and_open(
            &pool,
            &issued.verifier,
            &clock,
            &fresh,
            |identity, bearer| {
                let digest: [u8; 32] = Sha256::new()
                    .chain_update(b"piwas:email-verification:verifier:v1\0")
                    .chain_update(bearer)
                    .finalize()
                    .into();
                fresh_called =
                    identity == "fresh-restore@example.test" && digest == issued.verifier;
            }
        )
        .await,
        Ok(OpenOutcome::Opened)
    );
    assert!(fresh_called);
    pool.close().await;
}
