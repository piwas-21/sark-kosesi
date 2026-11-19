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
    let reviewer = inspection_staff(&pool, "legacy-inspector", 46, "cancellation.review").await;
    assert!(
        commerce_infrastructure::checkout_cancellations::inspect_post_dispatch_request(
            &pool,
            &reviewer,
            time.as_ref(),
            &order.identity.id
        )
        .await
        .expect("legacy inspection")
        .is_some()
    );
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
    time.0.store(1500, Ordering::SeqCst);
    let reviewer = inspection_staff(&pool, "account-inspector", 47, "cancellation.review").await;
    assert!(
        commerce_infrastructure::checkout_cancellations::inspect_post_dispatch_request(
            &pool,
            &reviewer,
            time.as_ref(),
            &order.identity.id
        )
        .await
        .expect("account-bound historical inspection")
        .is_some()
    );
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

async fn inspection_staff(
    pool: &PgPool,
    id: &str,
    byte: u8,
    capability: &str,
) -> commerce_infrastructure::staff_access::SessionToken {
    use commerce_infrastructure::staff_access::SessionToken;
    let token = SessionToken::from_bytes([byte; 32]);
    sqlx::query("INSERT INTO staff_accounts(id,enabled) VALUES($1,true)")
        .bind(id)
        .execute(pool)
        .await
        .expect("staff");
    sqlx::query("INSERT INTO staff_capabilities VALUES($1,$2)")
        .bind(id)
        .bind(capability)
        .execute(pool)
        .await
        .expect("capability");
    sqlx::query("INSERT INTO staff_sessions VALUES($1,$2,1000,4000,1400,NULL)")
        .bind(token.digest().as_slice())
        .bind(id)
        .execute(pool)
        .await
        .expect("session");
    token
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn staff_inspection_requires_live_dedicated_authority_and_exact_historical_custody(
    pool: PgPool,
) {
    use commerce_infrastructure::checkout_cancellations::{
        InspectionError, inspect_post_dispatch_request as inspect,
    };
    let (store, time, session, csrf, order) = payment::accepted(&pool).await;
    let key = GuestCredential::from_session(&session).expect("guest");
    let reviewer = inspection_staff(&pool, "cancel-reviewer", 41, "cancellation.review").await;
    let other = inspection_staff(&pool, "other-reviewer", 42, "stock.review").await;
    let id = &order.identity.id;
    assert!(matches!(
        inspect(&pool, &reviewer, time.as_ref(), "bad").await,
        Err(InspectionError::InvalidRequest)
    ));
    assert_eq!(
        inspect(&pool, &reviewer, time.as_ref(), id)
            .await
            .expect("absent before dispatch"),
        None
    );
    assert!(matches!(
        inspect(&pool, &other, time.as_ref(), id).await,
        Err(InspectionError::Denied)
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
    .expect("prepare");
    let attempt = &prepared.frozen.snapshot().binding.id;
    assert_eq!(
        inspect(&pool, &reviewer, time.as_ref(), id)
            .await
            .expect("absent with attempt"),
        None
    );
    start_dispatch(
        &pool,
        time.as_ref(),
        &payment::Ids(AtomicU64::new(10000)),
        attempt,
        &payment::config(),
    )
    .await
    .expect("dispatch");
    assert_eq!(
        inspect(&pool, &reviewer, time.as_ref(), id)
            .await
            .expect("absent after dispatch"),
        None
    );
    request_after_dispatch(&store, &key, &csrf, "request", id)
        .await
        .expect("pending request");
    let before = footprint(&pool).await;
    let result = inspect(&pool, &reviewer, time.as_ref(), id)
        .await
        .expect("inspection")
        .expect("exists");
    let digest: String = sqlx::query_scalar("SELECT checkout_post_dispatch_request_digest(r) FROM checkout_post_dispatch_requests r WHERE order_id=$1").bind(id).fetch_one(&pool).await.expect("digest");
    assert_eq!(
        (
            &result.order_id,
            result.requested_at,
            &result.request_digest
        ),
        (id, 1500, &digest)
    );
    assert_eq!(footprint(&pool).await, before);
    time.0.store(1499, Ordering::SeqCst);
    assert!(matches!(
        inspect(&pool, &reviewer, time.as_ref(), id).await,
        Err(InspectionError::ClockUnavailable)
    ));
    time.0.store(1500, Ordering::SeqCst);
    sqlx::query("UPDATE staff_sessions SET mfa_verified_at=NULL WHERE token_hash=$1")
        .bind(reviewer.digest().as_slice())
        .execute(&pool)
        .await
        .expect("clear MFA");
    assert!(matches!(
        inspect(&pool, &reviewer, time.as_ref(), id).await,
        Err(InspectionError::Denied)
    ));
    sqlx::query(
        "UPDATE staff_sessions SET mfa_verified_at=1400,revoked_at=1500 WHERE token_hash=$1",
    )
    .bind(reviewer.digest().as_slice())
    .execute(&pool)
    .await
    .expect("revoke");
    assert!(matches!(
        inspect(&pool, &reviewer, time.as_ref(), id).await,
        Err(InspectionError::Denied)
    ));
    sqlx::query("UPDATE staff_sessions SET revoked_at=NULL WHERE token_hash=$1")
        .bind(reviewer.digest().as_slice())
        .execute(&pool)
        .await
        .expect("restore session");
    sqlx::query("UPDATE staff_accounts SET enabled=false WHERE id='cancel-reviewer'")
        .execute(&pool)
        .await
        .expect("disable staff");
    assert!(matches!(
        inspect(&pool, &reviewer, time.as_ref(), id).await,
        Err(InspectionError::Denied)
    ));
    sqlx::query("UPDATE staff_accounts SET enabled=true WHERE id='cancel-reviewer'")
        .execute(&pool)
        .await
        .expect("restore staff");
    sqlx::query("UPDATE staff_sessions SET mfa_verified_at=1501 WHERE token_hash=$1")
        .bind(reviewer.digest().as_slice())
        .execute(&pool)
        .await
        .expect("future MFA");
    assert!(matches!(
        inspect(&pool, &reviewer, time.as_ref(), id).await,
        Err(InspectionError::Denied)
    ));
    sqlx::query("UPDATE staff_sessions SET mfa_verified_at=1400 WHERE token_hash=$1")
        .bind(reviewer.digest().as_slice())
        .execute(&pool)
        .await
        .expect("restore MFA");
    time.0.store(2301, Ordering::SeqCst);
    assert!(matches!(
        inspect(&pool, &reviewer, time.as_ref(), id).await,
        Err(InspectionError::Denied)
    ));
    time.0.store(1500, Ordering::SeqCst);
    let event: serde_json::Value = sqlx::query_scalar("SELECT to_jsonb(e) FROM commerce_outbox e WHERE e.kind='checkout.cancellation_requested.v1'").fetch_one(&pool).await.expect("event");
    pool.execute("ALTER TABLE commerce_outbox DISABLE TRIGGER USER; DELETE FROM commerce_outbox WHERE kind='checkout.cancellation_requested.v1'").await.expect("detach event");
    assert!(matches!(
        inspect(&pool, &reviewer, time.as_ref(), id).await,
        Err(InspectionError::InvalidStoredData)
    ));
    sqlx::query(
        "INSERT INTO commerce_outbox SELECT (jsonb_populate_record(NULL::commerce_outbox,$1)).*",
    )
    .bind(&event)
    .execute(&pool)
    .await
    .expect("restore event");
    pool.execute("ALTER TABLE commerce_outbox ENABLE TRIGGER USER")
        .await
        .expect("restore guard");
    assert_eq!(
        inspect(&pool, &reviewer, time.as_ref(), id)
            .await
            .expect("restored"),
        Some(result)
    );
    assert_eq!(footprint(&pool).await, before);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn staff_inspection_rechecks_authority_after_real_attempt_wait(pool: PgPool) {
    use commerce_infrastructure::checkout_cancellations::{
        InspectionError, inspect_post_dispatch_request as inspect,
    };
    let (store, time, session, csrf, order) = payment::accepted(&pool).await;
    let key = GuestCredential::from_session(&session).expect("guest");
    let reviewer = inspection_staff(&pool, "cancel-wait", 43, "cancellation.review").await;
    let prepared = prepare_attempt(
        &store,
        &key,
        &csrf,
        "pay",
        &payment::intent(&order),
        &payment::config(),
    )
    .await
    .expect("prepare");
    let attempt = &prepared.frozen.snapshot().binding.id;
    start_dispatch(
        &pool,
        time.as_ref(),
        &payment::Ids(AtomicU64::new(10000)),
        attempt,
        &payment::config(),
    )
    .await
    .expect("dispatch");
    request_after_dispatch(&store, &key, &csrf, "request", &order.identity.id)
        .await
        .expect("pending request");
    let mut blocker = pool.begin().await.expect("blocker");
    sqlx::query("SELECT id FROM payment_attempts WHERE id=$1 FOR UPDATE")
        .bind(attempt)
        .execute(&mut *blocker)
        .await
        .expect("attempt lock");
    let operation = inspect(&pool, &reviewer, time.as_ref(), &order.identity.id);
    let release = async {
        tokio::time::timeout(std::time::Duration::from_millis(900), async {
            loop {
                let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE '%SELECT id FROM payment_attempts WHERE id=$1 FOR UPDATE%')").fetch_one(&pool).await.expect("wait observation");
                if waiting { break; }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        }).await.expect("actual wait");
        time.0.store(2301, Ordering::SeqCst);
        blocker.rollback().await.expect("release");
    };
    let (result, ()) = tokio::join!(operation, release);
    assert!(matches!(result, Err(InspectionError::Denied)));
    time.0.store(1500, Ordering::SeqCst);
    assert!(
        inspect(&pool, &reviewer, time.as_ref(), &order.identity.id)
            .await
            .expect("fresh authority")
            .is_some()
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn staff_inspection_rejects_orphan_and_damaged_frozen_source_then_accepts_repairs(
    pool: PgPool,
) {
    use commerce_infrastructure::checkout_cancellations::{
        InspectionError, inspect_post_dispatch_request as inspect,
    };
    let (store, time, session, csrf, order) = payment::accepted(&pool).await;
    let key = GuestCredential::from_session(&session).expect("guest");
    let reviewer = inspection_staff(&pool, "cancel-damage", 44, "cancellation.review").await;
    let prepared = prepare_attempt(
        &store,
        &key,
        &csrf,
        "pay",
        &payment::intent(&order),
        &payment::config(),
    )
    .await
    .expect("prepare");
    let attempt = &prepared.frozen.snapshot().binding.id;
    start_dispatch(
        &pool,
        time.as_ref(),
        &payment::Ids(AtomicU64::new(10000)),
        attempt,
        &payment::config(),
    )
    .await
    .expect("dispatch");
    request_after_dispatch(&store, &key, &csrf, "request", &order.identity.id)
        .await
        .expect("pending request");
    let healthy = inspect(&pool, &reviewer, time.as_ref(), &order.identity.id)
        .await
        .expect("healthy")
        .expect("present");
    let row: serde_json::Value =
        sqlx::query_scalar("SELECT to_jsonb(r) FROM checkout_post_dispatch_requests r")
            .fetch_one(&pool)
            .await
            .expect("request row");
    pool.execute("ALTER TABLE checkout_post_dispatch_requests DISABLE TRIGGER USER; DELETE FROM checkout_post_dispatch_requests").await.expect("detach request");
    assert!(matches!(
        inspect(&pool, &reviewer, time.as_ref(), &order.identity.id).await,
        Err(InspectionError::InvalidStoredData)
    ));
    sqlx::query("INSERT INTO checkout_post_dispatch_requests SELECT (jsonb_populate_record(NULL::checkout_post_dispatch_requests,$1)).*").bind(&row).execute(&pool).await.expect("restore request");
    pool.execute("ALTER TABLE checkout_post_dispatch_requests ENABLE TRIGGER USER")
        .await
        .expect("guard");
    assert_eq!(
        inspect(&pool, &reviewer, time.as_ref(), &order.identity.id)
            .await
            .expect("restored"),
        Some(healthy)
    );
    let original: serde_json::Value =
        sqlx::query_scalar("SELECT snapshot FROM payment_attempts WHERE id=$1")
            .bind(attempt)
            .fetch_one(&pool)
            .await
            .expect("frozen snapshot");
    pool.execute("ALTER TABLE payment_attempts DISABLE TRIGGER USER")
        .await
        .expect("allow isolated damage");
    sqlx::query("UPDATE payment_attempts SET snapshot=jsonb_set(snapshot,'{binding,id}',to_jsonb('ffffffffffffffffffffffffffffffff'::text)) WHERE id=$1").bind(attempt).execute(&pool).await.expect("damage snapshot");
    assert!(matches!(
        inspect(&pool, &reviewer, time.as_ref(), &order.identity.id).await,
        Err(InspectionError::InvalidStoredData)
    ));
    sqlx::query("UPDATE payment_attempts SET snapshot=$2 WHERE id=$1")
        .bind(attempt)
        .bind(original)
        .execute(&pool)
        .await
        .expect("restore snapshot");
    pool.execute("ALTER TABLE payment_attempts ENABLE TRIGGER USER")
        .await
        .expect("restore guard");
    assert!(
        inspect(&pool, &reviewer, time.as_ref(), &order.identity.id)
            .await
            .expect("healthy again")
            .is_some()
    );
    let recovery: String = sqlx::query_scalar(
        "SELECT id FROM commerce_outbox WHERE kind='payment.reconciliation_required.v1'",
    )
    .fetch_one(&pool)
    .await
    .expect("recovery");
    pool.execute("ALTER TABLE commerce_outbox DISABLE TRIGGER USER; ALTER TABLE commerce_outbox DROP CONSTRAINT commerce_outbox_id_check; UPDATE commerce_outbox SET id='bad' WHERE kind='payment.reconciliation_required.v1'").await.expect("damage recovery id");
    assert!(matches!(
        inspect(&pool, &reviewer, time.as_ref(), &order.identity.id).await,
        Err(InspectionError::InvalidStoredData)
    ));
    sqlx::query("UPDATE commerce_outbox SET id=$1 WHERE kind='payment.reconciliation_required.v1'")
        .bind(recovery)
        .execute(&pool)
        .await
        .expect("restore recovery");
    pool.execute("ALTER TABLE commerce_outbox ADD CONSTRAINT commerce_outbox_id_check CHECK(id ~ '^[a-f0-9]{32}$'); ALTER TABLE commerce_outbox ENABLE TRIGGER USER").await.expect("restore guard");
    assert!(
        inspect(&pool, &reviewer, time.as_ref(), &order.identity.id)
            .await
            .expect("healthy final")
            .is_some()
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn inspection_maps_clock_database_and_claim_timeout_failures_without_disclosure(
    pool: PgPool,
) {
    use commerce_application::request_context::{Clock, ContextError};
    use commerce_infrastructure::checkout_cancellations::{
        InspectionError, inspect_post_dispatch_request as inspect,
    };
    struct UnavailableClock;
    impl Clock for UnavailableClock {
        fn unix_seconds(&self) -> Result<u64, ContextError> {
            Err(ContextError::ClockUnavailable)
        }
    }
    let (store, time, session, csrf, order) = payment::accepted(&pool).await;
    let key = GuestCredential::from_session(&session).expect("guest");
    let reviewer = inspection_staff(&pool, "cancel-io", 48, "cancellation.review").await;
    let prepared = prepare_attempt(
        &store,
        &key,
        &csrf,
        "pay",
        &payment::intent(&order),
        &payment::config(),
    )
    .await
    .expect("prepare");
    let attempt = &prepared.frozen.snapshot().binding.id;
    start_dispatch(
        &pool,
        time.as_ref(),
        &payment::Ids(AtomicU64::new(10000)),
        attempt,
        &payment::config(),
    )
    .await
    .expect("dispatch");
    request_after_dispatch(&store, &key, &csrf, "request", &order.identity.id)
        .await
        .expect("pending");
    let id = &order.identity.id;
    assert!(matches!(
        inspect(&pool, &reviewer, &UnavailableClock, id).await,
        Err(InspectionError::ClockUnavailable)
    ));
    pool.execute("ALTER TABLE staff_sessions RENAME TO test_unavailable_staff_sessions")
        .await
        .expect("isolate unavailable authority");
    assert!(matches!(
        inspect(&pool, &reviewer, time.as_ref(), id).await,
        Err(InspectionError::Database)
    ));
    pool.execute("ALTER TABLE test_unavailable_staff_sessions RENAME TO staff_sessions")
        .await
        .expect("restore staff");
    pool.execute("ALTER TABLE payment_attempts RENAME TO test_unavailable_payment_attempts")
        .await
        .expect("isolate unavailable index");
    assert!(matches!(
        inspect(&pool, &reviewer, time.as_ref(), id).await,
        Err(InspectionError::Database)
    ));
    pool.execute("ALTER TABLE test_unavailable_payment_attempts RENAME TO payment_attempts")
        .await
        .expect("restore attempt table");
    pool.execute("ALTER TABLE payment_dispatch_starts RENAME TO test_unavailable_dispatch_starts")
        .await
        .expect("isolate unavailable dispatch");
    assert!(matches!(
        inspect(&pool, &reviewer, time.as_ref(), id).await,
        Err(InspectionError::Database)
    ));
    pool.execute("ALTER TABLE test_unavailable_dispatch_starts RENAME TO payment_dispatch_starts")
        .await
        .expect("restore dispatch");
    pool.execute("ALTER FUNCTION checkout_post_dispatch_request_source(checkout_post_dispatch_requests) RENAME TO test_unavailable_request_source").await.expect("isolate unavailable validation");
    assert!(matches!(
        inspect(&pool, &reviewer, time.as_ref(), id).await,
        Err(InspectionError::Database)
    ));
    pool.execute("ALTER FUNCTION test_unavailable_request_source(checkout_post_dispatch_requests) RENAME TO checkout_post_dispatch_request_source").await.expect("restore validation");
    let mut blocker = pool.begin().await.expect("blocker");
    sqlx::query("SELECT id FROM payment_attempts WHERE id=$1 FOR UPDATE")
        .bind(attempt)
        .execute(&mut *blocker)
        .await
        .expect("attempt lock");
    assert!(matches!(
        inspect(&pool, &reviewer, time.as_ref(), id).await,
        Err(InspectionError::Database)
    ));
    blocker.rollback().await.expect("release lock");
    assert!(
        inspect(&pool, &reviewer, time.as_ref(), id)
            .await
            .expect("restored sources")
            .is_some()
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn inspection_rejects_damaged_attempt_id_and_missing_attempt_with_orphans(pool: PgPool) {
    use commerce_infrastructure::checkout_cancellations::{
        InspectionError, inspect_post_dispatch_request as inspect,
    };
    let (store, time, session, csrf, order) = payment::accepted(&pool).await;
    let key = GuestCredential::from_session(&session).expect("guest");
    let reviewer = inspection_staff(&pool, "cancel-index", 49, "cancellation.review").await;
    let prepared = prepare_attempt(
        &store,
        &key,
        &csrf,
        "pay",
        &payment::intent(&order),
        &payment::config(),
    )
    .await
    .expect("prepare");
    let attempt = &prepared.frozen.snapshot().binding.id;
    start_dispatch(
        &pool,
        time.as_ref(),
        &payment::Ids(AtomicU64::new(10000)),
        attempt,
        &payment::config(),
    )
    .await
    .expect("dispatch");
    request_after_dispatch(&store, &key, &csrf, "request", &order.identity.id)
        .await
        .expect("pending");
    let original: serde_json::Value =
        sqlx::query_scalar("SELECT to_jsonb(a) FROM payment_attempts a WHERE id=$1")
            .bind(attempt)
            .fetch_one(&pool)
            .await
            .expect("attempt");
    let row: serde_json::Value =
        sqlx::query_scalar("SELECT to_jsonb(r) FROM checkout_post_dispatch_requests r")
            .fetch_one(&pool)
            .await
            .expect("request");
    pool.execute("ALTER TABLE payment_attempts DISABLE TRIGGER ALL; ALTER TABLE payment_attempts DROP CONSTRAINT payment_attempts_id_check; ALTER TABLE payment_attempts DROP CONSTRAINT payment_attempts_key_binding").await.expect("isolate attempt corruption");
    sqlx::query("UPDATE payment_attempts SET id='bad' WHERE id=$1")
        .bind(attempt)
        .execute(&pool)
        .await
        .expect("damage id");
    assert!(matches!(
        inspect(&pool, &reviewer, time.as_ref(), &order.identity.id).await,
        Err(InspectionError::InvalidStoredData)
    ));
    sqlx::query("UPDATE payment_attempts SET id=$1 WHERE id='bad'")
        .bind(attempt)
        .execute(&pool)
        .await
        .expect("restore id");
    sqlx::query("DELETE FROM payment_attempts WHERE id=$1")
        .bind(attempt)
        .execute(&pool)
        .await
        .expect("detach attempt");
    assert!(matches!(
        inspect(&pool, &reviewer, time.as_ref(), &order.identity.id).await,
        Err(InspectionError::InvalidStoredData)
    ));
    pool.execute("ALTER TABLE checkout_post_dispatch_requests DISABLE TRIGGER USER; DELETE FROM checkout_post_dispatch_requests").await.expect("detach request");
    assert!(matches!(
        inspect(&pool, &reviewer, time.as_ref(), &order.identity.id).await,
        Err(InspectionError::InvalidStoredData)
    ));
    sqlx::query(
        "INSERT INTO payment_attempts SELECT (jsonb_populate_record(NULL::payment_attempts,$1)).*",
    )
    .bind(original)
    .execute(&pool)
    .await
    .expect("restore attempt");
    sqlx::query("INSERT INTO checkout_post_dispatch_requests SELECT (jsonb_populate_record(NULL::checkout_post_dispatch_requests,$1)).*").bind(row).execute(&pool).await.expect("restore request");
    pool.execute("ALTER TABLE payment_attempts ADD CONSTRAINT payment_attempts_id_check CHECK(id ~ '^[a-f0-9]{32}$'); ALTER TABLE payment_attempts ADD CONSTRAINT payment_attempts_key_binding CHECK(idempotency_key='piwas-test-payment-'||id); ALTER TABLE payment_attempts ENABLE TRIGGER ALL; ALTER TABLE checkout_post_dispatch_requests ENABLE TRIGGER USER").await.expect("restore guards");
    assert!(
        inspect(&pool, &reviewer, time.as_ref(), &order.identity.id)
            .await
            .expect("restored custody")
            .is_some()
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn cancellation_inbox_scans_one_validated_request_with_live_cursor_and_fresh_authority(
    pool: PgPool,
) {
    use commerce_application::Locale;
    use commerce_infrastructure::{
        checkout_cancellations::{InspectionError, next_post_dispatch_request as next},
        checkout_orders::create_order,
        checkout_quotes::issue_quote,
    };
    struct AdvancingClock(AtomicU64);
    impl commerce_application::request_context::Clock for AdvancingClock {
        fn unix_seconds(&self) -> Result<u64, commerce_application::request_context::ContextError> {
            Ok(self.0.fetch_add(1000, Ordering::SeqCst))
        }
    }
    let (store, time, first_session, first_csrf, first_order) = payment::accepted(&pool).await;
    let reviewer = inspection_staff(&pool, "inbox-reviewer", 51, "cancellation.review").await;
    let other = inspection_staff(&pool, "inbox-other", 52, "warehouse.manage").await;
    assert_eq!(
        next(&pool, &reviewer, time.as_ref(), None)
            .await
            .expect("empty"),
        None
    );
    assert!(matches!(
        next(
            &pool,
            &reviewer,
            &AdvancingClock(AtomicU64::new(1500)),
            None
        )
        .await,
        Err(InspectionError::Denied)
    ));

    assert!(matches!(
        next(&pool, &other, time.as_ref(), None).await,
        Err(InspectionError::Denied)
    ));
    assert!(matches!(
        next(&pool, &reviewer, time.as_ref(), Some("bad")).await,
        Err(InspectionError::InvalidRequest)
    ));
    pool.execute("UPDATE stock_balances SET on_hand=100")
        .await
        .expect("isolated stock");
    let mut orders = vec![(first_order, first_session, first_csrf)];
    for _ in 0..2 {
        let (session, csrf) = payment::quote::guest(&store).await;
        let key = GuestCredential::from_session(&session).expect("guest");
        let quote = issue_quote(
            &store,
            &key,
            &csrf,
            Locale::Nl,
            &payment::quote::request("quote"),
        )
        .await
        .expect("quote")
        .snapshot;
        let order = create_order(
            &store,
            &key,
            &csrf,
            "place",
            &payment::order::intent(&quote.binding.quote_id),
            &payment::order::seller(),
        )
        .await
        .expect("order")
        .snapshot;
        orders.push((order, session, csrf));
    }
    orders.sort_by(|a, b| a.0.identity.id.cmp(&b.0.identity.id));
    let ids = payment::Ids(AtomicU64::new(30000));
    for index in [1, 2] {
        let (order, session, csrf) = &orders[index];
        let key = GuestCredential::from_session(session).expect("guest");
        let prepared = prepare_attempt(
            &store,
            &key,
            csrf,
            "pay",
            &payment::intent(order),
            &payment::config(),
        )
        .await
        .expect("prepare");
        start_dispatch(
            &pool,
            time.as_ref(),
            &ids,
            &prepared.frozen.snapshot().binding.id,
            &payment::config(),
        )
        .await
        .expect("dispatch");
        request_after_dispatch(&store, &key, csrf, "request", &order.identity.id)
            .await
            .expect("request");
    }
    let before = footprint(&pool).await;
    let first = next(&pool, &reviewer, time.as_ref(), None)
        .await
        .expect("first")
        .expect("candidate");
    assert_eq!(first.order_id, orders[1].0.identity.id);
    let second = next(&pool, &reviewer, time.as_ref(), Some(&first.order_id))
        .await
        .expect("second")
        .expect("candidate");
    assert_eq!(second.order_id, orders[2].0.identity.id);
    assert_eq!(
        next(&pool, &reviewer, time.as_ref(), Some(&second.order_id))
            .await
            .expect("end"),
        None
    );
    assert_eq!(footprint(&pool).await, before);
    let (order, session, csrf) = &orders[0];
    let key = GuestCredential::from_session(session).expect("guest");
    let prepared = prepare_attempt(
        &store,
        &key,
        csrf,
        "pay",
        &payment::intent(order),
        &payment::config(),
    )
    .await
    .expect("prepare early");
    start_dispatch(
        &pool,
        time.as_ref(),
        &ids,
        &prepared.frozen.snapshot().binding.id,
        &payment::config(),
    )
    .await
    .expect("dispatch early");
    request_after_dispatch(&store, &key, csrf, "request", &order.identity.id)
        .await
        .expect("request early");
    assert_eq!(
        next(&pool, &reviewer, time.as_ref(), Some(&second.order_id))
            .await
            .expect("still at end"),
        None
    );
    assert_eq!(
        next(&pool, &reviewer, time.as_ref(), None)
            .await
            .expect("refresh")
            .expect("candidate")
            .order_id,
        order.identity.id
    );
    let event: serde_json::Value = sqlx::query_scalar("SELECT to_jsonb(e) FROM commerce_outbox e WHERE e.aggregate_id=$1 AND e.kind='checkout.cancellation_requested.v1'")
        .bind(&order.identity.id).fetch_one(&pool).await.expect("event");
    pool.execute("ALTER TABLE commerce_outbox DISABLE TRIGGER USER")
        .await
        .expect("damage fixture");
    sqlx::query("DELETE FROM commerce_outbox WHERE id=$1")
        .bind(event["id"].as_str().expect("id"))
        .execute(&pool)
        .await
        .expect("detach event");
    assert!(matches!(
        next(&pool, &reviewer, time.as_ref(), None).await,
        Err(InspectionError::InvalidStoredData)
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
        .expect("restore guard");
    assert_eq!(
        next(&pool, &reviewer, time.as_ref(), None)
            .await
            .expect("repaired")
            .expect("candidate")
            .order_id,
        order.identity.id
    );
    // The bounded candidate reader rejects a malformed stored key, even when
    // database guards are deliberately bypassed in this isolated fixture.
    pool.execute("ALTER TABLE checkout_post_dispatch_requests DISABLE TRIGGER ALL")
        .await
        .expect("damage request key");
    sqlx::query("UPDATE checkout_post_dispatch_requests SET order_id='bad' WHERE order_id=$1")
        .bind(&order.identity.id)
        .execute(&pool)
        .await
        .expect("malformed key");
    assert!(matches!(
        next(&pool, &reviewer, time.as_ref(), Some(&second.order_id)).await,
        Err(InspectionError::InvalidStoredData)
    ));
    sqlx::query("UPDATE checkout_post_dispatch_requests SET order_id=$1 WHERE order_id='bad'")
        .bind(&order.identity.id)
        .execute(&pool)
        .await
        .expect("restore key");
    pool.execute("ALTER TABLE checkout_post_dispatch_requests ENABLE TRIGGER ALL")
        .await
        .expect("restore request guards");
    pool.execute("ALTER TABLE checkout_post_dispatch_requests RENAME TO test_unavailable_requests")
        .await
        .expect("isolate database failure");
    assert!(matches!(
        next(&pool, &reviewer, time.as_ref(), None).await,
        Err(InspectionError::Database)
    ));
    pool.execute("ALTER TABLE test_unavailable_requests RENAME TO checkout_post_dispatch_requests")
        .await
        .expect("restore request table");
    assert!(
        next(&pool, &reviewer, time.as_ref(), None)
            .await
            .expect("restored custody")
            .is_some()
    );
    let mut tx = pool.begin().await.expect("plan");
    sqlx::query("SET LOCAL enable_seqscan=off")
        .execute(&mut *tx)
        .await
        .expect("plan policy");
    let plan: Vec<String> = sqlx::query_scalar("EXPLAIN SELECT order_id FROM checkout_post_dispatch_requests WHERE order_id COLLATE \"C\">$1 COLLATE \"C\" ORDER BY order_id COLLATE \"C\" LIMIT 1")
        .bind(&first.order_id).fetch_all(&mut *tx).await.expect("plan");
    assert!(
        plan.iter()
            .any(|line| line.contains("checkout_post_dispatch_request_order_c")),
        "{plan:?}"
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn cancellation_inbox_fences_authority_after_real_attempt_wait(pool: PgPool) {
    use commerce_infrastructure::checkout_cancellations::{
        InspectionError, next_post_dispatch_request as next,
    };
    let (store, time, session, csrf, order) = payment::accepted(&pool).await;
    let key = GuestCredential::from_session(&session).expect("guest");
    let reviewer = inspection_staff(&pool, "inbox-wait", 53, "cancellation.review").await;
    let prepared = prepare_attempt(
        &store,
        &key,
        &csrf,
        "pay",
        &payment::intent(&order),
        &payment::config(),
    )
    .await
    .expect("prepare");
    let attempt = &prepared.frozen.snapshot().binding.id;
    start_dispatch(
        &pool,
        time.as_ref(),
        &payment::Ids(AtomicU64::new(10000)),
        attempt,
        &payment::config(),
    )
    .await
    .expect("dispatch");
    request_after_dispatch(&store, &key, &csrf, "request", &order.identity.id)
        .await
        .expect("request");
    let mut blocker = pool.begin().await.expect("blocker");
    sqlx::query("SELECT id FROM payment_attempts WHERE id=$1 FOR UPDATE")
        .bind(attempt)
        .execute(&mut *blocker)
        .await
        .expect("attempt lock");
    let operation = next(&pool, &reviewer, time.as_ref(), None);
    let release = async {
        tokio::time::timeout(std::time::Duration::from_millis(900), async {
            loop {
                let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE '%SELECT id FROM payment_attempts WHERE id=$1 FOR UPDATE%')").fetch_one(&pool).await.expect("actual wait");
                if waiting { break; }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        }).await.expect("attempt wait");
        time.0.store(2301, Ordering::SeqCst);
        blocker.rollback().await.expect("release");
    };
    let (result, ()) = tokio::join!(operation, release);
    assert!(matches!(result, Err(InspectionError::Denied)));
    time.0.store(1500, Ordering::SeqCst);
    assert!(
        next(&pool, &reviewer, time.as_ref(), None)
            .await
            .expect("fresh")
            .is_some()
    );
    // Corruption between selection and the attempt claim is detected after
    // the wait instead of turning the selected candidate into an empty page.
    let request: serde_json::Value =
        sqlx::query_scalar("SELECT to_jsonb(r) FROM checkout_post_dispatch_requests r")
            .fetch_one(&pool)
            .await
            .expect("request");
    let event: serde_json::Value = sqlx::query_scalar("SELECT to_jsonb(e) FROM commerce_outbox e WHERE e.kind='checkout.cancellation_requested.v1'").fetch_one(&pool).await.expect("event");
    let mut blocker = pool.begin().await.expect("second blocker");
    sqlx::query("SELECT id FROM payment_attempts WHERE id=$1 FOR UPDATE")
        .bind(attempt)
        .execute(&mut *blocker)
        .await
        .expect("attempt lock");
    let operation = next(&pool, &reviewer, time.as_ref(), None);
    let damage = async {
        tokio::time::timeout(std::time::Duration::from_millis(900), async {
            loop {
                let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE '%SELECT id FROM payment_attempts WHERE id=$1 FOR UPDATE%')").fetch_one(&pool).await.expect("actual wait");
                if waiting { break; }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        }).await.expect("attempt wait");
        let mut corrupt = pool.begin().await.expect("isolated damage");
        sqlx::query("SET LOCAL session_replication_role=replica")
            .execute(&mut *corrupt)
            .await
            .expect("bypass fixture guards");
        sqlx::query("DELETE FROM checkout_post_dispatch_requests")
            .execute(&mut *corrupt)
            .await
            .expect("detach request");
        sqlx::query("DELETE FROM commerce_outbox WHERE kind='checkout.cancellation_requested.v1'")
            .execute(&mut *corrupt)
            .await
            .expect("detach event");
        corrupt.commit().await.expect("commit isolated corruption");
        blocker.rollback().await.expect("release attempt");
    };
    let (result, ()) = tokio::join!(operation, damage);
    assert!(matches!(result, Err(InspectionError::InvalidStoredData)));
    let mut repair = pool.begin().await.expect("repair");
    sqlx::query("SET LOCAL session_replication_role=replica")
        .execute(&mut *repair)
        .await
        .expect("repair fixture guards");
    sqlx::query("INSERT INTO checkout_post_dispatch_requests SELECT (jsonb_populate_record(NULL::checkout_post_dispatch_requests,$1)).*").bind(request).execute(&mut *repair).await.expect("restore request");
    sqlx::query(
        "INSERT INTO commerce_outbox SELECT (jsonb_populate_record(NULL::commerce_outbox,$1)).*",
    )
    .bind(event)
    .execute(&mut *repair)
    .await
    .expect("restore event");
    repair.commit().await.expect("complete repair");
    assert!(
        next(&pool, &reviewer, time.as_ref(), None)
            .await
            .expect("restored")
            .is_some()
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn combined_cancellation_and_packing_requires_both_grants_and_validates_each_source(
    pool: PgPool,
) {
    use commerce_domain::{
        Currency,
        payment_attempts::{
            PaymentStatus, ProviderPayment,
            observations::{ObservationIntent, ObservationSnapshot},
        },
    };
    use commerce_infrastructure::{
        checkout_cancellations::{
            CancellationPackingEvidence, InspectionError, PackingEvidence,
            inspect_cancellation_packing,
        },
        order_stock::settle,
        packing_custody::{self, Action, Component, Request, Units},
        payment_attempts::{identities::resolve, recognition::recognize},
    };
    // The actual composed reader remains under test; pin it so repeated calls
    // and lock-race joins do not embed its full future in the fixture future.
    fn inspect<'a>(
        pool: &'a PgPool,
        token: &'a commerce_infrastructure::staff_access::SessionToken,
        clock: &'a dyn commerce_application::request_context::Clock,
        order: &'a str,
    ) -> std::pin::Pin<
        Box<
            impl std::future::Future<
                Output = Result<Option<CancellationPackingEvidence>, InspectionError>,
            > + 'a,
        >,
    > {
        Box::pin(inspect_cancellation_packing(pool, token, clock, order))
    }
    let (store, time, session, csrf, order) = Box::pin(payment::accepted(&pool)).await;
    let key = GuestCredential::from_session(&session).expect("guest");
    let reviewer = Box::pin(inspection_staff(
        &pool,
        "combined-reviewer",
        81,
        "cancellation.review",
    ))
    .await;
    let warehouse = Box::pin(inspection_staff(
        &pool,
        "combined-warehouse",
        82,
        "warehouse.manage",
    ))
    .await;
    let id = &order.identity.id;
    assert!(matches!(
        inspect(&pool, &reviewer, time.as_ref(), id).await,
        Err(InspectionError::Denied)
    ));
    assert!(matches!(
        inspect(&pool, &warehouse, time.as_ref(), id).await,
        Err(InspectionError::Denied)
    ));
    pool.execute("INSERT INTO staff_capabilities VALUES('combined-reviewer','warehouse.manage')")
        .await
        .expect("second isolated grant");
    assert!(matches!(
        inspect(&pool, &reviewer, time.as_ref(), "invalid-order-id").await,
        Err(InspectionError::InvalidRequest)
    ));
    assert_eq!(
        inspect(&pool, &reviewer, time.as_ref(), id)
            .await
            .expect("no request"),
        None
    );
    let prepared = Box::pin(prepare_attempt(
        &store,
        &key,
        &csrf,
        "combined-pay",
        &payment::intent(&order),
        &payment::config(),
    ))
    .await
    .expect("prepared");
    let frozen = prepared.frozen.snapshot();
    let attempt = &frozen.binding.id;
    let ids = payment::Ids(AtomicU64::new(200_000));
    Box::pin(start_dispatch(
        &pool,
        time.as_ref(),
        &ids,
        attempt,
        &payment::config(),
    ))
    .await
    .expect("dispatch");
    // The empty view and a concurrent pending request serialize on the attempt.
    let mut blocker = pool.begin().await.expect("attempt blocker");
    sqlx::query("SELECT id FROM payment_attempts WHERE id=$1 FOR UPDATE")
        .bind(attempt)
        .execute(&mut *blocker)
        .await
        .expect("hold attempt");
    let view = inspect(&pool, &reviewer, time.as_ref(), id);
    let request = Box::pin(async {
        tokio::time::timeout(std::time::Duration::from_millis(900), async {
            loop {
                let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE '%SELECT id FROM payment_attempts WHERE id=$1 FOR UPDATE%')").fetch_one(&pool).await.expect("view wait");
                if waiting { break; }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        }).await.expect("view queued first");
        Box::pin(request_after_dispatch(
            &store,
            &key,
            &csrf,
            "combined-request",
            id,
        ))
        .await
        .expect("pending intent")
    });
    let release = Box::pin(async {
        tokio::time::timeout(std::time::Duration::from_millis(900), async {
            loop {
                let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE '%pending cancellation attempt%')").fetch_one(&pool).await.expect("request wait");
                if waiting { break; }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        }).await.expect("request queued after view");
        blocker.rollback().await.expect("release attempt");
    });
    let (prior_view, _, ()) = tokio::join!(view, request, release);
    assert_eq!(prior_view.expect("serialized absent view"), None);
    let before = Box::pin(footprint(&pool)).await;
    let pending = inspect(&pool, &reviewer, time.as_ref(), id)
        .await
        .expect("inspection")
        .expect("request");
    assert_eq!(pending.request.order_id, *id);
    assert_eq!(pending.request.requested_at, 1500);
    assert_eq!(pending.packing, PackingEvidence::Unavailable);
    time.0.store(1499, Ordering::SeqCst);
    assert!(matches!(
        inspect(&pool, &reviewer, time.as_ref(), id).await,
        Err(InspectionError::ClockUnavailable)
    ));
    time.0.store(1500, Ordering::SeqCst);
    assert_eq!(before, Box::pin(footprint(&pool)).await);

    // Sole synthetic provider observation in the disposable fixture, with native
    // constraints and real resolution, recognition, allocation and packing.
    let binding = &frozen.binding;
    let observation = ObservationSnapshot {
        schema_version: 1,
        id: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
        attempt_id: attempt.clone(),
        effect_key: "combined-paid-observation".into(),
        revision: 1,
        recorded_at: 1500,
        intent: ObservationIntent {
            payment: ProviderPayment {
                resource: "payment".into(),
                id: "tr_SyntheticCombinedPayment".into(),
                profile_id: binding.profile.id.clone(),
                mode: binding.profile.mode,
                currency: Currency::EUR,
                amount_minor: binding.amount_minor,
                metadata_schema_version: 1,
                order_id: id.clone(),
                attempt_id: attempt.clone(),
                status: PaymentStatus::Paid,
            },
            credential_fingerprint: binding.profile.credential_fingerprint.clone(),
        },
    };
    let mut tx = pool.begin().await.expect("fixture transaction");
    sqlx::query("INSERT INTO payment_observations(id,attempt_id,effect_key,revision,recorded_at,provider_id,credential_fingerprint,status,snapshot) VALUES($1,$2,$3,1,1500,$4,$5,'paid',$6)")
        .bind(&observation.id).bind(attempt).bind(&observation.effect_key).bind(&observation.intent.payment.id).bind(&observation.intent.credential_fingerprint)
        .bind(serde_json::to_value(&observation).expect("synthetic observation"))
        .execute(&mut *tx).await.expect("observation");
    sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) VALUES('bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb',$1,'payment.observation_recorded.v1',$2,1500)")
        .bind(id).bind(serde_json::json!({"schema_version":1,"order_id":id,"attempt_id":attempt,"observation_id":observation.id}))
        .execute(&mut *tx).await.expect("observation event");
    tx.commit().await.expect("reciprocal observation");
    Box::pin(resolve(
        &pool,
        time.as_ref(),
        &ids,
        attempt,
        "combined-resolution",
    ))
    .await
    .expect("identity");
    time.0.store(1501, Ordering::SeqCst);
    Box::pin(recognize(&pool, time.as_ref(), &ids, attempt))
        .await
        .expect("recognition");
    time.0.store(1502, Ordering::SeqCst);
    Box::pin(settle(&pool, time.as_ref(), &ids, id))
        .await
        .expect("allocation");
    for earlier in [1500, 1501] {
        time.0.store(earlier, Ordering::SeqCst);
        assert!(matches!(
            inspect(&pool, &reviewer, time.as_ref(), id).await,
            Err(InspectionError::ClockUnavailable)
        ));
    }
    time.0.store(1502, Ordering::SeqCst);
    let empty = inspect(&pool, &reviewer, time.as_ref(), id)
        .await
        .expect("validated empty")
        .expect("request");
    assert!(
        matches!(empty.packing, PackingEvidence::Validated(ref value) if value.history.is_empty())
    );
    // A later paid recognition can be newer than an already committed
    // allocation. The selected recognition timestamp remains part of the fence.
    let mut later = observation.clone();
    later.id = "cccccccccccccccccccccccccccccccc".into();
    later.effect_key = "combined-later-paid".into();
    later.revision = 2;
    later.recorded_at = 1503;
    let mut tx = pool.begin().await.expect("later fixture transaction");
    sqlx::query("INSERT INTO payment_observations(id,attempt_id,effect_key,revision,recorded_at,provider_id,credential_fingerprint,status,snapshot) VALUES($1,$2,$3,2,1503,$4,$5,'paid',$6)")
        .bind(&later.id).bind(attempt).bind(&later.effect_key).bind(&later.intent.payment.id).bind(&later.intent.credential_fingerprint)
        .bind(serde_json::to_value(&later).expect("synthetic later observation"))
        .execute(&mut *tx).await.expect("later observation");
    sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) VALUES('dddddddddddddddddddddddddddddddd',$1,'payment.observation_recorded.v1',$2,1503)")
        .bind(id).bind(serde_json::json!({"schema_version":1,"order_id":id,"attempt_id":attempt,"observation_id":later.id}))
        .execute(&mut *tx).await.expect("later observation event");
    tx.commit().await.expect("reciprocal later observation");
    time.0.store(1503, Ordering::SeqCst);
    Box::pin(resolve(
        &pool,
        time.as_ref(),
        &ids,
        attempt,
        "combined-later-resolution",
    ))
    .await
    .expect("later identity");
    Box::pin(recognize(&pool, time.as_ref(), &ids, attempt))
        .await
        .expect("later recognition");
    time.0.store(1502, Ordering::SeqCst);
    assert!(matches!(
        inspect(&pool, &reviewer, time.as_ref(), id).await,
        Err(InspectionError::ClockUnavailable)
    ));
    time.0.store(1503, Ordering::SeqCst);
    assert!(
        matches!(inspect(&pool, &reviewer, time.as_ref(), id).await.expect("current source").expect("request").packing, PackingEvidence::Validated(value) if value.history.is_empty())
    );
    let items = [Units {
        sku: "SET-FILLED".into(),
        units: 1,
    }];
    let packages = [vec![
        Component {
            code: "SEAT-COVER".into(),
            quantity: 2,
        },
        Component {
            code: "SEAT-FOAM".into(),
            quantity: 2,
        },
    ]];
    let packed = Box::pin(packing_custody::record(
        &pool,
        &reviewer,
        time.as_ref(),
        &ids,
        Request {
            order_id: id,
            effect_key: "combined-pack",
            confirmed: true,
            action: Action::Packed {
                units: &items,
                packages: &packages,
            },
        },
    ))
    .await
    .expect("packing assertion");
    let before = Box::pin(footprint(&pool)).await;
    let events_before: i64 = sqlx::query_scalar("SELECT count(*) FROM commerce_outbox")
        .fetch_one(&pool)
        .await
        .expect("event count");
    let combined = inspect(&pool, &reviewer, time.as_ref(), id)
        .await
        .expect("combined")
        .expect("request");
    assert_eq!(combined.request, pending.request);
    match combined.packing {
        PackingEvidence::Validated(value) => {
            assert_eq!(value.history.len(), 1);
            assert_eq!(value.history[0].id, packed.record.id);
            assert_eq!(
                value
                    .lines
                    .iter()
                    .map(|line| (
                        &*line.sku,
                        line.ordered,
                        line.active_asserted,
                        line.not_asserted
                    ))
                    .collect::<Vec<_>>(),
                vec![("SET-FILLED", 3, 1, 2)]
            );
        }
        PackingEvidence::Unavailable => panic!("validated packing must be available"),
    }
    assert_eq!(before, Box::pin(footprint(&pool)).await);
    let events_after: i64 = sqlx::query_scalar("SELECT count(*) FROM commerce_outbox")
        .fetch_one(&pool)
        .await
        .expect("unchanged event count");
    assert_eq!(events_before, events_after);

    // Hold packing history after the view owns the attempt and both grant rows.
    // A concurrent grant removal must wait until that complete view has returned.
    let mut blocker = pool.begin().await.expect("history blocker");
    blocker
        .execute("LOCK TABLE packing_custody IN ACCESS EXCLUSIVE MODE")
        .await
        .expect("hold history");
    let view = inspect(&pool, &reviewer, time.as_ref(), id);
    let revoke_grant = Box::pin(async {
        tokio::time::timeout(std::time::Duration::from_millis(900), async {
            loop {
                let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE '%packing_custody_history_exact%')").fetch_one(&pool).await.expect("view wait");
                if waiting { break; }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        }).await.expect("actual history wait");
        sqlx::query("DELETE FROM staff_capabilities WHERE staff_id='combined-reviewer' AND capability='warehouse.manage' /* combined-grant-delete */")
            .execute(&pool).await.expect("grant removal after view")
    });
    let release = Box::pin(async {
        tokio::time::timeout(std::time::Duration::from_millis(900), async {
            loop {
                let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE '%combined-grant-delete%')").fetch_one(&pool).await.expect("grant wait");
                if waiting { break; }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        }).await.expect("grant locked until view completes");
        blocker.rollback().await.expect("release history");
    });
    let (held_view, removed, ()) = tokio::join!(view, revoke_grant, release);
    assert!(held_view.expect("held view").is_some());
    assert_eq!(removed.rows_affected(), 1);
    assert!(matches!(
        inspect(&pool, &reviewer, time.as_ref(), id).await,
        Err(InspectionError::Denied)
    ));
    pool.execute("INSERT INTO staff_capabilities VALUES('combined-reviewer','warehouse.manage')")
        .await
        .expect("restore grant");

    // Time is checked again after a real wait for the original attempt claim.
    let mut blocker = pool.begin().await.expect("attempt blocker");
    sqlx::query("SELECT id FROM payment_attempts WHERE id=$1 FOR UPDATE")
        .bind(attempt)
        .execute(&mut *blocker)
        .await
        .expect("hold attempt");
    let view = inspect(&pool, &reviewer, time.as_ref(), id);
    let release = Box::pin(async {
        tokio::time::timeout(std::time::Duration::from_millis(900), async {
            loop {
                let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE '%SELECT id FROM payment_attempts WHERE id=$1 FOR UPDATE%')").fetch_one(&pool).await.expect("attempt wait");
                if waiting { break; }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        }).await.expect("actual attempt wait");
        time.0.store(2301, Ordering::SeqCst);
        blocker.rollback().await.expect("release attempt");
    });
    let (expired, ()) = tokio::join!(view, release);
    assert!(matches!(expired, Err(InspectionError::Denied)));
    time.0.store(1503, Ordering::SeqCst);
    assert!(
        inspect(&pool, &reviewer, time.as_ref(), id)
            .await
            .expect("fresh authority")
            .is_some()
    );
    assert_eq!(before, Box::pin(footprint(&pool)).await);
    pool.execute("ALTER TABLE commerce_outbox DISABLE TRIGGER USER; UPDATE commerce_outbox SET payload='{}' WHERE kind='order.packing_custody_recorded.v1'; ALTER TABLE commerce_outbox ENABLE TRIGGER USER").await.expect("damage fixture event");
    assert!(matches!(
        inspect(&pool, &reviewer, time.as_ref(), id).await,
        Err(InspectionError::InvalidStoredData)
    ));
    pool.execute("ALTER TABLE commerce_outbox DISABLE TRIGGER USER; UPDATE commerce_outbox e SET payload=packing_custody_event(p) FROM packing_custody p WHERE p.event_id=e.id AND e.kind='order.packing_custody_recorded.v1'; ALTER TABLE commerce_outbox ENABLE TRIGGER USER").await.expect("restore fixture event");
    assert!(
        inspect(&pool, &reviewer, time.as_ref(), id)
            .await
            .expect("repaired")
            .is_some()
    );
    let event: serde_json::Value = sqlx::query_scalar("SELECT to_jsonb(e) FROM commerce_outbox e WHERE e.kind='checkout.cancellation_requested.v1'").fetch_one(&pool).await.expect("pending event");
    pool.execute("ALTER TABLE commerce_outbox DISABLE TRIGGER USER; DELETE FROM commerce_outbox WHERE kind='checkout.cancellation_requested.v1'").await.expect("detach request event");
    assert!(matches!(
        inspect(&pool, &reviewer, time.as_ref(), id).await,
        Err(InspectionError::InvalidStoredData)
    ));
    sqlx::query(
        "INSERT INTO commerce_outbox SELECT (jsonb_populate_record(NULL::commerce_outbox,$1)).*",
    )
    .bind(&event)
    .execute(&pool)
    .await
    .expect("restore request event");
    pool.execute("ALTER TABLE commerce_outbox ENABLE TRIGGER USER")
        .await
        .expect("restore guard");
    assert!(
        inspect(&pool, &reviewer, time.as_ref(), id)
            .await
            .expect("restored request")
            .is_some()
    );

    // Stop the reader during packing-source validation, after it holds the
    // attempt. Unlike packing_custody, the allocation table is not read by the
    // writer's effect-key lookup before that writer claims the same attempt.
    let mut blocker = pool.begin().await.expect("hold allocation source");
    let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .expect("blocker identity");
    blocker
        .execute("LOCK TABLE order_stock_allocations IN ACCESS EXCLUSIVE MODE")
        .await
        .expect("allocation source lock");
    let (view_pid_tx, view_pid_rx) = tokio::sync::oneshot::channel();
    let view = inspect(&pool, &reviewer, time.as_ref(), id);
    let write = Box::pin(async {
        let view_pid: i32 = tokio::time::timeout(std::time::Duration::from_millis(900), async {
            loop {
                let waiting: Option<i32> = sqlx::query_scalar("SELECT pid FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE '%packing_custody_history_exact%' AND $1=ANY(pg_blocking_pids(pid))")
                    .bind(blocker_pid).fetch_optional(&pool).await.expect("view wait");
                if let Some(pid) = waiting { break pid; }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        }).await.expect("view owns original attempt and waits on allocation source");
        view_pid_tx.send(view_pid).expect("report blocked view");
        Box::pin(packing_custody::record(
            &pool,
            &reviewer,
            time.as_ref(),
            &ids,
            Request {
                order_id: id,
                effect_key: "combined-second-pack",
                confirmed: true,
                action: Action::Packed {
                    units: &items,
                    packages: &packages,
                },
            },
        ))
        .await
        .expect("subsequent packing assertion")
    });
    let release = Box::pin(async {
        let view_pid = view_pid_rx.await.expect("view identity");
        tokio::time::timeout(std::time::Duration::from_millis(900), async {
            loop {
                let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE '%SELECT id FROM payment_attempts WHERE id=$1 FOR UPDATE%' AND $1=ANY(pg_blocking_pids(pid)))")
                    .bind(view_pid).fetch_one(&pool).await.expect("packing wait");
                if waiting { break; }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        }).await.expect("packing writer waits on the view's attempt claim");
        let inserted: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM packing_custody WHERE effect_key='combined-second-pack'",
        )
        .fetch_one(&pool)
        .await
        .expect("no premature packing assertion");
        assert_eq!(inserted, 0);
        blocker.rollback().await.expect("release allocation source");
    });
    let (snapshot, next, ()) = tokio::join!(view, write, release);
    assert!(!next.replayed);
    assert_eq!(next.record.effect_key, "combined-second-pack");
    let event_exact: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM packing_custody p JOIN commerce_outbox e ON e.id=p.event_id WHERE p.id=$1 AND p.order_id=$2 AND p.effect_key='combined-second-pack' AND e.aggregate_id=p.order_id AND e.kind='order.packing_custody_recorded.v1' AND e.payload=packing_custody_event(p))")
        .bind(&next.record.id).bind(id).fetch_one(&pool).await.expect("committed assertion and event");
    assert!(event_exact);
    let earlier = snapshot.expect("snapshot").expect("request");
    assert!(
        matches!(earlier.packing, PackingEvidence::Validated(value) if value.history.len()==1 && value.history[0].id==packed.record.id)
    );
    let later = inspect(&pool, &reviewer, time.as_ref(), id)
        .await
        .expect("later view")
        .expect("request");
    assert!(
        matches!(later.packing, PackingEvidence::Validated(value) if value.history.len()==2 && value.history[1].id==next.record.id)
    );
    Box::pin(packing_custody::record(
        &pool,
        &reviewer,
        time.as_ref(),
        &ids,
        Request {
            order_id: id,
            effect_key: "combined-release",
            confirmed: true,
            action: Action::Released {
                target_id: &packed.record.id,
            },
        },
    ))
    .await
    .expect("release assertion");
    let after_release = inspect(&pool, &reviewer, time.as_ref(), id)
        .await
        .expect("released view")
        .expect("request");
    assert!(
        matches!(after_release.packing, PackingEvidence::Validated(value) if value.history.len()==3 && value.lines[0].active_asserted==1 && value.lines[0].not_asserted==2)
    );
    sqlx::query("UPDATE staff_sessions SET revoked_at=1500 WHERE token_hash=$1")
        .bind(reviewer.digest().as_slice())
        .execute(&pool)
        .await
        .expect("revoke");
    assert!(matches!(
        inspect(&pool, &reviewer, time.as_ref(), id).await,
        Err(InspectionError::Denied)
    ));
}
