//! Coherent damaged headers must fail independently of event-digest custody.
use super::*;

const RESEAL: &str = "UPDATE commerce_outbox e SET created_at=r.recorded_at,payload=jsonb_build_object('schema_version',1,'order_id',r.order_id,'attempt_id',r.attempt_id,'recognition_id',r.id,'recognition_sha256',payment_recognition_digest(r)) FROM payment_recognitions r WHERE e.kind='payment.recognized.v1' AND e.payload->>'recognition_id'=r.id";
const RESTORE: &str = "UPDATE payment_recognitions r SET (order_id,attempt_id,resolution_id,through_observation,recorded_at,state,currency,amount_minor)=(SELECT b.order_id,b.attempt_id,b.resolution_id,b.through_observation,b.recorded_at,b.state,b.currency,b.amount_minor FROM recognition_header_rows b WHERE b.id=r.id); UPDATE commerce_outbox e SET (created_at,payload)=(SELECT b.created_at,b.payload FROM recognition_header_events b WHERE b.id=e.id) WHERE e.kind='payment.recognized.v1'";

async fn backup(pool: &PgPool) {
    pool.execute("CREATE TABLE recognition_header_rows AS TABLE payment_recognitions; CREATE TABLE recognition_header_events AS TABLE commerce_outbox")
        .await.expect("exact isolated header and event repair sources");
}
async fn constraint(pool: &PgPool, name: &str) -> (String, bool) {
    sqlx::query_as("SELECT pg_get_constraintdef(oid),convalidated FROM pg_constraint WHERE conrelid='payment_recognitions'::regclass AND conname=$1")
        .bind(name).fetch_one(pool).await.expect("original named and validated constraint")
}
async fn matching_event_seals(pool: &PgPool) {
    // Deliberately build coherent forged envelopes so digest rejection cannot
    // conceal the separate closed-header and monotonic-journal boundaries.
    let matched: bool = sqlx::query_scalar("SELECT bool_and(EXISTS(SELECT 1 FROM commerce_outbox e WHERE e.kind='payment.recognized.v1' AND e.aggregate_id=r.order_id AND e.created_at=r.recorded_at AND e.payload=jsonb_build_object('schema_version',1,'order_id',r.order_id,'attempt_id',r.attempt_id,'recognition_id',r.id,'recognition_sha256',payment_recognition_digest(r)))) FROM payment_recognitions r")
        .fetch_one(pool).await.expect("coherent event-seal fixture control");
    assert!(matched);
}
async fn rejected_public_readers(pool: &PgPool, attempt: &str, prefix: i64) {
    assert_eq!(
        read_recorded(pool, attempt, prefix).await.err(),
        Some(PaymentStoreError::InvalidStoredData),
        "historical stored boundary"
    );
    assert_eq!(
        current(pool, attempt).await.err(),
        Some(PaymentStoreError::InvalidStoredData),
        "current stored boundary"
    );
    assert_eq!(
        recognize(pool, &BadPorts, &BadPorts, attempt).await.err(),
        Some(PaymentStoreError::InvalidStoredData),
        "canonical replay stored boundary"
    );
}
async fn healthy_public_readers(pool: &PgPool, original: &Recognition) {
    assert!(
        read_recorded(pool, &original.attempt_id, original.through_observation)
            .await
            .expect("exact repaired history")
            == Some(original.clone())
    );
    assert!(
        current(pool, &original.attempt_id)
            .await
            .expect("exact repaired current")
            == Some(original.clone())
    );
    let replay = recognize(pool, &BadPorts, &BadPorts, &original.attempt_id)
        .await
        .expect("exact repaired canonical replay without fresh ports");
    assert!(replay.replayed && replay.recognition == *original);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn recognition_coherent_invalid_currency_resolution_identifier_state_and_money_fail_public_reads(
    pool: PgPool,
) {
    let (time, frozen) = prepared(&pool, true).await;
    let attempt = &frozen.snapshot().binding.id;
    let identifiers = ids();
    record_verified(
        &pool,
        time.as_ref(),
        &identifiers,
        attempt,
        "paid",
        &fetched(frozen.snapshot(), "tr_A", "paid", false).await,
    )
    .await
    .expect("authenticated paid fact");
    resolve(&pool, time.as_ref(), &identifiers, attempt, "paid")
        .await
        .expect("paid resolution");
    let original = recognize(&pool, time.as_ref(), &identifiers, attempt)
        .await
        .expect("valid original recognition")
        .recognition;
    let physical = preserved(&pool).await;
    backup(&pool).await;
    for (name, drop_sql, change_sql, restore_constraint) in [
        (
            "payment_recognitions_currency_check",
            "ALTER TABLE payment_recognitions DROP CONSTRAINT payment_recognitions_currency_check",
            "UPDATE payment_recognitions SET currency='USD'",
            "ALTER TABLE payment_recognitions ADD CONSTRAINT payment_recognitions_currency_check CHECK(currency='EUR')",
        ),
        (
            "payment_recognitions_resolution_id_fkey",
            "ALTER TABLE payment_recognitions DROP CONSTRAINT payment_recognitions_resolution_id_fkey",
            "UPDATE payment_recognitions SET resolution_id='broken'",
            "ALTER TABLE payment_recognitions ADD CONSTRAINT payment_recognitions_resolution_id_fkey FOREIGN KEY(resolution_id) REFERENCES payment_identity_resolutions(id)",
        ),
        (
            "payment_recognitions_state_check",
            "ALTER TABLE payment_recognitions DROP CONSTRAINT payment_recognitions_state_check",
            "UPDATE payment_recognitions SET state='closed'",
            "ALTER TABLE payment_recognitions ADD CONSTRAINT payment_recognitions_state_check CHECK(state IN ('unconfirmed','received','needs_review'))",
        ),
    ] {
        let before = constraint(&pool, name).await;
        assert!(before.1);
        damage(&pool, &format!("{drop_sql}; {change_sql}; {RESEAL}")).await;
        matching_event_seals(&pool).await;
        rejected_public_readers(&pool, attempt, 1).await;
        damage(&pool, &format!("{RESTORE}; {restore_constraint}")).await;
        assert_eq!(
            constraint(&pool, name).await,
            before,
            "restore identical validated native constraint"
        );
        matching_event_seals(&pool).await;
        healthy_public_readers(&pool, &original).await;
        assert_eq!(recognition_counts(&pool).await, (1, 1, 1));
        assert_eq!(preserved(&pool).await, physical);
    }
    // Positive bounded money remains structurally valid but must still match the
    // original captured EUR amount; no native money constraint is removed.
    damage(
        &pool,
        &format!("UPDATE payment_recognitions SET amount_minor=1; {RESEAL}"),
    )
    .await;
    matching_event_seals(&pool).await;
    rejected_public_readers(&pool, attempt, 1).await;
    damage(&pool, RESTORE).await;
    matching_event_seals(&pool).await;
    healthy_public_readers(&pool, &original).await;
    assert_eq!(recognition_counts(&pool).await, (1, 1, 1));
    assert_eq!(preserved(&pool).await, physical);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn recognition_coherent_nonmonotonic_journal_rejects_latest_public_history_current_and_replay(
    pool: PgPool,
) {
    let (time, frozen) = prepared(&pool, true).await;
    let attempt = &frozen.snapshot().binding.id;
    let identifiers = ids();
    resolve(&pool, time.as_ref(), &identifiers, attempt, "zero")
        .await
        .expect("genuine prefix zero");
    let older = recognize(&pool, time.as_ref(), &identifiers, attempt)
        .await
        .expect("genuine zero recognition")
        .recognition;
    time.0.store(1600, Ordering::SeqCst);
    record_verified(
        &pool,
        time.as_ref(),
        &identifiers,
        attempt,
        "paid",
        &fetched(frozen.snapshot(), "tr_A", "paid", false).await,
    )
    .await
    .expect("genuine later paid fact");
    resolve(&pool, time.as_ref(), &identifiers, attempt, "paid")
        .await
        .expect("genuine prefix one");
    let latest = recognize(&pool, time.as_ref(), &identifiers, attempt)
        .await
        .expect("genuine paid recognition")
        .recognition;
    assert_eq!((older.recorded_at, latest.recorded_at), (1500, 1600));
    let physical = preserved(&pool).await;
    backup(&pool).await;
    damage(
        &pool,
        &format!(
            "UPDATE payment_recognitions SET recorded_at=1601 WHERE through_observation=0; {RESEAL}"
        ),
    )
    .await;
    matching_event_seals(&pool).await;
    // Original order/attempt/observation/resolution/ownership source facts stay
    // valid; only the recognition journal's chronological relation is damaged.
    assert!(
        crate::payment_attempts::identities::read_recorded(&pool, attempt, "zero")
            .await
            .expect("original source resolution remains valid")
            .is_some()
    );
    assert!(
        crate::payment_attempts::identities::read_recorded(&pool, attempt, "paid")
            .await
            .expect("later source resolution remains valid")
            .is_some()
    );
    rejected_public_readers(&pool, attempt, 1).await;
    damage(&pool, RESTORE).await;
    matching_event_seals(&pool).await;
    healthy_public_readers(&pool, &latest).await;
    assert!(
        read_recorded(&pool, attempt, 0)
            .await
            .expect("exact older receipt repair")
            == Some(older)
    );
    assert_eq!(recognition_counts(&pool).await, (2, 1, 2));
    assert_eq!(preserved(&pool).await, physical);
}
