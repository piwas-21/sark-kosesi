//! Owner-scoped dispatch evidence over disposable PostgreSQL, without provider I/O.
#[allow(dead_code)]
#[path = "support/payment_fixture.rs"]
mod fixture;
use commerce_infrastructure::{
    MIGRATOR,
    checkout_orders::OrderStoreError,
    guest_baskets::{GuestCredential, revoke_session},
    payment_attempts::{
        DispatchOutcome, PaymentStoreError, RecordedDispatch, owned_dispatch_status,
        prepare_attempt, start_dispatch,
    },
};
use serde_json::json;
use sqlx::{Executor, PgPool};
use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

async fn waiting(pool: &PgPool, blocker: i32) {
    tokio::time::timeout(Duration::from_millis(800), async {
        loop {
            let found: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_catalog.pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_catalog.pg_blocking_pids(pid)) AND query LIKE '%/* owned dispatch status */%')")
                .bind(blocker).fetch_one(pool).await.expect("actual attempt waiter");
            if found { return; }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }).await.expect("reader reached exact attempt lock");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn owner_reads_only_recorded_dispatch_and_rejects_damaged_event(pool: PgPool) {
    let (store, time, session, csrf, order) = fixture::accepted(&pool).await;
    let key = GuestCredential::from_session(&session).expect("owner");
    let (other, _) = fixture::quote::guest(&store).await;
    let other = GuestCredential::from_session(&other).expect("other owner");
    let oid = &order.identity.id;
    assert_eq!(
        owned_dispatch_status(&store, &key, oid).await,
        Err(PaymentStoreError::Unavailable)
    );
    let prepared = prepare_attempt(
        &store,
        &key,
        &csrf,
        "pay",
        &fixture::intent(&order),
        &fixture::config(),
    )
    .await
    .expect("prepared");
    assert_eq!(
        owned_dispatch_status(&store, &key, oid).await,
        Err(PaymentStoreError::Unavailable)
    );
    let id = &prepared.frozen.snapshot().binding.id;
    let id_checks: Vec<(String, String)> = sqlx::query_as("SELECT pg_catalog.quote_ident(c.conname),pg_catalog.pg_get_constraintdef(c.oid) FROM pg_catalog.pg_constraint c WHERE c.conrelid='public.payment_attempts'::pg_catalog.regclass AND c.contype='c' AND c.conkey=ARRAY[(SELECT attnum FROM pg_catalog.pg_attribute WHERE attrelid=c.conrelid AND attname='id')]::smallint[] AND pg_catalog.pg_get_constraintdef(c.oid) LIKE '%~%'")
        .fetch_all(&pool).await.expect("attempt ID check");
    assert_eq!(id_checks.len(), 1);
    let (check, definition) = &id_checks[0];
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!("ALTER TABLE payment_attempts DISABLE TRIGGER guard_payment_attempt; ALTER TABLE payment_attempts DROP CONSTRAINT {check}")))
        .execute(&pool).await.expect("synthetic imported ID fault");
    sqlx::query("UPDATE payment_attempts SET id='invalid-import' WHERE id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .expect("invalid attempt identity");
    assert_eq!(
        owned_dispatch_status(&store, &key, oid).await,
        Err(PaymentStoreError::InvalidStoredData)
    );
    sqlx::query("UPDATE payment_attempts SET id=$1 WHERE id='invalid-import'")
        .bind(id)
        .execute(&pool)
        .await
        .expect("restore attempt identity");
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!("ALTER TABLE payment_attempts ADD CONSTRAINT {check} {definition}; ALTER TABLE payment_attempts ENABLE TRIGGER guard_payment_attempt")))
        .execute(&pool).await.expect("restore attempt checks");
    assert!(matches!(
        start_dispatch(
            &pool,
            time.as_ref(),
            &fixture::Ids(AtomicU64::new(10000)),
            id,
            &fixture::config()
        )
        .await,
        Ok(DispatchOutcome::Start(_))
    ));
    let before = fixture::counts(&pool).await;
    let balances: serde_json::Value = sqlx::query_scalar(
        "SELECT jsonb_agg(to_jsonb(b) ORDER BY component_code) FROM stock_balances b",
    )
    .fetch_one(&pool)
    .await
    .expect("stock snapshot");
    let mut planner = pool.begin().await.expect("planner transaction");
    sqlx::query("SET LOCAL enable_seqscan=off")
        .execute(&mut *planner)
        .await
        .expect("index probe");
    let plan: Vec<String> = sqlx::query_scalar("EXPLAIN SELECT id FROM commerce_outbox WHERE aggregate_id=$1 AND kind='payment.reconciliation_required.v1' LIMIT 2")
        .bind(oid).fetch_all(&mut *planner).await.expect("bounded event plan");
    assert!(
        plan.iter()
            .any(|line| line.contains("commerce_outbox_original_order_kind"))
    );
    planner.rollback().await.expect("finish planner probe");
    assert_eq!(
        owned_dispatch_status(&store, &key, oid).await,
        Ok(RecordedDispatch)
    );
    assert!(owned_dispatch_status(&store, &other, oid).await.is_err());
    assert_eq!(
        owned_dispatch_status(&store, &key, "bad").await,
        Err(PaymentStoreError::InvalidRequest)
    );

    let saved: (String, i64, serde_json::Value) = sqlx::query_as("SELECT id,created_at,payload FROM commerce_outbox WHERE aggregate_id=$1 AND kind='payment.reconciliation_required.v1'")
        .bind(oid).fetch_one(&pool).await.expect("original event");
    pool.execute("ALTER TABLE commerce_outbox DISABLE TRIGGER guard_commerce_outbox")
        .await
        .expect("synthetic fault");
    sqlx::query("UPDATE commerce_outbox SET created_at=created_at+1 WHERE id=$1")
        .bind(&saved.0)
        .execute(&pool)
        .await
        .expect("damage event time");
    assert_eq!(
        owned_dispatch_status(&store, &key, oid).await,
        Err(PaymentStoreError::InvalidStoredData)
    );
    sqlx::query("UPDATE commerce_outbox SET created_at=$2 WHERE id=$1")
        .bind(&saved.0)
        .bind(saved.1)
        .execute(&pool)
        .await
        .expect("restore event time");
    let id_checks: Vec<(String, String)> = sqlx::query_as("SELECT pg_catalog.quote_ident(c.conname),pg_catalog.pg_get_constraintdef(c.oid) FROM pg_catalog.pg_constraint c WHERE c.conrelid='public.commerce_outbox'::pg_catalog.regclass AND c.contype='c' AND c.conkey=ARRAY[(SELECT attnum FROM pg_catalog.pg_attribute WHERE attrelid=c.conrelid AND attname='id')]::smallint[] AND pg_catalog.pg_get_constraintdef(c.oid) LIKE '%~%'")
        .fetch_all(&pool).await.expect("event ID check");
    assert_eq!(id_checks.len(), 1);
    let (check, definition) = &id_checks[0];
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "ALTER TABLE commerce_outbox DROP CONSTRAINT {check}"
    )))
    .execute(&pool)
    .await
    .expect("synthetic imported event fault");
    sqlx::query("UPDATE commerce_outbox SET id='invalid-import' WHERE id=$1")
        .bind(&saved.0)
        .execute(&pool)
        .await
        .expect("damage event ID");
    assert_eq!(
        owned_dispatch_status(&store, &key, oid).await,
        Err(PaymentStoreError::InvalidStoredData)
    );
    sqlx::query("UPDATE commerce_outbox SET id=$1 WHERE id='invalid-import'")
        .bind(&saved.0)
        .execute(&pool)
        .await
        .expect("restore event ID");
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "ALTER TABLE commerce_outbox ADD CONSTRAINT {check} {definition}"
    )))
    .execute(&pool)
    .await
    .expect("restore event check");
    sqlx::query("UPDATE commerce_outbox SET payload=jsonb_set(payload,'{dispatch_id}',to_jsonb('bad'::text)) WHERE id=$1")
        .bind(&saved.0).execute(&pool).await.expect("damage event");
    assert_eq!(
        owned_dispatch_status(&store, &key, oid).await,
        Err(PaymentStoreError::InvalidStoredData)
    );
    sqlx::query("DELETE FROM commerce_outbox WHERE id=$1")
        .bind(&saved.0)
        .execute(&pool)
        .await
        .expect("remove event");
    assert_eq!(
        owned_dispatch_status(&store, &key, oid).await,
        Err(PaymentStoreError::InvalidStoredData)
    );
    sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) VALUES($1,$2,'payment.reconciliation_required.v1',$3,$4)")
        .bind(&saved.0).bind(oid).bind(&saved.2).bind(saved.1).execute(&pool).await.expect("restore exact event");
    let index: String = sqlx::query_scalar("SELECT pg_catalog.pg_get_indexdef('commerce_outbox_original_order_kind'::pg_catalog.regclass)")
        .fetch_one(&pool).await.expect("original unique index");
    pool.execute("DROP INDEX commerce_outbox_original_order_kind")
        .await
        .expect("synthetic duplicate fault");
    sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) VALUES('cccccccccccccccccccccccccccccccc',$1,'payment.reconciliation_required.v1',$2,$3)")
        .bind(oid).bind(&saved.2).bind(saved.1).execute(&pool).await.expect("duplicate event");
    assert_eq!(
        owned_dispatch_status(&store, &key, oid).await,
        Err(PaymentStoreError::InvalidStoredData)
    );
    pool.execute("DELETE FROM commerce_outbox WHERE id='cccccccccccccccccccccccccccccccc'")
        .await
        .expect("remove duplicate");
    sqlx::raw_sql(sqlx::AssertSqlSafe(index))
        .execute(&pool)
        .await
        .expect("restore index");
    pool.execute("ALTER TABLE commerce_outbox ENABLE TRIGGER guard_commerce_outbox")
        .await
        .expect("restore guard");
    pool.execute(
        "ALTER TABLE payment_dispatch_starts DISABLE TRIGGER guard_payment_dispatch_start",
    )
    .await
    .expect("start fault");
    sqlx::query("UPDATE payment_dispatch_starts SET started_at=1499 WHERE attempt_id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .expect("damage start time");
    assert_eq!(
        owned_dispatch_status(&store, &key, oid).await,
        Err(PaymentStoreError::InvalidStoredData)
    );
    sqlx::query("UPDATE payment_dispatch_starts SET started_at=1500 WHERE attempt_id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .expect("restore start time");
    pool.execute("ALTER TABLE payment_dispatch_starts ENABLE TRIGGER guard_payment_dispatch_start")
        .await
        .expect("restore start guard");
    assert_eq!(
        owned_dispatch_status(&store, &key, oid).await,
        Ok(RecordedDispatch)
    );
    let other_basket: String =
        sqlx::query_scalar("SELECT id FROM guest_baskets WHERE id<>$1 LIMIT 1")
            .bind(&order.identity.basket_id)
            .fetch_one(&pool)
            .await
            .expect("other valid basket");
    pool.execute("ALTER TABLE payment_attempts DISABLE TRIGGER guard_payment_attempt")
        .await
        .expect("attempt owner fault");
    sqlx::query("UPDATE payment_attempts SET basket_id=$2 WHERE id=$1")
        .bind(id)
        .bind(&other_basket)
        .execute(&pool)
        .await
        .expect("damage attempt owner");
    assert_eq!(
        owned_dispatch_status(&store, &key, oid).await,
        Err(PaymentStoreError::InvalidStoredData)
    );
    sqlx::query("UPDATE payment_attempts SET basket_id=$2 WHERE id=$1")
        .bind(id)
        .bind(&order.identity.basket_id)
        .execute(&pool)
        .await
        .expect("restore attempt owner");
    pool.execute("ALTER TABLE payment_attempts ENABLE TRIGGER guard_payment_attempt")
        .await
        .expect("restore attempt owner guard");
    let original_body: String =
        sqlx::query_scalar("SELECT request_body FROM payment_attempts WHERE id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .expect("original frozen request");
    pool.execute("ALTER TABLE payment_attempts DISABLE TRIGGER guard_payment_attempt")
        .await
        .expect("attempt fault");
    sqlx::query("UPDATE payment_attempts SET request_body=request_body||' ' WHERE id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .expect("damage frozen request");
    assert_eq!(
        owned_dispatch_status(&store, &key, oid).await,
        Err(PaymentStoreError::InvalidStoredData)
    );
    sqlx::query("UPDATE payment_attempts SET request_body=$2 WHERE id=$1")
        .bind(id)
        .bind(&original_body)
        .execute(&pool)
        .await
        .expect("restore frozen request");
    pool.execute("ALTER TABLE payment_attempts ENABLE TRIGGER guard_payment_attempt")
        .await
        .expect("restore attempt guard");
    pool.execute("ALTER TABLE checkout_order_cancellations DISABLE TRIGGER guard_checkout_cancellation; ALTER TABLE checkout_order_cancellations DISABLE TRIGGER require_complete_checkout_cancellation").await.expect("cancellation fault");
    sqlx::query("INSERT INTO checkout_order_cancellations(order_id,basket_id,effect_key,canceled_at,event_id,transaction_id,attempt_id) VALUES($1,$2,'synthetic-fault',1500,'dddddddddddddddddddddddddddddddd',pg_current_xact_id(),$3)")
        .bind(oid).bind(&order.identity.basket_id).bind(id).execute(&pool).await.expect("contradictory cancellation");
    assert_eq!(
        owned_dispatch_status(&store, &key, oid).await,
        Err(PaymentStoreError::InvalidStoredData)
    );
    sqlx::query("DELETE FROM checkout_order_cancellations WHERE order_id=$1")
        .bind(oid)
        .execute(&pool)
        .await
        .expect("remove cancellation fault");
    pool.execute("ALTER TABLE checkout_order_cancellations ENABLE TRIGGER guard_checkout_cancellation; ALTER TABLE checkout_order_cancellations ENABLE TRIGGER require_complete_checkout_cancellation").await.expect("restore cancellation guards");
    time.0.store(1499, Ordering::SeqCst);
    assert_eq!(
        owned_dispatch_status(&store, &key, oid).await,
        Err(PaymentStoreError::ClockUnavailable)
    );
    time.0.store(1500, Ordering::SeqCst);
    assert_eq!(
        owned_dispatch_status(&store, &key, oid).await,
        Ok(RecordedDispatch)
    );
    assert_eq!(fixture::counts(&pool).await, before);
    let after: serde_json::Value = sqlx::query_scalar(
        "SELECT jsonb_agg(to_jsonb(b) ORDER BY component_code) FROM stock_balances b",
    )
    .fetch_one(&pool)
    .await
    .expect("stock after read");
    assert_eq!(after, balances);
    revoke_session(&store, &session, &csrf)
        .await
        .expect("revoke guest");
    assert!(matches!(
        owned_dispatch_status(&store, &key, oid).await,
        Err(PaymentStoreError::Boundary(OrderStoreError::Denied))
    ));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn reader_waits_for_dispatch_commit_and_reads_new_start(pool: PgPool) {
    let (store, _time, session, csrf, order) = fixture::accepted(&pool).await;
    let key = GuestCredential::from_session(&session).expect("owner");
    let prepared = prepare_attempt(
        &store,
        &key,
        &csrf,
        "pay",
        &fixture::intent(&order),
        &fixture::config(),
    )
    .await
    .expect("prepared");
    let id = &prepared.frozen.snapshot().binding.id;
    let mut blocker = pool.begin().await.expect("dispatch transaction");
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .expect("backend");
    sqlx::query("SELECT id FROM payment_attempts WHERE id=$1 FOR UPDATE")
        .bind(id)
        .execute(&mut *blocker)
        .await
        .expect("dispatch attempt lock");
    let start = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    sqlx::query("INSERT INTO payment_dispatch_starts(attempt_id,id,started_at) VALUES($1,$2,1500)")
        .bind(id)
        .bind(start)
        .execute(&mut *blocker)
        .await
        .expect("start");
    sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) VALUES('bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb',$1,'payment.reconciliation_required.v1',$2,1500)")
        .bind(&order.identity.id).bind(json!({"schema_version":1,"order_id":order.identity.id,"attempt_id":id,"dispatch_id":start})).execute(&mut *blocker).await.expect("matching event");
    let work = owned_dispatch_status(&store, &key, &order.identity.id);
    let release = async {
        waiting(&pool, pid).await;
        blocker.commit().await.expect("commit start");
    };
    let (result, ()) = tokio::join!(work, release);
    assert_eq!(result, Ok(RecordedDispatch));
    assert_eq!(fixture::counts(&pool).await.0, 1);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn expiry_after_attempt_wait_masks_both_custody_and_missing_start(pool: PgPool) {
    let (store, time, session, csrf, order) = fixture::accepted(&pool).await;
    let key = GuestCredential::from_session(&session).expect("owner");
    let prepared = prepare_attempt(
        &store,
        &key,
        &csrf,
        "pay",
        &fixture::intent(&order),
        &fixture::config(),
    )
    .await
    .expect("prepared");
    let id = &prepared.frozen.snapshot().binding.id;
    let expiry: i64 =
        sqlx::query_scalar("SELECT expires_at FROM guest_sessions WHERE basket_id=$1")
            .bind(&order.identity.basket_id)
            .fetch_one(&pool)
            .await
            .expect("guest expiry");
    for started in [false, true] {
        if started {
            assert!(matches!(
                start_dispatch(
                    &pool,
                    time.as_ref(),
                    &fixture::Ids(AtomicU64::new(10000)),
                    id,
                    &fixture::config()
                )
                .await,
                Ok(DispatchOutcome::Start(_))
            ));
        }
        let mut blocker = pool.begin().await.expect("blocker");
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .expect("backend");
        sqlx::query("SELECT id FROM payment_attempts WHERE id=$1 FOR UPDATE")
            .bind(id)
            .execute(&mut *blocker)
            .await
            .expect("attempt lock");
        let work = owned_dispatch_status(&store, &key, &order.identity.id);
        let release = async {
            waiting(&pool, pid).await;
            time.0
                .store(u64::try_from(expiry).expect("expiry"), Ordering::SeqCst);
            blocker.commit().await.expect("release");
        };
        let (result, ()) = tokio::join!(work, release);
        assert!(matches!(
            result,
            Err(PaymentStoreError::Boundary(OrderStoreError::Denied))
        ));
        time.0.store(1500, Ordering::SeqCst);
        assert_eq!(
            owned_dispatch_status(&store, &key, &order.identity.id).await,
            if started {
                Ok(RecordedDispatch)
            } else {
                Err(PaymentStoreError::Unavailable)
            }
        );
    }
}
