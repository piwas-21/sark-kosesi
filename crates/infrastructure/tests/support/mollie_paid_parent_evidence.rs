//! Private paid-parent aggregate custody using only isolated PostgreSQL and loopback.
use super::*;
use crate::payment_attempts::{
    PaymentStoreError,
    identities::resolve,
    observations::record_verified,
    paid_parent_evidence::{self, Start, Status},
    recognition::{State, recognize},
};
use sqlx::{Executor, PgPool};
use std::sync::atomic::Ordering;

const PAYMENT: &str = "tr_SYNTHETIC_NotARealPayment";

async fn paid(
    pool: &PgPool,
) -> (
    std::sync::Arc<crate::payment_test_fixture::quote::Time>,
    FrozenAttempt,
    crate::payment_test_fixture::Ids,
) {
    let (time, frozen) = observation_cases::prepared(pool, true).await;
    let ids = observation_cases::ids();
    let payment = observation_cases::fetched(frozen.snapshot(), PAYMENT, "paid", false).await;
    record_verified(
        pool,
        time.as_ref(),
        &ids,
        &frozen.snapshot().binding.id,
        "paid-source",
        &payment,
    )
    .await
    .expect("paid observation");
    resolve(
        pool,
        time.as_ref(),
        &ids,
        &frozen.snapshot().binding.id,
        "paid-resolution",
    )
    .await
    .expect("exact paid identity");
    let recognition = recognize(pool, time.as_ref(), &ids, &frozen.snapshot().binding.id)
        .await
        .expect("received recognition")
        .recognition;
    assert_eq!(recognition.state, State::Received);
    (time, frozen, ids)
}

