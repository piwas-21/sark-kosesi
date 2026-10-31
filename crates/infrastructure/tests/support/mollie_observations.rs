//! Disposable PostgreSQL + loopback authenticated fetches. No real funds/merchant facts.
use super::*;
use crate::payment_attempts::observations::{read_observation, read_recorded, record_verified};
use commerce_application::request_context::{Clock, ContextError, Identifiers};
use commerce_domain::payment_attempts::observations::ObservationSnapshot;
use std::sync::Arc;
async fn prepared(
    pool: &PgPool,
    start: bool,
) -> (Arc<crate::payment_test_fixture::quote::Time>, FrozenAttempt) {
    let (store, time, session, csrf, order) = crate::payment_test_fixture::accepted(pool).await;
    let owner = GuestCredential::from_session(&session).expect("guest");
    let c = config(&credential());
    let f = prepare_attempt(
        &store,
        &owner,
        &csrf,
        "payment",
        &crate::payment_test_fixture::intent(&order),
        &c,
    )
    .await
    .expect("prepared")
    .frozen;
    if start {
        let ids = crate::payment_test_fixture::Ids(AtomicU64::new(10000));
        let outcome = start_dispatch(pool, time.as_ref(), &ids, &f.snapshot().binding.id, &c)
            .await
            .expect("durable start");
        assert!(
            matches!(outcome, DispatchOutcome::Start(_)),
            "a lost permit still leaves started custody"
        );
    }
    (time, f)
}
async fn fetched(
    attempt: &AttemptSnapshot,
    id: &str,
    status: &str,
    rotated: bool,
) -> VerifiedPayment {
    let mut p = payment(attempt);
    p["id"] = id.into();
    p["status"] = status.into();
    let server = Server::start(vec![profile(), Reply::Json(200, p)]);
    let client = if rotated {
        let key = TestCredential::capture(
            &format!("test_{}", "rotated".repeat(5)),
            "pfl_SYNTHETIC_NotARealProfile",
        )
        .expect("rotated key");
        let c = config(&key);
        TestClient::local(key, c, server.base.clone(), Duration::from_secs(2)).expect("local")
    } else {
        local(&server)
    };
    let verified = client
        .verify_profile()
        .await
        .expect("profile")
        .fetch(attempt, id)
        .await
        .expect("authenticated matching fetch");
    assert_eq!(
        server.count(),
        2,
        "profile GET plus separate payment GET only"
    );
    verified
}
fn ids() -> crate::payment_test_fixture::Ids {
    crate::payment_test_fixture::Ids(AtomicU64::new(20000))
}
struct BadPorts;
impl Clock for BadPorts {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        Err(ContextError::ClockUnavailable)
    }
}
impl Identifiers for BadPorts {
    fn next(&self) -> Result<[u8; 16], ContextError> {
        Err(ContextError::EntropyUnavailable)
    }
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn fetched_observation_and_processing_event_commit_once_and_exact_receipts_remain_historical(
    pool: PgPool,
) {
    let (time, f) = prepared(&pool, true).await;
    let a = &f.snapshot().binding.id;
    let payment = fetched(f.snapshot(), "tr_OBSERVED", "paid", false).await;
    let ids = ids();
    let first = record_verified(&pool, time.as_ref(), &ids, a, "fetch-a", &payment)
        .await
        .expect("record fetched fact");
    assert!(!first.replayed);
    assert_eq!(first.snapshot.revision, 1);
    assert_eq!(first.snapshot.recorded_at, 1500);
    assert_eq!(first.snapshot.intent.payment.id, "tr_OBSERVED");
    assert_eq!(first.snapshot.intent.payment.amount_minor, 38245);
    assert_eq!(
        first.snapshot.intent.credential_fingerprint,
        "78a0e854453c8e82b9c97187636308791541cdbbcf28965b47a6ee3e33af97ef"
    );
    assert_eq!(
        crate::payment_test_fixture::counts(&pool).await,
        (1, 4, 1, 1, 2)
    );
    let replay = record_verified(&pool, &BadPorts, &BadPorts, a, "fetch-a", &payment)
        .await
        .expect("original receipt needs no fresh clock or entropy");
    assert!(replay.replayed);
    assert!(replay.snapshot == first.snapshot);
    assert!(
        read_recorded(&pool, a, "fetch-a")
            .await
            .expect("receipt")
            .expect("exists")
            == first.snapshot
    );
    assert!(
        read_recorded(&pool, a, "unseen")
            .await
            .expect("missing")
            .is_none()
    );
    assert!(
        read_observation(&pool, a, &first.snapshot.id)
            .await
            .expect("outbox read")
            == first.snapshot
    );
    assert_eq!(
        read_observation(&pool, a, &"8".repeat(32)).await.err(),
        Some(PaymentStoreError::NotFound)
    );
    let events: Vec<Value> = sqlx::query_scalar(
        "SELECT payload FROM commerce_outbox WHERE kind='payment.observation_recorded.v1'",
    )
    .fetch_all(&pool)
    .await
    .expect("event");
    assert_eq!(
        events,
        vec![
            json!({"schema_version":1,"order_id":f.snapshot().binding.order_id,"attempt_id":a,"observation_id":first.snapshot.id})
        ]
    );
    assert_eq!(
        crate::payment_test_fixture::counts(&pool).await,
        (1, 4, 1, 1, 2)
    );
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn reordered_statuses_and_distinct_provider_ids_remain_facts_without_replacing_or_applying_state(
    pool: PgPool,
) {
    let (time, f) = prepared(&pool, true).await;
    let a = &f.snapshot().binding.id;
    let ids = ids();
    let mut records = Vec::<ObservationSnapshot>::new();
    for (effect, id, status) in [
        ("paid-a", "tr_FIRST", "paid"),
        ("older-pending", "tr_FIRST", "pending"),
        ("second-identity", "tr_SECOND", "paid"),
    ] {
        let verified = fetched(f.snapshot(), id, status, false).await;
        records.push(
            record_verified(&pool, time.as_ref(), &ids, a, effect, &verified)
                .await
                .expect("retain every fetched fact")
                .snapshot,
        );
    }
    assert_eq!(
        records.iter().map(|s| s.revision).collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
    let statuses: Vec<String> =
        sqlx::query_scalar("SELECT status FROM payment_observations ORDER BY revision")
            .fetch_all(&pool)
            .await
            .expect("statuses");
    assert_eq!(statuses, vec!["paid", "pending", "paid"]);
    assert!(matches!(
        read_observation(&pool, a, &records[0].id)
            .await
            .expect("original paid")
            .intent
            .payment
            .status,
        PaymentStatus::Paid
    ));
    assert_eq!(
        crate::payment_test_fixture::counts(&pool).await,
        (1, 6, 1, 1, 2)
    );
    assert!(matches!(
        start_dispatch(&pool, time.as_ref(), &ids, a, &config(&credential())).await,
        Ok(DispatchOutcome::Reconcile { .. })
    ));
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn changed_fetched_intent_conflicts_on_the_same_effect_and_rotation_is_recorded_under_a_new_one(
    pool: PgPool,
) {
    let (time, f) = prepared(&pool, true).await;
    let a = &f.snapshot().binding.id;
    let ids = ids();
    let paid = fetched(f.snapshot(), "tr_FIRST", "paid", false).await;
    record_verified(&pool, time.as_ref(), &ids, a, "fetch-a", &paid)
        .await
        .expect("first");
    let pending = fetched(f.snapshot(), "tr_FIRST", "pending", false).await;
    let rotated = fetched(f.snapshot(), "tr_FIRST", "paid", true).await;
    for changed in [&pending, &rotated] {
        assert_eq!(
            record_verified(&pool, time.as_ref(), &ids, a, "fetch-a", changed)
                .await
                .err(),
            Some(PaymentStoreError::Conflict)
        );
    }
    let next = record_verified(&pool, time.as_ref(), &ids, a, "rotated-fetch", &rotated)
        .await
        .expect("new actual operation");
    assert_eq!(next.snapshot.revision, 2);
    assert_ne!(
        next.snapshot.intent.credential_fingerprint,
        f.snapshot().binding.profile.credential_fingerprint
    );
    assert_eq!(
        next.snapshot.intent.credential_fingerprint,
        rotated.credential_fingerprint()
    );
    assert_eq!(
        crate::payment_test_fixture::counts(&pool).await,
        (1, 5, 1, 1, 2)
    );
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn late_paid_after_actual_component_expiry_is_preserved_without_consumption_or_reallocation(
    pool: PgPool,
) {
    let (time, f) = prepared(&pool, true).await;
    time.0.store(5000, Ordering::SeqCst);
    let expired = crate::holds::expire_due_holds(&pool, 5000, 100)
        .await
        .expect("real expiry");
    assert_eq!(expired.expired, 1);
    assert_eq!(expired.failed, 0);
    assert_eq!(
        crate::payment_test_fixture::counts(&pool).await,
        (1, 3, 1, 1, 4)
    );
    let paid = fetched(f.snapshot(), "tr_LATE_PAID", "paid", false).await;
    let result = record_verified(
        &pool,
        time.as_ref(),
        &ids(),
        &f.snapshot().binding.id,
        "late-paid",
        &paid,
    )
    .await
    .expect("funds evidence survives expiry");
    assert_eq!(result.snapshot.recorded_at, 5000);
    assert!(matches!(
        result.snapshot.intent.payment.status,
        PaymentStatus::Paid
    ));
    let reserved: i64 = sqlx::query_scalar("SELECT sum(reserved)::bigint FROM stock_balances")
        .fetch_one(&pool)
        .await
        .expect("physical balance");
    assert_eq!(reserved, 0);
    assert_eq!(
        crate::payment_test_fixture::counts(&pool).await,
        (1, 4, 1, 1, 4)
    );
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn a_fetch_capability_for_another_snapshot_cannot_supply_order_or_amount_authority(
    pool: PgPool,
) {
    let (time, f) = prepared(&pool, true).await;
    let (_, other) = captured(38245);
    let wrong = fetched(other.snapshot(), "tr_OTHER_ORDER", "paid", false).await;
    assert_eq!(
        record_verified(
            &pool,
            time.as_ref(),
            &ids(),
            &f.snapshot().binding.id,
            "foreign",
            &wrong
        )
        .await
        .err(),
        Some(PaymentStoreError::Conflict)
    );
    assert_eq!(
        crate::payment_test_fixture::counts(&pool).await,
        (1, 3, 1, 1, 2)
    );
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn observation_requires_a_committed_dispatch_and_bounded_trusted_inputs(pool: PgPool) {
    let (time, f) = prepared(&pool, false).await;
    let paid = fetched(f.snapshot(), "tr_NOT_DISPATCHED", "paid", false).await;
    let a = &f.snapshot().binding.id;
    assert_eq!(
        record_verified(&pool, time.as_ref(), &ids(), a, "fetch", &paid)
            .await
            .err(),
        Some(PaymentStoreError::Unavailable)
    );
    assert_eq!(
        read_recorded(&pool, a, "fetch").await.err(),
        Some(PaymentStoreError::Unavailable)
    );
    assert_eq!(
        record_verified(
            &pool,
            time.as_ref(),
            &ids(),
            &"8".repeat(32),
            "fetch",
            &paid
        )
        .await
        .err(),
        Some(PaymentStoreError::NotFound)
    );
    for bad in ["", "bad", &"g".repeat(32)] {
        assert_eq!(
            record_verified(&pool, time.as_ref(), &ids(), bad, "fetch", &paid)
                .await
                .err(),
            Some(PaymentStoreError::InvalidRequest)
        );
    }
    for bad in ["", "bad effect", &"a".repeat(129)] {
        assert_eq!(
            record_verified(&pool, time.as_ref(), &ids(), a, bad, &paid)
                .await
                .err(),
            Some(PaymentStoreError::InvalidRequest)
        );
    }
    assert_eq!(
        read_recorded(&pool, "bad", "fetch").await.err(),
        Some(PaymentStoreError::InvalidRequest)
    );
    assert_eq!(
        read_observation(&pool, a, "bad").await.err(),
        Some(PaymentStoreError::InvalidRequest)
    );
    assert_eq!(
        read_observation(&pool, "bad", &"8".repeat(32)).await.err(),
        Some(PaymentStoreError::InvalidRequest)
    );
    assert_eq!(
        crate::payment_test_fixture::counts(&pool).await,
        (1, 2, 1, 1, 2)
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn competing_exact_observation_retries_commit_one_revision_and_one_event(pool: PgPool) {
    let (time, f) = prepared(&pool, true).await;
    let verified = Arc::new(fetched(f.snapshot(), "tr_CONTENDED", "paid", false).await);
    let ids = Arc::new(ids());
    let mut tasks = Vec::new();
    for _ in 0..8 {
        let p = pool.clone();
        let t = time.clone();
        let v = verified.clone();
        let i = ids.clone();
        let a = f.snapshot().binding.id.clone();
        tasks.push(tokio::spawn(async move {
            record_verified(&p, t.as_ref(), i.as_ref(), &a, "same-fetch", v.as_ref()).await
        }));
    }
    let mut receipts = Vec::new();
    for task in tasks {
        receipts.push(task.await.expect("task").expect("serialized receipt"));
    }
    assert_eq!(receipts.iter().filter(|r| !r.replayed).count(), 1);
    assert!(receipts.iter().all(|r| r.snapshot == receipts[0].snapshot));
    assert_eq!(receipts[0].snapshot.revision, 1);
    assert_eq!(
        crate::payment_test_fixture::counts(&pool).await,
        (1, 4, 1, 1, 2)
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM payment_observations")
        .fetch_one(&pool)
        .await
        .expect("facts");
    assert_eq!(count, 1);
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn concurrent_distinct_fetch_operations_keep_every_fact_with_contiguous_revisions(
    pool: PgPool,
) {
    let (time, f) = prepared(&pool, true).await;
    let verified = Arc::new(fetched(f.snapshot(), "tr_CONTENDED", "paid", false).await);
    let ids = Arc::new(ids());
    let mut tasks = Vec::new();
    for index in 0..8 {
        let p = pool.clone();
        let t = time.clone();
        let v = verified.clone();
        let i = ids.clone();
        let a = f.snapshot().binding.id.clone();
        tasks.push(tokio::spawn(async move {
            record_verified(
                &p,
                t.as_ref(),
                i.as_ref(),
                &a,
                &format!("fetch-{index}"),
                v.as_ref(),
            )
            .await
        }));
    }
    let mut revisions = Vec::new();
    for task in tasks {
        let receipt = task.await.expect("task").expect("serialized append");
        assert!(!receipt.replayed);
        revisions.push(receipt.snapshot.revision);
    }
    revisions.sort_unstable();
    assert_eq!(revisions, vec![1, 2, 3, 4, 5, 6, 7, 8]);
    assert_eq!(
        crate::payment_test_fixture::counts(&pool).await,
        (1, 11, 1, 1, 2)
    );
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn clock_entropy_and_prior_record_time_failures_never_leave_a_partial_fact(pool: PgPool) {
    let (time, f) = prepared(&pool, true).await;
    let a = &f.snapshot().binding.id;
    let paid = fetched(f.snapshot(), "tr_PORTS", "paid", false).await;
    let ids = ids();
    assert_eq!(
        record_verified(&pool, &BadPorts, &ids, a, "clock", &paid)
            .await
            .err(),
        Some(PaymentStoreError::ClockUnavailable)
    );
    assert!(matches!(
        record_verified(&pool, time.as_ref(), &BadPorts, a, "entropy", &paid).await,
        Err(PaymentStoreError::Boundary(
            crate::checkout_orders::OrderStoreError::EntropyUnavailable
        ))
    ));
    for bad in [1499, 9_007_199_254_740_992, u64::MAX] {
        time.0.store(bad, Ordering::SeqCst);
        assert_eq!(
            record_verified(&pool, time.as_ref(), &ids, a, "bad-time", &paid)
                .await
                .err(),
            Some(PaymentStoreError::ClockUnavailable)
        );
    }
    assert_eq!(
        crate::payment_test_fixture::counts(&pool).await,
        (1, 3, 1, 1, 2)
    );
    time.0.store(1600, Ordering::SeqCst);
    let first = record_verified(&pool, time.as_ref(), &ids, a, "good-time", &paid)
        .await
        .expect("first actual fact");
    assert_eq!(first.snapshot.revision, 1);
    time.0.store(1500, Ordering::SeqCst);
    assert_eq!(
        record_verified(&pool, time.as_ref(), &ids, a, "rewound-after-prior", &paid)
            .await
            .err(),
        Some(PaymentStoreError::ClockUnavailable)
    );
    assert_eq!(
        crate::payment_test_fixture::counts(&pool).await,
        (1, 4, 1, 1, 2)
    );
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn rewind_after_the_actual_observation_event_wait_rolls_back_both_fact_and_event(
    pool: PgPool,
) {
    let (time, f) = prepared(&pool, true).await;
    let paid = Arc::new(fetched(f.snapshot(), "tr_WAITED", "paid", false).await);
    let ids = Arc::new(ids());
    let mut blocker = pool.begin().await.expect("blocker");
    blocker
        .execute("LOCK TABLE commerce_outbox IN SHARE MODE")
        .await
        .expect("actual event wait");
    let p = pool.clone();
    let t = time.clone();
    let v = paid.clone();
    let i = ids.clone();
    let a = f.snapshot().binding.id.clone();
    let task = tokio::spawn(async move {
        record_verified(&p, t.as_ref(), i.as_ref(), &a, "waited-fetch", v.as_ref()).await
    });
    crate::payment_test_fixture::waiting(&pool).await;
    time.0.store(1499, Ordering::SeqCst);
    blocker.commit().await.expect("release wait");
    assert_eq!(
        task.await.expect("task").err(),
        Some(PaymentStoreError::ClockUnavailable)
    );
    assert_eq!(
        crate::payment_test_fixture::counts(&pool).await,
        (1, 3, 1, 1, 2)
    );
    assert!(
        read_recorded(&pool, &f.snapshot().binding.id, "waited-fetch")
            .await
            .expect("rolled back")
            .is_none()
    );
    time.0.store(1500, Ordering::SeqCst);
    let receipt = record_verified(
        &pool,
        time.as_ref(),
        ids.as_ref(),
        &f.snapshot().binding.id,
        "waited-fetch",
        paid.as_ref(),
    )
    .await
    .expect("recover original observation operation");
    assert_eq!(receipt.snapshot.revision, 1);
    assert_eq!(
        crate::payment_test_fixture::counts(&pool).await,
        (1, 4, 1, 1, 2)
    );
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn cancellation_during_event_storage_leaves_dispatch_reconcilable_and_no_partial_observation(
    pool: PgPool,
) {
    let (time, f) = prepared(&pool, true).await;
    let paid = Arc::new(fetched(f.snapshot(), "tr_CANCELLED", "paid", false).await);
    let ids = Arc::new(ids());
    let mut blocker = pool.begin().await.expect("blocker");
    blocker
        .execute("LOCK TABLE commerce_outbox IN SHARE MODE")
        .await
        .expect("actual event wait");
    let p = pool.clone();
    let t = time.clone();
    let v = paid.clone();
    let i = ids.clone();
    let a = f.snapshot().binding.id.clone();
    let task = tokio::spawn(async move {
        record_verified(
            &p,
            t.as_ref(),
            i.as_ref(),
            &a,
            "cancelled-fetch",
            v.as_ref(),
        )
        .await
    });
    crate::payment_test_fixture::waiting(&pool).await;
    task.abort();
    assert!(task.await.err().expect("cancelled").is_cancelled());
    blocker.rollback().await.expect("release");
    assert!(
        read_recorded(&pool, &f.snapshot().binding.id, "cancelled-fetch")
            .await
            .expect("no partial fact")
            .is_none()
    );
    assert_eq!(
        crate::payment_test_fixture::counts(&pool).await,
        (1, 3, 1, 1, 2)
    );
    assert!(matches!(
        start_dispatch(
            &pool,
            time.as_ref(),
            ids.as_ref(),
            &f.snapshot().binding.id,
            &config(&credential())
        )
        .await,
        Ok(DispatchOutcome::Reconcile { .. })
    ));
    assert_eq!(
        record_verified(
            &pool,
            time.as_ref(),
            ids.as_ref(),
            &f.snapshot().binding.id,
            "cancelled-fetch",
            paid.as_ref()
        )
        .await
        .expect("retry fact, never another create")
        .snapshot
        .revision,
        1
    );
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn database_faults_are_static_and_discard_query_or_provider_detail(pool: PgPool) {
    let (time, f) = prepared(&pool, true).await;
    let paid = fetched(f.snapshot(), "tr_PRIVATE_DETAIL", "paid", false).await;
    pool.close().await;
    for result in [
        record_verified(
            &pool,
            time.as_ref(),
            &ids(),
            &f.snapshot().binding.id,
            "closed",
            &paid,
        )
        .await
        .err(),
        read_recorded(&pool, &f.snapshot().binding.id, "closed")
            .await
            .err(),
        read_observation(&pool, &f.snapshot().binding.id, &"8".repeat(32))
            .await
            .err(),
    ] {
        assert_eq!(result, Some(PaymentStoreError::Database));
        assert_eq!(format!("{result:?}"), "Some(Database)");
    }
}

async fn native_insert(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    s: &ObservationSnapshot,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO payment_observations(id,attempt_id,effect_key,revision,recorded_at,provider_id,credential_fingerprint,status,snapshot) VALUES($1,$2,$3,$4,$5,$6,$7,'paid',$8)")
        .bind(&s.id).bind(&s.attempt_id).bind(&s.effect_key).bind(s.revision).bind(s.recorded_at).bind(&s.intent.payment.id).bind(&s.intent.credential_fingerprint).bind(serde_json::to_value(s).expect("synthetic native control")).execute(&mut **tx).await?;
    Ok(())
}
fn event(s: &ObservationSnapshot) -> Value {
    json!({"schema_version":1,"order_id":s.intent.payment.order_id,"attempt_id":s.attempt_id,"observation_id":s.id})
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn schema_requires_exact_minimal_facts_contiguous_time_and_deferred_processing_event(
    pool: PgPool,
) {
    let (time, f) = prepared(&pool, true).await;
    let paid = fetched(f.snapshot(), "tr_NATIVE", "paid", false).await;
    let first = record_verified(
        &pool,
        time.as_ref(),
        &ids(),
        &f.snapshot().binding.id,
        "first",
        &paid,
    )
    .await
    .expect("source positive")
    .snapshot;
    let mut next = first.clone();
    next.id = "8".repeat(32);
    next.effect_key = "native-second".into();
    next.revision = 2;
    next.recorded_at = 1600;
    let mut tx = pool.begin().await.expect("native omitted event");
    native_insert(&mut tx, &next)
        .await
        .expect("valid fact before deferred boundary");
    assert!(
        tx.commit().await.is_err(),
        "missing durable event must fail commit"
    );
    assert!(
        read_recorded(&pool, &f.snapshot().binding.id, "native-second")
            .await
            .expect("no orphan")
            .is_none()
    );
    let mut tx = pool.begin().await.expect("positive native transaction");
    native_insert(&mut tx, &next).await.expect("valid schema");
    sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) VALUES($1,$2,'payment.observation_recorded.v1',$3,$4)")
        .bind("7".repeat(32)).bind(&f.snapshot().binding.order_id).bind(event(&next)).bind(1600_i64).execute(&mut *tx).await.expect("exact event");
    tx.commit().await.expect("paired positive control");
    assert!(
        read_observation(&pool, &f.snapshot().binding.id, &next.id)
            .await
            .expect("revalidated native control")
            == next
    );
    for (revision, at, amount) in [(4, 1700, 38245), (3, 1599, 38245), (3, 1700, 38244)] {
        let mut bad = next.clone();
        bad.id = "6".repeat(32);
        bad.effect_key = "invalid-third".into();
        bad.revision = revision;
        bad.recorded_at = at;
        bad.intent.payment.amount_minor = amount;
        let mut tx = pool.begin().await.expect("negative native control");
        assert!(native_insert(&mut tx, &bad).await.is_err());
        tx.rollback().await.expect("rollback");
    }
    assert_eq!(
        crate::payment_test_fixture::counts(&pool).await,
        (1, 5, 1, 1, 2)
    );
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn new_event_kind_preserves_original_uniqueness_and_rejects_altered_or_unbound_events(
    pool: PgPool,
) {
    let (time, f) = prepared(&pool, true).await;
    let paid = fetched(f.snapshot(), "tr_EVENTS", "paid", false).await;
    let first = record_verified(
        &pool,
        time.as_ref(),
        &ids(),
        &f.snapshot().binding.id,
        "first",
        &paid,
    )
    .await
    .expect("source positive")
    .snapshot;
    for query in [
        "UPDATE payment_observations SET status=status",
        "DELETE FROM payment_observations",
        "UPDATE commerce_outbox SET kind=kind",
    ] {
        let result = match query {
            "UPDATE payment_observations SET status=status" => {
                pool.execute("UPDATE payment_observations SET status=status")
                    .await
            }
            "DELETE FROM payment_observations" => {
                pool.execute("DELETE FROM payment_observations").await
            }
            _ => pool.execute("UPDATE commerce_outbox SET kind=kind").await,
        };
        let error = result.expect_err("immutable");
        assert_eq!(
            error.as_database_error().and_then(|e| e.code()).as_deref(),
            Some("23514")
        );
    }
    let error=sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) SELECT $1,aggregate_id,kind,payload,created_at FROM commerce_outbox WHERE kind='checkout.order_created.v1'")
        .bind("f".repeat(32)).execute(&pool).await.expect_err("original uniqueness retained");
    assert_eq!(
        error.as_database_error().and_then(|e| e.code()).as_deref(),
        Some("23505")
    );
    for (field, value) in [
        ("schema_version", json!(2)),
        ("order_id", json!("4".repeat(32))),
        ("attempt_id", json!("4".repeat(32))),
        ("observation_id", json!("4".repeat(32))),
    ] {
        let mut next = first.clone();
        next.id = "6".repeat(32);
        next.effect_key = "second".into();
        next.revision = 2;
        next.recorded_at = 1600;
        let mut payload = event(&next);
        payload[field] = value;
        let mut tx = pool.begin().await.expect("new fact");
        native_insert(&mut tx, &next)
            .await
            .expect("valid pending fact");
        assert!(sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) VALUES($1,$2,'payment.observation_recorded.v1',$3,1600)")
            .bind("5".repeat(32)).bind(&f.snapshot().binding.order_id).bind(payload).execute(&mut *tx).await.is_err(),"{field}");
        tx.rollback().await.expect("rollback");
    }
    assert_eq!(
        crate::payment_test_fixture::counts(&pool).await,
        (1, 4, 1, 1, 2)
    );
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn damaged_snapshot_and_oversized_imports_fail_reads_and_retries_without_fallback(
    pool: PgPool,
) {
    let (time, f) = prepared(&pool, true).await;
    let paid = fetched(f.snapshot(), "tr_DAMAGE", "paid", false).await;
    let ids = ids();
    let a = &f.snapshot().binding.id;
    let first = record_verified(&pool, time.as_ref(), &ids, a, "first", &paid)
        .await
        .expect("healthy source")
        .snapshot;
    let original = serde_json::to_value(&first).expect("original closed projection");
    for oversized in [false, true] {
        let mut tx = pool.begin().await.expect("isolated import damage");
        tx.execute("ALTER TABLE payment_observations DISABLE TRIGGER USER")
            .await
            .expect("test-only damaged import");
        if oversized {
            tx.execute("ALTER TABLE payment_observations DROP CONSTRAINT payment_observations_snapshot_check").await.expect("test-only bypass size guard");
            tx.execute("UPDATE payment_observations SET snapshot=snapshot||jsonb_build_object('padding',repeat('x',8193))").await.expect("oversized import");
        } else {
            tx.execute("UPDATE payment_observations SET snapshot=jsonb_set(snapshot,'{intent,payment,amount_minor}','38244')").await.expect("coherent-looking false amount");
        }
        tx.execute("ALTER TABLE payment_observations ENABLE TRIGGER USER")
            .await
            .expect("restore immutability");
        tx.commit().await.expect("damaged fixture");
        assert_eq!(
            read_recorded(&pool, a, "first").await.err(),
            Some(PaymentStoreError::InvalidStoredData)
        );
        assert_eq!(
            read_observation(&pool, a, &first.id).await.err(),
            Some(PaymentStoreError::InvalidStoredData)
        );
        for effect in ["first", "new-after-damage"] {
            assert_eq!(
                record_verified(&pool, time.as_ref(), &ids, a, effect, &paid)
                    .await
                    .err(),
                Some(PaymentStoreError::InvalidStoredData)
            );
        }
        let mut tx = pool
            .begin()
            .await
            .expect("repair only this isolated fixture");
        tx.execute("ALTER TABLE payment_observations DISABLE TRIGGER USER")
            .await
            .expect("test repair");
        sqlx::query("UPDATE payment_observations SET snapshot=$1 WHERE id=$2")
            .bind(&original)
            .bind(&first.id)
            .execute(&mut *tx)
            .await
            .expect("exact original restore");
        if oversized {
            tx.execute("ALTER TABLE payment_observations ADD CONSTRAINT payment_observations_snapshot_check CHECK(jsonb_typeof(snapshot)='object' AND octet_length(snapshot::text)<=8192)").await.expect("restore exact check");
        }
        tx.execute("ALTER TABLE payment_observations ENABLE TRIGGER USER")
            .await
            .expect("restore guard");
        tx.commit().await.expect("repaired fixture");
        assert!(
            read_observation(&pool, a, &first.id)
                .await
                .expect("positive restored reader")
                == first
        );
    }
    assert_eq!(
        crate::payment_test_fixture::counts(&pool).await,
        (1, 4, 1, 1, 2)
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn a_damaged_missing_processing_event_cannot_be_treated_as_a_complete_receipt(pool: PgPool) {
    let (time, f) = prepared(&pool, true).await;
    let paid = fetched(f.snapshot(), "tr_MISSING_EVENT", "paid", false).await;
    let ids = ids();
    let a = &f.snapshot().binding.id;
    let first = record_verified(&pool, time.as_ref(), &ids, a, "first", &paid)
        .await
        .expect("complete source receipt")
        .snapshot;
    let (event_id,aggregate_id,payload,created_at):(String,String,Value,i64)=sqlx::query_as("SELECT id,aggregate_id,payload,created_at FROM commerce_outbox WHERE kind='payment.observation_recorded.v1'").fetch_one(&pool).await.expect("original event");
    let mut tx = pool.begin().await.expect("isolated damaged import");
    tx.execute("ALTER TABLE commerce_outbox DISABLE TRIGGER USER")
        .await
        .expect("test-only damage");
    tx.execute("DELETE FROM commerce_outbox WHERE kind='payment.observation_recorded.v1'")
        .await
        .expect("remove isolated fixture event");
    tx.execute("ALTER TABLE commerce_outbox ENABLE TRIGGER USER")
        .await
        .expect("restore guard");
    tx.commit().await.expect("fixture damage");
    assert_eq!(
        read_observation(&pool, a, &first.id).await.err(),
        Some(PaymentStoreError::InvalidStoredData)
    );
    assert_eq!(
        read_recorded(&pool, a, "first").await.err(),
        Some(PaymentStoreError::InvalidStoredData)
    );
    for effect in ["first", "after-orphan"] {
        assert_eq!(
            record_verified(&pool, time.as_ref(), &ids, a, effect, &paid)
                .await
                .err(),
            Some(PaymentStoreError::InvalidStoredData)
        );
    }
    sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) VALUES($1,$2,'payment.observation_recorded.v1',$3,$4)").bind(event_id).bind(aggregate_id).bind(payload).bind(created_at).execute(&pool).await.expect("restore exact isolated original event");
    assert!(
        read_observation(&pool, a, &first.id)
            .await
            .expect("positive restored receipt")
            == first
    );
    assert_eq!(
        crate::payment_test_fixture::counts(&pool).await,
        (1, 4, 1, 1, 2)
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn connection_default_repeatable_read_cannot_hide_facts_committed_during_an_append_claim(
    pool: PgPool,
) {
    let (time, f) = prepared(&pool, true).await;
    let verified = Arc::new(fetched(f.snapshot(), "tr_ISOLATION", "paid", false).await);
    let ids = Arc::new(ids());
    let rr = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .min_connections(2)
        .after_connect(|connection, _| {
            Box::pin(async move {
                sqlx::query("SET default_transaction_isolation='repeatable read'")
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect_with((*pool.connect_options()).clone())
        .await
        .expect("owned nondefault pool");
    let configured: String = sqlx::query_scalar("SHOW default_transaction_isolation")
        .fetch_one(&rr)
        .await
        .expect("actual nondefault session policy");
    assert_eq!(configured, "repeatable read");
    let mut blocker = pool.begin().await.expect("parent claim blocker");
    sqlx::query("SELECT id FROM payment_attempts WHERE id=$1 FOR UPDATE")
        .bind(&f.snapshot().binding.id)
        .fetch_one(&mut *blocker)
        .await
        .expect("owned attempt claim");
    let mut tasks = Vec::new();
    for effect in ["rr-first", "rr-second"] {
        let p = rr.clone();
        let t = time.clone();
        let v = verified.clone();
        let i = ids.clone();
        let a = f.snapshot().binding.id.clone();
        tasks.push(tokio::spawn(async move {
            record_verified(&p, t.as_ref(), i.as_ref(), &a, effect, v.as_ref()).await
        }));
    }
    tokio::time::timeout(Duration::from_millis(800),async {
        loop {
            let count:i64=sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'SELECT left(basket_id,33) FROM payment_attempts WHERE id=%FOR UPDATE'").fetch_one(&pool).await.expect("both real claims waiting");
            if count==2 {break;}tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }).await.expect("both claim snapshots overlap");
    blocker.commit().await.expect("release both contenders");
    let mut revisions = Vec::new();
    for task in tasks {
        let result = task.await.expect("task");
        assert!(
            result.is_ok(),
            "every distinct fetch fact must survive the claim wait"
        );
        let receipt = result.expect("asserted healthy append");
        revisions.push(receipt.snapshot.revision);
    }
    rr.close().await;
    revisions.sort_unstable();
    assert_eq!(revisions, vec![1, 2]);
    assert_eq!(
        crate::payment_test_fixture::counts(&pool).await,
        (1, 5, 1, 1, 2)
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn damaged_original_dispatch_identity_or_time_cannot_validate_a_historical_observation(
    pool: PgPool,
) {
    let (time, f) = prepared(&pool, true).await;
    let paid = fetched(f.snapshot(), "tr_DISPATCH_PROOF", "paid", false).await;
    let ids = ids();
    let a = &f.snapshot().binding.id;
    let first = record_verified(&pool, time.as_ref(), &ids, a, "first", &paid)
        .await
        .expect("healthy source receipt")
        .snapshot;
    let (id, at): (String, i64) =
        sqlx::query_as("SELECT id,started_at FROM payment_dispatch_starts WHERE attempt_id=$1")
            .bind(a)
            .fetch_one(&pool)
            .await
            .expect("original dispatch");
    for bad_id in [true, false] {
        let mut tx = pool
            .begin()
            .await
            .expect("isolated imported dispatch damage");
        tx.execute("ALTER TABLE payment_dispatch_starts DISABLE TRIGGER USER")
            .await
            .expect("test-only damage");
        if bad_id {
            tx.execute("ALTER TABLE payment_dispatch_starts DROP CONSTRAINT payment_dispatch_starts_id_check").await.expect("test-only id damage");
            tx.execute("UPDATE payment_dispatch_starts SET id='invalid-start'")
                .await
                .expect("invalid imported identity");
        } else {
            tx.execute("UPDATE payment_dispatch_starts SET started_at=5000")
                .await
                .expect("invalid imported original window");
        }
        tx.execute("ALTER TABLE payment_dispatch_starts ENABLE TRIGGER USER")
            .await
            .expect("restore guard");
        tx.commit().await.expect("damaged isolated import");
        assert_eq!(
            read_recorded(&pool, a, "first").await.err(),
            Some(PaymentStoreError::InvalidStoredData)
        );
        assert_eq!(
            read_observation(&pool, a, &first.id).await.err(),
            Some(PaymentStoreError::InvalidStoredData)
        );
        assert_eq!(
            record_verified(&pool, time.as_ref(), &ids, a, "first", &paid)
                .await
                .err(),
            Some(PaymentStoreError::InvalidStoredData)
        );
        let mut tx = pool.begin().await.expect("restore exact isolated original");
        tx.execute("ALTER TABLE payment_dispatch_starts DISABLE TRIGGER USER")
            .await
            .expect("test-only restore");
        sqlx::query("UPDATE payment_dispatch_starts SET id=$1,started_at=$2 WHERE attempt_id=$3")
            .bind(&id)
            .bind(at)
            .bind(a)
            .execute(&mut *tx)
            .await
            .expect("exact original restore");
        if bad_id {
            tx.execute("ALTER TABLE payment_dispatch_starts ADD CONSTRAINT payment_dispatch_starts_id_check CHECK(id ~ '^[a-f0-9]{32}$')").await.expect("restore exact id constraint");
        }
        tx.execute("ALTER TABLE payment_dispatch_starts ENABLE TRIGGER USER")
            .await
            .expect("restore guard");
        tx.commit().await.expect("healthy isolated original");
        assert!(
            read_observation(&pool, a, &first.id)
                .await
                .expect("positive restored receipt")
                == first
        );
    }
    assert_eq!(
        crate::payment_test_fixture::counts(&pool).await,
        (1, 4, 1, 1, 2)
    );
}
