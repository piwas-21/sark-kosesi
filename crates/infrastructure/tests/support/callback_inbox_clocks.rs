//! Actual write waits prove transaction boundaries, final clocks and crash replay.
use super::*;

#[sqlx::test(migrator = "MIGRATOR")]
async fn historical_lookup_observation_still_binds_payment_key_and_time_after_a_coherent_multi_record_import(
    pool: PgPool,
) {
    let (clock, f) = prepared(&pool, true).await;
    let i = ids();
    let op = operation(&pool, clock.as_ref(), &i, "tr_HISTORY_BINDING").await;
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, response(f.snapshot(), "tr_HISTORY_BINDING", "open")),
    ]);
    let c = client(&server).await;
    let source = fetch_lookup(&pool, clock.as_ref(), &i, &op.snapshot().id, &c)
        .await
        .expect("original receipt");
    let original_observation = process_lookup(&pool, clock.as_ref(), &i, &op.snapshot().id)
        .await
        .expect("original observation");
    let (event_id, payload, created_at): (String, Value, i64) =
        sqlx::query_as("SELECT id,payload,created_at FROM callback_processing_events")
            .fetch_one(&pool)
            .await
            .expect("exact independent event");
    for field in ["payment", "fingerprint", "recorded"] {
        let mut modified = source.snapshot.clone();
        match field {
            "payment" => modified.payment.status = PaymentStatus::Paid,
            "fingerprint" => modified.credential_fingerprint = "b".repeat(64),
            _ => modified.recorded_at = 1501,
        };
        // Bypass both independent records only inside this disposable import
        // fixture, so the historical comparison itself must reject the change.
        let mut tx = pool.begin().await.expect("isolated multi-record import");
        tx.execute("ALTER TABLE callback_lookup_receipts DISABLE TRIGGER USER")
            .await
            .expect("fixture bypass");
        sqlx::query("UPDATE callback_lookup_receipts SET snapshot=$1,recorded_at=$2")
            .bind(serde_json::to_value(&modified).expect("coherent closed import"))
            .bind(modified.recorded_at)
            .execute(&mut *tx)
            .await
            .expect("changed receipt");
        tx.execute("ALTER TABLE callback_lookup_receipts ENABLE TRIGGER USER")
            .await
            .expect("guard restored");
        tx.execute("ALTER TABLE callback_processing_events DISABLE TRIGGER USER")
            .await
            .expect("fixture bypass");
        tx.execute("UPDATE callback_processing_events e SET created_at=r.recorded_at,payload=jsonb_build_object('schema_version',1,'operation_id',r.operation_id,'receipt_id',r.id,'snapshot_sha256',encode(sha256(convert_to(r.snapshot::text,'UTF8')),'hex')) FROM callback_lookup_receipts r WHERE e.operation_id=r.operation_id").await.expect("coherent altered event");
        tx.execute("ALTER TABLE callback_processing_events ENABLE TRIGGER USER")
            .await
            .expect("guard restored");
        tx.commit().await.expect("import");
        assert!(
            read_lookup(&pool, &op.snapshot().id)
                .await
                .expect("coherent import passes its own closed receipt boundary")
                .is_some()
        );
        assert_eq!(
            process_lookup(&pool, &BadPorts, &BadPorts, &op.snapshot().id)
                .await
                .err(),
            Some(PaymentStoreError::InvalidStoredData),
            "{field}"
        );
        let mut tx = pool
            .begin()
            .await
            .expect("exact paired fixture restoration");
        tx.execute("ALTER TABLE callback_lookup_receipts DISABLE TRIGGER USER")
            .await
            .expect("fixture repair");
        sqlx::query("UPDATE callback_lookup_receipts SET snapshot=$1,recorded_at=1500")
            .bind(serde_json::to_value(&source.snapshot).expect("exact original"))
            .execute(&mut *tx)
            .await
            .expect("restore original receipt");
        tx.execute("ALTER TABLE callback_lookup_receipts ENABLE TRIGGER USER")
            .await
            .expect("guard restored");
        tx.execute("ALTER TABLE callback_processing_events DISABLE TRIGGER USER")
            .await
            .expect("fixture repair");
        sqlx::query("UPDATE callback_processing_events SET payload=$1,created_at=$2 WHERE id=$3")
            .bind(&payload)
            .bind(created_at)
            .bind(&event_id)
            .execute(&mut *tx)
            .await
            .expect("exact original event");
        tx.execute("ALTER TABLE callback_processing_events ENABLE TRIGGER USER")
            .await
            .expect("guard restored");
        tx.commit().await.expect("restored source");
        assert!(
            process_lookup(&pool, &BadPorts, &BadPorts, &op.snapshot().id)
                .await
                .expect("paired healthy historical replay")
                .snapshot
                == original_observation.snapshot
        );
    }
    assert_eq!(counts(&pool).await, (1, 1, 1, 1, 1));
    assert_eq!(server.count(), 2);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn receipt_recorded_after_restart_clock_cannot_create_an_older_observation_or_resolution(
    pool: PgPool,
) {
    let (clock, f) = prepared(&pool, true).await;
    let i = ids();
    admit(&pool, clock.as_ref(), &i, PROFILE, "tr_RECEIPT_CLOCK").await;
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, response(f.snapshot(), "tr_RECEIPT_CLOCK", "open")),
    ]);
    let c = client(&server).await;
    let mut claim = store::jobs::claim(&pool, clock.as_ref(), &i, PROFILE)
        .await
        .expect("claim")
        .expect("pending");
    let fetched = c
        .fetch_lookup(claim.take_permit().expect("one-use start"))
        .await
        .expect("authenticated OPEN");
    clock.0.store(1501, Ordering::SeqCst);
    record_lookup(&pool, clock.as_ref(), &i, &claim.operation_id, &fetched)
        .await
        .expect("receipt recorded one second later");
    clock.0.store(1500, Ordering::SeqCst);
    assert_eq!(
        process_lookup(&pool, clock.as_ref(), &i, &claim.operation_id)
            .await
            .err(),
        Some(PaymentStoreError::ClockUnavailable)
    );
    assert_eq!(
        run_callback_batch(&pool, clock.as_ref(), &i, &c, 1)
            .await
            .err(),
        Some(PaymentStoreError::ClockUnavailable)
    );
    assert_eq!(sqlx::query_as::<_,(i64,i64,i64)>("SELECT (SELECT count(*) FROM payment_observations),(SELECT count(*) FROM payment_identity_resolutions),(SELECT count(*) FROM callback_receipt_processing)").fetch_one(&pool).await.expect("no effects before receipt clock"),(0,0,0));
    clock.0.store(1501, Ordering::SeqCst);
    store::drain::process_operation(&pool, clock.as_ref(), &i, &claim.operation_id)
        .await
        .expect("healthy restored clock");
    assert!(
        process_lookup(&pool, &BadPorts, &BadPorts, &claim.operation_id)
            .await
            .expect("historical receipt replay needs no fresh clock or entropy")
            .replayed
    );
    store::jobs::finish(&pool, clock.as_ref(), &claim, JobState::Done, None)
        .await
        .expect("healthy checkpoint");
    assert_eq!(server.count(), 2);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn generation_insert_wait_expiry_or_cancellation_cannot_commit_partial_claim_or_permit(
    pool: PgPool,
) {
    let clock = time();
    let i = Arc::new(ids());
    let id = admit(&pool, clock.as_ref(), i.as_ref(), PROFILE, "tr_CLAIM_WAIT").await;
    for cancel in [false, true] {
        let mut blocker = pool.begin().await.expect("generation write blocker");
        blocker
            .execute("LOCK TABLE callback_inbox_generations IN SHARE MODE")
            .await
            .expect("real generation INSERT wait");
        let (p, t, i2) = (pool.clone(), clock.clone(), i.clone());
        let task =
            tokio::spawn(
                async move { store::jobs::claim(&p, t.as_ref(), i2.as_ref(), PROFILE).await },
            );
        waiting_query(&pool, "INSERT INTO callback_inbox_generations%").await;
        if cancel {
            task.abort();
            assert!(task.await.err().expect("cancelled claim").is_cancelled());
            blocker.commit().await.expect("release");
        } else {
            clock.0.store(1560, Ordering::SeqCst);
            blocker.commit().await.expect("release");
            assert!(
                task.await.expect("claim task").is_err(),
                "expired pre-I/O claim never yields a permit"
            );
        }
        assert_eq!(counts(&pool).await, (0, 0, 0, 0, 0));
        assert_eq!(
            read_callback_job(&pool, &id)
                .await
                .expect("original checkpoint")
                .expect("job")
                .generation,
            0
        );
        clock.0.store(1500, Ordering::SeqCst);
    }
    store::jobs::claim(&pool, clock.as_ref(), i.as_ref(), PROFILE)
        .await
        .expect("healthy claim")
        .expect("pending");
    assert_eq!(counts(&pool).await.0, 1);
}

