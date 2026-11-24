//! Check the late-stock inbox process against genuine synthetic review custody.
#![cfg(unix)]
#[allow(dead_code)]
#[path = "../../infrastructure/tests/support/payment_fixture.rs"]
mod payment_fixture;
#[path = "support/recovery_file.rs"]
mod recovery_file;
#[allow(dead_code)]
#[path = "support/staff_review.rs"]
mod staff_review;

use commerce_application::request_context::Clock;
use commerce_domain::inventory::WarehouseChange;
use commerce_infrastructure::{
    MIGRATOR,
    guest_baskets::GuestCredential,
    holds::{HoldCommand, transition_hold},
    late_stock::{Outcome, recover},
    payment_attempts::{prepare_attempt, start_dispatch},
    runtime::SystemClock,
    staff_access::SessionToken,
    stock::{WarehouseRequest, warehouse_movement},
};
use recovery_file::IntentFile;
use serde_json::{Value, json};
use sqlx::{Executor, PgPool};
use staff_review::{denied, recognize_paid_attempt, report};
use std::sync::atomic::{AtomicU64, Ordering};

const INBOX: &str = "--review-late-stock-inbox";
const REVIEWER: [u8; 32] = [51; 32];
const RECOVERER: [u8; 32] = [52; 32];
const STALE: [u8; 32] = [53; 32];

async fn seed_staff(pool: &PgPool) {
    pool.execute("INSERT INTO staff_accounts VALUES ('late-review',true),('late-recover',true),('late-stale',true); INSERT INTO staff_capabilities VALUES ('late-review','stock.review'),('late-recover','stock.recover'),('late-stale','stock.review')")
        .await.expect("separate synthetic staff grants");
    let now = i64::try_from(SystemClock.unix_seconds().expect("clock")).expect("SQL time");
    for (actor, bearer, mfa) in [
        ("late-review", REVIEWER, now - 1),
        ("late-recover", RECOVERER, now - 1),
        ("late-stale", STALE, now - 901),
    ] {
        sqlx::query("INSERT INTO staff_sessions(token_hash,staff_id,created_at,expires_at,mfa_verified_at) VALUES($1,$2,$3,$4,$5)")
            .bind(SessionToken::from_bytes(bearer).digest().as_slice())
            .bind(actor).bind(now - 950).bind(now + 300).bind(mfa)
            .execute(pool).await.expect("isolated staff session");
    }
}