async fn counts(pool: &PgPool) -> (i64, i64, i64) {
    sqlx::query_as("SELECT (SELECT count(*) FROM paid_parent_total_operations),(SELECT count(*) FROM paid_parent_total_receipts),(SELECT count(*) FROM commerce_outbox WHERE kind='payment.paid_parent_totals_observed.v1')").fetch_one(pool).await.expect("paid parent custody")
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn rotated_same_profile_permit_is_single_use_and_preserves_omission_zero_and_overgross(
    pool: PgPool,
) {
    let (time, frozen, ids) = paid(&pool).await;
    let mut full = payment(frozen.snapshot());
    full["amountRefunded"] = json!({"currency":"EUR","value":"395.01"});
    full["amountRemaining"] = json!({"currency":"EUR","value":"0.00"});
    full["amountCaptured"] = json!({"currency":"EUR","value":"382.45"});
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, payment(frozen.snapshot())),
        Reply::Json(200, full),
    ]);
    let raw_rotated = format!("test_{}", "rotated".repeat(5));
    let rotated_key = TestCredential::capture(&raw_rotated, "pfl_SYNTHETIC_NotARealProfile")
        .expect("rotated test key");
    let rotated_fingerprint = rotated_key.binding().credential_fingerprint.clone();
    let rotated_config = config(&rotated_key);
    let client = TestClient::local(
        rotated_key,
        rotated_config,
        server.base.clone(),
        Duration::from_secs(2),
    )
    .expect("loopback only")
    .verify_profile()
    .await
    .expect("actual test profile")
    .into_reconciliation();
    let id = &frozen.snapshot().binding.id;
    let permit =
        match paid_parent_evidence::start(&pool, time.as_ref(), &ids, id, "missing", &client)
            .await
            .expect("start")
        {
            Start::Fetch(permit) => permit,
            _ => panic!("new effect requires one fetch permit"),
        };
    assert!(matches!(
        paid_parent_evidence::start(&pool, time.as_ref(), &ids, id, "missing", &client)
            .await
            .expect("pending"),
        Start::Pending(_)
    ));
    assert!(matches!(
        paid_parent_evidence::lookup(&pool, id, "missing")
            .await
            .expect("lookup"),
        Some(Status::Pending(_))
    ));
    assert_eq!(server.count(), 1, "pending operation issues no payment GET");
    assert_eq!(counts(&pool).await, (1, 0, 0));
    let fetched = paid_parent_evidence::fetch(permit, time.as_ref(), &client)
        .await
        .expect("single authenticated parent GET");
    let receipt = paid_parent_evidence::record(&pool, time.as_ref(), &ids, &fetched)
        .await
        .expect("durable totals");
    assert!(!receipt.replayed);
    assert_eq!(
        receipt.operation.credential_fingerprint,
        rotated_fingerprint
    );
    assert_eq!(receipt.operation.payment_id, PAYMENT);
    assert_eq!(
        (
            receipt.totals.refunded_minor,
            receipt.totals.remaining_minor,
            receipt.totals.captured_minor,
            receipt.totals.charged_back_minor
        ),
        (None, None, None, None)
    );
    assert!(
        paid_parent_evidence::record(&pool, time.as_ref(), &ids, &fetched)
            .await
            .expect("exact record replay")
            .replayed
    );
    assert!(matches!(
        paid_parent_evidence::start(&pool, time.as_ref(), &ids, id, "missing", &client)
            .await
            .expect("completed replay"),
        Start::Complete(_)
    ));
    assert!(matches!(
        paid_parent_evidence::lookup(&pool, id, "missing")
            .await
            .expect("completed lookup"),
        Some(Status::Complete(_))
    ));
    assert_eq!(counts(&pool).await, (1, 1, 1));
    let permit =
        match paid_parent_evidence::start(&pool, time.as_ref(), &ids, id, "amounts", &client)
            .await
            .expect("second explicit effect")
        {
            Start::Fetch(p) => p,
            _ => panic!("new fetch"),
        };
    let fetched = paid_parent_evidence::fetch(permit, time.as_ref(), &client)
        .await
        .expect("independent authenticated read");
    let second = paid_parent_evidence::record(&pool, time.as_ref(), &ids, &fetched)
        .await
        .expect("reported aggregates");
    assert_eq!(
        (
            second.totals.refunded_minor,
            second.totals.remaining_minor,
            second.totals.captured_minor,
            second.totals.charged_back_minor
        ),
        (Some(39501), Some(0), Some(38245), None)
    );
    assert_eq!(counts(&pool).await, (2, 2, 2));
    let event: (bool, bool) = sqlx::query_as("SELECT e.payload=jsonb_build_object('schema_version',1,'order_id',o.order_id,'attempt_id',o.attempt_id,'operation_id',o.id,'receipt_sha256',paid_parent_total_receipt_digest(r,o)),r.transaction_id IS NOT NULL FROM paid_parent_total_operations o JOIN paid_parent_total_receipts r ON r.operation_id=o.id JOIN commerce_outbox e ON e.id=r.event_id WHERE o.id=$1").bind(&second.operation.id).fetch_one(&pool).await.expect("exact native event and transaction marker");
    assert_eq!(event, (true, true));
    pool.execute("ALTER TABLE paid_parent_total_operations DISABLE TRIGGER guard_paid_parent_total_operation")
        .await.expect("isolate operation history faults");
    for (effect, payment_id) in [("redirected", PAYMENT), ("amounts", "tr_WrongOriginal")] {
        sqlx::query(
            "UPDATE paid_parent_total_operations SET effect_key=$1,payment_id=$2 WHERE id=$3",
        )
        .bind(effect)
        .bind(payment_id)
        .bind(&second.operation.id)
        .execute(&pool)
        .await
        .expect("simulate coherent operation damage");
        assert!(matches!(
            paid_parent_evidence::lookup(&pool, id, effect).await,
            Err(PaymentStoreError::InvalidStoredData)
        ));
        if effect == "amounts" {
            assert!(
                matches!(
                    paid_parent_evidence::record(&pool, time.as_ref(), &ids, &fetched).await,
                    Err(PaymentStoreError::InvalidStoredData)
                ),
                "record rejects changed stored operation before source restoration"
            );
        }
    }
    sqlx::query(
        "UPDATE paid_parent_total_operations SET effect_key='amounts',payment_id=$1 WHERE id=$2",
    )
    .bind(PAYMENT)
    .bind(&second.operation.id)
    .execute(&pool)
    .await
    .expect("restore operation scope");
    sqlx::query(
        "UPDATE paid_parent_total_operations SET started_at=1499,fetch_before=1559 WHERE id=$1",
    )
    .bind(&second.operation.id)
    .execute(&pool)
    .await
    .expect("simulate earlier source with valid 60-second interval");
    assert!(
        matches!(
            paid_parent_evidence::lookup(&pool, id, "amounts").await,
            Err(PaymentStoreError::InvalidStoredData)
        ),
        "operation cannot predate its recognition"
    );
    sqlx::query(
        "UPDATE paid_parent_total_operations SET started_at=1500,fetch_before=1560 WHERE id=$1",
    )
    .bind(&second.operation.id)
    .execute(&pool)
    .await
    .expect("restore source times");
    assert!(matches!(
        paid_parent_evidence::lookup(&pool, id, "amounts").await,
        Ok(Some(Status::Complete(_)))
    ));
    pool.execute("ALTER TABLE paid_parent_total_operations DROP CONSTRAINT paid_parent_total_operations_fetch_before_check").await.expect("isolate malformed operation window");
    sqlx::query("UPDATE paid_parent_total_operations SET fetch_before=1561 WHERE id=$1")
        .bind(&second.operation.id)
        .execute(&pool)
        .await
        .expect("damage operation shape");
    assert!(
        matches!(
            paid_parent_evidence::lookup(&pool, id, "amounts").await,
            Err(PaymentStoreError::InvalidStoredData)
        ),
        "unbounded operation shape fails closed"
    );
    sqlx::query("UPDATE paid_parent_total_operations SET fetch_before=1560 WHERE id=$1")
        .bind(&second.operation.id)
        .execute(&pool)
        .await
        .expect("restore operation window");
    pool.execute("ALTER TABLE paid_parent_total_operations ADD CONSTRAINT paid_parent_total_operations_fetch_before_check CHECK(fetch_before=started_at+60)").await.expect("restore shape constraint");
    assert!(matches!(
        paid_parent_evidence::lookup(&pool, id, "amounts").await,
        Ok(Some(Status::Complete(_)))
    ));
    pool.execute(
        "ALTER TABLE paid_parent_total_receipts DISABLE TRIGGER guard_paid_parent_total_receipt",
    )
    .await
    .expect("isolate damaged receipt chronology");
    sqlx::query("UPDATE paid_parent_total_receipts SET recorded_at=1499 WHERE operation_id=$1")
        .bind(&second.operation.id)
        .execute(&pool)
        .await
        .expect("damage receipt chronology");
    assert!(
        matches!(
            paid_parent_evidence::lookup(&pool, id, "amounts").await,
            Err(PaymentStoreError::InvalidStoredData)
        ),
        "recording before fetch fails closed"
    );
    sqlx::query("UPDATE paid_parent_total_receipts SET recorded_at=$1 WHERE operation_id=$2")
        .bind(second.recorded_at)
        .bind(&second.operation.id)
        .execute(&pool)
        .await
        .expect("restore receipt chronology");
    pool.execute(
        "ALTER TABLE paid_parent_total_receipts ENABLE TRIGGER guard_paid_parent_total_receipt",
    )
    .await
    .expect("restore receipt guard");
    assert!(matches!(
        paid_parent_evidence::lookup(&pool, id, "amounts").await,
        Ok(Some(Status::Complete(_)))
    ));
    pool.execute(
        "ALTER TABLE paid_parent_total_operations ENABLE TRIGGER guard_paid_parent_total_operation",
    )
    .await
    .expect("restore operation guard");
    assert!(matches!(
        paid_parent_evidence::lookup(&pool, id, "amounts").await,
        Ok(Some(Status::Complete(restored))) if restored.totals == second.totals
    ));
    server.inspect(|requests| {
        assert_eq!(requests.len(), 3);
        for request in &requests[1..] {
            assert_eq!(request.method, "GET");
            assert_eq!(request.path, format!("/v2/payments/{PAYMENT}"));
            assert_eq!(
                request.header("Authorization"),
                Some(format!("Bearer {raw_rotated}").as_str())
            );
            assert!(request.body.is_empty());
        }
    });
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn missing_not_received_stale_and_damaged_recognition_never_grant_network(pool: PgPool) {
    let (time, frozen) = observation_cases::prepared(&pool, true).await;
    let ids = observation_cases::ids();
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, payment(frozen.snapshot())),
    ]);
    let client = local(&server)
        .verify_profile()
        .await
        .expect("verified profile")
        .into_reconciliation();
    let id = &frozen.snapshot().binding.id;
    assert!(
        paid_parent_evidence::start(&pool, time.as_ref(), &ids, id, "absent", &client)
            .await
            .is_err()
    );
    let open = observation_cases::fetched(frozen.snapshot(), PAYMENT, "open", false).await;
    record_verified(&pool, time.as_ref(), &ids, id, "open", &open)
        .await
        .expect("open observation");
    resolve(&pool, time.as_ref(), &ids, id, "open-resolution")
        .await
        .expect("open resolution");
    let recognition = recognize(&pool, time.as_ref(), &ids, id)
        .await
        .expect("unconfirmed recognition")
        .recognition;
    assert_eq!(recognition.state, State::Unconfirmed);
    assert!(
        paid_parent_evidence::start(&pool, time.as_ref(), &ids, id, "unconfirmed", &client)
            .await
            .is_err()
    );
    let paid = observation_cases::fetched(frozen.snapshot(), PAYMENT, "paid", false).await;
    record_verified(&pool, time.as_ref(), &ids, id, "paid", &paid)
        .await
        .expect("new evidence");
    assert!(
        paid_parent_evidence::start(&pool, time.as_ref(), &ids, id, "stale", &client)
            .await
            .is_err()
    );
    resolve(&pool, time.as_ref(), &ids, id, "paid-resolution")
        .await
        .expect("new resolution");
    let recognition = recognize(&pool, time.as_ref(), &ids, id)
        .await
        .expect("received")
        .recognition;
    assert_eq!(recognition.state, State::Received);
    pool.execute("ALTER TABLE payment_recognitions DISABLE TRIGGER guard_payment_recognition")
        .await
        .expect("isolate damaged historical recognition");
    sqlx::query("UPDATE payment_recognitions SET amount_minor=amount_minor-1 WHERE id=$1")
        .bind(&recognition.id)
        .execute(&pool)
        .await
        .expect("damage amount");
    assert!(
        paid_parent_evidence::start(&pool, time.as_ref(), &ids, id, "damaged", &client)
            .await
            .is_err()
    );
    sqlx::query("UPDATE payment_recognitions SET amount_minor=amount_minor+1 WHERE id=$1")
        .bind(&recognition.id)
        .execute(&pool)
        .await
        .expect("restore source");
    pool.execute("ALTER TABLE payment_recognitions ENABLE TRIGGER guard_payment_recognition")
        .await
        .expect("restore guard");
    assert_eq!(
        server.count(),
        1,
        "rejected attempts never fetch the parent"
    );
    assert_eq!(counts(&pool).await, (0, 0, 0));
    assert!(matches!(
        paid_parent_evidence::start(&pool, time.as_ref(), &ids, id, "healthy", &client)
            .await
            .expect("healthy positive control"),
        Start::Fetch(_)
    ));
    assert_eq!(server.count(), 1);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn wrong_profile_and_expired_permit_never_issue_parent_get(pool: PgPool) {
    let (time, frozen, ids) = paid(&pool).await;
    let server = Server::start(vec![profile()]);
    let client = local(&server)
        .verify_profile()
        .await
        .expect("profile")
        .into_reconciliation();
    let other_server = Server::start(vec![Reply::Json(
        200,
        json!({"resource":"profile","id":"pfl_OTHER"}),
    )]);
    let other_key = TestCredential::capture(&raw_key(), "pfl_OTHER").expect("other profile key");
    let other_config = config(&other_key);
    let other = TestClient::local(
        other_key,
        other_config,
        other_server.base.clone(),
        Duration::from_secs(2),
    )
    .expect("loopback")
    .verify_profile()
    .await
    .expect("actual different profile")
    .into_reconciliation();
    let rotated_server = Server::start(vec![profile()]);
    let rotated_key = TestCredential::capture(
        &format!("test_{}", "rotated".repeat(5)),
        "pfl_SYNTHETIC_NotARealProfile",
    )
    .expect("rotated test key");
    let rotated_config = config(&rotated_key);
    let rotated = TestClient::local(
        rotated_key,
        rotated_config,
        rotated_server.base.clone(),
        Duration::from_secs(2),
    )
    .expect("loopback")
    .verify_profile()
    .await
    .expect("same profile")
    .into_reconciliation();
    let id = &frozen.snapshot().binding.id;
    assert!(matches!(
        paid_parent_evidence::lookup(&pool, id, "absent").await,
        Ok(None)
    ));
    assert!(matches!(
        paid_parent_evidence::start(&pool, time.as_ref(), &ids, id, "wrong-profile", &other).await,
        Err(PaymentStoreError::Conflict)
    ));
    let permit =
        match paid_parent_evidence::start(&pool, time.as_ref(), &ids, id, "wrong-client", &client)
            .await
            .expect("permitted start")
        {
            Start::Fetch(p) => p,
            _ => panic!("first permit"),
        };
    assert!(
        matches!(
            paid_parent_evidence::start(&pool, time.as_ref(), &ids, id, "wrong-client", &rotated)
                .await,
            Err(PaymentStoreError::Conflict)
        ),
        "same profile with different key cannot replay the operation"
    );
    assert!(
        matches!(
            paid_parent_evidence::fetch(permit, time.as_ref(), &rotated).await,
            Err(MollieError::Custody(PaymentStoreError::Conflict))
        ),
        "permit binds its credential fingerprint"
    );
    assert!(matches!(
        paid_parent_evidence::start(&pool, time.as_ref(), &ids, id, "wrong-client", &client)
            .await
            .expect("lost capability stays pending"),
        Start::Pending(_)
    ));
    let permit = match paid_parent_evidence::start(
        &pool,
        time.as_ref(),
        &ids,
        id,
        "pre-start-clock",
        &client,
    )
    .await
    .expect("pre-start control")
    {
        Start::Fetch(p) => p,
        _ => panic!("new permit"),
    };
    time.0.store(1499, Ordering::SeqCst);
    assert!(matches!(
        paid_parent_evidence::fetch(permit, time.as_ref(), &client).await,
        Err(MollieError::Custody(PaymentStoreError::ClockUnavailable))
    ));
    time.0.store(1500, Ordering::SeqCst);
    let permit =
        match paid_parent_evidence::start(&pool, time.as_ref(), &ids, id, "expired", &client)
            .await
            .expect("second operation")
        {
            Start::Fetch(p) => p,
            _ => panic!("second permit"),
        };
    time.0.store(1560, Ordering::SeqCst);
    assert!(
        paid_parent_evidence::fetch(permit, time.as_ref(), &client)
            .await
            .is_err()
    );
    assert_eq!(
        (server.count(), other_server.count(), rotated_server.count()),
        (1, 1, 1),
        "only profile verification contacted loopback"
    );
    assert_eq!(counts(&pool).await, (3, 0, 0));
    pool.close().await;
    assert!(
        paid_parent_evidence::start(&pool, time.as_ref(), &ids, id, "closed", &client)
            .await
            .is_err()
    );
    assert_eq!(server.count(), 1, "closed storage grants no parent GET");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn late_clock_rewind_after_real_event_write_rolls_back_receipt_and_allows_retry(
    pool: PgPool,
) {
    let (time, frozen, ids) = paid(&pool).await;
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, payment(frozen.snapshot())),
    ]);
    let client = local(&server)
        .verify_profile()
        .await
        .expect("profile")
        .into_reconciliation();
    let id = &frozen.snapshot().binding.id;
    let permit = match paid_parent_evidence::start(&pool, time.as_ref(), &ids, id, "clock", &client)
        .await
        .expect("start")
    {
        Start::Fetch(p) => p,
        _ => panic!("first permit"),
    };
    let fetched = paid_parent_evidence::fetch(permit, time.as_ref(), &client)
        .await
        .expect("fetched");
    time.0.store(1499, Ordering::SeqCst);
    assert!(
        matches!(
            paid_parent_evidence::record(&pool, time.as_ref(), &ids, &fetched).await,
            Err(PaymentStoreError::ClockUnavailable)
        ),
        "record clock cannot precede fetch"
    );
    assert_eq!(counts(&pool).await, (1, 0, 0));
    time.0.store(1500, Ordering::SeqCst);
    pool.execute("CREATE FUNCTION test_parent_event_wait() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.kind='payment.paid_parent_totals_observed.v1' THEN PERFORM pg_advisory_xact_lock(1936876950,1); END IF; RETURN NEW; END; $$; CREATE TRIGGER z_test_parent_event_wait AFTER INSERT ON commerce_outbox FOR EACH ROW EXECUTE FUNCTION test_parent_event_wait()").await.expect("post-write gate");
    let mut gate = pool.begin().await.expect("gate");
    sqlx::query("SELECT pg_advisory_xact_lock(1936876950,1)")
        .execute(&mut *gate)
        .await
        .expect("hold");
    let work = paid_parent_evidence::record(&pool, time.as_ref(), &ids, &fetched);
    let release = async {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND wait_event='advisory' AND query LIKE '%INSERT INTO commerce_outbox%')").fetch_one(&pool).await.expect("actual outbox wait");
                if waiting { break; }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        }).await.expect("event reached real write gate");
        time.0.store(1499, Ordering::SeqCst);
        gate.rollback().await.expect("resume write");
    };
    let (result, ()) = tokio::join!(work, release);
    assert!(matches!(result, Err(PaymentStoreError::ClockUnavailable)));
    assert_eq!(counts(&pool).await, (1, 0, 0));
    assert_eq!(server.count(), 2);
    time.0.store(1500, Ordering::SeqCst);
    let saved = paid_parent_evidence::record(&pool, time.as_ref(), &ids, &fetched)
        .await
        .expect("same fetched evidence retries after rollback");
    assert!(!saved.replayed);
    assert_eq!(counts(&pool).await, (1, 1, 1));
    assert_eq!(server.count(), 2, "retry issues no new provider GET");
}

