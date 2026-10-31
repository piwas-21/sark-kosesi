//! Disposable PostgreSQL plus authenticated GET-only loopback recovery oracles.
use super::observation_cases::{BadPorts, ids, prepared};
use super::*;
use crate::{
    MIGRATOR,
    payment_attempts::reconciliation::{jobs, steps, *},
};
use sqlx::{Executor, PgPool};
use std::sync::{Arc, atomic::Ordering};
fn page(a: &AttemptSnapshot, candidates: &[(&str, &str)], next: Option<&str>) -> Reply {
    let payments: Vec<_> = candidates
        .iter()
        .map(|(id, status)| {
            let mut p = payment(a);
            p["id"] = (*id).into();
            p["status"] = (*status).into();
            p
        })
        .collect();
    let next = next.map_or(Value::Null, |id| {
        let mut u = url::Url::parse("https://api.mollie.com/v2/payments").expect("fixed");
        u.query_pairs_mut()
            .append_pair("from", id)
            .append_pair("limit", "10");
        json!({"href":u.as_str()})
    });
    Reply::Json(
        200,
        json!({"count":payments.len(),"_embedded":{"payments":payments},"_links":{"next":next}}),
    )
}
fn fetched(a: &AttemptSnapshot, id: &str, status: &str) -> Reply {
    let mut p = payment(a);
    p["id"] = id.into();
    p["status"] = status.into();
    Reply::Json(200, p)
}
async fn read_client(server: &Server) -> VerifiedReconciliationClient {
    let key = credential();
    ReconciliationClient {
        http: http::Http::build(
            server.base.clone(),
            key.authorization,
            Duration::from_secs(2),
        )
        .expect("test transport"),
        profile: key.binding,
    }
    .verify_profile()
    .await
    .expect("authenticated same profile")
}
fn get_only(server: &Server) {
    server.inspect(|rs| {
        assert!(rs.iter().all(|r| r.method == "GET"
            && r.body.is_empty()
            && r.header("Idempotency-Key").is_none()))
    });
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn full_scan_fetches_every_candidate_retains_ambiguity_and_stops_without_post(pool: PgPool) {
    let (time, f) = prepared(&pool, true).await;
    let a = &f.snapshot().binding.id;
    let server = Server::start(vec![
        profile(),
        page(f.snapshot(), &[("tr_A", "paid"), ("tr_B", "paid")], None),
        fetched(f.snapshot(), "tr_A", "paid"),
        fetched(f.snapshot(), "tr_B", "paid"),
    ]);
    let client = local(&server)
        .verify_profile()
        .await
        .expect("profile")
        .into_reconciliation();
    let i = ids();
    let batch = run_batch(&pool, time.as_ref(), &i, &client, 25)
        .await
        .expect("bounded supervised pass");
    assert_eq!(batch.processed, 4);
    assert_eq!(batch.needs_review, 1);
    assert_eq!(batch.retry_pending, 0);
    let job = read_job(&pool, a).await.expect("inspect").expect("job");
    assert!(matches!(job.review_reason, Some(ReviewReason::Ambiguous)));
    let resolution = crate::payment_attempts::identities::latest(&pool, a)
        .await
        .expect("resolution")
        .expect("exists");
    assert_eq!(resolution.summary.provider_ids, vec!["tr_A", "tr_B"]);
    assert_eq!(resolution.summary.paid_provider_ids, vec!["tr_A", "tr_B"]);
    assert_eq!(server.count(), 4);
    get_only(&server);
    assert_eq!(
        run_batch(&pool, time.as_ref(), &i, &client, 25)
            .await
            .expect("no automatic review restart")
            .processed,
        0
    );
    assert_eq!(
        crate::payment_test_fixture::counts(&pool).await.4,
        2,
        "no stock transition"
    );
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn empty_end_pages_and_late_paid_evidence_stop_for_review_without_absence_claim(
    pool: PgPool,
) {
    let (time, f) = prepared(&pool, true).await;
    time.0.store(5000, Ordering::SeqCst);
    let server = Server::start(vec![
        profile(),
        page(f.snapshot(), &[], Some("tr_NEXT")),
        page(f.snapshot(), &[], None),
    ]);
    let client = read_client(&server).await;
    let b = run_batch(&pool, time.as_ref(), &ids(), &client, 25)
        .await
        .expect("expired original stays recoverable");
    assert_eq!(b.processed, 4);
    let job = read_job(&pool, &f.snapshot().binding.id)
        .await
        .expect("job")
        .expect("found");
    assert!(matches!(job.review_reason, Some(ReviewReason::Unresolved)));
    assert_eq!(server.count(), 3);
    get_only(&server);
    server.inspect(|r| assert_eq!(r[2].path, "/v2/payments?limit=10&sort=desc&from=tr_NEXT"));
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn pending_identity_polls_with_delay_and_paid_is_retained_after_unknown_create(pool: PgPool) {
    let (time, f) = prepared(&pool, true).await;
    let a = &f.snapshot().binding.id;
    let server = Server::start(vec![
        profile(),
        page(f.snapshot(), &[("tr_A", "pending")], None),
        fetched(f.snapshot(), "tr_A", "pending"),
        fetched(f.snapshot(), "tr_A", "paid"),
    ]);
    let client = read_client(&server).await;
    let i = ids();
    let first = run_batch(&pool, time.as_ref(), &i, &client, 25)
        .await
        .expect("scan and defer poll");
    assert_eq!(first.processed, 3);
    assert_eq!(server.count(), 3);
    let job = read_job(&pool, a).await.expect("job").expect("found");
    assert!(job.phase == Phase::Poll);
    assert_eq!(job.due_at, 1560);
    assert_eq!(
        run_batch(&pool, time.as_ref(), &i, &client, 25)
            .await
            .expect("before due")
            .processed,
        0
    );
    time.0.store(1560, Ordering::SeqCst);
    let paid = run_batch(&pool, time.as_ref(), &i, &client, 25)
        .await
        .expect("poll");
    assert_eq!(paid.needs_review, 1);
    assert!(matches!(
        read_job(&pool, a)
            .await
            .expect("job")
            .expect("found")
            .review_reason,
        Some(ReviewReason::PaidObserved)
    ));
    get_only(&server);
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn provider_failures_back_off_exhaust_and_malformed_evidence_requires_review(pool: PgPool) {
    let (time, f) = prepared(&pool, true).await;
    let server = Server::start(vec![
        profile(),
        Reply::Json(503, json!({})),
        Reply::Json(503, json!({})),
        Reply::Json(503, json!({})),
        Reply::Json(503, json!({})),
        Reply::Json(503, json!({})),
    ]);
    let client = read_client(&server).await;
    let i = ids();
    for (at, due, failures) in [
        (1500, 1560, 1),
        (1560, 1680, 2),
        (1680, 1920, 3),
        (1920, 2400, 4),
        (2400, 2400, 5),
    ] {
        time.0.store(at, Ordering::SeqCst);
        let batch = run_batch(&pool, time.as_ref(), &i, &client, 25)
            .await
            .expect("retry decision");
        assert_eq!(batch.processed, 1);
        let job = read_job(&pool, &f.snapshot().binding.id)
            .await
            .expect("job")
            .expect("found");
        assert_eq!(job.failures, failures);
        assert_eq!(job.due_at, due);
    }
    assert!(matches!(
        read_job(&pool, &f.snapshot().binding.id)
            .await
            .expect("job")
            .expect("found")
            .review_reason,
        Some(ReviewReason::RetryExhausted)
    ));
    get_only(&server);
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn malformed_page_never_advances_or_retries_as_another_create(pool: PgPool) {
    let (time, f) = prepared(&pool, true).await;
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, json!({"unexpected":true})),
    ]);
    let client = read_client(&server).await;
    let b = run_batch(&pool, time.as_ref(), &ids(), &client, 25)
        .await
        .expect("operator stop");
    assert_eq!(b.needs_review, 1);
    assert!(matches!(
        read_job(&pool, &f.snapshot().binding.id)
            .await
            .expect("job")
            .expect("found")
            .review_reason,
        Some(ReviewReason::CustodyInvalid)
    ));
    get_only(&server);
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn crashed_page_checkpoint_is_reused_and_expired_worker_cannot_advance_progress(
    pool: PgPool,
) {
    let (time, f) = prepared(&pool, true).await;
    let a = &f.snapshot().binding.id;
    let i = ids();
    let server = Server::start(vec![
        profile(),
        page(f.snapshot(), &[("tr_A", "paid")], None),
        fetched(f.snapshot(), "tr_A", "paid"),
    ]);
    let client = read_client(&server).await;
    jobs::enqueue(&pool, time.as_ref(), client.profile_id())
        .await
        .expect("schedule");
    let stale = jobs::claim(&pool, time.as_ref(), client.profile_id())
        .await
        .expect("claim")
        .expect("first");
    assert!(
        jobs::claim(&pool, time.as_ref(), client.profile_id())
            .await
            .expect("exclusive")
            .is_none()
    );
    let result = steps::advance(&pool, time.as_ref(), &i, &client, &stale)
        .await
        .expect("page receipt durable before lost checkpoint");
    time.0.store(1560, Ordering::SeqCst);
    let current = jobs::claim(&pool, time.as_ref(), client.profile_id())
        .await
        .expect("reclaim")
        .expect("new lease");
    assert_eq!(current.job.generation, 2);
    assert_eq!(
        jobs::finish(&pool, time.as_ref(), &stale, &result)
            .await
            .err(),
        Some(RecoveryError::LeaseLost)
    );
    let recovered = steps::advance(&pool, time.as_ref(), &i, &client, &current)
        .await
        .expect("same page receipt without another GET");
    assert_eq!(server.count(), 2);
    jobs::finish(&pool, time.as_ref(), &current, &recovered)
        .await
        .expect("checkpoint");
    run_batch(&pool, time.as_ref(), &i, &client, 25)
        .await
        .expect("fetch and review");
    assert!(matches!(
        read_job(&pool, a)
            .await
            .expect("job")
            .expect("found")
            .review_reason,
        Some(ReviewReason::PaidObserved)
    ));
    assert_eq!(server.count(), 3);
    get_only(&server);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn nonpaid_poll_limit_and_step_limit_are_visible_operator_stops(pool: PgPool) {
    let (time, f) = prepared(&pool, true).await;
    let a = &f.snapshot().binding.id;
    let i = ids();
    let mut replies = vec![
        profile(),
        page(f.snapshot(), &[("tr_A", "open")], None),
        fetched(f.snapshot(), "tr_A", "open"),
    ];
    for _ in 0..10 {
        replies.push(fetched(f.snapshot(), "tr_A", "open"));
    }
    let server = Server::start(replies);
    let client = read_client(&server).await;
    run_batch(&pool, time.as_ref(), &i, &client, 25)
        .await
        .expect("discover");
    for at in (1560..=2100).step_by(60) {
        time.0.store(at, Ordering::SeqCst);
        run_batch(&pool, time.as_ref(), &i, &client, 25)
            .await
            .expect("bounded poll");
    }
    let job = read_job(&pool, a).await.expect("job").expect("found");
    assert_eq!(job.polls, 10);
    assert!(matches!(job.review_reason, Some(ReviewReason::PollLimit)));
    get_only(&server);
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn exhausted_claim_budget_cannot_become_a_silent_unclaimed_job(pool: PgPool) {
    let (time, f) = prepared(&pool, true).await;
    let a = &f.snapshot().binding.id;
    let server = Server::start(vec![profile(), page(f.snapshot(), &[], None)]);
    let client = read_client(&server).await;
    jobs::enqueue(&pool, time.as_ref(), client.profile_id())
        .await
        .expect("enqueue");
    pool.execute("ALTER TABLE payment_reconciliation_jobs DISABLE TRIGGER USER; UPDATE payment_reconciliation_jobs SET generation=1499,operation=1499; ALTER TABLE payment_reconciliation_jobs ENABLE TRIGGER USER").await.expect("bounded limit fixture");
    let b = run_batch(&pool, time.as_ref(), &ids(), &client, 1)
        .await
        .expect("last safe step");
    assert_eq!(b.processed, 1);
    assert_eq!(b.needs_review, 1);
    assert!(matches!(
        read_job(&pool, a)
            .await
            .expect("job")
            .expect("found")
            .review_reason,
        Some(ReviewReason::StepLimit)
    ));
    assert_eq!(
        run_batch(&pool, time.as_ref(), &ids(), &client, 25)
            .await
            .expect("no restart")
            .processed,
        0
    );
    get_only(&server);
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn cancelled_fetch_keeps_paid_evidence_and_new_lease_cannot_downgrade_it(pool: PgPool) {
    let (time, f) = prepared(&pool, true).await;
    let a = &f.snapshot().binding.id;
    let i = ids();
    let server = Server::start(vec![
        profile(),
        page(f.snapshot(), &[("tr_A", "pending")], None),
        fetched(f.snapshot(), "tr_A", "paid"),
        fetched(f.snapshot(), "tr_A", "open"),
    ]);
    let client = read_client(&server).await;
    run_batch(&pool, time.as_ref(), &i, &client, 1)
        .await
        .expect("checkpoint page");
    let stale = jobs::claim(&pool, time.as_ref(), client.profile_id())
        .await
        .expect("lease")
        .expect("claim");
    let result = steps::advance(&pool, time.as_ref(), &i, &client, &stale)
        .await
        .expect("paid evidence committed, checkpoint lost");
    time.0.store(1560, Ordering::SeqCst);
    let next = run_batch(&pool, time.as_ref(), &i, &client, 25)
        .await
        .expect("safe fresh GET and retained evidence");
    assert_eq!(next.needs_review, 1);
    assert_eq!(
        jobs::finish(&pool, time.as_ref(), &stale, &result)
            .await
            .err(),
        Some(RecoveryError::LeaseLost)
    );
    let r = crate::payment_attempts::identities::latest(&pool, a)
        .await
        .expect("identity")
        .expect("known");
    assert_eq!(r.summary.paid_provider_ids, vec!["tr_A"]);
    get_only(&server);
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn concurrent_supervisors_claim_once_and_never_hold_database_claims_during_io(pool: PgPool) {
    let (time, f) = prepared(&pool, true).await;
    let i = Arc::new(ids());
    let gate = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let server = Server::start(vec![profile(), Reply::Gate(gate.clone())]);
    let client = Arc::new(read_client(&server).await);
    let p = pool.clone();
    let t = time.clone();
    let v = client.clone();
    let ids = i.clone();
    let task =
        tokio::spawn(async move { run_batch(&p, t.as_ref(), ids.as_ref(), v.as_ref(), 1).await });
    tokio::time::timeout(Duration::from_secs(1), async {
        while server.count() < 2 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("provider request in progress");
    let other = run_batch(&pool, time.as_ref(), i.as_ref(), client.as_ref(), 1)
        .await
        .expect("no duplicate lease");
    assert_eq!(other.processed, 0);
    let mut tx = pool.begin().await.expect("independent claimant");
    tx.execute("SET LOCAL lock_timeout='100ms'")
        .await
        .expect("bound");
    sqlx::query("SELECT id FROM payment_attempts WHERE id=$1 FOR UPDATE NOWAIT")
        .bind(&f.snapshot().binding.id)
        .fetch_one(&mut *tx)
        .await
        .expect("no attempt lock across provider GET");
    tx.rollback().await.expect("release");
    gate.store(true, Ordering::SeqCst);
    let stopped = task
        .await
        .expect("task")
        .expect("malformed gate reply stops safely");
    assert_eq!(stopped.needs_review, 1);
    get_only(&server);
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn missing_authority_invalid_limits_clocks_and_storage_never_start_io(pool: PgPool) {
    let (time, f) = prepared(&pool, false).await;
    let server = Server::start(vec![profile()]);
    let client = read_client(&server).await;
    assert_eq!(
        run_batch(&pool, time.as_ref(), &ids(), &client, 0)
            .await
            .err(),
        Some(RecoveryError::InvalidRequest)
    );
    assert_eq!(
        run_batch(&pool, time.as_ref(), &ids(), &client, 26)
            .await
            .err(),
        Some(RecoveryError::InvalidRequest)
    );
    assert_eq!(
        run_batch(&pool, &BadPorts, &ids(), &client, 1).await.err(),
        Some(RecoveryError::ClockUnavailable)
    );
    assert_eq!(
        run_batch(&pool, time.as_ref(), &ids(), &client, 1)
            .await
            .expect("no started attempt")
            .processed,
        0
    );
    assert!(
        read_job(&pool, &f.snapshot().binding.id)
            .await
            .expect("no job")
            .is_none()
    );
    assert_eq!(
        read_job(&pool, "bad").await.err(),
        Some(RecoveryError::InvalidRequest)
    );
    time.0.store(u64::MAX, Ordering::SeqCst);
    assert_eq!(
        run_batch(&pool, time.as_ref(), &ids(), &client, 1)
            .await
            .err(),
        Some(RecoveryError::ClockUnavailable)
    );
    time.0.store(9_007_199_254_740_991, Ordering::SeqCst);
    assert_eq!(
        run_batch(&pool, time.as_ref(), &ids(), &client, 1)
            .await
            .err(),
        Some(RecoveryError::ClockUnavailable)
    );
    pool.close().await;
    assert_eq!(
        run_batch(&pool, time.as_ref(), &ids(), &client, 1)
            .await
            .err(),
        Some(RecoveryError::Storage)
    );
    assert_eq!(
        read_job(&pool, &f.snapshot().binding.id).await.err(),
        Some(RecoveryError::Storage)
    );
    assert_eq!(server.count(), 1);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn native_job_custody_rejects_orphans_rebinding_mutation_and_manual_restart(pool: PgPool) {
    let (time, f) = prepared(&pool, true).await;
    let a = &f.snapshot().binding.id;
    assert!(sqlx::query("INSERT INTO payment_reconciliation_jobs(attempt_id,source_event_id,created_at,updated_at,due_at) VALUES($1,$2,1500,1500,1500)").bind(a).bind("f".repeat(32)).execute(&pool).await.is_err());
    jobs::enqueue(&pool, time.as_ref(), &f.snapshot().binding.profile.id)
        .await
        .expect("healthy source");
    for sql in [
        "UPDATE payment_reconciliation_jobs SET source_event_id=repeat('f',32)",
        "UPDATE payment_reconciliation_jobs SET page_id=repeat('f',32)",
        "UPDATE payment_reconciliation_jobs SET updated_at=1499",
        "DELETE FROM payment_reconciliation_jobs",
    ] {
        assert!(pool.execute(sql).await.is_err());
    }
    let server = Server::start(vec![profile(), page(f.snapshot(), &[], None)]);
    let client = read_client(&server).await;
    run_batch(&pool, time.as_ref(), &ids(), &client, 25)
        .await
        .expect("explicit review stop");
    assert!(
        pool.execute("UPDATE payment_reconciliation_jobs SET phase='search',review_reason=NULL")
            .await
            .is_err(),
        "no silent operator restart"
    );
    pool.execute("ALTER TABLE payment_reconciliation_jobs DISABLE TRIGGER USER; UPDATE payment_reconciliation_jobs SET source_event_id=repeat('f',32); ALTER TABLE payment_reconciliation_jobs ENABLE TRIGGER USER").await.expect_err("source event FK remains enforced");
    // Roll back failed statement batch; the original operational receipt still validates.
    assert!(read_job(&pool, a).await.is_ok());
}

#[path = "payment_reconciliation_clocks.rs"]
mod clock_cases;
#[path = "payment_reconciliation_drift.rs"]
mod drift_cases;
#[path = "payment_reconciliation_limits.rs"]
mod limit_cases;

#[sqlx::test(migrations = false)]
async fn upgrade_preserves_paid_identity_and_claims_without_starting_a_recovery_job(pool: PgPool) {
    MIGRATOR
        .run_to(26, &pool)
        .await
        .expect("actual prior schema");
    let (time, f) = prepared(&pool, true).await;
    let a = &f.snapshot().binding.id;
    let i = ids();
    crate::payment_attempts::observations::record_verified(
        &pool,
        time.as_ref(),
        &i,
        a,
        "paid",
        &super::observation_cases::fetched(f.snapshot(), "tr_A", "paid", false).await,
    )
    .await
    .expect("prior authenticated paid fact");
    let resolution = crate::payment_attempts::identities::resolve(
        &pool,
        time.as_ref(),
        &i,
        a,
        "prior-resolution",
    )
    .await
    .expect("prior ownership")
    .resolution;
    let before = crate::payment_test_fixture::counts(&pool).await;
    MIGRATOR.run(&pool).await.expect("forward upgrade");
    assert_eq!(crate::payment_test_fixture::counts(&pool).await, before);
    let old = crate::payment_attempts::identities::read_recorded(&pool, a, "prior-resolution")
        .await
        .expect("old reader")
        .expect("receipt");
    assert!(old == resolution);
    assert!(read_job(&pool, a).await.expect("unstarted").is_none());
    jobs::enqueue(&pool, time.as_ref(), &f.snapshot().binding.profile.id)
        .await
        .expect("explicit scheduling");
    MIGRATOR.run(&pool).await.expect("idempotent migration");
    assert_eq!(
        read_job(&pool, a)
            .await
            .expect("durable job")
            .expect("found")
            .generation,
        0
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn unchanged_review_and_backoff_status_remain_visible_without_crossing_profiles(
    pool: PgPool,
) {
    let (time, f) = prepared(&pool, true).await;
    let other =
        super::identity_ownership::additional(&pool, time.clone(), "pfl_OTHER_SYNTHETIC").await;
    jobs::enqueue(&pool, time.as_ref(), &other.snapshot().binding.profile.id)
        .await
        .expect("other profile recovery");
    pool.execute(
        "UPDATE payment_reconciliation_jobs SET phase='needs_review',review_reason='unresolved'",
    )
    .await
    .expect("other profile stop");
    let server = Server::start(vec![profile(), Reply::Json(503, json!({}))]);
    let client = read_client(&server).await;
    let i = ids();
    let first = run_batch(&pool, time.as_ref(), &i, &client, 25)
        .await
        .expect("schedule backoff");
    assert_eq!(first.processed, 1);
    assert_eq!(first.retry_pending, 1);
    assert_eq!(first.needs_review, 0);
    let idle = run_batch(&pool, time.as_ref(), &i, &client, 25)
        .await
        .expect("unchanged backoff");
    assert_eq!(idle.processed, 0);
    assert_eq!(idle.retry_pending, 1);
    assert_eq!(idle.needs_review, 0);
    sqlx::query("UPDATE payment_reconciliation_jobs SET phase='needs_review',review_reason='custody_invalid' WHERE attempt_id=$1").bind(&f.snapshot().binding.id).execute(&pool).await.expect("explicit operator-visible stop");
    for _ in 0..2 {
        let stopped = run_batch(&pool, time.as_ref(), &i, &client, 25)
            .await
            .expect("unchanged review remains visible");
        assert_eq!(stopped.processed, 0);
        assert_eq!(stopped.needs_review, 1);
        assert_eq!(stopped.retry_pending, 0);
    }
    assert_eq!(
        jobs::outstanding(&pool, &other.snapshot().binding.profile.id)
            .await
            .expect("separate scope"),
        (1, 0)
    );
    assert_eq!(server.count(), 2);
    get_only(&server);
}
