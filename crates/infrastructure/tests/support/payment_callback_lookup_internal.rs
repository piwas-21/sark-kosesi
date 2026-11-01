//! Hostile in-memory handles cannot rebind a committed pre-I/O operation.
use super::*;
use crate::{
    MIGRATOR,
    payment_test_fixture::{Ids, quote::Time},
};
use std::sync::atomic::{AtomicU64, Ordering};

#[sqlx::test(migrator = "MIGRATOR")]
async fn transaction_owned_fetch_commit_requires_exact_operation_start_and_clock_before_minting_permit(
    pool: PgPool,
) {
    let clock = Time(AtomicU64::new(1500));
    let i = Ids(AtomicU64::new(100));
    let operation = prepare_lookup(&pool, &clock, &i, "pfl_SYNTHETIC", "tr_SYNTHETIC")
        .await
        .expect("committed operation");
    for variant in ["operation", "start", "missing_start", "rewind"] {
        clock.0.store(1500, Ordering::SeqCst);
        let mut tx = pool.begin().await.expect("owned transaction");
        if variant != "missing_start" {
            sqlx::query(
                "INSERT INTO callback_lookup_fetch_starts(operation_id,started_at) VALUES($1,1500)",
            )
            .bind(&operation.snapshot.id)
            .execute(&mut *tx)
            .await
            .expect("uncommitted original start");
        }
        let mut requested = operation.snapshot.clone();
        if variant == "operation" {
            requested.provider_id = "tr_OTHER".into();
        }
        let started = if variant == "start" { 1501 } else { 1500 };
        if variant == "rewind" {
            clock.0.store(1499, Ordering::SeqCst);
        }
        let error = commit_started(tx, &clock, requested, started, 1560)
            .await
            .err();
        assert_eq!(
            error,
            Some(if variant == "rewind" {
                PaymentStoreError::Conflict
            } else {
                PaymentStoreError::InvalidStoredData
            }),
            "{variant}"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM callback_lookup_fetch_starts")
                .fetch_one(&pool)
                .await
                .expect("no partial start"),
            0
        );
    }
    clock.0.store(1500, Ordering::SeqCst);
    let mut tx = pool.begin().await.expect("healthy owned transaction");
    sqlx::query(
        "INSERT INTO callback_lookup_fetch_starts(operation_id,started_at) VALUES($1,1500)",
    )
    .bind(&operation.snapshot.id)
    .execute(&mut *tx)
    .await
    .expect("matching start");
    commit_started(tx, &clock, operation.snapshot.clone(), 1500, 1560)
        .await
        .expect("postcommit permit only for exact durable source");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM callback_lookup_fetch_starts")
            .fetch_one(&pool)
            .await
            .expect("committed positive start"),
        1
    );
}
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
