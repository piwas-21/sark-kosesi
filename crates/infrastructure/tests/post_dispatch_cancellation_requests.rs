//! Pending post-dispatch intent over isolated PostgreSQL; no provider or fulfilment I/O.
#[allow(dead_code)]
#[path = "support/admitted_login.rs"]
mod admitted_login;
#[allow(dead_code)]
#[path = "support/admitted_proofs.rs"]
mod admitted_proofs;
#[allow(dead_code)]
#[path = "support/payment_fixture.rs"]
mod payment;

use commerce_infrastructure::{
    MIGRATOR,
    checkout_cancellations::{request_after_dispatch, request_verified_after_dispatch},
    checkout_orders::OrderStoreError,
    checkout_orders::create_verified_promoted_order,
    checkout_quotes::issue_verified_promoted_quote,
    customer_sessions::CustomerSessionToken,
    guest_baskets::GuestCredential,
    payment_attempts::{
        DispatchOutcome, prepare_attempt, prepare_verified_attempt, start_dispatch,
    },
};
use sqlx::{Executor, PgPool};
use std::sync::atomic::{AtomicU64, Ordering};

async fn footprint(pool: &PgPool) -> serde_json::Value {
    sqlx::query_scalar("SELECT jsonb_build_object('balances',(SELECT jsonb_agg(to_jsonb(b) ORDER BY component_code) FROM stock_balances b),'holds',(SELECT jsonb_agg(to_jsonb(h) ORDER BY id) FROM stock_holds h),'movements',(SELECT jsonb_agg(to_jsonb(m) ORDER BY sequence) FROM stock_movements m),'redemptions',(SELECT jsonb_agg(to_jsonb(r) ORDER BY order_id) FROM promotion_redemptions r),'cancellations',(SELECT count(*) FROM checkout_order_cancellations),'refunds',(SELECT count(*) FROM refund_reservations),'packing',(SELECT count(*) FROM packing_custody),'observations',(SELECT count(*) FROM payment_observations),'attempts',(SELECT jsonb_agg(to_jsonb(a) ORDER BY id) FROM payment_attempts a),'starts',(SELECT jsonb_agg(to_jsonb(s) ORDER BY id) FROM payment_dispatch_starts s),'recognitions',(SELECT jsonb_agg(to_jsonb(r) ORDER BY id) FROM payment_recognitions r))")
        .fetch_one(pool).await.expect("financial and stock footprint")
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn pending_request_requires_prior_dispatch_and_exact_owner_replays_without_effects(
    pool: PgPool,
) {
    let (store, time, session, csrf, order) = payment::accepted(&pool).await;
    let key = GuestCredential::from_session(&session).expect("guest");
    let (foreign_session, foreign_csrf) = payment::quote::guest(&store).await;
    let foreign = GuestCredential::from_session(&foreign_session).expect("other guest");
    assert!(matches!(
        request_after_dispatch(&store, &key, &csrf, "request", &order.identity.id).await,
        Err(OrderStoreError::Conflict)
    ));
    let prepared = prepare_attempt(
        &store,
        &key,
        &csrf,
        "pay",
        &payment::intent(&order),
        &payment::config(),
    )
    .await
    .expect("prepared");
    assert!(matches!(
        request_after_dispatch(&store, &key, &csrf, "request", &order.identity.id).await,
        Err(OrderStoreError::Conflict)
    ));
    let attempt = &prepared.frozen.snapshot().binding.id;
    assert!(matches!(
        start_dispatch(
            &pool,
            time.as_ref(),
            &payment::Ids(AtomicU64::new(10000)),
            attempt,
            &payment::config()
        )
        .await,
        Ok(DispatchOutcome::Start(_))
    ));
    let recovery: serde_json::Value = sqlx::query_scalar("SELECT to_jsonb(e) FROM commerce_outbox e WHERE e.kind='payment.reconciliation_required.v1'").fetch_one(&pool).await.expect("recovery event");
    pool.execute("ALTER TABLE commerce_outbox DISABLE TRIGGER USER")
        .await
        .expect("isolate recovery damage");
    // Bypass this disposable database's constraints to verify the reader rejects
    // a malformed recovery identifier and timestamp even with its payload intact.
    pool.execute("ALTER TABLE commerce_outbox DROP CONSTRAINT commerce_outbox_id_check")
        .await
        .expect("isolate recovery identifier damage");
    pool.execute(
        "UPDATE commerce_outbox SET id='bad' WHERE kind='payment.reconciliation_required.v1'",
    )
    .await
    .expect("damage recovery identifier");
    assert!(matches!(
        request_after_dispatch(&store, &key, &csrf, "request", &order.identity.id).await,
        Err(OrderStoreError::InvalidStoredData)
    ));
    sqlx::query("UPDATE commerce_outbox SET id=$1 WHERE kind='payment.reconciliation_required.v1'")
        .bind(recovery["id"].as_str().expect("original identifier"))
        .execute(&pool)
        .await
        .expect("restore recovery identifier");
    pool.execute("ALTER TABLE commerce_outbox ADD CONSTRAINT commerce_outbox_id_check CHECK (id ~ '^[a-f0-9]{32}$')")
        .await.expect("restore recovery identifier constraint");
    pool.execute("UPDATE commerce_outbox SET created_at=created_at+1 WHERE kind='payment.reconciliation_required.v1'")
        .await.expect("damage recovery timestamp");
    assert!(matches!(
        request_after_dispatch(&store, &key, &csrf, "request", &order.identity.id).await,
        Err(OrderStoreError::InvalidStoredData)
    ));
    sqlx::query(
        "UPDATE commerce_outbox SET created_at=$1 WHERE kind='payment.reconciliation_required.v1'",
    )
    .bind(recovery["created_at"].as_i64().expect("original timestamp"))
    .execute(&pool)
    .await
    .expect("restore recovery timestamp");
    pool.execute("DELETE FROM commerce_outbox WHERE kind='payment.reconciliation_required.v1'")
        .await
        .expect("detach recovery event");
    assert!(matches!(
        request_after_dispatch(&store, &key, &csrf, "request", &order.identity.id).await,
        Err(OrderStoreError::InvalidStoredData)
    ));
    sqlx::query(
        "INSERT INTO commerce_outbox SELECT (jsonb_populate_record(NULL::commerce_outbox,$1)).*",
    )
    .bind(recovery)
    .execute(&pool)
    .await
    .expect("restore recovery event");
    pool.execute("ALTER TABLE commerce_outbox ENABLE TRIGGER USER")
        .await
        .expect("restore event guards");
    let before = footprint(&pool).await;
    assert!(
        request_after_dispatch(
            &store,
            &foreign,
            &foreign_csrf,
            "request",
            &order.identity.id
        )
        .await
        .is_err()
    );
    assert!(matches!(
        request_after_dispatch(&store, &key, "wrong", "request", &order.identity.id).await,
        Err(OrderStoreError::Csrf)
    ));
    assert!(matches!(
        request_after_dispatch(&store, &key, &csrf, "bad key", &order.identity.id).await,
        Err(OrderStoreError::InvalidRequest)
    ));
    let (one, two) = tokio::join!(
        request_after_dispatch(&store, &key, &csrf, "request", &order.identity.id),
        request_after_dispatch(&store, &key, &csrf, "request", &order.identity.id),
    );
    let (one, two) = (one.expect("first"), two.expect("second"));
    assert_eq!(
        (&one.order_id, one.requested_at),
        (&order.identity.id, 1500)
    );
    assert_eq!(
        (&two.order_id, two.requested_at),
        (&order.identity.id, 1500)
    );
    assert_ne!(one.replayed, two.replayed);
    assert!(matches!(
        request_after_dispatch(&store, &key, &csrf, "different", &order.identity.id).await,
        Err(OrderStoreError::Conflict)
    ));
    assert_eq!(footprint(&pool).await, before);
    let row: (i64, i64, bool) = sqlx::query_as("SELECT (SELECT count(*) FROM checkout_post_dispatch_requests),(SELECT count(*) FROM commerce_outbox WHERE kind='checkout.cancellation_requested.v1'),EXISTS(SELECT 1 FROM checkout_post_dispatch_requests r JOIN commerce_outbox e ON e.id=r.event_id WHERE r.order_id=$1 AND r.attempt_id=$2 AND e.aggregate_id=r.order_id AND e.payload=checkout_post_dispatch_request_event(r) AND public.checkout_post_dispatch_request_source(r))")
        .bind(&order.identity.id).bind(attempt).fetch_one(&pool).await.expect("exact custody");
    assert_eq!(row, (1, 1, true));
    let payload: serde_json::Value = sqlx::query_scalar(
        "SELECT payload FROM commerce_outbox WHERE kind='checkout.cancellation_requested.v1'",
    )
    .fetch_one(&pool)
    .await
    .expect("minimal event");
    assert_eq!(payload.as_object().expect("object").len(), 3);
    assert_eq!(payload["order_id"], order.identity.id);
    assert!(
        payload
            .get("request_sha256")
            .and_then(|v| v.as_str())
            .is_some_and(|v| v.len() == 64)
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn detached_event_fails_closed_and_legacy_dispatch_shape_remains_supported(pool: PgPool) {
    let (store, time, session, csrf, order) = payment::accepted(&pool).await;
    let key = GuestCredential::from_session(&session).expect("guest");
    let prepared = prepare_attempt(
        &store,
        &key,
        &csrf,
        "pay",
        &payment::intent(&order),
        &payment::config(),
    )
    .await
    .expect("prepared");
    let attempt = &prepared.frozen.snapshot().binding.id;
    start_dispatch(
        &pool,
        time.as_ref(),
        &payment::Ids(AtomicU64::new(10000)),
        attempt,
        &payment::config(),
    )
    .await
    .expect("start");
    // Model a pre-migration-0073 start in this disposable database.
    pool.execute(
        "ALTER TABLE payment_dispatch_starts DISABLE TRIGGER guard_payment_dispatch_start",
    )
    .await
    .expect("isolate legacy shape");
    let xid: Option<String> = sqlx::query_scalar(
        "SELECT source_transaction_id::text FROM payment_dispatch_starts WHERE attempt_id=$1",
    )
    .bind(attempt)
    .fetch_one(&pool)
    .await
    .expect("start transaction");
    assert!(xid.is_some());
    sqlx::query(
        "UPDATE payment_dispatch_starts SET source_transaction_id=NULL WHERE attempt_id=$1",
    )
    .bind(attempt)
    .execute(&pool)
    .await
    .expect("legacy start");
    pool.execute("ALTER TABLE payment_dispatch_starts ENABLE TRIGGER guard_payment_dispatch_start")
        .await
        .expect("restore start guard");
    let created = request_after_dispatch(&store, &key, &csrf, "request", &order.identity.id)
        .await
        .expect("legacy dispatch accepted");
    assert!(!created.replayed);
    let event: serde_json::Value = sqlx::query_scalar("SELECT to_jsonb(e) FROM commerce_outbox e WHERE e.kind='checkout.cancellation_requested.v1'").fetch_one(&pool).await.expect("event");
    pool.execute("ALTER TABLE commerce_outbox DISABLE TRIGGER USER")
        .await
        .expect("damage isolated event");
    pool.execute("DELETE FROM commerce_outbox WHERE kind='checkout.cancellation_requested.v1'")
        .await
        .expect("detach event");
    assert!(matches!(
        request_after_dispatch(&store, &key, &csrf, "request", &order.identity.id).await,
        Err(OrderStoreError::InvalidStoredData)
    ));
    sqlx::query(
        "INSERT INTO commerce_outbox SELECT (jsonb_populate_record(NULL::commerce_outbox,$1)).*",
    )
    .bind(event)
    .execute(&pool)
    .await
    .expect("restore event");
    pool.execute("ALTER TABLE commerce_outbox ENABLE TRIGGER USER")
        .await
        .expect("restore event guards");
    assert!(
        request_after_dispatch(&store, &key, &csrf, "request", &order.identity.id)
            .await
            .expect("restored replay")
            .replayed
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn expired_guest_after_actual_outbox_wait_rolls_back_request_and_event(pool: PgPool) {
    let (store, time, session, csrf, order) = payment::accepted(&pool).await;
    let key = GuestCredential::from_session(&session).expect("guest");
    let prepared = prepare_attempt(
        &store,
        &key,
        &csrf,
        "pay",
        &payment::intent(&order),
        &payment::config(),
    )
    .await
    .expect("prepared");
    let attempt = &prepared.frozen.snapshot().binding.id;
    start_dispatch(
        &pool,
        time.as_ref(),
        &payment::Ids(AtomicU64::new(10000)),
        attempt,
        &payment::config(),
    )
    .await
    .expect("start");
    pool.execute("CREATE FUNCTION test_pending_request_wait() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.kind='checkout.cancellation_requested.v1' THEN PERFORM pg_advisory_xact_lock(1936876994,1); END IF; RETURN NEW; END; $$; CREATE TRIGGER z_test_pending_request_wait AFTER INSERT ON commerce_outbox FOR EACH ROW EXECUTE FUNCTION test_pending_request_wait()")
        .await.expect("event gate");
    let mut gate = pool.begin().await.expect("gate");
    sqlx::query("SELECT pg_advisory_xact_lock(1936876994,1)")
        .execute(&mut *gate)
        .await
        .expect("hold");
    let operation = request_after_dispatch(&store, &key, &csrf, "request", &order.identity.id);
    let release = async {
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND wait_event='advisory' AND query LIKE '%checkout.cancellation_requested.v1%')")
                    .fetch_one(&pool).await.expect("actual pending event wait");
                if blocked { break; }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        }).await.expect("pending event reached gate");
        time.0.store(999_999, Ordering::SeqCst);
        gate.rollback().await.expect("release gate");
    };
    let (result, ()) = tokio::join!(operation, release);
    assert!(matches!(result, Err(OrderStoreError::Denied)));
    let counts: (i64,i64) = sqlx::query_as("SELECT (SELECT count(*) FROM checkout_post_dispatch_requests),(SELECT count(*) FROM commerce_outbox WHERE kind='checkout.cancellation_requested.v1')").fetch_one(&pool).await.expect("no partial event");
    assert_eq!(counts, (0, 0));
}

async fn account(pool: &PgPool, email: &str, time: &payment::quote::Time) -> CustomerSessionToken {
    use admitted_proofs::{
        customer_passwords, email_verification, verified_customer_accounts::establish,
    };
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

#[sqlx::test(migrator = "MIGRATOR")]
async fn account_bound_request_requires_both_live_authorities_and_rechecks_after_wait(
    pool: PgPool,
) {
    use commerce_application::Locale;
    payment::quote::seed(&pool).await;
    pool.execute("INSERT INTO staff_capabilities VALUES('author','promotion.manage'),('reviewer','promotion.review')").await.expect("promotion grants");
    let mut policy = payment::promotion::policy();
    policy.limits.per_customer_redemptions = Some(1);
    payment::promotion::draft(&pool, &policy, "draft")
        .await
        .expect("draft");
    payment::promotion::decide(&pool, "review", 1, 0, true)
        .await
        .expect("approval");
    let (store, time) = payment::quote::store(&pool, 1100);
    let customer = account(&pool, "buyer@example.test", &time).await;
    let foreign = account(&pool, "other@example.test", &time).await;
    let (session, csrf) = payment::quote::guest(&store).await;
    let key = GuestCredential::from_session(&session).expect("guest");
    let quote = issue_verified_promoted_quote(
        &store,
        &key,
        &csrf,
        &customer,
        Locale::Nl,
        &payment::quote::request("quote"),
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
        &payment::order::intent(&quote.original.binding.quote_id),
        &payment::order::seller(),
    )
    .await
    .expect("account order")
    .snapshot;
    let prepared = prepare_verified_attempt(
        &store,
        &key,
        &csrf,
        &customer,
        "pay",
        &payment::intent(&order),
        &payment::config(),
    )
    .await
    .expect("account attempt");
    let attempt = &prepared.frozen.snapshot().binding.id;
    start_dispatch(
        &pool,
        time.as_ref(),
        &payment::Ids(AtomicU64::new(10000)),
        attempt,
        &payment::config(),
    )
    .await
    .expect("start");
    assert!(
        request_after_dispatch(&store, &key, &csrf, "request", &order.identity.id)
            .await
            .is_err()
    );
    assert!(
        request_verified_after_dispatch(
            &store,
            &key,
            &csrf,
            &foreign,
            "request",
            &order.identity.id
        )
        .await
        .is_err()
    );
    let before = footprint(&pool).await;
    pool.execute("CREATE FUNCTION test_pending_account_wait() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.kind='checkout.cancellation_requested.v1' THEN PERFORM pg_advisory_xact_lock(1936876995,1); END IF; RETURN NEW; END; $$; CREATE TRIGGER z_test_pending_account_wait AFTER INSERT ON commerce_outbox FOR EACH ROW EXECUTE FUNCTION test_pending_account_wait()").await.expect("account event gate");
    let mut gate = pool.begin().await.expect("gate");
    sqlx::query("SELECT pg_advisory_xact_lock(1936876995,1)")
        .execute(&mut *gate)
        .await
        .expect("hold");
    let operation = request_verified_after_dispatch(
        &store,
        &key,
        &csrf,
        &customer,
        "request",
        &order.identity.id,
    );
    let release = async {
        tokio::time::timeout(std::time::Duration::from_secs(2),async {
            loop {
                let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND wait_event='advisory' AND query LIKE '%checkout.cancellation_requested.v1%')").fetch_one(&pool).await.expect("actual event wait");
                if blocked { break; }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        }).await.expect("account request reached gate");
        time.0.store(44300, Ordering::SeqCst);
        gate.rollback().await.expect("release");
    };
    let (result, ()) = tokio::join!(operation, release);
    assert!(matches!(result, Err(OrderStoreError::Denied)));
    assert_eq!(footprint(&pool).await, before);
    let counts: (i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM checkout_post_dispatch_requests),(SELECT count(*) FROM commerce_outbox WHERE kind='checkout.cancellation_requested.v1')").fetch_one(&pool).await.expect("rollback");
    assert_eq!(counts, (0, 0));
    time.0.store(1100, Ordering::SeqCst);
    let receipt = request_verified_after_dispatch(
        &store,
        &key,
        &csrf,
        &customer,
        "request",
        &order.identity.id,
    )
    .await
    .expect("valid pending request");
    assert!(!receipt.replayed);
    assert!(
        request_verified_after_dispatch(
            &store,
            &key,
            &csrf,
            &customer,
            "request",
            &order.identity.id
        )
        .await
        .expect("bound replay")
        .replayed
    );
    assert_eq!(footprint(&pool).await, before);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn native_request_cannot_share_the_dispatch_start_transaction(pool: PgPool) {
    let (store, time, session, csrf, order) = payment::accepted(&pool).await;
    let key = GuestCredential::from_session(&session).expect("guest");
    let prepared = prepare_attempt(
        &store,
        &key,
        &csrf,
        "pay",
        &payment::intent(&order),
        &payment::config(),
    )
    .await
    .expect("prepared");
    let attempt = &prepared.frozen.snapshot().binding.id;
    let mut tx = pool.begin().await.expect("same transaction");
    let start = "dddddddddddddddddddddddddddddddd";
    let recovery = "cccccccccccccccccccccccccccccccc";
    let event = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
    sqlx::query("INSERT INTO payment_dispatch_starts(attempt_id,id,started_at) VALUES($1,$2,1500)")
        .bind(attempt)
        .bind(start)
        .execute(&mut *tx)
        .await
        .expect("same-transaction start");
    sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) VALUES($1,$2,'payment.reconciliation_required.v1',jsonb_build_object('schema_version',1,'order_id',$2::text,'attempt_id',$3::text,'dispatch_id',$4::text),1500)")
        .bind(recovery).bind(&order.identity.id).bind(attempt).bind(start).execute(&mut *tx).await.expect("same-transaction recovery event");
    let result = sqlx::query("INSERT INTO checkout_post_dispatch_requests(order_id,basket_id,effect_key,attempt_id,dispatch_id,requested_at,event_id) VALUES($1,$2,'same-transaction',$3,$4,1500,$5)")
        .bind(&order.identity.id).bind(&order.identity.basket_id).bind(attempt).bind(start).bind(event).execute(&mut *tx).await;
    let error = result.expect_err("uncommitted start cannot authorize request");
    assert_eq!(
        error.as_database_error().and_then(|e| e.code()).as_deref(),
        Some("23514")
    );
    tx.rollback().await.expect("rollback start");
    start_dispatch(
        &pool,
        time.as_ref(),
        &payment::Ids(AtomicU64::new(10000)),
        attempt,
        &payment::config(),
    )
    .await
    .expect("committed start");
    assert!(
        !request_after_dispatch(&store, &key, &csrf, "request", &order.identity.id)
            .await
            .expect("later request")
            .replayed
    );
}

#[sqlx::test(migrations = false)]
async fn populated_pre_provenance_start_survives_upgrade_and_accepts_pending_request(pool: PgPool) {
    MIGRATOR
        .run_to(72, &pool)
        .await
        .expect("pre-provenance schema");
    let (store, time, session, csrf, order) = payment::accepted(&pool).await;
    let key = GuestCredential::from_session(&session).expect("guest");
    let prepared = prepare_attempt(
        &store,
        &key,
        &csrf,
        "pay",
        &payment::intent(&order),
        &payment::config(),
    )
    .await
    .expect("prepared");
    let attempt = &prepared.frozen.snapshot().binding.id;
    start_dispatch(
        &pool,
        time.as_ref(),
        &payment::Ids(AtomicU64::new(10000)),
        attempt,
        &payment::config(),
    )
    .await
    .expect("legacy start");
    MIGRATOR
        .run(&pool)
        .await
        .expect("upgrade populated database");
    let source: Option<String> = sqlx::query_scalar(
        "SELECT source_transaction_id::text FROM payment_dispatch_starts WHERE attempt_id=$1",
    )
    .bind(attempt)
    .fetch_one(&pool)
    .await
    .expect("legacy source");
    assert!(source.is_none());
    let receipt = request_after_dispatch(&store, &key, &csrf, "request", &order.identity.id)
        .await
        .expect("legacy request");
    assert!(!receipt.replayed);
    assert_eq!(receipt.requested_at, 1500);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn native_missing_event_and_forged_event_are_rejected_with_healthy_control(pool: PgPool) {
    let (store, time, session, csrf, order) = payment::accepted(&pool).await;
    let key = GuestCredential::from_session(&session).expect("guest");
    let prepared = prepare_attempt(
        &store,
        &key,
        &csrf,
        "pay",
        &payment::intent(&order),
        &payment::config(),
    )
    .await
    .expect("prepared");
    let attempt = &prepared.frozen.snapshot().binding.id;
    start_dispatch(
        &pool,
        time.as_ref(),
        &payment::Ids(AtomicU64::new(10000)),
        attempt,
        &payment::config(),
    )
    .await
    .expect("start");
    let dispatch: String =
        sqlx::query_scalar("SELECT id FROM payment_dispatch_starts WHERE attempt_id=$1")
            .bind(attempt)
            .fetch_one(&pool)
            .await
            .expect("start id");
    let mut tx = pool.begin().await.expect("missing-event transaction");
    sqlx::query("INSERT INTO checkout_post_dispatch_requests(order_id,basket_id,effect_key,attempt_id,dispatch_id,requested_at,event_id) VALUES($1,$2,'raw',$3,$4,1500,'eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee')")
        .bind(&order.identity.id).bind(&order.identity.basket_id).bind(attempt).bind(&dispatch).execute(&mut *tx).await.expect("source accepted");
    let missing = sqlx::query("SET CONSTRAINTS require_checkout_post_dispatch_request IMMEDIATE")
        .execute(&mut *tx)
        .await
        .expect_err("missing reciprocal event");
    assert_eq!(
        missing
            .as_database_error()
            .and_then(|e| e.code())
            .as_deref(),
        Some("23514")
    );
    tx.rollback().await.expect("rollback");
    let forged=sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) VALUES('eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee',$1,'checkout.cancellation_requested.v1',jsonb_build_object('schema_version',1,'order_id',$1::text,'request_sha256',repeat('0',64)),1500)").bind(&order.identity.id).execute(&pool).await.expect_err("event without request");
    assert_eq!(
        forged.as_database_error().and_then(|e| e.code()).as_deref(),
        Some("23514")
    );
    assert!(
        !request_after_dispatch(&store, &key, &csrf, "request", &order.identity.id)
            .await
            .expect("healthy request")
            .replayed
    );
    assert!(sqlx::query("UPDATE checkout_post_dispatch_requests SET requested_at=requested_at+1 WHERE order_id=$1").bind(&order.identity.id).execute(&pool).await.is_err());
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn damaged_pending_record_fails_closed_and_restored_record_replays(pool: PgPool) {
    let (store, time, session, csrf, order) = payment::accepted(&pool).await;
    let key = GuestCredential::from_session(&session).expect("guest");
    let prepared = prepare_attempt(
        &store,
        &key,
        &csrf,
        "pay",
        &payment::intent(&order),
        &payment::config(),
    )
    .await
    .expect("prepared");
    start_dispatch(
        &pool,
        time.as_ref(),
        &payment::Ids(AtomicU64::new(10000)),
        &prepared.frozen.snapshot().binding.id,
        &payment::config(),
    )
    .await
    .expect("dispatch");
    assert!(matches!(
        request_after_dispatch(&store, &key, &csrf, "request", "bad").await,
        Err(OrderStoreError::InvalidRequest)
    ));
    request_after_dispatch(&store, &key, &csrf, "request", &order.identity.id)
        .await
        .expect("request");
    let original: (String, String, String, i64, String) = sqlx::query_as(
        "SELECT effect_key,attempt_id,dispatch_id,requested_at,event_id FROM checkout_post_dispatch_requests")
        .fetch_one(&pool).await.expect("original record");
    // Bypass database guards only inside this disposable database to exercise the reader's
    // independent validation of damaged persisted custody, restoring each field afterwards.
    pool.execute("ALTER TABLE checkout_post_dispatch_requests DISABLE TRIGGER ALL; \
        ALTER TABLE checkout_post_dispatch_requests DROP CONSTRAINT checkout_post_dispatch_requests_effect_key_check; \
        ALTER TABLE checkout_post_dispatch_requests DROP CONSTRAINT checkout_post_dispatch_requests_requested_at_check; \
        ALTER TABLE checkout_post_dispatch_requests DROP CONSTRAINT checkout_post_dispatch_requests_event_id_check")
        .await.expect("isolate corruption");
    for damage in [
        "UPDATE checkout_post_dispatch_requests SET promotion_account_id=1",
        "UPDATE checkout_post_dispatch_requests SET effect_key='bad key'",
        "UPDATE checkout_post_dispatch_requests SET attempt_id='bad'",
        "UPDATE checkout_post_dispatch_requests SET dispatch_id='bad'",
        "UPDATE checkout_post_dispatch_requests SET event_id='bad'",
        "UPDATE checkout_post_dispatch_requests SET requested_at=-1",
        "UPDATE checkout_post_dispatch_requests SET dispatch_id='ffffffffffffffffffffffffffffffff'",
    ] {
        pool.execute(damage).await.expect("damage field");
        assert!(
            matches!(
                request_after_dispatch(&store, &key, &csrf, "request", &order.identity.id).await,
                Err(OrderStoreError::InvalidStoredData)
            ),
            "failed to reject {damage}"
        );
        sqlx::query("UPDATE checkout_post_dispatch_requests SET promotion_account_id=NULL,effect_key=$1,attempt_id=$2,dispatch_id=$3,requested_at=$4,event_id=$5")
            .bind(&original.0).bind(&original.1).bind(&original.2).bind(original.3).bind(&original.4)
            .execute(&pool).await.expect("restore original record");
        assert!(
            request_after_dispatch(&store, &key, &csrf, "request", &order.identity.id)
                .await
                .expect("restored record")
                .replayed
        );
    }
    pool.execute(
        "UPDATE checkout_post_dispatch_requests SET order_id='ffffffffffffffffffffffffffffffff'",
    )
    .await
    .expect("detach effect from requested order");
    assert!(matches!(
        request_after_dispatch(&store, &key, &csrf, "request", &order.identity.id).await,
        Err(OrderStoreError::Conflict)
    ));
    sqlx::query("UPDATE checkout_post_dispatch_requests SET order_id=$1")
        .bind(&order.identity.id)
        .execute(&pool)
        .await
        .expect("restore effect owner");

    pool.execute(
        "ALTER TABLE payment_attempts DISABLE TRIGGER ALL; \
        ALTER TABLE payment_attempts DROP CONSTRAINT payment_attempts_id_check; \
        ALTER TABLE payment_attempts DROP CONSTRAINT payment_attempts_key_binding",
    )
    .await
    .expect("isolate attempt corruption");
    sqlx::query("UPDATE payment_attempts SET id='bad' WHERE id=$1")
        .bind(&original.1)
        .execute(&pool)
        .await
        .expect("damage attempt id");
    assert!(matches!(
        request_after_dispatch(&store, &key, &csrf, "request", &order.identity.id).await,
        Err(OrderStoreError::InvalidStoredData)
    ));
    sqlx::query("UPDATE payment_attempts SET id=$1 WHERE id='bad'")
        .bind(&original.1)
        .execute(&pool)
        .await
        .expect("restore attempt id");
    let original_snapshot: serde_json::Value =
        sqlx::query_scalar("SELECT snapshot FROM payment_attempts WHERE id=$1")
            .bind(&original.1)
            .fetch_one(&pool)
            .await
            .expect("snapshot");
    sqlx::query("UPDATE payment_attempts SET snapshot=jsonb_set(snapshot,'{binding,id}',to_jsonb('ffffffffffffffffffffffffffffffff'::text)) WHERE id=$1")
        .bind(&original.1).execute(&pool).await.expect("damage frozen binding");
    assert!(matches!(
        request_after_dispatch(&store, &key, &csrf, "request", &order.identity.id).await,
        Err(OrderStoreError::InvalidStoredData)
    ));
    sqlx::query("UPDATE payment_attempts SET snapshot=$2 WHERE id=$1")
        .bind(&original.1)
        .bind(original_snapshot)
        .execute(&pool)
        .await
        .expect("restore frozen binding");
    assert!(
        request_after_dispatch(&store, &key, &csrf, "request", &order.identity.id)
            .await
            .expect("restored custody")
            .replayed
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn clock_before_dispatch_or_saved_request_fails_closed(pool: PgPool) {
    let (store, time, session, csrf, order) = payment::accepted(&pool).await;
    let key = GuestCredential::from_session(&session).expect("guest");
    let prepared = prepare_attempt(
        &store,
        &key,
        &csrf,
        "pay",
        &payment::intent(&order),
        &payment::config(),
    )
    .await
    .expect("prepared");
    start_dispatch(
        &pool,
        time.as_ref(),
        &payment::Ids(AtomicU64::new(10000)),
        &prepared.frozen.snapshot().binding.id,
        &payment::config(),
    )
    .await
    .expect("dispatch");
    time.0.store(1499, Ordering::SeqCst);
    assert!(matches!(
        request_after_dispatch(&store, &key, &csrf, "request", &order.identity.id).await,
        Err(OrderStoreError::ClockUnavailable)
    ));
    time.0.store(1510, Ordering::SeqCst);
    request_after_dispatch(&store, &key, &csrf, "request", &order.identity.id)
        .await
        .expect("request");
    time.0.store(1509, Ordering::SeqCst);
    assert!(matches!(
        request_after_dispatch(&store, &key, &csrf, "request", &order.identity.id).await,
        Err(OrderStoreError::ClockUnavailable)
    ));
    time.0.store(1510, Ordering::SeqCst);
    assert!(
        request_after_dispatch(&store, &key, &csrf, "request", &order.identity.id)
            .await
            .expect("replay")
            .replayed
    );
}
