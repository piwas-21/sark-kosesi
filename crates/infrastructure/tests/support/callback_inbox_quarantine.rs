//! A saved success cannot hide damaged proof or block another original order.
use super::*;

async fn saved_disposition_damage(pool: &PgPool, field: &str) {
    let (clock, original) = prepared(pool, true).await;
    let i = ids();
    let other =
        super::super::super::identity_ownership::additional(pool, clock.clone(), PROFILE).await;
    let bad = admit(pool, clock.as_ref(), &i, PROFILE, "tr_SAVED_BAD").await;
    let mut claim = store::jobs::claim(pool, clock.as_ref(), &i, PROFILE)
        .await
        .expect("original claim")
        .expect("pending intake");
    assert!(
        sqlx::query(
            "INSERT INTO callback_processing_quarantine(operation_id,recorded_at) VALUES($1,1500)"
        )
        .bind(&claim.operation_id)
        .execute(pool)
        .await
        .is_err(),
        "no quarantine premarker before receipt or processing"
    );
    let original_server = Server::start(vec![
        profile(),
        Reply::Json(200, response(original.snapshot(), "tr_SAVED_BAD", "open")),
    ]);
    let original_client = client(&original_server).await;
    let fetched = original_client
        .fetch_lookup(claim.take_permit().expect("one GET permit"))
        .await
        .expect("authenticated original OPEN");
    record_lookup(pool, clock.as_ref(), &i, &claim.operation_id, &fetched)
        .await
        .expect("immutable receipt");
    assert!(
        store::drain::process_operation(pool, clock.as_ref(), &i, &claim.operation_id)
            .await
            .expect("processing commits before checkpoint")
            == store::drain::Disposition::Observed
    );
    assert!(
        read_callback_job(pool, &bad)
            .await
            .expect("job")
            .expect("exists")
            .state
            == JobState::Leased
    );
    let saved: Value = sqlx::query_scalar(
        "SELECT to_jsonb(p) FROM callback_receipt_processing p WHERE operation_id=$1",
    )
    .bind(&claim.operation_id)
    .fetch_one(pool)
    .await
    .expect("original processing row");
    let (observation, resolution): (String, String) = sqlx::query_as("SELECT observation_id,resolution_id FROM callback_receipt_processing WHERE operation_id=$1")
        .bind(&claim.operation_id).fetch_one(pool).await.expect("genuine proof identities");
    let receipt: Value =
        sqlx::query_scalar("SELECT snapshot FROM callback_lookup_receipts WHERE operation_id=$1")
            .bind(&claim.operation_id)
            .fetch_one(pool)
            .await
            .expect("exact receipt");
    let observation_snapshot: Value =
        sqlx::query_scalar("SELECT snapshot FROM payment_observations WHERE id=$1")
            .bind(&observation)
            .fetch_one(pool)
            .await
            .expect("exact observation");
    let resolution_summary: Value =
        sqlx::query_scalar("SELECT summary FROM payment_identity_resolutions WHERE id=$1")
            .bind(&resolution)
            .fetch_one(pool)
            .await
            .expect("exact resolution");
    let (table, damage, repair, identity, snapshot): (
        &str,
        &'static str,
        &'static str,
        &str,
        &Value,
    ) = match field {
        "receipt" => (
            "callback_lookup_receipts",
            "UPDATE callback_lookup_receipts SET snapshot=jsonb_set(snapshot,'{payment,status}','\"paid\"') WHERE operation_id=$1",
            "UPDATE callback_lookup_receipts SET snapshot=$2 WHERE operation_id=$1",
            &claim.operation_id,
            &receipt,
        ),
        "observation" => (
            "payment_observations",
            "UPDATE payment_observations SET status='paid',snapshot=jsonb_set(snapshot,'{intent,payment,status}','\"paid\"') WHERE id=$1",
            "UPDATE payment_observations SET status='open',snapshot=$2 WHERE id=$1",
            &observation,
            &observation_snapshot,
        ),
        "resolution" => (
            "payment_identity_resolutions",
            "UPDATE payment_identity_resolutions SET summary=jsonb_set(summary,'{paid_provider_ids}','[\"tr_SAVED_BAD\"]') WHERE id=$1",
            "UPDATE payment_identity_resolutions SET summary=$2 WHERE id=$1",
            &resolution,
            &resolution_summary,
        ),
        _ => panic!("unknown fixture case"),
    };
    let mut tx = pool.begin().await.expect("isolated damaged import");
    // Table names are the three closed literals immediately above; only this
    // disposable database bypasses immutable native guards for corruption probes.
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "ALTER TABLE {table} DISABLE TRIGGER USER"
    )))
    .execute(&mut *tx)
    .await
    .expect("fixture bypass");
    sqlx::query(damage)
        .bind(identity)
        .execute(&mut *tx)
        .await
        .expect("one proof damaged");
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "ALTER TABLE {table} ENABLE TRIGGER USER"
    )))
    .execute(&mut *tx)
    .await
    .expect("native guards restored");
    tx.commit().await.expect("damaged import retained");
    assert_eq!(
        store::drain::read_disposition(pool, &claim.operation_id)
            .await
            .err(),
        Some(PaymentStoreError::InvalidStoredData)
    );
    assert!(
        sqlx::query(
            "INSERT INTO callback_processing_quarantine(operation_id,recorded_at) VALUES($1,1499)"
        )
        .bind(&claim.operation_id)
        .execute(pool)
        .await
        .is_err(),
        "native quarantine cannot precede original source"
    );

    clock.0.store(1560, Ordering::SeqCst);
    let good = admit(pool, clock.as_ref(), &i, PROFILE, "tr_UNRELATED_GOOD").await;
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, response(other.snapshot(), "tr_UNRELATED_GOOD", "open")),
    ]);
    let c = client(&server).await;
    let batch = run_callback_batch(pool, clock.as_ref(), &i, &c, 1)
        .await
        .expect("saved invalid success quarantined without blocking unrelated work");
    assert_eq!(batch.processed, 1);
    assert_eq!(batch.needs_review, 1);
    assert_eq!(batch.retry_pending, 0);
    assert!(
        read_callback_job(pool, &good)
            .await
            .expect("healthy successor")
            .expect("job")
            .settled
    );
    let job = read_callback_job(pool, &bad)
        .await
        .expect("bad job still visible")
        .expect("job");
    assert!(job.state == JobState::NeedsReview);
    assert!(job.review_reason == Some(ReviewReason::CustodyInvalid));
    assert!(!job.settled);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM callback_inbox_active WHERE intake_id=$1"
        )
        .bind(&bad)
        .fetch_one(pool)
        .await
        .expect("retained admission slot"),
        1
    );
    assert!(
        store::drain::read_disposition(pool, &claim.operation_id)
            .await
            .expect("source-only visible exception")
            == Some(store::drain::Disposition::CustodyInvalid)
    );
    assert_eq!(
        sqlx::query_scalar::<_, Value>(
            "SELECT to_jsonb(p) FROM callback_receipt_processing p WHERE operation_id=$1"
        )
        .bind(&claim.operation_id)
        .fetch_one(pool)
        .await
        .expect("unchanged historical success"),
        saved
    );
    let quarantine: Value = sqlx::query_scalar(
        "SELECT to_jsonb(q) FROM callback_processing_quarantine q WHERE operation_id=$1",
    )
    .bind(&claim.operation_id)
    .fetch_one(pool)
    .await
    .expect("independent immutable exception");
    assert_eq!(
        quarantine,
        serde_json::json!({"operation_id": claim.operation_id, "recorded_at": 1560})
    );
    assert!(
        sqlx::query(
            "UPDATE callback_processing_quarantine SET recorded_at=1561 WHERE operation_id=$1"
        )
        .bind(&claim.operation_id)
        .execute(pool)
        .await
        .is_err(),
        "quarantine cannot be rewritten"
    );
    assert!(
        sqlx::query("DELETE FROM callback_processing_quarantine WHERE operation_id=$1")
            .bind(&claim.operation_id)
            .execute(pool)
            .await
            .is_err(),
        "quarantine cannot be erased"
    );

    let mut tx = pool.begin().await.expect("paired exact source repair");
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "ALTER TABLE {table} DISABLE TRIGGER USER"
    )))
    .execute(&mut *tx)
    .await
    .expect("fixture repair");
    sqlx::query(repair)
        .bind(identity)
        .bind(snapshot)
        .execute(&mut *tx)
        .await
        .expect("exact original proof restored");
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "ALTER TABLE {table} ENABLE TRIGGER USER"
    )))
    .execute(&mut *tx)
    .await
    .expect("native guards restored");
    tx.commit().await.expect("source repaired");
    assert!(
        read_lookup(pool, &claim.operation_id)
            .await
            .expect("healthy original receipt after exact repair")
            .is_some()
    );
    for _ in 0..2 {
        let later = run_callback_batch(pool, clock.as_ref(), &i, &c, 25)
            .await
            .expect("unchanged later pass succeeds visibly");
        assert_eq!(
            (
                later.processed,
                later.drained,
                later.needs_review,
                later.retry_pending
            ),
            (0, 0, 1, 0)
        );
        assert!(
            !read_callback_job(pool, &bad)
                .await
                .expect("permanent review")
                .expect("job")
                .settled
        );
        assert!(
            store::drain::read_disposition(pool, &claim.operation_id)
                .await
                .expect("repair cannot erase quarantine")
                == Some(store::drain::Disposition::CustodyInvalid)
        );
    }
    assert_eq!(sqlx::query_as::<_, (i64, i64, i64)>("SELECT (SELECT count(*) FROM callback_lookup_operations),(SELECT count(*) FROM payment_observations),(SELECT count(*) FROM payment_identity_resolutions)")
        .fetch_one(pool).await.expect("no replacement GET or financial evidence"), (2, 2, 2));
    assert_eq!(original_server.count(), 2);
    assert_eq!(server.count(), 2);
    get_only(&original_server);
    get_only(&server);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn saved_observed_receipt_damage_is_quarantined_without_head_of_line_blocking(pool: PgPool) {
    saved_disposition_damage(&pool, "receipt").await;
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn saved_observed_observation_damage_is_quarantined_without_head_of_line_blocking(
    pool: PgPool,
) {
    saved_disposition_damage(&pool, "observation").await;
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn saved_observed_resolution_damage_is_quarantined_without_head_of_line_blocking(
    pool: PgPool,
) {
    saved_disposition_damage(&pool, "resolution").await;
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn quarantine_closed_source_and_clock_controls_retain_a_healthy_immutable_exception(
    pool: PgPool,
) {
    let (clock, _, op) = processing::open(&pool).await;
    assert_eq!(
        store::quarantine::read(&pool, "bad").await.err(),
        Some(PaymentStoreError::InvalidRequest)
    );
    assert_eq!(
        store::quarantine::persist(&pool, &BadPorts, "bad")
            .await
            .err(),
        Some(PaymentStoreError::InvalidRequest)
    );
    assert_eq!(
        store::quarantine::persist(&pool, &BadPorts, &op)
            .await
            .err(),
        Some(PaymentStoreError::ClockUnavailable)
    );
    clock.0.store(1500, Ordering::SeqCst);
    assert_eq!(
        store::quarantine::persist(&pool, clock.as_ref(), &op)
            .await
            .err(),
        Some(PaymentStoreError::ClockUnavailable)
    );
    assert!(
        !store::quarantine::read(&pool, &op)
            .await
            .expect("no exception before valid clock")
    );
    clock.0.store(1504, Ordering::SeqCst);
    store::quarantine::persist(&pool, clock.as_ref(), &op)
        .await
        .expect("healthy source-only exception");
    store::quarantine::persist(&pool, clock.as_ref(), &op)
        .await
        .expect("exact duplicate leaves immutable exception");
    let saved: Value = sqlx::query_scalar(
        "SELECT to_jsonb(p) FROM callback_receipt_processing p WHERE operation_id=$1",
    )
    .bind(&op)
    .fetch_one(&pool)
    .await
    .expect("original proof");
    let mut tx = pool
        .begin()
        .await
        .expect("isolated damaged exception header");
    tx.execute("ALTER TABLE callback_processing_quarantine DISABLE TRIGGER USER")
        .await
        .expect("fixture bypass");
    sqlx::query("UPDATE callback_processing_quarantine SET recorded_at=1499 WHERE operation_id=$1")
        .bind(&op)
        .execute(&mut *tx)
        .await
        .expect("impossible imported chronology");
    tx.execute("ALTER TABLE callback_processing_quarantine ENABLE TRIGGER USER")
        .await
        .expect("guard restored");
    tx.commit().await.expect("import");
    assert_eq!(
        store::quarantine::read(&pool, &op).await.err(),
        Some(PaymentStoreError::InvalidStoredData)
    );
    let mut tx = pool
        .begin()
        .await
        .expect("exact exception repair and missing proof probe");
    tx.execute("ALTER TABLE callback_processing_quarantine DISABLE TRIGGER USER")
        .await
        .expect("fixture bypass");
    sqlx::query("UPDATE callback_processing_quarantine SET recorded_at=1504 WHERE operation_id=$1")
        .bind(&op)
        .execute(&mut *tx)
        .await
        .expect("exact chronology restored");
    tx.execute("ALTER TABLE callback_processing_quarantine ENABLE TRIGGER USER")
        .await
        .expect("guard restored");
    tx.execute("ALTER TABLE callback_receipt_processing DISABLE TRIGGER USER")
        .await
        .expect("isolated missing historical disposition");
    sqlx::query("DELETE FROM callback_receipt_processing WHERE operation_id=$1")
        .bind(&op)
        .execute(&mut *tx)
        .await
        .expect("missing original proof import");
    tx.execute("ALTER TABLE callback_receipt_processing ENABLE TRIGGER USER")
        .await
        .expect("guard restored");
    tx.commit().await.expect("import");
    assert_eq!(
        store::quarantine::read(&pool, &op).await.err(),
        Some(PaymentStoreError::InvalidStoredData)
    );
    let mut tx = pool.begin().await.expect("exact original proof repair");
    tx.execute("ALTER TABLE callback_receipt_processing DISABLE TRIGGER USER")
        .await
        .expect("fixture repair");
    sqlx::query("INSERT INTO callback_receipt_processing SELECT * FROM jsonb_populate_record(NULL::callback_receipt_processing,$1)")
        .bind(saved).execute(&mut *tx).await.expect("exact historical row restored");
    tx.execute("ALTER TABLE callback_receipt_processing ENABLE TRIGGER USER")
        .await
        .expect("guard restored");
    tx.commit().await.expect("repair");
    assert!(
        store::quarantine::read(&pool, &op)
            .await
            .expect("paired healthy original proof")
    );
}

async fn quarantine_wait(pool: &PgPool, cancel: bool) {
    let (clock, _, op) = processing::open(pool).await;
    let mut blocker = pool.begin().await.expect("exception INSERT blocker");
    blocker
        .execute("LOCK TABLE callback_processing_quarantine IN SHARE MODE")
        .await
        .expect("actual quarantine write lock");
    let (p, t, operation) = (pool.clone(), clock.clone(), op.clone());
    let task =
        tokio::spawn(async move { store::quarantine::persist(&p, t.as_ref(), &operation).await });
    waiting_query(pool, "INSERT INTO callback_processing_quarantine%").await;
    if cancel {
        task.abort();
        assert!(
            task.await
                .expect_err("cancelled quarantine write")
                .is_cancelled()
        );
        blocker.commit().await.expect("release write blocker");
    } else {
        clock.0.store(1499, Ordering::SeqCst);
        blocker.commit().await.expect("release write blocker");
        assert_eq!(
            task.await.expect("quarantine task").err(),
            Some(PaymentStoreError::ClockUnavailable)
        );
    }
    assert!(
        !store::quarantine::read(pool, &op)
            .await
            .expect("actual write fully rolled back")
    );
    assert!(
        store::drain::read_disposition(pool, &op)
            .await
            .expect("unchanged genuine success")
            == Some(store::drain::Disposition::Observed)
    );
    clock.0.store(1504, Ordering::SeqCst);
    store::quarantine::persist(pool, clock.as_ref(), &op)
        .await
        .expect("healthy write after wait/cancellation");
    assert!(
        store::quarantine::read(pool, &op)
            .await
            .expect("healthy exception committed")
    );
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn quarantine_actual_write_wait_rewind_rolls_back_before_a_healthy_retry(pool: PgPool) {
    quarantine_wait(&pool, false).await;
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn quarantine_actual_write_wait_cancellation_rolls_back_before_a_healthy_retry(pool: PgPool) {
    quarantine_wait(&pool, true).await;
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn quarantine_persist_requires_a_committed_row_when_an_imported_trigger_silently_suppresses_the_write(
    pool: PgPool,
) {
    let (clock, _, op) = processing::open(&pool).await;
    // Simulate damaged database behavior in this disposable schema. The original
    // native source/immutability guard still executes before the later trigger.
    pool.execute("CREATE FUNCTION test_suppress_callback_quarantine() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RETURN NULL; END; $$")
        .await.expect("isolated silent-write suppression function");
    pool.execute("CREATE TRIGGER zz_test_suppress_callback_quarantine BEFORE INSERT ON callback_processing_quarantine FOR EACH ROW EXECUTE FUNCTION test_suppress_callback_quarantine()")
        .await.expect("isolated silent-write suppression trigger");
    assert_eq!(
        store::quarantine::persist(&pool, clock.as_ref(), &op)
            .await
            .err(),
        Some(PaymentStoreError::InvalidStoredData),
        "successful SQL execution without committed custody is rejected"
    );
    assert!(
        !store::quarantine::read(&pool, &op)
            .await
            .expect("no fictional receipt")
    );
    assert!(
        store::drain::read_disposition(&pool, &op)
            .await
            .expect("original immutable processing untouched")
            == Some(store::drain::Disposition::Observed)
    );
    pool.execute(
        "DROP TRIGGER zz_test_suppress_callback_quarantine ON callback_processing_quarantine",
    )
    .await
    .expect("remove isolated damaged behavior");
    pool.execute("DROP FUNCTION test_suppress_callback_quarantine()")
        .await
        .expect("remove fixture function");
    store::quarantine::persist(&pool, clock.as_ref(), &op)
        .await
        .expect("paired healthy committed exception");
    assert!(
        store::quarantine::read(&pool, &op)
            .await
            .expect("healthy committed custody")
    );
}

async fn done_before_settlement(
    pool: &PgPool,
) -> (
    Arc<crate::payment_test_fixture::quote::Time>,
    String,
    String,
) {
    let (clock, _, op) = processing::open(pool).await;
    let intake: String = sqlx::query_scalar(
        "SELECT intake_id FROM callback_inbox_generations WHERE operation_id=$1",
    )
    .bind(&op)
    .fetch_one(pool)
    .await
    .expect("exact generation source");
    sqlx::query("UPDATE callback_inbox_jobs SET state='done',lease_until=NULL,updated_at=1504,due_at=1504 WHERE intake_id=$1")
        .bind(&intake).execute(pool).await.expect("completed checkpoint before separate settlement");
    assert!(
        !read_callback_job(pool, &intake)
            .await
            .expect("healthy done job")
            .expect("job")
            .settled
    );
    (clock, op, intake)
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn quarantine_winning_job_lock_rejects_a_settlement_statement_started_before_its_commit(
    pool: PgPool,
) {
    let (clock, op, intake) = done_before_settlement(&pool).await;
    let mut blocker = pool.begin().await.expect("job lock ordering fixture");
    sqlx::query("SELECT intake_id FROM callback_inbox_jobs WHERE intake_id=$1 FOR UPDATE")
        .bind(&intake)
        .fetch_one(&mut *blocker)
        .await
        .expect("hold exact job row");
    let (p, t, operation) = (pool.clone(), clock.clone(), op.clone());
    let quarantine =
        tokio::spawn(async move { store::quarantine::persist(&p, t.as_ref(), &operation).await });
    waiting_query(&pool, "INSERT INTO callback_processing_quarantine%").await;
    let (p, id) = (pool.clone(), intake.clone());
    let settlement = tokio::spawn(async move {
        let mut tx = p.begin().await.expect("atomic native settlement");
        sqlx::query("UPDATE callback_inbox_jobs SET settled=true,updated_at=1504,due_at=1504 WHERE intake_id=$1")
            .bind(&id).execute(&mut *tx).await?;
        sqlx::query("DELETE FROM callback_inbox_active WHERE intake_id=$1")
            .bind(&id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await
    });
    waiting_query(&pool, "UPDATE callback_inbox_jobs SET settled=true%").await;
    blocker
        .commit()
        .await
        .expect("quarantine gets first queued job lock");
    quarantine
        .await
        .expect("quarantine task")
        .expect("exception wins and commits");
    let error = settlement
        .await
        .expect("settlement task")
        .expect_err("fresh postlock proof rejects preexisting statement snapshot");
    assert_eq!(
        error.as_database_error().and_then(|e| e.code()).as_deref(),
        Some("23514")
    );
    assert!(
        store::quarantine::read(&pool, &op)
            .await
            .expect("exception retained")
    );
    assert!(
        !read_callback_job(&pool, &intake)
            .await
            .expect("consistent retained active job")
            .expect("job")
            .settled
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM callback_inbox_active WHERE intake_id=$1"
        )
        .bind(&intake)
        .fetch_one(&pool)
        .await
        .expect("slot retained"),
        1
    );
    store::jobs::sweep(&pool, clock.as_ref(), PROFILE)
        .await
        .expect("quarantine does not self-deadlock settlement sweep");
    assert!(
        !read_callback_job(&pool, &intake)
            .await
            .expect("later pass retains slot")
            .expect("job")
            .settled
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn settlement_winning_job_lock_rejects_a_late_quarantine_instead_of_hiding_it_without_capacity(
    pool: PgPool,
) {
    let (clock, op, intake) = done_before_settlement(&pool).await;
    let mut settlement = pool
        .begin()
        .await
        .expect("native settlement owns job lock first");
    sqlx::query("SELECT intake_id FROM callback_inbox_jobs WHERE intake_id=$1 FOR UPDATE")
        .bind(&intake)
        .fetch_one(&mut *settlement)
        .await
        .expect("hold exact job row");
    let (p, t, operation) = (pool.clone(), clock.clone(), op.clone());
    let quarantine =
        tokio::spawn(async move { store::quarantine::persist(&p, t.as_ref(), &operation).await });
    waiting_query(&pool, "INSERT INTO callback_processing_quarantine%").await;
    sqlx::query("UPDATE callback_inbox_jobs SET settled=true,updated_at=1504,due_at=1504 WHERE intake_id=$1")
        .bind(&intake).execute(&mut *settlement).await.expect("healthy complete proof settles");
    sqlx::query("DELETE FROM callback_inbox_active WHERE intake_id=$1")
        .bind(&intake)
        .execute(&mut *settlement)
        .await
        .expect("same transaction releases slot");
    settlement.commit().await.expect("settlement wins");
    assert_eq!(
        quarantine.await.expect("late exception task").err(),
        Some(PaymentStoreError::Database),
        "native source rejects exception after capacity was released"
    );
    assert!(
        !store::quarantine::read(&pool, &op)
            .await
            .expect("no hidden exception")
    );
    assert!(
        read_callback_job(&pool, &intake)
            .await
            .expect("healthy settled job")
            .expect("job")
            .settled
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM callback_inbox_active WHERE intake_id=$1"
        )
        .bind(&intake)
        .fetch_one(&pool)
        .await
        .expect("slot released exactly once"),
        0
    );
}
