//! Durable private lookup custody; disposable DB and synthetic authenticated GETs.
use super::observation_cases::{BadPorts, ids, prepared};
use super::*;
use crate::payment_attempts::{callbacks::lookup::*, identities};
use sqlx::{Executor, PgPool};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

const PROFILE: &str = "pfl_SYNTHETIC_NotARealProfile";
fn time() -> Arc<crate::payment_test_fixture::quote::Time> {
    Arc::new(crate::payment_test_fixture::quote::Time(AtomicU64::new(
        1500,
    )))
}
fn response(a: &AttemptSnapshot, id: &str, status: &str) -> Value {
    let mut p = payment(a);
    p["id"] = id.into();
    p["status"] = status.into();
    p
}
async fn client(server: &Server) -> VerifiedReconciliationClient {
    local(server)
        .verify_profile()
        .await
        .expect("profile")
        .into_reconciliation()
}
async fn operation(
    pool: &PgPool,
    clock: &dyn commerce_application::request_context::Clock,
    i: &dyn commerce_application::request_context::Identifiers,
    id: &str,
) -> LookupOperation {
    prepare_lookup(pool, clock, i, PROFILE, id)
        .await
        .expect("server operation committed")
}
async fn counts(pool: &PgPool) -> (i64, i64, i64, i64, i64) {
    sqlx::query_as("SELECT (SELECT count(*) FROM callback_lookup_operations),(SELECT count(*) FROM callback_lookup_receipts),(SELECT count(*) FROM callback_processing_events),(SELECT count(*) FROM checkout_orders),(SELECT count(*) FROM payment_observations)").fetch_one(pool).await.expect("literal custody counts")
}
fn get_only(server: &Server) {
    server.inspect(|rs| {
        assert!(rs.iter().all(|r| r.method == "GET"
            && r.body.is_empty()
            && r.header("Idempotency-Key").is_none()))
    });
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn lookup_receipt_restarts_without_another_get_and_original_processing_replays_once(
    pool: PgPool,
) {
    let (clock, f) = prepared(&pool, true).await;
    let i = ids();
    let op = operation(&pool, clock.as_ref(), &i, "tr_DURABLE").await;
    assert_eq!(op.snapshot().schema_version, 1);
    assert_eq!(op.snapshot().created_at, 1500);
    assert_eq!(op.snapshot().id.len(), 32);
    assert_eq!(op.snapshot().profile_id, PROFILE);
    assert_eq!(op.snapshot().provider_id, "tr_DURABLE");
    assert_eq!(counts(&pool).await, (1, 0, 0, 1, 0));
    assert!(
        read_lookup(&pool, &op.snapshot().id)
            .await
            .expect("not fetched")
            .is_none()
    );
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, response(f.snapshot(), "tr_DURABLE", "paid")),
    ]);
    let c = client(&server).await;
    let first = fetch_lookup(&pool, clock.as_ref(), &i, &op.snapshot().id, &c)
        .await
        .expect("durable authenticated receipt");
    assert!(!first.replayed);
    assert!(first.snapshot.operation == *op.snapshot());
    assert_eq!(first.snapshot.payment.amount_minor, 38245);
    assert_eq!(first.snapshot.recorded_at, 1500);
    assert_eq!(
        first.snapshot.credential_fingerprint,
        "78a0e854453c8e82b9c97187636308791541cdbbcf28965b47a6ee3e33af97ef"
    );
    assert_eq!(counts(&pool).await, (1, 1, 1, 1, 0));
    let replay = fetch_lookup(&pool, &BadPorts, &BadPorts, &op.snapshot().id, &c)
        .await
        .expect("saved receipt before unavailable GET/ports");
    assert!(replay.replayed && replay.snapshot == first.snapshot);
    let restored = read_lookup(&pool, &op.snapshot().id)
        .await
        .expect("restart")
        .expect("committed receipt");
    assert!(restored.snapshot == first.snapshot);
    assert_eq!(server.count(), 2, "profile and exactly one payment GET");
    let observation = process_lookup(&pool, clock.as_ref(), &i, &op.snapshot().id)
        .await
        .expect("full original matching");
    assert_eq!(
        observation.snapshot.effect_key,
        format!("callback-lookup-{}", op.snapshot().id)
    );
    assert_eq!(observation.snapshot.intent.payment.id, "tr_DURABLE");
    let again = process_lookup(&pool, &BadPorts, &BadPorts, &op.snapshot().id)
        .await
        .expect("crash after observation commit");
    assert!(again.replayed && again.snapshot == observation.snapshot);
    assert_eq!(counts(&pool).await, (1, 1, 1, 1, 1));
    assert_eq!(
        crate::payment_test_fixture::counts(&pool).await,
        (1, 4, 1, 1, 2)
    );
    get_only(&server);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn authenticated_unknown_original_stays_visible_without_fictional_order_or_order_event(
    pool: PgPool,
) {
    let (_, f) = captured(38245);
    let clock = time();
    let i = ids();
    let op = operation(&pool, clock.as_ref(), &i, "tr_UNMATCHED").await;
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, response(f.snapshot(), "tr_UNMATCHED", "paid")),
    ]);
    let c = client(&server).await;
    let receipt = fetch_lookup(&pool, clock.as_ref(), &i, &op.snapshot().id, &c)
        .await
        .expect("unknown authenticated fact retained");
    assert_eq!(receipt.snapshot.payment.attempt_id, "1".repeat(32));
    assert_eq!(
        process_lookup(&pool, clock.as_ref(), &i, &op.snapshot().id)
            .await
            .err(),
        Some(PaymentStoreError::NotFound)
    );
    assert_eq!(counts(&pool).await, (1, 1, 1, 0, 0));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM commerce_outbox")
            .fetch_one(&pool)
            .await
            .expect("no order aggregate"),
        0
    );
    assert!(
        read_lookup(&pool, &op.snapshot().id)
            .await
            .expect("visible unresolved receipt")
            .expect("present")
            .snapshot
            == receipt.snapshot
    );
    get_only(&server);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn durable_callback_metadata_never_replaces_original_amount_order_or_dispatch(pool: PgPool) {
    let (clock, f) = prepared(&pool, false).await;
    let i = ids();
    for field in ["amount", "order", "dispatch"] {
        let op = operation(&pool, clock.as_ref(), &i, "tr_WRONG_ORIGINAL").await;
        let mut p = response(f.snapshot(), "tr_WRONG_ORIGINAL", "paid");
        match field {
            "amount" => p["amount"]["value"] = "382.44".into(),
            "order" => p["metadata"]["order_id"] = "f".repeat(32).into(),
            _ => {}
        }
        let server = Server::start(vec![profile(), Reply::Json(200, p)]);
        let c = client(&server).await;
        let receipt = fetch_lookup(&pool, clock.as_ref(), &i, &op.snapshot().id, &c)
            .await
            .expect("authenticated fact retained before association");
        assert_eq!(
            process_lookup(&pool, clock.as_ref(), &i, &op.snapshot().id)
                .await
                .err(),
            Some(PaymentStoreError::Unavailable)
        );
        assert!(
            read_lookup(&pool, &op.snapshot().id)
                .await
                .expect("read")
                .expect("unmatched remains visible")
                .snapshot
                == receipt.snapshot
        );
    }
    assert_eq!(counts(&pool).await, (3, 3, 3, 1, 0));
    let outcome = crate::payment_attempts::start_dispatch(
        &pool,
        clock.as_ref(),
        &i,
        &f.snapshot().binding.id,
        &config(&credential()),
    )
    .await
    .expect("original dispatch");
    assert!(matches!(
        outcome,
        crate::payment_attempts::DispatchOutcome::Start(_)
    ));
    let operations: Vec<String> =
        sqlx::query_scalar("SELECT id FROM callback_lookup_operations ORDER BY id")
            .fetch_all(&pool)
            .await
            .expect("immutable ops");
    for op in operations.iter().take(2) {
        assert_eq!(
            process_lookup(&pool, clock.as_ref(), &i, op).await.err(),
            Some(PaymentStoreError::Conflict)
        );
    }
    process_lookup(&pool, clock.as_ref(), &i, &operations[2])
        .await
        .expect("matching saved fact after original dispatch");
    assert_eq!(counts(&pool).await, (3, 3, 3, 1, 1));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn callback_custody_keeps_late_paid_when_a_later_distinct_lookup_says_open(pool: PgPool) {
    let (clock, f) = prepared(&pool, true).await;
    let i = ids();
    let first = operation(&pool, clock.as_ref(), &i, "tr_LATE_DURABLE").await;
    clock.0.store(5000, Ordering::SeqCst);
    assert_eq!(
        crate::holds::expire_due_holds(&pool, 5000, 25)
            .await
            .expect("actual expiry")
            .expired,
        1
    );
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, response(f.snapshot(), "tr_LATE_DURABLE", "paid")),
        Reply::Json(200, response(f.snapshot(), "tr_LATE_DURABLE", "open")),
    ]);
    let c = client(&server).await;
    fetch_lookup(&pool, clock.as_ref(), &i, &first.snapshot().id, &c)
        .await
        .expect("paid despite hold expiry");
    let second = operation(&pool, clock.as_ref(), &i, "tr_LATE_DURABLE").await;
    fetch_lookup(&pool, clock.as_ref(), &i, &second.snapshot().id, &c)
        .await
        .expect("distinct reordered fact");
    process_lookup(&pool, clock.as_ref(), &i, &second.snapshot().id)
        .await
        .expect("process open first");
    process_lookup(&pool, clock.as_ref(), &i, &first.snapshot().id)
        .await
        .expect("recover saved paid later");
    let resolution = identities::resolve(
        &pool,
        clock.as_ref(),
        &i,
        &f.snapshot().binding.id,
        "resolve-durable-callback",
    )
    .await
    .expect("separate summary")
    .resolution;
    assert_eq!(
        resolution.summary.paid_provider_ids,
        vec!["tr_LATE_DURABLE"]
    );
    assert_eq!(counts(&pool).await, (2, 2, 2, 1, 2));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT sum(reserved)::bigint FROM stock_balances")
            .fetch_one(&pool)
            .await
            .expect("stock unchanged after expiry"),
        0
    );
    get_only(&server);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn exact_concurrent_receipt_retries_commit_one_event_and_operation_cannot_refetch(
    pool: PgPool,
) {
    let (clock, f) = prepared(&pool, true).await;
    let i = Arc::new(ids());
    let op = operation(&pool, clock.as_ref(), i.as_ref(), "tr_SAME_LOOKUP").await;
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, response(f.snapshot(), "tr_SAME_LOOKUP", "paid")),
    ]);
    let c = client(&server).await;
    let permit = start_lookup(&pool, clock.as_ref(), &op)
        .await
        .expect("unique pre-I/O custody");
    let fetched = Arc::new(c.fetch_lookup(permit).await.expect("authenticated"));
    assert_eq!(
        record_lookup(
            &pool,
            clock.as_ref(),
            i.as_ref(),
            "invalid",
            fetched.as_ref()
        )
        .await
        .err(),
        Some(PaymentStoreError::InvalidRequest)
    );
    let mut tasks = Vec::new();
    for _ in 0..4 {
        let (p, t, i, v, id) = (
            pool.clone(),
            clock.clone(),
            i.clone(),
            fetched.clone(),
            op.snapshot().id.clone(),
        );
        tasks.push(tokio::spawn(async move {
            record_lookup(&p, t.as_ref(), i.as_ref(), &id, v.as_ref()).await
        }));
    }
    let mut receipts = Vec::new();
    for task in tasks {
        receipts.push(task.await.expect("task").expect("serialized exact retry"));
    }
    assert_eq!(receipts.iter().filter(|r| !r.replayed).count(), 1);
    assert!(receipts.iter().all(|r| r.snapshot == receipts[0].snapshot));
    assert!(
        start_lookup(&pool, clock.as_ref(), &op).await.is_err(),
        "single-use start cannot repeat GET"
    );
    assert_eq!(counts(&pool).await, (1, 1, 1, 1, 0));
}

#[path = "mollie_callback_custody_guards.rs"]
mod guards;

#[path = "mollie_callback_custody_native.rs"]
mod native;

#[path = "callback_inbox.rs"]
mod inbox;
