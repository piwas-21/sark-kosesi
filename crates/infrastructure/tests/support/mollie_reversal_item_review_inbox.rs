//! Real isolated custody, ordering and authority for the bounded review inbox.
use super::*;
use crate::{
    payment_attempts::reversal_item_supervision::{
        inbox,
        review::{self, Reason, ReviewError, Summary},
    },
    staff_access::SessionToken,
};

struct ExpiringClock(std::sync::atomic::AtomicUsize);
impl commerce_application::request_context::Clock for ExpiringClock {
    fn unix_seconds(&self) -> Result<u64, commerce_application::request_context::ContextError> {
        Ok(if self.0.fetch_add(1, Ordering::SeqCst) == 0 {
            1500
        } else {
            2000
        })
    }
}

async fn add_review(
    pool: &PgPool,
    time: &Arc<crate::payment_test_fixture::quote::Time>,
    frozen: &FrozenAttempt,
    ids: &impl commerce_application::request_context::Identifiers,
    paid: &str,
    spec: (ReversalKind, &str, &str),
) -> (
    crate::payment_attempts::reversal_item_supervision::Job,
    Server,
) {
    let (kind, effect, payment_id) = spec;
    let child_id = format!(
        "{}{}",
        if kind == ReversalKind::Refund {
            "re_"
        } else {
            "chb_"
        },
        effect
    );
    let item = match kind {
        ReversalKind::Refund => {
            json!({"resource":"refund","id":child_id,"paymentId":payment_id,"mode":"test"})
        }
        ReversalKind::Chargeback => {
            json!({"resource":"chargeback","id":child_id,"paymentId":payment_id})
        }
    };
    let page_body =
        json!({"count":1,"_embedded":{(collection(kind)):[item]},"_links":{"next":null}});
    let mut parent = payment(frozen.snapshot());
    parent["id"] = payment_id.into();
    let page_server = Server::start(vec![
        profile(),
        Reply::Json(200, parent.clone()),
        Reply::Json(200, page_body),
    ]);
    let page_client = local(&page_server)
        .verify_profile()
        .await
        .expect("page profile")
        .into_reconciliation();
    let verified = page_client
        .reversal_page(frozen.snapshot(), payment_id, kind, None)
        .await
        .expect("verified page");
    let page = reversal_pages::record(pool, time.as_ref(), ids, effect, None, &verified)
        .await
        .expect("page custody")
        .id;
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, parent),
        Reply::Json(200, json!({})),
    ]);
    let client = local(&server)
        .verify_profile()
        .await
        .expect("profile")
        .into_reconciliation();
    let job = enroll(
        pool,
        time.as_ref(),
        ids,
        &client,
        EnrollIntent {
            attempt_id: &frozen.snapshot().binding.id,
            observation_id: paid,
            page_id: &page,
            ordinal: 1,
            enrollment_key: effect,
        },
    )
    .await
    .expect("enroll");
    run_batch(pool, time.as_ref(), ids, &client, 1)
        .await
        .expect("one stopped item");
    let stopped = read_job(pool, &job.id)
        .await
        .expect("valid job")
        .expect("present");
    assert_eq!(
        (stopped.state.as_str(), stopped.review_reason.as_deref()),
        ("review", Some("invalid"))
    );
    (stopped, server)
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn inbox_pages_exact_equal_time_stops_without_effects_and_fails_closed(pool: PgPool) {
    let (time, frozen, ids, paid) = reversal_supervision_cases::fixture(&pool).await;
    staff_review_cases::staff(&pool).await;
    let token = SessionToken::from_bytes([11; 32]);
    assert_eq!(
        inbox::next(&pool, &token, time.as_ref(), None).await,
        Ok(None)
    );
    let expiring = ExpiringClock(std::sync::atomic::AtomicUsize::new(0));
    assert_eq!(
        inbox::next(&pool, &token, &expiring, None).await,
        Err(ReviewError::Denied)
    );
    assert_eq!(
        expiring.0.load(Ordering::SeqCst),
        2,
        "empty result rechecks authority"
    );
    assert_eq!(
        inbox::next(
            &pool,
            &SessionToken::from_bytes([12; 32]),
            time.as_ref(),
            None
        )
        .await,
        Err(ReviewError::Denied)
    );
    let (a, server_a) = add_review(
        &pool,
        &time,
        &frozen,
        &ids,
        &paid,
        (ReversalKind::Refund, "refund", PAYMENT),
    )
    .await;
    let (b, server_b) = add_review(
        &pool,
        &time,
        &frozen,
        &ids,
        &paid,
        (ReversalKind::Chargeback, "chargeback", PAYMENT),
    )
    .await;
    assert_eq!(a.updated_at, b.updated_at, "tie uses job ID");
    let mut expected = [a.id.clone(), b.id.clone()];
    expected.sort();
    let counts: (i64,i64,i64) = sqlx::query_as("SELECT (SELECT count(*) FROM reversal_item_jobs),(SELECT count(*) FROM reversal_item_operations),(SELECT count(*) FROM commerce_outbox)").fetch_one(&pool).await.expect("effects before");
    let network = (server_a.count(), server_b.count());
    let first = inbox::next(&pool, &token, time.as_ref(), None)
        .await
        .expect("first")
        .expect("entry");
    assert_eq!(first.position.job_id(), expected[0]);
    assert_eq!(
        first.summary,
        Summary {
            reason: Reason::Invalid,
            generation: 1,
            late_receipt_present: false
        }
    );
    let second = inbox::next(&pool, &token, time.as_ref(), Some(&first.position))
        .await
        .expect("second")
        .expect("entry");
    assert_eq!(second.position.job_id(), expected[1]);
    assert_eq!(second.position.updated_at(), a.updated_at);
    assert_eq!(
        inbox::next(&pool, &token, time.as_ref(), Some(&second.position)).await,
        Ok(None)
    );
    assert_eq!((server_a.count(), server_b.count()), network);
    let after: (i64,i64,i64) = sqlx::query_as("SELECT (SELECT count(*) FROM reversal_item_jobs),(SELECT count(*) FROM reversal_item_operations),(SELECT count(*) FROM commerce_outbox)").fetch_one(&pool).await.expect("effects after");
    assert_eq!(after, counts);
    pool.execute(
        "ALTER TABLE reversal_item_operations DISABLE TRIGGER guard_reversal_item_operation",
    )
    .await
    .expect("isolate corruption");
    sqlx::query("UPDATE reversal_item_operations SET outcome='uncertain' WHERE job_id=$1")
        .bind(&expected[0])
        .execute(&pool)
        .await
        .expect("damage outcome");
    pool.execute(
        "ALTER TABLE reversal_item_operations ENABLE TRIGGER guard_reversal_item_operation",
    )
    .await
    .expect("restore guard");
    assert_eq!(
        inbox::next(&pool, &token, time.as_ref(), None).await,
        Err(ReviewError::Unavailable)
    );
    let expiring = ExpiringClock(std::sync::atomic::AtomicUsize::new(0));
    assert_eq!(
        inbox::next(&pool, &token, &expiring, None).await,
        Err(ReviewError::Denied)
    );
    assert_eq!(
        expiring.0.load(Ordering::SeqCst),
        2,
        "corrupt result rechecks authority"
    );
}

