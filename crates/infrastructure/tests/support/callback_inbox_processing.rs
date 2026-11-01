//! Revalidated processing headers reject isolated imported damage before authority.
use super::*;
use store::drain::{Disposition, process_operation, read_disposition};
async fn open(
    pool: &PgPool,
) -> (
    Arc<crate::payment_test_fixture::quote::Time>,
    crate::payment_test_fixture::Ids,
    String,
) {
    let (clock, f) = prepared(pool, true).await;
    let i = ids();
    admit(pool, clock.as_ref(), &i, PROFILE, "tr_PROCESSING_PROOF").await;
    let mut claim = store::jobs::claim(pool, clock.as_ref(), &i, PROFILE)
        .await
        .expect("claim")
        .expect("job");
    clock.0.store(1501, Ordering::SeqCst);
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, response(f.snapshot(), "tr_PROCESSING_PROOF", "open")),
    ]);
    let c = client(&server).await;
    let fetched = c
        .fetch_lookup(claim.take_permit().expect("unique permit"))
        .await
        .expect("actual authenticated GET");
    record_lookup(pool, clock.as_ref(), &i, &claim.operation_id, &fetched)
        .await
        .expect("fetched receipt");
    clock.0.store(1504, Ordering::SeqCst);
    assert!(
        process_operation(pool, clock.as_ref(), &i, &claim.operation_id)
            .await
            .expect("genuine original proof")
            == Disposition::Observed
    );
    assert_eq!(server.count(), 2);
    (clock, i, claim.operation_id)
}
async fn constraints(pool: &PgPool) -> Vec<(String, String)> {
    sqlx::query_as("SELECT conname,pg_get_constraintdef(oid) FROM pg_constraint WHERE conrelid='callback_receipt_processing'::regclass AND contype='c'")
        .fetch_all(pool).await.expect("fixture check definitions")
}
async fn set_header(
    pool: &PgPool,
    operation: &str,
    update: &'static str,
    checks: &[(String, String)],
) {
    let mut tx = pool.begin().await.expect("damaged import fixture");
    tx.execute("ALTER TABLE callback_receipt_processing DISABLE TRIGGER ALL")
        .await
        .expect("isolated native guard bypass");
    for (name, _) in checks {
        // These are native constraint names from this owned disposable fixture.
        assert!(name.bytes().all(|v| v.is_ascii_alphanumeric() || v == b'_'));
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "ALTER TABLE callback_receipt_processing DROP CONSTRAINT {name}"
        )))
        .execute(&mut *tx)
        .await
        .expect("isolated damaged header import");
    }
    sqlx::query(update)
        .bind(operation)
        .execute(&mut *tx)
        .await
        .expect("literal header damage");
    tx.commit().await.expect("retain isolated imported damage");
}
async fn repair(pool: &PgPool, operation: &str, saved: &Value, checks: &[(String, String)]) {
    let mut tx = pool.begin().await.expect("exact fixture repair");
    sqlx::query("UPDATE callback_receipt_processing p SET event_id=s.event_id,processed_at=s.processed_at,outcome=s.outcome,observation_id=s.observation_id,resolution_id=s.resolution_id FROM jsonb_populate_record(NULL::callback_receipt_processing,$1) s WHERE p.operation_id=$2")
        .bind(saved).bind(operation).execute(&mut *tx).await.expect("exact original processing row");
    for (name, definition) in checks {
        // Definitions come only from pg_get_constraintdef over the unchanged
        // checks installed by the owning migration in this disposable database.
        assert!(name.bytes().all(|v| v.is_ascii_alphanumeric() || v == b'_'));
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "ALTER TABLE callback_receipt_processing ADD CONSTRAINT {name} {definition}"
        )))
        .execute(&mut *tx)
        .await
        .expect("native guard exact restoration");
    }
    tx.execute("ALTER TABLE callback_receipt_processing ENABLE TRIGGER ALL")
        .await
        .expect("native immutable guard restoration");
    tx.commit().await.expect("healthy fixture repaired");
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn imported_processing_headers_reject_missing_malformed_foreign_and_false_review_proofs(
    pool: PgPool,
) {
    let (clock, i, op) = open(&pool).await;
    let saved: Value = sqlx::query_scalar(
        "SELECT to_jsonb(p) FROM callback_receipt_processing p WHERE operation_id=$1",
    )
    .bind(&op)
    .fetch_one(&pool)
    .await
    .expect("immutable original header");
    let checks = constraints(&pool).await;
    for update in [
        "UPDATE callback_receipt_processing SET event_id=repeat('f',32) WHERE operation_id=$1",
        "UPDATE callback_receipt_processing SET processed_at=1499 WHERE operation_id=$1",
        "UPDATE callback_receipt_processing SET processed_at=1500 WHERE operation_id=$1",
        "UPDATE callback_receipt_processing SET outcome='unknown' WHERE operation_id=$1",
        "UPDATE callback_receipt_processing SET observation_id=NULL WHERE operation_id=$1",
        "UPDATE callback_receipt_processing SET observation_id='malformed' WHERE operation_id=$1",
        "UPDATE callback_receipt_processing SET resolution_id=NULL WHERE operation_id=$1",
        "UPDATE callback_receipt_processing SET resolution_id='malformed' WHERE operation_id=$1",
        "UPDATE callback_receipt_processing SET resolution_id=repeat('f',32) WHERE operation_id=$1",
        "UPDATE callback_receipt_processing SET outcome='custody_invalid' WHERE operation_id=$1",
        "UPDATE callback_receipt_processing SET outcome='custody_invalid',observation_id=NULL WHERE operation_id=$1",
        "UPDATE callback_receipt_processing SET outcome='unmatched' WHERE operation_id=$1",
        "UPDATE callback_receipt_processing SET outcome='unmatched',observation_id=NULL WHERE operation_id=$1",
    ] {
        set_header(&pool, &op, update, &checks).await;
        assert_eq!(
            read_disposition(&pool, &op).await.err(),
            Some(PaymentStoreError::InvalidStoredData),
            "{update}"
        );
        repair(&pool, &op, &saved, &checks).await;
        assert!(
            read_disposition(&pool, &op)
                .await
                .expect("healthy exact source repair")
                == Some(Disposition::Observed)
        );
    }
    assert!(
        process_operation(&pool, &BadPorts, &BadPorts, &op)
            .await
            .expect("exact disposition replay without new ports")
            == Disposition::Observed
    );
    assert_eq!(
        process_operation(&pool, clock.as_ref(), &i, "malformed")
            .await
            .err(),
        Some(PaymentStoreError::InvalidRequest)
    );
    for limit in [0, 26] {
        assert_eq!(
            store::drain::drain_receipts(&pool, clock.as_ref(), &i, PROFILE, limit)
                .await
                .err(),
            Some(PaymentStoreError::InvalidRequest)
        );
    }
}

