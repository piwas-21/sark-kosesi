//! Private pre-attempt cancellation, over isolated PostgreSQL and without provider I/O.
#[allow(dead_code)]
#[path = "support/admitted_login.rs"]
mod admitted_login;
#[allow(dead_code)]
#[path = "support/admitted_proofs.rs"]
mod admitted_proofs;
#[allow(dead_code)]
#[path = "support/payment_fixture.rs"]
mod payment;

use admitted_proofs::{
    customer_passwords, email_verification, verified_customer_accounts::establish,
};
use commerce_application::Locale;
use commerce_infrastructure::{
    MIGRATOR,
    checkout_cancellations::{
        cancel_order, cancel_verified_order, owned_cancellation, owned_verified_cancellation,
    },
    checkout_orders::{OrderStoreError, create_verified_promoted_order},
    checkout_quotes::issue_verified_promoted_quote,
    customer_sessions::CustomerSessionToken,
    guest_baskets::GuestCredential,
    payment_attempts::{
        DispatchOutcome, PaymentStoreError, prepare_attempt, prepare_verified_attempt,
        start_dispatch,
    },
};
use payment::{order, promotion, quote};
use sqlx::{Executor, PgPool};
use std::sync::atomic::{AtomicU64, Ordering};

async fn account(pool: &PgPool, email: &str, time: &quote::Time) -> CustomerSessionToken {
    let proof = email_verification::issue(pool, email, time)
        .await
        .expect("verification proof");
    establish(pool, email, &proof, time)
        .await
        .expect("verified account");
    let proof = customer_passwords::issue(pool, email, time)
        .await
        .expect("password proof");
    customer_passwords::replace(pool, email, &proof, "correct horse battery staple", time)
        .await
        .expect("password setup");
    admitted_login::login(pool, email, "correct horse battery staple", time)
        .await
        .expect("real login")
        .token
}

async fn inventory_and_redemptions(pool: &PgPool) -> serde_json::Value {
    sqlx::query_scalar("SELECT jsonb_build_object('balances',(SELECT jsonb_agg(to_jsonb(b) ORDER BY component_code) FROM stock_balances b),'holds',(SELECT jsonb_agg(to_jsonb(h) ORDER BY id) FROM stock_holds h),'movements',(SELECT jsonb_agg(to_jsonb(m) ORDER BY sequence) FROM stock_movements m),'redemptions',(SELECT jsonb_agg(to_jsonb(r) ORDER BY order_id) FROM promotion_redemptions r))").fetch_one(pool).await.expect("stock and promotion custody")
}

async fn counts(pool: &PgPool) -> (i64, i64, i64, i64) {
    sqlx::query_as("SELECT (SELECT count(*) FROM checkout_order_cancellations),(SELECT count(*) FROM commerce_outbox WHERE kind='checkout.order_canceled.v1'),(SELECT count(*) FROM payment_attempts),(SELECT count(*) FROM payment_dispatch_starts)").fetch_one(pool).await.expect("cancellation and payment effects")
}

async fn waiting(pool: &PgPool, minimum: i64) {
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let count: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock'").fetch_one(pool).await.expect("observe waits");
            if count >= minimum { return; }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }).await.expect("actual lock wait");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn guest_cancellation_replays_and_blocks_new_payment_without_stock_changes(pool: PgPool) {
    let (store, _, session, csrf, placed) = payment::accepted(&pool).await;
    let key = GuestCredential::from_session(&session).expect("owner");
    let (other, other_csrf) = quote::guest(&store).await;
    let other_key = GuestCredential::from_session(&other).expect("other guest");
    let before = inventory_and_redemptions(&pool).await;
    assert!(
        cancel_order(
            &store,
            &other_key,
            &other_csrf,
            "cancel",
            &placed.identity.id
        )
        .await
        .is_err()
    );
    let receipt = cancel_order(&store, &key, &csrf, "cancel", &placed.identity.id)
        .await
        .expect("cancel accepted guest order");
    assert_eq!(
        (&receipt.order_id, receipt.canceled_at, receipt.replayed),
        (&placed.identity.id, 1500, false)
    );
    let replay = cancel_order(&store, &key, &csrf, "cancel", &placed.identity.id)
        .await
        .expect("exact replay");
    assert_eq!(
        (&replay.order_id, replay.canceled_at, replay.replayed),
        (&placed.identity.id, 1500, true)
    );
    assert!(matches!(
        cancel_order(&store, &key, &csrf, "changed", &placed.identity.id).await,
        Err(OrderStoreError::Conflict)
    ));
    assert!(
        owned_cancellation(&store, &key, &placed.identity.id)
            .await
            .is_ok()
    );
    assert!(
        owned_cancellation(&store, &other_key, &placed.identity.id)
            .await
            .is_err()
    );
    assert!(matches!(
        prepare_attempt(
            &store,
            &key,
            &csrf,
            "pay",
            &payment::intent(&placed),
            &payment::config()
        )
        .await,
        Err(PaymentStoreError::Boundary(OrderStoreError::Conflict))
    ));
    assert_eq!(counts(&pool).await, (1, 1, 0, 0));
    assert_eq!(inventory_and_redemptions(&pool).await, before);
}

