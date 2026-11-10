//! Isolated native faults after real writes; loopback only, no provider calls.
use super::*;
use crate::payment_attempts::{
    PaymentStoreError,
    identities::resolve,
    observations::record_verified,
    paid_parent_evidence::{self, Start},
    recognition::recognize,
};
use commerce_application::payment_requests::PaymentMethod;
use sqlx::{Executor, PgPool};
use std::sync::{Arc, atomic::Ordering};

const PAYMENT: &str = "tr_SYNTHETIC_NotARealPayment";

async fn paid(
    pool: &PgPool,
) -> (
    Arc<crate::payment_test_fixture::quote::Time>,
    FrozenAttempt,
    crate::payment_test_fixture::Ids,
) {
    let (time, frozen) = observation_cases::prepared(pool, true).await;
    let ids = observation_cases::ids();
    let fact = observation_cases::fetched(frozen.snapshot(), PAYMENT, "paid", false).await;
    record_verified(
        pool,
        time.as_ref(),
        &ids,
        &frozen.snapshot().binding.id,
        "paid",
        &fact,
    )
    .await
    .expect("paid fact");
    resolve(
        pool,
        time.as_ref(),
        &ids,
        &frozen.snapshot().binding.id,
        "resolution",
    )
    .await
    .expect("identity");
    recognize(pool, time.as_ref(), &ids, &frozen.snapshot().binding.id)
        .await
        .expect("received");
    (time, frozen, ids)
}

async fn counts(pool: &PgPool) -> (i64, i64, i64) {
    sqlx::query_as("SELECT (SELECT count(*) FROM paid_parent_total_operations),(SELECT count(*) FROM paid_parent_total_receipts),(SELECT count(*) FROM commerce_outbox WHERE kind='payment.paid_parent_totals_observed.v1')")
        .fetch_one(pool).await.expect("effect counts")
}

