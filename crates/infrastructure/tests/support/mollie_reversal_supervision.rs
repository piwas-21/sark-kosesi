//! Isolated PostgreSQL and loopback tests for bounded reversal-page supervision.
use super::*;
use crate::payment_attempts::{
    observations::record_verified,
    reversal_supervision::{EnrollIntent, RecoveryError, enroll, inspect, read_job, run_batch},
};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use tracing::instrument::WithSubscriber;

const PAYMENT: &str = "tr_SYNTHETIC_NotARealPayment";
#[path = "mollie_reversal_supervision_enrollment.rs"]
mod enrollment_cases;
#[path = "mollie_reversal_worker_entry.rs"]
pub(crate) mod worker_entry;

pub(crate) async fn fixture(
    pool: &PgPool,
) -> (
    Arc<crate::payment_test_fixture::quote::Time>,
    FrozenAttempt,
    crate::payment_test_fixture::Ids,
    String,
) {
    let (time, frozen) = observation_cases::prepared(pool, true).await;
    let ids = observation_cases::ids();
    let paid = observation_cases::fetched(frozen.snapshot(), PAYMENT, "paid", false).await;
    let observation = record_verified(
        pool,
        time.as_ref(),
        &ids,
        &frozen.snapshot().binding.id,
        "paid-source",
        &paid,
    )
    .await
    .expect("paid source");
    (time, frozen, ids, observation.snapshot.id)
}
fn page(next: Option<&str>) -> Reply {
    let link = next.map_or(Value::Null, |from| {
        let mut url = url::Url::parse(&format!(
            "https://api.mollie.com/v2/payments/{PAYMENT}/refunds"
        ))
        .expect("fixed URL");
        url.query_pairs_mut()
            .append_pair("from", from)
            .append_pair("limit", "10");
        json!({"href":url.as_str()})
    });
    Reply::Json(
        200,
        json!({"count":0,"_embedded":{"refunds":[]},"_links":{"next":link}}),
    )
}

