//! Native refund-review custody using synthetic isolated PostgreSQL and loopback evidence.
use super::*;
use crate::{
    payment_attempts::{identities, observations, paid_parent_evidence, recognition, reversals},
    refund_execution_proposals::{ProposalError, inspect_reversal_evidence},
    staff_access::SessionToken,
};
use commerce_application::request_context::{Clock, Identifiers};
use order_stock_cases::fixture as orders;
use sqlx::Executor;
use std::sync::atomic::Ordering;

const PAYMENT: &str = "tr_SYNTHETIC_NotARealPayment";
const OTHER_PAYMENT: &str = "tr_OTHER_ReviewFixture";
const CHILD: &str = "re_REVIEWCUSTODY";

async fn staff(pool: &PgPool, byte: u8, grant: &str) -> SessionToken {
    let id = format!("review-evidence-{byte}");
    sqlx::query("INSERT INTO staff_accounts VALUES($1,true)")
        .bind(&id)
        .execute(pool)
        .await
        .expect("isolated staff");
    sqlx::query("INSERT INTO staff_capabilities VALUES($1,$2)")
        .bind(&id)
        .bind(grant)
        .execute(pool)
        .await
        .expect("explicit finance grant");
    let token = SessionToken::from_bytes([byte; 32]);
    sqlx::query("INSERT INTO staff_sessions VALUES($1,$2,1000,4000,1400,NULL)")
        .bind(token.digest().as_slice())
        .bind(&id)
        .execute(pool)
        .await
        .expect("recent synthetic MFA");
    token
}

async fn child(frozen: &FrozenAttempt, parent: &str, phase: &str) -> VerifiedReversal {
    let mut paid = payment(frozen.snapshot());
    paid["id"] = json!(parent);
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, paid),
        Reply::Json(
            200,
            json!({
                "resource":"refund","id":CHILD,"paymentId":parent,"mode":"test",
                "amount":{"currency":"EUR","value":"12.34"},"status":phase,
                "createdAt":"2024-03-20T09:13:37.123456789Z"
            }),
        ),
    ]);
    let verified = local(&server)
        .verify_profile()
        .await
        .expect("synthetic verified profile")
        .into_reconciliation()
        .fetch_refund(frozen.snapshot(), parent, CHILD)
        .await
        .expect("authentic original-order child evidence");
    assert_eq!(server.count(), 3);
    verified
}

async fn start_parent(
    pool: &PgPool,
    frozen: &FrozenAttempt,
    clock: &dyn Clock,
    ids: &dyn Identifiers,
) -> (
    Server,
    VerifiedReconciliationClient,
    paid_parent_evidence::Permit,
) {
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, payment(frozen.snapshot())),
    ]);
    let client = local(&server)
        .verify_profile()
        .await
        .expect("synthetic profile")
        .into_reconciliation();
    let permit = match Box::pin(paid_parent_evidence::start(
        pool,
        clock,
        ids,
        &frozen.snapshot().binding.id,
        "review-evidence-parent",
        &client,
    ))
    .await
    .expect("original authorized parent operation")
    {
        paid_parent_evidence::Start::Fetch(permit) => permit,
        _ => panic!("new operation must issue the fetch permit"),
    };
    (server, client, permit)
}

