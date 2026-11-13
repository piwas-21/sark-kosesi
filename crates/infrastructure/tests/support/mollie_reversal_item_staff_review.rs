//! Exact review inspection against actual PostgreSQL and synthetic loopback custody.
use super::*;
use crate::{
    payment_attempts::reversal_item_supervision::review::{self, Reason, ReviewError, Summary},
    staff_access::SessionToken,
};

pub(super) async fn staff(pool: &PgPool) {
    pool.execute("INSERT INTO staff_accounts(id,enabled) VALUES('reversal-reviewer',true),('reversal-foreign',true),('reversal-revoked',true),('reversal-stale',true); INSERT INTO staff_capabilities(staff_id,capability) VALUES('reversal-reviewer','reversal.review'),('reversal-revoked','reversal.review'),('reversal-stale','reversal.review'),('reversal-foreign','stock.review')").await.expect("unseeded synthetic staff");
    for (id, byte, expires, mfa, revoked) in [
        ("reversal-reviewer", 11, 2000, 1500, None),
        ("reversal-foreign", 12, 4600, 1500, None),
        ("reversal-revoked", 13, 4600, 1500, Some(1500)),
        ("reversal-stale", 14, 4600, 1000, None),
    ] {
        sqlx::query("INSERT INTO staff_sessions(token_hash,staff_id,created_at,expires_at,mfa_verified_at,revoked_at) VALUES($1,$2,1000,$3,$4,$5)")
            .bind(SessionToken::from_bytes([byte;32]).digest().as_slice()).bind(id).bind(expires).bind(mfa).bind(revoked)
            .execute(pool).await.expect("synthetic staff session");
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn review_is_exact_authorized_read_only_and_preserves_late_receipt(pool: PgPool) {
    let (time, frozen, ids, paid) = reversal_supervision_cases::fixture(&pool).await;
    staff(&pool).await;
    let reviewer = SessionToken::from_bytes([11; 32]);
    let page = page_fixture(
        &pool,
        time.as_ref(),
        &ids,
        frozen.snapshot(),
        ReversalKind::Refund,
        "page",
    )
    .await;
    let gate = Arc::new(AtomicBool::new(false));
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, payment(frozen.snapshot())),
        Reply::GateJson(gate.clone(), child(ReversalKind::Refund)),
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
            observation_id: &paid,
            page_id: &page,
            ordinal: 1,
            enrollment_key: "item",
        },
    )
    .await
    .expect("enroll");
    assert_eq!(
        review::inspect(&pool, &reviewer, time.as_ref(), &job.id).await,
        Ok(None)
    );
    assert_eq!(
        review::inspect(&pool, &reviewer, time.as_ref(), "bad-id").await,
        Err(ReviewError::InvalidRequest)
    );
    assert_eq!(
        review::inspect(
            &pool,
            &reviewer,
            time.as_ref(),
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        )
        .await,
        Ok(None)
    );
    for byte in [12, 13] {
        assert_eq!(
            review::inspect(
                &pool,
                &SessionToken::from_bytes([byte; 32]),
                time.as_ref(),
                &job.id
            )
            .await,
            Err(ReviewError::Denied)
        );
    }
    let first = run_batch(&pool, time.as_ref(), &ids, &client, 1);
    let inspect_parked = async {
        tokio::time::timeout(Duration::from_secs(2), async {
            while server.count() < 3 {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .expect("synthetic GET entered");
        time.0.store(1561, Ordering::SeqCst);
        run_batch(&pool, time.as_ref(), &ids, &client, 1)
            .await
            .expect("park uncertain");
        let count = server.count();
        let before: (i64,i64,i64) = sqlx::query_as("SELECT (SELECT count(*) FROM reversal_item_jobs),(SELECT count(*) FROM reversal_item_operations),(SELECT count(*) FROM commerce_outbox)").fetch_one(&pool).await.expect("before");
        assert_eq!(
            review::inspect(&pool, &reviewer, time.as_ref(), &job.id).await,
            Ok(Some(Summary {
                reason: Reason::Uncertain,
                generation: 1,
                late_receipt_present: false
            }))
        );
        let after: (i64,i64,i64) = sqlx::query_as("SELECT (SELECT count(*) FROM reversal_item_jobs),(SELECT count(*) FROM reversal_item_operations),(SELECT count(*) FROM commerce_outbox)").fetch_one(&pool).await.expect("after");
        assert_eq!(before, after);
        assert_eq!(server.count(), count, "inspection cannot call provider");
        gate.store(true, Ordering::SeqCst);
    };
    let (first, ()) = tokio::join!(first, inspect_parked);
    assert!(first.is_err());
    let before = server.count();
    assert_eq!(
        review::inspect(&pool, &reviewer, time.as_ref(), &job.id).await,
        Ok(Some(Summary {
            reason: Reason::Uncertain,
            generation: 1,
            late_receipt_present: true
        }))
    );
    assert_eq!(server.count(), before);
    // Corrupt the otherwise immutable checkpoint in this disposable database.
    pool.execute(
        "ALTER TABLE reversal_item_operations DISABLE TRIGGER guard_reversal_item_operation",
    )
    .await
    .expect("disable isolated guard");
    pool.execute("UPDATE reversal_item_operations SET outcome='invalid' WHERE outcome='uncertain'")
        .await
        .expect("inject isolated mismatch");
    pool.execute(
        "ALTER TABLE reversal_item_operations ENABLE TRIGGER guard_reversal_item_operation",
    )
    .await
    .expect("restore isolated guard");
    assert_eq!(
        review::inspect(&pool, &reviewer, time.as_ref(), &job.id).await,
        Err(ReviewError::Unavailable)
    );
    pool.execute(
        "ALTER TABLE reversal_item_operations DISABLE TRIGGER guard_reversal_item_operation",
    )
    .await
    .expect("disable isolated guard for repair");
    pool.execute("UPDATE reversal_item_operations SET outcome='uncertain' WHERE outcome='invalid'")
        .await
        .expect("restore exact checkpoint");
    pool.execute(
        "ALTER TABLE reversal_item_operations ENABLE TRIGGER guard_reversal_item_operation",
    )
    .await
    .expect("restore isolated guard after repair");
    assert_eq!(
        review::inspect(&pool, &reviewer, time.as_ref(), &job.id).await,
        Ok(Some(Summary {
            reason: Reason::Uncertain,
            generation: 1,
            late_receipt_present: true
        }))
    );
    time.0.store(1901, Ordering::SeqCst);
    assert_eq!(
        review::inspect(
            &pool,
            &SessionToken::from_bytes([14; 32]),
            time.as_ref(),
            &job.id
        )
        .await,
        Err(ReviewError::Denied)
    );
    assert_eq!(server.count(), before);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn expiry_after_real_attempt_wait_withholds_even_absent_summary(pool: PgPool) {
    let (time, frozen, ids, paid) = reversal_supervision_cases::fixture(&pool).await;
    staff(&pool).await;
    let page = page_fixture(
        &pool,
        time.as_ref(),
        &ids,
        frozen.snapshot(),
        ReversalKind::Refund,
        "page",
    )
    .await;
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
            observation_id: &paid,
            page_id: &page,
            ordinal: 1,
            enrollment_key: "item",
        },
    )
    .await
    .expect("enroll");
    let mut held = pool.begin().await.expect("hold attempt");
    sqlx::query("SELECT id FROM payment_attempts WHERE id=$1 FOR UPDATE")
        .bind(&job.attempt_id)
        .fetch_one(&mut *held)
        .await
        .expect("attempt lock");
    let worker_pool = pool.clone();
    let worker_time = time.clone();
    let id = job.id.clone();
    let worker = tokio::spawn(async move {
        review::inspect(
            &worker_pool,
            &SessionToken::from_bytes([11; 32]),
            worker_time.as_ref(),
            &id,
        )
        .await
    });
    tokio::time::timeout(Duration::from_millis(800), async {
        loop {
            let waiting: i64=sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'SELECT left(basket_id,33) FROM payment_attempts%FOR UPDATE%'").fetch_one(&pool).await.expect("waiter");
            if waiting == 1 { break; }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }).await.expect("real attempt wait");
    time.0.store(2000, Ordering::SeqCst);
    held.rollback().await.expect("release attempt");
    assert_eq!(worker.await.expect("worker"), Err(ReviewError::Denied));
    let current = read_job(&pool, &job.id)
        .await
        .expect("read")
        .expect("present");
    assert_eq!(current.state, "pending");
    assert_eq!(server.count(), 1);

    // A concurrent revocation cannot pass the shared staff claim until inspection
    // releases it. A new inspection then observes the committed revocation.
    time.0.store(1500, Ordering::SeqCst);
    let mut held = pool.begin().await.expect("hold attempt again");
    sqlx::query("SELECT id FROM payment_attempts WHERE id=$1 FOR UPDATE")
        .bind(&job.attempt_id)
        .fetch_one(&mut *held)
        .await
        .expect("attempt lock again");
    let inspect_pool = pool.clone();
    let inspect_time = time.clone();
    let inspect_id = job.id.clone();
    let inspection = tokio::spawn(async move {
        review::inspect(
            &inspect_pool,
            &SessionToken::from_bytes([11; 32]),
            inspect_time.as_ref(),
            &inspect_id,
        )
        .await
    });
    tokio::time::timeout(Duration::from_millis(800), async {
        loop {
            let waiting: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'SELECT left(basket_id,33) FROM payment_attempts%FOR UPDATE%'")
                .fetch_one(&pool).await.expect("inspection waiter");
            if waiting == 1 { break; }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }).await.expect("inspection holds staff before attempt wait");
    let revoke_pool = pool.clone();
    let revoke = tokio::spawn(async move {
        sqlx::query("UPDATE staff_sessions SET revoked_at=1500 WHERE token_hash=$1 /* reversal-review-revocation */")
            .bind(SessionToken::from_bytes([11;32]).digest().as_slice()).execute(&revoke_pool).await
    });
    tokio::time::timeout(Duration::from_millis(800), async {
        loop {
            let waiting: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE '%/* reversal-review-revocation */%'")
                .fetch_one(&pool).await.expect("revocation waiter");
            if waiting == 1 { break; }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }).await.expect("revocation waits for staff claim");
    held.rollback().await.expect("release attempt again");
    assert_eq!(inspection.await.expect("inspection task"), Ok(None));
    assert_eq!(
        revoke
            .await
            .expect("revocation task")
            .expect("revoke")
            .rows_affected(),
        1
    );
    assert_eq!(
        review::inspect(
            &pool,
            &SessionToken::from_bytes([11; 32]),
            time.as_ref(),
            &job.id
        )
        .await,
        Err(ReviewError::Denied)
    );
    assert_eq!(server.count(), 1);
}

#[sqlx::test(migrations = false)]
async fn capability_upgrade_preserves_all_existing_grants_without_seeding_review(pool: PgPool) {
    crate::MIGRATOR
        .run_to(61, &pool)
        .await
        .expect("prior migration chain");
    pool.execute("INSERT INTO staff_accounts(id,enabled) VALUES('upgrade-review',true)")
        .await
        .expect("staff");
    let prior = [
        "catalogue.manage",
        "catalogue.review",
        "warehouse.manage",
        "market.manage",
        "market.review",
        "promotion.manage",
        "promotion.review",
        "payment.reconcile",
        "stock.review",
        "stock.recover",
        "refund.prepare",
        "refund.execution.prepare",
    ];
    for capability in prior {
        sqlx::query(
            "INSERT INTO staff_capabilities(staff_id,capability) VALUES('upgrade-review',$1)",
        )
        .bind(capability)
        .execute(&pool)
        .await
        .expect("existing grant");
    }
    crate::MIGRATOR
        .run_to(64, &pool)
        .await
        .expect("capability upgrade");
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM staff_capabilities WHERE staff_id='upgrade-review'",
    )
    .fetch_one(&pool)
    .await
    .expect("existing grants");
    assert_eq!(count, prior.len() as i64);
    let seeded: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM staff_capabilities WHERE capability='reversal.review'",
    )
    .fetch_one(&pool)
    .await
    .expect("new grants");
    assert_eq!(seeded, 0);
    sqlx::query("INSERT INTO staff_capabilities(staff_id,capability) VALUES('upgrade-review','reversal.review')").execute(&pool).await.expect("explicit new grant");
    assert!(sqlx::query("INSERT INTO staff_capabilities(staff_id,capability) VALUES('upgrade-review','reversal.recover')").execute(&pool).await.is_err());
}
