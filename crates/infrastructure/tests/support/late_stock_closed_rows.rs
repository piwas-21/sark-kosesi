//! Coherent matching event digests cannot authorize malformed stored envelopes.
use super::*;
fn malformed(healthy: &Value, late: bool) -> Vec<Value> {
    let mut cases = vec![];
    for key in ["original", "components"] {
        for replacement in [Value::Null, json!({}), json!([])] {
            let mut bad = healthy.clone();
            bad[key] = replacement;
            cases.push(bad);
        }
    }
    let mut bad = healthy.clone();
    bad["original"] = Value::Array(vec![healthy["original"][0].clone(); 21]);
    cases.push(bad);
    let mut bad = healthy.clone();
    bad["components"] = Value::Array(vec![healthy["components"][0].clone(); 1281]);
    cases.push(bad);
    let mut bad = healthy.clone();
    bad["original"][0] = json!([]);
    cases.push(bad);
    let mut bad = healthy.clone();
    bad["original"][0]
        .as_object_mut()
        .expect("healthy captured object")
        .remove("member");
    cases.push(bad);
    let mut bad = healthy.clone();
    bad["original"][0]["member"] = json!([]);
    cases.push(bad);
    for replacement in [
        json!([]),
        Value::Array(vec![
            healthy["original"][0]["member"]["components"][0]
                .clone();
            65
        ]),
    ] {
        let mut bad = healthy.clone();
        bad["original"][0]["member"]["components"] = replacement;
        cases.push(bad);
    }
    let mut bad = healthy.clone();
    bad["original"][0]["member"]["components"][0] = json!([]);
    cases.push(bad);
    for replacement in [
        Value::Null,
        json!([]),
        Value::Array(vec![healthy["original"][0]["pairs"][0].clone(); 65]),
    ] {
        let mut bad = healthy.clone();
        bad["original"][0]["pairs"] = replacement;
        cases.push(bad);
    }
    let mut bad = healthy.clone();
    bad["original"][0]["pairs"][0] = json!([]);
    cases.push(bad);
    let mut bad = healthy.clone();
    bad["components"][0] = json!([]);
    cases.push(bad);
    let mut bad = healthy.clone();
    bad["components"][0]["before"] = json!([]);
    cases.push(bad);
    let mut bad = healthy.clone();
    bad["original"][0]["member"]["units"] = 0.into();
    cases.push(bad);
    let mut bad = healthy.clone();
    bad["original"][0]["pairs"][0]["reserve_digest"] = "bad".into();
    cases.push(bad);
    let mut bad = healthy.clone();
    bad["original"][0]["caller_authorized"] = true.into();
    cases.push(bad);
    let mut bad = healthy.clone();
    bad["original"][0]["member"]["hold_id"] = "f".repeat(32).into();
    cases.push(bad);
    let mut bad = healthy.clone();
    let sequence = bad["original"][0]["pairs"][0]["reserve_sequence"]
        .as_i64()
        .expect("healthy original reserve");
    bad["original"][0]["pairs"][0]["reserve_sequence"] = (sequence + 99999).into();
    cases.push(bad);
    let mut bad = healthy.clone();
    bad["original"][0]["pairs"][0]["reserve_digest"] = "0".repeat(64).into();
    cases.push(bad);
    if !late {
        let mut bad = healthy.clone();
        let sequence = bad["original"][0]["pairs"][0]["terminal_sequence"]
            .as_i64()
            .expect("genuine released member");
        bad["original"][0]["pairs"][0]["terminal_sequence"] = (sequence + 99999).into();
        cases.push(bad);
        let mut bad = healthy.clone();
        bad["original"][0]["pairs"][0]["terminal_digest"] = "0".repeat(64).into();
        cases.push(bad);
    }
    if late {
        for replacement in [
            Value::Null,
            json!([]),
            Value::Array(vec![healthy["fresh"][0].clone(); 21]),
        ] {
            let mut bad = healthy.clone();
            bad["fresh"] = replacement;
            cases.push(bad);
        }
        let mut bad = healthy.clone();
        bad["fresh"][0] = json!([]);
        cases.push(bad);
        let mut bad = healthy.clone();
        bad["fresh"][0]["components"] = json!([]);
        cases.push(bad);
        let mut bad = healthy.clone();
        bad["fresh"][0]["components"][0] = json!([]);
        cases.push(bad);
        for key in ["expired", "reserved", "after"] {
            let mut bad = healthy.clone();
            bad["components"][0][key] = json!([]);
            cases.push(bad);
        }
        let mut bad = healthy.clone();
        bad["fresh"][0]["revision"] = 2.into();
        cases.push(bad);
        let mut bad = healthy.clone();
        for stage in ["before", "expired", "reserved", "after"] {
            let amount = bad["components"][0][stage]["on_hand"]
                .as_i64()
                .expect("closed physical stage");
            bad["components"][0][stage]["on_hand"] = (amount + 1).into();
        }
        cases.push(bad);
    }
    cases
}
async fn closed_rows(pool: PgPool, review: bool) {
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
        .expect("genuine review source");
    }
    let outcome = recover(&pool, f.time.as_ref(), f.ids.as_ref(), &f.order.identity.id)
        .await
        .expect("genuine canonical receipt")
        .outcome;
    let healthy: Value = sqlx::query_scalar(if review {
        "SELECT footprint FROM late_stock_review_cases"
    } else {
        "SELECT footprint FROM late_stock_allocations"
    })
    .fetch_one(&pool)
    .await
    .expect("exact healthy envelope");
    let mutation = if review {
        "UPDATE late_stock_review_cases SET footprint=$1; UPDATE commerce_outbox e SET payload=jsonb_set(e.payload,'{case_sha256}',to_jsonb(late_stock_review_digest(s))) FROM late_stock_review_cases s WHERE e.aggregate_id=s.order_id AND e.kind='order.stock_recovery_review.v1'"
    } else {
        "UPDATE late_stock_allocations SET footprint=$1; UPDATE commerce_outbox e SET payload=jsonb_set(e.payload,'{allocation_sha256}',to_jsonb(late_stock_digest(s))) FROM late_stock_allocations s WHERE e.aggregate_id=s.order_id AND e.kind='order.stock_late_allocated.v1'"
    };
    for bad in malformed(&healthy, !review) {
        edit(&pool, mutation, std::slice::from_ref(&bad)).await;
        let digest_matches:bool=sqlx::query_scalar(if review{"SELECT e.payload->>'case_sha256'=late_stock_review_digest(s) FROM late_stock_review_cases s JOIN commerce_outbox e ON e.aggregate_id=s.order_id AND e.kind='order.stock_recovery_review.v1'"}else{"SELECT e.payload->>'allocation_sha256'=late_stock_digest(s) FROM late_stock_allocations s JOIN commerce_outbox e ON e.aggregate_id=s.order_id AND e.kind='order.stock_late_allocated.v1'"}).fetch_one(&pool).await.expect("coherent independent event digest");
        assert!(
            digest_matches,
            "reach closed stored boundary rather than event mismatch"
        );
        reject(&pool, &f).await;
        edit(&pool, mutation, std::slice::from_ref(&healthy)).await;
        replay_without_ports(&pool, &f, outcome.clone()).await;
    }
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn late_stock_allocation_closed_nested_shapes_and_bounds_reject_even_with_matching_event_then_exact_repair(
    pool: PgPool,
) {
    closed_rows(pool, false).await;
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn late_stock_review_closed_nested_shapes_and_bounds_reject_even_with_matching_event_then_exact_repair(
    pool: PgPool,
) {
    closed_rows(pool, true).await;
}
