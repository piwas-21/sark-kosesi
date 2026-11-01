//! Concurrent claims, migration preservation and hostile native/import controls.
use super::*;
use sha2::{Digest, Sha256};

#[sqlx::test(migrator = "MIGRATOR")]
async fn a_coherently_changed_receipt_never_reconstructs_authenticated_authority_after_restart(
    pool: PgPool,
) {
    let (clock, f) = prepared(&pool, true).await;
    let i = ids();
    let op = operation(&pool, clock.as_ref(), &i, "tr_REPLAY_INTENT").await;
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, response(f.snapshot(), "tr_REPLAY_INTENT", "open")),
    ]);
    let c = client(&server).await;
    let permit = start_lookup(&pool, clock.as_ref(), &op)
        .await
        .expect("unique original start");
    let fetched = c
        .fetch_lookup(permit)
        .await
        .expect("authenticated original OPEN");
    let first = record_lookup(&pool, clock.as_ref(), &i, &op.snapshot().id, &fetched)
        .await
        .expect("original receipt");
    assert!(matches!(first.snapshot.payment.status, PaymentStatus::Open));
    let canonical: String =
        sqlx::query_scalar("SELECT snapshot::text FROM callback_lookup_receipts")
            .fetch_one(&pool)
            .await
            .expect("canonical persisted JSONB");
    let independently_hashed = format!("{:x}", Sha256::digest(canonical.as_bytes()));
    let recorded_hash: String =
        sqlx::query_scalar("SELECT payload->>'snapshot_sha256' FROM callback_processing_events")
            .fetch_one(&pool)
            .await
            .expect("independent immutable event binding");
    assert_eq!(
        recorded_hash, independently_hashed,
        "SQL canonical SHA-256 agrees with a separate implementation"
    );
    for (path, value) in [
        ("/payment/status", json!("paid")),
        ("/payment/amount_minor", json!(38244)),
        ("/credential_fingerprint", json!("b".repeat(64))),
    ] {
        let mut changed = serde_json::to_value(&first.snapshot).expect("exact original");
        *changed.pointer_mut(path).expect("literal field") = value;
        let mut tx = pool.begin().await.expect("isolated coherent import damage");
        tx.execute("ALTER TABLE callback_lookup_receipts DISABLE TRIGGER USER")
            .await
            .expect("test-only immutable bypass");
        sqlx::query("UPDATE callback_lookup_receipts SET snapshot=$1")
            .bind(changed)
            .execute(&mut *tx)
            .await
            .expect("different otherwise valid fact");
        tx.execute("ALTER TABLE callback_lookup_receipts ENABLE TRIGGER USER")
            .await
            .expect("guard restored");
        tx.commit().await.expect("import fixture");
        assert_eq!(
            read_lookup(&pool, &op.snapshot().id).await.err(),
            Some(PaymentStoreError::InvalidStoredData),
            "{path}"
        );
        assert_eq!(
            process_lookup(&pool, clock.as_ref(), &i, &op.snapshot().id)
                .await
                .err(),
            Some(PaymentStoreError::InvalidStoredData),
            "{path}"
        );
        assert_eq!(
            fetch_lookup(&pool, clock.as_ref(), &i, &op.snapshot().id, &c)
                .await
                .err(),
            Some(MollieError::Custody(PaymentStoreError::InvalidStoredData)),
            "{path}"
        );
        assert_eq!(
            record_lookup(&pool, clock.as_ref(), &i, &op.snapshot().id, &fetched)
                .await
                .err(),
            Some(PaymentStoreError::InvalidStoredData),
            "{path}"
        );
        assert_eq!(counts(&pool).await, (1, 1, 1, 1, 0));
        assert_eq!(
            server.count(),
            2,
            "saved corruption cannot trigger a fallback GET"
        );
        let mut tx = pool.begin().await.expect("exact fixture repair");
        tx.execute("ALTER TABLE callback_lookup_receipts DISABLE TRIGGER USER")
            .await
            .expect("test repair");
        sqlx::query("UPDATE callback_lookup_receipts SET snapshot=$1")
            .bind(serde_json::to_value(&first.snapshot).expect("exact source"))
            .execute(&mut *tx)
            .await
            .expect("restore original OPEN fact");
        tx.execute("ALTER TABLE callback_lookup_receipts ENABLE TRIGGER USER")
            .await
            .expect("guard restored");
        tx.commit().await.expect("repaired fixture");
        let restored = record_lookup(&pool, &BadPorts, &BadPorts, &op.snapshot().id, &fetched)
            .await
            .expect("paired healthy exact replay");
        assert!(restored.replayed && restored.snapshot == first.snapshot);
    }
    let observed = process_lookup(&pool, clock.as_ref(), &i, &op.snapshot().id)
        .await
        .expect("only original independently bound OPEN fact is processable");
    assert!(matches!(
        observed.snapshot.intent.payment.status,
        PaymentStatus::Open
    ));
    assert_eq!(counts(&pool).await, (1, 1, 1, 1, 1));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn a_saved_lookup_cannot_acquire_authority_from_a_genuinely_later_dispatch(pool: PgPool) {
    let (clock, f) = prepared(&pool, false).await;
    let i = ids();
    let early = operation(&pool, clock.as_ref(), &i, "tr_CHRONOLOGY").await;
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, response(f.snapshot(), "tr_CHRONOLOGY", "paid")),
        Reply::Json(200, response(f.snapshot(), "tr_CHRONOLOGY", "paid")),
    ]);
    let c = client(&server).await;
    let saved = fetch_lookup(&pool, clock.as_ref(), &i, &early.snapshot().id, &c)
        .await
        .expect("authenticated early fact remains durable");
    clock.0.store(1501, Ordering::SeqCst);
    crate::payment_attempts::start_dispatch(
        &pool,
        clock.as_ref(),
        &i,
        &f.snapshot().binding.id,
        &config(&credential()),
    )
    .await
    .expect("actual later dispatch");
    assert_eq!(
        process_lookup(&pool, clock.as_ref(), &i, &early.snapshot().id)
            .await
            .err(),
        Some(PaymentStoreError::Conflict)
    );
    assert_eq!(counts(&pool).await, (1, 1, 1, 1, 0));
    assert!(
        read_lookup(&pool, &early.snapshot().id)
            .await
            .expect("unmatched receipt remains")
            .expect("present")
            .snapshot
            == saved.snapshot
    );
    let later = operation(&pool, clock.as_ref(), &i, "tr_CHRONOLOGY").await;
    fetch_lookup(&pool, clock.as_ref(), &i, &later.snapshot().id, &c)
        .await
        .expect("new independently authenticated fact");
    process_lookup(&pool, clock.as_ref(), &i, &later.snapshot().id)
        .await
        .expect("paired healthy dispatch chronology");
    assert_eq!(counts(&pool).await, (2, 2, 2, 1, 1));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn concurrent_helpers_commit_one_get_start_and_never_refetch_an_unresolved_operation(
    pool: PgPool,
) {
    let (_, f) = captured(38245);
    let clock = time();
    let i = Arc::new(ids());
    let op = operation(&pool, clock.as_ref(), i.as_ref(), "tr_ONCE").await;
    let gate = Arc::new(AtomicBool::new(false));
    let server = Server::start(vec![
        profile(),
        Reply::GateJson(gate.clone(), response(f.snapshot(), "tr_ONCE", "paid")),
    ]);
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
    .expect("first GET outstanding");
    assert_eq!(
        fetch_lookup(
            &pool,
            clock.as_ref(),
            i.as_ref(),
            &op.snapshot().id,
            c.as_ref()
        )
        .await
        .err(),
        Some(MollieError::Custody(PaymentStoreError::Unavailable))
    );
    assert_eq!(server.count(), 2, "competitor cannot run another GET");
    gate.store(true, Ordering::SeqCst);
    task.await.expect("task").expect("original GET custody");
    assert!(
        fetch_lookup(&pool, &BadPorts, &BadPorts, &op.snapshot().id, c.as_ref())
            .await
            .expect("receipt after restart")
            .replayed
    );
    assert_eq!(server.count(), 2);
    let starts: i64 = sqlx::query_scalar("SELECT count(*) FROM callback_lookup_fetch_starts")
        .fetch_one(&pool)
        .await
        .expect("starts");
    assert_eq!(starts, 1);
    assert_eq!(counts(&pool).await, (1, 1, 1, 0, 0));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn nondefault_repeatable_read_claim_waits_still_replay_both_receipt_writers(pool: PgPool) {
    let (_, f) = captured(38245);
    let clock = time();
    let i = Arc::new(ids());
    let op = operation(&pool, clock.as_ref(), i.as_ref(), "tr_RR").await;
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, response(f.snapshot(), "tr_RR", "paid")),
    ]);
    let c = client(&server).await;
    let permit = start_lookup(&pool, clock.as_ref(), &op)
        .await
        .expect("single start");
    let fetched = Arc::new(c.fetch_lookup(permit).await.expect("authenticated"));
    let rr = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .min_connections(2)
        .after_connect(|connection, _| {
            Box::pin(async move {
                sqlx::query("SET default_transaction_isolation='repeatable read'")
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect_with((*pool.connect_options()).clone())
        .await
        .expect("owned RR pool");
    assert_eq!(
        sqlx::query_scalar::<_, String>("SHOW default_transaction_isolation")
            .fetch_one(&rr)
            .await
            .expect("configured policy"),
        "repeatable read"
    );
    let mut blocker = pool.begin().await.expect("actual operation claim blocker");
    sqlx::query("SELECT id FROM callback_lookup_operations WHERE id=$1 FOR UPDATE")
        .bind(&op.snapshot().id)
        .fetch_one(&mut *blocker)
        .await
        .expect("locked");
    let mut tasks = Vec::new();
    for _ in 0..2 {
        let (p, t, i, v, id) = (
            rr.clone(),
            clock.clone(),
            i.clone(),
            fetched.clone(),
            op.snapshot().id.clone(),
        );
        tasks.push(tokio::spawn(async move {
            record_lookup(&p, t.as_ref(), i.as_ref(), &id, v.as_ref()).await
        }));
    }
    tokio::time::timeout(Duration::from_millis(800),async {loop {let waiting:i64=sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'SELECT left(id,33)%callback_lookup_operations%FOR UPDATE'").fetch_one(&pool).await.expect("both claim snapshots overlap");if waiting==2 {break;}tokio::time::sleep(Duration::from_millis(5)).await;}}).await.expect("both writers blocked on real claim");
    blocker.commit().await.expect("release contenders");
    let mut receipts = Vec::new();
    for task in tasks {
        receipts.push(
            task.await
                .expect("task")
                .expect("read committed claim refresh"),
        );
    }
    assert_eq!(receipts.iter().filter(|r| !r.replayed).count(), 1);
    assert!(receipts[0].snapshot == receipts[1].snapshot);
    rr.close().await;
    assert_eq!(counts(&pool).await, (1, 1, 1, 0, 0));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn native_receipt_and_event_guards_bind_every_projection_before_complete_positive_commit(
    pool: PgPool,
) {
    let (_, f) = captured(38245);
    let clock = time();
    let i = ids();
    let op = operation(&pool, clock.as_ref(), &i, "tr_SQL_BINDING").await;
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, response(f.snapshot(), "tr_SQL_BINDING", "paid")),
    ]);
    let c = client(&server).await;
    let source = fetch_lookup(&pool, clock.as_ref(), &i, &op.snapshot().id, &c)
        .await
        .expect("healthy source");
    let next = operation(&pool, clock.as_ref(), &i, "tr_SQL_BINDING").await;
    let permit = start_lookup(&pool, clock.as_ref(), &next)
        .await
        .expect("pre-I/O start");
    let mut s = source.snapshot.clone();
    s.id = "a".repeat(32);
    s.operation = next.snapshot().clone();
    let good = serde_json::to_value(&s).expect("closed");
    for (path, value) in [
        ("/schema_version", json!(2)),
        ("/id", json!("b".repeat(32))),
        ("/operation/profile_id", json!("pfl_OTHER")),
        ("/started_at", json!(1499)),
        ("/recorded_at", json!(1499)),
        ("/credential_fingerprint", json!("invalid")),
        ("/payment/resource", json!("refund")),
        ("/payment/id", json!("tr_OTHER")),
        ("/payment/profile_id", json!("pfl_OTHER")),
        ("/payment/mode", json!("live")),
        ("/payment/currency", json!("USD")),
        ("/payment/metadata_schema_version", json!(2)),
        ("/payment/order_id", json!("bad")),
        ("/payment/attempt_id", json!("bad")),
        ("/payment/amount_minor", json!(0)),
        ("/payment/amount_minor", json!(9_007_199_254_740_992i64)),
        ("/payment/status", json!("unknown")),
    ] {
        let mut bad = good.clone();
        *bad.pointer_mut(path).expect("literal path") = value;
        let mut tx = pool.begin().await.expect("isolated native control");
        let error=sqlx::query("INSERT INTO callback_lookup_receipts(id,operation_id,recorded_at,snapshot) VALUES($1,$2,1500,$3)").bind(&s.id).bind(&s.operation.id).bind(bad).execute(&mut *tx).await.expect_err("false native projection rejected");
        assert_eq!(
            error.as_database_error().and_then(|e| e.code()).as_deref(),
            Some("23514"),
            "{path}"
        );
        tx.rollback().await.expect("isolated rollback");
    }
    for field in [
        "operation",
        "receipt",
        "created",
        "kind",
        "payload",
        "digest",
    ] {
        let mut tx = pool.begin().await.expect("valid pending receipt");
        sqlx::query("INSERT INTO callback_lookup_receipts(id,operation_id,recorded_at,snapshot) VALUES($1,$2,1500,$3)").bind(&s.id).bind(&s.operation.id).bind(&good).execute(&mut *tx).await.expect("valid minimal source");
        let operation_id = if field == "operation" {
            op.snapshot().id.clone()
        } else {
            s.operation.id.clone()
        };
        let receipt_id = if field == "receipt" {
            source.snapshot.id.clone()
        } else {
            s.id.clone()
        };
        let created = if field == "created" { 1501 } else { 1500 };
        let kind = if field == "kind" {
            "wrong.v1"
        } else {
            "payment.callback_lookup_recorded.v1"
        };
        let canonical: String =
            sqlx::query_scalar("SELECT snapshot::text FROM callback_lookup_receipts WHERE id=$1")
                .bind(&s.id)
                .fetch_one(&mut *tx)
                .await
                .expect("native persisted snapshot");
        let snapshot_sha256 = if field == "digest" {
            "f".repeat(64)
        } else {
            format!("{:x}", Sha256::digest(canonical.as_bytes()))
        };
        let payload = if field == "payload" {
            json!({"schema_version":2,"operation_id":s.operation.id,"receipt_id":s.id,"snapshot_sha256":snapshot_sha256})
        } else {
            json!({"schema_version":1,"operation_id":s.operation.id,"receipt_id":s.id,"snapshot_sha256":snapshot_sha256})
        };
        let error=sqlx::query("INSERT INTO callback_processing_events(id,operation_id,receipt_id,created_at,kind,payload) VALUES($1,$2,$3,$4,$5,$6)").bind("b".repeat(32)).bind(operation_id).bind(receipt_id).bind(created).bind(kind).bind(payload).execute(&mut *tx).await.expect_err("false native processing event rejected");
        assert_eq!(
            error.as_database_error().and_then(|e| e.code()).as_deref(),
            Some("23514"),
            "{field}"
        );
        tx.rollback().await.expect("rollback paired control");
    }
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, response(f.snapshot(), "tr_SQL_BINDING", "paid")),
    ]);
    let fetched = client(&server)
        .await
        .fetch_lookup(permit)
        .await
        .expect("one unused permit");
    record_lookup(&pool, clock.as_ref(), &i, &next.snapshot().id, &fetched)
        .await
        .expect("paired complete positive");
    assert_eq!(counts(&pool).await, (2, 2, 2, 0, 0));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn imported_size_start_operation_and_header_damage_fail_closed_until_exact_restoration(
    pool: PgPool,
) {
    let (_, f) = captured(38245);
    let clock = time();
    let i = ids();
    let op = operation(&pool, clock.as_ref(), &i, "tr_HEADERS").await;
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, response(f.snapshot(), "tr_HEADERS", "paid")),
    ]);
    let c = client(&server).await;
    let source = fetch_lookup(&pool, clock.as_ref(), &i, &op.snapshot().id, &c)
        .await
        .expect("complete healthy source");
    for field in ["size", "start", "operation", "recorded", "event"] {
        let mut tx = pool.begin().await.expect("isolated imported damage");
        match field {
            "size" => {
                tx.execute("ALTER TABLE callback_lookup_receipts DISABLE TRIGGER USER")
                    .await
                    .expect("fixture bypass");
                tx.execute("ALTER TABLE callback_lookup_receipts DROP CONSTRAINT callback_lookup_receipts_snapshot_check").await.expect("test size bypass");
                tx.execute("UPDATE callback_lookup_receipts SET snapshot=snapshot||jsonb_build_object('padding',repeat('x',8193))").await.expect("oversized imported snapshot");
                tx.execute("ALTER TABLE callback_lookup_receipts ENABLE TRIGGER USER")
                    .await
                    .expect("guard restored");
            }
            "start" => {
                tx.execute("ALTER TABLE callback_lookup_fetch_starts DISABLE TRIGGER USER")
                    .await
                    .expect("fixture bypass");
                tx.execute("UPDATE callback_lookup_fetch_starts SET started_at=1501")
                    .await
                    .expect("mismatched native start");
                tx.execute("ALTER TABLE callback_lookup_fetch_starts ENABLE TRIGGER USER")
                    .await
                    .expect("guard restored");
            }
            "operation" => {
                tx.execute("ALTER TABLE callback_lookup_operations DISABLE TRIGGER USER")
                    .await
                    .expect("fixture bypass");
                tx.execute("UPDATE callback_lookup_operations SET profile_id='pfl_OTHER'")
                    .await
                    .expect("coherent but rebound operation");
                tx.execute("ALTER TABLE callback_lookup_operations ENABLE TRIGGER USER")
                    .await
                    .expect("guard restored");
            }
            "recorded" => {
                tx.execute("ALTER TABLE callback_lookup_receipts DISABLE TRIGGER USER")
                    .await
                    .expect("fixture bypass");
                tx.execute("UPDATE callback_lookup_receipts SET recorded_at=1501")
                    .await
                    .expect("header mismatch");
                tx.execute("ALTER TABLE callback_lookup_receipts ENABLE TRIGGER USER")
                    .await
                    .expect("guard restored");
            }
            _ => {
                tx.execute("ALTER TABLE callback_processing_events DISABLE TRIGGER USER")
                    .await
                    .expect("fixture bypass");
                tx.execute("UPDATE callback_processing_events SET payload=jsonb_set(payload,'{schema_version}','2')").await.expect("event mismatch");
                tx.execute("ALTER TABLE callback_processing_events ENABLE TRIGGER USER")
                    .await
                    .expect("guard restored");
            }
        }
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
        let mut tx = pool.begin().await.expect("exact isolated restoration");
        match field {
            "size" | "recorded" => {
                tx.execute("ALTER TABLE callback_lookup_receipts DISABLE TRIGGER USER")
                    .await
                    .expect("fixture repair");
                sqlx::query("UPDATE callback_lookup_receipts SET snapshot=$1,recorded_at=1500")
                    .bind(serde_json::to_value(&source.snapshot).expect("exact source"))
                    .execute(&mut *tx)
                    .await
                    .expect("restore source");
                if field == "size" {
                    tx.execute("ALTER TABLE callback_lookup_receipts ADD CONSTRAINT callback_lookup_receipts_snapshot_check CHECK(jsonb_typeof(snapshot)='object' AND octet_length(snapshot::text)<=8192)").await.expect("restore exact size check");
                }
                tx.execute("ALTER TABLE callback_lookup_receipts ENABLE TRIGGER USER")
                    .await
                    .expect("guard restored");
            }
            "start" => {
                tx.execute("ALTER TABLE callback_lookup_fetch_starts DISABLE TRIGGER USER")
                    .await
                    .expect("fixture repair");
                tx.execute("UPDATE callback_lookup_fetch_starts SET started_at=1500")
                    .await
                    .expect("exact start");
                tx.execute("ALTER TABLE callback_lookup_fetch_starts ENABLE TRIGGER USER")
                    .await
                    .expect("guard restored");
            }
            "operation" => {
                tx.execute("ALTER TABLE callback_lookup_operations DISABLE TRIGGER USER")
                    .await
                    .expect("fixture repair");
                sqlx::query("UPDATE callback_lookup_operations SET profile_id=$1")
                    .bind(PROFILE)
                    .execute(&mut *tx)
                    .await
                    .expect("exact scope");
                tx.execute("ALTER TABLE callback_lookup_operations ENABLE TRIGGER USER")
                    .await
                    .expect("guard restored");
            }
            _ => {
                tx.execute("ALTER TABLE callback_processing_events DISABLE TRIGGER USER")
                    .await
                    .expect("fixture repair");
                tx.execute("UPDATE callback_processing_events SET payload=jsonb_set(payload,'{schema_version}','1')").await.expect("exact payload");
                tx.execute("ALTER TABLE callback_processing_events ENABLE TRIGGER USER")
                    .await
                    .expect("guard restored");
            }
        }
        tx.commit().await.expect("repaired fixture");
        assert!(
            read_lookup(&pool, &op.snapshot().id)
                .await
                .expect("paired healthy read")
                .expect("source")
                .snapshot
                == source.snapshot
        );
    }
    assert_eq!(server.count(), 2);
}

