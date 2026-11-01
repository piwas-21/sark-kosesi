//! Header/event self-consistency never replaces original received and frozen-source proof.
use super::row_boundaries::other_paid_order;
use super::*;

async fn delayed_received(pool: &PgPool, scarce: bool) -> (Fixture, String, Outcome) {
    let f = accepted(pool, false, false).await;
    resolve(
        pool,
        f.time.as_ref(),
        f.ids.as_ref(),
        &f.frozen.snapshot().binding.id,
        "genuine-zero-prefix",
    )
    .await
    .expect("genuine initial empty prefix");
    let unconfirmed = recognize(
        pool,
        f.time.as_ref(),
        f.ids.as_ref(),
        &f.frozen.snapshot().binding.id,
    )
    .await
    .expect("immutable genuine unconfirmed recognition")
    .recognition
    .id;
    if scarce {
        expire_original(pool, &f).await;
        receive(
            pool,
            "genuine-late-foam-scarcity",
            "SEAT-FOAM",
            WarehouseChange::Count(2),
        )
        .await;
    }
    f.time.0.store(2100, Ordering::SeqCst);
    observe(
        pool,
        &f,
        "tr_DELAYED_PAID_HEADER",
        "paid",
        "genuine-after-deadline-paid",
    )
    .await;
    let recognized = recognize(
        pool,
        f.time.as_ref(),
        f.ids.as_ref(),
        &f.frozen.snapshot().binding.id,
    )
    .await
    .expect("genuine received after original deadline")
    .recognition;
    assert_eq!(recognized.recorded_at, 2100);
    f.time.0.store(2200, Ordering::SeqCst);
    let outcome = recover(pool, f.time.as_ref(), f.ids.as_ref(), &f.order.identity.id)
        .await
        .expect("genuine admitted late allocation or immutable scarcity stop")
        .outcome;
    assert_eq!(matches!(outcome, Outcome::NeedsReview(_)), scarce);
    (f, unconfirmed, outcome)
}
async fn late_header(pool: &PgPool, row: &Value) {
    edit(pool,"UPDATE late_stock_allocations SET attempt_id=$1->>'attempt_id',recognition_id=$1->>'recognition_id',original_group_id=$1->>'original_group_id',fresh_group_id=$1->>'fresh_group_id',allocated_at=($1->>'allocated_at')::bigint,expires_at=($1->>'expires_at')::bigint,footprint=$1->'footprint' WHERE id=$1->>'id'; UPDATE order_stock_allocations SET attempt_id=$1->>'attempt_id' WHERE order_id=$1->>'order_id'; UPDATE commerce_outbox e SET created_at=s.allocated_at,payload=jsonb_build_object('schema_version',1,'order_id',s.order_id,'allocation_id',s.id,'allocation_sha256',late_stock_digest(s)) FROM late_stock_allocations s WHERE e.aggregate_id=s.order_id AND e.kind='order.stock_late_allocated.v1'",std::slice::from_ref(row)).await;
    let matched:bool=sqlx::query_scalar("SELECT e.payload->>'allocation_sha256'=late_stock_digest(s) AND e.created_at=s.allocated_at FROM late_stock_allocations s JOIN commerce_outbox e ON e.aggregate_id=s.order_id AND e.kind='order.stock_late_allocated.v1' WHERE s.id=$1").bind(row["id"].as_str().expect("known stored id")).fetch_one(pool).await.expect("coherent event header positive calibration");
    assert!(matched);
}
async fn case_header(pool: &PgPool, row: &Value) {
    edit(pool,"UPDATE late_stock_review_cases SET attempt_id=$1->>'attempt_id',recognition_id=$1->>'recognition_id',group_id=$1->>'group_id',assessed_at=($1->>'assessed_at')::bigint,footprint=$1->'footprint' WHERE id=$1->>'id'; UPDATE commerce_outbox e SET created_at=s.assessed_at,payload=jsonb_build_object('schema_version',1,'order_id',s.order_id,'case_id',s.id,'case_sha256',late_stock_review_digest(s)) FROM late_stock_review_cases s WHERE e.aggregate_id=s.order_id AND e.kind='order.stock_recovery_review.v1'",std::slice::from_ref(row)).await;
    let matched:bool=sqlx::query_scalar("SELECT e.payload->>'case_sha256'=late_stock_review_digest(s) AND e.created_at=s.assessed_at FROM late_stock_review_cases s JOIN commerce_outbox e ON e.aggregate_id=s.order_id AND e.kind='order.stock_recovery_review.v1' WHERE s.id=$1").bind(row["id"].as_str().expect("known stored id")).fetch_one(pool).await.expect("coherent case event positive calibration");
    assert!(matched);
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn late_stock_history_binds_original_received_order_group_deadline_and_recognition_time_even_after_reseal(
    pool: PgPool,
) {
    let (f, unconfirmed, outcome) = delayed_received(&pool, false).await;
    let second = other_paid_order(&pool, &f).await;
    let healthy: Value =
        sqlx::query_scalar("SELECT to_jsonb(s) FROM late_stock_allocations s WHERE order_id=$1")
            .bind(&f.order.identity.id)
            .fetch_one(&pool)
            .await
            .expect("exact canonical header");
    let other_recognition: String = sqlx::query_scalar(
        "SELECT recognition_id FROM payment_recognition_current WHERE order_id=$1",
    )
    .bind(&second.order.identity.id)
    .fetch_one(&pool)
    .await
    .expect("genuine different paid order recognition");
    let mut cases = vec![];
    let mut bad = healthy.clone();
    bad["recognition_id"] = unconfirmed.into();
    cases.push(bad);
    let mut bad = healthy.clone();
    bad["attempt_id"] = second.frozen.snapshot().binding.id.clone().into();
    bad["recognition_id"] = other_recognition.into();
    cases.push(bad);
    let mut bad = healthy.clone();
    bad["original_group_id"] = second.order.identity.stock_group_id.clone().into();
    cases.push(bad);
    let mut bad = healthy.clone();
    bad["allocated_at"] = 1999.into();
    bad["expires_at"] = 2059.into();
    cases.push(bad);
    let mut bad = healthy.clone();
    bad["allocated_at"] = 2000.into();
    bad["expires_at"] = 2060.into();
    cases.push(bad);
    for bad in cases {
        late_header(&pool, &bad).await;
        reject(&pool, &f).await;
        late_header(&pool, &healthy).await;
        replay_without_ports(&pool, &f, outcome.clone()).await;
    }
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn late_stock_review_history_binds_received_original_group_deadline_clock_and_native_component_order(
    pool: PgPool,
) {
    let (f, unconfirmed, outcome) = delayed_received(&pool, true).await;
    let second = other_paid_order(&pool, &f).await;
    let healthy: Value =
        sqlx::query_scalar("SELECT to_jsonb(s) FROM late_stock_review_cases s WHERE order_id=$1")
            .bind(&f.order.identity.id)
            .fetch_one(&pool)
            .await
            .expect("exact immutable case header");
    let other_recognition: String = sqlx::query_scalar(
        "SELECT recognition_id FROM payment_recognition_current WHERE order_id=$1",
    )
    .bind(&second.order.identity.id)
    .fetch_one(&pool)
    .await
    .expect("genuine different paid order recognition");
    let mut cases = vec![];
    let mut bad = healthy.clone();
    bad["recognition_id"] = unconfirmed.into();
    cases.push(bad);
    let mut bad = healthy.clone();
    bad["attempt_id"] = second.frozen.snapshot().binding.id.clone().into();
    bad["recognition_id"] = other_recognition.into();
    cases.push(bad);
    let mut bad = healthy.clone();
    bad["group_id"] = second.order.identity.stock_group_id.clone().into();
    cases.push(bad);
    let mut bad = healthy.clone();
    bad["assessed_at"] = 1999.into();
    cases.push(bad);
    let mut bad = healthy.clone();
    bad["assessed_at"] = 2000.into();
    cases.push(bad);
    let mut bad = healthy.clone();
    bad["footprint"]["components"]
        .as_array_mut()
        .expect("two genuine aggregate components")
        .reverse();
    cases.push(bad);
    for bad in cases {
        case_header(&pool, &bad).await;
        reject(&pool, &f).await;
        case_header(&pool, &healthy).await;
        replay_without_ports(&pool, &f, outcome.clone()).await;
    }
}