async fn effects(pool: &PgPool) -> Value {
    sqlx::query_scalar("SELECT jsonb_build_object('stock',(SELECT jsonb_agg(to_jsonb(s) ORDER BY component_code) FROM stock_balances s),'moves',(SELECT count(*) FROM stock_movements),'holds',(SELECT jsonb_agg(to_jsonb(h) ORDER BY id) FROM stock_holds h),'cases',(SELECT count(*) FROM late_stock_review_cases),'decisions',(SELECT count(*) FROM late_stock_dispositions),'allocations',(SELECT count(*) FROM order_stock_allocations),'events',(SELECT count(*) FROM commerce_outbox))")
        .fetch_one(pool).await.expect("physical stock and custody effects")
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn late_stock_cli_separates_review_from_confirmed_recovery(pool: PgPool) {
    seed_staff(&pool).await;
    let empty = json!({"kind":"late_stock_review_inbox","case":null,"nextCursor":null});
    assert_eq!(report(&pool, &[INBOX], &REVIEWER), empty);
    denied(&pool, &[INBOX], &STALE, "late stock review unavailable");

    let (store, clock, session, csrf, order) = Box::pin(payment_fixture::accepted(&pool)).await;
    let owner = GuestCredential::from_session(&session).expect("fixture owner");
    let ids = payment_fixture::Ids(AtomicU64::new(87_000));
    let config = payment_fixture::config();
    let prepared = Box::pin(prepare_attempt(
        &store,
        &owner,
        &csrf,
        "late-worker-pay",
        &payment_fixture::intent(&order),
        &config,
    ))
    .await
    .expect("prepared fixture attempt");
    let attempt = prepared.frozen.snapshot();
    Box::pin(start_dispatch(
        &pool,
        clock.as_ref(),
        &ids,
        &attempt.binding.id,
        &config,
    ))
    .await
    .expect("durable fixture dispatch");
    recognize_paid_attempt(&pool, clock.as_ref(), &ids, attempt).await;

    let original_holds: Vec<String> =
        sqlx::query_scalar("SELECT id FROM stock_holds WHERE basket_group_id=$1 ORDER BY id")
            .bind(&order.identity.stock_group_id)
            .fetch_all(&pool)
            .await
            .expect("original hold members");
    assert!(!original_holds.is_empty());
    for hold in original_holds {
        transition_hold(
            &pool,
            &hold,
            HoldCommand::Expire {
                actor_id: "fixture-expiry",
            },
            2000,
        )
        .await
        .expect("genuine original expiry");
    }
    warehouse_movement(
        &pool,
        WarehouseRequest {
            effect_key: "fixture-foam-count",
            component_code: "SEAT-FOAM",
            change: WarehouseChange::Count(2),
            actor: "fixture-warehouse",
            reason: "synthetic physical count",
        },
    )
    .await
    .expect("genuine scarce stock balance");
    clock.0.store(2000, Ordering::SeqCst);
    let order_id = &order.identity.id;
    let receipt = Box::pin(recover(&pool, clock.as_ref(), &ids, order_id))
        .await
        .expect("actual late stock recovery stop");
    let Outcome::NeedsReview(case) = receipt.outcome else {
        panic!("scarce stock must stop")
    };
    let digest: String = sqlx::query_scalar("SELECT payload->>'case_sha256' FROM commerce_outbox WHERE aggregate_id=$1 AND kind='order.stock_recovery_review.v1'")
        .bind(order_id).fetch_one(&pool).await.expect("recorded case digest");
    assert_eq!(digest.len(), 64);

    let before = effects(&pool).await;
    assert_eq!(
        report(&pool, &[INBOX], &REVIEWER),
        json!({"kind":"late_stock_review_inbox","case":{
            "caseId":case.id,"caseDigest":digest,"orderId":order_id,
            "reason":"component_scarcity","status":"open","nextGeneration":1,"assessedAt":2000
        },"nextCursor":case.id})
    );
    assert_eq!(report(&pool, &[INBOX, &case.id], &REVIEWER), empty);
    denied(&pool, &[INBOX], &RECOVERER, "late stock review unavailable");
    assert_eq!(effects(&pool).await, before);

    let intent = IntentFile::new();
    let args = ["--retry-late-stock", &case.id, intent.path(), "--confirm"];
    let payload = |case_digest: &str, generation, effect| {
        json!({"caseDigest":case_digest,"expectedGeneration":generation,
            "effectKey":effect,"reason":"Synthetic report of a checked physical count"})
    };
    let first_effect = format!("{:032x}", 1);
    let next_effect = format!("{:032x}", 2);
    intent.write(&payload(&digest, 1, &first_effect));
    denied(&pool, &args, &REVIEWER, "late stock retry unavailable");
    intent.write(&payload(&"f".repeat(64), 1, &first_effect));
    denied(&pool, &args, &RECOVERER, "late stock retry unavailable");
    intent.write(&payload(&digest, 2, &next_effect));
    denied(&pool, &args, &RECOVERER, "late stock retry unavailable");
    assert_eq!(effects(&pool).await, before);

    intent.write(&payload(&digest, 1, &first_effect));
    let first = report(&pool, &args, &RECOVERER);
    let (id, at): (String, i64) = sqlx::query_as(
        "SELECT id,recorded_at FROM late_stock_dispositions WHERE case_id=$1 AND generation=1",
    )
    .bind(&case.id)
    .fetch_one(&pool)
    .await
    .expect("one first decision");
    assert_eq!(
        first,
        json!({"kind":"late_stock_retry","decisionId":id,"caseId":case.id,
        "generation":1,"recordedAt":at,"outcome":{"state":"still_scarce"},"replayed":false})
    );
    let after_first = effects(&pool).await;
    assert_eq!(after_first["allocations"], 0);
    assert_eq!(after_first["stock"], before["stock"]);
    assert_eq!(after_first["moves"], before["moves"]);

    warehouse_movement(
        &pool,
        WarehouseRequest {
            effect_key: "fixture-foam-restocked",
            component_code: "SEAT-FOAM",
            change: WarehouseChange::Count(6),
            actor: "fixture-warehouse",
            reason: "synthetic physical replenishment",
        },
    )
    .await
    .expect("genuine restock before a fresh decision");
    intent.write(&payload(&digest, 2, &next_effect));
    let second = report(&pool, &args, &RECOVERER);
    let (id, at, allocation): (String, i64, String) = sqlx::query_as("SELECT id,recorded_at,allocation_id FROM late_stock_dispositions WHERE case_id=$1 AND generation=2")
        .bind(&case.id).fetch_one(&pool).await.expect("one allocation decision");
    assert_eq!(
        second,
        json!({"kind":"late_stock_retry","decisionId":id,"caseId":case.id,
        "generation":2,"recordedAt":at,"outcome":{"state":"allocated","allocationId":allocation},"replayed":false})
    );
    let after_second = effects(&pool).await;
    assert_eq!(after_second["allocations"], 1);
    assert_eq!(after_second["cases"], 1);
    assert_eq!(after_second["decisions"], 2);

    intent.write(&payload(&digest, 1, &first_effect));
    let mut expected_replay = first;
    expected_replay["replayed"] = true.into();
    assert_eq!(report(&pool, &args, &RECOVERER), expected_replay);
    assert_eq!(effects(&pool).await, after_second);

    pool.execute("ALTER TABLE commerce_outbox DISABLE TRIGGER USER; DELETE FROM commerce_outbox WHERE kind='order.stock_recovery_review.v1'; ALTER TABLE commerce_outbox ENABLE TRIGGER USER")
        .await.expect("isolated damaged reciprocal custody");
    let before = effects(&pool).await;
    denied(&pool, &[INBOX], &REVIEWER, "late stock review unavailable");
    assert_eq!(effects(&pool).await, before);
}