async fn bound_promoted_cancellation(pool: PgPool, post_attempt: bool) {
    quote::seed(&pool).await;
    pool.execute("INSERT INTO staff_capabilities VALUES('author','promotion.manage'),('reviewer','promotion.review')").await.expect("independent grants");
    let mut policy = promotion::policy();
    policy.limits.per_customer_redemptions = Some(1);
    promotion::draft(&pool, &policy, "draft")
        .await
        .expect("draft");
    promotion::decide(&pool, "review", 1, 0, true)
        .await
        .expect("approval");
    let (store, time) = quote::store(&pool, 1100);
    let customer = account(&pool, "buyer@example.test", &time).await;
    let other = account(&pool, "other@example.test", &time).await;
    let (session, csrf) = quote::guest(&store).await;
    let key = GuestCredential::from_session(&session).expect("guest");
    let quote = issue_verified_promoted_quote(
        &store,
        &key,
        &csrf,
        &customer,
        Locale::Nl,
        &quote::request("quote"),
        "COURTYARD",
    )
    .await
    .expect("account quote")
    .snapshot;
    let placed = create_verified_promoted_order(
        &store,
        &key,
        &csrf,
        &customer,
        "place",
        &order::intent(&quote.original.binding.quote_id),
        &order::seller(),
    )
    .await
    .expect("bound order")
    .snapshot;
    let before = inventory_and_redemptions(&pool).await;
    if post_attempt {
        prepare_verified_attempt(
            &store,
            &key,
            &csrf,
            &customer,
            "pay",
            &payment::intent(&placed),
            &payment::config(),
        )
        .await
        .expect("admitted account attempt");
    }
    assert!(
        cancel_order(&store, &key, &csrf, "cancel", &placed.identity.id)
            .await
            .is_err()
    );
    assert!(
        cancel_verified_order(&store, &key, &csrf, &other, "cancel", &placed.identity.id)
            .await
            .is_err()
    );
    pool.execute("CREATE FUNCTION test_cancellation_session_wait() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.kind='checkout.order_canceled.v1' THEN PERFORM pg_advisory_xact_lock(1936876941,1); END IF; RETURN NEW; END; $$; CREATE TRIGGER z_test_cancellation_session_wait AFTER INSERT ON commerce_outbox FOR EACH ROW EXECUTE FUNCTION test_cancellation_session_wait()").await.expect("post-write session gate");
    let mut gate = pool.begin().await.expect("session gate");
    sqlx::query("SELECT pg_advisory_xact_lock(1936876941,1)")
        .execute(&mut *gate)
        .await
        .expect("hold gate");
    let work = cancel_verified_order(
        &store,
        &key,
        &csrf,
        &customer,
        "cancel",
        &placed.identity.id,
    );
    let release = async {
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND wait_event='advisory' AND query LIKE '%INSERT INTO commerce_outbox%')").fetch_one(&pool).await.expect("actual event write wait");
                if blocked { break; }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        }).await.expect("event reached gate");
        time.0.store(44300, Ordering::SeqCst);
        gate.rollback().await.expect("release");
    };
    let (result, ()) = tokio::join!(work, release);
    assert!(
        matches!(result, Err(OrderStoreError::Denied)),
        "expired customer session cannot commit"
    );
    assert_eq!(counts(&pool).await, (0, 0, i64::from(post_attempt), 0));
    assert_eq!(inventory_and_redemptions(&pool).await, before);
    time.0.store(1100, Ordering::SeqCst);
    let canceled = cancel_verified_order(
        &store,
        &key,
        &csrf,
        &customer,
        "cancel",
        &placed.identity.id,
    )
    .await
    .expect("authorized cancellation");
    assert_eq!(
        (&canceled.order_id, canceled.canceled_at, canceled.replayed),
        (&placed.identity.id, 1100, false)
    );
    assert!(
        cancel_verified_order(
            &store,
            &key,
            &csrf,
            &customer,
            "cancel",
            &placed.identity.id
        )
        .await
        .expect("replay")
        .replayed
    );
    assert!(
        owned_verified_cancellation(&store, &key, &customer, &placed.identity.id)
            .await
            .is_ok()
    );
    assert!(
        owned_verified_cancellation(&store, &key, &other, &placed.identity.id)
            .await
            .is_err()
    );
    assert!(
        owned_cancellation(&store, &key, &placed.identity.id)
            .await
            .is_err()
    );
    assert!(matches!(
        prepare_verified_attempt(
            &store,
            &key,
            &csrf,
            &customer,
            "pay",
            &payment::intent(&placed),
            &payment::config()
        )
        .await,
        Err(PaymentStoreError::Boundary(OrderStoreError::Conflict))
    ));
    assert_eq!(counts(&pool).await, (1, 1, i64::from(post_attempt), 0));
    assert_eq!(inventory_and_redemptions(&pool).await, before);
    let account_id: i64 = sqlx::query_scalar(
        "SELECT promotion_account_id FROM checkout_order_cancellations WHERE order_id=$1",
    )
    .bind(&placed.identity.id)
    .fetch_one(&pool)
    .await
    .expect("bound marker");
    pool.execute(
        "ALTER TABLE checkout_order_cancellations DISABLE TRIGGER guard_checkout_cancellation",
    )
    .await
    .expect("isolate damaged account marker");
    sqlx::query(
        "UPDATE checkout_order_cancellations SET promotion_account_id=NULL WHERE order_id=$1",
    )
    .bind(&placed.identity.id)
    .execute(&pool)
    .await
    .expect("detach marker");
    assert!(matches!(
        owned_verified_cancellation(&store, &key, &customer, &placed.identity.id).await,
        Err(OrderStoreError::InvalidStoredData)
    ));
    assert!(matches!(
        prepare_verified_attempt(
            &store,
            &key,
            &csrf,
            &customer,
            "pay",
            &payment::intent(&placed),
            &payment::config()
        )
        .await,
        Err(PaymentStoreError::Boundary(OrderStoreError::Conflict))
    ));
    sqlx::query(
        "UPDATE checkout_order_cancellations SET promotion_account_id=$1 WHERE order_id=$2",
    )
    .bind(account_id)
    .bind(&placed.identity.id)
    .execute(&pool)
    .await
    .expect("restore marker");
    pool.execute(
        "ALTER TABLE checkout_order_cancellations ENABLE TRIGGER guard_checkout_cancellation",
    )
    .await
    .expect("restore guard");
    assert!(
        owned_verified_cancellation(&store, &key, &customer, &placed.identity.id)
            .await
            .is_ok(),
        "restored account custody remains readable"
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn bound_pre_attempt_cancellation_preserves_account_authority(pool: PgPool) {
    bound_promoted_cancellation(pool, false).await;
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn bound_undispatched_cancellation_preserves_account_authority(pool: PgPool) {
    bound_promoted_cancellation(pool, true).await;
}

#[sqlx::test(migrations = false)]
async fn upgraded_started_attempt_blocks_cancellation_without_touching_stock(pool: PgPool) {
    MIGRATOR.run_to(55, &pool).await.expect("schema 55");
    let (store, time, session, csrf, placed) = payment::accepted(&pool).await;
    let key = GuestCredential::from_session(&session).expect("owner");
    let config = payment::config();
    let before = inventory_and_redemptions(&pool).await;
    let prepared = prepare_attempt(
        &store,
        &key,
        &csrf,
        "pay",
        &payment::intent(&placed),
        &config,
    )
    .await
    .expect("prepared attempt");
    MIGRATOR.run_to(56, &pool).await.expect("schema 56");
    let restored = prepare_attempt(
        &store,
        &key,
        &csrf,
        "pay",
        &payment::intent(&placed),
        &config,
    )
    .await
    .expect("populated upgrade preserves the original attempt");
    assert!(restored.replayed);
    assert!(restored.frozen.snapshot() == prepared.frozen.snapshot());
    assert_eq!(restored.frozen.body(), prepared.frozen.body());
    let error = sqlx::query("INSERT INTO checkout_order_cancellations(order_id,basket_id,effect_key,canceled_at,event_id) VALUES($1,$2,'native-cancel',1500,'eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee')")
        .bind(&placed.identity.id).bind(&placed.identity.basket_id).execute(&pool).await.expect_err("direct cancellation after payment");
    assert_guard(error, "Cancellation cannot follow a payment attempt");
    MIGRATOR.run_to(61, &pool).await.expect("schema 61");
    let ids = payment::Ids(AtomicU64::new(10000));
    assert!(matches!(
        start_dispatch(
            &pool,
            time.as_ref(),
            &ids,
            &prepared.frozen.snapshot().binding.id,
            &config
        )
        .await,
        Ok(DispatchOutcome::Start(_))
    ));
    assert!(matches!(
        cancel_order(&store, &key, &csrf, "cancel", &placed.identity.id).await,
        Err(OrderStoreError::Conflict)
    ));
    assert_eq!(counts(&pool).await, (0, 0, 1, 1));
    assert_eq!(inventory_and_redemptions(&pool).await, before);
}

async fn competing_payment_and_cancellation(pool: PgPool, cancellation_first: bool) {
    let (store, _, session, csrf, placed) = payment::accepted(&pool).await;
    let before = inventory_and_redemptions(&pool).await;
    let mut gate = pool.begin().await.expect("basket gate");
    sqlx::query("SELECT id FROM guest_baskets WHERE id=$1 FOR UPDATE")
        .bind(&placed.identity.basket_id)
        .execute(&mut *gate)
        .await
        .expect("hold basket");
    let do_cancel = || {
        let store = store.clone();
        let key = GuestCredential::from_session(&session).expect("cancel owner");
        let csrf = csrf.clone();
        let id = placed.identity.id.clone();
        tokio::spawn(async move { cancel_order(&store, &key, &csrf, "cancel", &id).await })
    };
    let do_payment = || {
        let store = store.clone();
        let key = GuestCredential::from_session(&session).expect("payment owner");
        let csrf = csrf.clone();
        let intent = payment::intent(&placed);
        tokio::spawn(async move {
            prepare_attempt(&store, &key, &csrf, "pay", &intent, &payment::config()).await
        })
    };
    let (cancel, pay) = if cancellation_first {
        let first = do_cancel();
        waiting(&pool, 1).await;
        let second = do_payment();
        waiting(&pool, 2).await;
        (first, second)
    } else {
        let first = do_payment();
        waiting(&pool, 1).await;
        let second = do_cancel();
        waiting(&pool, 2).await;
        (second, first)
    };
    gate.rollback().await.expect("release competing contenders");
    let cancel = cancel.await.expect("cancellation task");
    let pay = pay.await.expect("payment task");
    if cancellation_first {
        assert!(cancel.is_ok(), "first cancellation commits");
        assert!(matches!(
            pay,
            Err(PaymentStoreError::Boundary(OrderStoreError::Conflict))
        ));
        assert_eq!(counts(&pool).await, (1, 1, 0, 0));
    } else {
        assert!(pay.is_ok(), "first payment commits");
        assert!(cancel.is_ok(), "undispatched payment can be canceled");
        assert_eq!(counts(&pool).await, (1, 1, 1, 0));
    }
    assert_eq!(inventory_and_redemptions(&pool).await, before);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn cancellation_queued_first_wins_against_payment(pool: PgPool) {
    competing_payment_and_cancellation(pool, true).await;
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn payment_queued_first_wins_against_cancellation(pool: PgPool) {
    competing_payment_and_cancellation(pool, false).await;
}

fn assert_guard(error: sqlx::Error, message: &str) {
    let sqlx::Error::Database(error) = error else {
        panic!("expected native database rejection")
    };
    assert_eq!(error.code().as_deref(), Some("23514"));
    assert_eq!(error.message(), message);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn native_receipt_event_and_payment_exclusion_fail_closed(pool: PgPool) {
    let (store, _, session, _, placed) = payment::accepted(&pool).await;
    let key = GuestCredential::from_session(&session).expect("owner");
    let before = inventory_and_redemptions(&pool).await;
    let id = &placed.identity.id;
    let basket = &placed.identity.basket_id;
    let event_id = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
    let insert = "INSERT INTO checkout_order_cancellations(order_id,basket_id,effect_key,canceled_at,event_id) VALUES($1,$2,'direct',1500,$3)";
    let event = "INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) VALUES($1,$2,'checkout.order_canceled.v1',$3,1500)";
    let mut tx = pool.begin().await.expect("isolation transaction");
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
        .execute(&mut *tx)
        .await
        .expect("repeatable read");
    let error = sqlx::query(insert)
        .bind(id)
        .bind(basket)
        .bind(event_id)
        .execute(&mut *tx)
        .await
        .expect_err("non-read-committed cancellation");
    assert_guard(error, "Cancellation requires read committed");
    tx.rollback().await.expect("rollback");
    let mut tx = pool.begin().await.expect("orphan event transaction");
    let error = sqlx::query(event)
        .bind(event_id)
        .bind(id)
        .bind(serde_json::json!({"schema_version":1,"order_id":id}))
        .execute(&mut *tx)
        .await
        .expect_err("event without receipt");
    assert_guard(
        error,
        "Cancellation event requires exact same-transaction receipt",
    );
    tx.rollback().await.expect("rollback");
    let mut tx = pool.begin().await.expect("missing event transaction");
    sqlx::query(insert)
        .bind(id)
        .bind(basket)
        .bind(event_id)
        .execute(&mut *tx)
        .await
        .expect("native receipt");
    let error = sqlx::query("SET CONSTRAINTS require_complete_checkout_cancellation IMMEDIATE")
        .execute(&mut *tx)
        .await
        .expect_err("missing reciprocal event");
    assert_guard(error, "Cancellation requires its exact event");
    tx.rollback().await.expect("rollback");
    let mut tx = pool.begin().await.expect("forged event transaction");
    sqlx::query(insert)
        .bind(id)
        .bind(basket)
        .bind(event_id)
        .execute(&mut *tx)
        .await
        .expect("native receipt");
    let error = sqlx::query(event)
        .bind(event_id)
        .bind(id)
        .bind(serde_json::json!({"schema_version":1,"order_id":id,"extra":true}))
        .execute(&mut *tx)
        .await
        .expect_err("forged event");
    assert_guard(
        error,
        "Cancellation event requires exact same-transaction receipt",
    );
    tx.rollback().await.expect("rollback");
    assert_eq!(counts(&pool).await, (0, 0, 0, 0));
    let mut tx = pool.begin().await.expect("positive native transaction");
    sqlx::query(insert)
        .bind(id)
        .bind(basket)
        .bind(event_id)
        .execute(&mut *tx)
        .await
        .expect("native receipt");
    sqlx::query(event)
        .bind(event_id)
        .bind(id)
        .bind(serde_json::json!({"schema_version":1,"order_id":id}))
        .execute(&mut *tx)
        .await
        .expect("exact same-transaction event");
    tx.commit().await.expect("reciprocal receipt commits");
    assert!(owned_cancellation(&store, &key, id).await.is_ok());
    let error = sqlx::query("INSERT INTO payment_attempts(id,order_id,basket_id,effect_key,intent,idempotency_key,created_at,dispatch_before,snapshot,configuration,configuration_material,request_body) VALUES('ffffffffffffffffffffffffffffffff',$1,$2,'native-pay','{}','piwas-test-payment-ffffffffffffffffffffffffffffffff',1500,2000,'{}','{}','{}','{}')")
        .bind(id).bind(basket).execute(&pool).await.expect_err("direct payment after cancellation");
    assert_guard(error, "Payment admission cannot follow cancellation");
    assert_eq!(counts(&pool).await, (1, 1, 0, 0));
    assert_eq!(inventory_and_redemptions(&pool).await, before);
    for sql in [
        "UPDATE checkout_order_cancellations SET canceled_at=canceled_at",
        "DELETE FROM checkout_order_cancellations",
    ] {
        assert!(
            pool.execute(sql).await.is_err(),
            "cancellation history is immutable"
        );
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn clock_reversal_after_cancellation_event_write_rolls_back_all_effects(pool: PgPool) {
    let (store, time, session, csrf, placed) = payment::accepted(&pool).await;
    let key = GuestCredential::from_session(&session).expect("owner");
    let before = inventory_and_redemptions(&pool).await;
    pool.execute("CREATE FUNCTION test_cancellation_event_wait() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.kind='checkout.order_canceled.v1' THEN PERFORM pg_advisory_xact_lock(1936876940,1); END IF; RETURN NEW; END; $$; CREATE TRIGGER z_test_cancellation_event_wait AFTER INSERT ON commerce_outbox FOR EACH ROW EXECUTE FUNCTION test_cancellation_event_wait()").await.expect("post-write gate");
    let mut gate = pool.begin().await.expect("gate");
    sqlx::query("SELECT pg_advisory_xact_lock(1936876940,1)")
        .execute(&mut *gate)
        .await
        .expect("hold");
    let work = cancel_order(&store, &key, &csrf, "cancel", &placed.identity.id);
    let release = async {
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND wait_event='advisory' AND query LIKE '%INSERT INTO commerce_outbox%')").fetch_one(&pool).await.expect("actual outbox wait");
                if blocked { break; }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        }).await.expect("event reached write gate");
        time.0.store(1499, Ordering::SeqCst);
        gate.rollback().await.expect("resume write");
    };
    let (result, ()) = tokio::join!(work, release);
    assert!(matches!(result, Err(OrderStoreError::ClockUnavailable)));
    assert_eq!(counts(&pool).await, (0, 0, 0, 0));
    assert_eq!(inventory_and_redemptions(&pool).await, before);
    time.0.store(1500, Ordering::SeqCst);
    assert!(
        !cancel_order(&store, &key, &csrf, "cancel", &placed.identity.id)
            .await
            .expect("valid clock positive control")
            .replayed
    );
    assert_eq!(counts(&pool).await, (1, 1, 0, 0));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn invalid_inputs_and_preorder_clock_are_rejected_without_effects(pool: PgPool) {
    quote::seed(&pool).await;
    let (store, time) = quote::store(&pool, 1100);
    let (session, csrf) = quote::guest(&store).await;
    let key = GuestCredential::from_session(&session).expect("owner");
    let captured = commerce_infrastructure::checkout_quotes::issue_quote(
        &store,
        &key,
        &csrf,
        Locale::Nl,
        &quote::request("quote"),
    )
    .await
    .expect("quote")
    .snapshot;
    time.0.store(1200, Ordering::SeqCst);
    let placed = commerce_infrastructure::checkout_orders::create_order(
        &store,
        &key,
        &csrf,
        "place",
        &order::intent(&captured.binding.quote_id),
        &order::seller(),
    )
    .await
    .expect("later order")
    .snapshot;
    for effect in ["", "bad key"] {
        assert!(matches!(
            cancel_order(&store, &key, &csrf, effect, &placed.identity.id).await,
            Err(OrderStoreError::InvalidRequest)
        ));
    }
    assert!(matches!(
        cancel_order(&store, &key, &csrf, "cancel", "bad").await,
        Err(OrderStoreError::InvalidRequest)
    ));
    assert!(matches!(
        owned_cancellation(&store, &key, "bad").await,
        Err(OrderStoreError::InvalidRequest)
    ));
    time.0.store(1199, Ordering::SeqCst);
    assert!(matches!(
        cancel_order(&store, &key, &csrf, "cancel", &placed.identity.id).await,
        Err(OrderStoreError::ClockUnavailable)
    ));
    assert_eq!(counts(&pool).await, (0, 0, 0, 0));
    time.0.store(1200, Ordering::SeqCst);
    assert!(
        !cancel_order(&store, &key, &csrf, "cancel", &placed.identity.id)
            .await
            .expect("valid request")
            .replayed
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn cancellation_effect_cannot_be_reused_for_another_order_in_the_same_basket(pool: PgPool) {
    quote::seed(&pool).await;
    let (store, _) = quote::store(&pool, 1100);
    let (session, csrf) = quote::guest(&store).await;
    let key = GuestCredential::from_session(&session).expect("guest");
    let first = commerce_infrastructure::checkout_quotes::issue_quote(
        &store,
        &key,
        &csrf,
        Locale::Nl,
        &quote::request("quote-one"),
    )
    .await
    .expect("first quote")
    .snapshot;
    let first = commerce_infrastructure::checkout_orders::create_order(
        &store,
        &key,
        &csrf,
        "order-one",
        &order::intent(&first.binding.quote_id),
        &order::seller(),
    )
    .await
    .expect("first order")
    .snapshot;
    let second = commerce_infrastructure::checkout_quotes::issue_quote(
        &store,
        &key,
        &csrf,
        Locale::Nl,
        &quote::request("quote-two"),
    )
    .await
    .expect("second quote")
    .snapshot;
    let second = commerce_infrastructure::checkout_orders::create_order(
        &store,
        &key,
        &csrf,
        "order-two",
        &order::intent(&second.binding.quote_id),
        &order::seller(),
    )
    .await
    .expect("second order")
    .snapshot;
    assert_eq!(first.identity.basket_id, second.identity.basket_id);
    let before = inventory_and_redemptions(&pool).await;
    cancel_order(&store, &key, &csrf, "same-effect", &first.identity.id)
        .await
        .expect("first cancellation");
    assert!(matches!(
        cancel_order(&store, &key, &csrf, "same-effect", &second.identity.id).await,
        Err(OrderStoreError::Conflict)
    ));
    assert_eq!(counts(&pool).await, (1, 1, 0, 0));
    pool.execute("ALTER TABLE checkout_order_cancellations DISABLE TRIGGER guard_checkout_cancellation; CREATE TRIGGER test_keep_insert_guard BEFORE INSERT ON checkout_order_cancellations FOR EACH ROW EXECUTE FUNCTION guard_checkout_cancellation(); CREATE FUNCTION test_forge_cancellation_xid() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.kind='checkout.order_canceled.v1' THEN UPDATE checkout_order_cancellations SET transaction_id=(SELECT transaction_id FROM checkout_order_cancellations WHERE order_id<>NEW.aggregate_id LIMIT 1) WHERE order_id=NEW.aggregate_id; END IF; RETURN NEW; END; $$; CREATE TRIGGER z_test_forge_cancellation_xid AFTER INSERT ON commerce_outbox FOR EACH ROW EXECUTE FUNCTION test_forge_cancellation_xid()").await.expect("isolated post-event transaction marker forgery");
    assert!(
        matches!(
            cancel_order(&store, &key, &csrf, "other-effect", &second.identity.id).await,
            Err(OrderStoreError::InvalidStoredData)
        ),
        "write must retain its own transaction marker"
    );
    assert_eq!(counts(&pool).await, (1, 1, 0, 0));
    pool.execute("DROP TRIGGER z_test_forge_cancellation_xid ON commerce_outbox; DROP TRIGGER test_keep_insert_guard ON checkout_order_cancellations; ALTER TABLE checkout_order_cancellations ENABLE TRIGGER guard_checkout_cancellation").await.expect("restore guard");
    assert!(
        !cancel_order(&store, &key, &csrf, "other-effect", &second.identity.id)
            .await
            .expect("independent cancellation")
            .replayed
    );
    assert_eq!(counts(&pool).await, (2, 2, 0, 0));
    assert_eq!(inventory_and_redemptions(&pool).await, before);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn effect_corruption_after_event_write_rolls_back_and_healthy_retry_succeeds(pool: PgPool) {
    let (store, _, session, csrf, placed) = payment::accepted(&pool).await;
    let key = GuestCredential::from_session(&session).expect("owner");
    let before = inventory_and_redemptions(&pool).await;
    pool.execute("ALTER TABLE checkout_order_cancellations DISABLE TRIGGER guard_checkout_cancellation; CREATE TRIGGER test_keep_insert_guard BEFORE INSERT ON checkout_order_cancellations FOR EACH ROW EXECUTE FUNCTION guard_checkout_cancellation(); CREATE FUNCTION test_forge_cancellation_effect() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.kind='checkout.order_canceled.v1' THEN UPDATE checkout_order_cancellations SET effect_key='different-effect' WHERE order_id=NEW.aggregate_id; END IF; RETURN NEW; END; $$; CREATE TRIGGER z_test_forge_cancellation_effect AFTER INSERT ON commerce_outbox FOR EACH ROW EXECUTE FUNCTION test_forge_cancellation_effect()").await.expect("isolated post-event corruption");
    assert!(
        matches!(
            cancel_order(&store, &key, &csrf, "cancel", &placed.identity.id).await,
            Err(OrderStoreError::InvalidStoredData)
        ),
        "post-write readback must verify original effect"
    );
    assert_eq!(counts(&pool).await, (0, 0, 0, 0));
    assert_eq!(inventory_and_redemptions(&pool).await, before);
    pool.execute("DROP TRIGGER z_test_forge_cancellation_effect ON commerce_outbox; DROP TRIGGER test_keep_insert_guard ON checkout_order_cancellations; ALTER TABLE checkout_order_cancellations ENABLE TRIGGER guard_checkout_cancellation").await.expect("restore guards");
    assert!(
        !cancel_order(&store, &key, &csrf, "cancel", &placed.identity.id)
            .await
            .expect("intact positive control")
            .replayed
    );
    assert_eq!(counts(&pool).await, (1, 1, 0, 0));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn damaged_guest_receipt_fails_closed_but_still_blocks_payment(pool: PgPool) {
    let (store, time, session, csrf, placed) = payment::accepted(&pool).await;
    let _customer = account(&pool, "other@example.test", &time).await;
    let account_id: i64 = sqlx::query_scalar(
        "SELECT id FROM verified_customer_accounts WHERE email_identity='other@example.test'",
    )
    .fetch_one(&pool)
    .await
    .expect("verified account");
    let key = GuestCredential::from_session(&session).expect("owner");
    cancel_order(&store, &key, &csrf, "cancel", &placed.identity.id)
        .await
        .expect("valid receipt");
    let event_id: String =
        sqlx::query_scalar("SELECT event_id FROM checkout_order_cancellations WHERE order_id=$1")
            .bind(&placed.identity.id)
            .fetch_one(&pool)
            .await
            .expect("event id");
    pool.execute("ALTER TABLE checkout_order_cancellations DISABLE TRIGGER guard_checkout_cancellation; ALTER TABLE checkout_order_cancellations DROP CONSTRAINT checkout_order_cancellations_effect_key_check, DROP CONSTRAINT checkout_order_cancellations_event_id_check").await.expect("isolate damaged imports");
    for (mode, statement) in [
        (
            "account",
            "UPDATE checkout_order_cancellations SET promotion_account_id=$1 WHERE order_id=$2",
        ),
        (
            "effect",
            "UPDATE checkout_order_cancellations SET effect_key='bad key' WHERE $1::bigint IS NOT NULL AND order_id=$2",
        ),
        (
            "event id",
            "UPDATE checkout_order_cancellations SET event_id='bad' WHERE $1::bigint IS NOT NULL AND order_id=$2",
        ),
        (
            "missing event",
            "UPDATE checkout_order_cancellations SET event_id='ffffffffffffffffffffffffffffffff' WHERE $1::bigint IS NOT NULL AND order_id=$2",
        ),
        (
            "preorder time",
            "UPDATE checkout_order_cancellations SET canceled_at=1099 WHERE $1::bigint IS NOT NULL AND order_id=$2",
        ),
    ] {
        sqlx::query(statement)
            .bind(account_id)
            .bind(&placed.identity.id)
            .execute(&pool)
            .await
            .expect("inject receipt damage");
        assert!(
            matches!(
                owned_cancellation(&store, &key, &placed.identity.id).await,
                Err(OrderStoreError::InvalidStoredData)
            ),
            "damaged {mode} cannot authorize history"
        );
        assert!(
            matches!(
                prepare_attempt(
                    &store,
                    &key,
                    &csrf,
                    "pay",
                    &payment::intent(&placed),
                    &payment::config()
                )
                .await,
                Err(PaymentStoreError::Boundary(OrderStoreError::Conflict))
            ),
            "damaged {mode} cannot permit a payment"
        );
        sqlx::query("UPDATE checkout_order_cancellations SET promotion_account_id=NULL,effect_key='cancel',event_id=$1,canceled_at=1500 WHERE order_id=$2").bind(&event_id).bind(&placed.identity.id).execute(&pool).await.expect("restore exact receipt");
        assert!(
            owned_cancellation(&store, &key, &placed.identity.id)
                .await
                .is_ok(),
            "restored {mode} remains readable"
        );
    }
    pool.execute("ALTER TABLE checkout_order_cancellations ADD CONSTRAINT checkout_order_cancellations_effect_key_check CHECK(effect_key ~ '^[a-zA-Z0-9-]{1,128}$'), ADD CONSTRAINT checkout_order_cancellations_event_id_check CHECK(event_id ~ '^[a-f0-9]{32}$'); ALTER TABLE checkout_order_cancellations ENABLE TRIGGER guard_checkout_cancellation").await.expect("restore constraints and immutability");
    assert_eq!(counts(&pool).await, (1, 1, 0, 0));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn contradictory_attempt_and_cancellation_import_fails_closed(pool: PgPool) {
    let (store, _, session, csrf, placed) = payment::accepted(&pool).await;
    let key = GuestCredential::from_session(&session).expect("owner");
    prepare_attempt(
        &store,
        &key,
        &csrf,
        "pay",
        &payment::intent(&placed),
        &payment::config(),
    )
    .await
    .expect("genuine prepared attempt");
    pool.execute(
        "ALTER TABLE checkout_order_cancellations DISABLE TRIGGER guard_checkout_cancellation",
    )
    .await
    .expect("isolate forbidden import");
    let mut tx = pool.begin().await.expect("forbidden import transaction");
    let event = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
    sqlx::query("INSERT INTO checkout_order_cancellations(order_id,basket_id,effect_key,canceled_at,event_id,transaction_id) VALUES($1,$2,'cancel',1500,$3,pg_current_xact_id())").bind(&placed.identity.id).bind(&placed.identity.basket_id).bind(event).execute(&mut *tx).await.expect("simulated contradictory cancellation");
    sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) VALUES($1,$2,'checkout.order_canceled.v1',$3,1500)").bind(event).bind(&placed.identity.id).bind(serde_json::json!({"schema_version":1,"order_id":placed.identity.id})).execute(&mut *tx).await.expect("exact reciprocal event");
    tx.commit()
        .await
        .expect("commit intentionally damaged fixture");
    assert!(matches!(
        owned_cancellation(&store, &key, &placed.identity.id).await,
        Err(OrderStoreError::InvalidStoredData)
    ));
    assert!(
        matches!(
            prepare_attempt(
                &store,
                &key,
                &csrf,
                "pay",
                &payment::intent(&placed),
                &payment::config()
            )
            .await,
            Err(PaymentStoreError::Boundary(OrderStoreError::Conflict))
        ),
        "even existing attempt replay must fail closed"
    );
    pool.execute("ALTER TABLE commerce_outbox DISABLE TRIGGER guard_commerce_outbox")
        .await
        .expect("isolate fixture repair");
    sqlx::query("DELETE FROM commerce_outbox WHERE id=$1")
        .bind(event)
        .execute(&pool)
        .await
        .expect("remove forged event");
    sqlx::query("DELETE FROM checkout_order_cancellations WHERE order_id=$1")
        .bind(&placed.identity.id)
        .execute(&pool)
        .await
        .expect("remove forged cancellation");
    pool.execute("ALTER TABLE commerce_outbox ENABLE TRIGGER guard_commerce_outbox; ALTER TABLE checkout_order_cancellations ENABLE TRIGGER guard_checkout_cancellation").await.expect("restore guards");
    assert!(
        prepare_attempt(
            &store,
            &key,
            &csrf,
            "pay",
            &payment::intent(&placed),
            &payment::config()
        )
        .await
        .expect("intact existing attempt replay")
        .replayed
    );
}
