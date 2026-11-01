//! Native and damaged-import controls paired with exact healthy recovery.
use super::*;

async fn waiting_callback(pool: &PgPool) {
    tokio::time::timeout(Duration::from_millis(800), async {
        loop {
            let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'INSERT INTO callback_processing_events%')")
                .fetch_one(pool).await.expect("actual callback event wait");
            if waiting { break; }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }).await.expect("callback processing INSERT reached actual lock");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn lookup_operations_validate_ports_identifiers_and_missing_custody_without_effects(
    pool: PgPool,
) {
    let clock = time();
    let i = ids();
    for (profile, id) in [
        ("", "tr_X"),
        ("pfl_", "tr_X"),
        (PROFILE, ""),
        (PROFILE, "tr_"),
        (PROFILE, "tr_bad\n"),
    ] {
        assert_eq!(
            prepare_lookup(&pool, clock.as_ref(), &i, profile, id)
                .await
                .err(),
            Some(PaymentStoreError::InvalidRequest)
        );
    }
    assert_eq!(
        prepare_lookup(&pool, &BadPorts, &i, PROFILE, "tr_X")
            .await
            .err(),
        Some(PaymentStoreError::ClockUnavailable)
    );
    assert!(
        prepare_lookup(&pool, clock.as_ref(), &BadPorts, PROFILE, "tr_X")
            .await
            .is_err()
    );
    for at in [9_007_199_254_740_992, u64::MAX] {
        clock.0.store(at, Ordering::SeqCst);
        assert_eq!(
            prepare_lookup(&pool, clock.as_ref(), &i, PROFILE, "tr_X")
                .await
                .err(),
            Some(PaymentStoreError::ClockUnavailable)
        );
    }
    for bad in ["", "invalid", &"g".repeat(32)] {
        assert_eq!(
            read_operation(&pool, bad).await.err(),
            Some(PaymentStoreError::InvalidRequest)
        );
        assert_eq!(
            read_lookup(&pool, bad).await.err(),
            Some(PaymentStoreError::InvalidRequest)
        );
        assert_eq!(
            process_lookup(&pool, clock.as_ref(), &i, bad).await.err(),
            Some(PaymentStoreError::InvalidRequest)
        );
    }
    assert_eq!(
        read_operation(&pool, &"f".repeat(32)).await.err(),
        Some(PaymentStoreError::NotFound)
    );
    clock.0.store(1500, Ordering::SeqCst);
    let op = operation(&pool, clock.as_ref(), &i, "tr_VALID").await;
    assert!(
        read_operation(&pool, &op.snapshot().id)
            .await
            .expect("healthy operation reader")
            .snapshot()
            == op.snapshot()
    );
    assert_eq!(
        process_lookup(&pool, clock.as_ref(), &i, &op.snapshot().id)
            .await
            .err(),
        Some(PaymentStoreError::Unavailable)
    );
    assert_eq!(counts(&pool).await, (1, 0, 0, 0, 0));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn lookup_fetch_releases_db_claims_before_io_and_cancellation_preserves_original_operation(
    pool: PgPool,
) {
    let (clock, f) = prepared(&pool, true).await;
    let i = Arc::new(ids());
    let op = operation(&pool, clock.as_ref(), i.as_ref(), "tr_IN_FLIGHT").await;
    let gate = Arc::new(AtomicBool::new(false));
    let server = Server::start(vec![profile(), Reply::Gate(gate)]);
    let c = Arc::new(client(&server).await);
    let (p, t, i2, c2, id) = (
        pool.clone(),
        clock.clone(),
        i.clone(),
        c.clone(),
        op.snapshot().id.clone(),
    );
    let task =
        tokio::spawn(
            async move { fetch_lookup(&p, t.as_ref(), i2.as_ref(), &id, c2.as_ref()).await },
        );
    tokio::time::timeout(Duration::from_secs(1), async {
        while server.count() < 2 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("payment GET reached");
    let mut tx = pool.begin().await.expect("independent DB claims");
    tx.execute("SET LOCAL lock_timeout='100ms'")
        .await
        .expect("bounded");
    sqlx::query("SELECT id FROM callback_lookup_operations WHERE id=$1 FOR UPDATE NOWAIT")
        .bind(&op.snapshot().id)
        .fetch_one(&mut *tx)
        .await
        .expect("no callback operation lock during GET");
    sqlx::query("SELECT id FROM payment_attempts WHERE id=$1 FOR UPDATE NOWAIT")
        .bind(&f.snapshot().binding.id)
        .fetch_one(&mut *tx)
        .await
        .expect("no original attempt lock during GET");
    tx.rollback().await.expect("release");
    task.abort();
    assert!(task.await.err().expect("cancelled").is_cancelled());
    assert_eq!(counts(&pool).await, (1, 0, 0, 1, 0));
    assert!(
        read_lookup(&pool, &op.snapshot().id)
            .await
            .expect("still pending")
            .is_none()
    );
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, response(f.snapshot(), "tr_IN_FLIGHT", "paid")),
    ]);
    let next = operation(&pool, clock.as_ref(), i.as_ref(), "tr_IN_FLIGHT").await;
    fetch_lookup(
        &pool,
        clock.as_ref(),
        i.as_ref(),
        &next.snapshot().id,
        &client(&server).await,
    )
    .await
    .expect("fresh server operation fetches after lost response");
    assert!(
        read_lookup(&pool, &op.snapshot().id)
            .await
            .expect("lost start remains pending")
            .is_none()
    );
    assert_eq!(counts(&pool).await, (2, 1, 1, 1, 0));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn operation_binding_wrong_profile_and_failed_ports_cannot_create_a_receipt(pool: PgPool) {
    let (_, f) = captured(38245);
    let clock = time();
    let i = ids();
    let op = operation(&pool, clock.as_ref(), &i, "tr_BOUND").await;
    let other = operation(&pool, clock.as_ref(), &i, "tr_BOUND").await;
    let foreign = prepare_lookup(&pool, clock.as_ref(), &i, "pfl_OTHER", "tr_BOUND")
        .await
        .expect("other profile operation");
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, response(f.snapshot(), "tr_BOUND", "paid")),
    ]);
    let c = client(&server).await;
    let permit = start_lookup(&pool, clock.as_ref(), &op)
        .await
        .expect("unique pre-I/O start");
    let fetched = c
        .fetch_lookup(permit)
        .await
        .expect("bound authenticated result");
    assert_eq!(
        record_lookup(&pool, clock.as_ref(), &i, &other.snapshot().id, &fetched)
            .await
            .err(),
        Some(PaymentStoreError::Conflict)
    );
    assert_eq!(
        fetch_lookup(&pool, clock.as_ref(), &i, &foreign.snapshot().id, &c)
            .await
            .err(),
        Some(MollieError::ProfileMismatch)
    );
    let permit = start_lookup(&pool, clock.as_ref(), &foreign)
        .await
        .expect("foreign immutable start");
    assert_eq!(
        c.fetch_lookup(permit).await.err(),
        Some(MollieError::ProfileMismatch)
    );
    assert_eq!(
        record_lookup(&pool, &BadPorts, &i, &op.snapshot().id, &fetched)
            .await
            .err(),
        Some(PaymentStoreError::ClockUnavailable)
    );
    assert!(
        record_lookup(
            &pool,
            clock.as_ref(),
            &BadPorts,
            &op.snapshot().id,
            &fetched
        )
        .await
        .is_err()
    );
    clock.0.store(1499, Ordering::SeqCst);
    assert!(
        record_lookup(&pool, clock.as_ref(), &i, &op.snapshot().id, &fetched)
            .await
            .is_err()
    );
    assert_eq!(counts(&pool).await, (3, 0, 0, 0, 0));
    clock.0.store(1500, Ordering::SeqCst);
    record_lookup(&pool, clock.as_ref(), &i, &op.snapshot().id, &fetched)
        .await
        .expect("paired healthy custody");
    assert_eq!(counts(&pool).await, (3, 1, 1, 0, 0));
    assert_eq!(server.count(), 2, "wrong profile never reaches transport");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn actual_event_wait_clock_rewind_and_cancellation_roll_back_receipt_and_event(pool: PgPool) {
    let (_, f) = captured(38245);
    let clock = time();
    let i = Arc::new(ids());
    for cancel in [false, true] {
        let op = operation(&pool, clock.as_ref(), i.as_ref(), "tr_WAIT_CUSTODY").await;
        let server = Server::start(vec![
            profile(),
            Reply::Json(200, response(f.snapshot(), "tr_WAIT_CUSTODY", "paid")),
        ]);
        let c = client(&server).await;
        let permit = start_lookup(&pool, clock.as_ref(), &op)
            .await
            .expect("unique start");
        let fetched = Arc::new(
            c.fetch_lookup(permit)
                .await
                .expect("authenticated operation"),
        );
        let mut blocker = pool.begin().await.expect("actual write blocker");
        blocker
            .execute("LOCK TABLE callback_processing_events IN SHARE MODE")
            .await
            .expect("receipt event INSERT waits");
        let (p, t, i2, v, id) = (
            pool.clone(),
            clock.clone(),
            i.clone(),
            fetched.clone(),
            op.snapshot().id.clone(),
        );
        let task = tokio::spawn(async move {
            record_lookup(&p, t.as_ref(), i2.as_ref(), &id, v.as_ref()).await
        });
        waiting_callback(&pool).await;
        if cancel {
            task.abort();
            assert!(task.await.err().expect("cancelled").is_cancelled());
            blocker.commit().await.expect("release insert wait");
        } else {
            clock.0.store(1499, Ordering::SeqCst);
            blocker.commit().await.expect("release insert wait");
            assert_eq!(
                task.await.expect("task").err(),
                Some(PaymentStoreError::ClockUnavailable)
            );
        }
        assert!(
            read_lookup(&pool, &op.snapshot().id)
                .await
                .expect("no partial receipt")
                .is_none()
        );
        clock.0.store(1500, Ordering::SeqCst);
        record_lookup(
            &pool,
            clock.as_ref(),
            i.as_ref(),
            &op.snapshot().id,
            fetched.as_ref(),
        )
        .await
        .expect("exact operation recovers");
    }
    assert_eq!(counts(&pool).await, (2, 2, 2, 0, 0));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn native_callback_history_is_immutable_and_receipts_require_exact_durable_events(
    pool: PgPool,
) {
    let (_, f) = captured(38245);
    let clock = time();
    let i = ids();
    let op = operation(&pool, clock.as_ref(), &i, "tr_NATIVE").await;
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, response(f.snapshot(), "tr_NATIVE", "paid")),
    ]);
    let c = client(&server).await;
    let first = fetch_lookup(&pool, clock.as_ref(), &i, &op.snapshot().id, &c)
        .await
        .expect("complete healthy custody");
    for query in [
        "UPDATE callback_lookup_operations SET created_at=created_at",
        "DELETE FROM callback_lookup_operations",
        "UPDATE callback_lookup_fetch_starts SET started_at=started_at",
        "DELETE FROM callback_lookup_fetch_starts",
        "UPDATE callback_lookup_receipts SET recorded_at=recorded_at",
        "DELETE FROM callback_lookup_receipts",
        "UPDATE callback_processing_events SET created_at=created_at",
        "DELETE FROM callback_processing_events",
    ] {
        let error = pool
            .execute(query)
            .await
            .expect_err("immutable callback history");
        assert_eq!(
            error.as_database_error().and_then(|e| e.code()).as_deref(),
            Some("23514"),
            "{query}"
        );
    }
    let next = operation(&pool, clock.as_ref(), &i, "tr_NATIVE").await;
    let _permit = start_lookup(&pool, clock.as_ref(), &next)
        .await
        .expect("native receipt requires pre-I/O custody");
    let mut snapshot = first.snapshot.clone();
    snapshot.id = "a".repeat(32);
    snapshot.operation = next.snapshot().clone();
    let mut tx = pool.begin().await.expect("native orphan control");
    sqlx::query("INSERT INTO callback_lookup_receipts(id,operation_id,recorded_at,snapshot) VALUES($1,$2,$3,$4)").bind(&snapshot.id).bind(&snapshot.operation.id).bind(snapshot.recorded_at).bind(serde_json::to_value(&snapshot).expect("closed snapshot")).execute(&mut *tx).await.expect("valid pending receipt");
    let error = tx.commit().await.expect_err("deferred event completeness");
    assert_eq!(
        error.as_database_error().and_then(|e| e.code()).as_deref(),
        Some("23514")
    );
    assert!(
        read_lookup(&pool, &next.snapshot().id)
            .await
            .expect("orphan rollback")
            .is_none()
    );
    assert_eq!(counts(&pool).await, (2, 1, 1, 0, 0));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn damaged_closed_snapshot_headers_and_event_imports_fail_read_and_processing_without_fallback(
    pool: PgPool,
) {
    let (_, f) = captured(38245);
    let clock = time();
    let i = ids();
    let op = operation(&pool, clock.as_ref(), &i, "tr_IMPORT").await;
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, response(f.snapshot(), "tr_IMPORT", "paid")),
    ]);
    let c = client(&server).await;
    let first = fetch_lookup(&pool, clock.as_ref(), &i, &op.snapshot().id, &c)
        .await
        .expect("healthy source");
    let original = serde_json::to_value(&first.snapshot).expect("source projection");
    let mut cases = Vec::new();
    let mut extra = original.clone();
    extra["unknown"] = true.into();
    cases.push(extra);
    let mut amount = original.clone();
    amount["payment"]["amount_minor"] = json!(0);
    cases.push(amount);
    let mut header = original.clone();
    header["id"] = "f".repeat(32).into();
    cases.push(header);
    let mut array = original.clone();
    array["payment"] = json!([
        "payment",
        "tr_IMPORT",
        PROFILE,
        "test",
        "EUR",
        38245,
        1,
        "2".repeat(32),
        "1".repeat(32),
        "paid"
    ]);
    cases.push(array);
    let mut op_array = original.clone();
    op_array["operation"] = json!([1, op.snapshot().id, PROFILE, "tr_IMPORT", 1500]);
    cases.push(op_array);
    for bad in cases {
        let mut tx = pool.begin().await.expect("isolated import damage");
        tx.execute("ALTER TABLE callback_lookup_receipts DISABLE TRIGGER USER")
            .await
            .expect("test fixture bypass");
        sqlx::query("UPDATE callback_lookup_receipts SET snapshot=$1")
            .bind(bad)
            .execute(&mut *tx)
            .await
            .expect("damaged import");
        tx.execute("ALTER TABLE callback_lookup_receipts ENABLE TRIGGER USER")
            .await
            .expect("restore guards");
        tx.commit().await.expect("import");
        assert_eq!(
            read_lookup(&pool, &op.snapshot().id).await.err(),
            Some(PaymentStoreError::InvalidStoredData)
        );
        assert_eq!(
            process_lookup(&pool, clock.as_ref(), &i, &op.snapshot().id)
                .await
                .err(),
            Some(PaymentStoreError::InvalidStoredData)
        );
        assert_eq!(
            fetch_lookup(&pool, clock.as_ref(), &i, &op.snapshot().id, &c)
                .await
                .err(),
            Some(MollieError::Custody(PaymentStoreError::InvalidStoredData))
        );
        let mut tx = pool.begin().await.expect("repair only isolated fixture");
        tx.execute("ALTER TABLE callback_lookup_receipts DISABLE TRIGGER USER")
            .await
            .expect("fixture repair");
        sqlx::query("UPDATE callback_lookup_receipts SET snapshot=$1")
            .bind(&original)
            .execute(&mut *tx)
            .await
            .expect("restore exact snapshot");
        tx.execute("ALTER TABLE callback_lookup_receipts ENABLE TRIGGER USER")
            .await
            .expect("restore guard");
        tx.commit().await.expect("repair");
        assert!(
            read_lookup(&pool, &op.snapshot().id)
                .await
                .expect("paired healthy reader")
                .expect("receipt")
                .snapshot
                == first.snapshot
        );
    }
    let (event_id, operation_id, receipt_id, created_at, kind, payload): (
        String,
        String,
        String,
        i64,
        String,
        Value,
    ) = sqlx::query_as(
        "SELECT id,operation_id,receipt_id,created_at,kind,payload FROM callback_processing_events",
    )
    .fetch_one(&pool)
    .await
    .expect("original event");
    let mut tx = pool.begin().await.expect("isolated missing event");
    tx.execute("ALTER TABLE callback_processing_events DISABLE TRIGGER USER")
        .await
        .expect("test import");
    tx.execute("DELETE FROM callback_processing_events")
        .await
        .expect("remove custody");
    tx.execute("ALTER TABLE callback_processing_events ENABLE TRIGGER USER")
        .await
        .expect("restore guard");
    tx.commit().await.expect("damaged fixture");
    assert_eq!(
        read_lookup(&pool, &op.snapshot().id).await.err(),
        Some(PaymentStoreError::InvalidStoredData)
    );
    assert_eq!(
        process_lookup(&pool, clock.as_ref(), &i, &op.snapshot().id)
            .await
            .err(),
        Some(PaymentStoreError::InvalidStoredData)
    );
    sqlx::query("INSERT INTO callback_processing_events(id,operation_id,receipt_id,created_at,kind,payload) VALUES($1,$2,$3,$4,$5,$6)").bind(event_id).bind(operation_id).bind(receipt_id).bind(created_at).bind(kind).bind(payload).execute(&pool).await.expect("exact original custody restored");
    assert!(
        read_lookup(&pool, &op.snapshot().id)
            .await
            .expect("positive restored event")
            .expect("complete")
            .snapshot
            == first.snapshot
    );
    assert_eq!(
        server.count(),
        2,
        "invalid saved custody never triggers fallback GET"
    );
    assert_eq!(counts(&pool).await, (1, 1, 1, 0, 0));
}