async fn operation_wait(pool: &PgPool) {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND wait_event='advisory' AND query LIKE '%INSERT INTO paid_parent_total_operations%')")
                .fetch_one(pool).await.expect("operation lock wait");
            if waiting { break; }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }).await.expect("actual operation wait");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn altered_valid_operation_is_rejected_after_actual_insert(pool: PgPool) {
    let (time, frozen, ids) = paid(&pool).await;
    let server = Server::start(vec![profile()]);
    let client = local(&server)
        .verify_profile()
        .await
        .expect("profile")
        .into_reconciliation();
    pool.execute("CREATE FUNCTION test_parent_operation_fingerprint() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN NEW.credential_fingerprint=repeat('0',64); RETURN NEW; END; $$; CREATE TRIGGER z_test_parent_operation_fingerprint BEFORE INSERT ON paid_parent_total_operations FOR EACH ROW EXECUTE FUNCTION test_parent_operation_fingerprint()")
        .await.expect("valid post-guard fingerprint substitution");
    let id = &frozen.snapshot().binding.id;
    assert!(matches!(
        paid_parent_evidence::start(&pool, time.as_ref(), &ids, id, "fault", &client).await,
        Err(PaymentStoreError::InvalidStoredData)
    ));
    assert_eq!(counts(&pool).await, (0, 0, 0));
    pool.execute(
        "DROP TRIGGER z_test_parent_operation_fingerprint ON paid_parent_total_operations",
    )
    .await
    .expect("remove fault");
    assert!(matches!(
        paid_parent_evidence::start(&pool, time.as_ref(), &ids, id, "fault", &client)
            .await
            .expect("positive"),
        Start::Fetch(_)
    ));
    assert_eq!(counts(&pool).await, (1, 0, 0));
    assert_eq!(server.count(), 1);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn expired_during_operation_insert_rolls_back_without_permit(pool: PgPool) {
    let (time, frozen, ids) = paid(&pool).await;
    let server = Server::start(vec![profile()]);
    let client = local(&server)
        .verify_profile()
        .await
        .expect("profile")
        .into_reconciliation();
    pool.execute("CREATE FUNCTION test_parent_operation_wait() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_advisory_xact_lock(1936876952,1); RETURN NEW; END; $$; CREATE TRIGGER z_test_parent_operation_wait AFTER INSERT ON paid_parent_total_operations FOR EACH ROW EXECUTE FUNCTION test_parent_operation_wait()")
        .await.expect("actual write gate");
    let mut gate = pool.begin().await.expect("gate");
    sqlx::query("SELECT pg_advisory_xact_lock(1936876952,1)")
        .execute(&mut *gate)
        .await
        .expect("hold gate");
    let work = paid_parent_evidence::start(
        &pool,
        time.as_ref(),
        &ids,
        &frozen.snapshot().binding.id,
        "wait",
        &client,
    );
    let release = async {
        operation_wait(&pool).await;
        time.0.store(1560, Ordering::SeqCst);
        gate.rollback().await.expect("release gate");
    };
    let (result, ()) = tokio::join!(work, release);
    assert!(matches!(result, Err(PaymentStoreError::Unavailable)));
    assert_eq!(counts(&pool).await, (0, 0, 0));
    assert_eq!(server.count(), 1);
    pool.execute("DROP TRIGGER z_test_parent_operation_wait ON paid_parent_total_operations")
        .await
        .expect("remove gate");
    time.0.store(1500, Ordering::SeqCst);
    assert!(matches!(
        paid_parent_evidence::start(
            &pool,
            time.as_ref(),
            &ids,
            &frozen.snapshot().binding.id,
            "wait",
            &client
        )
        .await
        .expect("positive"),
        Start::Fetch(_)
    ));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn changed_receipt_readback_rolls_back_and_different_history_conflicts(pool: PgPool) {
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
        match paid_parent_evidence::start(&pool, time.as_ref(), &ids, id, "receipt", &client)
            .await
            .expect("start")
        {
            Start::Fetch(p) => p,
            _ => panic!("expected permit"),
        };
    let fetched = paid_parent_evidence::fetch(permit, time.as_ref(), &client)
        .await
        .expect("loopback GET");
    pool.execute("CREATE FUNCTION test_parent_receipt_amount() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN NEW.refunded_minor=111; RETURN NEW; END; $$; CREATE TRIGGER z_test_parent_receipt_amount BEFORE INSERT ON paid_parent_total_receipts FOR EACH ROW EXECUTE FUNCTION test_parent_receipt_amount()")
        .await.expect("valid post-guard amount substitution");
    assert!(matches!(
        paid_parent_evidence::record(&pool, time.as_ref(), &ids, &fetched).await,
        Err(PaymentStoreError::InvalidStoredData)
    ));
    assert_eq!(counts(&pool).await, (1, 0, 0));
    pool.execute("DROP TRIGGER z_test_parent_receipt_amount ON paid_parent_total_receipts")
        .await
        .expect("remove fault");
    let receipt = paid_parent_evidence::record(&pool, time.as_ref(), &ids, &fetched)
        .await
        .expect("positive record");
    assert_eq!(receipt.totals.refunded_minor, None);
    pool.execute("ALTER TABLE paid_parent_total_receipts DISABLE TRIGGER guard_paid_parent_total_receipt; ALTER TABLE commerce_outbox DISABLE TRIGGER guard_commerce_outbox; UPDATE paid_parent_total_receipts SET refunded_minor=111; UPDATE commerce_outbox e SET payload=paid_parent_total_receipt_event(r,o) FROM paid_parent_total_receipts r JOIN paid_parent_total_operations o ON o.id=r.operation_id WHERE e.id=r.event_id")
        .await.expect("isolated coherent receipt substitution");
    assert!(matches!(
        paid_parent_evidence::record(&pool, time.as_ref(), &ids, &fetched).await,
        Err(PaymentStoreError::Conflict)
    ));
    pool.execute("UPDATE paid_parent_total_receipts SET refunded_minor=NULL; UPDATE commerce_outbox e SET payload=paid_parent_total_receipt_event(r,o) FROM paid_parent_total_receipts r JOIN paid_parent_total_operations o ON o.id=r.operation_id WHERE e.id=r.event_id; ALTER TABLE paid_parent_total_receipts ENABLE TRIGGER guard_paid_parent_total_receipt; ALTER TABLE commerce_outbox ENABLE TRIGGER guard_commerce_outbox")
        .await.expect("restore coherent receipt and guards");
    assert!(
        paid_parent_evidence::record(&pool, time.as_ref(), &ids, &fetched)
            .await
            .expect("original replay")
            .replayed
    );
    assert_eq!(counts(&pool).await, (1, 1, 1));
    assert_eq!(server.count(), 2);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn replacement_frozen_attempt_cannot_absorb_earlier_fetched_response(pool: PgPool) {
    let (time, frozen, ids) = paid(&pool).await;
    let original_config = config(&credential());
    let mut altered_policy = original_config.snapshot().clone();
    altered_policy.method = PaymentMethod::Bancontact;
    let altered_config =
        Configuration::capture(altered_policy).expect("valid alternate configuration");
    let mut altered_binding = frozen.snapshot().binding.clone();
    altered_binding.configuration_sha256 = altered_config.identity().sha256;
    let altered = FrozenAttempt::capture(altered_binding, &altered_config)
        .expect("coherent alternate attempt");
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
    let permit = match paid_parent_evidence::start(
        &pool,
        time.as_ref(),
        &ids,
        id,
        "attempt-change",
        &client,
    )
    .await
    .expect("start")
    {
        Start::Fetch(p) => p,
        _ => panic!("expected permit"),
    };
    let fetched = paid_parent_evidence::fetch(permit, time.as_ref(), &client)
        .await
        .expect("loopback GET");
    pool.execute("ALTER TABLE payment_attempts DISABLE TRIGGER guard_payment_attempt")
        .await
        .expect("isolated import corruption");
    sqlx::query("UPDATE payment_attempts SET snapshot=$1,configuration=$2,configuration_material=$3,request_body=$4 WHERE id=$5")
        .bind(serde_json::to_value(altered.snapshot()).expect("snapshot"))
        .bind(serde_json::to_value(altered_config.snapshot()).expect("configuration"))
        .bind(altered_config.fingerprint_material()).bind(altered.body()).bind(id)
        .execute(&pool).await.expect("coherent alternate stored attempt");
    assert!(matches!(
        paid_parent_evidence::record(&pool, time.as_ref(), &ids, &fetched).await,
        Err(PaymentStoreError::InvalidStoredData)
    ));
    assert_eq!(counts(&pool).await, (1, 0, 0));
    sqlx::query("UPDATE payment_attempts SET snapshot=$1,configuration=$2,configuration_material=$3,request_body=$4 WHERE id=$5")
        .bind(serde_json::to_value(frozen.snapshot()).expect("snapshot"))
        .bind(serde_json::to_value(original_config.snapshot()).expect("configuration"))
        .bind(original_config.fingerprint_material()).bind(frozen.body()).bind(id)
        .execute(&pool).await.expect("restore captured attempt");
    pool.execute("ALTER TABLE payment_attempts ENABLE TRIGGER guard_payment_attempt")
        .await
        .expect("restore guard");
    assert!(
        !paid_parent_evidence::record(&pool, time.as_ref(), &ids, &fetched)
            .await
            .expect("positive record")
            .replayed
    );
    assert_eq!(counts(&pool).await, (1, 1, 1));
}
