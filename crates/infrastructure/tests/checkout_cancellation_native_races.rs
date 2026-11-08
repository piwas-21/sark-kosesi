//! Direct PostgreSQL writers exercise the database's post-wait exclusion proof.
#[allow(dead_code)]
#[path = "support/payment_fixture.rs"]
mod payment;

use commerce_application::payment_requests::FrozenAttempt;
use commerce_domain::{Currency, checkout_orders::OrderSnapshot, payment_attempts::AttemptBinding};
use commerce_infrastructure::{
    MIGRATOR,
    checkout_orders::OrderStoreError,
    guest_baskets::GuestCredential,
    payment_attempts::{PaymentStoreError, prepare_attempt},
};
use serde_json::{json, to_value};
use sqlx::{PgPool, Postgres, Transaction};
use std::time::Duration;

async fn insert_cancel(
    tx: &mut Transaction<'_, Postgres>,
    order: &OrderSnapshot,
) -> Result<(), sqlx::Error> {
    let id = &order.identity;
    let event = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
    sqlx::query("INSERT INTO checkout_order_cancellations(order_id,basket_id,effect_key,canceled_at,event_id) VALUES($1,$2,'native-cancel',1500,$3) /* native cancellation race */")
        .bind(&id.id).bind(&id.basket_id).bind(event).execute(&mut **tx).await?;
    sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) VALUES($1,$2,'checkout.order_canceled.v1',$3,1500)")
        .bind(event).bind(&id.id).bind(json!({"schema_version":1,"order_id":id.id})).execute(&mut **tx).await?;
    Ok(())
}

async fn insert_payment(
    tx: &mut Transaction<'_, Postgres>,
    order: &OrderSnapshot,
) -> Result<(), sqlx::Error> {
    let config = payment::config();
    let identity = config.identity();
    let id = &order.identity;
    let attempt = "ffffffffffffffffffffffffffffffff";
    let frozen = FrozenAttempt::capture(
        AttemptBinding {
            id: attempt.into(),
            order_id: id.id.clone(),
            basket_id: id.basket_id.clone(),
            stock_group_id: id.stock_group_id.clone(),
            profile: identity.profile,
            configuration_sha256: identity.sha256,
            currency: Currency::EUR,
            amount_minor: order.payable_total().gross_minor,
            created_at: 1500,
            dispatch_before: id.hold_expires_at,
        },
        &config,
    )
    .expect("canonical synthetic attempt");
    let snapshot = frozen.snapshot();
    sqlx::query("INSERT INTO payment_attempts(id,order_id,basket_id,effect_key,intent,idempotency_key,created_at,dispatch_before,snapshot,configuration,configuration_material,request_body) VALUES($1,$2,$3,'native-pay',$4,$5,1500,$6,$7,$8,$9,$10) /* native cancellation race */")
        .bind(attempt).bind(&id.id).bind(&id.basket_id).bind(to_value(payment::intent(order)).expect("intent"))
        .bind(snapshot.idempotency_key()).bind(id.hold_expires_at).bind(to_value(snapshot).expect("snapshot"))
        .bind(to_value(config.snapshot()).expect("configuration")).bind(config.fingerprint_material())
        .bind(frozen.body()).execute(&mut **tx).await?;
    sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) VALUES('dddddddddddddddddddddddddddddddd',$1,'payment.attempt_prepared.v1',$2,1500)")
        .bind(&id.id).bind(json!({"schema_version":1,"order_id":id.id,"attempt_id":attempt})).execute(&mut **tx).await?;
    Ok(())
}

async fn native_race(pool: PgPool, cancel_first: bool) {
    let (store, _, session, csrf, order) = payment::accepted(&pool).await;
    let mut winner = pool.begin().await.expect("winner transaction");
    if cancel_first {
        insert_cancel(&mut winner, &order)
            .await
            .expect("cancellation writes");
    } else {
        insert_payment(&mut winner, &order)
            .await
            .expect("payment writes");
    }
    let second_pool = pool.clone();
    let second_order = order.clone();
    let second = tokio::spawn(async move {
        let mut tx = second_pool.begin().await.expect("native competitor");
        if cancel_first {
            insert_payment(&mut tx, &second_order).await?;
        } else {
            insert_cancel(&mut tx, &second_order).await?;
        }
        tx.commit().await
    });
    let trusted = cancel_first.then(|| {
        let key = GuestCredential::from_session(&session).expect("owner");
        let intent = payment::intent(&order);
        tokio::spawn(async move {
            prepare_attempt(&store, &key, &csrf, "trusted", &intent, &payment::config()).await
        })
    });
    tokio::time::timeout(Duration::from_millis(800), async {
        loop {
            let waiting: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND pid<>pg_backend_pid() AND wait_event_type='Lock' AND (query LIKE '%/* native cancellation race */%' OR query LIKE '%guest_baskets%FOR UPDATE%')")
                .fetch_one(&pool).await.expect("inspect the actual basket lock wait");
            if waiting >= if cancel_first { 2 } else { 1 } {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("writers reached the same basket lock before commit");
    winner
        .commit()
        .await
        .expect("winner commits exact reciprocal effects");
    let error = second
        .await
        .expect("native competitor completed")
        .expect_err("the queued native write must fail");
    let sqlx::Error::Database(error) = error else {
        panic!("expected the native database guard")
    };
    assert_eq!(error.code().as_deref(), Some("23514"));
    assert_eq!(
        error.message(),
        if cancel_first {
            "Payment admission cannot follow cancellation"
        } else {
            "Cancellation cannot follow a payment attempt"
        }
    );
    if let Some(trusted) = trusted {
        assert!(matches!(
            trusted.await.expect("trusted competitor completed"),
            Err(PaymentStoreError::Boundary(OrderStoreError::Conflict))
        ));
    }
    let counts: (i64, i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM checkout_order_cancellations),(SELECT count(*) FROM payment_attempts),(SELECT count(*) FROM commerce_outbox)")
        .fetch_one(&pool).await.expect("exact surviving effects");
    assert_eq!(counts, if cancel_first { (1, 0, 2) } else { (0, 1, 2) });
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn native_cancellation_winner_blocks_queued_native_and_trusted_payments(pool: PgPool) {
    native_race(pool, true).await;
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn native_payment_winner_blocks_queued_native_cancellation(pool: PgPool) {
    native_race(pool, false).await;
}