pub(crate) async fn verified_test_page(
    attempt: &AttemptSnapshot,
    cursor: Option<&ReversalCursor>,
    next: Option<&str>,
) -> ReversalPage {
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, payment(attempt)),
        page(next),
    ]);
    local(&server)
        .verify_profile()
        .await
        .expect("profile")
        .into_reconciliation()
        .reversal_page(attempt, PAYMENT, ReversalKind::Refund, cursor)
        .await
        .expect("authenticated page")
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn two_pages_use_exactly_two_gets_each_and_terminal_scan_stays_unresolved(pool: PgPool) {
    let (time, frozen) = observation_cases::prepared(&pool, true).await;
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, payment(frozen.snapshot())),
        page(Some("re_NEXT")),
        Reply::Json(200, payment(frozen.snapshot())),
        page(None),
    ]);
    let client = local(&server)
        .verify_profile()
        .await
        .expect("profile")
        .into_reconciliation();
    let ids = observation_cases::ids();
    let paid = observation_cases::fetched(frozen.snapshot(), PAYMENT, "paid", false).await;
    let observation = record_verified(
        &pool,
        time.as_ref(),
        &ids,
        &frozen.snapshot().binding.id,
        "paid-source",
        &paid,
    )
    .await
    .expect("paid");
    let job = enroll(
        &pool,
        time.as_ref(),
        &ids,
        &client,
        EnrollIntent {
            attempt_id: &frozen.snapshot().binding.id,
            observation_id: &observation.snapshot.id,
            kind: ReversalKind::Refund,
            enrollment_key: "scan",
        },
    )
    .await
    .expect("enrolled");
    assert!(
        run_batch(&pool, time.as_ref(), &ids, &client, 0)
            .await
            .is_err()
    );
    assert!(
        run_batch(&pool, time.as_ref(), &ids, &client, 26)
            .await
            .is_err()
    );
    assert!(
        run_batch(&pool, &observation_cases::BadPorts, &ids, &client, 1)
            .await
            .is_err()
    );
    assert_eq!(server.count(), 1, "invalid limits and clock issue no GET");
    run_batch(&pool, time.as_ref(), &ids, &client, 1)
        .await
        .expect("first page");
    assert_eq!(server.count(), 3);
    let rotated_key = TestCredential::capture(
        &format!("test_{}", "rotated".repeat(5)),
        "pfl_SYNTHETIC_NotARealProfile",
    )
    .expect("rotated key");
    let rotated_config = config(&rotated_key);
    let rotated_server = Server::start(vec![profile()]);
    let rotated = TestClient::local(
        rotated_key,
        rotated_config,
        rotated_server.base.clone(),
        Duration::from_secs(2),
    )
    .expect("loopback")
    .verify_profile()
    .await
    .expect("same profile")
    .into_reconciliation();
    assert_eq!(
        run_batch(&pool, time.as_ref(), &ids, &rotated, 1)
            .await
            .expect("rotated key cannot take original scan")
            .processed,
        0
    );
    assert_eq!(rotated_server.count(), 1);
    run_batch(&pool, time.as_ref(), &ids, &client, 1)
        .await
        .expect("second page");
    assert_eq!(server.count(), 5);
    let stopped = read_job(&pool, &job.id)
        .await
        .expect("valid custody")
        .expect("job");
    assert_eq!(stopped.state, "review");
    let pages: i64 =
        sqlx::query_scalar("SELECT count(*) FROM payment_reversal_pages WHERE attempt_id=$1")
            .bind(&job.attempt_id)
            .fetch_one(&pool)
            .await
            .expect("pages");
    assert_eq!(pages, 2);
    run_batch(&pool, time.as_ref(), &ids, &client, 25)
        .await
        .expect("no automatic restart");
    assert_eq!(server.count(), 5);
    server.inspect(|requests| {
        assert!(
            requests
                .iter()
                .all(|r| r.method == "GET" && r.body.is_empty())
        );
        assert_eq!(
            requests[4].path,
            format!("/v2/payments/{PAYMENT}/refunds?limit=10&from=re_NEXT")
        );
    });
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn concurrent_workers_do_not_duplicate_a_started_provider_read(pool: PgPool) {
    let (time, frozen) = observation_cases::prepared(&pool, true).await;
    let gate = Arc::new(AtomicBool::new(false));
    let server = Server::start(vec![
        profile(),
        Reply::GateJson(gate.clone(), payment(frozen.snapshot())),
        page(None),
    ]);
    let client = local(&server)
        .verify_profile()
        .await
        .expect("profile")
        .into_reconciliation();
    let ids = observation_cases::ids();
    let paid = observation_cases::fetched(frozen.snapshot(), PAYMENT, "paid", false).await;
    let observation = record_verified(
        &pool,
        time.as_ref(),
        &ids,
        &frozen.snapshot().binding.id,
        "paid-source",
        &paid,
    )
    .await
    .expect("paid");
    enroll(
        &pool,
        time.as_ref(),
        &ids,
        &client,
        EnrollIntent {
            attempt_id: &frozen.snapshot().binding.id,
            observation_id: &observation.snapshot.id,
            kind: ReversalKind::Refund,
            enrollment_key: "scan",
        },
    )
    .await
    .expect("enrolled");
    let one = run_batch(&pool, time.as_ref(), &ids, &client, 1);
    let two = async {
        tokio::time::timeout(Duration::from_secs(2), async {
            while server.count() < 2 {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .expect("first request entered");
        let result = run_batch(&pool, time.as_ref(), &ids, &client, 1).await;
        gate.store(true, Ordering::SeqCst);
        result
    };
    let (a, b) = tokio::join!(one, two);
    a.expect("worker one");
    b.expect("worker two");
    assert_eq!(server.count(), 3, "only one parent and one page request");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn late_page_is_durable_and_recovered_without_another_provider_read(pool: PgPool) {
    let (time, frozen) = observation_cases::prepared(&pool, true).await;
    let gate = Arc::new(AtomicBool::new(false));
    let Reply::Json(_, body) = page(None) else {
        panic!("page fixture")
    };
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, payment(frozen.snapshot())),
        Reply::GateJson(gate.clone(), body),
    ]);
    let client = local(&server)
        .verify_profile()
        .await
        .expect("profile")
        .into_reconciliation();
    let ids = observation_cases::ids();
    let paid = observation_cases::fetched(frozen.snapshot(), PAYMENT, "paid", false).await;
    let observation = record_verified(
        &pool,
        time.as_ref(),
        &ids,
        &frozen.snapshot().binding.id,
        "paid-source",
        &paid,
    )
    .await
    .expect("paid");
    let job = enroll(
        &pool,
        time.as_ref(),
        &ids,
        &client,
        EnrollIntent {
            attempt_id: &frozen.snapshot().binding.id,
            observation_id: &observation.snapshot.id,
            kind: ReversalKind::Refund,
            enrollment_key: "scan",
        },
    )
    .await
    .expect("enrolled");
    let first = run_batch(&pool, time.as_ref(), &ids, &client, 1);
    let expire = async {
        tokio::time::timeout(Duration::from_secs(2), async {
            while server.count() < 3 {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .expect("page request entered");
        time.0.store(1561, Ordering::SeqCst);
        gate.store(true, Ordering::SeqCst);
    };
    let (result, ()) = tokio::join!(first, expire);
    assert!(matches!(result, Err(RecoveryError::LeaseLost)));
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM payment_reversal_pages WHERE attempt_id=$1")
            .bind(&job.attempt_id)
            .fetch_one(&pool)
            .await
            .expect("pages");
    assert_eq!(count, 1, "late verified page remains durable");
    // A plain page read does not wait for the failed finish transaction's rollback.
    let mut fence = pool.begin().await.expect("late page rollback fence");
    sqlx::query("SELECT id FROM payment_attempts WHERE id=$1 FOR UPDATE")
        .bind(&job.attempt_id)
        .fetch_one(&mut *fence)
        .await
        .expect("failed finish released attempt");
    fence.rollback().await.expect("release recovery fence");
    assert_eq!(
        run_batch(&pool, time.as_ref(), &ids, &client, 1)
            .await
            .expect("recover receipt")
            .processed,
        1
    );
    let recovered = read_job(&pool, &job.id)
        .await
        .expect("custody")
        .expect("job");
    assert_eq!(recovered.state, "review");
    assert_eq!(recovered.review_reason.as_deref(), Some("ended"));
    assert_eq!(server.count(), 3, "recovery issues no GET");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn completed_unavailable_read_backs_off_but_malformed_response_stops_for_review(
    pool: PgPool,
) {
    let (time, frozen) = observation_cases::prepared(&pool, true).await;
    let server = Server::start(vec![
        profile(),
        Reply::Json(503, json!({})),
        Reply::Json(200, payment(frozen.snapshot())),
        Reply::Json(200, json!({})),
    ]);
    let client = local(&server)
        .verify_profile()
        .await
        .expect("profile")
        .into_reconciliation();
    let ids = observation_cases::ids();
    let paid = observation_cases::fetched(frozen.snapshot(), PAYMENT, "paid", false).await;
    let observation = record_verified(
        &pool,
        time.as_ref(),
        &ids,
        &frozen.snapshot().binding.id,
        "paid-source",
        &paid,
    )
    .await
    .expect("paid");
    let job = enroll(
        &pool,
        time.as_ref(),
        &ids,
        &client,
        EnrollIntent {
            attempt_id: &frozen.snapshot().binding.id,
            observation_id: &observation.snapshot.id,
            kind: ReversalKind::Refund,
            enrollment_key: "scan",
        },
    )
    .await
    .expect("enrolled");
    run_batch(&pool, time.as_ref(), &ids, &client, 25)
        .await
        .expect("unavailable read");
    let pending = read_job(&pool, &job.id)
        .await
        .expect("custody")
        .expect("job");
    assert_eq!(pending.state, "pending");
    assert_eq!(pending.failures, 1);
    assert_eq!(server.count(), 2);
    time.0.store(pending.due_at as u64, Ordering::SeqCst);
    run_batch(&pool, time.as_ref(), &ids, &client, 25)
        .await
        .expect("malformed read");
    let stopped = read_job(&pool, &job.id)
        .await
        .expect("custody")
        .expect("job");
    assert_eq!(stopped.review_reason.as_deref(), Some("invalid"));
    assert_eq!(server.count(), 4);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn cancelled_inflight_start_parks_after_expiry_without_repeat(pool: PgPool) {
    let (time, frozen) = observation_cases::prepared(&pool, true).await;
    let gate = Arc::new(AtomicBool::new(false));
    let server = Server::start(vec![
        profile(),
        Reply::GateJson(gate.clone(), payment(frozen.snapshot())),
        page(None),
    ]);
    let client = local(&server)
        .verify_profile()
        .await
        .expect("profile")
        .into_reconciliation();
    let ids = observation_cases::ids();
    let paid = observation_cases::fetched(frozen.snapshot(), PAYMENT, "paid", false).await;
    let observation = record_verified(
        &pool,
        time.as_ref(),
        &ids,
        &frozen.snapshot().binding.id,
        "paid-source",
        &paid,
    )
    .await
    .expect("paid");
    let job = enroll(
        &pool,
        time.as_ref(),
        &ids,
        &client,
        EnrollIntent {
            attempt_id: &frozen.snapshot().binding.id,
            observation_id: &observation.snapshot.id,
            kind: ReversalKind::Refund,
            enrollment_key: "scan",
        },
    )
    .await
    .expect("enrolled");
    {
        let mut started = Box::pin(run_batch(&pool, time.as_ref(), &ids, &client, 1));
        tokio::select! {
            result = &mut started => panic!("request unexpectedly completed: {}", result.is_ok()),
            () = async { tokio::time::timeout(Duration::from_secs(2), async {
                while server.count() < 2 { tokio::time::sleep(Duration::from_millis(2)).await; }
            }).await.expect("parent request entered"); } => {}
        }
    }
    gate.store(true, Ordering::SeqCst);
    time.0.store(1561, Ordering::SeqCst);
    run_batch(&pool, time.as_ref(), &ids, &client, 25)
        .await
        .expect("park uncertain start");
    let stopped = read_job(&pool, &job.id)
        .await
        .expect("custody")
        .expect("job");
    assert_eq!(stopped.review_reason.as_deref(), Some("uncertain"));
    assert_eq!(
        server.count(),
        2,
        "recovery never repeats an uncertain read"
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn three_completed_unavailable_reads_exhaust_the_retry_budget(pool: PgPool) {
    let (time, frozen, ids, observation_id) = fixture(&pool).await;
    let server = Server::start(vec![
        profile(),
        Reply::Json(503, json!({})),
        Reply::Json(503, json!({})),
        Reply::Json(503, json!({})),
    ]);
    let client = local(&server)
        .verify_profile()
        .await
        .expect("profile")
        .into_reconciliation();
    let job = enroll(
        &pool,
        time.as_ref(),
        &ids,
        &client,
        EnrollIntent {
            attempt_id: &frozen.snapshot().binding.id,
            observation_id: &observation_id,
            kind: ReversalKind::Refund,
            enrollment_key: "scan",
        },
    )
    .await
    .expect("enroll");
    for expected in 1..=3 {
        run_batch(&pool, time.as_ref(), &ids, &client, 25)
            .await
            .expect("completed read");
        let current = read_job(&pool, &job.id)
            .await
            .expect("custody")
            .expect("job");
        assert_eq!(current.failures, expected);
        if expected < 3 {
            assert_eq!(current.state, "pending");
            time.0.store(current.due_at as u64, Ordering::SeqCst);
        } else {
            assert_eq!(current.review_reason.as_deref(), Some("retry_exhausted"));
        }
    }
    run_batch(&pool, time.as_ref(), &ids, &client, 25)
        .await
        .expect("review remains stopped");
    assert_eq!(server.count(), 4);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn expired_inflight_start_parks_first_and_late_page_remains_durable(pool: PgPool) {
    let (time, frozen, ids, observation_id) = fixture(&pool).await;
    let gate = Arc::new(AtomicBool::new(false));
    let Reply::Json(_, body) = page(None) else {
        panic!("page fixture")
    };
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, payment(frozen.snapshot())),
        page(Some("re_NEXT")),
        Reply::Json(200, payment(frozen.snapshot())),
        Reply::GateJson(gate.clone(), body),
    ]);
    let client = local(&server)
        .verify_profile()
        .await
        .expect("profile")
        .into_reconciliation();
    let job = enroll(
        &pool,
        time.as_ref(),
        &ids,
        &client,
        EnrollIntent {
            attempt_id: &frozen.snapshot().binding.id,
            observation_id: &observation_id,
            kind: ReversalKind::Refund,
            enrollment_key: "scan",
        },
    )
    .await
    .expect("enroll");
    assert!(matches!(
        inspect(&pool, "bad").await,
        Err(RecoveryError::InvalidRequest)
    ));
    assert!(
        inspect(&pool, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
            .await
            .expect("absent")
            .is_none()
    );
    assert_eq!(
        inspect(&pool, &job.id)
            .await
            .expect("pending")
            .expect("job")
            .late_page_present,
        None
    );
    run_batch(&pool, time.as_ref(), &ids, &client, 1)
        .await
        .expect("record predecessor");
    let first = run_batch(&pool, time.as_ref(), &ids, &client, 1);
    let logs = Arc::new(Mutex::new(Vec::new()));
    let recover = async {
        tokio::time::timeout(Duration::from_secs(2), async {
            while server.count() < 5 {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .expect("page request entered");
        time.0.store(1561, Ordering::SeqCst);
        run_batch(&pool, time.as_ref(), &ids, &client, 1)
            .await
            .expect("park uncertainty");
        let stopped = read_job(&pool, &job.id)
            .await
            .expect("custody")
            .expect("job");
        assert_eq!(stopped.review_reason.as_deref(), Some("uncertain"));
        let before: (i64, i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM reversal_scan_operations),(SELECT count(*) FROM payment_reversal_pages),(SELECT count(*) FROM commerce_outbox)")
            .fetch_one(&pool).await.expect("before inspection");
        let inspection = inspect(&pool, &job.id)
            .await
            .expect("uncertain")
            .expect("job");
        assert_eq!(inspection.late_page_present, Some(false));
        assert!(inspection.job == stopped);
        assert_eq!(stopped.generation, 2);
        assert!(stopped.tip_page_id.is_some());
        // The existing predecessor is equal to the operation start in this
        // fixture. Moving it one second forward remains before the stop but
        // makes it impossible for that operation to have used it.
        pool.execute("ALTER TABLE payment_reversal_pages DISABLE TRIGGER USER; ALTER TABLE commerce_outbox DISABLE TRIGGER USER")
            .await.expect("isolate predecessor control");
        sqlx::query("UPDATE payment_reversal_pages p SET recorded_at=o.started_at+1 FROM reversal_scan_operations o JOIN reversal_scan_jobs j ON j.id=o.job_id WHERE o.job_id=$1 AND o.generation=2 AND p.id=j.tip_page_id")
            .bind(&job.id).execute(&pool).await.expect("late predecessor");
        sqlx::query("UPDATE commerce_outbox e SET created_at=p.recorded_at,payload=payment_reversal_page_event(p) FROM payment_reversal_pages p JOIN reversal_scan_jobs j ON p.id=j.tip_page_id WHERE j.id=$1 AND e.id=p.event_id")
            .bind(&job.id).execute(&pool).await.expect("rebind predecessor event");
        assert!(matches!(
            inspect(&pool, &job.id).await,
            Err(RecoveryError::InvalidStoredData)
        ));
        sqlx::query("UPDATE payment_reversal_pages p SET recorded_at=o.started_at FROM reversal_scan_operations o JOIN reversal_scan_jobs j ON j.id=o.job_id WHERE o.job_id=$1 AND o.generation=2 AND p.id=j.tip_page_id")
            .bind(&job.id).execute(&pool).await.expect("predecessor at start");
        sqlx::query("UPDATE commerce_outbox e SET created_at=p.recorded_at,payload=payment_reversal_page_event(p) FROM payment_reversal_pages p JOIN reversal_scan_jobs j ON p.id=j.tip_page_id WHERE j.id=$1 AND e.id=p.event_id")
            .bind(&job.id).execute(&pool).await.expect("restore predecessor event");
        assert_eq!(
            inspect(&pool, &job.id)
                .await
                .expect("equal predecessor time")
                .expect("job")
                .late_page_present,
            Some(false)
        );
        pool.execute("ALTER TABLE payment_reversal_pages ENABLE TRIGGER USER; ALTER TABLE commerce_outbox ENABLE TRIGGER USER; ALTER TABLE reversal_scan_jobs DISABLE TRIGGER USER")
            .await.expect("isolate failure count controls");
        for failures in [2, 3] {
            sqlx::query("UPDATE reversal_scan_jobs SET failures=$2 WHERE id=$1")
                .bind(&job.id)
                .bind(failures)
                .execute(&pool)
                .await
                .expect("corrupt failure count");
            assert!(matches!(
                inspect(&pool, &job.id).await,
                Err(RecoveryError::InvalidStoredData)
            ));
        }
        sqlx::query("UPDATE reversal_scan_jobs SET failures=0 WHERE id=$1")
            .bind(&job.id)
            .execute(&pool)
            .await
            .expect("restore failures");
        pool.execute("ALTER TABLE reversal_scan_jobs ENABLE TRIGGER USER")
            .await
            .expect("restore job guard");
        crate::mollie::reversal_worker_fixture::inspect(&pool, &job.id)
            .with_subscriber(worker_entry::Capture(logs.clone()))
            .await
            .expect("redacted absent output");
        let after: (i64, i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM reversal_scan_operations),(SELECT count(*) FROM payment_reversal_pages),(SELECT count(*) FROM commerce_outbox)")
            .fetch_one(&pool).await.expect("after inspection");
        assert_eq!(before, after);
        assert_eq!(server.count(), 5, "inspection does not contact provider");
        gate.store(true, Ordering::SeqCst);
    };
    let (result, ()) = tokio::join!(first, recover);
    assert!(result.is_err(), "recovered fence cannot advance again");
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM payment_reversal_pages WHERE attempt_id=$1")
            .bind(&job.attempt_id)
            .fetch_one(&pool)
            .await
            .expect("page evidence");
    assert_eq!(count, 2);
    let stopped = read_job(&pool, &job.id)
        .await
        .expect("custody")
        .expect("job");
    assert_eq!(stopped.review_reason.as_deref(), Some("uncertain"));
    assert_eq!(server.count(), 5);
    let inspection = inspect(&pool, &job.id)
        .await
        .expect("late page")
        .expect("job");
    assert_eq!(inspection.late_page_present, Some(true));
    assert!(
        inspection.job == stopped,
        "inspection cannot advance checkpoint"
    );
    crate::mollie::reversal_worker_fixture::inspect(&pool, &job.id)
        .with_subscriber(worker_entry::Capture(logs.clone()))
        .await
        .expect("redacted present output");
    {
        let events = logs.lock().expect("captured output");
        for expected in ["absent", "present"] {
            assert!(events.iter().any(|event| {
                event
                    .iter()
                    .any(|(key, value)| key == "late_page" && value == expected)
            }));
        }
        for event in events.iter() {
            assert_eq!(event.len(), 5, "only the redacted scan state is emitted");
            for (_, value) in event {
                assert!(!value.contains(PAYMENT));
                assert!(!value.contains(&job.id));
                assert!(!value.contains(&job.credential_fingerprint));
            }
        }
    }
    assert_eq!(server.count(), 5, "inspection does not contact provider");
    let mut lock = pool.begin().await.expect("attempt lock");
    sqlx::query("SELECT id FROM payment_attempts WHERE id=$1 FOR UPDATE")
        .bind(&job.attempt_id)
        .fetch_one(&mut *lock)
        .await
        .expect("claim attempt");
    let reader_pool = pool.clone();
    let reader_id = job.id.clone();
    let reader = tokio::spawn(async move { inspect(&reader_pool, &reader_id).await });
    tokio::time::timeout(Duration::from_millis(800), async {
        loop {
            let waiting: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'SELECT left(basket_id,33) FROM payment_attempts WHERE id=%FOR UPDATE'")
                .fetch_one(&pool).await.expect("read lock wait");
            if waiting > 0 { break; }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }).await.expect("inspection waits for the attempt claim");
    lock.rollback().await.expect("release attempt");
    assert_eq!(
        reader
            .await
            .expect("reader task")
            .expect("reader result")
            .expect("job")
            .late_page_present,
        Some(true)
    );

    // The page source clock may precede the stop's finish while still meeting
    // the original operation's start boundary. Rebind the synthetic event.
    pool.execute("ALTER TABLE payment_reversal_pages DISABLE TRIGGER USER; ALTER TABLE commerce_outbox DISABLE TRIGGER USER")
        .await.expect("isolate source clock controls");
    sqlx::query("UPDATE payment_reversal_pages p SET recorded_at=o.started_at FROM reversal_scan_operations o WHERE o.job_id=$1 AND o.generation=2 AND p.effect_key=o.id")
        .bind(&job.id).execute(&pool).await.expect("at operation start");
    sqlx::query("UPDATE commerce_outbox e SET created_at=p.recorded_at,payload=payment_reversal_page_event(p) FROM payment_reversal_pages p JOIN reversal_scan_operations o ON p.effect_key=o.id WHERE o.job_id=$1 AND o.generation=2 AND e.id=p.event_id")
        .bind(&job.id).execute(&pool).await.expect("rebind event");
    assert_eq!(
        inspect(&pool, &job.id)
            .await
            .expect("start time is valid")
            .expect("job")
            .late_page_present,
        Some(true)
    );
    sqlx::query("UPDATE payment_reversal_pages p SET recorded_at=o.started_at-1 FROM reversal_scan_operations o WHERE o.job_id=$1 AND o.generation=2 AND p.effect_key=o.id")
        .bind(&job.id).execute(&pool).await.expect("before operation start");
    sqlx::query("UPDATE commerce_outbox e SET created_at=p.recorded_at,payload=payment_reversal_page_event(p) FROM payment_reversal_pages p JOIN reversal_scan_operations o ON p.effect_key=o.id WHERE o.job_id=$1 AND o.generation=2 AND e.id=p.event_id")
        .bind(&job.id).execute(&pool).await.expect("rebind earlier event");
    assert!(matches!(
        inspect(&pool, &job.id).await,
        Err(RecoveryError::InvalidStoredData)
    ));
    sqlx::query("UPDATE payment_reversal_pages p SET recorded_at=o.started_at FROM reversal_scan_operations o WHERE o.job_id=$1 AND o.generation=2 AND p.effect_key=o.id")
        .bind(&job.id).execute(&pool).await.expect("restore page");
    sqlx::query("UPDATE commerce_outbox e SET created_at=p.recorded_at,payload=payment_reversal_page_event(p) FROM payment_reversal_pages p JOIN reversal_scan_operations o ON p.effect_key=o.id WHERE o.job_id=$1 AND o.generation=2 AND e.id=p.event_id")
        .bind(&job.id).execute(&pool).await.expect("restore event");
    sqlx::query("UPDATE commerce_outbox e SET payload='{}'::jsonb FROM payment_reversal_pages p JOIN reversal_scan_operations o ON p.effect_key=o.id WHERE o.job_id=$1 AND o.generation=2 AND e.id=p.event_id")
        .bind(&job.id).execute(&pool).await.expect("damage event");
    assert!(matches!(
        inspect(&pool, &job.id).await,
        Err(RecoveryError::InvalidStoredData)
    ));
    sqlx::query("UPDATE commerce_outbox e SET payload=payment_reversal_page_event(p) FROM payment_reversal_pages p JOIN reversal_scan_operations o ON p.effect_key=o.id WHERE o.job_id=$1 AND o.generation=2 AND e.id=p.event_id")
        .bind(&job.id).execute(&pool).await.expect("restore event");
    sqlx::query("UPDATE payment_reversal_pages p SET credential_fingerprint=repeat('a',64) FROM reversal_scan_operations o WHERE o.job_id=$1 AND o.generation=2 AND p.effect_key=o.id")
        .bind(&job.id).execute(&pool).await.expect("wrong credential");
    sqlx::query("UPDATE commerce_outbox e SET payload=payment_reversal_page_event(p) FROM payment_reversal_pages p JOIN reversal_scan_operations o ON p.effect_key=o.id WHERE o.job_id=$1 AND o.generation=2 AND e.id=p.event_id")
        .bind(&job.id).execute(&pool).await.expect("rebind wrong credential");
    assert!(matches!(
        inspect(&pool, &job.id).await,
        Err(RecoveryError::InvalidStoredData)
    ));
    sqlx::query("UPDATE payment_reversal_pages p SET credential_fingerprint=j.credential_fingerprint FROM reversal_scan_operations o JOIN reversal_scan_jobs j ON j.id=o.job_id WHERE o.job_id=$1 AND o.generation=2 AND p.effect_key=o.id")
        .bind(&job.id).execute(&pool).await.expect("restore credential");
    sqlx::query("UPDATE commerce_outbox e SET payload=payment_reversal_page_event(p) FROM payment_reversal_pages p JOIN reversal_scan_operations o ON p.effect_key=o.id WHERE o.job_id=$1 AND o.generation=2 AND e.id=p.event_id")
        .bind(&job.id).execute(&pool).await.expect("restore event");
    pool.execute("ALTER TABLE payment_reversal_pages ENABLE TRIGGER USER; ALTER TABLE commerce_outbox ENABLE TRIGGER USER; ALTER TABLE reversal_scan_operations DISABLE TRIGGER USER")
        .await.expect("isolate operation control");
    sqlx::query(
        "UPDATE reversal_scan_operations SET outcome='invalid' WHERE job_id=$1 AND generation=2",
    )
    .bind(&job.id)
    .execute(&pool)
    .await
    .expect("wrong operation outcome");
    assert!(matches!(
        inspect(&pool, &job.id).await,
        Err(RecoveryError::InvalidStoredData)
    ));
    sqlx::query(
        "UPDATE reversal_scan_operations SET outcome='uncertain' WHERE job_id=$1 AND generation=2",
    )
    .bind(&job.id)
    .execute(&pool)
    .await
    .expect("restore operation outcome");
    sqlx::query(
        "UPDATE reversal_scan_operations SET finish_xid=start_xid WHERE job_id=$1 AND generation=2",
    )
    .bind(&job.id)
    .execute(&pool)
    .await
    .expect("damage transaction proof");
    assert!(matches!(
        inspect(&pool, &job.id).await,
        Err(RecoveryError::InvalidStoredData)
    ));
    assert_eq!(
        server.count(),
        3,
        "corrupt custody cannot trigger a provider call"
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn attempt_page_capacity_parks_without_a_provider_request(pool: PgPool) {
    let (time, frozen, ids, observation_id) = fixture(&pool).await;
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, payment(frozen.snapshot())),
        page(None),
    ]);
    let client = local(&server)
        .verify_profile()
        .await
        .expect("profile")
        .into_reconciliation();
    let job = enroll(
        &pool,
        time.as_ref(),
        &ids,
        &client,
        EnrollIntent {
            attempt_id: &frozen.snapshot().binding.id,
            observation_id: &observation_id,
            kind: ReversalKind::Refund,
            enrollment_key: "scan",
        },
    )
    .await
    .expect("enroll");
    let verified = client
        .reversal_page(frozen.snapshot(), PAYMENT, ReversalKind::Refund, None)
        .await
        .expect("authenticated page fixture");
    for n in 0..128 {
        crate::payment_attempts::reversal_pages::record(
            &pool,
            time.as_ref(),
            &ids,
            &format!("capacity-{n}"),
            None,
            &verified,
        )
        .await
        .expect("bounded historical root");
    }
    assert_eq!(server.count(), 3);
    run_batch(&pool, time.as_ref(), &ids, &client, 1)
        .await
        .expect("cap review");
    let stopped = read_job(&pool, &job.id)
        .await
        .expect("custody")
        .expect("job");
    assert_eq!(stopped.review_reason.as_deref(), Some("scan_limit"));
    assert_eq!(
        server.count(),
        3,
        "capacity is checked before any provider I/O"
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn damaged_open_tip_is_reviewed_before_any_further_provider_request(pool: PgPool) {
    let (time, frozen, ids, observation_id) = fixture(&pool).await;
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, payment(frozen.snapshot())),
        page(Some("re_NEXT")),
    ]);
    let client = local(&server)
        .verify_profile()
        .await
        .expect("profile")
        .into_reconciliation();
    let job = enroll(
        &pool,
        time.as_ref(),
        &ids,
        &client,
        EnrollIntent {
            attempt_id: &frozen.snapshot().binding.id,
            observation_id: &observation_id,
            kind: ReversalKind::Refund,
            enrollment_key: "scan",
        },
    )
    .await
    .expect("enroll");
    run_batch(&pool, time.as_ref(), &ids, &client, 1)
        .await
        .expect("open root");
    let open = read_job(&pool, &job.id)
        .await
        .expect("custody")
        .expect("job");
    let tip = open.tip_page_id.expect("open tip");
    use sqlx::Executor;
    pool.execute("ALTER TABLE payment_reversal_pages DISABLE TRIGGER USER")
        .await
        .expect("fault injection");
    sqlx::query("UPDATE payment_reversal_pages SET candidates=ARRAY['re_FORGED'] WHERE id=$1")
        .bind(&tip)
        .execute(&pool)
        .await
        .expect("tamper");
    pool.execute("ALTER TABLE payment_reversal_pages ENABLE TRIGGER USER")
        .await
        .expect("restore guards");
    run_batch(&pool, time.as_ref(), &ids, &client, 1)
        .await
        .expect("invalid tip review");
    assert!(matches!(
        read_job(&pool, &job.id).await,
        Err(crate::payment_attempts::reversal_supervision::RecoveryError::InvalidStoredData)
    ));
    let state: (String, Option<String>) =
        sqlx::query_as("SELECT state, review_reason FROM reversal_scan_jobs WHERE id=$1")
            .bind(&job.id)
            .fetch_one(&pool)
            .await
            .expect("review checkpoint");
    assert_eq!(state, ("review".into(), Some("invalid".into())));
    assert_eq!(
        server.count(),
        3,
        "damaged tip cannot authorize another GET"
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn failed_page_write_below_capacity_stops_as_invalid(pool: PgPool) {
    use sqlx::Executor;
    let (time, frozen, ids, observation_id) = fixture(&pool).await;
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, payment(frozen.snapshot())),
        page(None),
    ]);
    let client = local(&server)
        .verify_profile()
        .await
        .expect("profile")
        .into_reconciliation();
    let job = enroll(
        &pool,
        time.as_ref(),
        &ids,
        &client,
        EnrollIntent {
            attempt_id: &frozen.snapshot().binding.id,
            observation_id: &observation_id,
            kind: ReversalKind::Refund,
            enrollment_key: "scan",
        },
    )
    .await
    .expect("enroll");
    pool.execute("CREATE FUNCTION suppress_supervised_page() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RETURN NULL; END $$").await.expect("fault function");
    pool.execute("CREATE TRIGGER suppress_supervised_page BEFORE INSERT ON payment_reversal_pages FOR EACH ROW EXECUTE FUNCTION suppress_supervised_page()").await.expect("inject fault");
    run_batch(&pool, time.as_ref(), &ids, &client, 1)
        .await
        .expect("custody failure reviewed");
    pool.execute("DROP TRIGGER suppress_supervised_page ON payment_reversal_pages")
        .await
        .expect("remove fault");
    let stopped = read_job(&pool, &job.id)
        .await
        .expect("custody")
        .expect("job");
    assert_eq!(stopped.review_reason.as_deref(), Some("invalid"));
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM payment_reversal_pages WHERE attempt_id=$1")
            .bind(&job.attempt_id)
            .fetch_one(&pool)
            .await
            .expect("page count");
    assert_eq!(count, 0);
    assert_eq!(server.count(), 3);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn attempt_capacity_reached_during_io_stops_as_capped(pool: PgPool) {
    let (time, frozen, ids, observation_id) = fixture(&pool).await;
    let gate = Arc::new(AtomicBool::new(false));
    let Reply::Json(_, body) = page(None) else {
        panic!("page fixture")
    };
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, payment(frozen.snapshot())),
        page(None),
        Reply::Json(200, payment(frozen.snapshot())),
        Reply::GateJson(gate.clone(), body),
    ]);
    let client = local(&server)
        .verify_profile()
        .await
        .expect("profile")
        .into_reconciliation();
    let job = enroll(
        &pool,
        time.as_ref(),
        &ids,
        &client,
        EnrollIntent {
            attempt_id: &frozen.snapshot().binding.id,
            observation_id: &observation_id,
            kind: ReversalKind::Refund,
            enrollment_key: "scan",
        },
    )
    .await
    .expect("enroll");
    let seed = client
        .reversal_page(frozen.snapshot(), PAYMENT, ReversalKind::Refund, None)
        .await
        .expect("authenticated page fixture");
    for n in 0..127 {
        crate::payment_attempts::reversal_pages::record(
            &pool,
            time.as_ref(),
            &ids,
            &format!("capacity-race-{n}"),
            None,
            &seed,
        )
        .await
        .expect("historical root");
    }
    let run = run_batch(&pool, time.as_ref(), &ids, &client, 1);
    let compete = async {
        tokio::time::timeout(Duration::from_secs(2), async {
            while server.count() < 5 {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .expect("page request entered");
        crate::payment_attempts::reversal_pages::record(
            &pool,
            time.as_ref(),
            &ids,
            "capacity-race-last",
            None,
            &seed,
        )
        .await
        .expect("other scan fills final slot");
        gate.store(true, Ordering::SeqCst);
    };
    let (result, ()) = tokio::join!(run, compete);
    result.expect("cap checkpoint");
    let stopped = read_job(&pool, &job.id)
        .await
        .expect("custody")
        .expect("job");
    assert_eq!(stopped.review_reason.as_deref(), Some("scan_limit"));
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM payment_reversal_pages WHERE attempt_id=$1")
            .bind(&job.attempt_id)
            .fetch_one(&pool)
            .await
            .expect("page count");
    assert_eq!(count, 128);
    assert_eq!(server.count(), 5);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn externally_followed_tip_stops_without_another_provider_request(pool: PgPool) {
    let (time, frozen, ids, observation_id) = fixture(&pool).await;
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, payment(frozen.snapshot())),
        page(Some("re_NEXT")),
        Reply::Json(200, payment(frozen.snapshot())),
        page(None),
    ]);
    let client = local(&server)
        .verify_profile()
        .await
        .expect("profile")
        .into_reconciliation();
    let job = enroll(
        &pool,
        time.as_ref(),
        &ids,
        &client,
        EnrollIntent {
            attempt_id: &frozen.snapshot().binding.id,
            observation_id: &observation_id,
            kind: ReversalKind::Refund,
            enrollment_key: "scan",
        },
    )
    .await
    .expect("enroll");
    run_batch(&pool, time.as_ref(), &ids, &client, 1)
        .await
        .expect("open root");
    let open = read_job(&pool, &job.id)
        .await
        .expect("custody")
        .expect("job");
    let tip = open.tip_page_id.expect("open tip");
    let cursor = crate::payment_attempts::reversal_pages::resume(&pool, &job.attempt_id, &tip)
        .await
        .expect("tip custody")
        .expect("unfollowed");
    let external = client
        .reversal_page(
            frozen.snapshot(),
            PAYMENT,
            ReversalKind::Refund,
            Some(&cursor),
        )
        .await
        .expect("separate authenticated successor");
    crate::payment_attempts::reversal_pages::record(
        &pool,
        time.as_ref(),
        &ids,
        "external-successor",
        Some(&tip),
        &external,
    )
    .await
    .expect("successor");
    assert_eq!(server.count(), 5);
    run_batch(&pool, time.as_ref(), &ids, &client, 1)
        .await
        .expect("followed tip reviewed");
    let stopped = read_job(&pool, &job.id)
        .await
        .expect("valid historical tip")
        .expect("job");
    assert_eq!(stopped.review_reason.as_deref(), Some("invalid"));
    assert_eq!(
        server.count(),
        5,
        "followed tip cannot authorize a duplicate read"
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn lease_expiring_before_provider_io_never_sends_a_request(pool: PgPool) {
    use commerce_application::request_context::{Clock, ContextError};
    use std::sync::atomic::AtomicU64;
    struct ExpiringClock(AtomicU64);
    impl Clock for ExpiringClock {
        fn unix_seconds(&self) -> Result<u64, ContextError> {
            let call = self.0.fetch_add(1, Ordering::SeqCst);
            Ok(if call < 3 { 1500 } else { 1560 })
        }
    }
    let (time, frozen, ids, observation_id) = fixture(&pool).await;
    let server = Server::start(vec![profile()]);
    let client = local(&server)
        .verify_profile()
        .await
        .expect("profile")
        .into_reconciliation();
    let job = enroll(
        &pool,
        time.as_ref(),
        &ids,
        &client,
        EnrollIntent {
            attempt_id: &frozen.snapshot().binding.id,
            observation_id: &observation_id,
            kind: ReversalKind::Refund,
            enrollment_key: "scan",
        },
    )
    .await
    .expect("enroll");
    let clock = ExpiringClock(AtomicU64::new(0));
    assert!(matches!(
        run_batch(&pool, &clock, &ids, &client, 1).await,
        Err(crate::payment_attempts::reversal_supervision::RecoveryError::LeaseLost)
    ));
    assert_eq!(
        server.count(),
        1,
        "expired authorization cannot send parent GET"
    );
    let leased = read_job(&pool, &job.id)
        .await
        .expect("custody")
        .expect("job");
    assert_eq!(leased.state, "leased");
    time.0.store(1560, Ordering::SeqCst);
    run_batch(&pool, time.as_ref(), &ids, &client, 1)
        .await
        .expect("expired start reviewed");
    let stopped = read_job(&pool, &job.id)
        .await
        .expect("custody")
        .expect("job");
    assert_eq!(stopped.review_reason.as_deref(), Some("uncertain"));
    assert_eq!(server.count(), 1);
}
