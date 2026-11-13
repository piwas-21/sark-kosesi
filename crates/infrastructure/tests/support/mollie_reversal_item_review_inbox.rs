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
    kind: ReversalKind,
    effect: &str,
) -> (
    crate::payment_attempts::reversal_item_supervision::Job,
    Server,
) {
    let page = page_fixture(pool, time.as_ref(), ids, frozen.snapshot(), kind, effect).await;
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, payment(frozen.snapshot())),
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
        ReversalKind::Refund,
        "refund",
    )
    .await;
    let (b, server_b) = add_review(
        &pool,
        &time,
        &frozen,
        &ids,
        &paid,
        ReversalKind::Chargeback,
        "chargeback",
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
        ReversalKind::Refund,
        "expiry",
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
        ReversalKind::Refund,
        "change",
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
        "EXPLAIN SELECT id,attempt_id,updated_at FROM reversal_item_jobs WHERE state='review' ORDER BY updated_at,id COLLATE \"C\" LIMIT 1",
        "EXPLAIN SELECT id,attempt_id,updated_at FROM reversal_item_jobs WHERE state='review' AND (updated_at,id COLLATE \"C\")>(1,'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa' COLLATE \"C\") ORDER BY updated_at,id COLLATE \"C\" LIMIT 1",
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
