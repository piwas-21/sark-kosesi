//! Oversized private JSON must never enter native digest work on public reads.
use super::*;
#[sqlx::test(migrator = "MIGRATOR")]
async fn oversized_settlement_footprint_is_rejected_before_digest_evaluation_and_exact_repair_restores_reads(
    pool: PgPool,
) {
    let f = accepted(&pool, false, true).await;
    let original = settle(&pool, f.time.as_ref(), f.ids.as_ref(), &f.order.identity.id)
        .await
        .expect("genuine healthy settlement")
        .settlement;
    backup(&pool).await;
    let original_function: String = sqlx::query_scalar(
        "SELECT pg_get_functiondef('order_stock_digest(order_stock_settlements)'::regprocedure)",
    )
    .fetch_one(&pool)
    .await
    .expect("exact original digest definition");
    let copied_function =
        original_function.replacen("order_stock_digest(", "stock_fixture_original_digest(", 1);
    assert_ne!(
        copied_function, original_function,
        "copy exact function under a fixture-only name"
    );
    sqlx::raw_sql(sqlx::AssertSqlSafe(copied_function))
        .execute(&pool)
        .await
        .expect("private exact original digest copy");
    pool.execute("CREATE OR REPLACE FUNCTION order_stock_digest(s order_stock_settlements) RETURNS text LANGUAGE plpgsql IMMUTABLE STRICT AS $$ BEGIN IF octet_length(s.footprint::text)>1048576 THEN RAISE EXCEPTION 'Fixture oversized digest evaluated' USING ERRCODE='P0001'; END IF; RETURN stock_fixture_original_digest(s); END $$").await.expect("controlled oversized-only digest poison");
    let healthy_same:bool=sqlx::query_scalar("SELECT order_stock_digest(s)=stock_fixture_original_digest(s) FROM order_stock_settlements s").fetch_one(&pool).await.expect("healthy digest positive control");
    assert!(healthy_same);
    assert!(
        read_settlement(&pool, &f.order.identity.id)
            .await
            .expect("healthy original definition remains usable")
            == Some(original.clone())
    );
    let original_check:(String,bool)=sqlx::query_as("SELECT pg_get_constraintdef(oid),convalidated FROM pg_constraint WHERE conrelid='order_stock_settlements'::regclass AND conname='order_stock_settlements_footprint_check'").fetch_one(&pool).await.expect("exact validated source check");
    assert!(original_check.1);
    damage(&pool,"ALTER TABLE order_stock_settlements DROP CONSTRAINT order_stock_settlements_footprint_check; UPDATE order_stock_settlements SET footprint=footprint||jsonb_build_object('oversized_padding',repeat('x',1048577))").await;
    let bytes: i32 =
        sqlx::query_scalar("SELECT octet_length(footprint::text) FROM order_stock_settlements")
            .fetch_one(&pool)
            .await
            .expect("oversize instrument calibration");
    assert!(bytes > 1_048_576);
    let poison = sqlx::query_scalar::<_, String>(
        "SELECT order_stock_digest(s) FROM order_stock_settlements s",
    )
    .fetch_one(&pool)
    .await
    .expect_err("instrument must detect direct oversized digest work");
    assert_eq!(
        poison.as_database_error().and_then(|e| e.code()).as_deref(),
        Some("P0001")
    );
    assert_eq!(
        read_settlement(&pool, &f.order.identity.id).await.err(),
        Some(SettlementError::InvalidStoredData),
        "public reader rejects before poisoned digest, without a storage error"
    );
    assert_eq!(
        settle(
            &pool,
            &super::super::super::observation_cases::BadPorts,
            &super::super::super::observation_cases::BadPorts,
            &f.order.identity.id
        )
        .await
        .err(),
        Some(SettlementError::InvalidStoredData)
    );
    damage(&pool,"UPDATE order_stock_settlements s SET footprint=b.footprint FROM stock_fixture_settlements b WHERE s.id=b.id; ALTER TABLE order_stock_settlements ADD CONSTRAINT order_stock_settlements_footprint_check CHECK(jsonb_typeof(footprint)='object' AND octet_length(footprint::text)<=1048576)").await;
    sqlx::raw_sql(sqlx::AssertSqlSafe(original_function.clone()))
        .execute(&pool)
        .await
        .expect("restore exact native digest definition");
    pool.execute("DROP FUNCTION stock_fixture_original_digest(order_stock_settlements)")
        .await
        .expect("remove only fixture digest copy");
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT pg_get_functiondef('order_stock_digest(order_stock_settlements)'::regprocedure)"
        )
        .fetch_one(&pool)
        .await
        .expect("restored function metadata"),
        original_function
    );
    assert_eq!(sqlx::query_as::<_,(String,bool)>("SELECT pg_get_constraintdef(oid),convalidated FROM pg_constraint WHERE conrelid='order_stock_settlements'::regclass AND conname='order_stock_settlements_footprint_check'").fetch_one(&pool).await.expect("restored validated check"),original_check);
    assert!(
        read_settlement(&pool, &f.order.identity.id)
            .await
            .expect("exact repaired healthy receipt")
            == Some(original)
    );
}
