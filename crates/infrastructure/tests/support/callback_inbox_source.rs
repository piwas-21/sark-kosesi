//! Bounded global custody and source-only readers remain independently checked.
use super::*;

#[sqlx::test(migrator = "MIGRATOR")]
async fn global_header_audit_accepts_two_hundred_and_rejects_an_imported_two_hundred_first_across_profiles(
    pool: PgPool,
) {
    let clock = time();
    let i = ids();
    for window in 0..2 {
        clock.0.store(1500 + window * 60, Ordering::SeqCst);
        for profile in 0..4 {
            for n in 0..25 {
                admit(
                    &pool,
                    clock.as_ref(),
                    &i,
                    &format!("pfl_BOUND_{profile}"),
                    &format!("tr_BOUND_{window}_{profile}_{n}"),
                )
                .await;
            }
        }
    }
    assert_eq!(intake_counts(&pool).await, (200, 200, 200));
    assert!(
        store::jobs::claim(&pool, clock.as_ref(), &i, "pfl_IDLE")
            .await
            .expect("healthy global boundary before profile filter")
            .is_none()
    );
    let mut tx = pool.begin().await.expect("isolated over-capacity import");
    tx.execute("ALTER TABLE callback_inbox_active DISABLE TRIGGER USER")
        .await
        .expect("fixture bypass only active admission guard");
    tx.commit().await.expect("isolated test fixture bypass");
    clock.0.store(1620, Ordering::SeqCst);
    admit(&pool, clock.as_ref(), &i, "pfl_IMPORTED", "tr_IMPORTED_201").await;
    pool.execute("ALTER TABLE callback_inbox_active ENABLE TRIGGER USER")
        .await
        .expect("restore native guard");
    assert_eq!(intake_counts(&pool).await, (201, 201, 201));
    assert_eq!(
        store::jobs::claim(&pool, clock.as_ref(), &i, "pfl_IDLE")
            .await
            .err(),
        Some(PaymentStoreError::InvalidStoredData),
        "global bound is checked before an idle or unrelated profile can filter out damage"
    );
    assert_eq!(
        store::jobs::sweep(&pool, clock.as_ref(), "pfl_BOUND_0")
            .await
            .err(),
        Some(PaymentStoreError::InvalidStoredData)
    );
    assert_eq!(counts(&pool).await, (0, 0, 0, 0, 0));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn redacted_saved_exception_requires_independent_full_intake_source_even_with_a_real_receipt(
    pool: PgPool,
) {
    let (clock, f) = prepared(&pool, true).await;
    let i = ids();
    admit(&pool, clock.as_ref(), &i, PROFILE, "tr_SOURCE_EXCEPTION").await;
    let server = Server::start(vec![
        profile(),
        Reply::Json(200, response(f.snapshot(), "tr_SOURCE_EXCEPTION", "open")),
    ]);
    let c = client(&server).await;
    let mut claim = store::jobs::claim(&pool, clock.as_ref(), &i, PROFILE)
        .await
        .expect("claim")
        .expect("pending");
    let fetched = c
        .fetch_lookup(claim.take_permit().expect("consumed start"))
        .await
        .expect("authenticated receipt");
    record_lookup(&pool, clock.as_ref(), &i, &claim.operation_id, &fetched)
        .await
        .expect("durable raw receipt");
    sqlx::query("INSERT INTO callback_receipt_processing(operation_id,event_id,processed_at,outcome) SELECT operation_id,id,1500,'custody_invalid' FROM callback_processing_events WHERE operation_id=$1")
        .bind(&claim.operation_id).execute(&pool).await.expect("conservative redacted stop");
    assert!(matches!(
        store::drain::read_disposition(&pool, &claim.operation_id)
            .await
            .expect("healthy exact source"),
        Some(store::drain::Disposition::CustodyInvalid)
    ));
    let payload: Value = sqlx::query_scalar("SELECT payload FROM callback_intake_events")
        .fetch_one(&pool)
        .await
        .expect("original source header");
    let mut tx = pool.begin().await.expect("isolated full source mismatch");
    tx.execute("ALTER TABLE callback_intake_events DISABLE TRIGGER USER")
        .await
        .expect("fixture bypass");
    tx.execute("UPDATE callback_intake_events SET payload=jsonb_set(payload,'{provider_id}','\"tr_OTHER\"')")
        .await.expect("typed but independently false source");
    tx.execute("ALTER TABLE callback_intake_events ENABLE TRIGGER USER")
        .await
        .expect("restore guard");
    tx.commit().await.expect("imported source mismatch");
    assert_eq!(
        store::drain::read_disposition(&pool, &claim.operation_id)
            .await
            .err(),
        Some(PaymentStoreError::InvalidStoredData)
    );
    let mut tx = pool.begin().await.expect("paired source repair");
    tx.execute("ALTER TABLE callback_intake_events DISABLE TRIGGER USER")
        .await
        .expect("fixture repair");
    sqlx::query("UPDATE callback_intake_events SET payload=$1")
        .bind(payload)
        .execute(&mut *tx)
        .await
        .expect("exact original source");
    tx.execute("ALTER TABLE callback_intake_events ENABLE TRIGGER USER")
        .await
        .expect("restore guard");
    tx.commit().await.expect("restored source");
    assert!(matches!(
        store::drain::read_disposition(&pool, &claim.operation_id)
            .await
            .expect("paired healthy proof"),
        Some(store::drain::Disposition::CustodyInvalid)
    ));
    assert_eq!(counts(&pool).await, (1, 1, 1, 1, 0));
    assert_eq!(server.count(), 2);
    get_only(&server);
}