async fn processing_wait(pool: &PgPool, cancel: bool) {
    let (clock, f) = prepared(pool, true).await;
    let i = Arc::new(ids());
    let id = admit(pool, clock.as_ref(), i.as_ref(), PROFILE, "tr_PROCESS_WAIT").await;
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, response(f.snapshot(), "tr_PROCESS_WAIT", "open")),
    ]);
    let c = Arc::new(client(&server).await);
    let mut blocker = pool.begin().await.expect("processing disposition blocker");
    blocker
        .execute("LOCK TABLE callback_receipt_processing IN SHARE MODE")
        .await
        .expect("actual disposition INSERT wait");
    let (p, t, i2, c2) = (pool.clone(), clock.clone(), i.clone(), c.clone());
    let task = tokio::spawn(async move {
        run_callback_batch(&p, t.as_ref(), i2.as_ref(), c2.as_ref(), 1).await
    });
    waiting_query(pool, "INSERT INTO callback_receipt_processing%").await;
    if cancel {
        task.abort();
        assert!(
            task.await
                .err()
                .expect("cancelled disposition")
                .is_cancelled()
        );
        blocker.commit().await.expect("release");
    } else {
        clock.0.store(1499, Ordering::SeqCst);
        blocker.commit().await.expect("release");
        assert_eq!(
            task.await.expect("supervisor task").err(),
            Some(PaymentStoreError::ClockUnavailable)
        );
    }
    assert_eq!(
        counts(pool).await,
        (1, 1, 1, 1, 1),
        "independently committed observation survives lost processing checkpoint"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM callback_receipt_processing")
            .fetch_one(pool)
            .await
            .expect("no partial disposition"),
        0
    );
    clock.0.store(1560, Ordering::SeqCst);
    let recovered = run_callback_batch(pool, clock.as_ref(), i.as_ref(), c.as_ref(), 1)
        .await
        .expect("recover saved receipt before another GET");
    assert_eq!(recovered.processed, 0);
    assert_eq!(server.count(), 2);
    assert_eq!(counts(pool).await, (1, 1, 1, 1, 1));
    assert!(
        read_callback_job(pool, &id)
            .await
            .expect("read")
            .expect("job")
            .settled
    );
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn disposition_write_clock_rewind_preserves_receipt_and_observation_for_exact_recovery(
    pool: PgPool,
) {
    processing_wait(&pool, false).await;
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn disposition_write_cancellation_preserves_receipt_and_observation_for_exact_recovery(
    pool: PgPool,
) {
    processing_wait(&pool, true).await;
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn checkpoint_wait_lease_expiry_preserves_all_facts_and_recovers_without_another_get(
    pool: PgPool,
) {
    let (clock, f) = prepared(&pool, true).await;
    let i = Arc::new(ids());
    let id = admit(&pool, clock.as_ref(), i.as_ref(), PROFILE, "tr_FINISH_WAIT").await;
    let gate = Arc::new(AtomicBool::new(false));
    let server = Server::start(vec![
        profile(),
        Reply::GateJson(
            gate.clone(),
            response(f.snapshot(), "tr_FINISH_WAIT", "open"),
        ),
    ]);
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
    let mut blocker = pool.begin().await.expect("current checkpoint blocker");
    sqlx::query("SELECT intake_id FROM callback_inbox_jobs WHERE intake_id=$1 FOR UPDATE")
        .bind(&id)
        .fetch_one(&mut *blocker)
        .await
        .expect("actual job row lock after GET began");
    gate.store(true, Ordering::SeqCst);
    waiting_query(&pool, "UPDATE callback_inbox_jobs SET state=%").await;
    clock.0.store(1560, Ordering::SeqCst);
    blocker.commit().await.expect("release checkpoint wait");
    let result = task.await.expect("supervisor task");
    assert!(
        result.is_ok() || result.err() == Some(PaymentStoreError::Conflict),
        "lease loss retains fetched facts and is either reported or handled"
    );
    assert_eq!(counts(&pool).await, (1, 1, 1, 1, 1));
    let recovered = run_callback_batch(&pool, clock.as_ref(), i.as_ref(), c.as_ref(), 1)
        .await
        .expect("recover committed disposition");
    assert_eq!(recovered.processed, 0);
    assert_eq!(server.count(), 2);
    assert!(
        read_callback_job(&pool, &id)
            .await
            .expect("read")
            .expect("job")
            .settled
    );
}
