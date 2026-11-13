//! Synthetic data for the owned, native PostgreSQL restore rehearsal only.
use crate::{
    MIGRATOR,
    customer_credential_admission::{AdmissionKey, AdmissionLimits},
    email_verification::delivery::{VerificationDeliveryKey, issue_for_delivery, register_key},
};

#[tokio::test]
#[ignore = "run only through scripts/rehearse_restore.py against its owned database"]
async fn seed_synthetic() {
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
    pool.close().await;
}
