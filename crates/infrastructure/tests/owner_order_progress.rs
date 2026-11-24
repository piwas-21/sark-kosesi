//! Private exact-order progress over isolated PostgreSQL with synthetic payment evidence.
#[allow(dead_code)]
#[path = "support/admitted_login.rs"]
mod admitted_login;
#[allow(dead_code)]
#[path = "support/admitted_proofs.rs"]
mod admitted_proofs;
#[allow(dead_code)]
#[path = "support/payment_fixture.rs"]
mod fixture;

use commerce_application::Locale;
use commerce_infrastructure::{
    MIGRATOR,
    checkout_cancellations::{cancel_order, request_after_dispatch},
    checkout_orders::{
        OrderStoreError, OwnerGrossPaymentProgress as Gross, OwnerOrderLifecycle as Lifecycle,
        OwnerOrderProgress, create_verified_promoted_order, owned_order_progress,
        owned_verified_order_progress,
    },
    checkout_quotes::issue_verified_promoted_quote,
    customer_sessions::CustomerSessionToken,
    guest_baskets::GuestCredential,
    payment_attempts::{
        DispatchOutcome,
        identities::resolve,
        owned_verified_dispatch_status, prepare_attempt, prepare_verified_attempt,
        recognition::{State, recognize},
        start_dispatch,
    },
};
use serde_json::{Value, json};
use sqlx::{Executor, PgPool, Postgres, Transaction};
use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

fn accepted(gross_payment: Gross, pending_cancellation: bool) -> OwnerOrderProgress {
    OwnerOrderProgress {
        order: Lifecycle::Accepted,
        pending_cancellation,
        gross_payment,
    }
}