fn native_error(error: sqlx::Error, message: &str) {
    let sqlx::Error::Database(error) = error else {
        panic!("expected native PostgreSQL rejection")
    };
    assert_eq!(error.code().as_deref(), Some("23514"));
    assert_eq!(error.message(), message);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn native_receipt_requires_valid_chronology_and_exact_same_transaction_event(pool: PgPool) {
    let (time, frozen, ids) = paid(&pool).await;
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, payment(frozen.snapshot())),
    ]);
    let client = local(&server)
        .verify_profile()
        .await
        .expect("profile")
        .into_reconciliation();
    let id = &frozen.snapshot().binding.id;
    let permit =
        match paid_parent_evidence::start(&pool, time.as_ref(), &ids, id, "native", &client)
            .await
            .expect("admitted operation")
        {
            Start::Fetch(p) => p,
            _ => panic!("first permit"),
        };
    let operation: String = sqlx::query_scalar(
        "SELECT id FROM paid_parent_total_operations WHERE attempt_id=$1 AND effect_key='native'",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .expect("operation");
    let event = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
    let insert = "INSERT INTO paid_parent_total_receipts(operation_id,fetched_at,recorded_at,event_id) VALUES($1,$2,$3,$4)";
    let mut tx = pool
        .begin()
        .await
        .expect("uncommitted operation transaction");
    sqlx::query("INSERT INTO paid_parent_total_operations(id,order_id,attempt_id,effect_key,recognition_id,through_observation,profile_id,payment_id,credential_fingerprint,started_at,fetch_before) SELECT 'ffffffffffffffffffffffffffffffff',order_id,attempt_id,'same-xid',recognition_id,through_observation,profile_id,payment_id,credential_fingerprint,started_at,fetch_before FROM paid_parent_total_operations WHERE id=$1").bind(&operation).execute(&mut *tx).await.expect("otherwise-valid operation");
    let error = sqlx::query(insert)
        .bind("ffffffffffffffffffffffffffffffff")
        .bind(1500_i64)
        .bind(1500_i64)
        .bind(event)
        .execute(&mut *tx)
        .await
        .expect_err("receipt cannot share admission transaction");
    native_error(
        error,
        "Paid parent receipt requires exact historical source and chronology",
    );
    tx.rollback().await.expect("rollback uncommitted pair");
    let mut tx = pool.begin().await.expect("chronology transaction");
    let error = sqlx::query(insert)
        .bind(&operation)
        .bind(1499_i64)
        .bind(1500_i64)
        .bind(event)
        .execute(&mut *tx)
        .await
        .expect_err("fetch before admission");
    native_error(
        error,
        "Paid parent receipt requires exact historical source and chronology",
    );
    tx.rollback().await.expect("rollback");
    let mut tx = pool.begin().await.expect("missing event transaction");
    sqlx::query(insert)
        .bind(&operation)
        .bind(1500_i64)
        .bind(1500_i64)
        .bind(event)
        .execute(&mut *tx)
        .await
        .expect("native receipt");
    let error = sqlx::query("SET CONSTRAINTS require_paid_parent_total_receipt IMMEDIATE")
        .execute(&mut *tx)
        .await
        .expect_err("missing reciprocal event");
    native_error(error, "Paid parent receipt requires exact reciprocal event");
    tx.rollback().await.expect("rollback");
    let mut tx = pool.begin().await.expect("forged event transaction");
    sqlx::query(insert)
        .bind(&operation)
        .bind(1500_i64)
        .bind(1500_i64)
        .bind(event)
        .execute(&mut *tx)
        .await
        .expect("native receipt");
    let error = sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) VALUES($1,$2,'payment.paid_parent_totals_observed.v1',$3,1500)").bind(event).bind(&frozen.snapshot().binding.order_id).bind(json!({"schema_version":1,"order_id":frozen.snapshot().binding.order_id,"attempt_id":id,"operation_id":operation,"receipt_sha256":"0".repeat(64)})).execute(&mut *tx).await.expect_err("forged digest event");
    native_error(
        error,
        "Paid parent event requires exact same-transaction receipt",
    );
    tx.rollback().await.expect("rollback");
    assert_eq!(counts(&pool).await, (1, 0, 0));
    let fetched = paid_parent_evidence::fetch(permit, time.as_ref(), &client)
        .await
        .expect("authenticated positive control");
    paid_parent_evidence::record(&pool, time.as_ref(), &ids, &fetched)
        .await
        .expect("exact receipt and reciprocal event");
    assert_eq!(counts(&pool).await, (1, 1, 1));
    for statement in [
        "UPDATE paid_parent_total_operations SET effect_key=effect_key",
        "DELETE FROM paid_parent_total_operations",
        "UPDATE paid_parent_total_receipts SET refunded_minor=refunded_minor",
        "DELETE FROM paid_parent_total_receipts",
    ] {
        assert!(
            pool.execute(statement).await.is_err(),
            "history is immutable"
        );
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn response_after_deadline_records_original_historical_watermark_after_new_evidence(
    pool: PgPool,
) {
    use std::sync::{Arc, atomic::AtomicBool};
    let (time, frozen, ids) = paid(&pool).await;
    let gate = Arc::new(AtomicBool::new(false));
    let server = Server::start(vec![
        profile(),
        Reply::GateJson(gate.clone(), payment(frozen.snapshot())),
    ]);
    let client = local(&server)
        .verify_profile()
        .await
        .expect("profile")
        .into_reconciliation();
    let id = &frozen.snapshot().binding.id;
    let permit = match paid_parent_evidence::start(&pool, time.as_ref(), &ids, id, "slow", &client)
        .await
        .expect("committed permit")
    {
        Start::Fetch(p) => p,
        _ => panic!("first permit"),
    };
    let original_watermark = match paid_parent_evidence::lookup(&pool, id, "slow")
        .await
        .expect("pending lookup")
    {
        Some(Status::Pending(op)) => op.through_observation,
        _ => panic!("pending operation"),
    };
    assert_eq!(original_watermark, 1);
    let work = paid_parent_evidence::fetch(permit, time.as_ref(), &client);
    let release = async {
        tokio::time::timeout(Duration::from_secs(2), async {
            while server.count() < 2 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("payment GET reached gated loopback response");
        time.0.store(1561, Ordering::SeqCst);
        gate.store(true, Ordering::SeqCst);
    };
    let (fetched, ()) = tokio::join!(work, release);
    let fetched = fetched.expect("in-time GET may complete after permit deadline");
    let later = observation_cases::fetched(frozen.snapshot(), PAYMENT, "open", false).await;
    record_verified(&pool, time.as_ref(), &ids, id, "later-open", &later)
        .await
        .expect("newer observation");
    assert!(
        matches!(
            paid_parent_evidence::start(&pool, time.as_ref(), &ids, id, "stale-new", &client).await,
            Err(PaymentStoreError::Unavailable)
        ),
        "stale current recognition cannot grant a new GET"
    );
    resolve(&pool, time.as_ref(), &ids, id, "later-resolution")
        .await
        .expect("fresh identity resolution");
    let current = recognize(&pool, time.as_ref(), &ids, id)
        .await
        .expect("new current recognition")
        .recognition;
    assert_eq!(current.through_observation, 2);
    let receipt = paid_parent_evidence::record(&pool, time.as_ref(), &ids, &fetched)
        .await
        .expect("record original historical source");
    assert_eq!(
        (
            receipt.operation.through_observation,
            receipt.fetched_at,
            receipt.recorded_at
        ),
        (1, 1561, 1561)
    );
    assert_eq!(counts(&pool).await, (1, 1, 1));
    assert_eq!(
        server.count(),
        2,
        "only the one previously admitted parent GET ran"
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn native_source_seal_survives_early_constraint_check_and_later_evidence_can_advance(
    pool: PgPool,
) {
    let (time, frozen, ids) = paid(&pool).await;
    let server = Server::start(vec![profile()]);
    let client = local(&server)
        .verify_profile()
        .await
        .expect("profile")
        .into_reconciliation();
    let id = &frozen.snapshot().binding.id;
    assert!(matches!(
        paid_parent_evidence::start(&pool, time.as_ref(), &ids, id, "template", &client)
            .await
            .expect("committed source"),
        Start::Fetch(_)
    ));
    let operation: String = sqlx::query_scalar(
        "SELECT id FROM paid_parent_total_operations WHERE attempt_id=$1 AND effect_key='template'",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .expect("template");
    let mut tx = pool.begin().await.expect("native transaction");
    sqlx::query("INSERT INTO paid_parent_total_operations(id,order_id,attempt_id,effect_key,recognition_id,through_observation,profile_id,payment_id,credential_fingerprint,started_at,fetch_before) SELECT 'ffffffffffffffffffffffffffffffff',order_id,attempt_id,'early-check',recognition_id,through_observation,profile_id,payment_id,credential_fingerprint,started_at,fetch_before FROM paid_parent_total_operations WHERE id=$1").bind(&operation).execute(&mut *tx).await.expect("new operation");
    sqlx::query("SET CONSTRAINTS require_paid_parent_total_operation IMMEDIATE")
        .execute(&mut *tx)
        .await
        .expect("force source check early");
    let error = sqlx::query(
        "UPDATE payment_recognition_current SET recognition_id=recognition_id WHERE order_id=$1",
    )
    .bind(&frozen.snapshot().binding.order_id)
    .execute(&mut *tx)
    .await
    .expect_err("same-transaction source mutation forbidden");
    native_error(
        error,
        "Paid parent operation source is sealed for this transaction",
    );
    tx.rollback()
        .await
        .expect("roll back operation and mutation");
    assert_eq!(counts(&pool).await, (1, 0, 0));
    let later = observation_cases::fetched(frozen.snapshot(), PAYMENT, "open", false).await;
    record_verified(&pool, time.as_ref(), &ids, id, "later-open", &later)
        .await
        .expect("new observation in a later transaction");
    resolve(&pool, time.as_ref(), &ids, id, "later-resolution")
        .await
        .expect("later resolution");
    assert_eq!(
        recognize(&pool, time.as_ref(), &ids, id)
            .await
            .expect("new current recognition")
            .recognition
            .through_observation,
        2
    );
    assert_eq!(
        server.count(),
        1,
        "source seal and later evidence require no parent GET"
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn bounded_native_history_rejects_129th_and_non_read_committed_admission(pool: PgPool) {
    let (time, frozen, ids) = paid(&pool).await;
    let server = Server::start(vec![profile()]);
    let client = local(&server)
        .verify_profile()
        .await
        .expect("profile")
        .into_reconciliation();
    let id = &frozen.snapshot().binding.id;
    assert!(matches!(
        paid_parent_evidence::start(&pool, time.as_ref(), &ids, id, "template", &client)
            .await
            .expect("first operation"),
        Start::Fetch(_)
    ));
    let template: String = sqlx::query_scalar(
        "SELECT id FROM paid_parent_total_operations WHERE attempt_id=$1 AND effect_key='template'",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .expect("template");
    let one = "INSERT INTO paid_parent_total_operations(id,order_id,attempt_id,effect_key,recognition_id,through_observation,profile_id,payment_id,credential_fingerprint,started_at,fetch_before) SELECT $2,order_id,attempt_id,$3,recognition_id,through_observation,profile_id,payment_id,credential_fingerprint,started_at,fetch_before FROM paid_parent_total_operations WHERE id=$1";
    let mut tx = pool.begin().await.expect("repeatable read transaction");
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
        .execute(&mut *tx)
        .await
        .expect("isolation");
    let error = sqlx::query(one)
        .bind(&template)
        .bind("eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee")
        .bind("wrong-isolation")
        .execute(&mut *tx)
        .await
        .expect_err("repeatable read cannot admit operation");
    native_error(error, "Paid parent operation requires read committed");
    tx.rollback().await.expect("rollback");
    let mut tx = pool.begin().await.expect("bounded batch");
    sqlx::query("INSERT INTO paid_parent_total_operations(id,order_id,attempt_id,effect_key,recognition_id,through_observation,profile_id,payment_id,credential_fingerprint,started_at,fetch_before) SELECT lpad(to_hex(200000+n),32,'0'),o.order_id,o.attempt_id,'batch-'||n::text,o.recognition_id,o.through_observation,o.profile_id,o.payment_id,o.credential_fingerprint,o.started_at,o.fetch_before FROM paid_parent_total_operations o CROSS JOIN generate_series(1,127) n WHERE o.id=$1").bind(&template).execute(&mut *tx).await.expect("exactly 128 operations including template");
    sqlx::query("SAVEPOINT before_over_limit")
        .execute(&mut *tx)
        .await
        .expect("savepoint");
    let error = sqlx::query(one)
        .bind(&template)
        .bind("ffffffffffffffffffffffffffffffff")
        .bind("over-limit")
        .execute(&mut *tx)
        .await
        .expect_err("native 129th operation rejected");
    native_error(error, "Paid parent operation limit reached");
    sqlx::query("ROLLBACK TO SAVEPOINT before_over_limit")
        .execute(&mut *tx)
        .await
        .expect("recover valid batch");
    tx.commit()
        .await
        .expect("128 exact source operations commit");
    assert_eq!(counts(&pool).await, (128, 0, 0));
    assert!(matches!(
        paid_parent_evidence::start(&pool, time.as_ref(), &ids, id, "new-over-limit", &client)
            .await,
        Err(PaymentStoreError::Unavailable)
    ));
    assert!(matches!(
        paid_parent_evidence::start(&pool, time.as_ref(), &ids, id, "template", &client)
            .await
            .expect("historical pending replay"),
        Start::Pending(_)
    ));
    assert_eq!(server.count(), 1, "limit checks issue no parent GET");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn start_clock_cannot_precede_received_recognition_or_latest_operation(pool: PgPool) {
    let (time, frozen, ids) = paid(&pool).await;
    let server = Server::start(vec![profile()]);
    let client = local(&server)
        .verify_profile()
        .await
        .expect("profile")
        .into_reconciliation();
    let id = &frozen.snapshot().binding.id;
    time.0.store(1499, Ordering::SeqCst);
    assert!(matches!(
        paid_parent_evidence::start(
            &pool,
            time.as_ref(),
            &ids,
            id,
            "before-recognition",
            &client
        )
        .await,
        Err(PaymentStoreError::ClockUnavailable)
    ));
    time.0.store(1510, Ordering::SeqCst);
    assert!(matches!(
        paid_parent_evidence::start(&pool, time.as_ref(), &ids, id, "latest", &client)
            .await
            .expect("valid operation"),
        Start::Fetch(_)
    ));
    time.0.store(1509, Ordering::SeqCst);
    assert!(matches!(
        paid_parent_evidence::start(&pool, time.as_ref(), &ids, id, "before-latest", &client).await,
        Err(PaymentStoreError::ClockUnavailable)
    ));
    time.0.store(1510, Ordering::SeqCst);
    assert!(matches!(
        paid_parent_evidence::start(&pool, time.as_ref(), &ids, id, "current", &client)
            .await
            .expect("healthy clock"),
        Start::Fetch(_)
    ));
    assert_eq!(counts(&pool).await, (2, 0, 0));
    assert_eq!(server.count(), 1, "clock admission issues no parent GET");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn clock_rewind_while_real_parent_get_waits_rejects_fetched_evidence(pool: PgPool) {
    use std::sync::{Arc, atomic::AtomicBool};
    let (time, frozen, ids) = paid(&pool).await;
    let gate = Arc::new(AtomicBool::new(false));
    let server = Server::start(vec![
        profile(),
        Reply::GateJson(gate.clone(), payment(frozen.snapshot())),
        Reply::Json(200, payment(frozen.snapshot())),
    ]);
    let client = local(&server)
        .verify_profile()
        .await
        .expect("profile")
        .into_reconciliation();
    let id = &frozen.snapshot().binding.id;
    let permit =
        match paid_parent_evidence::start(&pool, time.as_ref(), &ids, id, "rewind", &client)
            .await
            .expect("first operation")
        {
            Start::Fetch(p) => p,
            _ => panic!("permit"),
        };
    let work = paid_parent_evidence::fetch(permit, time.as_ref(), &client);
    let release = async {
        tokio::time::timeout(Duration::from_secs(2), async {
            while server.count() < 2 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("actual GET reached loopback");
        time.0.store(1499, Ordering::SeqCst);
        gate.store(true, Ordering::SeqCst);
    };
    let (result, ()) = tokio::join!(work, release);
    assert!(matches!(
        result,
        Err(MollieError::Custody(PaymentStoreError::ClockUnavailable))
    ));
    assert_eq!(counts(&pool).await, (1, 0, 0));
    time.0.store(1500, Ordering::SeqCst);
    assert!(matches!(
        paid_parent_evidence::start(&pool, time.as_ref(), &ids, id, "rewind", &client)
            .await
            .expect("uncertain operation stays pending"),
        Start::Pending(_)
    ));
    let permit =
        match paid_parent_evidence::start(&pool, time.as_ref(), &ids, id, "healthy", &client)
            .await
            .expect("independent request")
        {
            Start::Fetch(p) => p,
            _ => panic!("healthy permit"),
        };
    let fetched = paid_parent_evidence::fetch(permit, time.as_ref(), &client)
        .await
        .expect("healthy GET");
    paid_parent_evidence::record(&pool, time.as_ref(), &ids, &fetched)
        .await
        .expect("healthy record");
    assert_eq!(counts(&pool).await, (2, 1, 1));
    assert_eq!(server.count(), 3);
}

// Compile the actual worker inspection module against this crate's existing
// canonical reversal-pages fixture, without widening production transport APIs.
use crate::mollie::reversal_worker_fixture as reversal_pages;
#[path = "../../../worker/src/paid_parent.rs"]
mod worker_inspect;

async fn worker_event(pool: &PgPool, attempt: &str, effect: &str) -> Vec<(String, String)> {
    use super::reversal_supervision_cases::worker_entry::Capture;
    use std::sync::{Arc, Mutex};
    use tracing::instrument::WithSubscriber;
    let captured = Arc::new(Mutex::new(Vec::new()));
    let input = worker_inspect::Inspection::parse(attempt, effect).expect("valid worker input");
    worker_inspect::inspect(pool, input)
        .with_subscriber(Capture(captured.clone()))
        .await
        .expect("worker inspection");
    let events = captured.lock().expect("captured fields");
    assert_eq!(events.len(), 1, "one inspection emits one event");
    let mut fields = events[0].clone();
    fields.sort();
    fields
}
fn expected_worker_fields(fields: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut fields: Vec<_> = fields
        .iter()
        .map(|(key, value)| ((*key).into(), (*value).into()))
        .collect();
    fields.sort();
    fields
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn actual_worker_inspects_absent_pending_and_complete_without_effects_or_provider_io(
    pool: PgPool,
) {
    let (time, frozen, ids) = paid(&pool).await;
    let mut source = payment(frozen.snapshot());
    source["amountRefunded"] = json!({"currency":"EUR","value":"0.00"});
    source["amountCaptured"] = json!({"currency":"EUR","value":"382.45"});
    source["amountChargedBack"] = json!({"currency":"EUR","value":"0.00"});
    let server = Server::start(vec![profile(), Reply::Json(200, source)]);
    let client = local(&server)
        .verify_profile()
        .await
        .expect("verified synthetic profile")
        .into_reconciliation();
    let attempt = &frozen.snapshot().binding.id;
    let pending = match paid_parent_evidence::start(
        &pool,
        time.as_ref(),
        &ids,
        attempt,
        "pending-effect",
        &client,
    )
    .await
    .expect("genuine pending operation")
    {
        Start::Fetch(p) => p,
        _ => panic!("first permit"),
    };
    let complete = match paid_parent_evidence::start(
        &pool,
        time.as_ref(),
        &ids,
        attempt,
        "complete-effect",
        &client,
    )
    .await
    .expect("genuine complete operation")
    {
        Start::Fetch(p) => p,
        _ => panic!("second permit"),
    };
    let fetched = paid_parent_evidence::fetch(complete, time.as_ref(), &client)
        .await
        .expect("authenticated loopback parent GET");
    let receipt = paid_parent_evidence::record(&pool, time.as_ref(), &ids, &fetched)
        .await
        .expect("exact receipt");
    assert_eq!(
        (
            receipt.totals.refunded_minor,
            receipt.totals.remaining_minor,
            receipt.totals.captured_minor,
            receipt.totals.charged_back_minor
        ),
        (Some(0), None, Some(38245), Some(0))
    );
    let _pending_permit = pending;
    let before = counts(&pool).await;
    let requests = server.count();
    assert_eq!(before, (2, 1, 1));
    assert_eq!(requests, 2);
    let absent = worker_event(&pool, attempt, "absent-effect").await;
    assert!(
        absent
            == expected_worker_fields(&[
                ("code", "paid_parent_evidence_state"),
                ("state", "absent")
            ])
    );
    let pending = worker_event(&pool, attempt, "pending-effect").await;
    assert!(
        pending
            == expected_worker_fields(&[
                ("code", "paid_parent_evidence_state"),
                ("state", "pending")
            ])
    );
    let complete = worker_event(&pool, attempt, "complete-effect").await;
    assert!(
        complete
            == expected_worker_fields(&[
                ("code", "paid_parent_evidence_state"),
                ("state", "complete"),
                ("refunded_present", "true"),
                ("remaining_present", "false"),
                ("captured_present", "true"),
                ("charged_back_present", "true")
            ])
    );
    let snapshot_sql = "SELECT jsonb_build_object('operation', to_jsonb(o), 'receipt', to_jsonb(r), 'event', to_jsonb(e))::text FROM paid_parent_total_operations o JOIN paid_parent_total_receipts r ON r.operation_id=o.id JOIN commerce_outbox e ON e.id=r.event_id WHERE o.id=$1";
    let original: String = sqlx::query_scalar(snapshot_sql)
        .bind(&receipt.operation.id)
        .fetch_one(&pool)
        .await
        .expect("original evidence snapshot");
    pool.execute(
        "ALTER TABLE paid_parent_total_receipts DISABLE TRIGGER guard_paid_parent_total_receipt",
    )
    .await
    .expect("isolate receipt corruption");
    sqlx::query("UPDATE paid_parent_total_receipts SET refunded_minor=1 WHERE operation_id=$1")
        .bind(&receipt.operation.id)
        .execute(&pool)
        .await
        .expect("corrupt one aggregate cent");
    let damaged: String = sqlx::query_scalar(snapshot_sql)
        .bind(&receipt.operation.id)
        .fetch_one(&pool)
        .await
        .expect("damaged evidence snapshot");
    assert!(damaged != original, "fault changed the receipt");
    {
        use super::reversal_supervision_cases::worker_entry::Capture;
        use std::sync::{Arc, Mutex};
        use tracing::instrument::WithSubscriber;

        let events = Arc::new(Mutex::new(Vec::new()));
        let input = worker_inspect::Inspection::parse(attempt, "complete-effect")
            .expect("valid worker input");
        let result = worker_inspect::inspect(&pool, input)
            .with_subscriber(Capture(events.clone()))
            .await;
        assert!(
            matches!(result, Err(error) if error.to_string() == "paid-parent evidence unavailable")
        );
        assert!(events.lock().expect("captured events").is_empty());
    }
    let after_inspection: String = sqlx::query_scalar(snapshot_sql)
        .bind(&receipt.operation.id)
        .fetch_one(&pool)
        .await
        .expect("evidence after damaged inspection");
    assert!(
        after_inspection == damaged,
        "inspection made no evidence changes"
    );
    assert_eq!(counts(&pool).await, before);
    assert_eq!(server.count(), requests);
    sqlx::query("UPDATE paid_parent_total_receipts SET refunded_minor=0 WHERE operation_id=$1")
        .bind(&receipt.operation.id)
        .execute(&pool)
        .await
        .expect("restore aggregate cent");
    pool.execute(
        "ALTER TABLE paid_parent_total_receipts ENABLE TRIGGER guard_paid_parent_total_receipt",
    )
    .await
    .expect("restore receipt guard");
    let restored: String = sqlx::query_scalar(snapshot_sql)
        .bind(&receipt.operation.id)
        .fetch_one(&pool)
        .await
        .expect("restored evidence snapshot");
    assert!(restored == original, "original evidence restored");
    assert!(worker_event(&pool, attempt, "complete-effect").await == complete);
    assert_eq!(
        server.count(),
        requests,
        "inspection never calls the provider"
    );
    assert_eq!(
        counts(&pool).await,
        before,
        "inspection leaves evidence and outbox unchanged"
    );
}
