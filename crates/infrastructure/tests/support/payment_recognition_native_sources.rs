//! Missing original sources must independently block raw SQL recognition.
//! Register as a child of `payment_recognition_native.rs` to share native fixtures.
use super::*;

async fn remove_source(pool: &PgPool, kind: Option<&str>) -> Value {
    let mut tx = pool.begin().await.expect("owned source-damage fixture");
    tx.execute("ALTER TABLE commerce_outbox DISABLE TRIGGER USER; ALTER TABLE payment_provider_identity_claims DISABLE TRIGGER USER")
        .await.expect("disable only owned source immutability guards");
    let original = if let Some(kind) = kind {
        let row: Value =
            sqlx::query_scalar("SELECT to_jsonb(e) FROM commerce_outbox e WHERE kind=$1")
                .bind(kind)
                .fetch_one(&mut *tx)
                .await
                .expect("exact original event");
        let deleted = sqlx::query("DELETE FROM commerce_outbox WHERE kind=$1")
            .bind(kind)
            .execute(&mut *tx)
            .await
            .expect("remove one required event");
        assert_eq!(deleted.rows_affected(), 1);
        row
    } else {
        let row: Value =
            sqlx::query_scalar("SELECT to_jsonb(c) FROM payment_provider_identity_claims c")
                .fetch_one(&mut *tx)
                .await
                .expect("exact original identity claim");
        let deleted = tx
            .execute("DELETE FROM payment_provider_identity_claims")
            .await
            .expect("remove required provider ownership");
        assert_eq!(deleted.rows_affected(), 1);
        row
    };
    tx.execute("SET CONSTRAINTS ALL IMMEDIATE")
        .await
        .expect("flush genuine foreign-key checks");
    tx.execute("ALTER TABLE commerce_outbox ENABLE TRIGGER USER; ALTER TABLE payment_provider_identity_claims ENABLE TRIGGER USER")
        .await.expect("restore source guards before native recognition");
    tx.commit()
        .await
        .expect("public native guard sees committed damage");
    original
}

