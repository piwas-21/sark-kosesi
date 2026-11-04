//! Lower typed boundaries reject incomplete fresh custody independently of decoder gates.
use super::*;
use crate::{MIGRATOR, mollie::tests::order_stock_cases::fixture as orders};
use sqlx::Executor;
use std::sync::atomic::Ordering;
#[test]
fn immutable_order_snapshot_errors_have_closed_static_historical_classification() {
    use crate::checkout_orders::OrderStoreError;
    assert_eq!(
        order_error(OrderStoreError::Database),
        LateStockError::Database
    );
    for error in [
        OrderStoreError::InvalidRequest,
        OrderStoreError::Denied,
        OrderStoreError::Csrf,
        OrderStoreError::Conflict,
        OrderStoreError::RateLimited,
        OrderStoreError::NotFound,
        OrderStoreError::QuoteNotFound,
        OrderStoreError::QuoteExpired,
        OrderStoreError::QuoteChanged,
        OrderStoreError::Unavailable,
        OrderStoreError::InvalidStoredData,
        OrderStoreError::ClockUnavailable,
        OrderStoreError::EntropyUnavailable,
    ] {
        assert_eq!(order_error(error), LateStockError::InvalidStoredData);
    }
}
async fn positive(pool: &PgPool, receipt: &LateAllocation) {
    let mut tx = pool.begin().await.expect("paired healthy history proof");
    allocation(&mut tx, receipt)
        .await
        .expect("same original and fresh custody is valid");
    tx.rollback().await.expect("positive rollback");
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn late_history_private_typed_membership_requires_every_fresh_member_and_exact_original_source(
    pool: PgPool,
) {
    let f = orders::accepted(&pool, false, true).await;
    f.time.0.store(2000, Ordering::SeqCst);
    let Outcome::Late(late) =
        crate::late_stock::recover(&pool, f.time.as_ref(), f.ids.as_ref(), &f.order.identity.id)
            .await
            .expect("genuine native-proved allocation")
            .outcome
    else {
        panic!("intact source allocates")
    };
    positive(&pool, &late).await;
    let mut missing = late.clone();
    missing.footprint.fresh.pop();
    let mut wrong = late.clone();
    wrong.footprint.fresh[0].units = 2;
    let mut invalid_window = late.clone();
    invalid_window.expires_at = i64::MIN;
    for bad in [missing, wrong, invalid_window] {
        let mut tx = pool.begin().await.expect("direct lower typed boundary");
        assert_eq!(
            allocation(&mut tx, &bad).await.err(),
            Some(LateStockError::InvalidStoredData)
        );
        tx.rollback().await.expect("negative rollback");
        positive(&pool, &late).await;
    }
    let mut tx = pool
        .begin()
        .await
        .expect("coherent still-active fresh source import");
    tx.execute("ALTER TABLE stock_holds DISABLE TRIGGER USER; ALTER TABLE stock_movements DISABLE TRIGGER USER").await.expect("only isolated source fixture guards");
    sqlx::query("DELETE FROM stock_movements WHERE action='consume-hold' AND hold_id IN(SELECT id FROM stock_holds WHERE basket_group_id=$1)").bind(&late.fresh_group_id).execute(&mut *tx).await.expect("remove actual fresh terminal movements in fixture");
    sqlx::query("UPDATE stock_holds SET state='active' WHERE basket_group_id=$1")
        .bind(&late.fresh_group_id)
        .execute(&mut *tx)
        .await
        .expect("coherent complete reserved fresh members");
    tx.execute("UPDATE stock_balances SET on_hand=10,reserved=4 WHERE component_code='SEAT-COVER'; UPDATE stock_balances SET on_hand=4,reserved=2 WHERE component_code='SEAT-FOAM'").await.expect("actual complete pre-consume physical backing");
    assert_eq!(
        allocation(&mut tx, &late).await.err(),
        Some(LateStockError::InvalidStoredData),
        "reserved fresh source still cannot stand in for consumed allocation"
    );
    tx.rollback()
        .await
        .expect("restore exact rows guards and stock");
    positive(&pool, &late).await;
    let mut tx = pool.begin().await.expect("real database failure contract");
    tx.execute("SELECT * FROM late_fixture_missing_relation")
        .await
        .expect_err("calibrated transaction database failure");
    assert_eq!(
        context(
            &mut tx,
            &late.order_id,
            &late.attempt_id,
            &late.recognition_id
        )
        .await
        .err(),
        Some(LateStockError::Database)
    );
    tx.rollback()
        .await
        .expect("discard aborted fixture transaction");
    positive(&pool, &late).await;
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn late_history_native_proof_errors_have_literal_static_classification_and_redaction(
    pool: PgPool,
) {
    let violation=sqlx::query("DO $$ BEGIN RAISE EXCEPTION 'fixture native proof violation' USING ERRCODE='23514'; END $$").execute(&pool).await.expect_err("actual native proof error");
    assert_eq!(proof_error(violation), LateStockError::InvalidStoredData);
    let storage=sqlx::query("DO $$ BEGIN RAISE EXCEPTION 'fixture private storage marker' USING ERRCODE='P0001'; END $$").execute(&pool).await.expect_err("actual other database error");
    assert!(
        format!("{storage:?}").contains("fixture private storage marker"),
        "positive diagnostic leak detector calibration"
    );
    let public = proof_error(storage);
    assert_eq!(public, LateStockError::Database);
    assert!(!format!("{public:?}").contains("fixture private storage marker"));
    let protocol = proof_error(sqlx::Error::Protocol(
        "fixture private protocol marker".into(),
    ));
    assert_eq!(protocol, LateStockError::Database);
    assert!(!format!("{protocol:?}").contains("fixture private protocol marker"));
}