async fn footprint(pool: &PgPool) -> Value {
    sqlx::query_scalar(
        "SELECT jsonb_build_array(
          (SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM checkout_orders t),
          (SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM payment_attempts t),
          (SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM payment_recognitions t),
          (SELECT jsonb_agg(to_jsonb(t) ORDER BY order_id) FROM payment_recognition_current t),
          (SELECT jsonb_agg(to_jsonb(t) ORDER BY order_id) FROM checkout_post_dispatch_requests t),
          (SELECT jsonb_agg(to_jsonb(t) ORDER BY order_id) FROM checkout_order_cancellations t),
          (SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM commerce_outbox t),
          (SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM stock_holds t),
          (SELECT jsonb_agg(to_jsonb(t) ORDER BY component_code) FROM stock_balances t),
          (SELECT count(*) FROM stock_movements),
          (SELECT count(*) FROM refund_reservations),
          (SELECT count(*) FROM packing_custody))",
    )
    .fetch_one(pool)
    .await
    .expect("order, payment, outbox and physical custody footprint")
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn exact_guest_reads_accepted_and_pre_dispatch_receipt_without_effects(pool: PgPool) {
    let (store, _, session, csrf, order) = fixture::accepted(&pool).await;
    let key = GuestCredential::from_session(&session).expect("owner");
    let (foreign, _) = fixture::quote::guest(&store).await;
    let foreign = GuestCredential::from_session(&foreign).expect("other basket");
    let id = &order.identity.id;
    let before = footprint(&pool).await;
    assert_eq!(
        owned_order_progress(&store, &key, id).await,
        Ok(accepted(Gross::NoRecordedDispatch, false))
    );
    for (key, requested) in [
        (&foreign, id.as_str()),
        (&key, "dddddddddddddddddddddddddddddddd"),
    ] {
        assert_eq!(
            owned_order_progress(&store, key, requested).await,
            Err(OrderStoreError::NotFound)
        );
    }
    assert_eq!(
        owned_order_progress(&store, &key, "bad").await,
        Err(OrderStoreError::InvalidRequest)
    );
    assert_eq!(footprint(&pool).await, before);

    prepare_attempt(
        &store,
        &key,
        &csrf,
        "payment",
        &fixture::intent(&order),
        &fixture::config(),
    )
    .await
    .expect("admitted undispatched attempt");
    assert_eq!(
        owned_order_progress(&store, &key, id).await,
        Ok(accepted(Gross::NoRecordedDispatch, false))
    );
    cancel_order(&store, &key, &csrf, "cancel", id)
        .await
        .expect("predispatch cancellation");
    let before = footprint(&pool).await;
    assert_eq!(
        owned_order_progress(&store, &key, id).await,
        Ok(OwnerOrderProgress {
            order: Lifecycle::CancelledBeforeDispatch,
            pending_cancellation: false,
            gross_payment: Gross::NoRecordedDispatch,
        })
    );
    assert_eq!(footprint(&pool).await, before);

    pool.execute(
        "ALTER TABLE checkout_order_cancellations DISABLE TRIGGER guard_checkout_cancellation",
    )
    .await
    .expect("isolated cancellation binding corruption setup");
    sqlx::query("UPDATE checkout_order_cancellations SET basket_id=(SELECT id FROM guest_baskets WHERE id<>$2 LIMIT 1) WHERE order_id=$1")
        .bind(id).bind(&order.identity.basket_id).execute(&pool).await.expect("displace cancellation basket binding");
    assert_eq!(
        owned_order_progress(&store, &key, id).await,
        Err(OrderStoreError::InvalidStoredData)
    );
    sqlx::query("UPDATE checkout_order_cancellations SET basket_id=$2 WHERE order_id=$1")
        .bind(id)
        .bind(&order.identity.basket_id)
        .execute(&pool)
        .await
        .expect("restore exact cancellation basket");
    pool.execute("ALTER TABLE commerce_outbox DISABLE TRIGGER guard_commerce_outbox")
        .await
        .expect("isolated cancellation event corruption setup");
    sqlx::query("UPDATE commerce_outbox SET created_at=created_at+1 WHERE aggregate_id=$1 AND kind='checkout.order_canceled.v1'")
        .bind(id).execute(&pool).await.expect("damage exact cancellation event");
    assert_eq!(
        owned_order_progress(&store, &key, id).await,
        Err(OrderStoreError::InvalidStoredData)
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn missing_evidence_rejects_orphan_events_before_any_progress_is_returned(pool: PgPool) {
    let (store, time, session, csrf, order) = fixture::accepted(&pool).await;
    let key = GuestCredential::from_session(&session).expect("owner");
    let id = &order.identity.id;
    let event_id = "cccccccccccccccccccccccccccccccc";
    let healthy = || owned_order_progress(&store, &key, id);
    let event = async |kind: &str| {
        sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,created_at,payload) VALUES($1,$2,$3,1500,$4)")
            .bind(event_id).bind(id).bind(kind).bind(json!({
                "schema_version": 1, "order_id": id,
                "recognition_id": "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
            }))
            .execute(&pool).await.expect("synthetic detached event");
    };
    let remove = async || {
        sqlx::query("DELETE FROM commerce_outbox WHERE id=$1")
            .bind(event_id)
            .execute(&pool)
            .await
            .expect("remove detached event");
    };
    pool.execute("ALTER TABLE commerce_outbox DISABLE TRIGGER guard_commerce_outbox")
        .await
        .expect("simulated damaged import");
    sqlx::query("UPDATE commerce_outbox SET created_at=created_at+1 WHERE aggregate_id=$1 AND kind='checkout.order_created.v1'")
        .bind(id).execute(&pool).await.expect("damage exact order acceptance event");
    assert_eq!(healthy().await, Err(OrderStoreError::InvalidStoredData));
    sqlx::query("UPDATE commerce_outbox SET created_at=created_at-1 WHERE aggregate_id=$1 AND kind='checkout.order_created.v1'")
        .bind(id).execute(&pool).await.expect("restore exact order acceptance event");
    for kind in [
        "checkout.order_canceled.v1",
        "payment.attempt_prepared.v1",
        "payment.reconciliation_required.v1",
        "payment.recognized.v1",
    ] {
        assert_eq!(
            healthy().await,
            Ok(accepted(Gross::NoRecordedDispatch, false))
        );
        event(kind).await;
        assert_eq!(
            healthy().await,
            Err(OrderStoreError::InvalidStoredData),
            "{kind}"
        );
        remove().await;
    }
    let configuration = fixture::config();
    let prepared = prepare_attempt(
        &store,
        &key,
        &csrf,
        "pay",
        &fixture::intent(&order),
        &configuration,
    )
    .await
    .expect("prepared attempt");
    assert!(matches!(
        start_dispatch(
            &pool,
            time.as_ref(),
            &fixture::Ids(AtomicU64::new(10000)),
            &prepared.frozen.snapshot().binding.id,
            &configuration
        )
        .await,
        Ok(DispatchOutcome::Start(_))
    ));
    assert_eq!(healthy().await, Ok(accepted(Gross::NoRecognition, false)));
    event("payment.recognized.v1").await;
    assert_eq!(healthy().await, Err(OrderStoreError::InvalidStoredData));
    remove().await;
    assert_eq!(healthy().await, Ok(accepted(Gross::NoRecognition, false)));
    pool.execute(
        "ALTER TABLE payment_dispatch_starts DISABLE TRIGGER guard_payment_dispatch_start",
    )
    .await
    .expect("isolated local-start corruption setup");
    sqlx::query("UPDATE payment_dispatch_starts SET started_at=1499 WHERE attempt_id=$1")
        .bind(&prepared.frozen.snapshot().binding.id)
        .execute(&pool)
        .await
        .expect("damage start time");
    assert_eq!(healthy().await, Err(OrderStoreError::InvalidStoredData));
}

#[sqlx::test(migrations = false)]
async fn recognition_order_index_preserves_populated_history_on_upgrade(pool: PgPool) {
    MIGRATOR.run_to(83, &pool).await.expect("prior schema");
    let (store, time, session, csrf, order) = fixture::accepted(&pool).await;
    let key = GuestCredential::from_session(&session).expect("owner");
    let id = &order.identity.id;
    let configuration = fixture::config();
    let prepared = prepare_attempt(
        &store,
        &key,
        &csrf,
        "pay",
        &fixture::intent(&order),
        &configuration,
    )
    .await
    .expect("prepared attempt");
    let attempt = &prepared.frozen.snapshot().binding.id;
    let ids = fixture::Ids(AtomicU64::new(10000));
    assert!(matches!(
        start_dispatch(&pool, time.as_ref(), &ids, attempt, &configuration).await,
        Ok(DispatchOutcome::Start(_))
    ));
    fixture::append_observation(
        &pool,
        prepared.frozen.snapshot(),
        1,
        "tr_SyntheticA",
        "paid",
    )
    .await;
    resolve(&pool, time.as_ref(), &ids, attempt, "resolve-paid")
        .await
        .expect("resolve paid observation");
    recognize(&pool, time.as_ref(), &ids, attempt)
        .await
        .expect("received gross recognition");
    let before = footprint(&pool).await;
    MIGRATOR
        .run_to(84, &pool)
        .await
        .expect("install scoped index");
    assert_eq!(footprint(&pool).await, before);
    assert_eq!(
        owned_order_progress(&store, &key, id).await,
        Ok(accepted(Gross::Recognized(State::Received), false))
    );
    let mut tx = pool.begin().await.expect("planner probe");
    sqlx::query("SET LOCAL enable_seqscan=off")
        .execute(&mut *tx)
        .await
        .expect("probe index applicability");
    let plan: Vec<String> = sqlx::query_scalar("EXPLAIN SELECT 1 FROM public.commerce_outbox WHERE aggregate_id=$1 AND kind='payment.recognized.v1' LIMIT 1")
        .bind(id).fetch_all(&mut *tx).await.expect("exact per-order lookup plan");
    assert!(
        plan.iter()
            .any(|line| line.contains("commerce_outbox_payment_recognition_order"))
    );
    assert!(
        plan.iter()
            .any(|line| line.contains("Index Cond:") && line.contains("aggregate_id"))
    );
    tx.rollback().await.expect("finish explain");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn pending_intent_and_current_gross_recognition_stay_independent(pool: PgPool) {
    let (store, time, session, csrf, order) = fixture::accepted(&pool).await;
    let key = GuestCredential::from_session(&session).expect("owner");
    let id = &order.identity.id;
    let configuration = fixture::config();
    let prepared = prepare_attempt(
        &store,
        &key,
        &csrf,
        "payment",
        &fixture::intent(&order),
        &configuration,
    )
    .await
    .expect("prepared")
    .frozen;
    let attempt = &prepared.snapshot().binding.id;
    let ids = fixture::Ids(AtomicU64::new(10000));
    assert!(matches!(
        start_dispatch(&pool, time.as_ref(), &ids, attempt, &configuration).await,
        Ok(DispatchOutcome::Start(_))
    ));
    assert_eq!(
        owned_order_progress(&store, &key, id).await,
        Ok(accepted(Gross::NoRecognition, false))
    );
    request_after_dispatch(&store, &key, &csrf, "cancel-request", id)
        .await
        .expect("recorded intent");
    assert_eq!(
        owned_order_progress(&store, &key, id).await,
        Ok(accepted(Gross::NoRecognition, true))
    );

    for (revision, provider, status, expected) in [
        (0, "", "", State::Unconfirmed),
        (1, "tr_SyntheticA", "paid", State::Received),
        (2, "tr_SyntheticB", "open", State::NeedsReview),
    ] {
        let effect = format!("resolve-{revision}");
        if revision != 0 {
            fixture::append_observation(&pool, prepared.snapshot(), revision, provider, status)
                .await;
            assert_eq!(
                owned_order_progress(&store, &key, id).await,
                Err(OrderStoreError::Unavailable),
                "obsolete recognition cannot be projected"
            );
        }
        resolve(&pool, time.as_ref(), &ids, attempt, &effect)
            .await
            .expect("resolve current authenticated prefix");
        time.0.store(1530, Ordering::SeqCst);
        recognize(&pool, time.as_ref(), &ids, attempt)
            .await
            .expect("recognize canonical current prefix");
        let before = footprint(&pool).await;
        assert_eq!(
            owned_order_progress(&store, &key, id).await,
            Ok(accepted(Gross::Recognized(expected), true))
        );
        time.0.store(1515, Ordering::SeqCst);
        assert_eq!(
            owned_order_progress(&store, &key, id).await,
            Err(OrderStoreError::ClockUnavailable)
        );
        time.0.store(1530, Ordering::SeqCst);
        assert_eq!(footprint(&pool).await, before);
    }
    pool.execute("ALTER TABLE commerce_outbox DISABLE TRIGGER guard_commerce_outbox")
        .await
        .expect("isolated pending event corruption setup");
    sqlx::query("UPDATE commerce_outbox SET created_at=created_at+1 WHERE aggregate_id=$1 AND kind='checkout.cancellation_requested.v1'")
        .bind(id).execute(&pool).await.expect("damage exact pending event");
    assert_eq!(
        owned_order_progress(&store, &key, id).await,
        Err(OrderStoreError::InvalidStoredData)
    );
}

async fn attempt_lock(pool: &PgPool, id: &str) -> (Transaction<'static, Postgres>, i32) {
    let mut tx = pool.begin().await.expect("attempt blocker");
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *tx)
        .await
        .expect("blocker process");
    sqlx::query("SELECT id FROM payment_attempts WHERE id=$1 FOR UPDATE")
        .bind(id)
        .execute(&mut *tx)
        .await
        .expect("original attempt lock");
    (tx, pid)
}

async fn waiting(pool: &PgPool, blocker: i32) {
    tokio::time::timeout(Duration::from_millis(800), async {
        loop {
            let waiting: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pg_stat_activity
             WHERE datname=current_database() AND wait_event_type='Lock'
             AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%/* owned dispatch status */%')",
            )
            .bind(blocker)
            .fetch_one(pool)
            .await
            .expect("observe actual attempt waiter");
            if waiting {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("owner read reached the original attempt");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn guest_expiry_masks_source_error_after_real_attempt_wait(pool: PgPool) {
    let (store, time, session, csrf, order) = fixture::accepted(&pool).await;
    let key = GuestCredential::from_session(&session).expect("owner");
    let id = &order.identity.id;
    let prepared = prepare_attempt(
        &store,
        &key,
        &csrf,
        "payment",
        &fixture::intent(&order),
        &fixture::config(),
    )
    .await
    .expect("prepared");
    let attempt = &prepared.frozen.snapshot().binding.id;
    let expiry: u64 = u64::try_from(
        sqlx::query_scalar::<_, i64>("SELECT expires_at FROM guest_sessions WHERE basket_id=$1")
            .bind(&order.identity.basket_id)
            .fetch_one(&pool)
            .await
            .expect("guest expiry"),
    )
    .expect("positive fixture clock");
    pool.execute("ALTER TABLE commerce_outbox DISABLE TRIGGER guard_commerce_outbox")
        .await
        .expect("isolated prepared event corruption setup");
    sqlx::query("UPDATE commerce_outbox SET created_at=created_at+1 WHERE aggregate_id=$1 AND kind='payment.attempt_prepared.v1'")
        .bind(id).execute(&pool).await.expect("damage exact preparation event");
    assert_eq!(
        owned_order_progress(&store, &key, id).await,
        Err(OrderStoreError::InvalidStoredData)
    );
    let (blocked, _) = attempt_lock(&pool, attempt).await;
    assert_eq!(
        owned_order_progress(&store, &key, id).await,
        Err(OrderStoreError::Database),
        "a live holder sees a redacted lock timeout"
    );
    blocked.rollback().await.expect("release after timeout");
    let (blocker, pid) = attempt_lock(&pool, attempt).await;
    let release = async {
        waiting(&pool, pid).await;
        time.0.store(expiry, Ordering::SeqCst);
        blocker.rollback().await.expect("release");
    };
    let (result, ()) = tokio::join!(owned_order_progress(&store, &key, id), release);
    assert_eq!(result, Err(OrderStoreError::Denied));
    time.0.store(1500, Ordering::SeqCst);
    sqlx::query("UPDATE commerce_outbox SET created_at=created_at-1 WHERE aggregate_id=$1 AND kind='payment.attempt_prepared.v1'")
        .bind(id).execute(&pool).await.expect("restore exact preparation event");
    assert_eq!(
        owned_order_progress(&store, &key, id).await,
        Ok(accepted(Gross::NoRecordedDispatch, false))
    );
}

async fn account(pool: &PgPool, email: &str, time: &fixture::quote::Time) -> CustomerSessionToken {
    use admitted_proofs::{
        customer_passwords, email_verification, verified_customer_accounts::establish,
    };
    let proof = email_verification::issue(pool, email, time)
        .await
        .expect("verify proof");
    establish(pool, email, &proof, time)
        .await
        .expect("verified account");
    let proof = customer_passwords::issue(pool, email, time)
        .await
        .expect("password proof");
    let password = "correct horse battery staple";
    customer_passwords::replace(pool, email, &proof, password, time)
        .await
        .expect("password custody");
    admitted_login::login(pool, email, password, time)
        .await
        .expect("owned session")
        .token
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn account_bound_progress_requires_both_held_authorities_after_wait(pool: PgPool) {
    fixture::quote::seed(&pool).await;
    pool.execute("INSERT INTO staff_capabilities VALUES('author','promotion.manage'),('reviewer','promotion.review')")
        .await.expect("promotion grants");
    let mut policy = fixture::promotion::policy();
    policy.limits.per_customer_redemptions = Some(1);
    fixture::promotion::draft(&pool, &policy, "draft")
        .await
        .expect("draft");
    fixture::promotion::decide(&pool, "review", 1, 0, true)
        .await
        .expect("review");
    let (store, time) = fixture::quote::store(&pool, 1100);
    let customer = account(&pool, "buyer@example.test", &time).await;
    let foreign = account(&pool, "other@example.test", &time).await;
    let (session, csrf) = fixture::quote::guest(&store).await;
    let key = GuestCredential::from_session(&session).expect("basket");
    let quote = issue_verified_promoted_quote(
        &store,
        &key,
        &csrf,
        &customer,
        Locale::Nl,
        &fixture::quote::request("quote"),
        "COURTYARD",
    )
    .await
    .expect("account quote")
    .snapshot;
    let order = create_verified_promoted_order(
        &store,
        &key,
        &csrf,
        &customer,
        "place",
        &fixture::order::intent(&quote.original.binding.quote_id),
        &fixture::order::seller(),
    )
    .await
    .expect("account order")
    .snapshot;
    let id = &order.identity.id;
    let configuration = fixture::config();
    let prepared = prepare_verified_attempt(
        &store,
        &key,
        &csrf,
        &customer,
        "pay",
        &fixture::intent(&order),
        &configuration,
    )
    .await
    .expect("bound attempt");
    let attempt = &prepared.frozen.snapshot().binding.id;
    let expiry: (i64, i64) = sqlx::query_as(
        "SELECT
       (SELECT min(expires_at) FROM customer_sessions),
       (SELECT expires_at FROM guest_sessions WHERE basket_id=$1)",
    )
    .bind(&order.identity.basket_id)
    .fetch_one(&pool)
    .await
    .expect("session expiry window");
    assert!(expiry.0 < expiry.1, "account-only expiration");
    assert_eq!(
        owned_order_progress(&store, &key, id).await,
        Err(OrderStoreError::NotFound)
    );
    assert_eq!(
        owned_verified_order_progress(&store, &key, &foreign, id).await,
        Err(OrderStoreError::NotFound)
    );
    assert_eq!(
        owned_verified_order_progress(&store, &key, &customer, id).await,
        Ok(accepted(Gross::NoRecordedDispatch, false))
    );
    let (blocker, pid) = attempt_lock(&pool, attempt).await;
    let release = async {
        waiting(&pool, pid).await;
        time.0.store(
            u64::try_from(expiry.0).expect("account expiry"),
            Ordering::SeqCst,
        );
        blocker.rollback().await.expect("release");
    };
    let (result, ()) = tokio::join!(
        owned_verified_order_progress(&store, &key, &customer, id),
        release
    );
    assert_eq!(result, Err(OrderStoreError::Denied));
    time.0.store(1100, Ordering::SeqCst);
    assert!(matches!(
        start_dispatch(
            &pool,
            time.as_ref(),
            &fixture::Ids(AtomicU64::new(10000)),
            attempt,
            &configuration
        )
        .await,
        Ok(DispatchOutcome::Start(_))
    ));
    assert!(
        owned_verified_dispatch_status(&store, &key, &customer, id)
            .await
            .is_ok()
    );
    assert_eq!(
        owned_verified_order_progress(&store, &key, &customer, id).await,
        Ok(accepted(Gross::NoRecognition, false))
    );
}
