//! Real accepted order, authenticated payment recognition, and isolated PostgreSQL.
use super::order_stock_cases::fixture::{accepted, footprint, observe};
use crate::{
    MIGRATOR,
    payment_attempts::recognition::recognize,
    refund_reservations::{RefundError, RefundLine, RefundRequest, reserve},
    staff_access::SessionToken,
};
use sqlx::{Executor, PgPool};
use std::sync::atomic::Ordering;

#[path = "refund_reservation_capacity.rs"]
mod capacity;

async fn staff(pool: &PgPool, id: &str, byte: u8) -> SessionToken {
    let token = SessionToken::from_bytes([byte; 32]);
    sqlx::query("INSERT INTO staff_accounts(id,enabled) VALUES($1,true)")
        .bind(id)
        .execute(pool)
        .await
        .expect("staff");
    sqlx::query("INSERT INTO staff_capabilities VALUES($1,'refund.prepare')")
        .bind(id)
        .execute(pool)
        .await
        .expect("refund grant");
    sqlx::query("INSERT INTO staff_sessions VALUES($1,$2,1000,4000,1400,NULL)")
        .bind(token.digest().as_slice())
        .bind(id)
        .execute(pool)
        .await
        .expect("verified session");
    token
}

async fn recognition(pool: &PgPool, order: &str) -> String {
    sqlx::query_scalar("SELECT recognition_id FROM payment_recognition_current WHERE order_id=$1")
        .bind(order)
        .fetch_one(pool)
        .await
        .expect("received recognition")
}
fn request<'a>(
    order_id: &'a str,
    proof: &'a str,
    key: &'a str,
    lines: &'a [RefundLine<'a>],
    shipping: i64,
) -> RefundRequest<'a> {
    RefundRequest {
        order_id,
        expected_recognition_id: proof,
        effect_key: key,
        lines,
        shipping_gross_minor: shipping,
        reason: "Synthetic customer adjustment",
        confirmed: true,
    }
}
async fn counts(pool: &PgPool) -> (i64, i64) {
    sqlx::query_as("SELECT (SELECT count(*) FROM refund_reservations),(SELECT count(*) FROM commerce_outbox WHERE kind='order.refund_reserved.v1')")
        .fetch_one(pool).await.expect("reservation and event counts")
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn partial_then_complete_preserves_captured_sku_and_shipping_tax_without_other_effects(
    pool: PgPool,
) {
    let f = accepted(&pool, false, true).await;
    let token = staff(&pool, "refund-finance", 94).await;
    let order = &f.order.identity.id;
    let proof = recognition(&pool, order).await;
    let physical = footprint(&pool).await;
    let source = super::order_stock_cases::fixture::unchanged(&pool).await;
    let events: Option<serde_json::Value> = sqlx::query_scalar("SELECT jsonb_agg(to_jsonb(e) ORDER BY id) FROM commerce_outbox e WHERE kind <> 'order.refund_reserved.v1'").fetch_one(&pool).await.expect("existing non-refund events");
    assert_eq!(f.order.quote.total.gross_minor, 25900);
    let partial = [
        RefundLine {
            sku: "SET-COVERS",
            gross_minor: 1000,
        },
        RefundLine {
            sku: "SET-FILLED",
            gross_minor: 2000,
        },
    ];
    let a = reserve(
        &pool,
        &token,
        f.time.as_ref(),
        f.ids.as_ref(),
        request(order, &proof, "refund-partial", &partial, 101),
    )
    .await
    .expect("authorized partial reservation");
    assert!(!a.replayed);
    assert_eq!((a.generation, a.actor_id.as_str()), (1, "refund-finance"));
    let lines = a
        .allocation
        .lines
        .iter()
        .map(|x| (x.sku.as_str(), x.gross_minor, x.net_minor, x.vat_minor))
        .collect::<Vec<_>>();
    assert_eq!(
        lines,
        vec![
            ("SET-COVERS", 1000, 826, 174),
            ("SET-FILLED", 2000, 1653, 347)
        ]
    );
    assert_eq!(
        (
            a.allocation.shipping.gross_minor,
            a.allocation.shipping.net_minor,
            a.allocation.shipping.vat_minor
        ),
        (101, 83, 18)
    );
    assert_eq!(
        (
            a.allocation.total.gross_minor,
            a.allocation.total.net_minor,
            a.allocation.total.vat_minor
        ),
        (3101, 2562, 539)
    );
    let complete = [
        RefundLine {
            sku: "SET-COVERS",
            gross_minor: 11345,
        },
        RefundLine {
            sku: "SET-FILLED",
            gross_minor: 10345,
        },
    ];
    let b = reserve(
        &pool,
        &token,
        f.time.as_ref(),
        f.ids.as_ref(),
        request(order, &proof, "refund-complete", &complete, 1109),
    )
    .await
    .expect("remaining captured balances");
    assert_eq!(b.generation, 2);
    let lines = b
        .allocation
        .lines
        .iter()
        .map(|x| (x.sku.as_str(), x.gross_minor, x.net_minor, x.vat_minor))
        .collect::<Vec<_>>();
    assert_eq!(
        lines,
        vec![
            ("SET-COVERS", 11345, 9376, 1969),
            ("SET-FILLED", 10345, 8549, 1796)
        ]
    );
    assert_eq!(
        (
            b.allocation.shipping.gross_minor,
            b.allocation.shipping.net_minor,
            b.allocation.shipping.vat_minor
        ),
        (1109, 917, 192)
    );
    assert_eq!(
        (
            b.allocation.total.gross_minor,
            b.allocation.total.net_minor,
            b.allocation.total.vat_minor
        ),
        (22799, 18842, 3957)
    );
    assert_eq!(counts(&pool).await, (2, 2));
    assert_eq!(footprint(&pool).await, physical);
    assert_eq!(
        super::order_stock_cases::fixture::unchanged(&pool).await,
        source
    );
    let after_events: Option<serde_json::Value> = sqlx::query_scalar("SELECT jsonb_agg(to_jsonb(e) ORDER BY id) FROM commerce_outbox e WHERE kind <> 'order.refund_reserved.v1'").fetch_one(&pool).await.expect("unchanged non-refund events");
    assert_eq!(after_events, events);
    let other: i64 = sqlx::query_scalar("SELECT count(*) FROM commerce_outbox WHERE kind LIKE '%refund%' AND kind <> 'order.refund_reserved.v1'").fetch_one(&pool).await.expect("no executed refund event");
    assert_eq!(other, 0);
    assert!(
        pool.execute("UPDATE refund_reservations SET reason='changed'")
            .await
            .is_err()
    );
    assert!(
        pool.execute("DELETE FROM refund_reservations")
            .await
            .is_err()
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn same_key_replays_once_and_competing_last_cent_cannot_overreserve(pool: PgPool) {
    let f = accepted(&pool, false, true).await;
    let token = staff(&pool, "refund-finance", 94).await;
    let order = &f.order.identity.id;
    let proof = recognition(&pool, order).await;
    let partial = [
        RefundLine {
            sku: "SET-COVERS",
            gross_minor: 12344,
        },
        RefundLine {
            sku: "SET-FILLED",
            gross_minor: 0,
        },
    ];
    let (a, b) = tokio::join!(
        reserve(
            &pool,
            &token,
            f.time.as_ref(),
            f.ids.as_ref(),
            request(order, &proof, "refund-same", &partial, 0)
        ),
        reserve(
            &pool,
            &token,
            f.time.as_ref(),
            f.ids.as_ref(),
            request(order, &proof, "refund-same", &partial, 0)
        )
    );
    let a = a.expect("first same-key request");
    let b = b.expect("second same-key request");
    assert_eq!(a.id, b.id);
    assert_ne!(a.replayed, b.replayed);
    assert_eq!(counts(&pool).await, (1, 1));
    let last = [
        RefundLine {
            sku: "SET-COVERS",
            gross_minor: 1,
        },
        RefundLine {
            sku: "SET-FILLED",
            gross_minor: 0,
        },
    ];
    let (a, b) = tokio::join!(
        reserve(
            &pool,
            &token,
            f.time.as_ref(),
            f.ids.as_ref(),
            request(order, &proof, "refund-last-a", &last, 0)
        ),
        reserve(
            &pool,
            &token,
            f.time.as_ref(),
            f.ids.as_ref(),
            request(order, &proof, "refund-last-b", &last, 0)
        )
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    let failure = if let Err(error) = a {
        error
    } else {
        b.expect_err("one loser")
    };
    assert!(matches!(failure, RefundError::InvalidRequest));
    assert_eq!(counts(&pool).await, (2, 2));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn authority_replay_binding_and_current_payment_proof_are_enforced(pool: PgPool) {
    let f = accepted(&pool, false, true).await;
    let token = staff(&pool, "refund-finance", 94).await;
    let other = staff(&pool, "other-finance", 95).await;
    let order = &f.order.identity.id;
    let proof = recognition(&pool, order).await;
    let line = [
        RefundLine {
            sku: "SET-COVERS",
            gross_minor: 1,
        },
        RefundLine {
            sku: "SET-FILLED",
            gross_minor: 0,
        },
    ];
    reserve(
        &pool,
        &token,
        f.time.as_ref(),
        f.ids.as_ref(),
        request(order, &proof, "refund-key", &line, 0),
    )
    .await
    .expect("positive authorized control");
    let changed = [
        RefundLine {
            sku: "SET-COVERS",
            gross_minor: 2,
        },
        RefundLine {
            sku: "SET-FILLED",
            gross_minor: 0,
        },
    ];
    assert!(matches!(
        reserve(
            &pool,
            &token,
            f.time.as_ref(),
            f.ids.as_ref(),
            request(order, &proof, "refund-key", &changed, 0)
        )
        .await,
        Err(RefundError::Conflict)
    ));
    assert!(matches!(
        reserve(
            &pool,
            &other,
            f.time.as_ref(),
            f.ids.as_ref(),
            request(order, &proof, "refund-key", &line, 0)
        )
        .await,
        Err(RefundError::Conflict)
    ));
    f.time.0.store(2301, Ordering::SeqCst);
    assert!(matches!(
        reserve(
            &pool,
            &token,
            f.time.as_ref(),
            f.ids.as_ref(),
            request(order, &proof, "expired-mfa", &line, 0)
        )
        .await,
        Err(RefundError::Denied)
    ));
    f.time.0.store(1500, Ordering::SeqCst);
    observe(&pool, &f, "tr_AMBIGUOUS", "open", "later-ambiguity").await;
    recognize(
        &pool,
        f.time.as_ref(),
        f.ids.as_ref(),
        &f.frozen.snapshot().binding.id,
    )
    .await
    .expect("later needs-review recognition");
    assert!(matches!(
        reserve(
            &pool,
            &token,
            f.time.as_ref(),
            f.ids.as_ref(),
            request(order, &proof, "stale-proof", &line, 0)
        )
        .await,
        Err(RefundError::Conflict) | Err(RefundError::Unavailable)
    ));
    let historical = reserve(
        &pool,
        &token,
        f.time.as_ref(),
        f.ids.as_ref(),
        request(order, &proof, "refund-key", &line, 0),
    )
    .await
    .expect("later ambiguity does not erase the exact reservation");
    assert!(historical.replayed);
    assert_eq!(historical.recognition_id, proof);
    assert_eq!(counts(&pool).await, (1, 1));
}

fn check_violation(error: sqlx::Error) {
    assert_eq!(
        error.as_database_error().and_then(|e| e.code()).as_deref(),
        Some("23514")
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn native_vat_deltas_straddle_the_exact_boundary_at_the_capture_ceiling(pool: PgPool) {
    let (before, after, zero_shipping): (i64, i64, i64) = sqlx::query_as(
        "SELECT refund_reservation_vat_delta(9007199254740991,1563232928508767,1125899906842638,1)::bigint,
                refund_reservation_vat_delta(9007199254740991,1563232928508767,1125899906842639,1)::bigint,
                refund_reservation_vat_delta(0,0,0,0)::bigint",
    )
    .fetch_one(&pool)
    .await
    .expect("the same exact native VAT calculation used by the history validator");
    assert_eq!((before, after, zero_shipping), (0, 1, 0));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn native_receipts_require_bounded_exact_allocation_and_reciprocal_same_transaction_event(
    pool: PgPool,
) {
    let f = accepted(&pool, false, true).await;
    let token = staff(&pool, "refund-finance", 94).await;
    let order = &f.order.identity.id;
    let proof = recognition(&pool, order).await;
    let lines = [
        RefundLine {
            sku: "SET-COVERS",
            gross_minor: 1000,
        },
        RefundLine {
            sku: "SET-FILLED",
            gross_minor: 0,
        },
    ];
    reserve(
        &pool,
        &token,
        f.time.as_ref(),
        f.ids.as_ref(),
        request(order, &proof, "native-healthy", &lines, 0),
    )
    .await
    .expect("healthy receipt and event");
    let exact: bool = sqlx::query_scalar("SELECT refund_reservation_history_exact($1)")
        .bind(order)
        .fetch_one(&pool)
        .await
        .expect("native history");
    assert!(exact, "positive native history control");
    let oversized = sqlx::query("INSERT INTO refund_reservations(id,order_id,attempt_id,recognition_id,actor_id,effect_key,generation,reason,reserved_at,allocation,event_id,transaction_id) SELECT 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',order_id,attempt_id,recognition_id,actor_id,'native-oversize',2,reason,reserved_at,allocation||jsonb_build_object('padding',repeat('x',20000)),'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb',pg_current_xact_id() FROM refund_reservations WHERE effect_key='native-healthy'").execute(&pool).await.expect_err("oversized native allocation rejected");
    check_violation(oversized);
    let detached = sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) SELECT 'cccccccccccccccccccccccccccccccc',aggregate_id,kind,payload,created_at FROM commerce_outbox WHERE kind='order.refund_reserved.v1'").execute(&pool).await.expect_err("detached historical event rejected");
    check_violation(detached);
    let orphan = sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) VALUES('dddddddddddddddddddddddddddddddd',$1,'order.refund_reserved.v1',jsonb_build_object('schema_version',1,'order_id',$1,'reservation_id','eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee','reservation_sha256',repeat('0',64)),1500)").bind(order).execute(&pool).await.expect_err("orphan event rejected");
    check_violation(orphan);
    let mut tx = pool.begin().await.expect("isolated raw transaction");
    sqlx::query("INSERT INTO refund_reservations(id,order_id,attempt_id,recognition_id,actor_id,effect_key,generation,reason,reserved_at,allocation,event_id,transaction_id) SELECT 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',order_id,attempt_id,recognition_id,actor_id,'native-bad-tax',2,reason,reserved_at,jsonb_set(jsonb_set(jsonb_set(jsonb_set(allocation,'{lines,0,net_minor}','825'),'{lines,0,vat_minor}','175'),'{total,net_minor}','825'),'{total,vat_minor}','175'),'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb',pg_current_xact_id() FROM refund_reservations WHERE effect_key='native-healthy'").execute(&mut *tx).await.expect("well-shaped but incorrect raw allocation staged");
    sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) SELECT event_id,order_id,'order.refund_reserved.v1',jsonb_build_object('schema_version',1,'order_id',order_id,'reservation_id',id,'reservation_sha256',refund_reservation_digest(r)),reserved_at FROM refund_reservations r WHERE effect_key='native-bad-tax'").execute(&mut *tx).await.expect("matching event staged");
    check_violation(
        tx.commit()
            .await
            .expect_err("native commit recomputes cumulative VAT"),
    );
    let mut tx = pool.begin().await.expect("orphan receipt transaction");
    sqlx::query("INSERT INTO refund_reservations(id,order_id,attempt_id,recognition_id,actor_id,effect_key,generation,reason,reserved_at,allocation,event_id,transaction_id) SELECT 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',order_id,attempt_id,recognition_id,actor_id,'native-orphan',2,reason,reserved_at,allocation,'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb',pg_current_xact_id() FROM refund_reservations WHERE effect_key='native-healthy'").execute(&mut *tx).await.expect("orphan receipt staged");
    check_violation(
        tx.commit()
            .await
            .expect_err("receipt requires event in same transaction"),
    );
    assert_eq!(counts(&pool).await, (1, 1));
    let exact: bool = sqlx::query_scalar("SELECT refund_reservation_history_exact($1)")
        .bind(order)
        .fetch_one(&pool)
        .await
        .expect("surviving exact history");
    assert!(exact);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn sixteen_lifetime_reservations_are_a_hard_limit(pool: PgPool) {
    let f = accepted(&pool, false, true).await;
    let token = staff(&pool, "refund-finance", 94).await;
    let order = &f.order.identity.id;
    let proof = recognition(&pool, order).await;
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
    for generation in 1..=16 {
        let key = format!("lifetime-{generation}");
        let receipt = reserve(
            &pool,
            &token,
            f.time.as_ref(),
            f.ids.as_ref(),
            request(order, &proof, &key, &lines, 0),
        )
        .await
        .expect("distinct lifetime reservation");
        assert_eq!(receipt.generation, generation);
    }
    assert!(matches!(
        reserve(
            &pool,
            &token,
            f.time.as_ref(),
            f.ids.as_ref(),
            request(order, &proof, "lifetime-seventeen", &lines, 0)
        )
        .await,
        Err(RefundError::Conflict)
    ));
    assert_eq!(counts(&pool).await, (16, 16));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn early_constraint_flush_seals_payment_proof_until_transaction_end(pool: PgPool) {
    let f = accepted(&pool, false, true).await;
    let token = staff(&pool, "refund-finance", 94).await;
    let order = &f.order.identity.id;
    let proof = recognition(&pool, order).await;
    let lines = [
        RefundLine {
            sku: "SET-COVERS",
            gross_minor: 1000,
        },
        RefundLine {
            sku: "SET-FILLED",
            gross_minor: 0,
        },
    ];
    reserve(
        &pool,
        &token,
        f.time.as_ref(),
        f.ids.as_ref(),
        request(order, &proof, "seal-healthy", &lines, 0),
    )
    .await
    .expect("healthy native template");
    let mut tx = pool.begin().await.expect("raw reservation transaction");
    sqlx::query("INSERT INTO refund_reservations(id,order_id,attempt_id,recognition_id,actor_id,effect_key,generation,reason,reserved_at,allocation,event_id,transaction_id) SELECT 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',order_id,attempt_id,recognition_id,actor_id,'seal-second',2,reason,reserved_at,jsonb_set(jsonb_set(jsonb_set(jsonb_set(allocation,'{lines,0,net_minor}','827'),'{lines,0,vat_minor}','173'),'{total,net_minor}','827'),'{total,vat_minor}','173'),'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb',pg_current_xact_id() FROM refund_reservations WHERE effect_key='seal-healthy'").execute(&mut *tx).await.expect("valid next raw receipt");
    sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) SELECT event_id,order_id,'order.refund_reserved.v1',jsonb_build_object('schema_version',1,'order_id',order_id,'reservation_id',id,'reservation_sha256',refund_reservation_digest(r)),reserved_at FROM refund_reservations r WHERE effect_key='seal-second'").execute(&mut *tx).await.expect("reciprocal event");
    tx.execute("SET CONSTRAINTS ALL IMMEDIATE")
        .await
        .expect("valid receipt and event flush");
    let denied = sqlx::query(
        "UPDATE payment_recognition_current SET recognition_id=recognition_id WHERE order_id=$1",
    )
    .bind(order)
    .execute(&mut *tx)
    .await
    .expect_err("early flush seals payment proof");
    let database = denied.as_database_error().expect("native seal error");
    assert_eq!(database.code().as_deref(), Some("23514"));
    assert!(
        database
            .message()
            .contains("Completed refund reservation source cannot change in this transaction")
    );
    tx.rollback().await.expect("rollback rejected transaction");
    let result = sqlx::query(
        "UPDATE payment_recognition_current SET recognition_id=recognition_id WHERE order_id=$1",
    )
    .bind(order)
    .execute(&pool)
    .await
    .expect("fresh transaction has no leaked seal");
    assert_eq!(result.rows_affected(), 1);
    assert_eq!(counts(&pool).await, (1, 1));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn mfa_expiring_during_real_final_fence_wait_rolls_back_reservation(pool: PgPool) {
    let f = accepted(&pool, false, true).await;
    let token = staff(&pool, "refund-finance", 94).await;
    let proof = recognition(&pool, &f.order.identity.id).await;
    let physical = footprint(&pool).await;
    let mut blocker = pool.begin().await.expect("own final fence");
    blocker
        .execute("SELECT pg_advisory_xact_lock(1936876902,1)")
        .await
        .expect("first reservation fence");
    let p = pool.clone();
    let time = f.time.clone();
    let ids = f.ids.clone();
    let order = f.order.identity.id.clone();
    let token_task = SessionToken::from_bytes([94; 32]);
    let task = tokio::spawn(async move {
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
        reserve(
            &p,
            &token_task,
            time.as_ref(),
            ids.as_ref(),
            request(&order, &proof, "wait-mfa", &lines, 0),
        )
        .await
    });
    tokio::time::timeout(std::time::Duration::from_millis(800), async {
        loop {
            let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_locks WHERE locktype='advisory' AND database=(SELECT oid FROM pg_database WHERE datname=current_database()) AND classid=1936876902::oid AND objid=1::oid AND objsubid=2 AND NOT granted)").fetch_one(&pool).await.expect("actual exact fence wait");
            if waiting { break; }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }).await.expect("reservation reached final native fence");
    assert_eq!(counts(&pool).await, (0, 0));
    f.time.0.store(2301, Ordering::SeqCst);
    blocker.commit().await.expect("release final validation");
    assert!(matches!(
        task.await.expect("reservation task"),
        Err(RefundError::Denied)
    ));
    assert_eq!(counts(&pool).await, (0, 0));
    assert_eq!(footprint(&pool).await, physical);
    f.time.0.store(1500, Ordering::SeqCst);
    let proof = recognition(&pool, &f.order.identity.id).await;
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
    reserve(
        &pool,
        &token,
        f.time.as_ref(),
        f.ids.as_ref(),
        request(&f.order.identity.id, &proof, "wait-mfa", &lines, 0),
    )
    .await
    .expect("fresh authority permits exact retry");
    assert_eq!(counts(&pool).await, (1, 1));
}