// Explicitly run with scripts/with_postgres.py and P1_REVIEW_WORKER_BIN set to
// the absolute path of the matching owned Cargo output named commerce-worker.
// This separate subprocess test complements the normal inbox custody tests.
// P1_POSTGRES_BIN=/Library/PostgreSQL/18/bin P1_REVIEW_WORKER_BIN=<worker> \
//   python3 scripts/with_postgres.py -- <infra-lib-test> --exact \
//   mollie::tests::reversal_item_supervision_cases::review_inbox_cases::worker_process_reads_two_review_stops_and_empty_without_effects --ignored
// Require the test runner to report exactly one passed test.
#[tokio::test]
#[ignore = "requires an explicitly supplied worker and wrapper-owned PostgreSQL"]
async fn worker_process_reads_two_review_stops_and_empty_without_effects() {
    use sqlx::{ConnectOptions, postgres::PgConnectOptions, postgres::PgPoolOptions};
    use std::{
        io::Write,
        path::PathBuf,
        process::{Command, Stdio},
        time::{Duration, Instant, SystemTime, UNIX_EPOCH},
    };

    assert_eq!(std::env::var("APP_ENV").as_deref(), Ok("test"));
    assert!(std::env::var_os("PGOPTIONS").is_none());
    let worker = PathBuf::from(std::env::var_os("P1_REVIEW_WORKER_BIN").expect("worker path"));
    assert!(worker.is_absolute());
    assert_eq!(
        worker.file_name().and_then(|s| s.to_str()),
        Some("commerce-worker")
    );
    assert_eq!(worker.canonicalize().expect("worker exists"), worker);
    assert!(worker.is_file());
    let marker = std::env::var("P1_POSTGRES_DISPOSABLE_MARKER").expect("disposable marker");
    assert!(
        marker.len() == 64
            && marker
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    );
    let database = std::env::var("DATABASE_URL").expect("disposable database");
    let parsed = url::Url::parse(&database).expect("valid database URL");
    assert!(parsed.query().is_none() && parsed.fragment().is_none());
    let options: PgConnectOptions = database.parse().expect("valid database options");
    assert!(options.get_options().is_none());
    assert_eq!(options.get_host(), "127.0.0.1");
    assert_eq!(options.get_username(), "p1_test");
    assert_eq!(options.get_database(), Some("postgres"));
    let parent = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(options.clone())
        .await
        .expect("disposable cluster");
    let (actual_marker, database_name, username): (Option<String>, String, String) =
        sqlx::query_as(
            "SELECT current_setting('sark.disposable_marker',true),current_database(),current_user",
        )
        .fetch_one(&parent)
        .await
        .expect("cluster identity");
    assert_eq!(actual_marker.as_deref(), Some(marker.as_str()));
    assert_eq!(
        (database_name.as_str(), username.as_str()),
        ("postgres", "p1_test")
    );
    let name = format!("review_worker_{}", &marker[..20]);
    sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE {name}")))
        .execute(&parent)
        .await
        .expect("fresh fixture database");
    let fixture_options = options.database(&name);
    let worker_database = fixture_options.to_url_lossy().to_string();
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect_with(fixture_options)
        .await
        .expect("fixture database");
    crate::MIGRATOR
        .run(&pool)
        .await
        .expect("fixture migrations");

    let (time, frozen, ids, paid) = reversal_supervision_cases::fixture(&pool).await;
    let (a, server_a) = add_review(
        &pool,
        &time,
        &frozen,
        &ids,
        &paid,
        (ReversalKind::Refund, "refund", PAYMENT),
    )
    .await;
    let (b, server_b) = add_review(
        &pool,
        &time,
        &frozen,
        &ids,
        &paid,
        (ReversalKind::Chargeback, "chargeback", PAYMENT),
    )
    .await;
    assert_eq!(a.updated_at, b.updated_at);
    let mut jobs = [a.id.clone(), b.id.clone()];
    jobs.sort();
    let token = [41_u8; 32];
    let now = i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_secs(),
    )
    .expect("time");
    sqlx::query("INSERT INTO staff_accounts(id,enabled) VALUES('worker-reviewer',true)")
        .execute(&pool)
        .await
        .expect("staff");
    sqlx::query("INSERT INTO staff_capabilities(staff_id,capability) VALUES('worker-reviewer','reversal.review')").execute(&pool).await.expect("capability");
    sqlx::query("INSERT INTO staff_sessions(token_hash,staff_id,created_at,expires_at,mfa_verified_at) VALUES($1,'worker-reviewer',$2,$3,$4)")
        .bind(SessionToken::from_bytes(token).digest().as_slice()).bind(now - 5).bind(now + 600).bind(now - 1)
        .execute(&pool).await.expect("fresh staff session");

    let snapshot = || async {
        sqlx::query_as::<_, (String, String, String)>("SELECT (SELECT COALESCE(jsonb_agg(to_jsonb(j) ORDER BY id),'[]'::jsonb)::text FROM reversal_item_jobs j),(SELECT COALESCE(jsonb_agg(to_jsonb(o) ORDER BY id),'[]'::jsonb)::text FROM reversal_item_operations o),(SELECT COALESCE(jsonb_agg(to_jsonb(e) ORDER BY id),'[]'::jsonb)::text FROM commerce_outbox e)")
            .fetch_one(&pool).await.expect("custody snapshot")
    };
    let before = snapshot().await;
    let network = (server_a.count(), server_b.count());
    let run = |cursor: Option<(i64, &str)>| {
        let mut command = Command::new(&worker);
        command
            .env_clear()
            .env("APP_ENV", "test")
            .env("DATABASE_URL", &worker_database)
            .arg("--review-reversal-inbox")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some((updated_at, job)) = cursor {
            command.arg(updated_at.to_string()).arg(job);
        }
        let mut child = command.spawn().expect("spawn owned worker");
        child
            .stdin
            .take()
            .expect("worker stdin")
            .write_all(&token)
            .expect("synthetic token");
        let start = Instant::now();
        while child.try_wait().expect("poll worker").is_none()
            && start.elapsed() < Duration::from_secs(15)
        {
            std::thread::sleep(Duration::from_millis(20));
        }
        if child.try_wait().expect("poll worker").is_none() {
            child.kill().expect("stop stuck worker");
            let _ = child.wait();
            panic!("worker exceeded deadline");
        }
        let output = child.wait_with_output().expect("worker output");
        assert!(output.status.success());
        assert!(output.stderr.is_empty());
        assert!(
            !output
                .stdout
                .windows(token.len())
                .any(|bytes| bytes == token)
        );
        assert!(
            !output
                .stdout
                .windows(worker_database.len())
                .any(|bytes| bytes == worker_database.as_bytes())
        );
        let lines: Vec<_> = output
            .stdout
            .split(|b| *b == b'\n')
            .filter(|line| !line.is_empty())
            .collect();
        assert_eq!(lines.len(), 1);
        let event: Value = serde_json::from_slice(lines[0]).expect("structured worker output");
        event.get("fields").cloned().expect("worker fields")
    };
    for (cursor, expected) in [
        (None, &jobs[0]),
        (Some((a.updated_at, jobs[0].as_str())), &jobs[1]),
    ] {
        assert_eq!(
            run(cursor),
            json!({"code":"reversal_item_review_inbox_entry", "updated_at":a.updated_at, "job_id":expected, "reason":"invalid", "generation":1, "late_receipt_present":false})
        );
    }
    assert_eq!(
        run(Some((a.updated_at, &jobs[1]))),
        json!({"code":"reversal_item_review_inbox_empty"})
    );
    assert_eq!(snapshot().await, before);
    assert_eq!((server_a.count(), server_b.count()), network);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn inbox_withholds_summary_when_staff_expires_during_real_attempt_wait(pool: PgPool) {
    let (time, frozen, ids, paid) = reversal_supervision_cases::fixture(&pool).await;
    staff_review_cases::staff(&pool).await;
    let (job, server) = add_review(
        &pool,
        &time,
        &frozen,
        &ids,
        &paid,
        (ReversalKind::Refund, "expiry", PAYMENT),
    )
    .await;
    let before = server.count();
    let mut held = pool.begin().await.expect("hold attempt");
    sqlx::query("SELECT id FROM payment_attempts WHERE id=$1 FOR UPDATE")
        .bind(&job.attempt_id)
        .fetch_one(&mut *held)
        .await
        .expect("attempt claim");
    let task_pool = pool.clone();
    let task_time = time.clone();
    let task = tokio::spawn(async move {
        inbox::next(
            &task_pool,
            &SessionToken::from_bytes([11; 32]),
            task_time.as_ref(),
            None,
        )
        .await
    });
    tokio::time::timeout(Duration::from_millis(800), async { loop {
        let waiting: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'SELECT left(basket_id,33) FROM payment_attempts%FOR UPDATE%'").fetch_one(&pool).await.expect("waiter");
        if waiting == 1 { break; } tokio::time::sleep(Duration::from_millis(5)).await;
    }}).await.expect("actual attempt wait");
    time.0.store(2000, Ordering::SeqCst);
    held.rollback().await.expect("release attempt");
    assert_eq!(task.await.expect("inbox task"), Err(ReviewError::Denied));
    assert_eq!(server.count(), before);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn inbox_refuses_a_valid_changed_candidate_after_claim_wait(pool: PgPool) {
    let (time, frozen, ids, paid) = reversal_supervision_cases::fixture(&pool).await;
    staff_review_cases::staff(&pool).await;
    let token = SessionToken::from_bytes([11; 32]);
    let (job, server) = add_review(
        &pool,
        &time,
        &frozen,
        &ids,
        &paid,
        (ReversalKind::Refund, "change", PAYMENT),
    )
    .await;
    let before = server.count();
    // Isolated corruption control: move the job and its terminal operation together,
    // preserving otherwise valid history, only after the cursor candidate is captured.
    pool.execute("ALTER TABLE reversal_item_jobs DISABLE TRIGGER guard_reversal_item_job; ALTER TABLE reversal_item_operations DISABLE TRIGGER guard_reversal_item_operation; ALTER TABLE reversal_item_operations DISABLE TRIGGER require_reversal_item_operation_checkpoint").await.expect("disable fixture guards");
    let mut held = pool.begin().await.expect("hold attempt");
    sqlx::query("SELECT id FROM payment_attempts WHERE id=$1 FOR UPDATE")
        .bind(&job.attempt_id)
        .fetch_one(&mut *held)
        .await
        .expect("attempt claim");
    let task_pool = pool.clone();
    let task_time = time.clone();
    let task = tokio::spawn(async move {
        inbox::next(
            &task_pool,
            &SessionToken::from_bytes([11; 32]),
            task_time.as_ref(),
            None,
        )
        .await
    });
    tokio::time::timeout(Duration::from_millis(800), async { loop {
        let waiting: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'SELECT left(basket_id,33) FROM payment_attempts%FOR UPDATE%'").fetch_one(&pool).await.expect("waiter");
        if waiting == 1 { break; } tokio::time::sleep(Duration::from_millis(5)).await;
    }}).await.expect("candidate captured before attempt claim");
    sqlx::query(
        "UPDATE reversal_item_jobs SET updated_at=updated_at+1,due_at=due_at+1 WHERE id=$1",
    )
    .bind(&job.id)
    .execute(&mut *held)
    .await
    .expect("move checkpoint");
    sqlx::query("UPDATE reversal_item_operations SET finished_at=finished_at+1 WHERE job_id=$1")
        .bind(&job.id)
        .execute(&mut *held)
        .await
        .expect("move matching finish");
    held.commit().await.expect("publish changed candidate");
    assert_eq!(
        task.await.expect("inbox task"),
        Err(ReviewError::Unavailable)
    );
    pool.execute("ALTER TABLE reversal_item_jobs ENABLE TRIGGER guard_reversal_item_job; ALTER TABLE reversal_item_operations ENABLE TRIGGER guard_reversal_item_operation; ALTER TABLE reversal_item_operations ENABLE TRIGGER require_reversal_item_operation_checkpoint").await.expect("restore fixture guards");
    let valid = review::inspect(&pool, &token, time.as_ref(), &job.id)
        .await
        .expect("changed receipt independently valid");
    assert!(valid.is_some());
    let current = inbox::next(&pool, &token, time.as_ref(), None)
        .await
        .expect("fresh scan")
        .expect("changed entry");
    assert_eq!(current.position.updated_at(), job.updated_at + 1);
    assert_eq!(server.count(), before);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn review_index_is_partial_and_supports_both_bounded_keyset_shapes(pool: PgPool) {
    let definition: String =
        sqlx::query_scalar("SELECT pg_get_indexdef('reversal_item_review_inbox_cursor'::regclass)")
            .fetch_one(&pool)
            .await
            .expect("index definition");
    assert!(
        definition.contains("updated_at")
            && definition.contains("COLLATE \"C\"")
            && definition.contains("WHERE (state = 'review'::text)")
    );
    let mut tx = pool.begin().await.expect("explain");
    tx.execute("SET LOCAL enable_seqscan=off")
        .await
        .expect("index control");
    for sql in [
        "EXPLAIN SELECT id,updated_at FROM reversal_item_jobs WHERE state='review' ORDER BY updated_at,id COLLATE \"C\" LIMIT 1",
        "EXPLAIN SELECT id,updated_at FROM reversal_item_jobs WHERE state='review' AND (updated_at,id COLLATE \"C\")>(1,'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa' COLLATE \"C\") ORDER BY updated_at,id COLLATE \"C\" LIMIT 1",
    ] {
        let rows: Vec<String> = sqlx::query_scalar(sql)
            .fetch_all(&mut *tx)
            .await
            .expect("plan");
        assert!(
            rows.iter()
                .any(|line| line.contains("reversal_item_review_inbox_cursor")),
            "matching index is usable"
        );
    }
    tx.rollback().await.expect("release");
}

#[sqlx::test(migrations = false)]
async fn inbox_upgrade_preserves_equal_time_stops_from_distinct_attempts(pool: PgPool) {
    use crate::payment_test_fixture::{self, order, quote};
    use commerce_application::{Locale, guest_access::GuestLimits};
    use commerce_infrastructure::{
        checkout_orders::create_order,
        checkout_quotes::issue_quote,
        guest_baskets::{GuestCredential, GuestSessionStore},
        payment_attempts::{observations::record_verified, prepare_attempt, start_dispatch},
    };
    crate::MIGRATOR
        .run_to(65, &pool)
        .await
        .expect("prior schema");
    let (time, first_frozen, ids, first_paid) = reversal_supervision_cases::fixture(&pool).await;
    staff_review_cases::staff(&pool).await;
    let (first, first_server) = add_review(
        &pool,
        &time,
        &first_frozen,
        &ids,
        &first_paid,
        (ReversalKind::Refund, "upgrade-first", PAYMENT),
    )
    .await;

    // Reuse the approved synthetic catalogue, but create another guest/order and
    // payment identity with a distinct identifier stream and provider ID.
    sqlx::query("UPDATE stock_balances SET on_hand = on_hand + 6")
        .execute(&pool)
        .await
        .expect("seed stock for second synthetic order");
    time.0.store(1100, Ordering::SeqCst);
    let store = GuestSessionStore::new(
        pool.clone(),
        time.clone(),
        Arc::new(payment_test_fixture::Ids(
            std::sync::atomic::AtomicU64::new(100_000),
        )),
        GuestLimits::default(),
    )
    .expect("independent guest store");
    let (session, csrf) = quote::guest(&store).await;
    let owner = GuestCredential::from_session(&session).expect("second guest");
    let quoted = issue_quote(
        &store,
        &owner,
        &csrf,
        Locale::Nl,
        &quote::request("second-quote"),
    )
    .await
    .expect("second quote")
    .snapshot;
    let second_order = create_order(
        &store,
        &owner,
        &csrf,
        "second-order",
        &order::intent(&quoted.binding.quote_id),
        &order::seller(),
    )
    .await
    .expect("second order")
    .snapshot;
    time.0.store(1500, Ordering::SeqCst);
    let configuration = config(&credential());
    let second_frozen = prepare_attempt(
        &store,
        &owner,
        &csrf,
        "second-attempt",
        &payment_test_fixture::intent(&second_order),
        &configuration,
    )
    .await
    .expect("second attempt")
    .frozen;
    assert_ne!(
        first_frozen.snapshot().binding.id,
        second_frozen.snapshot().binding.id
    );
    start_dispatch(
        &pool,
        time.as_ref(),
        &ids,
        &second_frozen.snapshot().binding.id,
        &configuration,
    )
    .await
    .expect("second dispatch");
    let payment_id = "tr_SECOND_INBOX_SYNTHETIC";
    let paid =
        observation_cases::fetched(second_frozen.snapshot(), payment_id, "paid", false).await;
    let second_paid = record_verified(
        &pool,
        time.as_ref(),
        &ids,
        &second_frozen.snapshot().binding.id,
        "second-paid",
        &paid,
    )
    .await
    .expect("second paid source")
    .snapshot
    .id;
    let (second, second_server) = add_review(
        &pool,
        &time,
        &second_frozen,
        &ids,
        &second_paid,
        (ReversalKind::Refund, "upgrade-second", payment_id),
    )
    .await;
    assert_ne!(first.attempt_id, second.attempt_id);
    assert_eq!(first.updated_at, second.updated_at);
    let before: (i64,i64,i64) = sqlx::query_as("SELECT (SELECT count(*) FROM reversal_item_jobs),(SELECT count(*) FROM reversal_item_operations),(SELECT count(*) FROM commerce_outbox)").fetch_one(&pool).await.expect("before upgrade");
    let calls = (first_server.count(), second_server.count());
    crate::MIGRATOR.run(&pool).await.expect("add inbox index");
    let after: (i64,i64,i64) = sqlx::query_as("SELECT (SELECT count(*) FROM reversal_item_jobs),(SELECT count(*) FROM reversal_item_operations),(SELECT count(*) FROM commerce_outbox)").fetch_one(&pool).await.expect("after upgrade");
    assert_eq!(before, after);
    let mut expected = [(first.id, first.attempt_id), (second.id, second.attempt_id)];
    expected.sort_by(|a, b| a.0.cmp(&b.0));
    let token = SessionToken::from_bytes([11; 32]);
    let mut other_attempt = pool.begin().await.expect("hold unrelated attempt");
    sqlx::query("SELECT id FROM payment_attempts WHERE id=$1 FOR UPDATE")
        .bind(&expected[1].1)
        .fetch_one(&mut *other_attempt)
        .await
        .expect("hold second attempt claim");
    let a = inbox::next(&pool, &token, time.as_ref(), None)
        .await
        .expect("first scan does not claim unrelated attempt")
        .expect("first entry");
    other_attempt
        .rollback()
        .await
        .expect("release unrelated attempt");
    let b = inbox::next(&pool, &token, time.as_ref(), Some(&a.position))
        .await
        .expect("second scan")
        .expect("second entry");
    assert_eq!(
        [a.position.job_id(), b.position.job_id()],
        [expected[0].0.as_str(), expected[1].0.as_str()]
    );
    assert_eq!(
        a.summary,
        Summary {
            reason: Reason::Invalid,
            generation: 1,
            late_receipt_present: false
        }
    );
    assert_eq!(
        b.summary,
        Summary {
            reason: Reason::Invalid,
            generation: 1,
            late_receipt_present: false
        }
    );
    assert_eq!(
        inbox::next(&pool, &token, time.as_ref(), Some(&b.position)).await,
        Ok(None)
    );
    assert_eq!((first_server.count(), second_server.count()), calls);
}