async fn restore_source(pool: &PgPool, kind: Option<&str>, original: Value) {
    let mut tx = pool.begin().await.expect("exact source repair");
    tx.execute("ALTER TABLE commerce_outbox DISABLE TRIGGER USER; ALTER TABLE payment_provider_identity_claims DISABLE TRIGGER USER")
        .await.expect("owned fixture repair guards");
    if let Some(kind) = kind {
        sqlx::query("INSERT INTO commerce_outbox SELECT * FROM jsonb_populate_record(NULL::commerce_outbox,$1)")
            .bind(&original).execute(&mut *tx).await.expect("restore exact event bytes and identity");
        let restored: Value =
            sqlx::query_scalar("SELECT to_jsonb(e) FROM commerce_outbox e WHERE kind=$1")
                .bind(kind)
                .fetch_one(&mut *tx)
                .await
                .expect("restored event");
        assert_eq!(restored, original);
    } else {
        sqlx::query("INSERT INTO payment_provider_identity_claims SELECT * FROM jsonb_populate_record(NULL::payment_provider_identity_claims,$1)")
            .bind(&original).execute(&mut *tx).await.expect("restore exact original ownership claim");
        let restored: Value =
            sqlx::query_scalar("SELECT to_jsonb(c) FROM payment_provider_identity_claims c")
                .fetch_one(&mut *tx)
                .await
                .expect("restored original claim");
        assert_eq!(restored, original);
    }
    tx.execute("SET CONSTRAINTS ALL IMMEDIATE")
        .await
        .expect("check original deferred ownership FK before restoring guards");
    tx.execute("ALTER TABLE commerce_outbox ENABLE TRIGGER USER; ALTER TABLE payment_provider_identity_claims ENABLE TRIGGER USER")
        .await.expect("restore all source guards");
    tx.commit()
        .await
        .expect("exact original source repair commits");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn recognition_native_sources_are_required_before_any_canonical_row_exists(pool: PgPool) {
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
    .expect("authenticated paid evidence");
    let resolution = resolve(&pool, time.as_ref(), &identifiers, attempt, "paid")
        .await
        .expect("complete paid prefix and native identity ownership")
        .resolution;
    let candidate = Recognition {
        id: "a".repeat(32),
        order_id: frozen.snapshot().binding.order_id.clone(),
        attempt_id: attempt.clone(),
        resolution_id: resolution.id,
        through_observation: 1,
        recorded_at: 1500,
        state: State::Received,
        currency: commerce_domain::Currency::EUR,
        amount_minor: 38245,
    };
    let physical_before = preserved(&pool).await;
    assert_eq!(recognition_counts(&pool).await, (0, 0, 0));
    for kind in [
        Some("payment.observation_recorded.v1"),
        None,
        Some("checkout.order_created.v1"),
        Some("payment.attempt_prepared.v1"),
        Some("payment.reconciliation_required.v1"),
        Some("payment.identity_resolved.v1"),
    ] {
        let original = remove_source(&pool, kind).await;
        let mut denied = pool.begin().await.expect("raw native importer");
        let error = insert(&mut denied, &candidate)
            .await
            .expect_err("missing authenticated source must deny a fresh native recognition");
        let database = error.as_database_error().expect("native database guard");
        assert_eq!(database.code().as_deref(), Some("23514"), "source {kind:?}");
        assert!(
            database
                .message()
                .contains("complete current authenticated evidence"),
            "must fail native source proof, not uniqueness, rewind or unrelated SQL: {kind:?}"
        );
        denied.rollback().await.expect("failed importer rollback");
        assert_eq!(recognition_counts(&pool).await, (0, 0, 0));
        restore_source(&pool, kind, original).await;

        // This identical candidate must pass every native and deferred guard.
        // Rollback leaves no canonical row that could mask the next source fault.
        let mut healthy = pool.begin().await.expect("paired healthy native control");
        insert(&mut healthy, &candidate)
            .await
            .expect("same candidate passes repaired source proof");
        pointer(&mut healthy, &candidate)
            .await
            .expect("exact current pointer");
        event(&mut healthy, &candidate.id, false)
            .await
            .expect("exact event and digest");
        healthy
            .execute("SET CONSTRAINTS ALL IMMEDIATE")
            .await
            .expect("complete journal, current pointer and event custody");
        let committed_shape: (i64, i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM payment_recognitions),(SELECT count(*) FROM payment_recognition_current),(SELECT count(*) FROM commerce_outbox WHERE kind='payment.recognized.v1')")
            .fetch_one(&mut *healthy).await.expect("atomic positive-control shape");
        assert_eq!(committed_shape, (1, 1, 1));
        healthy
            .rollback()
            .await
            .expect("preserve pristine next control");
        assert_eq!(recognition_counts(&pool).await, (0, 0, 0));
        assert_eq!(preserved(&pool).await, physical_before);
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn recognition_native_rejects_damage_inside_a_multi_observation_prefix(pool: PgPool) {
    let (time, frozen) = prepared(&pool, true).await;
    let attempt = &frozen.snapshot().binding.id;
    let identifiers = ids();
    for (effect, at) in [("first", 1500), ("middle", 1501), ("last", 1502)] {
        time.0.store(at, Ordering::SeqCst);
        record_verified(
            &pool,
            time.as_ref(),
            &identifiers,
            attempt,
            effect,
            &fetched(frozen.snapshot(), "tr_A", "paid", false).await,
        )
        .await
        .expect("authenticated ordered observation");
    }
    let resolution = resolve(&pool, time.as_ref(), &identifiers, attempt, "three")
        .await
        .expect("complete three-observation resolution")
        .resolution;
    let candidate = Recognition {
        id: "a".repeat(32),
        order_id: frozen.snapshot().binding.order_id.clone(),
        attempt_id: attempt.clone(),
        resolution_id: resolution.id,
        through_observation: 3,
        recorded_at: 1502,
        state: State::Received,
        currency: commerce_domain::Currency::EUR,
        amount_minor: 38245,
    };
    let before = preserved(&pool).await;
    for (label, damage) in [
        ("gap", "DELETE FROM payment_observations WHERE revision=2"),
        (
            "middle projection",
            "UPDATE payment_observations SET snapshot=jsonb_set(snapshot,'{intent,payment,amount_minor}','1') WHERE revision=2",
        ),
        (
            "missing middle event",
            "DELETE FROM commerce_outbox WHERE kind='payment.observation_recorded.v1' AND payload->>'observation_id'=(SELECT id FROM payment_observations WHERE revision=2)",
        ),
        (
            "non-exact middle event",
            "UPDATE commerce_outbox SET payload=payload||'{\"extra\":true}'::jsonb WHERE kind='payment.observation_recorded.v1' AND payload->>'observation_id'=(SELECT id FROM payment_observations WHERE revision=2)",
        ),
        (
            "event time mismatch",
            "UPDATE commerce_outbox SET created_at=1502 WHERE kind='payment.observation_recorded.v1' AND payload->>'observation_id'=(SELECT id FROM payment_observations WHERE revision=2)",
        ),
        (
            "time regression",
            "UPDATE payment_observations SET recorded_at=CASE revision WHEN 2 THEN 1502 ELSE 1501 END, snapshot=jsonb_set(snapshot,'{recorded_at}',to_jsonb(CASE revision WHEN 2 THEN 1502 ELSE 1501 END)) WHERE revision IN (2,3); UPDATE commerce_outbox e SET created_at=o.recorded_at FROM payment_observations o WHERE e.kind='payment.observation_recorded.v1' AND e.payload->>'observation_id'=o.id AND o.revision IN (2,3)",
        ),
        (
            "after resolution",
            "UPDATE payment_observations SET recorded_at=1503,snapshot=jsonb_set(snapshot,'{recorded_at}','1503') WHERE revision=3; UPDATE commerce_outbox SET created_at=1503 WHERE kind='payment.observation_recorded.v1' AND payload->>'observation_id'=(SELECT id FROM payment_observations WHERE revision=3)",
        ),
    ] {
        let mut tx = pool.begin().await.expect("isolated damaged source");
        tx.execute("ALTER TABLE payment_observations DISABLE TRIGGER USER; ALTER TABLE commerce_outbox DISABLE TRIGGER USER")
            .await.expect("disable only fixture immutability guards");
        sqlx::raw_sql(damage)
            .execute(&mut *tx)
            .await
            .expect("damage exact source");
        tx.execute("ALTER TABLE payment_observations ENABLE TRIGGER USER; ALTER TABLE commerce_outbox ENABLE TRIGGER USER")
            .await.expect("restore guards before native recognition");
        let error = insert(&mut tx, &candidate)
            .await
            .expect_err("damaged prefix rejected");
        let database = error.as_database_error().expect("native database guard");
        assert_eq!(database.code().as_deref(), Some("23514"), "{label}");
        assert!(
            database
                .message()
                .contains("complete current authenticated evidence"),
            "wrong rejection for {label}"
        );
        tx.rollback().await.expect("restore exact source");

        let mut healthy = pool.begin().await.expect("paired healthy native control");
        insert(&mut healthy, &candidate)
            .await
            .expect("repaired prefix accepted");
        pointer(&mut healthy, &candidate)
            .await
            .expect("current pointer");
        event(&mut healthy, &candidate.id, false)
            .await
            .expect("exact event");
        healthy
            .execute("SET CONSTRAINTS ALL IMMEDIATE")
            .await
            .expect("complete native custody");
        healthy.rollback().await.expect("preserve next control");
        assert_eq!(recognition_counts(&pool).await, (0, 0, 0));
        assert_eq!(preserved(&pool).await, before);
    }
}
