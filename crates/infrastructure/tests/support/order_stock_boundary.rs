//! Public typed-error redaction, late port failure and deterministic replay controls.
use super::*;
use crate::payment_attempts::PaymentStoreError;
use commerce_application::request_context::{Clock, ContextError, Identifiers};
use commerce_domain::order_stock::SettlementPolicyError;
#[test]
fn settlement_boundary_errors_keep_literal_public_classification_and_discard_private_diagnostics() {
    use crate::checkout_orders::OrderStoreError;
    for (source, expected) in [
        (
            PaymentStoreError::InvalidRequest,
            SettlementError::InvalidRequest,
        ),
        (PaymentStoreError::NotFound, SettlementError::NotFound),
        (PaymentStoreError::Unavailable, SettlementError::Unavailable),
        (
            PaymentStoreError::ClockUnavailable,
            SettlementError::ClockUnavailable,
        ),
        (PaymentStoreError::Database, SettlementError::Database),
        (
            PaymentStoreError::Boundary(OrderStoreError::Database),
            SettlementError::Database,
        ),
        (
            PaymentStoreError::Boundary(OrderStoreError::InvalidRequest),
            SettlementError::InvalidStoredData,
        ),
        (
            PaymentStoreError::Conflict,
            SettlementError::InvalidStoredData,
        ),
        (
            PaymentStoreError::InvalidStoredData,
            SettlementError::InvalidStoredData,
        ),
    ] {
        assert_eq!(SettlementError::from(source), expected);
    }
    for (source, expected) in [
        (
            SettlementPolicyError::InvalidSnapshot,
            SettlementError::InvalidStoredData,
        ),
        (
            SettlementPolicyError::InvalidClock,
            SettlementError::ClockUnavailable,
        ),
        (
            SettlementPolicyError::RecoveryRequired,
            SettlementError::RecoveryRequired,
        ),
    ] {
        assert_eq!(SettlementError::from(source), expected);
    }
    let private = sqlx::Error::Protocol("fixture-private-sql-and-provider-marker".into());
    assert!(
        format!("{private:?}").contains("fixture-private-sql-and-provider-marker"),
        "positive redaction detector calibration"
    );
    let public = SettlementError::from(private);
    assert_eq!(public, SettlementError::Database);
    assert!(!format!("{public:?}").contains("fixture-private-sql-and-provider-marker"));
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn closed_database_is_reported_statically_for_stock_settlement_and_history(pool: PgPool) {
    let f = accepted(&pool, false, true).await;
    pool.close().await;
    assert_eq!(
        settle(&pool, f.time.as_ref(), f.ids.as_ref(), &f.order.identity.id)
            .await
            .err(),
        Some(SettlementError::Database)
    );
    assert_eq!(
        read_settlement(&pool, &f.order.identity.id).await.err(),
        Some(SettlementError::Database)
    );
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn original_attempt_wait_rechecks_canonical_settlement_before_requiring_current_authority(
    pool: PgPool,
) {
    let f = accepted(&pool, false, true).await;
    let mut blocker = pool.begin().await.expect("canonical serialization gate");
    sqlx::query("SELECT id FROM payment_attempts WHERE id=$1 FOR UPDATE")
        .bind(&f.frozen.snapshot().binding.id)
        .execute(&mut *blocker)
        .await
        .expect("hold original attempt before all callers inspect empty receipt");
    let mut tasks = vec![];
    for _ in 0..2 {
        let p = pool.clone();
        let time = f.time.clone();
        let ids = f.ids.clone();
        let order = f.order.identity.id.clone();
        tasks.push(tokio::spawn(async move {
            settle(&p, time.as_ref(), ids.as_ref(), &order).await
        }));
    }
    tokio::time::timeout(Duration::from_millis(800),async{loop{let n:i64=sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'SELECT id FROM payment_attempts WHERE id=%FOR UPDATE'").fetch_one(&pool).await.expect("actual canonical attempt waits");if n>=2{break;}tokio::time::sleep(Duration::from_millis(5)).await;}}).await.expect("all callers saw no receipt before waiting");
    blocker
        .commit()
        .await
        .expect("release one canonical writer");
    let mut receipt = None;
    let mut replayed = 0;
    for task in tasks {
        let result = task
            .await
            .expect("serialized caller")
            .expect("canonical receipt after actual wait");
        replayed += usize::from(result.replayed);
        if let Some(ref original) = receipt {
            assert!(original == &result.settlement);
        }
        receipt = Some(result.settlement);
    }
    assert_eq!(replayed, 1);
    assert_eq!(counts(&pool).await, (1, 1, 3, 1, 2));
}
struct FailingClock {
    successful_reads: u64,
    calls: AtomicU64,
}
impl Clock for FailingClock {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        if self.calls.fetch_add(1, Ordering::SeqCst) < self.successful_reads {
            Ok(1500)
        } else {
            Err(ContextError::ClockUnavailable)
        }
    }
}
struct OneIdentifier(AtomicU64);
impl Identifiers for OneIdentifier {
    fn next(&self) -> Result<[u8; 16], ContextError> {
        if self.0.fetch_add(1, Ordering::SeqCst) == 0 {
            Ok([0x71; 16])
        } else {
            Err(ContextError::EntropyUnavailable)
        }
    }
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn late_clock_or_event_identifier_failure_rolls_back_every_prepared_stock_effect(
    pool: PgPool,
) {
    let f = accepted(&pool, false, true).await;
    let before = footprint(&pool).await;
    for successful_reads in [1, 2] {
        let clock = FailingClock {
            successful_reads,
            calls: AtomicU64::new(0),
        };
        assert_eq!(
            settle(&pool, &clock, f.ids.as_ref(), &f.order.identity.id)
                .await
                .err(),
            Some(SettlementError::ClockUnavailable)
        );
        assert_eq!(clock.calls.load(Ordering::SeqCst), successful_reads + 1);
        assert_eq!(footprint(&pool).await, before);
    }
    let entropy = OneIdentifier(AtomicU64::new(0));
    assert_eq!(
        settle(&pool, f.time.as_ref(), &entropy, &f.order.identity.id)
            .await
            .err(),
        Some(SettlementError::EntropyUnavailable)
    );
    assert_eq!(
        entropy.0.load(Ordering::SeqCst),
        2,
        "late event identity allocation was reached"
    );
    assert_eq!(footprint(&pool).await, before);
    assert!(
        !settle(&pool, f.time.as_ref(), f.ids.as_ref(), &f.order.identity.id)
            .await
            .expect("healthy original source after every failed port")
            .replayed
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn claimed_consumption_accepts_one_fresh_member_and_rejects_adopting_its_terminal_replay(
    pool: PgPool,
) {
    let f = accepted(&pool, false, true).await;
    let before = footprint(&pool).await;
    let mut tx = pool
        .begin()
        .await
        .expect("caller-owned fresh financial and physical claims");
    tx.execute("SET TRANSACTION ISOLATION LEVEL READ COMMITTED")
        .await
        .expect("supported caller isolation");
    let (_, recognition) = crate::payment_attempts::recognition::current_in_transaction(
        &mut tx,
        &f.frozen.snapshot().binding.id,
    )
    .await
    .expect("fresh received authority retained under original attempt claim");
    assert_eq!(
        recognition.expect("current receipt").state,
        crate::payment_attempts::recognition::State::Received
    );
    sqlx::query("SELECT id FROM basket_hold_groups WHERE id=$1 FOR SHARE")
        .bind(&f.order.identity.stock_group_id)
        .execute(&mut *tx)
        .await
        .expect("original sealed group claim");
    let holds: Vec<String> = sqlx::query_scalar(
        "SELECT id FROM stock_holds WHERE basket_group_id=$1 ORDER BY id FOR UPDATE",
    )
    .bind(&f.order.identity.stock_group_id)
    .fetch_all(&mut *tx)
    .await
    .expect("all original member claims");
    tx.execute("SELECT component_code FROM stock_balances ORDER BY component_code FOR UPDATE")
        .await
        .expect("same ordered union physical claims");
    crate::holds::consume_claimed(&mut tx, &holds[0], &f.order.identity.id, 1500)
        .await
        .expect("one fresh original member can be consumed");
    let first: (i64, i64) = sqlx::query_as(
        "SELECT on_hand,reserved FROM stock_balances WHERE component_code='SEAT-COVER'",
    )
    .fetch_one(&mut *tx)
    .await
    .expect("literal first-member physical effect");
    assert_eq!(first, (8, 2));
    assert!(
        matches!(
            crate::holds::consume_claimed(&mut tx, &holds[0], &f.order.identity.id, 1500).await,
            Err(crate::holds::HoldWriteError::InvalidRequest)
        ),
        "settlement helper must reject terminal adoption rather than silently accept a per-hold replay"
    );
    let second: (i64, i64) = sqlx::query_as(
        "SELECT on_hand,reserved FROM stock_balances WHERE component_code='SEAT-COVER'",
    )
    .fetch_one(&mut *tx)
    .await
    .expect("no repeated physical effect");
    assert_eq!(second, (8, 2));
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM stock_movements WHERE action='consume-hold'"
        )
        .fetch_one(&mut *tx)
        .await
        .expect("exactly one first-member movement"),
        1
    );
    tx.rollback()
        .await
        .expect("original complete reservation restored");
    assert_eq!(footprint(&pool).await, before);
}
