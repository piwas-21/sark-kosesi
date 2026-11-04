use super::*;
use crate::payment_attempts::PaymentStoreError;
use commerce_application::request_context::{Clock, ContextError};

struct At(u64);
impl Clock for At {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        Ok(self.0)
    }
}

struct RewindAfterWrite(AtomicU64);
impl Clock for RewindAfterWrite {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        Ok(if self.0.fetch_add(1, Ordering::Relaxed) < 2 {
            1502
        } else {
            1501
        })
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn rejects_invalid_locators_and_a_different_authenticated_frozen_snapshot(pool: PgPool) {
    let (time, frozen) = observation_cases::prepared(&pool, true).await;
    let ids = observation_cases::ids();
    let attempt = &frozen.snapshot().binding.id;
    let missing = "f".repeat(32);
    let page = listed(
        frozen.snapshot(),
        ReversalKind::Refund,
        None,
        None,
        &[],
        false,
    )
    .await;
    for (owner, id) in [("invalid", missing.as_str()), (attempt.as_str(), "invalid")] {
        assert!(matches!(
            read_page(&pool, owner, id).await,
            Err(PaymentStoreError::InvalidRequest)
        ));
        assert!(matches!(
            resume(&pool, owner, id).await,
            Err(PaymentStoreError::InvalidRequest)
        ));
    }
    assert!(matches!(
        read_page(&pool, attempt, &missing).await,
        Err(PaymentStoreError::NotFound)
    ));
    assert!(matches!(
        resume(&pool, attempt, &missing).await,
        Err(PaymentStoreError::NotFound)
    ));
    assert!(matches!(
        record(
            &pool,
            time.as_ref(),
            &ids,
            "invalid-predecessor",
            Some("bad"),
            &page
        )
        .await,
        Err(PaymentStoreError::InvalidRequest)
    ));
    assert!(matches!(
        record(
            &pool,
            time.as_ref(),
            &ids,
            "missing-predecessor",
            Some(&missing),
            &page
        )
        .await,
        Err(PaymentStoreError::NotFound)
    ));

    let mut changed = frozen.snapshot().clone();
    changed.request_sha256 = "f".repeat(64);
    let changed = listed(&changed, ReversalKind::Refund, None, None, &[], false).await;
    assert!(matches!(
        record(&pool, time.as_ref(), &ids, "frozen-scope", None, &changed).await,
        Err(PaymentStoreError::Conflict)
    ));
    assert_eq!(counts(&pool).await, (0, 0));
    record(&pool, time.as_ref(), &ids, "frozen-scope", None, &page)
        .await
        .expect("original frozen scope still works");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn clock_rewinds_never_add_partial_page_custody(pool: PgPool) {
    let (_, frozen) = observation_cases::prepared(&pool, true).await;
    let ids = observation_cases::ids();
    let page = listed(
        frozen.snapshot(),
        ReversalKind::Refund,
        None,
        None,
        &[],
        false,
    )
    .await;
    assert!(matches!(
        record(&pool, &At(1499), &ids, "before-dispatch", None, &page).await,
        Err(PaymentStoreError::ClockUnavailable)
    ));
    assert!(matches!(
        record(
            &pool,
            &observation_cases::BadPorts,
            &ids,
            "clock-unavailable",
            None,
            &page
        )
        .await,
        Err(PaymentStoreError::ClockUnavailable)
    ));
    record(&pool, &At(1501), &ids, "clock-control", None, &page)
        .await
        .expect("valid recording time");
    assert!(matches!(
        record(&pool, &At(1500), &ids, "before-latest", None, &page).await,
        Err(PaymentStoreError::ClockUnavailable)
    ));
    let rewind = RewindAfterWrite(AtomicU64::new(0));
    assert!(matches!(
        record(&pool, &rewind, &ids, "final-clock-rewind", None, &page).await,
        Err(PaymentStoreError::ClockUnavailable)
    ));
    assert_eq!(rewind.0.load(Ordering::Relaxed), 3);
    assert_eq!(counts(&pool).await, (1, 1));
    record(&pool, &At(1502), &ids, "final-clock-rewind", None, &page)
        .await
        .expect("retry succeeds with a monotonic clock");
    assert_eq!(counts(&pool).await, (2, 2));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn exact_readback_rolls_back_valid_looking_row_changes_and_detached_transactions(
    pool: PgPool,
) {
    let (time, frozen) = observation_cases::prepared(&pool, true).await;
    let ids = observation_cases::ids();
    let page = listed(
        frozen.snapshot(),
        ReversalKind::Refund,
        None,
        None,
        &["re_Original"],
        false,
    )
    .await;
    pool.execute("CREATE FUNCTION mutate_page_receipt() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN NEW.candidates=ARRAY['re_Changed']; RETURN NEW; END; $$").await.expect("fixture mutation");
    pool.execute("CREATE TRIGGER zz_mutate_page_receipt BEFORE INSERT ON payment_reversal_pages FOR EACH ROW EXECUTE FUNCTION mutate_page_receipt()").await.expect("mutate after native admission");
    assert!(matches!(
        record(&pool, time.as_ref(), &ids, "readback", None, &page).await,
        Err(PaymentStoreError::InvalidStoredData)
    ));
    assert_eq!(counts(&pool).await, (0, 0));

    // Fault-inject a detached XID and disable native reciprocal checks to exercise the Rust fence.
    pool.execute(
        "ALTER TABLE payment_reversal_pages DISABLE TRIGGER require_complete_reversal_page",
    )
    .await
    .expect("isolate Rust transaction fence");
    pool.execute("ALTER TABLE commerce_outbox DISABLE TRIGGER guard_commerce_outbox")
        .await
        .expect("isolate Rust transaction fence");
    pool.execute("CREATE OR REPLACE FUNCTION mutate_page_receipt() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN NEW.transaction_id='1'::xid8; RETURN NEW; END; $$").await.expect("detach saved row");
    assert!(matches!(
        record(&pool, time.as_ref(), &ids, "readback", None, &page).await,
        Err(PaymentStoreError::InvalidStoredData)
    ));
    assert_eq!(counts(&pool).await, (0, 0));
    pool.execute("DROP TRIGGER zz_mutate_page_receipt ON payment_reversal_pages")
        .await
        .expect("restore native admission");
    pool.execute(
        "ALTER TABLE payment_reversal_pages ENABLE TRIGGER require_complete_reversal_page",
    )
    .await
    .expect("restore reciprocal guard");
    pool.execute("ALTER TABLE commerce_outbox ENABLE TRIGGER guard_commerce_outbox")
        .await
        .expect("restore event guard");
    record(&pool, time.as_ref(), &ids, "readback", None, &page)
        .await
        .expect("healthy exact retry");
    assert_eq!(counts(&pool).await, (1, 1));
}
