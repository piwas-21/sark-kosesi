//! Synthetic review stops and real PostgreSQL authority, audit and atomicity oracles.
use super::observation_cases::{ids, prepared};
use super::*;
use crate::{
    MIGRATOR,
    payment_attempts::reconciliation::{jobs, operator::*, *},
    staff_access::SessionToken,
};
use sqlx::{Executor, PgPool};
use std::sync::{Arc, atomic::Ordering};
async fn finance(pool: &PgPool) -> SessionToken {
    let token = SessionToken::from_bytes([94; 32]);
    pool.execute("INSERT INTO staff_accounts(id,enabled) VALUES('finance',true); INSERT INTO staff_capabilities VALUES('finance','payment.reconcile')").await.expect("synthetic grant");
    sqlx::query("INSERT INTO staff_sessions VALUES($1,'finance',1000,4000,1400,NULL)")
        .bind(token.digest().as_slice())
        .execute(pool)
        .await
        .expect("synthetic verified MFA");
    token
}
async fn stop(
    pool: &PgPool,
    clock: &dyn commerce_application::request_context::Clock,
    f: &FrozenAttempt,
) {
    jobs::enqueue(pool, clock, &f.snapshot().binding.profile.id)
        .await
        .expect("explicit job");
    pool.execute("UPDATE payment_reconciliation_jobs SET phase='needs_review',review_reason='retry_exhausted',failures=5,generation=generation+1,operation=operation+1").await.expect("synthetic transient stop");
}
fn request<'a>(f: &'a FrozenAttempt, effect: &'a str, generation: i64) -> ResumeRequest<'a> {
    ResumeRequest {
        attempt_id: &f.snapshot().binding.id,
        profile_id: &f.snapshot().binding.profile.id,
        expected_generation: generation,
        expected_resume_revision: 0,
        effect_key: effect,
        reason: "Synthetic transport incident checked",
        confirmed: true,
    }
}
async fn receipts(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM payment_recovery_resumes")
        .fetch_one(pool)
        .await
        .expect("audit count")
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn finance_resume_commits_once_preserves_budgets_and_historical_replay_never_restarts(
    pool: PgPool,
) {
    let (time, f) = prepared(&pool, true).await;
    let token = finance(&pool).await;
    stop(&pool, time.as_ref(), &f).await;
    let before = crate::payment_test_fixture::counts(&pool).await;
    let first = resume_transient(&pool, &token, time.as_ref(), request(&f, "resume", 1))
        .await
        .expect("authorized bounded resume");
    assert!(!first.replayed);
    assert_eq!(first.revision, 1);
    assert!(first.before.phase == Phase::NeedsReview && first.after.phase == Phase::Search);
    assert_eq!(first.after.failures, 0);
    assert_eq!(first.after.generation, 1);
    assert_eq!(first.after.operation, 1);
    assert_eq!(first.after.source_event_id, first.before.source_event_id);
    assert_eq!(first.after.created_at, 1500);
    assert_eq!(first.after.due_at, 1500);
    let server = Server::start(vec![
        profile(),
        Reply::Json(
            200,
            json!({"count":0,"_embedded":{"payments":[]},"_links":{"next":null}}),
        ),
    ]);
    let client = local(&server)
        .verify_profile()
        .await
        .expect("authenticated profile")
        .into_reconciliation();
    let batch = run_batch(&pool, time.as_ref(), &ids(), &client, 25)
        .await
        .expect("only bounded GET discovery resumes");
    assert_eq!(batch.needs_review, 1);
    assert_eq!(server.count(), 2);
    server.inspect(|rs| assert!(rs.iter().all(|r| r.method == "GET")));
    let replay = resume_transient(&pool, &token, time.as_ref(), request(&f, "resume", 1))
        .await
        .expect("historical receipt");
    assert!(replay.replayed);
    assert_eq!(replay.after.generation, 1);
    assert!(
        read_job(&pool, &f.snapshot().binding.id)
            .await
            .expect("read")
            .expect("job")
            .phase
            == Phase::NeedsReview
    );
    assert_eq!(receipts(&pool).await, 1);
    let after = crate::payment_test_fixture::counts(&pool).await;
    assert_eq!(after.0, before.0);
    assert_eq!((after.2, after.3, after.4), (before.2, before.3, before.4));
    assert!(
        pool.execute("UPDATE payment_recovery_resumes SET reason='changed'")
            .await
            .is_err()
    );
    assert!(
        pool.execute("DELETE FROM payment_recovery_resumes")
            .await
            .is_err()
    );
    assert!(
        pool.execute("UPDATE payment_reconciliation_jobs SET phase='search',review_reason=NULL")
            .await
            .is_err()
    );
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn three_lifetime_resumes_never_reset_generation_operations_or_authorize_a_fourth(
    pool: PgPool,
) {
    let (time, f) = prepared(&pool, true).await;
    let token = finance(&pool).await;
    for generation in 1..=4 {
        stop(&pool, time.as_ref(), &f).await;
        let key = format!("resume-{generation}");
        let mut r = request(&f, &key, generation);
        r.expected_resume_revision = i32::try_from(generation - 1).expect("bounded counter");
        let result = resume_transient(&pool, &token, time.as_ref(), r).await;
        if generation <= 3 {
            let receipt = result.expect("bounded resume");
            assert_eq!(i64::from(receipt.revision), generation);
            assert_eq!(receipt.after.generation, generation);
            assert_eq!(receipt.after.operation, generation);
        } else {
            assert_eq!(result.err(), Some(OperatorError::Conflict));
        }
    }
    assert_eq!(receipts(&pool).await, 3);
    let before: Value = sqlx::query_scalar("SELECT to_jsonb(j) FROM payment_reconciliation_jobs j")
        .fetch_one(&pool)
        .await
        .expect("third consumed stop");
    let mut after = before.clone();
    after["phase"] = "search".into();
    after["failures"] = 0.into();
    after["review_reason"] = Value::Null;
    after["resume_revision"] = 4.into();
    assert!(sqlx::query("INSERT INTO payment_recovery_resumes VALUES($1,4,'native-fourth','finance','fourth native decision',$2,$3,$4)").bind(&f.snapshot().binding.id).bind(&f.snapshot().binding.profile.id).bind(before).bind(after).execute(&pool).await.is_err(), "a fourth native decision cannot commit");
    assert_eq!(receipts(&pool).await, 3);
    assert!(
        read_job(&pool, &f.snapshot().binding.id)
            .await
            .expect("read")
            .expect("job")
            .phase
            == Phase::NeedsReview
    );
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn missing_wrong_revoked_expired_and_stale_mfa_authority_cannot_resume_or_replay(
    pool: PgPool,
) {
    let (time, f) = prepared(&pool, true).await;
    let token = finance(&pool).await;
    stop(&pool, time.as_ref(), &f).await;
    assert_eq!(
        resume_transient(
            &pool,
            &SessionToken::from_bytes([95; 32]),
            time.as_ref(),
            request(&f, "resume", 1)
        )
        .await
        .err(),
        Some(OperatorError::Denied)
    );
    for damage in [
        "DELETE FROM staff_capabilities WHERE staff_id='finance'",
        "UPDATE staff_accounts SET enabled=false WHERE id='finance'",
        "UPDATE staff_sessions SET revoked_at=1500 WHERE staff_id='finance'",
        "UPDATE staff_sessions SET expires_at=1500 WHERE staff_id='finance'",
        "UPDATE staff_sessions SET mfa_verified_at=NULL WHERE staff_id='finance'",
    ] {
        let mut tx = pool.begin().await.expect("damage authority");
        tx.execute(damage).await.expect("authority negative");
        tx.commit().await.expect("apply negative");
        assert_eq!(
            resume_transient(&pool, &token, time.as_ref(), request(&f, "resume", 1))
                .await
                .err(),
            Some(OperatorError::Denied)
        );
        pool.execute("INSERT INTO staff_capabilities VALUES('finance','payment.reconcile') ON CONFLICT DO NOTHING; UPDATE staff_accounts SET enabled=true WHERE id='finance'; UPDATE staff_sessions SET expires_at=4000,mfa_verified_at=1400,revoked_at=NULL WHERE staff_id='finance'").await.expect("positive authority restoration");
    }
    time.0.store(2301, Ordering::SeqCst);
    assert_eq!(
        resume_transient(&pool, &token, time.as_ref(), request(&f, "resume", 1))
            .await
            .err(),
        Some(OperatorError::Denied)
    );
    time.0.store(1500, Ordering::SeqCst);
    resume_transient(&pool, &token, time.as_ref(), request(&f, "resume", 1))
        .await
        .expect("positive control");
    pool.execute("DELETE FROM staff_capabilities WHERE staff_id='finance'")
        .await
        .expect("revoke");
    assert_eq!(
        resume_transient(&pool, &token, time.as_ref(), request(&f, "resume", 1))
            .await
            .err(),
        Some(OperatorError::Denied)
    );
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn confirmation_full_intent_profile_generation_and_terminal_stops_fail_closed(pool: PgPool) {
    let (time, f) = prepared(&pool, true).await;
    let token = finance(&pool).await;
    stop(&pool, time.as_ref(), &f).await;
    for (field, expected) in [
        ("confirmation", OperatorError::InvalidRequest),
        ("negative-generation", OperatorError::InvalidRequest),
        ("negative-resume", OperatorError::InvalidRequest),
        ("empty-profile", OperatorError::InvalidRequest),
        ("effect", OperatorError::InvalidRequest),
        ("reason", OperatorError::InvalidRequest),
        ("id", OperatorError::InvalidRequest),
        ("generation", OperatorError::Conflict),
        ("profile", OperatorError::Conflict),
        ("missing", OperatorError::NotFound),
    ] {
        let mut r = request(&f, "resume", 1);
        match field {
            "confirmation" => r.confirmed = false,
            "negative-generation" => r.expected_generation = -1,
            "negative-resume" => r.expected_resume_revision = -1,
            "empty-profile" => r.profile_id = "",
            "effect" => r.effect_key = "bad!",
            "reason" => r.reason = "\n",
            "id" => r.attempt_id = "bad",
            "generation" => r.expected_generation = 0,
            "profile" => r.profile_id = "pfl_OTHER",
            "missing" => r.attempt_id = "ffffffffffffffffffffffffffffffff",
            _ => unreachable!(),
        }
        assert_eq!(
            resume_transient(&pool, &token, time.as_ref(), r)
                .await
                .err(),
            Some(expected)
        );
    }
    resume_transient(&pool, &token, time.as_ref(), request(&f, "resume", 1))
        .await
        .expect("healthy decision");
    for field in ["reason", "generation", "profile", "resume"] {
        let mut r = request(&f, "resume", 1);
        match field {
            "reason" => r.reason = "changed",
            "generation" => r.expected_generation = 0,
            "resume" => r.expected_resume_revision = 1,
            _ => r.profile_id = "pfl_OTHER",
        }
        assert_eq!(
            resume_transient(&pool, &token, time.as_ref(), r)
                .await
                .err(),
            Some(OperatorError::Conflict)
        );
    }
    assert_eq!(receipts(&pool).await, 1);
    for reason in [
        "paid_observed",
        "ambiguous",
        "unresolved",
        "scan_limit",
        "poll_limit",
        "step_limit",
        "custody_invalid",
    ] {
        let mut tx = pool
            .begin()
            .await
            .expect("synthetic distinct terminal stop");
        tx.execute("ALTER TABLE payment_reconciliation_jobs DISABLE TRIGGER USER")
            .await
            .expect("test-only fixture changes");
        sqlx::query("UPDATE payment_reconciliation_jobs SET phase='needs_review',review_reason=$1")
            .bind(reason)
            .execute(&mut *tx)
            .await
            .expect("fixture");
        tx.execute("ALTER TABLE payment_reconciliation_jobs ENABLE TRIGGER USER")
            .await
            .expect("guard");
        tx.commit().await.expect("fixture committed");
        assert_eq!(
            resume_transient(&pool, &token, time.as_ref(), request(&f, "different", 1))
                .await
                .err(),
            Some(OperatorError::Conflict)
        );
    }
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn concurrent_exact_and_conflicting_decisions_serialize_and_leave_one_audit(pool: PgPool) {
    let (time, f) = prepared(&pool, true).await;
    let token = Arc::new(finance(&pool).await);
    let f = Arc::new(f);
    stop(&pool, time.as_ref(), &f).await;
    let mut tasks = Vec::new();
    for _ in 0..2 {
        let (p, t, f, k) = (pool.clone(), time.clone(), f.clone(), token.clone());
        tasks.push(tokio::spawn(async move {
            resume_transient(&p, &k, t.as_ref(), request(&f, "same", 1)).await
        }));
    }
    let a = tasks
        .remove(0)
        .await
        .expect("task")
        .expect("serialized receipt");
    let b = tasks
        .remove(0)
        .await
        .expect("task")
        .expect("serialized receipt");
    assert_ne!(a.replayed, b.replayed);
    assert_eq!(receipts(&pool).await, 1);
    assert_eq!(
        resume_transient(&pool, &token, time.as_ref(), request(&f, "other", 1))
            .await
            .err(),
        Some(OperatorError::Conflict)
    );
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn post_write_authority_expiry_rolls_back_both_decision_and_resume(pool: PgPool) {
    let (time, f) = prepared(&pool, true).await;
    let token = Arc::new(finance(&pool).await);
    stop(&pool, time.as_ref(), &f).await;
    let mut blocker = pool.begin().await.expect("write blocker");
    blocker
        .execute("LOCK TABLE payment_recovery_resumes IN SHARE MODE")
        .await
        .expect("block decision insertion");
    let (p, t, k) = (pool.clone(), time.clone(), token.clone());
    let task =
        tokio::spawn(
            async move { resume_transient(&p, &k, t.as_ref(), request(&f, "wait", 1)).await },
        );
    tokio::time::timeout(Duration::from_millis(800),async {loop {
        let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'INSERT INTO payment_recovery_resumes%')").fetch_one(&pool).await.expect("actual insert wait");
        if waiting {break;}tokio::time::sleep(Duration::from_millis(5)).await;
    }}).await.expect("actual post-authority write wait");
    time.0.store(2301, Ordering::SeqCst);
    blocker.commit().await.expect("release wait");
    assert_eq!(task.await.expect("task").err(), Some(OperatorError::Denied));
    assert_eq!(receipts(&pool).await, 0);
    let job: String = sqlx::query_scalar("SELECT phase FROM payment_reconciliation_jobs")
        .fetch_one(&pool)
        .await
        .expect("unchanged job");
    assert_eq!(job, "needs_review");
}
#[sqlx::test(migrations = false)]
async fn upgrade_retains_original_review_stop_and_installs_no_finance_authority(pool: PgPool) {
    MIGRATOR.run_to(27, &pool).await.expect("prior schema");
    let (time, f) = prepared(&pool, true).await;
    stop(&pool, time.as_ref(), &f).await;
    let mut old: Value =
        sqlx::query_scalar("SELECT to_jsonb(j) FROM payment_reconciliation_jobs j")
            .fetch_one(&pool)
            .await
            .expect("prior schema snapshot");
    old["resume_revision"] = 0.into();
    MIGRATOR.run(&pool).await.expect("forward upgrade");
    let new = read_job(&pool, &f.snapshot().binding.id)
        .await
        .expect("current reader")
        .expect("job");
    assert_eq!(old, serde_json::to_value(new).expect("after"));
    assert_eq!(receipts(&pool).await, 0);
    let grants: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM staff_capabilities WHERE capability='payment.reconcile'",
    )
    .fetch_one(&pool)
    .await
    .expect("empty authority");
    assert_eq!(grants, 0);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn resumed_candidate_checkpoint_reuses_page_and_fetches_late_paid_without_post(pool: PgPool) {
    let (time, f) = prepared(&pool, true).await;
    let a = &f.snapshot().binding.id;
    let mut paid = payment(f.snapshot());
    paid["id"] = "tr_RETRY".into();
    paid["status"] = "paid".into();
    let server = Server::start(vec![
        profile(),
        Reply::Json(
            200,
            json!({"count":1,"_embedded":{"payments":[paid.clone()]},"_links":{"next":null}}),
        ),
        Reply::Json(503, json!({})),
        Reply::Json(503, json!({})),
        Reply::Json(503, json!({})),
        Reply::Json(503, json!({})),
        Reply::Json(503, json!({})),
        Reply::Json(200, paid),
    ]);
    let client = local(&server)
        .verify_profile()
        .await
        .expect("profile")
        .into_reconciliation();
    let i = ids();
    for at in [1500, 1560, 1680, 1920, 2400] {
        time.0.store(at, Ordering::SeqCst);
        run_batch(&pool, time.as_ref(), &i, &client, 25)
            .await
            .expect("bounded backoff");
    }
    let before = read_job(&pool, a).await.expect("read").expect("job");
    assert!(before.page_id.is_some());
    assert_eq!(before.candidate_index, 0);
    assert_eq!(before.failures, 5);
    let token = finance(&pool).await;
    pool.execute("UPDATE staff_sessions SET mfa_verified_at=2400 WHERE staff_id='finance'")
        .await
        .expect("fresh synthetic MFA");
    let receipt = resume_transient(
        &pool,
        &token,
        time.as_ref(),
        request(&f, "candidate-retry", before.generation),
    )
    .await
    .expect("resume exact checkpoint");
    assert_eq!(receipt.after.page_id, before.page_id);
    assert!(receipt.after.phase == Phase::Candidates);
    let result = run_batch(&pool, time.as_ref(), &i, &client, 25)
        .await
        .expect("fetch and resolve only");
    assert_eq!(result.needs_review, 1);
    assert_eq!(server.count(), 8);
    let summary = crate::payment_attempts::identities::latest(&pool, a)
        .await
        .expect("retained paid")
        .expect("resolution");
    assert_eq!(summary.summary.paid_provider_ids, vec!["tr_RETRY"]);
    assert_eq!(crate::payment_test_fixture::counts(&pool).await.4, 2);
    server.inspect(|rs| {
        assert_eq!(
            rs.iter()
                .filter(|r| r.path == "/v2/payments?limit=10&sort=desc")
                .count(),
            1
        )
    });
    assert_eq!(
        resume_transient(
            &pool,
            &token,
            time.as_ref(),
            request(&f, "no-paid-restart", receipt.after.generation + 2)
        )
        .await
        .err(),
        Some(OperatorError::Conflict)
    );
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn native_orphan_decision_and_changed_checkpoint_cannot_commit(pool: PgPool) {
    let (time, f) = prepared(&pool, true).await;
    let _token = finance(&pool).await;
    stop(&pool, time.as_ref(), &f).await;
    let job = read_job(&pool, &f.snapshot().binding.id)
        .await
        .expect("read")
        .expect("job");
    let before = serde_json::to_value(job).expect("snapshot");
    let mut after = before.clone();
    after["phase"] = "search".into();
    after["failures"] = 0.into();
    after["resume_revision"] = 1.into();
    after["review_reason"] = Value::Null;
    let mut tx = pool.begin().await.expect("orphan fixture");
    sqlx::query("INSERT INTO payment_recovery_resumes VALUES($1,1,'orphan','finance','test incident',$2,$3,$4)").bind(&f.snapshot().binding.id).bind(&f.snapshot().binding.profile.id).bind(&before).bind(&after).execute(&mut *tx).await.expect("valid decision before deferred atomicity check");
    assert!(
        tx.commit().await.is_err(),
        "a decision without the exact checkpoint change cannot commit"
    );
    assert_eq!(receipts(&pool).await, 0);
    for field in [
        "operation",
        "generation",
        "source_event_id",
        "created_at",
        "page_id",
    ] {
        let mut bad = after.clone();
        bad[field] = if ["source_event_id", "page_id"].contains(&field) {
            Value::String("f".repeat(32))
        } else {
            0.into()
        };
        assert!(sqlx::query("INSERT INTO payment_recovery_resumes VALUES($1,1,'damaged','finance','test incident',$2,$3,$4)").bind(&f.snapshot().binding.id).bind(&f.snapshot().binding.profile.id).bind(&before).bind(bad).execute(&pool).await.is_err());
    }
    assert_eq!(receipts(&pool).await, 0);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn damaged_audit_cannot_rebind_actor_profile_source_or_future_history(pool: PgPool) {
    let (time, f) = prepared(&pool, true).await;
    let token = finance(&pool).await;
    stop(&pool, time.as_ref(), &f).await;
    resume_transient(&pool, &token, time.as_ref(), request(&f, "original", 1))
        .await
        .expect("original decision");
    let (before, after): (Value, Value) =
        sqlx::query_as("SELECT before_state,after_state FROM payment_recovery_resumes")
            .fetch_one(&pool)
            .await
            .expect("original snapshots");
    pool.execute("INSERT INTO staff_accounts(id,enabled) VALUES('other-finance',true)")
        .await
        .expect("other identity");
    for field in [
        "actor",
        "profile",
        "source_event_id",
        "created_at",
        "generation",
        "updated_at",
        "resume_revision",
    ] {
        let (mut b, mut a) = (before.clone(), after.clone());
        let (mut actor, mut profile) = ("finance", "pfl_SYNTHETIC_NotARealProfile");
        let mut revision = 1;
        match field {
            "actor" => actor = "other-finance",
            "profile" => profile = "pfl_OTHER",
            "source_event_id" => {
                b[field] = "c".repeat(32).into();
                a[field] = b[field].clone();
            }
            "created_at" => {
                b[field] = 1000.into();
                a[field] = 1000.into();
            }
            "generation" => {
                b[field] = 2.into();
                a[field] = 2.into();
            }
            "resume_revision" => {
                b[field] = 1.into();
                a[field] = 2.into();
                revision = 2;
            }
            "updated_at" => {
                a[field] = 1600.into();
                a["due_at"] = 1600.into();
            }
            _ => unreachable!(),
        }
        let mut tx = pool.begin().await.expect("isolated imported damage");
        tx.execute("ALTER TABLE payment_recovery_resumes DISABLE TRIGGER USER")
            .await
            .expect("test damage");
        sqlx::query("UPDATE payment_recovery_resumes SET actor=$1,profile_id=$2,before_state=$3,after_state=$4,revision=$5").bind(actor).bind(profile).bind(b).bind(a).bind(revision).execute(&mut *tx).await.expect("damaged import");
        tx.execute("ALTER TABLE payment_recovery_resumes ENABLE TRIGGER USER")
            .await
            .expect("restore guard");
        tx.commit().await.expect("import committed");
        let expected = if ["actor", "profile"].contains(&field) {
            OperatorError::Conflict
        } else {
            OperatorError::InvalidStoredData
        };
        assert_eq!(
            resume_transient(&pool, &token, time.as_ref(), request(&f, "original", 1))
                .await
                .err(),
            Some(expected),
            "{field}"
        );
    }
    let mut tx = pool.begin().await.expect("restore original audit");
    tx.execute("ALTER TABLE payment_recovery_resumes DISABLE TRIGGER USER")
        .await
        .expect("fixture restore");
    sqlx::query("UPDATE payment_recovery_resumes SET actor='finance',profile_id=$1,before_state=$2,after_state=$3,revision=1").bind(&f.snapshot().binding.profile.id).bind(before).bind(after).execute(&mut *tx).await.expect("exact original restore");
    tx.execute("ALTER TABLE payment_recovery_resumes ENABLE TRIGGER USER")
        .await
        .expect("guard restored");
    tx.commit().await.expect("restore committed");
    assert!(
        resume_transient(&pool, &token, time.as_ref(), request(&f, "original", 1))
            .await
            .expect("restored positive replay")
            .replayed
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn native_consumed_resume_revision_cannot_rewind_or_reuse_a_historical_decision(
    pool: PgPool,
) {
    let (time, f) = prepared(&pool, true).await;
    let token = finance(&pool).await;
    stop(&pool, time.as_ref(), &f).await;
    resume_transient(&pool, &token, time.as_ref(), request(&f, "first", 1))
        .await
        .expect("first consumed decision");
    assert!(
        pool.execute("UPDATE payment_reconciliation_jobs SET resume_revision=0")
            .await
            .is_err(),
        "working jobs cannot rewind consumption"
    );
    pool.execute("UPDATE payment_reconciliation_jobs SET phase='needs_review',review_reason='retry_exhausted',failures=5").await.expect("same generation transient stop from a direct import");
    for sql in [
        "UPDATE payment_reconciliation_jobs SET phase='search',failures=0,review_reason=NULL",
        "UPDATE payment_reconciliation_jobs SET phase='search',failures=0,review_reason=NULL,resume_revision=0",
        "UPDATE payment_reconciliation_jobs SET phase='search',failures=0,review_reason=NULL,resume_revision=2",
    ] {
        assert!(
            pool.execute(sql).await.is_err(),
            "old receipt must not grant another resume: {sql}"
        );
    }
    assert_eq!(receipts(&pool).await, 1);
    let stopped = read_job(&pool, &f.snapshot().binding.id)
        .await
        .expect("read")
        .expect("job");
    assert_eq!(stopped.resume_revision, 1);
    assert!(stopped.phase == Phase::NeedsReview);
    assert!(
        resume_transient(&pool, &token, time.as_ref(), request(&f, "first", 1))
            .await
            .expect("history only")
            .replayed
    );
    assert!(
        read_job(&pool, &f.snapshot().binding.id)
            .await
            .expect("read")
            .expect("job")
            .phase
            == Phase::NeedsReview
    );
    assert_eq!(
        resume_transient(&pool, &token, time.as_ref(), request(&f, "second", 1))
            .await
            .err(),
        Some(OperatorError::Conflict),
        "stale stop consumption cannot authorize another decision"
    );
    let mut second = request(&f, "second", 1);
    second.expected_resume_revision = 1;
    let next = resume_transient(&pool, &token, time.as_ref(), second)
        .await
        .expect("a new audited decision is required even at the same generation");
    assert_eq!(next.revision, 2);
    assert_eq!(next.after.resume_revision, 2);
    assert_eq!(receipts(&pool).await, 2);
}