async fn custody(pool: &PgPool) -> (Value, Value) {
    let money_and_events = sqlx::query_scalar(
        "SELECT jsonb_build_object(
          'reservations',(SELECT jsonb_agg(to_jsonb(r) ORDER BY id) FROM refund_reservations r),
          'proposals',(SELECT jsonb_agg(to_jsonb(p) ORDER BY id) FROM refund_execution_proposals p),
          'operations',(SELECT jsonb_agg(to_jsonb(o) ORDER BY id) FROM paid_parent_total_operations o),
          'receipts',(SELECT jsonb_agg(to_jsonb(r) ORDER BY operation_id) FROM paid_parent_total_receipts r),
          'reversals',(SELECT jsonb_agg(to_jsonb(r) ORDER BY id) FROM payment_reversal_observations r),
          'recognition',(SELECT jsonb_agg(to_jsonb(r) ORDER BY id) FROM payment_recognitions r),
          'events',(SELECT jsonb_agg(to_jsonb(e) ORDER BY id) FROM commerce_outbox e))",
    ).fetch_one(pool).await.expect("exact money and custody rows");
    (money_and_events, orders::footprint(pool).await)
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn snapshot_binds_current_recognition_and_replays_exact_combined_custody(pool: PgPool) {
    let (clock, frozen, ids) = Box::pin(paid_parent_evidence_cases::paid(&pool)).await;
    let binding = &frozen.snapshot().binding;
    let token = staff(&pool, 93, "refund.execution.prepare").await;
    let read = || {
        Box::pin(inspect_reversal_evidence(
            &pool,
            &token,
            clock.as_ref(),
            &binding.order_id,
        ))
    };
    let empty = read()
        .await
        .expect("initial inspection")
        .expect("received proof");
    assert_eq!(empty.order_id, binding.order_id);
    assert_eq!(empty.through_observation, 1);
    assert_eq!(
        (
            empty.parent_operations,
            empty.parent_pending,
            empty.parent_complete,
            empty.child_observations
        ),
        (0, 0, 0, 0)
    );
    let (server, client, permit) =
        Box::pin(start_parent(&pool, &frozen, clock.as_ref(), &ids)).await;
    let pending = read()
        .await
        .expect("after durable operation")
        .expect("received proof");
    assert_eq!(
        (
            pending.parent_operations,
            pending.parent_pending,
            pending.parent_complete,
            pending.child_observations
        ),
        (1, 1, 0, 0)
    );
    assert_ne!(pending.digest, empty.digest);
    let fetched = paid_parent_evidence::fetch(permit, clock.as_ref(), &client)
        .await
        .expect("loopback paid-parent input");
    Box::pin(paid_parent_evidence::record(
        &pool,
        clock.as_ref(),
        &ids,
        &fetched,
    ))
    .await
    .expect("native parent receipt");
    let complete = read()
        .await
        .expect("after durable receipt")
        .expect("received proof");
    assert_eq!(
        (
            complete.parent_operations,
            complete.parent_pending,
            complete.parent_complete,
            complete.child_observations
        ),
        (1, 0, 1, 0)
    );
    assert_ne!(complete.digest, pending.digest);
    Box::pin(reversals::record(
        &pool,
        clock.as_ref(),
        &ids,
        "review-evidence-child",
        &child(&frozen, PAYMENT, "pending").await,
    ))
    .await
    .expect("locally retained child");
    let before = custody(&pool).await;
    let combined = read()
        .await
        .expect("inspect stored evidence")
        .expect("received proof");
    assert_eq!(
        (
            combined.parent_operations,
            combined.parent_pending,
            combined.parent_complete,
            combined.child_observations
        ),
        (1, 0, 1, 1)
    );
    assert_ne!(combined.digest, complete.digest);
    assert_eq!(combined.digest.len(), 64);
    assert!(
        combined
            .digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    );
    let debug = format!("{combined:?}");
    for sensitive in [PAYMENT, CHILD, &binding.id, "amount", "totals"] {
        assert!(
            !debug.contains(sensitive),
            "output keeps private custody redacted"
        );
    }
    assert_eq!(read().await.expect("exact repeat"), Some(combined));
    assert_eq!(custody(&pool).await, before);
    assert_eq!(server.count(), 2, "inspection issues no provider fetch");

    clock.0.store(1510, Ordering::SeqCst);
    let observed = observation_cases::fetched(frozen.snapshot(), PAYMENT, "paid", false).await;
    Box::pin(observations::record_verified(
        &pool,
        clock.as_ref(),
        &ids,
        &binding.id,
        "review-current-prefix",
        &observed,
    ))
    .await
    .expect("new genuine paid observation");
    assert_eq!(read().await.expect("stale current recognition"), None);
    Box::pin(identities::resolve(
        &pool,
        clock.as_ref(),
        &ids,
        &binding.id,
        "review-current-prefix",
    ))
    .await
    .expect("complete current identity");
    assert_eq!(
        read().await.expect("resolution alone is insufficient"),
        None
    );
    let refreshed = Box::pin(recognition::recognize(
        &pool,
        clock.as_ref(),
        &ids,
        &binding.id,
    ))
    .await
    .expect("fresh recognition")
    .recognition;
    let next = read()
        .await
        .expect("fresh paired custody")
        .expect("received proof");
    assert_eq!(next.through_observation, 2);
    assert_eq!(next.recognition_id, refreshed.id);
    assert_eq!((next.parent_operations, next.child_observations), (1, 1));
    assert_ne!(next.digest, complete.digest);
    clock.0.store(1509, Ordering::SeqCst);
    assert_eq!(read().await, Err(ProposalError::ClockUnavailable));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn snapshot_rejects_a_valid_retained_child_for_a_different_parent(pool: PgPool) {
    let (clock, frozen, ids) = Box::pin(paid_parent_evidence_cases::paid(&pool)).await;
    let binding = &frozen.snapshot().binding;
    let token = staff(&pool, 94, "refund.execution.prepare").await;
    let read = || {
        Box::pin(inspect_reversal_evidence(
            &pool,
            &token,
            clock.as_ref(),
            &binding.order_id,
        ))
    };
    assert!(read().await.expect("matching control").is_some());
    let row = Box::pin(reversals::record(
        &pool,
        clock.as_ref(),
        &ids,
        "different-parent",
        &child(&frozen, OTHER_PAYMENT, "refunded").await,
    ))
    .await
    .expect("valid retained evidence under another candidate parent");
    assert_eq!(row.payment_id, OTHER_PAYMENT);
    assert_eq!(
        reversals::inspect_set(&pool, &binding.id)
            .await
            .expect("valid detached journal")
            .observations,
        1
    );
    let before = custody(&pool).await;
    assert_eq!(read().await, Err(ProposalError::Conflict));
    assert_eq!(
        custody(&pool).await,
        before,
        "valid evidence remains in custody"
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn snapshot_requires_exact_authority_and_received_source(pool: PgPool) {
    let fixture = Box::pin(orders::accepted(&pool, false, false)).await;
    let order = &fixture.order.identity.id;
    let attempt = &fixture.frozen.snapshot().binding.id;
    let token = staff(&pool, 95, "refund.execution.prepare").await;
    let reserver = staff(&pool, 96, "refund.prepare").await;
    let read = |token, order| {
        Box::pin(inspect_reversal_evidence(
            &pool,
            token,
            fixture.time.as_ref(),
            order,
        ))
    };
    assert_eq!(
        read(&token, "invalid-order").await,
        Err(ProposalError::InvalidRequest)
    );
    assert_eq!(read(&reserver, order).await, Err(ProposalError::Denied));
    let absent = "0".repeat(32);
    assert_eq!(
        read(&token, &absent).await.expect("unrelated absent order"),
        None
    );
    assert_eq!(read(&token, order).await.expect("no recognition"), None);
    Box::pin(orders::observe(
        &pool,
        &fixture,
        "tr_OPEN_REVIEW",
        "open",
        "review-unreceived",
    ))
    .await;
    let open = Box::pin(recognition::recognize(
        &pool,
        fixture.time.as_ref(),
        fixture.ids.as_ref(),
        attempt,
    ))
    .await
    .expect("valid pending recognition")
    .recognition;
    assert_ne!(open.state, recognition::State::Received);
    assert_eq!(
        read(&token, order).await.expect("unreceived recognition"),
        None
    );
    pool.execute("ALTER TABLE commerce_outbox DISABLE TRIGGER USER; DELETE FROM commerce_outbox WHERE kind='payment.recognized.v1'; ALTER TABLE commerce_outbox ENABLE TRIGGER USER")
        .await.expect("isolated damaged recognition custody");
    assert_eq!(
        read(&token, order).await,
        Err(ProposalError::InvalidStoredData)
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn snapshot_checks_recent_mfa_again_after_the_original_attempt_claim(pool: PgPool) {
    let (clock, frozen, _) = Box::pin(paid_parent_evidence_cases::paid(&pool)).await;
    let binding = &frozen.snapshot().binding;
    let token = staff(&pool, 97, "refund.execution.prepare").await;
    let before = custody(&pool).await;
    let mut blocker = pool.begin().await.expect("isolated claim holder");
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .expect("claim holder PID");
    sqlx::query("SELECT id FROM payment_attempts WHERE id=$1 FOR UPDATE")
        .bind(&binding.id)
        .execute(&mut *blocker)
        .await
        .expect("claim original attempt");
    let reader = {
        let pool = pool.clone();
        let clock = clock.clone();
        let order = binding.order_id.clone();
        tokio::spawn(async move {
            Box::pin(inspect_reversal_evidence(
                &pool,
                &token,
                clock.as_ref(),
                &order,
            ))
            .await
        })
    };
    tokio::time::timeout(Duration::from_millis(800), async {
        loop {
            let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND $1=ANY(pg_blocking_pids(pid)))")
                .bind(pid).fetch_one(&pool).await.expect("actual attempt blocker");
            if waiting {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }).await.expect("snapshot waits on the original attempt row");
    clock.0.store(2301, Ordering::SeqCst);
    blocker.rollback().await.expect("release original attempt");
    assert_eq!(
        reader.await.expect("reader task"),
        Err(ProposalError::Denied)
    );
    assert_eq!(custody(&pool).await, before);
    clock.0.store(1500, Ordering::SeqCst);
    let token = SessionToken::from_bytes([97; 32]);
    assert!(
        Box::pin(inspect_reversal_evidence(
            &pool,
            &token,
            clock.as_ref(),
            &binding.order_id,
        ))
        .await
        .expect("fresh control")
        .is_some()
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn snapshot_distinguishes_unstarted_order_from_malformed_attempt_custody(pool: PgPool) {
    let (clock, frozen) = Box::pin(observation_cases::prepared(&pool, false)).await;
    let binding = &frozen.snapshot().binding;
    let token = staff(&pool, 98, "refund.execution.prepare").await;
    let read = || {
        Box::pin(inspect_reversal_evidence(
            &pool,
            &token,
            clock.as_ref(),
            &binding.order_id,
        ))
    };
    assert_eq!(
        read().await.expect("no dispatch for original attempt"),
        None
    );
    pool.execute("ALTER TABLE payment_attempts DISABLE TRIGGER USER; ALTER TABLE payment_attempts DROP CONSTRAINT payment_attempts_id_check; UPDATE payment_attempts SET id='broken',idempotency_key='piwas-test-payment-broken'")
        .await.expect("isolated malformed original attempt identity");
    assert_eq!(read().await, Err(ProposalError::InvalidStoredData));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn snapshot_returns_database_failure_if_claim_or_child_journal_is_unavailable(pool: PgPool) {
    let (clock, frozen, _) = Box::pin(paid_parent_evidence_cases::paid(&pool)).await;
    let binding = &frozen.snapshot().binding;
    let token = staff(&pool, 99, "refund.execution.prepare").await;
    let read = || {
        Box::pin(inspect_reversal_evidence(
            &pool,
            &token,
            clock.as_ref(),
            &binding.order_id,
        ))
    };
    let original = read().await.expect("uncontended original evidence");
    let before = custody(&pool).await;
    for lock in [
        "LOCK TABLE payment_attempts IN ACCESS EXCLUSIVE MODE",
        "LOCK TABLE payment_reversal_observations IN ACCESS EXCLUSIVE MODE",
    ] {
        let mut blocker = pool.begin().await.expect("isolated blocker");
        blocker.execute(lock).await.expect("hold exact relation");
        assert_eq!(read().await, Err(ProposalError::Database));
        blocker.rollback().await.expect("release exact relation");
        assert_eq!(read().await.expect("exact restored evidence"), original);
    }
    assert_eq!(custody(&pool).await, before);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn snapshot_rechecks_time_against_every_retained_custody_stage(pool: PgPool) {
    let (clock, frozen, ids) = Box::pin(paid_parent_evidence_cases::paid(&pool)).await;
    let binding = &frozen.snapshot().binding;
    let token = staff(&pool, 100, "refund.execution.prepare").await;
    let read = || {
        Box::pin(inspect_reversal_evidence(
            &pool,
            &token,
            clock.as_ref(),
            &binding.order_id,
        ))
    };
    let first = read()
        .await
        .expect("initial recognition")
        .expect("received proof");
    assert_eq!(first.through_observation, 1);

    clock.0.store(1510, Ordering::SeqCst);
    let (_server, client, permit) =
        Box::pin(start_parent(&pool, &frozen, clock.as_ref(), &ids)).await;
    assert_eq!(
        read()
            .await
            .expect("start at exact time")
            .expect("evidence")
            .parent_pending,
        1
    );
    clock.0.store(1509, Ordering::SeqCst);
    assert_eq!(read().await, Err(ProposalError::ClockUnavailable));

    clock.0.store(1520, Ordering::SeqCst);
    let fetched = paid_parent_evidence::fetch(permit, clock.as_ref(), &client)
        .await
        .expect("bounded loopback fetch");
    Box::pin(paid_parent_evidence::record(
        &pool,
        clock.as_ref(),
        &ids,
        &fetched,
    ))
    .await
    .expect("later completed parent receipt");
    assert_eq!(
        read()
            .await
            .expect("receipt at exact time")
            .expect("evidence")
            .parent_complete,
        1
    );
    clock.0.store(1519, Ordering::SeqCst);
    assert_eq!(read().await, Err(ProposalError::ClockUnavailable));

    clock.0.store(1530, Ordering::SeqCst);
    Box::pin(reversals::record(
        &pool,
        clock.as_ref(),
        &ids,
        "review-newer-child",
        &child(&frozen, PAYMENT, "pending").await,
    ))
    .await
    .expect("later retained child");
    let before = custody(&pool).await;
    let complete = read()
        .await
        .expect("child at exact time")
        .expect("evidence");
    assert_eq!(complete.child_observations, 1);
    assert_eq!(complete.recognition_id, first.recognition_id);
    clock.0.store(1529, Ordering::SeqCst);
    assert_eq!(read().await, Err(ProposalError::ClockUnavailable));
    assert_eq!(custody(&pool).await, before);
}

#[sqlx::test(migrations = false)]
async fn schema83_preserves_parent_custody_and_fences_orphan_or_changed_events(pool: PgPool) {
    MIGRATOR
        .run_to(82, &pool)
        .await
        .expect("prior exact schema");
    let (clock, frozen, ids) = Box::pin(paid_parent_evidence_cases::paid(&pool)).await;
    let binding = &frozen.snapshot().binding;
    let token = staff(&pool, 101, "refund.execution.prepare").await;
    let (_server, client, permit) =
        Box::pin(start_parent(&pool, &frozen, clock.as_ref(), &ids)).await;
    let fetched = paid_parent_evidence::fetch(permit, clock.as_ref(), &client)
        .await
        .expect("bounded loopback fetch");
    let receipt = Box::pin(paid_parent_evidence::record(
        &pool,
        clock.as_ref(),
        &ids,
        &fetched,
    ))
    .await
    .expect("original schema 82 receipt");
    let order_events = || {
        sqlx::query_scalar::<_, Value>(
            "SELECT to_jsonb(e) FROM commerce_outbox e WHERE aggregate_id=$1 ORDER BY id",
        )
        .bind(&binding.order_id)
        .fetch_all(&pool)
    };
    let before = order_events().await.expect("original outbox rows");

    // Planner volume only. These deliberately unrelated aggregates are never run
    // through normal custody paths and exist only in the disposable database.
    pool.execute(
        "ALTER TABLE commerce_outbox DISABLE TRIGGER ALL;
         INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at)
         SELECT md5('parent-planner-event:' || n),md5('parent-planner-order:' || (n / 64)),
                'payment.paid_parent_totals_observed.v1',jsonb_build_object('operation_id', md5('parent-planner-operation:' || n)),0
         FROM generate_series(1,16384) AS source(n);
         ALTER TABLE commerce_outbox ENABLE TRIGGER ALL;",
    ).await.expect("synthetic planner controls");
    MIGRATOR.run_to(83, &pool).await.expect("new order index");
    assert_eq!(
        order_events().await.expect("preserved order events"),
        before
    );
    pool.execute("ANALYZE commerce_outbox")
        .await
        .expect("planner statistics");
    let plan: Value = sqlx::query_scalar(concat!(
        "EXPLAIN (ANALYZE, FORMAT JSON) ",
        include_str!("../../src/payment_attempts/paid_parent_evidence/scoped_event_count.sql")
    ))
    .bind(&binding.order_id)
    .fetch_one(&pool)
    .await
    .expect("actual sparse order plan");
    let limit = &plan[0]["Plan"]["Plans"][0];
    assert_eq!(limit["Actual Rows"].as_f64(), Some(1.0));
    let rendered = plan.to_string();
    assert!(
        rendered.contains("commerce_outbox_paid_parent_totals_order"),
        "{plan}"
    );
    assert!(
        rendered.contains("Index Cond") && rendered.contains(&binding.order_id),
        "{plan}"
    );

    let read = || {
        Box::pin(inspect_reversal_evidence(
            &pool,
            &token,
            clock.as_ref(),
            &binding.order_id,
        ))
    };
    let intact = read()
        .await
        .expect("schema 83 paired custody")
        .expect("exact original evidence");
    assert_eq!(intact.parent_complete, 1);
    let mut damage = pool.begin().await.expect("isolated orphan fixture");
    damage
        .execute("ALTER TABLE commerce_outbox DISABLE TRIGGER USER")
        .await
        .expect("fixture-only event import");
    sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) VALUES (md5('parent-orphan'),$1,'payment.paid_parent_totals_observed.v1','{}',1500)")
        .bind(&binding.order_id).execute(&mut *damage).await.expect("order-scoped malformed orphan");
    damage
        .execute("ALTER TABLE commerce_outbox ENABLE TRIGGER USER")
        .await
        .expect("restore append-only trigger");
    damage.commit().await.expect("isolate imported damage");
    assert_eq!(read().await, Err(ProposalError::InvalidStoredData));
    assert_eq!(
        paid_parent_evidence::inspect_set(&pool, &binding.id)
            .await
            .expect("existing detached contract")
            .complete,
        1
    );

    let mut restore = pool.begin().await.expect("isolated event repair");
    restore.execute("ALTER TABLE commerce_outbox DISABLE TRIGGER USER; DELETE FROM commerce_outbox WHERE id=md5('parent-orphan')")
        .await.expect("remove only test orphan");
    sqlx::query("UPDATE commerce_outbox SET payload=payload || jsonb_build_object('receipt_sha256',repeat('0',64)) WHERE id=$1")
        .bind(&receipt.event_id).execute(&mut *restore).await.expect("damaged retained receipt event");
    restore
        .execute("ALTER TABLE commerce_outbox ENABLE TRIGGER USER")
        .await
        .expect("restore append-only trigger");
    restore.commit().await.expect("isolate changed import");
    assert_eq!(read().await, Err(ProposalError::InvalidStoredData));
    let original_event = before
        .iter()
        .find(|event| event["id"] == receipt.event_id)
        .expect("original exact event");
    let mut restore = pool.begin().await.expect("isolated exact event repair");
    restore
        .execute("ALTER TABLE commerce_outbox DISABLE TRIGGER USER")
        .await
        .expect("fixture repair");
    sqlx::query("UPDATE commerce_outbox SET payload=$2 WHERE id=$1")
        .bind(&receipt.event_id)
        .bind(&original_event["payload"])
        .execute(&mut *restore)
        .await
        .expect("restore exact event digest");
    restore
        .execute("ALTER TABLE commerce_outbox ENABLE TRIGGER USER")
        .await
        .expect("restore append-only trigger");
    restore
        .commit()
        .await
        .expect("publish exact original event");
    assert_eq!(read().await.expect("repaired scope"), Some(intact));
    assert_eq!(
        order_events().await.expect("exact restored order events"),
        before
    );
}