#[sqlx::test(migrations = false)]
async fn schema28_upgrade_preserves_original_payment_custody_and_installs_no_lookup_or_authority(
    pool: PgPool,
) {
    MIGRATOR
        .run_to(28, &pool)
        .await
        .expect("accepted previous schema");
    let (_, f) = prepared(&pool, true).await;
    let before = crate::payment_test_fixture::counts(&pool).await;
    let grants: Vec<(String, String)> = sqlx::query_as(
        "SELECT staff_id,capability FROM staff_capabilities ORDER BY staff_id,capability",
    )
    .fetch_all(&pool)
    .await
    .expect("original authority");
    MIGRATOR.run(&pool).await.expect("upgrade");
    MIGRATOR.run(&pool).await.expect("idempotent upgrade");
    assert_eq!(crate::payment_test_fixture::counts(&pool).await, before);
    assert_eq!(counts(&pool).await, (0, 0, 0, 1, 0));
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT id FROM payment_attempts")
            .fetch_one(&pool)
            .await
            .expect("original identity"),
        f.snapshot().binding.id
    );
    assert_eq!(
        sqlx::query_as::<_, (String, String)>(
            "SELECT staff_id,capability FROM staff_capabilities ORDER BY staff_id,capability"
        )
        .fetch_all(&pool)
        .await
        .expect("unchanged authority"),
        grants
    );
}
