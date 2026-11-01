//! Raw SQL guards are an independent authority boundary, not Rust validation mirrors.
use super::*;
const INSERT: &str = "INSERT INTO payment_recognitions(id,order_id,attempt_id,resolution_id,through_observation,recorded_at,state,currency,amount_minor) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)";
async fn insert(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    r: &Recognition,
) -> Result<(), sqlx::Error> {
    sqlx::query(INSERT)
        .bind(&r.id)
        .bind(&r.order_id)
        .bind(&r.attempt_id)
        .bind(&r.resolution_id)
        .bind(r.through_observation)
        .bind(r.recorded_at)
        .bind(r.state.code())
        .bind("EUR")
        .bind(r.amount_minor)
        .execute(&mut **tx)
        .await
        .map(|_| ())
}
async fn event(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    id: &str,
    padding: bool,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) SELECT $1,r.order_id,'payment.recognized.v1',jsonb_build_object('schema_version',1,'order_id',r.order_id,'attempt_id',r.attempt_id,'recognition_id',r.id,'recognition_sha256',payment_recognition_digest(r)) || $3::jsonb,r.recorded_at FROM payment_recognitions r WHERE r.id=$2").bind(format!("e{}", &id[1..])).bind(id).bind(if padding{json!({"padding":true})}else{json!({})}).execute(&mut **tx).await.map(|_|())
}
async fn pointer(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    r: &Recognition,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO payment_recognition_current(order_id,recognition_id) VALUES($1,$2) ON CONFLICT(order_id) DO UPDATE SET recognition_id=EXCLUDED.recognition_id")
        .bind(&r.order_id)
        .bind(&r.id)
        .execute(&mut **tx)
        .await
        .map(|_| ())
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn recognition_native_sql_rejects_forged_state_money_binding_prefix_time_and_non_rc_isolation(
    pool: PgPool,
) {
    let (time, f) = prepared(&pool, true).await;
    let a = &f.snapshot().binding.id;
    let i = ids();
    record_verified(
        &pool,
        time.as_ref(),
        &i,
        a,
        "paid",
        &fetched(f.snapshot(), "tr_A", "paid", false).await,
    )
    .await
    .expect("paid");
    let resolution = resolve(&pool, time.as_ref(), &i, a, "paid")
        .await
        .expect("resolution")
        .resolution;
    let r = Recognition {
        id: "a".repeat(32),
        order_id: f.snapshot().binding.order_id.clone(),
        attempt_id: a.clone(),
        resolution_id: resolution.id,
        through_observation: 1,
        recorded_at: 1500,
        state: State::Received,
        currency: commerce_domain::Currency::EUR,
        amount_minor: 38245,
    };
    let changes: [fn(&mut Recognition); 9] = [
        |r| r.state = State::Unconfirmed,
        |r| r.state = State::NeedsReview,
        |r| r.amount_minor = 38244,
        |r| r.order_id = "f".repeat(32),
        |r| r.attempt_id = "f".repeat(32),
        |r| r.resolution_id = "f".repeat(32),
        |r| r.through_observation = 0,
        |r| r.through_observation = 1001,
        |r| r.recorded_at = 1499,
    ];
    for change in changes {
        let mut bad = r.clone();
        change(&mut bad);
        let mut tx = pool.begin().await.expect("hostile native transaction");
        assert!(insert(&mut tx, &bad).await.is_err());
        tx.rollback().await.expect("rollback");
    }
    for isolation in ["REPEATABLE READ", "SERIALIZABLE"] {
        let mut tx = pool.begin().await.expect("non RC");
        tx.execute(if isolation == "REPEATABLE READ" {
            "SET TRANSACTION ISOLATION LEVEL REPEATABLE READ"
        } else {
            "SET TRANSACTION ISOLATION LEVEL SERIALIZABLE"
        })
        .await
        .expect("isolation");
        assert!(
            insert(&mut tx, &r).await.is_err(),
            "native journal must reject {isolation}"
        );
        tx.rollback().await.expect("rollback");
    }
    for (state, currency, money) in [
        ("received", "USD", 38245_i64),
        ("unknown", "EUR", 38245),
        ("received", "EUR", 0),
        ("received", "EUR", 9_007_199_254_740_992),
    ] {
        let error = sqlx::query(INSERT)
            .bind(&r.id)
            .bind(&r.order_id)
            .bind(&r.attempt_id)
            .bind(&r.resolution_id)
            .bind(1_i64)
            .bind(1500_i64)
            .bind(state)
            .bind(currency)
            .bind(money)
            .execute(&pool)
            .await
            .expect_err("literal hostile native row");
        assert_eq!(
            error.as_database_error().and_then(|e| e.code()).as_deref(),
            Some("23514")
        );
    }
    assert_eq!(recognition_counts(&pool).await, (0, 0, 0));
    // Paired healthy native insert proves the rejection instrument reaches valid rows.
    let mut tx = pool.begin().await.expect("positive native");
    insert(&mut tx, &r).await.expect("valid journal");
    pointer(&mut tx, &r).await.expect("valid pointer");
    event(&mut tx, &r.id, false).await.expect("valid event");
    tx.commit().await.expect("complete native commit");
    assert!(current(&pool, a).await.expect("native read") == Some(r.clone()));
    for sql in [
        "UPDATE payment_recognitions SET amount_minor=38245",
        "DELETE FROM payment_recognitions",
        "DELETE FROM payment_recognition_current",
        "UPDATE payment_recognition_current SET recognition_id='ffffffffffffffffffffffffffffffff'",
        "UPDATE commerce_outbox SET created_at=1501 WHERE kind='payment.recognized.v1'",
        "DELETE FROM commerce_outbox WHERE kind='payment.recognized.v1'",
    ] {
        assert!(
            pool.execute(sql).await.is_err(),
            "immutable/detached: {sql}"
        );
    }
    for isolation in ["REPEATABLE READ", "SERIALIZABLE"] {
        let mut tx = pool.begin().await.expect("pointer isolation");
        tx.execute(if isolation == "REPEATABLE READ" {
            "SET TRANSACTION ISOLATION LEVEL REPEATABLE READ"
        } else {
            "SET TRANSACTION ISOLATION LEVEL SERIALIZABLE"
        })
        .await
        .expect("isolation");
        assert!(
            sqlx::query("UPDATE payment_recognition_current SET recognition_id=$1")
                .bind(&r.id)
                .execute(&mut *tx)
                .await
                .is_err(),
            "pointer {isolation}"
        );
        tx.rollback().await.expect("rollback");
    }
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn recognition_native_commit_requires_both_pointer_and_exact_atomic_event(pool: PgPool) {
    let (time, f) = prepared(&pool, true).await;
    let a = &f.snapshot().binding.id;
    let i = ids();
    let resolution = resolve(&pool, time.as_ref(), &i, a, "zero")
        .await
        .expect("zero resolution")
        .resolution;
    let r = Recognition {
        id: "a".repeat(32),
        order_id: f.snapshot().binding.order_id.clone(),
        attempt_id: a.clone(),
        resolution_id: resolution.id,
        through_observation: 0,
        recorded_at: 1500,
        state: State::Unconfirmed,
        currency: commerce_domain::Currency::EUR,
        amount_minor: 38245,
    };
    for (with_pointer, with_event) in [(false, false), (true, false), (false, true)] {
        let mut tx = pool.begin().await.expect("partial native");
        insert(&mut tx, &r).await.expect("valid candidate");
        if with_pointer {
            pointer(&mut tx, &r).await.expect("pointer");
        }
        if with_event {
            event(&mut tx, &r.id, false).await.expect("event");
        }
        assert!(tx.commit().await.is_err(), "partial custody cannot commit");
        assert_eq!(recognition_counts(&pool).await, (0, 0, 0));
    }
    let mut tx = pool.begin().await.expect("forged event");
    insert(&mut tx, &r).await.expect("journal");
    pointer(&mut tx, &r).await.expect("pointer");
    assert!(
        event(&mut tx, &r.id, true).await.is_err(),
        "closed event payload"
    );
    tx.rollback().await.expect("rollback");
    assert!(
        !recognize(&pool, time.as_ref(), &i, a)
            .await
            .expect("healthy retry")
            .replayed
    );
    assert_eq!(recognition_counts(&pool).await, (1, 1, 1));
}

async fn append_paid(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    f: &FrozenAttempt,
    verified: &VerifiedPayment,
) {
    let a = &f.snapshot().binding.id;
    let snapshot = commerce_domain::payment_attempts::observations::ObservationSnapshot {
        schema_version: 1,
        id: "d".repeat(32),
        attempt_id: a.clone(),
        effect_key: "native-paid".into(),
        revision: 1,
        recorded_at: 1500,
        intent: commerce_domain::payment_attempts::observations::ObservationIntent {
            payment: verified.payment().clone(),
            credential_fingerprint: verified.credential_fingerprint().into(),
        },
    };
    sqlx::query("INSERT INTO payment_observations(id,attempt_id,effect_key,revision,recorded_at,provider_id,credential_fingerprint,status,snapshot) VALUES($1,$2,'native-paid',1,1500,'tr_A',$3,'paid',$4)").bind(&snapshot.id).bind(a).bind(&snapshot.intent.credential_fingerprint).bind(serde_json::to_value(&snapshot).expect("snapshot")).execute(&mut **tx).await.expect("native paid append");
    sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) VALUES($1,$2,'payment.observation_recorded.v1',$3,1500)").bind("c".repeat(32)).bind(&f.snapshot().binding.order_id).bind(json!({"schema_version":1,"order_id":f.snapshot().binding.order_id,"attempt_id":a,"observation_id":snapshot.id})).execute(&mut **tx).await.expect("native observation custody");
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn recognition_native_two_advances_in_one_transaction_keep_both_events_and_only_latest_pointer(
    pool: PgPool,
) {
    let (time, f) = prepared(&pool, true).await;
    let a = &f.snapshot().binding.id;
    let i = ids();
    let resolution = resolve(&pool, time.as_ref(), &i, a, "zero")
        .await
        .expect("zero resolution")
        .resolution;
    let before = preserved(&pool).await;
    let verified = fetched(f.snapshot(), "tr_A", "paid", false).await;
    let zero = Recognition {
        id: "a".repeat(32),
        order_id: f.snapshot().binding.order_id.clone(),
        attempt_id: a.clone(),
        resolution_id: resolution.id,
        through_observation: 0,
        recorded_at: 1500,
        state: State::Unconfirmed,
        currency: commerce_domain::Currency::EUR,
        amount_minor: 38245,
    };
    let mut tx = pool.begin().await.expect("one native transaction");
    insert(&mut tx, &zero).await.expect("first journal");
    pointer(&mut tx, &zero).await.expect("first pointer");
    event(&mut tx, &zero.id, false).await.expect("first event");
    append_paid(&mut tx, &f, &verified).await;
    sqlx::query("INSERT INTO payment_identity_resolutions(id,attempt_id,effect_key,revision,through_observation,recorded_at,summary) VALUES($1,$2,'native-paid',2,1,1500,'{\"provider_ids\":[\"tr_A\"],\"paid_provider_ids\":[\"tr_A\"]}')").bind("f".repeat(32)).bind(a).execute(&mut *tx).await.expect("second native resolution");
    sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) VALUES($1,$2,'payment.identity_resolved.v1',$3,1500)").bind("b".repeat(32)).bind(&zero.order_id).bind(json!({"schema_version":1,"order_id":zero.order_id,"attempt_id":a,"resolution_id":"f".repeat(32)})).execute(&mut *tx).await.expect("resolution custody");
    let paid = Recognition {
        id: "9".repeat(32),
        resolution_id: "f".repeat(32),
        through_observation: 1,
        state: State::Received,
        ..zero.clone()
    };
    insert(&mut tx, &paid).await.expect("second journal");
    pointer(&mut tx, &paid).await.expect("latest pointer");
    event(&mut tx, &paid.id, false).await.expect("second event");
    tx.commit()
        .await
        .expect("deferred completeness retains historical first journal");
    assert_eq!(recognition_counts(&pool).await, (2, 1, 2));
    assert!(read_recorded(&pool, a, 0).await.expect("historical zero") == Some(zero));
    assert!(read_recorded(&pool, a, 1).await.expect("historical paid") == Some(paid.clone()));
    assert!(current(&pool, a).await.expect("latest paid") == Some(paid));
    assert_eq!(preserved(&pool).await, before);
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn recognition_native_journal_and_pointer_observe_paid_append_after_actual_attempt_wait(
    pool: PgPool,
) {
    let (time, f) = prepared(&pool, true).await;
    let a = &f.snapshot().binding.id;
    let i = ids();
    resolve(&pool, time.as_ref(), &i, a, "zero")
        .await
        .expect("zero resolution");
    let old = recognize(&pool, time.as_ref(), &i, a)
        .await
        .expect("zero pointer")
        .recognition;
    let verified = fetched(f.snapshot(), "tr_A", "paid", false).await;
    let mut writer = pool.begin().await.expect("append writer");
    sqlx::query("SELECT id FROM payment_attempts WHERE id=$1 FOR UPDATE")
        .bind(a)
        .execute(&mut *writer)
        .await
        .expect("attempt claim");
    append_paid(&mut writer, &f, &verified).await;
    let p = pool.clone();
    let stale = Recognition {
        id: "b".repeat(32),
        ..old.clone()
    };
    let journal = tokio::spawn(async move {
        let mut tx = p.begin().await.expect("journal importer");
        tx.execute("SET LOCAL lock_timeout='1s'; SET LOCAL statement_timeout='2s'")
            .await
            .expect("bounded native wait");
        let error = insert(&mut tx, &stale)
            .await
            .expect_err("stale native journal");
        assert!(error.as_database_error().is_some_and(|e| {
            e.message()
                .contains("complete current authenticated evidence")
        }));
        tx.rollback().await.expect("rollback");
    });
    let p = pool.clone();
    let id = old.id.clone();
    let pointer = tokio::spawn(async move {
        let mut tx = p.begin().await.expect("pointer importer");
        tx.execute("SET LOCAL lock_timeout='1s'; SET LOCAL statement_timeout='2s'")
            .await
            .expect("bounded native wait");
        let error = sqlx::query("UPDATE payment_recognition_current SET recognition_id=$1")
            .bind(id)
            .execute(&mut *tx)
            .await
            .expect_err("stale native pointer");
        assert!(
            error
                .as_database_error()
                .is_some_and(|e| e.message().contains("exact latest recognition"))
        );
        tx.rollback().await.expect("rollback");
    });
    tokio::time::timeout(Duration::from_millis(800),async{loop{let n:i64=sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND (query LIKE 'INSERT INTO payment_recognitions%' OR query LIKE 'UPDATE payment_recognition_current%')").fetch_one(&pool).await.expect("native lock waits");if n>=2{break;}tokio::time::sleep(Duration::from_millis(5)).await;}}).await.expect("both native guards actually wait");
    writer.commit().await.expect("paid append");
    journal.await.expect("fresh journal guard");
    pointer.await.expect("fresh pointer guard");
    assert_eq!(recognition_counts(&pool).await, (1, 1, 1));
    assert_eq!(
        current(&pool, a).await.err(),
        Some(PaymentStoreError::Unavailable)
    );
}

#[path = "payment_recognition_native_sources.rs"]
mod sources;
