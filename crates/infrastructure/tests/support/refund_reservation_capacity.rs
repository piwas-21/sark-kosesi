//! Maximum valid recognition history under the production transaction limits.
//! This module is nested beneath `refund_reservations_cases` to reuse its fixture.
use super::*;
use crate::payment_attempts::{
    identities::resolve,
    observations::record_verified,
    recognition::{State, recognize},
};

#[sqlx::test(migrator = "MIGRATOR")]
async fn sixteen_distinct_received_proofs_at_maximum_observation_prefix_remain_usable(
    pool: PgPool,
) {
    let f = accepted(&pool, false, true).await;
    let token = staff(&pool, "refund-finance", 94).await;
    let order = &f.order.identity.id;
    let attempt = &f.frozen.snapshot().binding.id;
    let physical = footprint(&pool).await;

    // Copy only the authenticated first paid observation; ordinary INSERT triggers
    // validate every contiguous revision, snapshot, and reciprocal outbox event.
    // Separate identifiers are reserved for fixture copies and event rows.
    let mut tx = pool.begin().await.expect("bulk fixture transaction");
    let inserted = sqlx::query(r#"
        INSERT INTO payment_observations(id,attempt_id,effect_key,revision,recorded_at,provider_id,credential_fingerprint,status,snapshot)
        SELECT 'a'||lpad(to_hex(n),31,'0'), o.attempt_id, 'capacity-observation-'||n, n,
               o.recorded_at,o.provider_id,o.credential_fingerprint,o.status,
               jsonb_set(jsonb_set(jsonb_set(o.snapshot,'{id}',to_jsonb('a'||lpad(to_hex(n),31,'0'))),
                    '{effect_key}',to_jsonb('capacity-observation-'||n)), '{revision}',to_jsonb(n))
        FROM payment_observations o CROSS JOIN generate_series(2::bigint,984::bigint) AS n
        WHERE o.attempt_id=$1 AND o.revision=1 ORDER BY n
    "#).bind(attempt).execute(&mut *tx).await.expect("native contiguous observations");
    assert_eq!(inserted.rows_affected(), 983);
    let events = sqlx::query(r#"
        INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at)
        SELECT 'b'||lpad(to_hex(o.revision),31,'0'), $2, 'payment.observation_recorded.v1',
               jsonb_build_object('schema_version',1,'order_id',$2,'attempt_id',o.attempt_id,'observation_id',o.id),o.recorded_at
        FROM payment_observations o WHERE o.attempt_id=$1 AND o.revision BETWEEN 2 AND 984 ORDER BY o.revision
    "#).bind(attempt).bind(order).execute(&mut *tx).await.expect("native matching observation events");
    assert_eq!(events.rows_affected(), 983);
    tx.commit()
        .await
        .expect("native deferred observation custody");

    let verified =
        super::super::order_stock_cases::fixture::verified(&f.frozen, "tr_SETTLEMENT", "paid")
            .await;
    let lines = [
        RefundLine {
            sku: "SET-COVERS",
            gross_minor: 1,
        },
        RefundLine {
            sku: "SET-FILLED",
            gross_minor: 0,
        },
    ];
    let mut first = None;
    let mut proofs = Vec::new();
    for prefix in 985..=1000 {
        let effect = format!("capacity-verified-{prefix}");
        let observation = record_verified(
            &pool,
            f.time.as_ref(),
            f.ids.as_ref(),
            attempt,
            &effect,
            &verified,
        )
        .await
        .expect("validated observation at capacity prefix");
        assert_eq!(observation.snapshot.revision, prefix);
        let resolved = resolve(&pool, f.time.as_ref(), f.ids.as_ref(), attempt, &effect)
            .await
            .expect("current complete identity resolution");
        assert_eq!(resolved.resolution.through_observation, prefix);
        let receipt = recognize(&pool, f.time.as_ref(), f.ids.as_ref(), attempt)
            .await
            .expect("received recognition for exact prefix");
        assert_eq!(
            (
                receipt.recognition.state,
                receipt.recognition.through_observation,
                receipt.recognition.amount_minor
            ),
            (State::Received, prefix, 25900)
        );
        assert!(!receipt.replayed);
        let key = format!("capacity-reservation-{prefix}");
        let reservation = reserve(
            &pool,
            &token,
            f.time.as_ref(),
            f.ids.as_ref(),
            request(order, &receipt.recognition.id, &key, &lines, 0),
        )
        .await
        .expect("one cent with 2-second native statement limit");
        assert_eq!(i64::from(reservation.generation), prefix - 984);
        assert_eq!(reservation.allocation.total.gross_minor, 1);
        if first.is_none() {
            first = Some((receipt.recognition.id.clone(), reservation.id.clone()));
        }
        proofs.push(receipt.recognition.id);
    }
    proofs.sort();
    proofs.dedup();
    assert_eq!(proofs.len(), 16);
    let (proof, first_id) = first.expect("first reservation");
    let replay = reserve(
        &pool,
        &token,
        f.time.as_ref(),
        f.ids.as_ref(),
        request(order, &proof, "capacity-reservation-985", &lines, 0),
    )
    .await
    .expect("historical first receipt under full journal");
    assert!(replay.replayed);
    assert_eq!(replay.id, first_id);
    let shape: (i64,i64,i64,i64,i64,i64,i64) = sqlx::query_as(r#"
        SELECT (SELECT count(*) FROM payment_observations WHERE attempt_id=$1),
               (SELECT count(*) FROM payment_identity_resolutions WHERE attempt_id=$1 AND through_observation BETWEEN 985 AND 1000),
               (SELECT count(*) FROM commerce_outbox e JOIN payment_identity_resolutions r ON r.id=e.payload->>'resolution_id' WHERE e.kind='payment.identity_resolved.v1' AND r.attempt_id=$1 AND r.through_observation BETWEEN 985 AND 1000),
               (SELECT count(*) FROM payment_recognitions WHERE attempt_id=$1 AND through_observation BETWEEN 985 AND 1000 AND state='received'),
               (SELECT count(*) FROM commerce_outbox WHERE kind='payment.recognized.v1' AND aggregate_id=$2),
               (SELECT count(*) FROM refund_reservations WHERE order_id=$2),
               (SELECT coalesce(sum((allocation->'total'->>'gross_minor')::bigint),0)::bigint FROM refund_reservations WHERE order_id=$2)
    "#).bind(attempt).bind(order).fetch_one(&pool).await.expect("bounded literal journal totals");
    // The initial authentic paid observation has its own recognition and event.
    assert_eq!(shape, (1000, 16, 16, 16, 17, 16, 16));
    assert_eq!(counts(&pool).await, (16, 16));
    assert_eq!(footprint(&pool).await, physical);
    let mut validation = pool.begin().await.expect("bounded native validation");
    validation
        .execute("SET LOCAL statement_timeout='2s'")
        .await
        .expect("actual operation budget");
    let exact: bool = sqlx::query_scalar("SELECT refund_reservation_history_exact($1)")
        .bind(order)
        .fetch_one(&mut *validation)
        .await
        .expect("full native proof within 2 seconds");
    assert!(exact);
    validation.rollback().await.expect("read-only validation");
}
