//! Isolated database proof of cancellation versus dispatch, without provider I/O.
#[allow(dead_code)]
#[path = "support/payment_fixture.rs"]
mod payment;

use commerce_infrastructure::{
    MIGRATOR,
    checkout_cancellations::{cancel_order, owned_cancellation},
    checkout_orders::OrderStoreError,
    guest_baskets::GuestCredential,
    payment_attempts::{DispatchOutcome, PaymentStoreError, prepare_attempt, start_dispatch},
};
use serde_json::json;
use sqlx::{Executor, PgPool, Postgres, Transaction};
use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

fn assert_guard(error: sqlx::Error, expected: &str) {
    let sqlx::Error::Database(error) = error else {
        panic!("expected database rejection")
    };
    assert_eq!(error.code().as_deref(), Some("23514"));
    assert_eq!(error.message(), expected);
}

async fn insert_cancel(
    tx: &mut Transaction<'_, Postgres>,
    order: &str,
    basket: &str,
    attempt: Option<&str>,
) -> Result<(), sqlx::Error> {
    let event = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
    sqlx::query("INSERT INTO checkout_order_cancellations(order_id,basket_id,effect_key,canceled_at,event_id,attempt_id) VALUES($1,$2,'native-cancel',1500,$3,$4) /* cancellation dispatch race */")
        .bind(order).bind(basket).bind(event).bind(attempt).execute(&mut **tx).await?;
    sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) VALUES($1,$2,'checkout.order_canceled.v1',$3,1500)")
        .bind(event).bind(order).bind(json!({"schema_version":1,"order_id":order})).execute(&mut **tx).await?;
    Ok(())
}

async fn insert_dispatch(
    tx: &mut Transaction<'_, Postgres>,
    order: &str,
    attempt: &str,
) -> Result<(), sqlx::Error> {
    let dispatch = "dddddddddddddddddddddddddddddddd";
    sqlx::query("INSERT INTO payment_dispatch_starts(attempt_id,id,started_at) VALUES($1,$2,1500) /* cancellation dispatch race */")
        .bind(attempt).bind(dispatch).execute(&mut **tx).await?;
    sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) VALUES('cccccccccccccccccccccccccccccccc',$1,'payment.reconciliation_required.v1',$2,1500)")
        .bind(order).bind(json!({"schema_version":1,"order_id":order,"attempt_id":attempt,"dispatch_id":dispatch})).execute(&mut **tx).await?;
    Ok(())
}

