//! Exact crash and stale-generation controls; fetched facts survive checkpoint fences.
use super::*;

#[sqlx::test(migrator = "MIGRATOR")]
async fn five_lost_get_permits_exhaust_the_lifetime_budget_without_a_sixth_operation_or_get(
    pool: PgPool,
) {
    let clock = time();
    let i = ids();
    let id = admit(&pool, clock.as_ref(), &i, PROFILE, "tr_LOST_PERMITS").await;
    for generation in 1..=5 {
        clock
            .0
            .store(1500 + 60 * (generation - 1), Ordering::SeqCst);
        let mut claim = store::jobs::claim(&pool, clock.as_ref(), &i, PROFILE)
            .await
            .expect("bounded claim")
            .expect("pending");
        assert_eq!(
            claim.job.generation,
            i64::try_from(generation).expect("small literal")
        );
        claim
            .take_permit()
            .expect("simulate lost permit before transport");
    }
    clock.0.store(1800, Ordering::SeqCst);
    let server = Server::start(vec![profile()]);
    let c = client(&server).await;
    let batch = run_callback_batch(&pool, clock.as_ref(), &i, &c, 25)
        .await
        .expect("visible generation-limit stop");
    assert_eq!(batch.processed, 0);
    assert_eq!(batch.needs_review, 1);
    assert!(
        read_callback_job(&pool, &id)
            .await
            .expect("read")
            .expect("job")
            .review_reason
            == Some(ReviewReason::GenerationLimit)
    );
    assert_eq!(counts(&pool).await, (5, 0, 0, 0, 0));
    assert_eq!(server.count(), 1);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn checkpoint_clock_before_its_claim_never_changes_pending_work(pool: PgPool) {
    let clock = time();
    let i = ids();
    let id = admit(&pool, clock.as_ref(), &i, PROFILE, "tr_EARLY_CHECKPOINT").await;
    let claim = store::jobs::claim(&pool, clock.as_ref(), &i, PROFILE)
        .await
        .expect("claim")
        .expect("pending");
    clock.0.store(1499, Ordering::SeqCst);
    assert_eq!(
        store::jobs::finish(&pool, clock.as_ref(), &claim, JobState::Pending, None)
            .await
            .err(),
        Some(PaymentStoreError::ClockUnavailable)
    );
    clock.0.store(1500, Ordering::SeqCst);
    store::jobs::finish(&pool, clock.as_ref(), &claim, JobState::Pending, None)
        .await
        .expect("paired healthy clock");
    assert_eq!(
        read_callback_job(&pool, &id)
            .await
            .expect("read")
            .expect("job")
            .due_at,
        1560
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn completed_current_generation_cannot_release_capacity_while_an_older_generation_is_unresolved(
    pool: PgPool,
) {
    let (clock, f) = prepared(&pool, true).await;
    let i = ids();
    let id = admit(
        &pool,
        clock.as_ref(),
        &i,
        PROFILE,
        "tr_CAPACITY_GENERATIONS",
    )
    .await;
    let server = Server::start(vec![
        profile(),
        Reply::Json(
            200,
            response(f.snapshot(), "tr_CAPACITY_GENERATIONS", "open"),
        ),
        Reply::Json(
            200,
            response(f.snapshot(), "tr_CAPACITY_GENERATIONS", "open"),
        ),
    ]);
    let c = client(&server).await;
    let mut old = store::jobs::claim(&pool, clock.as_ref(), &i, PROFILE)
        .await
        .expect("old claim")
        .expect("pending");
    let old_result = c
        .fetch_lookup(old.take_permit().expect("old GET start"))
        .await
        .expect("old authenticated result not yet committed");
    clock.0.store(1560, Ordering::SeqCst);
    run_callback_batch(&pool, clock.as_ref(), &i, &c, 1)
        .await
        .expect("new OPEN generation completes");
    let job = read_callback_job(&pool, &id)
        .await
        .expect("job")
        .expect("exists");
    assert!(job.state == JobState::Done);
    assert!(!job.settled);
    clock.0.store(1620, Ordering::SeqCst);
    for n in 0..30 {
        admit(
            &pool,
            clock.as_ref(),
            &i,
            PROFILE,
            &format!("tr_CAP_FILL_A_{n}"),
        )
        .await;
    }
    clock.0.store(1680, Ordering::SeqCst);
    for n in 0..19 {
        admit(
            &pool,
            clock.as_ref(),
            &i,
            PROFILE,
            &format!("tr_CAP_FILL_B_{n}"),
        )
        .await;
    }
    assert!(
        admit_callback(&pool, clock.as_ref(), &i, PROFILE, "tr_FALSE_FREE_SLOT")
            .await
            .is_err(),
        "complete latest OPEN plus unresolved old generation still occupies fiftieth slot"
    );
    assert_eq!(intake_counts(&pool).await, (50, 50, 50));
    let mut imported = pool
        .begin()
        .await
        .expect("isolated false settlement import");
    imported
        .execute("ALTER TABLE callback_inbox_jobs DISABLE TRIGGER USER")
        .await
        .expect("test-only mutable job bypass");
    sqlx::query("UPDATE callback_inbox_jobs SET settled=true WHERE intake_id=$1")
        .bind(&id)
        .execute(&mut *imported)
        .await
        .expect("one-field false settled flag");
    imported
        .execute("ALTER TABLE callback_inbox_jobs ENABLE TRIGGER USER")
        .await
        .expect("guard restored");
    imported.commit().await.expect("import fixture");
    assert_eq!(
        read_callback_job(&pool, &id).await.err(),
        Some(PaymentStoreError::InvalidStoredData),
        "independent active custody disagrees with imported flag"
    );
    assert!(
        admit_callback(&pool, clock.as_ref(), &i, PROFILE, "tr_IMPORTED_FALSE_SLOT")
            .await
            .is_err(),
        "one corrupt flag cannot release native capacity"
    );
    assert_eq!(intake_counts(&pool).await, (50, 50, 50));
    let mut imported = pool.begin().await.expect("exact fixture repair");
    imported
        .execute("ALTER TABLE callback_inbox_jobs DISABLE TRIGGER USER")
        .await
        .expect("fixture repair");
    sqlx::query("UPDATE callback_inbox_jobs SET settled=false WHERE intake_id=$1")
        .bind(&id)
        .execute(&mut *imported)
        .await
        .expect("restore unsettled flag");
    imported
        .execute("ALTER TABLE callback_inbox_jobs ENABLE TRIGGER USER")
        .await
        .expect("guard restored");
    imported.commit().await.expect("repaired fixture");
    record_lookup(&pool, clock.as_ref(), &i, &old.operation_id, &old_result)
        .await
        .expect("retain old independent OPEN result");
    assert_eq!(
        store::drain::drain_receipts(&pool, clock.as_ref(), &i, PROFILE, 25)
            .await
            .expect("old receipt processing"),
        1
    );
    store::jobs::sweep(&pool, clock.as_ref(), PROFILE)
        .await
        .expect("settle only complete generation set");
    assert!(
        read_callback_job(&pool, &id)
            .await
            .expect("read")
            .expect("job")
            .settled
    );
    admit(&pool, clock.as_ref(), &i, PROFILE, "tr_TRULY_FREE_SLOT").await;
    assert_eq!(intake_counts(&pool).await, (51, 51, 51));
    assert_eq!(server.count(), 3);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn receipt_awaiting_disposition_behind_a_bounded_backlog_never_creates_another_get_generation(
    pool: PgPool,
) {
    let (clock, f) = prepared(&pool, true).await;
    let i = ids();
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, response(f.snapshot(), "tr_SAVED_BACKLOG", "open")),
        Reply::Json(200, response(f.snapshot(), "tr_SAVED_BACKLOG", "open")),
        Reply::Json(200, response(f.snapshot(), "tr_SAVED_BACKLOG", "open")),
    ]);
    let c = client(&server).await;
    let mut intakes = Vec::new();
    for _ in 0..3 {
        let id = admit(&pool, clock.as_ref(), &i, PROFILE, "tr_SAVED_BACKLOG").await;
        let mut claim = store::jobs::claim(&pool, clock.as_ref(), &i, PROFILE)
            .await
            .expect("claim")
            .expect("pending");
        assert_eq!(claim.job.intake_id, id);
        let fetched = c
            .fetch_lookup(claim.take_permit().expect("single use"))
            .await
            .expect("authenticated saved result");
        record_lookup(&pool, clock.as_ref(), &i, &claim.operation_id, &fetched)
            .await
            .expect("crash after receipt before processing/checkpoint");
        intakes.push(id);
    }
    clock.0.store(1560, Ordering::SeqCst);
    let first = run_callback_batch(&pool, clock.as_ref(), &i, &c, 1)
        .await
        .expect("bounded backlog processing");
    assert_eq!(
        first.processed, 0,
        "saved receipts prevent a second GET even when outside bounded drain prefix"
    );
    assert_eq!(server.count(), 4);
    assert_eq!(counts(&pool).await.0, 3);
    for id in &intakes {
        assert_eq!(
            read_callback_job(&pool, id)
                .await
                .expect("job")
                .expect("exists")
                .generation,
            1
        );
    }
    run_callback_batch(&pool, clock.as_ref(), &i, &c, 25)
        .await
        .expect("later pass drains remainder");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM callback_receipt_processing")
            .fetch_one(&pool)
            .await
            .expect("all receipts disposed"),
        3
    );
    assert_eq!(server.count(), 4);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn claim_commits_exact_operation_generation_and_single_use_start_before_any_provider_io(
    pool: PgPool,
) {
    let clock = time();
    let i = ids();
    let id = admit(&pool, clock.as_ref(), &i, PROFILE, "tr_CLAIM").await;
    let mut claim = store::jobs::claim(&pool, clock.as_ref(), &i, PROFILE)
        .await
        .expect("claim")
        .expect("pending");
    assert_eq!(claim.job.intake_id, id);
    assert_eq!(claim.job.generation, 1);
    assert_eq!(claim.until, 1560);
    let bindings:(String,i64,String,i64,i64,i64)=sqlx::query_as("SELECT g.intake_id,g.generation,g.operation_id,g.created_at,g.lease_until,s.started_at FROM callback_inbox_generations g JOIN callback_lookup_fetch_starts s ON s.operation_id=g.operation_id").fetch_one(&pool).await.expect("exact immutable mapping and start");
    assert_eq!(
        bindings,
        (id.clone(), 1, claim.operation_id.clone(), 1500, 1560, 1500)
    );
    assert!(
        store::jobs::claim(&pool, clock.as_ref(), &i, PROFILE)
            .await
            .expect("competing supervisor")
            .is_none()
    );
    claim.take_permit().expect("one postcommit permit");
    assert_eq!(
        claim.take_permit().err(),
        Some(PaymentStoreError::Unavailable)
    );
    assert_eq!(counts(&pool).await, (1, 0, 0, 0, 0));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn gated_get_holds_no_database_claims_and_cancellation_never_reuses_a_started_operation(
    pool: PgPool,
) {
    let (clock, f) = prepared(&pool, true).await;
    let i = Arc::new(ids());
    let id = admit(
        &pool,
        clock.as_ref(),
        i.as_ref(),
        PROFILE,
        "tr_CANCEL_INBOX",
    )
    .await;
    let gate = Arc::new(AtomicBool::new(false));
    let server = Server::start(vec![profile(), Reply::Gate(gate)]);
    let c = Arc::new(client(&server).await);
    let (p, t, i2, c2) = (pool.clone(), clock.clone(), i.clone(), c.clone());
    let task = tokio::spawn(async move {
        run_callback_batch(&p, t.as_ref(), i2.as_ref(), c2.as_ref(), 1).await
    });
    tokio::time::timeout(Duration::from_secs(1), async {
        while server.count() < 2 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("GET in flight");
    let mut tx = pool.begin().await.expect("independent claims");
    tx.execute("SET LOCAL lock_timeout='100ms'")
        .await
        .expect("bounded");
    for query in [
        "SELECT * FROM callback_inbox_jobs FOR UPDATE NOWAIT",
        "SELECT * FROM callback_lookup_operations FOR UPDATE NOWAIT",
        "SELECT * FROM callback_inbox_generations FOR UPDATE NOWAIT",
        "SELECT * FROM payment_attempts FOR UPDATE NOWAIT",
    ] {
        sqlx::query(query)
            .fetch_all(&mut *tx)
            .await
            .expect("no transaction over GET");
    }
    tx.rollback().await.expect("release");
    task.abort();
    assert!(
        task.await
            .err()
            .expect("cancelled supervisor")
            .is_cancelled()
    );
    assert_eq!(counts(&pool).await, (1, 0, 0, 1, 0));
    assert_eq!(
        read_callback_job(&pool, &id)
            .await
            .expect("job")
            .expect("exists")
            .generation,
        1
    );
    clock.0.store(1560, Ordering::SeqCst);
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, response(f.snapshot(), "tr_CANCEL_INBOX", "open")),
    ]);
    let c = client(&server).await;
    run_callback_batch(&pool, clock.as_ref(), i.as_ref(), &c, 1)
        .await
        .expect("fresh generation only");
    assert_eq!(counts(&pool).await, (2, 1, 1, 1, 1));
    let job = read_callback_job(&pool, &id)
        .await
        .expect("read")
        .expect("job");
    assert_eq!(job.generation, 2);
    assert!(!job.settled, "lost old generation still consumes capacity");
}

async fn delayed_paid(pool: &PgPool, stop: bool) {
    let (clock, f) = prepared(pool, true).await;
    let i = ids();
    let id = admit(pool, clock.as_ref(), &i, PROFILE, "tr_DELAYED_PAID").await;
    let gate = Arc::new(AtomicBool::new(false));
    let second = if stop {
        let mut p = response(f.snapshot(), "tr_DELAYED_PAID", "open");
        p["profileId"] = "pfl_OTHER".into();
        p
    } else {
        response(f.snapshot(), "tr_DELAYED_PAID", "open")
    };
    let server = Server::start(vec![
        profile(),
        Reply::GateJson(
            gate.clone(),
            response(f.snapshot(), "tr_DELAYED_PAID", "paid"),
        ),
        Reply::Json(200, second),
    ]);
    let c = Arc::new(client(&server).await);
    let mut old = store::jobs::claim(pool, clock.as_ref(), &i, PROFILE)
        .await
        .expect("old claim")
        .expect("pending");
    let permit = old.take_permit().expect("old permit");
    let c2 = c.clone();
    let task = tokio::spawn(async move { c2.fetch_lookup(permit).await });
    tokio::time::timeout(Duration::from_secs(1), async {
        while server.count() < 2 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("old GET in flight");
    clock.0.store(1560, Ordering::SeqCst);
    let newer = run_callback_batch(pool, clock.as_ref(), &i, c.as_ref(), 1)
        .await
        .expect("fresh current generation");
    assert_eq!(newer.processed, 1);
    let before = read_callback_job(pool, &id)
        .await
        .expect("job")
        .expect("exists");
    assert_eq!(before.generation, 2);
    if stop {
        assert!(before.state == JobState::NeedsReview);
    } else {
        assert!(before.state == JobState::Done);
        assert!(!before.settled);
    }
    gate.store(true, Ordering::SeqCst);
    let fetched = task
        .await
        .expect("old transport task")
        .expect("independent late PAID");
    record_lookup(pool, clock.as_ref(), &i, &old.operation_id, &fetched)
        .await
        .expect("stale lease may retain fetched fact");
    assert_eq!(
        store::jobs::finish(pool, clock.as_ref(), &old, JobState::Done, None)
            .await
            .err(),
        Some(PaymentStoreError::Conflict),
        "stale worker cannot checkpoint"
    );
    let drained = store::drain::drain_receipts(pool, clock.as_ref(), &i, PROFILE, 25)
        .await
        .expect("late event drains despite latest state");
    assert_eq!(drained, 1);
    let after = read_callback_job(pool, &id)
        .await
        .expect("job retained")
        .expect("exists");
    assert_eq!(after.generation, 2);
    assert!(after.state == before.state);
    let resolution = identities::latest(pool, &f.snapshot().binding.id)
        .await
        .expect("summary")
        .expect("exists");
    assert_eq!(
        resolution.summary.paid_provider_ids,
        vec!["tr_DELAYED_PAID"]
    );
    let reported = run_callback_batch(pool, clock.as_ref(), &i, c.as_ref(), 25)
        .await
        .expect("unchanged operational report");
    assert_eq!(reported.processed, 0);
    assert_eq!(reported.needs_review, 1);
    assert_eq!(server.count(), 3);
    get_only(&server);
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn old_generation_paid_after_new_open_is_drained_without_advancing_stale_checkpoint(
    pool: PgPool,
) {
    delayed_paid(&pool, false).await;
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn old_generation_paid_after_current_review_stop_remains_drainable_and_visible(pool: PgPool) {
    delayed_paid(&pool, true).await;
}
