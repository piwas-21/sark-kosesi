//! Calibrated poison proves the entire event/digest expression is bounded first.
use super::*;
async fn oversize(pool: PgPool, review: bool) {
    let f = accepted(&pool, false, true).await;
    f.time.0.store(2000, Ordering::SeqCst);
    if review {
        let holds = original_holds(&pool, &f).await;
        transition_hold(
            &pool,
            &holds[0],
            HoldCommand::Release {
                owner_id: &f.order.identity.id,
                actor_id: "fixture-release",
            },
            1500,
        )
        .await
        .expect("genuine source-bound review");
    }
    let outcome = recover(&pool, f.time.as_ref(), f.ids.as_ref(), &f.order.identity.id)
        .await
        .expect("healthy original receipt")
        .outcome;
    assert_eq!(matches!(outcome, Outcome::NeedsReview(_)), review);
    let (table, function, signature, check) = if review {
        (
            "late_stock_review_cases",
            "late_stock_review_digest",
            "late_stock_review_digest(late_stock_review_cases)",
            "late_stock_review_cases_footprint_check",
        )
    } else {
        (
            "late_stock_allocations",
            "late_stock_digest",
            "late_stock_digest(late_stock_allocations)",
            "late_stock_allocations_footprint_check",
        )
    };
    let exact: Value = sqlx::query_scalar(if review {
        "SELECT to_jsonb(s) FROM late_stock_review_cases s"
    } else {
        "SELECT to_jsonb(s) FROM late_stock_allocations s"
    })
    .fetch_one(&pool)
    .await
    .expect("exact healthy stored footprint");
    let definition: String = sqlx::query_scalar("SELECT pg_get_functiondef($1::regprocedure)")
        .bind(signature)
        .fetch_one(&pool)
        .await
        .expect("exact source function");
    let copy = definition.replacen(&format!("{function}("), "late_fixture_original_digest(", 1);
    assert_ne!(copy, definition);
    sqlx::raw_sql(sqlx::AssertSqlSafe(copy))
        .execute(&pool)
        .await
        .expect("exact private fixture copy");
    let poison = format!(
        "CREATE OR REPLACE FUNCTION {function}(s {table}) RETURNS text LANGUAGE plpgsql IMMUTABLE STRICT AS $$ BEGIN IF octet_length(s.footprint::text)>2097152 THEN RAISE EXCEPTION 'Fixture oversized digest evaluated' USING ERRCODE='P0001'; END IF; RETURN late_fixture_original_digest(s); END $$"
    );
    sqlx::raw_sql(sqlx::AssertSqlSafe(poison))
        .execute(&pool)
        .await
        .expect("oversize-only calibrated digest poison");
    let healthy:bool=sqlx::query_scalar(if review{"SELECT late_stock_review_digest(s)=late_fixture_original_digest(s) FROM late_stock_review_cases s"}else{"SELECT late_stock_digest(s)=late_fixture_original_digest(s) FROM late_stock_allocations s"}).fetch_one(&pool).await.expect("healthy source function unchanged");
    assert!(healthy);
    replay_without_ports(&pool, &f, outcome.clone()).await;
    let original_check:(String,bool)=sqlx::query_as("SELECT pg_get_constraintdef(oid),convalidated FROM pg_constraint WHERE conrelid=$1::regclass AND conname=$2").bind(table).bind(check).fetch_one(&pool).await.expect("exact validated bound check");
    assert!(original_check.1);
    edit(&pool,if review{"ALTER TABLE late_stock_review_cases DROP CONSTRAINT late_stock_review_cases_footprint_check; UPDATE late_stock_review_cases SET footprint=footprint||jsonb_build_object('oversized_padding',repeat('x',2097153))"}else{"ALTER TABLE late_stock_allocations DROP CONSTRAINT late_stock_allocations_footprint_check; UPDATE late_stock_allocations SET footprint=footprint||jsonb_build_object('oversized_padding',repeat('x',2097153))"},&[]).await;
    let bytes: i32 = sqlx::query_scalar(if review {
        "SELECT octet_length(footprint::text) FROM late_stock_review_cases"
    } else {
        "SELECT octet_length(footprint::text) FROM late_stock_allocations"
    })
    .fetch_one(&pool)
    .await
    .expect("oversize actual stored payload");
    assert!(bytes > 2_097_152);
    let poison = sqlx::query_scalar::<_, String>(if review {
        "SELECT late_stock_review_digest(s) FROM late_stock_review_cases s"
    } else {
        "SELECT late_stock_digest(s) FROM late_stock_allocations s"
    })
    .fetch_one(&pool)
    .await
    .expect_err("direct digest work must trigger calibrated detector");
    assert_eq!(
        poison.as_database_error().and_then(|e| e.code()).as_deref(),
        Some("P0001")
    );
    reject(&pool, &f).await;
    edit(&pool,if review{"UPDATE late_stock_review_cases SET footprint=$1->'footprint'; ALTER TABLE late_stock_review_cases ADD CONSTRAINT late_stock_review_cases_footprint_check CHECK(jsonb_typeof(footprint)='object' AND octet_length(footprint::text)<=2097152)"}else{"UPDATE late_stock_allocations SET footprint=$1->'footprint'; ALTER TABLE late_stock_allocations ADD CONSTRAINT late_stock_allocations_footprint_check CHECK(jsonb_typeof(footprint)='object' AND octet_length(footprint::text)<=2097152)"},std::slice::from_ref(&exact)).await;
    sqlx::raw_sql(sqlx::AssertSqlSafe(definition.clone()))
        .execute(&pool)
        .await
        .expect("restore exact real digest function");
    pool.execute(if review {
        "DROP FUNCTION late_fixture_original_digest(late_stock_review_cases)"
    } else {
        "DROP FUNCTION late_fixture_original_digest(late_stock_allocations)"
    })
    .await
    .expect("remove only private fixture function");
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT pg_get_functiondef($1::regprocedure)")
            .bind(signature)
            .fetch_one(&pool)
            .await
            .expect("restored function"),
        definition
    );
    assert_eq!(sqlx::query_as::<_,(String,bool)>("SELECT pg_get_constraintdef(oid),convalidated FROM pg_constraint WHERE conrelid=$1::regclass AND conname=$2").bind(table).bind(check).fetch_one(&pool).await.expect("restored validated check"),original_check);
    replay_without_ports(&pool, &f, outcome).await;
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn late_stock_allocation_oversize_is_rejected_before_poisoned_digest_and_exact_repair_recovers(
    pool: PgPool,
) {
    oversize(pool, false).await;
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn late_stock_review_oversize_is_rejected_before_poisoned_digest_and_exact_repair_recovers(
    pool: PgPool,
) {
    oversize(pool, true).await;
}