async fn waiting(pool: &PgPool) {
    tokio::time::timeout(Duration::from_millis(800), async {
        loop {
            let found: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND pid<>pg_backend_pid() AND wait_event_type='Lock' AND (query LIKE '%payment_attempts%FOR UPDATE%' OR query LIKE '%cancellation dispatch race%'))")
                .fetch_one(pool).await.expect("actual attempt lock wait");
            if found { break; }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }).await.expect("contender reached attempt lock");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn cancellation_committing_first_blocks_raw_and_trusted_dispatch(pool: PgPool) {
    let (store, time, session, csrf, order) = payment::accepted(&pool).await;
    let key = GuestCredential::from_session(&session).expect("owner");
    let config = payment::config();
    let prepared = prepare_attempt(
        &store,
        &key,
        &csrf,
        "pay",
        &payment::intent(&order),
        &config,
    )
    .await
    .expect("attempt");
    let before: serde_json::Value = sqlx::query_scalar("SELECT jsonb_build_object('balances',(SELECT jsonb_agg(to_jsonb(b) ORDER BY component_code) FROM stock_balances b),'holds',(SELECT jsonb_agg(to_jsonb(h) ORDER BY id) FROM stock_holds h),'redemptions',(SELECT jsonb_agg(to_jsonb(r) ORDER BY order_id) FROM promotion_redemptions r))").fetch_one(&pool).await.expect("stock and promotion custody");
    let id = prepared.frozen.snapshot().binding.id.clone();
    let mut winner = pool.begin().await.expect("winner");
    insert_cancel(
        &mut winner,
        &order.identity.id,
        &order.identity.basket_id,
        Some(&id),
    )
    .await
    .expect("cancel writes");
    let worker_pool = pool.clone();
    let worker_id = id.clone();
    let worker_time = time.clone();
    let worker = tokio::spawn(async move {
        start_dispatch(
            &worker_pool,
            worker_time.as_ref(),
            &payment::Ids(AtomicU64::new(10000)),
            &worker_id,
            &payment::config(),
        )
        .await
    });
    waiting(&pool).await;
    winner.commit().await.expect("cancellation commits");
    assert!(matches!(
        worker.await.expect("worker"),
        Err(PaymentStoreError::Boundary(OrderStoreError::Conflict))
    ));
    let mut raw = pool.begin().await.expect("raw dispatch");
    assert_guard(
        insert_dispatch(&mut raw, &order.identity.id, &id)
            .await
            .expect_err("raw dispatch denied"),
        "Dispatch cannot follow cancellation",
    );
    raw.rollback().await.expect("raw rollback");
    time.0.store(2000, Ordering::SeqCst);
    assert!(
        cancel_order(&store, &key, &csrf, "native-cancel", &order.identity.id)
            .await
            .expect("exact historical replay")
            .replayed
    );
    assert!(
        owned_cancellation(&store, &key, &order.identity.id)
            .await
            .is_ok()
    );
    assert!(matches!(
        prepare_attempt(
            &store,
            &key,
            &csrf,
            "pay",
            &payment::intent(&order),
            &config
        )
        .await,
        Err(PaymentStoreError::Boundary(OrderStoreError::Conflict))
    ));
    let effects: (i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM checkout_order_cancellations),(SELECT count(*) FROM payment_dispatch_starts)").fetch_one(&pool).await.expect("effects");
    assert_eq!(effects, (1, 0));
    let after: serde_json::Value = sqlx::query_scalar("SELECT jsonb_build_object('balances',(SELECT jsonb_agg(to_jsonb(b) ORDER BY component_code) FROM stock_balances b),'holds',(SELECT jsonb_agg(to_jsonb(h) ORDER BY id) FROM stock_holds h),'redemptions',(SELECT jsonb_agg(to_jsonb(r) ORDER BY order_id) FROM promotion_redemptions r))").fetch_one(&pool).await.expect("unchanged custody");
    assert_eq!(after, before);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn dispatch_committing_first_blocks_raw_and_trusted_cancellation(pool: PgPool) {
    let (store, time, session, csrf, order) = payment::accepted(&pool).await;
    let key = GuestCredential::from_session(&session).expect("owner");
    let config = payment::config();
    let prepared = prepare_attempt(
        &store,
        &key,
        &csrf,
        "pay",
        &payment::intent(&order),
        &config,
    )
    .await
    .expect("attempt");
    let id = prepared.frozen.snapshot().binding.id.clone();
    let mut winner = pool.begin().await.expect("winner");
    insert_dispatch(&mut winner, &order.identity.id, &id)
        .await
        .expect("dispatch writes");
    let worker_store = store.clone();
    let worker_key = GuestCredential::from_session(&session).expect("owner");
    let order_id = order.identity.id.clone();
    let worker = tokio::spawn(async move {
        cancel_order(&worker_store, &worker_key, &csrf, "cancel", &order_id).await
    });
    waiting(&pool).await;
    winner.commit().await.expect("dispatch commits");
    assert!(matches!(
        worker.await.expect("worker"),
        Err(OrderStoreError::Conflict)
    ));
    let mut raw = pool.begin().await.expect("raw cancellation");
    assert_guard(
        insert_cancel(
            &mut raw,
            &order.identity.id,
            &order.identity.basket_id,
            Some(&id),
        )
        .await
        .expect_err("raw cancellation denied"),
        "Cancellation cannot follow dispatch start",
    );
    raw.rollback().await.expect("raw rollback");
    assert!(matches!(
        start_dispatch(
            &pool,
            time.as_ref(),
            &payment::Ids(AtomicU64::new(10000)),
            &id,
            &config
        )
        .await,
        Ok(DispatchOutcome::Reconcile { .. })
    ));
    let effects: (i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM checkout_order_cancellations),(SELECT count(*) FROM payment_dispatch_starts)").fetch_one(&pool).await.expect("effects");
    assert_eq!(effects, (0, 1));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn raw_receipt_requires_exact_attempt_and_read_committed_dispatch(pool: PgPool) {
    let (store, _, session, csrf, order) = payment::accepted(&pool).await;
    let key = GuestCredential::from_session(&session).expect("owner");
    let prepared = prepare_attempt(
        &store,
        &key,
        &csrf,
        "pay",
        &payment::intent(&order),
        &payment::config(),
    )
    .await
    .expect("attempt");
    let id = &prepared.frozen.snapshot().binding.id;
    for (attempt, message) in [
        (None, "Cancellation cannot follow a payment attempt"),
        (
            Some("ffffffffffffffffffffffffffffffff"),
            "Cancellation requires exact prepared attempt",
        ),
    ] {
        let mut tx = pool.begin().await.expect("transaction");
        assert_guard(
            insert_cancel(
                &mut tx,
                &order.identity.id,
                &order.identity.basket_id,
                attempt,
            )
            .await
            .expect_err("incorrect attempt denied"),
            message,
        );
        tx.rollback().await.expect("rollback");
    }
    let mut tx = pool.begin().await.expect("repeatable read");
    tx.execute("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
        .await
        .expect("set isolation");
    assert_guard(
        insert_dispatch(&mut tx, &order.identity.id, id)
            .await
            .expect_err("repeatable read denied"),
        "Dispatch requires read committed",
    );
    tx.rollback().await.expect("rollback");
    let mut tx = pool.begin().await.expect("positive control");
    insert_cancel(
        &mut tx,
        &order.identity.id,
        &order.identity.basket_id,
        Some(id),
    )
    .await
    .expect("matching undispatched attempt");
    tx.commit().await.expect("complete receipt");
}

#[sqlx::test(migrations = false)]
async fn legacy_cancellation_survives_upgrade_with_null_attempt_and_exact_replay(pool: PgPool) {
    MIGRATOR.run_to(57, &pool).await.expect("old schema");
    let (store, _, session, csrf, order) = payment::accepted(&pool).await;
    let key = GuestCredential::from_session(&session).expect("owner");
    let event = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
    let mut tx = pool.begin().await.expect("old transaction");
    sqlx::query("INSERT INTO checkout_order_cancellations(order_id,basket_id,effect_key,canceled_at,event_id) VALUES($1,$2,'legacy',1500,$3)").bind(&order.identity.id).bind(&order.identity.basket_id).bind(event).execute(&mut *tx).await.expect("legacy receipt");
    sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) VALUES($1,$2,'checkout.order_canceled.v1',$3,1500)").bind(event).bind(&order.identity.id).bind(json!({"schema_version":1,"order_id":order.identity.id})).execute(&mut *tx).await.expect("legacy event");
    tx.commit().await.expect("old commit");
    MIGRATOR.run_to(61, &pool).await.expect("upgrade");
    let attempt: Option<String> =
        sqlx::query_scalar("SELECT attempt_id FROM checkout_order_cancellations")
            .fetch_one(&pool)
            .await
            .expect("legacy marker");
    assert_eq!(attempt, None);
    let receipt = cancel_order(&store, &key, &csrf, "legacy", &order.identity.id)
        .await
        .expect("historical replay");
    assert!(receipt.replayed);
    assert_eq!(receipt.canceled_at, 1500);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn damaged_attempt_identifiers_cannot_create_or_replay_cancellation(pool: PgPool) {
    let (store, _, session, csrf, order) = payment::accepted(&pool).await;
    let key = GuestCredential::from_session(&session).expect("owner");
    let prepared = prepare_attempt(
        &store,
        &key,
        &csrf,
        "pay",
        &payment::intent(&order),
        &payment::config(),
    )
    .await
    .expect("attempt");
    let id = prepared.frozen.snapshot().binding.id.clone();
    // Model a damaged import with broken checks and references in an isolated DB.
    pool.execute("ALTER TABLE payment_attempts DROP CONSTRAINT payment_attempts_id_check, DROP CONSTRAINT payment_attempts_key_binding").await.expect("test-only damage setup");
    async fn change_id(pool: &PgPool, old: &str, new: &str, order: &str) {
        let mut tx = pool.begin().await.expect("isolated damaged import");
        tx.execute("SET LOCAL session_replication_role = replica")
            .await
            .expect("disable test-only triggers");
        sqlx::query("UPDATE payment_attempts SET id=$2, idempotency_key='piwas-test-payment-'||$2 WHERE id=$1")
            .bind(old).bind(new).execute(&mut *tx).await.expect("replace imported attempt id");
        sqlx::query("UPDATE checkout_order_cancellations SET attempt_id=$2 WHERE attempt_id=$1")
            .bind(old)
            .bind(new)
            .execute(&mut *tx)
            .await
            .expect("replace imported receipt reference");
        sqlx::query("UPDATE commerce_outbox SET payload=jsonb_set(payload,'{attempt_id}',to_jsonb($2::text)) WHERE aggregate_id=$1 AND kind='payment.attempt_prepared.v1'")
            .bind(order).bind(new).execute(&mut *tx).await.expect("replace imported prepared event reference");
        tx.commit().await.expect("commit damaged import");
    }
    change_id(&pool, &id, "bad", &order.identity.id).await;
    assert!(matches!(
        cancel_order(&store, &key, &csrf, "cancel", &order.identity.id).await,
        Err(OrderStoreError::InvalidStoredData)
    ));
    change_id(&pool, "bad", &id, &order.identity.id).await;
    assert!(
        !cancel_order(&store, &key, &csrf, "cancel", &order.identity.id)
            .await
            .expect("intact positive control")
            .replayed
    );
    change_id(&pool, &id, "bad", &order.identity.id).await;
    assert!(matches!(
        owned_cancellation(&store, &key, &order.identity.id).await,
        Err(OrderStoreError::InvalidStoredData)
    ));
    change_id(&pool, "bad", &id, &order.identity.id).await;
    assert!(
        cancel_order(&store, &key, &csrf, "cancel", &order.identity.id)
            .await
            .expect("restored exact replay")
            .replayed
    );
}
