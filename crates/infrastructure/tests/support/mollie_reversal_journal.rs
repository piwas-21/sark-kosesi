//! Synthetic authenticated loopback evidence against isolated PostgreSQL.
use super::*;
use crate::payment_attempts::reversals::{lookup, record};
use sqlx::Executor;

const PAYMENT: &str = "tr_SYNTHETIC_NotARealPayment";
const REFUND: &str = "re_JOURNAL";
const CHARGEBACK: &str = "chb_JOURNAL";

async fn fetched(attempt: &AttemptSnapshot, child: Value, chargeback: bool) -> VerifiedReversal {
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, payment(attempt)),
        Reply::Json(200, child),
    ]);
    let client = local(&server)
        .verify_profile()
        .await
        .expect("verified test profile")
        .into_reconciliation();
    let result = if chargeback {
        client.fetch_chargeback(attempt, PAYMENT, CHARGEBACK).await
    } else {
        client.fetch_refund(attempt, PAYMENT, REFUND).await
    }
    .expect("authenticated paid parent and exact child");
    server.inspect(|requests| {
        assert_eq!(requests.len(), 3);
        for request in requests {
            assert_eq!(request.method, "GET");
            assert_eq!(
                request.header("Authorization"),
                Some(format!("Bearer {}", raw_key()).as_str())
            );
            assert!(request.body.is_empty());
        }
        assert_eq!(requests[1].path, format!("/v2/payments/{PAYMENT}"));
        let (collection, id) = if chargeback {
            ("chargebacks", CHARGEBACK)
        } else {
            ("refunds", REFUND)
        };
        assert_eq!(
            requests[2].path,
            format!("/v2/payments/{PAYMENT}/{collection}/{id}")
        );
    });
    result
}
fn refund(status: &str, amount: &str) -> Value {
    json!({"resource":"refund","id":REFUND,"paymentId":PAYMENT,"mode":"test",
        "amount":{"currency":"EUR","value":amount},"status":status,
        "createdAt":"2024-03-20T09:13:37.123456789Z","description":"private marker"})
}
fn chargeback(reversed: Option<Value>) -> Value {
    let mut value = json!({"resource":"chargeback","id":CHARGEBACK,"paymentId":PAYMENT,
        "amount":{"currency":"EUR","value":"12.34"},"createdAt":"2024-03-20T09:13:37.123456789Z"});
    if let Some(reversed) = reversed {
        value["reversedAt"] = reversed;
    }
    value
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn authenticated_fact_replays_once_and_does_not_transition_money_or_stock(pool: PgPool) {
    let (time, frozen) = observation_cases::prepared(&pool, true).await;
    let before = crate::payment_test_fixture::counts(&pool).await;
    let verified = fetched(frozen.snapshot(), refund("pending", "12.34"), false).await;
    let ids = observation_cases::ids();
    let first = record(&pool, time.as_ref(), &ids, "journal-first", &verified)
        .await
        .expect("atomic observation");
    assert!(!first.replayed);
    assert_eq!(first.order_id, frozen.snapshot().binding.order_id);
    assert_eq!(first.attempt_id, frozen.snapshot().binding.id);
    assert_eq!(first.phase, ReversalPhase::RefundPending);
    assert_eq!(first.amount_minor, 1234);
    assert_eq!(first.payment_id, PAYMENT);
    assert_eq!(first.child_id, REFUND);
    assert_eq!(first.recorded_at, 1500);
    let replay = record(
        &pool,
        &observation_cases::BadPorts,
        &observation_cases::BadPorts,
        "journal-first",
        &verified,
    )
    .await
    .expect("historical exact replay");
    assert!(replay.replayed);
    assert_eq!(replay.id, first.id);
    let stored = lookup(&pool, &first.attempt_id, "journal-first")
        .await
        .expect("valid receipt")
        .expect("present");
    assert_eq!(stored.id, first.id);
    assert!(
        lookup(&pool, &first.attempt_id, "unknown")
            .await
            .expect("missing")
            .is_none()
    );
    let payloads: Vec<Value> = sqlx::query_scalar(
        "SELECT payload FROM commerce_outbox WHERE kind='payment.reversal_observed.v1'",
    )
    .fetch_all(&pool)
    .await
    .expect("outbox");
    assert_eq!(payloads.len(), 1);
    let payload = payloads[0].as_object().expect("closed event object");
    assert_eq!(payload.len(), 5);
    assert_eq!(payload.get("schema_version"), Some(&json!(1)));
    assert_eq!(payload.get("order_id"), Some(&json!(first.order_id)));
    assert_eq!(payload.get("attempt_id"), Some(&json!(first.attempt_id)));
    assert_eq!(payload.get("observation_id"), Some(&json!(first.id)));
    let digest = payload
        .get("observation_sha256")
        .and_then(Value::as_str)
        .expect("digest");
    assert_eq!(digest.len(), 64);
    assert!(
        digest
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    );
    let after = crate::payment_test_fixture::counts(&pool).await;
    assert_eq!(
        after,
        (before.0, before.1 + 1, before.2, before.3, before.4)
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn concurrent_exact_retry_and_changed_intent_are_serialized(pool: PgPool) {
    let (time, frozen) = observation_cases::prepared(&pool, true).await;
    let verified = fetched(frozen.snapshot(), refund("pending", "12.34"), false).await;
    let changed = fetched(frozen.snapshot(), refund("refunded", "12.34"), false).await;
    let ids = observation_cases::ids();
    let (a, b) = tokio::join!(
        record(&pool, time.as_ref(), &ids, "racing-effect", &verified),
        record(&pool, time.as_ref(), &ids, "racing-effect", &verified)
    );
    let a = a.expect("first contender");
    let b = b.expect("second contender");
    assert_eq!(a.id, b.id);
    assert_ne!(a.replayed, b.replayed);
    assert!(
        record(&pool, time.as_ref(), &ids, "racing-effect", &changed)
            .await
            .is_err()
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM payment_reversal_observations")
        .fetch_one(&pool)
        .await
        .expect("facts");
    assert_eq!(count, 1);
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM commerce_outbox WHERE kind='payment.reversal_observed.v1'",
    )
    .fetch_one(&pool)
    .await
    .expect("events");
    assert_eq!(count, 1);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn rotated_same_profile_key_is_retained_and_changes_the_effect_intent(pool: PgPool) {
    let (time, frozen) = observation_cases::prepared(&pool, true).await;
    let original = fetched(frozen.snapshot(), refund("pending", "12.34"), false).await;
    let key_text = format!("test_{}", "rotated".repeat(5));
    let key = TestCredential::capture(&key_text, "pfl_SYNTHETIC_NotARealProfile")
        .expect("rotated test key");
    let config = config(&key);
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, payment(frozen.snapshot())),
        Reply::Json(200, refund("pending", "12.34")),
    ]);
    let client = TestClient::local(key, config, server.base.clone(), Duration::from_secs(2))
        .expect("loopback")
        .verify_profile()
        .await
        .expect("profile")
        .into_reconciliation();
    let rotated = client
        .fetch_refund(frozen.snapshot(), PAYMENT, REFUND)
        .await
        .expect("authenticated rotated fetch");
    server.inspect(|requests| {
        assert_eq!(requests.len(), 3);
        for request in requests {
            assert_eq!(request.method, "GET");
            assert_eq!(
                request.header("Authorization"),
                Some(format!("Bearer {key_text}").as_str())
            );
            assert!(request.body.is_empty());
        }
    });
    let ids = observation_cases::ids();
    let first = record(&pool, time.as_ref(), &ids, "original-key", &original)
        .await
        .expect("first credential");
    assert!(matches!(
        record(&pool, time.as_ref(), &ids, "original-key", &rotated).await,
        Err(PaymentStoreError::Conflict)
    ));
    let second = record(&pool, time.as_ref(), &ids, "rotated-key", &rotated)
        .await
        .expect("separate rotated observation");
    assert_ne!(first.credential_fingerprint, second.credential_fingerprint);
    assert_eq!(
        second.credential_fingerprint,
        rotated.credential_fingerprint()
    );
    assert_eq!(
        lookup(&pool, &second.attempt_id, "rotated-key")
            .await
            .expect("stored credential")
            .expect("present")
            .credential_fingerprint,
        rotated.credential_fingerprint()
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn distinct_phases_timestamps_and_over_original_amount_are_preserved(pool: PgPool) {
    let (time, frozen) = observation_cases::prepared(&pool, true).await;
    let ids = observation_cases::ids();
    let cases = [
        (
            "over-original",
            refund("refunded", "395.00"),
            false,
            ReversalPhase::RefundRefunded,
            39500,
        ),
        (
            "chargeback-unknown",
            chargeback(None),
            true,
            ReversalPhase::ChargebackReversalUnknown,
            1234,
        ),
        (
            "chargeback-received",
            chargeback(Some(Value::Null)),
            true,
            ReversalPhase::ChargebackReceived,
            1234,
        ),
        (
            "chargeback-reversed",
            chargeback(Some(json!("2024-03-21T09:13:37.987654321Z"))),
            true,
            ReversalPhase::ChargebackReversed,
            1234,
        ),
    ];
    for (effect, body, is_chargeback, phase, amount) in cases {
        let fact = fetched(frozen.snapshot(), body, is_chargeback).await;
        let receipt = record(&pool, time.as_ref(), &ids, effect, &fact)
            .await
            .expect("retain independent source fact");
        assert_eq!(receipt.phase, phase);
        assert_eq!(receipt.amount_minor, amount);
    }
    type StoredObservation = (String, i64, i64, i32, Option<i64>, Option<i32>);
    let rows: Vec<StoredObservation> = sqlx::query_as("SELECT effect_key, amount_minor, source_created_seconds, source_created_nanos, source_reversed_seconds, source_reversed_nanos FROM payment_reversal_observations ORDER BY effect_key")
        .fetch_all(&pool).await.expect("exact source timestamps");
    assert_eq!(
        rows,
        vec![
            (
                "chargeback-received".into(),
                1234,
                1710926017,
                123456789,
                None,
                None
            ),
            (
                "chargeback-reversed".into(),
                1234,
                1710926017,
                123456789,
                Some(1711012417),
                Some(987654321)
            ),
            (
                "chargeback-unknown".into(),
                1234,
                1710926017,
                123456789,
                None,
                None
            ),
            (
                "over-original".into(),
                39500,
                1710926017,
                123456789,
                None,
                None
            ),
        ]
    );
}

async fn native_copy(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    source_id: &str,
    id: &str,
    event_id: &str,
    effect: &str,
) {
    sqlx::query("INSERT INTO payment_reversal_observations(id,attempt_id,order_id,effect_key,payment_id,child_id,phase,amount_minor,credential_fingerprint,source_created_seconds,source_created_nanos,source_reversed_seconds,source_reversed_nanos,recorded_at,event_id) SELECT $1,attempt_id,order_id,$2,payment_id,child_id,phase,amount_minor,credential_fingerprint,source_created_seconds,source_created_nanos,source_reversed_seconds,source_reversed_nanos,recorded_at,$3 FROM payment_reversal_observations WHERE id=$4")
        .bind(id).bind(effect).bind(event_id).bind(source_id).execute(&mut **tx).await.expect("valid copied native fact");
}
async fn native_payload(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>, id: &str) -> Value {
    sqlx::query_scalar("SELECT payment_reversal_observation_event(o) FROM payment_reversal_observations o WHERE o.id=$1")
        .bind(id).fetch_one(&mut **tx).await.expect("canonical stored event")
}
async fn native_event(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    event_id: &str,
    order_id: &str,
    payload: Value,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) VALUES($1,$2,'payment.reversal_observed.v1',$3,1500)")
        .bind(event_id).bind(order_id).bind(payload).execute(&mut **tx).await?;
    Ok(())
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn native_custody_rejects_omitted_detached_and_altered_events_with_positive_control(
    pool: PgPool,
) {
    let (time, frozen) = observation_cases::prepared(&pool, true).await;
    let fact = fetched(frozen.snapshot(), refund("pending", "12.34"), false).await;
    let first = record(
        &pool,
        time.as_ref(),
        &observation_cases::ids(),
        "native-source",
        &fact,
    )
    .await
    .expect("authenticated source");
    let id = "8".repeat(32);
    let event_id = "7".repeat(32);
    let mut tx = pool.begin().await.expect("omitted event transaction");
    native_copy(&mut tx, &first.id, &id, &event_id, "native-next").await;
    assert!(
        tx.commit().await.is_err(),
        "deferred guard must reject an orphan"
    );
    assert!(
        lookup(&pool, &first.attempt_id, "native-next")
            .await
            .expect("absence")
            .is_none()
    );

    let mut tx = pool.begin().await.expect("detached event transaction");
    let detached = json!({"schema_version":1,"order_id":first.order_id,"attempt_id":first.attempt_id,"observation_id":id,"observation_sha256":"0".repeat(64)});
    assert!(
        native_event(&mut tx, &event_id, &first.order_id, detached)
            .await
            .is_err()
    );
    tx.rollback().await.expect("detach rollback");
    for (field, value) in [
        ("schema_version", json!(2)),
        ("order_id", json!("4".repeat(32))),
        ("attempt_id", json!("4".repeat(32))),
        ("observation_id", json!("4".repeat(32))),
        ("observation_sha256", json!("0".repeat(64))),
    ] {
        let mut tx = pool.begin().await.expect("altered event transaction");
        native_copy(&mut tx, &first.id, &id, &event_id, "native-next").await;
        let mut altered = native_payload(&mut tx, &id).await;
        altered[field] = value;
        assert!(
            native_event(&mut tx, &event_id, &first.order_id, altered)
                .await
                .is_err(),
            "{field}"
        );
        tx.rollback().await.expect("alteration rollback");
    }
    let mut tx = pool.begin().await.expect("missing digest transaction");
    native_copy(&mut tx, &first.id, &id, &event_id, "native-next").await;
    let mut incomplete = native_payload(&mut tx, &id).await;
    incomplete
        .as_object_mut()
        .expect("event object")
        .remove("observation_sha256");
    assert!(
        native_event(&mut tx, &event_id, &first.order_id, incomplete)
            .await
            .is_err()
    );
    tx.rollback().await.expect("incomplete event rollback");
    let mut tx = pool.begin().await.expect("positive native transaction");
    native_copy(&mut tx, &first.id, &id, &event_id, "native-next").await;
    let payload = native_payload(&mut tx, &id).await;
    native_event(&mut tx, &event_id, &first.order_id, payload)
        .await
        .expect("exact same-transaction event");
    tx.commit().await.expect("complete paired native fact");
    assert_eq!(
        lookup(&pool, &first.attempt_id, "native-next")
            .await
            .expect("healthy positive control")
            .expect("present")
            .id,
        id
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn suppressed_fact_or_event_never_returns_success_and_rolls_back(pool: PgPool) {
    let (time, frozen) = observation_cases::prepared(&pool, true).await;
    let fact = fetched(frozen.snapshot(), refund("pending", "12.34"), false).await;
    let ids = observation_cases::ids();
    let before = crate::payment_test_fixture::counts(&pool).await;
    pool.execute("CREATE FUNCTION test_suppress_reversal() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RETURN NULL; END $$").await.expect("isolated fault function");
    for (create, remove, effect) in [
        (
            "CREATE TRIGGER suppress_fact BEFORE INSERT ON payment_reversal_observations FOR EACH ROW EXECUTE FUNCTION test_suppress_reversal()",
            "DROP TRIGGER suppress_fact ON payment_reversal_observations",
            "suppressed-fact",
        ),
        (
            "CREATE TRIGGER suppress_event BEFORE INSERT ON commerce_outbox FOR EACH ROW WHEN (NEW.kind='payment.reversal_observed.v1') EXECUTE FUNCTION test_suppress_reversal()",
            "DROP TRIGGER suppress_event ON commerce_outbox",
            "suppressed-event",
        ),
    ] {
        pool.execute(create).await.expect("install isolated fault");
        assert!(
            record(&pool, time.as_ref(), &ids, effect, &fact)
                .await
                .is_err()
        );
        pool.execute(remove).await.expect("remove fault");
        assert!(
            lookup(&pool, &frozen.snapshot().binding.id, effect)
                .await
                .expect("rollback read")
                .is_none()
        );
        assert_eq!(crate::payment_test_fixture::counts(&pool).await, before);
    }
    let healthy = record(&pool, time.as_ref(), &ids, "healthy-control", &fact)
        .await
        .expect("healthy writes succeed");
    assert!(!healthy.replayed);
    assert!(
        lookup(&pool, &healthy.attempt_id, "healthy-control")
            .await
            .expect("positive read")
            .is_some()
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn per_attempt_capacity_denies_new_facts_but_retains_historical_replay(pool: PgPool) {
    let (time, frozen) = observation_cases::prepared(&pool, true).await;
    let fact = fetched(frozen.snapshot(), refund("pending", "12.34"), false).await;
    let ids = observation_cases::ids();
    let first = record(&pool, time.as_ref(), &ids, "capacity-source", &fact)
        .await
        .expect("source positive");
    let mut tx = pool.begin().await.expect("bounded native fixture");
    for i in 0..127_u128 {
        let id = format!("{:032x}", 1_000_000_u128 + i);
        let event_id = format!("{:032x}", 2_000_000_u128 + i);
        let effect = format!("capacity-{i}");
        native_copy(&mut tx, &first.id, &id, &event_id, &effect).await;
        let payload = native_payload(&mut tx, &id).await;
        native_event(&mut tx, &event_id, &first.order_id, payload)
            .await
            .expect("exact paired fixture event");
    }
    tx.commit().await.expect("bounded fixture commits");
    assert!(matches!(
        record(&pool, time.as_ref(), &ids, "capacity-exceeded", &fact).await,
        Err(PaymentStoreError::Unavailable)
    ));
    let replay = record(&pool, time.as_ref(), &ids, "capacity-source", &fact)
        .await
        .expect("historical receipt remains available");
    assert!(replay.replayed);
    assert_eq!(replay.id, first.id);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM payment_reversal_observations")
        .fetch_one(&pool)
        .await
        .expect("bounded facts");
    assert_eq!(count, 128);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn recording_clock_cannot_precede_dispatch_or_previous_fact(pool: PgPool) {
    use std::sync::atomic::Ordering;
    let (time, frozen) = observation_cases::prepared(&pool, true).await;
    let fact = fetched(frozen.snapshot(), refund("pending", "12.34"), false).await;
    let ids = observation_cases::ids();
    time.0.store(1499, Ordering::SeqCst);
    assert!(
        record(&pool, time.as_ref(), &ids, "before-dispatch", &fact)
            .await
            .is_err()
    );
    time.0.store(1600, Ordering::SeqCst);
    record(&pool, time.as_ref(), &ids, "later", &fact)
        .await
        .expect("valid later record");
    time.0.store(1599, Ordering::SeqCst);
    assert!(
        record(&pool, time.as_ref(), &ids, "clock-rewind", &fact)
            .await
            .is_err()
    );
    assert!(
        lookup(&pool, &frozen.snapshot().binding.id, "clock-rewind")
            .await
            .expect("absence")
            .is_none()
    );
    time.0.store(1600, Ordering::SeqCst);
    assert!(
        record(&pool, time.as_ref(), &ids, "equal-time", &fact)
            .await
            .is_ok()
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn damaged_saved_event_fails_lookup_and_replay_until_exactly_restored(pool: PgPool) {
    let (time, frozen) = observation_cases::prepared(&pool, true).await;
    let fact = fetched(frozen.snapshot(), refund("pending", "12.34"), false).await;
    let ids = observation_cases::ids();
    let first = record(&pool, time.as_ref(), &ids, "event-damage", &fact)
        .await
        .expect("complete source receipt");
    let original: Value = sqlx::query_scalar("SELECT payload FROM commerce_outbox WHERE id=$1")
        .bind(&first.event_id)
        .fetch_one(&pool)
        .await
        .expect("exact event");
    let mut tx = pool.begin().await.expect("isolated imported damage");
    tx.execute("ALTER TABLE commerce_outbox DISABLE TRIGGER USER")
        .await
        .expect("test-only damage");
    sqlx::query(
        "UPDATE commerce_outbox SET payload=jsonb_set(payload,'{schema_version}','2') WHERE id=$1",
    )
    .bind(&first.event_id)
    .execute(&mut *tx)
    .await
    .expect("alter event");
    tx.execute("ALTER TABLE commerce_outbox ENABLE TRIGGER USER")
        .await
        .expect("restore guard");
    tx.commit().await.expect("commit isolated damage");
    assert!(matches!(
        lookup(&pool, &first.attempt_id, "event-damage").await,
        Err(PaymentStoreError::InvalidStoredData)
    ));
    assert!(matches!(
        record(&pool, time.as_ref(), &ids, "event-damage", &fact).await,
        Err(PaymentStoreError::InvalidStoredData)
    ));
    let mut tx = pool.begin().await.expect("restore exact event");
    tx.execute("ALTER TABLE commerce_outbox DISABLE TRIGGER USER")
        .await
        .expect("test-only repair");
    sqlx::query("UPDATE commerce_outbox SET payload=$1 WHERE id=$2")
        .bind(original)
        .bind(&first.event_id)
        .execute(&mut *tx)
        .await
        .expect("restore original bytes");
    tx.execute("ALTER TABLE commerce_outbox ENABLE TRIGGER USER")
        .await
        .expect("restore guard");
    tx.commit().await.expect("complete repair");
    let restored = lookup(&pool, &first.attempt_id, "event-damage")
        .await
        .expect("healthy control")
        .expect("present");
    assert_eq!(restored.id, first.id);
}

async fn fetched_identity(
    attempt: &AttemptSnapshot,
    payment_id: &str,
    child_id: &str,
    mut child: Value,
) -> VerifiedReversal {
    let mut parent = payment(attempt);
    parent["id"] = payment_id.into();
    child["paymentId"] = payment_id.into();
    child["id"] = child_id.into();
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, parent),
        Reply::Json(200, child),
    ]);
    let client = local(&server)
        .verify_profile()
        .await
        .expect("profile")
        .into_reconciliation();
    let fact = client
        .fetch_chargeback(attempt, payment_id, child_id)
        .await
        .expect("authenticated parent and chargeback");
    server.inspect(|requests| {
        assert_eq!(requests.len(), 3);
        for request in requests {
            assert_eq!(request.method, "GET");
            assert_eq!(
                request.header("Authorization"),
                Some(format!("Bearer {}", raw_key()).as_str())
            );
            assert!(request.body.is_empty());
        }
        assert_eq!(requests[1].path, format!("/v2/payments/{payment_id}"));
        assert_eq!(
            requests[2].path,
            format!("/v2/payments/{payment_id}/chargebacks/{child_id}")
        );
    });
    fact
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn different_frozen_attempt_and_unavailable_clock_fail_without_effects(pool: PgPool) {
    let (time, frozen) = observation_cases::prepared(&pool, true).await;
    let ids = observation_cases::ids();
    let before = crate::payment_test_fixture::counts(&pool).await;
    let mut alternate = frozen.snapshot().clone();
    alternate.binding.dispatch_before -= 1;
    let alternate_fact = fetched(&alternate, refund("pending", "12.34"), false).await;
    assert!(matches!(
        record(
            &pool,
            time.as_ref(),
            &ids,
            "different-attempt",
            &alternate_fact
        )
        .await,
        Err(PaymentStoreError::Conflict)
    ));
    let fact = fetched(frozen.snapshot(), refund("pending", "12.34"), false).await;
    assert!(matches!(
        record(&pool, &observation_cases::BadPorts, &ids, "no-clock", &fact).await,
        Err(PaymentStoreError::ClockUnavailable)
    ));
    for effect in ["different-attempt", "no-clock"] {
        assert!(
            lookup(&pool, &frozen.snapshot().binding.id, effect)
                .await
                .expect("clean absence")
                .is_none()
        );
    }
    assert_eq!(crate::payment_test_fixture::counts(&pool).await, before);
    assert!(
        record(&pool, time.as_ref(), &ids, "healthy-attempt", &fact)
            .await
            .is_ok()
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn replay_binds_both_provider_ids_amount_and_each_timestamp_component(pool: PgPool) {
    let (time, frozen) = observation_cases::prepared(&pool, true).await;
    let ids = observation_cases::ids();
    let body = chargeback(Some(json!("2024-03-21T09:13:37.987654321Z")));
    let original = fetched_identity(frozen.snapshot(), PAYMENT, CHARGEBACK, body.clone()).await;
    let first = record(&pool, time.as_ref(), &ids, "intent-fields", &original)
        .await
        .expect("first fact");
    let mut variations = Vec::new();
    variations.push(
        fetched_identity(
            frozen.snapshot(),
            "tr_OTHER_PAYMENT",
            CHARGEBACK,
            body.clone(),
        )
        .await,
    );
    variations
        .push(fetched_identity(frozen.snapshot(), PAYMENT, "chb_OTHER_CHILD", body.clone()).await);
    for (field, value) in [
        ("/amount/value", json!("12.35")),
        ("/createdAt", json!("2024-03-20T09:13:38.123456789Z")),
        ("/createdAt", json!("2024-03-20T09:13:37.123456788Z")),
        ("/reversedAt", json!("2024-03-21T09:13:38.987654321Z")),
        ("/reversedAt", json!("2024-03-21T09:13:37.987654320Z")),
    ] {
        let mut changed = body.clone();
        *changed.pointer_mut(field).expect("source field") = value;
        variations.push(fetched_identity(frozen.snapshot(), PAYMENT, CHARGEBACK, changed).await);
    }
    for changed in &variations {
        assert!(matches!(
            record(&pool, time.as_ref(), &ids, "intent-fields", changed).await,
            Err(PaymentStoreError::Conflict)
        ));
    }
    let replay = record(&pool, time.as_ref(), &ids, "intent-fields", &original)
        .await
        .expect("exact positive replay");
    assert!(replay.replayed);
    assert_eq!(replay.id, first.id);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM payment_reversal_observations")
        .fetch_one(&pool)
        .await
        .expect("fact count");
    assert_eq!(count, 1);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn altered_valid_insert_amount_is_detected_and_rolled_back_before_healthy_retry(
    pool: PgPool,
) {
    let (time, frozen) = observation_cases::prepared(&pool, true).await;
    let fact = fetched(frozen.snapshot(), refund("pending", "12.34"), false).await;
    let ids = observation_cases::ids();
    let before = crate::payment_test_fixture::counts(&pool).await;
    pool.execute("CREATE FUNCTION test_alter_reversal_amount() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN NEW.amount_minor := NEW.amount_minor + 1; RETURN NEW; END $$").await.expect("isolated alteration function");
    pool.execute("CREATE TRIGGER alter_fact BEFORE INSERT ON payment_reversal_observations FOR EACH ROW EXECUTE FUNCTION test_alter_reversal_amount()").await.expect("install alteration");
    assert!(matches!(
        record(&pool, time.as_ref(), &ids, "altered-write", &fact).await,
        Err(PaymentStoreError::InvalidStoredData)
    ));
    pool.execute("DROP TRIGGER alter_fact ON payment_reversal_observations")
        .await
        .expect("remove alteration");
    assert!(
        lookup(&pool, &frozen.snapshot().binding.id, "altered-write")
            .await
            .expect("rollback read")
            .is_none()
    );
    assert_eq!(crate::payment_test_fixture::counts(&pool).await, before);
    let healthy = record(&pool, time.as_ref(), &ids, "altered-write", &fact)
        .await
        .expect("healthy retry");
    assert_eq!(healthy.amount_minor, 1234);
    assert!(!healthy.replayed);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn changed_stored_amount_or_phase_breaks_digest_bound_receipt_until_restored(pool: PgPool) {
    let (time, frozen) = observation_cases::prepared(&pool, true).await;
    let fact = fetched(frozen.snapshot(), refund("pending", "12.34"), false).await;
    let ids = observation_cases::ids();
    let first = record(&pool, time.as_ref(), &ids, "row-damage", &fact)
        .await
        .expect("authenticated original");
    for alter in [
        "UPDATE payment_reversal_observations SET amount_minor=1235 WHERE effect_key='row-damage'",
        "UPDATE payment_reversal_observations SET phase='refund_refunded' WHERE effect_key='row-damage'",
    ] {
        let mut tx = pool.begin().await.expect("isolated stored damage");
        tx.execute("ALTER TABLE payment_reversal_observations DISABLE TRIGGER USER")
            .await
            .expect("test-only damage");
        tx.execute(alter)
            .await
            .expect("valid-looking altered source");
        tx.execute("ALTER TABLE payment_reversal_observations ENABLE TRIGGER USER")
            .await
            .expect("restore native guards");
        tx.commit().await.expect("imported damage");
        assert!(matches!(
            lookup(&pool, &first.attempt_id, "row-damage").await,
            Err(PaymentStoreError::InvalidStoredData)
        ));
        assert!(matches!(
            record(&pool, time.as_ref(), &ids, "row-damage", &fact).await,
            Err(PaymentStoreError::InvalidStoredData)
        ));
        let mut tx = pool.begin().await.expect("exact repair");
        tx.execute("ALTER TABLE payment_reversal_observations DISABLE TRIGGER USER")
            .await
            .expect("test-only repair");
        tx.execute("UPDATE payment_reversal_observations SET amount_minor=1234,phase='refund_pending' WHERE effect_key='row-damage'").await.expect("restore original values");
        tx.execute("ALTER TABLE payment_reversal_observations ENABLE TRIGGER USER")
            .await
            .expect("restore native guards");
        tx.commit().await.expect("repaired receipt");
        let replay = record(&pool, time.as_ref(), &ids, "row-damage", &fact)
            .await
            .expect("healthy exact replay");
        assert!(replay.replayed);
        assert_eq!(replay.id, first.id);
    }
}

#[path = "mollie_reversal_journal_upgrade.rs"]
mod upgrade;
