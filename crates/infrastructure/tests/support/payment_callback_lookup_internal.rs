//! Hostile in-memory handles cannot rebind a committed pre-I/O operation.
use super::*;
use crate::{
    MIGRATOR,
    payment_test_fixture::{Ids, quote::Time},
};
use std::sync::atomic::{AtomicU64, Ordering};
#[sqlx::test(migrator = "MIGRATOR")]
async fn altered_operation_handles_and_start_clock_rewind_never_mint_fetch_permits(pool: PgPool) {
    let clock = Time(AtomicU64::new(1500));
    let i = Ids(AtomicU64::new(100));
    let operation = prepare_lookup(&pool, &clock, &i, "pfl_SYNTHETIC", "tr_SYNTHETIC")
        .await
        .expect("committed operation");
    let mut changed = operation.snapshot.clone();
    changed.provider_id = "tr_OTHER".into();
    assert_eq!(
        start_lookup(&pool, &clock, &LookupOperation { snapshot: changed })
            .await
            .err(),
        Some(PaymentStoreError::InvalidStoredData)
    );
    clock.0.store(1499, Ordering::SeqCst);
    assert_eq!(
        start_lookup(&pool, &clock, &operation).await.err(),
        Some(PaymentStoreError::ClockUnavailable)
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM callback_lookup_fetch_starts")
            .fetch_one(&pool)
            .await
            .expect("no partial start"),
        0
    );
    clock.0.store(1500, Ordering::SeqCst);
    start_lookup(&pool, &clock, &operation)
        .await
        .expect("paired healthy handle and clock");
}