async fn historical_resolution(pool: &PgPool, receipt_at: u64) {
    let (clock, f) = prepared(pool, true).await;
    let i = ids();
    admit(pool, clock.as_ref(), &i, PROFILE, "tr_EARLY_PREFIX").await;
    let mut claim = store::jobs::claim(pool, clock.as_ref(), &i, PROFILE)
        .await
        .expect("claim")
        .expect("job");
    let historical = identities::resolve(
        pool,
        clock.as_ref(),
        &i,
        &f.snapshot().binding.id,
        &format!("callback-inbox-{}", claim.operation_id),
    )
    .await
    .expect("real historical empty prefix");
    assert_eq!(historical.resolution.through_observation, 0);
    clock.0.store(receipt_at, Ordering::SeqCst);
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, response(f.snapshot(), "tr_EARLY_PREFIX", "open")),
    ]);
    let c = client(&server).await;
    let fetched = c
        .fetch_lookup(claim.take_permit().expect("unique GET permit"))
        .await
        .expect("authenticated later GET");
    record_lookup(pool, clock.as_ref(), &i, &claim.operation_id, &fetched)
        .await
        .expect("later receipt");
    assert!(
        process_operation(pool, clock.as_ref(), &i, &claim.operation_id)
            .await
            .expect("historical incomplete proof becomes visible exception")
            == Disposition::CustodyInvalid
    );
    let saved:(String,Option<String>,Option<String>)=sqlx::query_as("SELECT outcome,observation_id,resolution_id FROM callback_receipt_processing WHERE operation_id=$1").bind(&claim.operation_id).fetch_one(pool).await.expect("redacted exception only");
    assert_eq!(saved, ("custody_invalid".into(), None, None));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM payment_observations")
            .fetch_one(pool)
            .await
            .expect("new genuine observation retained"),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM payment_identity_resolutions")
            .fetch_one(pool)
            .await
            .expect("historical prefix not silently replaced"),
        1
    );
    assert!(
        process_operation(pool, &BadPorts, &BadPorts, &claim.operation_id)
            .await
            .expect("exact exception replay")
            == Disposition::CustodyInvalid
    );
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn historical_resolution_before_later_receipt_is_a_redacted_review_exception(pool: PgPool) {
    historical_resolution(&pool, 1501).await;
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn historical_same_second_resolution_with_short_prefix_is_a_redacted_review_exception(
    pool: PgPool,
) {
    historical_resolution(&pool, 1500).await;
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn a_normal_processing_outcome_requires_its_event_even_when_both_event_references_are_missing(
    pool: PgPool,
) {
    let (_, _, op) = open(&pool).await;
    let saved: Value = sqlx::query_scalar(
        "SELECT to_jsonb(p) FROM callback_receipt_processing p WHERE operation_id=$1",
    )
    .bind(&op)
    .fetch_one(&pool)
    .await
    .expect("saved processing");
    let event: Value = sqlx::query_scalar(
        "SELECT to_jsonb(e) FROM callback_processing_events e WHERE operation_id=$1",
    )
    .bind(&op)
    .fetch_one(&pool)
    .await
    .expect("saved original event");
    let checks = constraints(&pool).await;
    set_header(
        &pool,
        &op,
        "UPDATE callback_receipt_processing SET event_id=NULL WHERE operation_id=$1",
        &checks,
    )
    .await;
    let mut tx = pool.begin().await.expect("event import fixture");
    tx.execute("ALTER TABLE callback_processing_events DISABLE TRIGGER ALL")
        .await
        .expect("isolated event bypass");
    sqlx::query("DELETE FROM callback_processing_events WHERE operation_id=$1")
        .bind(&op)
        .execute(&mut *tx)
        .await
        .expect("missing event import");
    tx.commit().await.expect("imported missing event");
    assert_eq!(
        read_disposition(&pool, &op).await.err(),
        Some(PaymentStoreError::InvalidStoredData)
    );
    let mut tx = pool.begin().await.expect("exact event repair");
    sqlx::query("INSERT INTO callback_processing_events SELECT (jsonb_populate_record(NULL::callback_processing_events,$1)).*").bind(&event).execute(&mut *tx).await.expect("exact original event");
    tx.execute("ALTER TABLE callback_processing_events ENABLE TRIGGER ALL")
        .await
        .expect("restore native event guards");
    tx.commit().await.expect("event restored");
    repair(&pool, &op, &saved, &checks).await;
    assert!(
        read_disposition(&pool, &op)
            .await
            .expect("paired exact custody restoration")
            == Some(Disposition::Observed)
    );
}
struct ProbeClock {
    first: u64,
    later: u64,
    reads: std::sync::atomic::AtomicUsize,
}
impl commerce_application::request_context::Clock for ProbeClock {
    fn unix_seconds(&self) -> Result<u64, commerce_application::request_context::ContextError> {
        Ok(if self.reads.fetch_add(1, Ordering::SeqCst) == 0 {
            self.first
        } else {
            self.later
        })
    }
}
async fn remove_disposition(pool: &PgPool, operation: &str) {
    let mut tx = pool
        .begin()
        .await
        .expect("isolated processing removal fixture");
    tx.execute("ALTER TABLE callback_receipt_processing DISABLE TRIGGER USER")
        .await
        .expect("fixture bypass");
    sqlx::query("DELETE FROM callback_receipt_processing WHERE operation_id=$1")
        .bind(operation)
        .execute(&mut *tx)
        .await
        .expect("retain authentic receipt without disposition");
    tx.execute("ALTER TABLE callback_receipt_processing ENABLE TRIGGER USER")
        .await
        .expect("restore native processing guards");
    tx.commit().await.expect("isolated fixture ready");
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn private_exception_write_checks_source_receipt_and_final_clock_before_committing(
    pool: PgPool,
) {
    let (_, _, op) = open(&pool).await;
    let saved: Value = sqlx::query_scalar(
        "SELECT to_jsonb(p) FROM callback_receipt_processing p WHERE operation_id=$1",
    )
    .bind(&op)
    .fetch_one(&pool)
    .await
    .expect("saved genuine processing");
    for (first, later, recorded_at, healthy) in [
        (1499, 1499, None, 1504),
        (1504, 1504, Some(1505), 1505),
        (1504, 1503, None, 1504),
    ] {
        remove_disposition(&pool, &op).await;
        let clock = ProbeClock {
            first,
            later,
            reads: std::sync::atomic::AtomicUsize::new(0),
        };
        assert_eq!(
            store::drain::proof_tests::persist_clock_probe(&pool, &clock, &op, recorded_at)
                .await
                .err(),
            Some(PaymentStoreError::ClockUnavailable)
        );
        assert!(
            read_disposition(&pool, &op)
                .await
                .expect("failed write rolled back")
                .is_none()
        );
        let healthy_clock = ProbeClock {
            first: healthy,
            later: healthy,
            reads: std::sync::atomic::AtomicUsize::new(0),
        };
        assert!(
            store::drain::proof_tests::persist_clock_probe(&pool, &healthy_clock, &op, recorded_at)
                .await
                .expect("paired healthy clocks commit actual exception")
                == Disposition::CustodyInvalid
        );
        remove_disposition(&pool, &op).await;
        // Exact original fixture restoration, not an operational replay/reset API.
        let mut tx = pool.begin().await.expect("restore genuine processing");
        tx.execute("ALTER TABLE callback_receipt_processing DISABLE TRIGGER USER")
            .await
            .expect("fixture restoration bypass");
        sqlx::query("INSERT INTO callback_receipt_processing SELECT (jsonb_populate_record(NULL::callback_receipt_processing,$1)).*").bind(&saved).execute(&mut *tx).await.expect("exact original processing");
        tx.execute("ALTER TABLE callback_receipt_processing ENABLE TRIGGER USER")
            .await
            .expect("native guard restored");
        tx.commit().await.expect("original processing restored");
        assert!(
            read_disposition(&pool, &op)
                .await
                .expect("genuine original proof remains valid")
                == Some(Disposition::Observed)
        );
    }
}
