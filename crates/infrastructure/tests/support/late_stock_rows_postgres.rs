//! Coherent imports cannot replace the one owning allocation with contradictory custody.
use super::*;
use std::sync::atomic::AtomicU64;

pub(super) async fn other_paid_order(pool: &PgPool, _first: &Fixture) -> Fixture {
    let time = Arc::new(crate::payment_test_fixture::quote::Time(AtomicU64::new(
        1500,
    )));
    let store = crate::guest_baskets::GuestSessionStore::new(
        pool.clone(),
        time.clone(),
        Arc::new(crate::payment_test_fixture::Ids(AtomicU64::new(0xfa000000))),
        commerce_application::guest_access::GuestLimits::default(),
    )
    .expect("separate genuine basket/order namespace");
    let second = fixture::accepted_with_store(pool, store, time, true, false).await;
    observe(
        pool,
        &second,
        "tr_HEADER_SECOND",
        "paid",
        "header-second-paid",
    )
    .await;
    recognize(
        pool,
        second.time.as_ref(),
        second.ids.as_ref(),
        &second.frozen.snapshot().binding.id,
    )
    .await
    .expect("genuine different-order received recognition");
    second
}
async fn constraint_definition(pool: &PgPool, table: &str, name: &str) -> (String, bool) {
    sqlx::query_as("SELECT pg_get_constraintdef(oid),convalidated FROM pg_constraint WHERE conrelid=$1::regclass AND conname=$2")
        .bind(table).bind(name).fetch_one(pool).await.expect("exact validated fixture constraint")
}
async fn contradictory_event(pool: &PgPool, f: &Fixture, kind: &str, outcome: &Outcome) {
    let value = json!({"order":f.order.identity.id,"kind":kind});
    edit(pool,"INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) VALUES(repeat('f',32),$1->>'order',$1->>'kind','{}',2000)",std::slice::from_ref(&value)).await;
    reject(pool, f).await;
    edit(
        pool,
        "DELETE FROM commerce_outbox WHERE id=repeat('f',32)",
        &[],
    )
    .await;
    replay_without_ports(pool, f, outcome.clone()).await;
}
async fn fake_original(pool: &PgPool, f: &Fixture, insert: bool) {
    let mut tx = pool
        .begin()
        .await
        .expect("isolated contradictory original receipt");
    tx.execute("ALTER TABLE order_stock_settlements DISABLE TRIGGER USER")
        .await
        .expect("only original receipt fixture guards");
    if insert {
        sqlx::query("INSERT INTO order_stock_settlements(id,order_id,attempt_id,recognition_id,group_id,actor,settled_at,footprint) SELECT repeat('e',32),o.id,a.id,p.recognition_id,o.stock_group_id,'payment-settlement',1500,'{}' FROM checkout_orders o JOIN payment_attempts a ON a.order_id=o.id JOIN payment_recognition_current p ON p.order_id=o.id WHERE o.id=$1")
            .bind(&f.order.identity.id).execute(&mut *tx).await.expect("hostile contradictory owning row");
    } else {
        tx.execute("DELETE FROM order_stock_settlements WHERE id=repeat('e',32)")
            .await
            .expect("remove exact imported contradiction");
    }
    tx.execute("SET CONSTRAINTS ALL IMMEDIATE")
        .await
        .expect("flush internal FK events before guard repair");
    tx.execute("ALTER TABLE order_stock_settlements ENABLE TRIGGER USER")
        .await
        .expect("restore original guard metadata");
    tx.commit()
        .await
        .expect("controlled import visible to public reader");
}
async fn fake_late(pool: &PgPool, f: &Fixture, second: &Fixture, insert: bool) {
    let value = json!({"order":f.order.identity.id,"group":second.order.identity.stock_group_id});
    if insert {
        edit(pool,"INSERT INTO late_stock_allocations(id,order_id,attempt_id,recognition_id,original_group_id,fresh_group_id,actor,allocated_at,expires_at,footprint) SELECT repeat('e',32),o.id,a.id,p.recognition_id,o.stock_group_id,$1->>'group','late-payment-allocation',2000,2060,'{}' FROM checkout_orders o JOIN payment_attempts a ON a.order_id=o.id JOIN payment_recognition_current p ON p.order_id=o.id WHERE o.id=$1->>'order'",std::slice::from_ref(&value)).await;
    } else {
        edit(
            pool,
            "DELETE FROM late_stock_allocations WHERE id=repeat('e',32)",
            &[],
        )
        .await;
    }
}
async fn fake_review(pool: &PgPool, f: &Fixture, insert: bool) {
    let value = json!({"order":f.order.identity.id});
    if insert {
        edit(pool,"INSERT INTO late_stock_review_cases(id,order_id,attempt_id,recognition_id,group_id,actor,reason,assessed_at,footprint) SELECT repeat('e',32),o.id,a.id,p.recognition_id,o.stock_group_id,'late-stock-review','component_scarcity',2000,'{}' FROM checkout_orders o JOIN payment_attempts a ON a.order_id=o.id JOIN payment_recognition_current p ON p.order_id=o.id WHERE o.id=$1->>'order'",std::slice::from_ref(&value)).await;
    } else {
        edit(
            pool,
            "DELETE FROM late_stock_review_cases WHERE id=repeat('e',32)",
            &[],
        )
        .await;
    }
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn late_stock_late_claim_headers_and_cross_path_contradictions_are_closed_and_exactly_repairable(
    pool: PgPool,
) {
    let f = accepted(&pool, false, true).await;
    f.time.0.store(2000, Ordering::SeqCst);
    let outcome = recover(&pool, f.time.as_ref(), f.ids.as_ref(), &f.order.identity.id)
        .await
        .expect("genuine late allocation")
        .outcome;
    let second = other_paid_order(&pool, &f).await;
    let claim: Value =
        sqlx::query_scalar("SELECT to_jsonb(a) FROM order_stock_allocations a WHERE order_id=$1")
            .bind(&f.order.identity.id)
            .fetch_one(&pool)
            .await
            .expect("exact canonical claim");
    for kind in ["order.stock_settled.v1", "order.stock_recovery_review.v1"] {
        contradictory_event(&pool, &f, kind, &outcome).await;
    }
    fake_original(&pool, &f, true).await;
    reject(&pool, &f).await;
    fake_original(&pool, &f, false).await;
    replay_without_ports(&pool, &f, outcome.clone()).await;
    fake_review(&pool, &f, true).await;
    reject(&pool, &f).await;
    fake_review(&pool, &f, false).await;
    replay_without_ports(&pool, &f, outcome.clone()).await;
    let kind_check = constraint_definition(
        &pool,
        "order_stock_allocations",
        "order_stock_allocations_kind_check",
    )
    .await;
    edit(&pool,"ALTER TABLE order_stock_allocations DROP CONSTRAINT order_stock_allocations_kind_check; UPDATE order_stock_allocations SET kind='unknown' WHERE order_id=$1->>'order_id'",std::slice::from_ref(&claim)).await;
    reject(&pool, &f).await;
    edit(&pool,"UPDATE order_stock_allocations SET kind=$1->>'kind' WHERE order_id=$1->>'order_id'; ALTER TABLE order_stock_allocations ADD CONSTRAINT order_stock_allocations_kind_check CHECK(kind IN ('original','late'))",std::slice::from_ref(&claim)).await;
    assert_eq!(
        constraint_definition(
            &pool,
            "order_stock_allocations",
            "order_stock_allocations_kind_check"
        )
        .await,
        kind_check
    );
    replay_without_ports(&pool, &f, outcome.clone()).await;
    let receipt_check = constraint_definition(
        &pool,
        "order_stock_allocations",
        "order_stock_allocations_receipt_id_check",
    )
    .await;
    edit(&pool,"ALTER TABLE order_stock_allocations DROP CONSTRAINT order_stock_allocations_receipt_id_check; UPDATE order_stock_allocations SET receipt_id='broken' WHERE order_id=$1->>'order_id'",std::slice::from_ref(&claim)).await;
    reject(&pool, &f).await;
    edit(&pool,"UPDATE order_stock_allocations SET receipt_id=$1->>'receipt_id' WHERE order_id=$1->>'order_id'; ALTER TABLE order_stock_allocations ADD CONSTRAINT order_stock_allocations_receipt_id_check CHECK(receipt_id ~ '^[a-f0-9]{32}$')",std::slice::from_ref(&claim)).await;
    assert_eq!(
        constraint_definition(
            &pool,
            "order_stock_allocations",
            "order_stock_allocations_receipt_id_check"
        )
        .await,
        receipt_check
    );
    replay_without_ports(&pool, &f, outcome.clone()).await;
    let attempt_fk = constraint_definition(
        &pool,
        "order_stock_allocations",
        "order_stock_allocations_attempt_id_fkey",
    )
    .await;
    edit(&pool,"ALTER TABLE order_stock_allocations DROP CONSTRAINT order_stock_allocations_attempt_id_fkey; UPDATE order_stock_allocations SET attempt_id='broken' WHERE order_id=$1->>'order_id'",std::slice::from_ref(&claim)).await;
    reject(&pool, &f).await;
    edit(&pool,"UPDATE order_stock_allocations SET attempt_id=$1->>'attempt_id' WHERE order_id=$1->>'order_id'; ALTER TABLE order_stock_allocations ADD CONSTRAINT order_stock_allocations_attempt_id_fkey FOREIGN KEY(attempt_id) REFERENCES payment_dispatch_starts(attempt_id)",std::slice::from_ref(&claim)).await;
    assert_eq!(
        constraint_definition(
            &pool,
            "order_stock_allocations",
            "order_stock_allocations_attempt_id_fkey"
        )
        .await,
        attempt_fk
    );
    replay_without_ports(&pool, &f, outcome.clone()).await;
    let wrong = json!({"order":f.order.identity.id,"attempt":second.frozen.snapshot().binding.id});
    edit(
        &pool,
        "UPDATE order_stock_allocations SET attempt_id=$1->>'attempt' WHERE order_id=$1->>'order'",
        std::slice::from_ref(&wrong),
    )
    .await;
    reject(&pool, &f).await;
    edit(&pool,"UPDATE order_stock_allocations SET attempt_id=$1->>'attempt_id' WHERE order_id=$1->>'order_id'",std::slice::from_ref(&claim)).await;
    replay_without_ports(&pool, &f, outcome.clone()).await;
    edit(&pool,"ALTER TABLE order_stock_allocations ALTER COLUMN kind DROP NOT NULL; UPDATE order_stock_allocations SET kind=NULL WHERE order_id=$1->>'order_id'",std::slice::from_ref(&claim)).await;
    reject(&pool, &f).await;
    edit(&pool,"UPDATE order_stock_allocations SET kind=$1->>'kind' WHERE order_id=$1->>'order_id'; ALTER TABLE order_stock_allocations ALTER COLUMN kind SET NOT NULL",std::slice::from_ref(&claim)).await;
    replay_without_ports(&pool, &f, outcome).await;
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn late_stock_original_claim_rejects_wrong_receipt_late_row_and_late_event_then_replays_healthy(
    pool: PgPool,
) {
    let f = accepted(&pool, false, true).await;
    let settled =
        crate::order_stock::settle(&pool, f.time.as_ref(), f.ids.as_ref(), &f.order.identity.id)
            .await
            .expect("genuine canonical original")
            .settlement;
    let second = other_paid_order(&pool, &f).await;
    let outcome = Outcome::Original(settled);
    let claim: Value =
        sqlx::query_scalar("SELECT to_jsonb(a) FROM order_stock_allocations a WHERE order_id=$1")
            .bind(&f.order.identity.id)
            .fetch_one(&pool)
            .await
            .expect("exact original claim");
    edit(&pool,"UPDATE order_stock_allocations SET receipt_id=repeat('f',32) WHERE order_id=$1->>'order_id'",std::slice::from_ref(&claim)).await;
    reject(&pool, &f).await;
    assert_eq!(
        crate::order_stock::read_settlement(&pool, &f.order.identity.id)
            .await
            .err(),
        Some(crate::order_stock::SettlementError::InvalidStoredData),
        "old original reader also independently requires exact shared ownership"
    );
    assert_eq!(
        crate::order_stock::settle(
            &pool,
            &super::super::super::observation_cases::BadPorts,
            &super::super::super::observation_cases::BadPorts,
            &f.order.identity.id
        )
        .await
        .err(),
        Some(crate::order_stock::SettlementError::InvalidStoredData)
    );
    edit(&pool,"UPDATE order_stock_allocations SET receipt_id=$1->>'receipt_id' WHERE order_id=$1->>'order_id'",std::slice::from_ref(&claim)).await;
    replay_without_ports(&pool, &f, outcome.clone()).await;
    fake_late(&pool, &f, &second, true).await;
    reject(&pool, &f).await;
    fake_late(&pool, &f, &second, false).await;
    replay_without_ports(&pool, &f, outcome.clone()).await;
    contradictory_event(&pool, &f, "order.stock_late_allocated.v1", &outcome).await;
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn late_stock_review_index_rejects_original_or_late_receipts_and_events_without_adopting_them(
    pool: PgPool,
) {
    let f = accepted(&pool, false, true).await;
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
    .expect("genuine review stop");
    f.time.0.store(2000, Ordering::SeqCst);
    let outcome = recover(&pool, f.time.as_ref(), f.ids.as_ref(), &f.order.identity.id)
        .await
        .expect("immutable source-bound review")
        .outcome;
    let second = other_paid_order(&pool, &f).await;
    fake_original(&pool, &f, true).await;
    reject(&pool, &f).await;
    fake_original(&pool, &f, false).await;
    replay_without_ports(&pool, &f, outcome.clone()).await;
    fake_late(&pool, &f, &second, true).await;
    reject(&pool, &f).await;
    fake_late(&pool, &f, &second, false).await;
    replay_without_ports(&pool, &f, outcome.clone()).await;
    for kind in ["order.stock_settled.v1", "order.stock_late_allocated.v1"] {
        contradictory_event(&pool, &f, kind, &outcome).await;
    }
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn late_stock_allocation_fence_remains_positive_after_matching_event_digest(pool: PgPool) {
    let f = accepted(&pool, false, true).await;
    f.time.0.store(2000, Ordering::SeqCst);
    let outcome = recover(&pool, f.time.as_ref(), f.ids.as_ref(), &f.order.identity.id)
        .await
        .expect("healthy late receipt")
        .outcome;
    let row: Value = sqlx::query_scalar("SELECT to_jsonb(s) FROM late_stock_allocations s")
        .fetch_one(&pool)
        .await
        .expect("exact original header");
    let check = constraint_definition(
        &pool,
        "late_stock_allocations",
        "late_stock_allocations_fence_key_check",
    )
    .await;
    edit(&pool,"ALTER TABLE late_stock_allocations DROP CONSTRAINT late_stock_allocations_fence_key_check; ALTER TABLE late_stock_allocations ALTER COLUMN fence_key SET GENERATED BY DEFAULT; UPDATE late_stock_allocations SET fence_key=0; UPDATE commerce_outbox e SET payload=jsonb_set(e.payload,'{allocation_sha256}',to_jsonb(late_stock_digest(s))) FROM late_stock_allocations s WHERE e.aggregate_id=s.order_id AND e.kind='order.stock_late_allocated.v1'",&[]).await;
    reject(&pool, &f).await;
    edit(&pool,"UPDATE late_stock_allocations SET fence_key=($1->>'fence_key')::integer; ALTER TABLE late_stock_allocations ALTER COLUMN fence_key SET GENERATED ALWAYS; ALTER TABLE late_stock_allocations ADD CONSTRAINT late_stock_allocations_fence_key_check CHECK(fence_key>0); UPDATE commerce_outbox e SET payload=jsonb_set(e.payload,'{allocation_sha256}',to_jsonb(late_stock_digest(s))) FROM late_stock_allocations s WHERE e.aggregate_id=s.order_id AND e.kind='order.stock_late_allocated.v1'",std::slice::from_ref(&row)).await;
    assert_eq!(
        constraint_definition(
            &pool,
            "late_stock_allocations",
            "late_stock_allocations_fence_key_check"
        )
        .await,
        check
    );
    replay_without_ports(&pool, &f, outcome).await;
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn late_stock_review_fence_and_closed_reason_fail_even_after_matching_event_then_exact_repair(
    pool: PgPool,
) {
    let f = accepted(&pool, false, true).await;
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
    .expect("genuine release stop");
    f.time.0.store(2000, Ordering::SeqCst);
    let outcome = recover(&pool, f.time.as_ref(), f.ids.as_ref(), &f.order.identity.id)
        .await
        .expect("healthy immutable review")
        .outcome;
    let row: Value = sqlx::query_scalar("SELECT to_jsonb(s) FROM late_stock_review_cases s")
        .fetch_one(&pool)
        .await
        .expect("exact original case header");
    let check = constraint_definition(
        &pool,
        "late_stock_review_cases",
        "late_stock_review_cases_fence_key_check",
    )
    .await;
    edit(&pool,"ALTER TABLE late_stock_review_cases DROP CONSTRAINT late_stock_review_cases_fence_key_check; ALTER TABLE late_stock_review_cases ALTER COLUMN fence_key SET GENERATED BY DEFAULT; UPDATE late_stock_review_cases SET fence_key=0; UPDATE commerce_outbox e SET payload=jsonb_set(e.payload,'{case_sha256}',to_jsonb(late_stock_review_digest(s))) FROM late_stock_review_cases s WHERE e.aggregate_id=s.order_id AND e.kind='order.stock_recovery_review.v1'",&[]).await;
    reject(&pool, &f).await;
    edit(&pool,"UPDATE late_stock_review_cases SET fence_key=($1->>'fence_key')::integer; ALTER TABLE late_stock_review_cases ALTER COLUMN fence_key SET GENERATED ALWAYS; ALTER TABLE late_stock_review_cases ADD CONSTRAINT late_stock_review_cases_fence_key_check CHECK(fence_key>0); UPDATE commerce_outbox e SET payload=jsonb_set(e.payload,'{case_sha256}',to_jsonb(late_stock_review_digest(s))) FROM late_stock_review_cases s WHERE e.aggregate_id=s.order_id AND e.kind='order.stock_recovery_review.v1'",std::slice::from_ref(&row)).await;
    assert_eq!(
        constraint_definition(
            &pool,
            "late_stock_review_cases",
            "late_stock_review_cases_fence_key_check"
        )
        .await,
        check
    );
    replay_without_ports(&pool, &f, outcome.clone()).await;
    let reason_check = constraint_definition(
        &pool,
        "late_stock_review_cases",
        "late_stock_review_cases_reason_check",
    )
    .await;
    edit(&pool,"ALTER TABLE late_stock_review_cases DROP CONSTRAINT late_stock_review_cases_reason_check; UPDATE late_stock_review_cases SET reason='unknown'; UPDATE commerce_outbox e SET payload=jsonb_set(e.payload,'{case_sha256}',to_jsonb(late_stock_review_digest(s))) FROM late_stock_review_cases s WHERE e.aggregate_id=s.order_id AND e.kind='order.stock_recovery_review.v1'",&[]).await;
    reject(&pool, &f).await;
    edit(&pool,"UPDATE late_stock_review_cases SET reason=$1->>'reason'; ALTER TABLE late_stock_review_cases ADD CONSTRAINT late_stock_review_cases_reason_check CHECK(reason IN ('component_scarcity','release_intent_unresolved','prior_consumption_without_allocation')); UPDATE commerce_outbox e SET payload=jsonb_set(e.payload,'{case_sha256}',to_jsonb(late_stock_review_digest(s))) FROM late_stock_review_cases s WHERE e.aggregate_id=s.order_id AND e.kind='order.stock_recovery_review.v1'",std::slice::from_ref(&row)).await;
    assert_eq!(
        constraint_definition(
            &pool,
            "late_stock_review_cases",
            "late_stock_review_cases_reason_check"
        )
        .await,
        reason_check
    );
    replay_without_ports(&pool, &f, outcome).await;
}
