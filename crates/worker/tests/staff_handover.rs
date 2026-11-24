//! Exercise the local worker against real PostgreSQL and synthetic order custody.
#![cfg(unix)]
#[allow(dead_code)]
#[path = "../../infrastructure/tests/support/payment_fixture.rs"]
mod payment_fixture;
#[path = "support/staff_review.rs"]
mod staff_review;

use commerce_application::request_context::Clock;
use commerce_infrastructure::{
    MIGRATOR,
    guest_baskets::GuestCredential,
    handover_evidence, packing_custody,
    payment_attempts::{prepare_attempt, start_dispatch},
    runtime::SystemClock,
    staff_access::SessionToken,
};
use serde_json::json;
use sqlx::{Executor, PgPool};
use staff_review::{denied, report, settle_paid_attempt};
use std::sync::atomic::AtomicU64;

async fn settled_order(pool: &PgPool, ids: &payment_fixture::Ids) -> String {
    let (store, clock, session, csrf, order) = payment_fixture::accepted(pool).await;
    let config = payment_fixture::config();
    let owner = GuestCredential::from_session(&session).expect("fixture owner");
    let prepared = prepare_attempt(
        &store,
        &owner,
        &csrf,
        "handover-worker-attempt",
        &payment_fixture::intent(&order),
        &config,
    )
    .await
    .expect("original attempt");
    let attempt = prepared.frozen.snapshot();
    let attempt_id = &attempt.binding.id;
    start_dispatch(pool, clock.as_ref(), ids, attempt_id, &config)
        .await
        .expect("original dispatch start");
    settle_paid_attempt(pool, clock.as_ref(), ids, attempt).await;
    order.identity.id
}

const PACKER: [u8; 32] = [81; 32];
const REVIEWER: [u8; 32] = [82; 32];
const STALE: [u8; 32] = [83; 32];

async fn seed_staff(pool: &PgPool) {
    pool.execute("INSERT INTO staff_accounts VALUES ('synthetic-packer',true),('synthetic-handover-reader',true),('synthetic-stale-handover',true); INSERT INTO staff_capabilities VALUES ('synthetic-packer','warehouse.manage'),('synthetic-handover-reader','handover.record'),('synthetic-stale-handover','handover.record')")
        .await.expect("isolated staff grants");
    let now = i64::try_from(SystemClock.unix_seconds().expect("system time")).expect("SQL time");
    for (actor, bearer, mfa) in [
        ("synthetic-packer", PACKER, now - 1),
        ("synthetic-handover-reader", REVIEWER, now - 1),
        ("synthetic-stale-handover", STALE, now - 901),
    ] {
        sqlx::query("INSERT INTO staff_sessions(token_hash,staff_id,created_at,expires_at,mfa_verified_at) VALUES ($1,$2,$3,$4,$5)")
            .bind(SessionToken::from_bytes(bearer).digest().as_slice())
            .bind(actor).bind(now - 950).bind(now + 300).bind(mfa)
            .execute(pool).await.expect("isolated staff session");
    }
}

async fn footprint(pool: &PgPool) -> (i64, i64, i64, i64) {
    sqlx::query_as("SELECT (SELECT count(*) FROM handover_evidence_submissions),(SELECT count(*) FROM packing_custody),(SELECT count(*) FROM commerce_outbox),(SELECT count(*) FROM stock_movements)")
        .fetch_one(pool).await.expect("handover and related effect counts")
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn handover_cli_projects_only_authorized_historical_evidence(pool: PgPool) {
    let ids = payment_fixture::Ids(AtomicU64::new(84_000));
    let order = settled_order(&pool, &ids).await;
    let args = ["--inspect-handover-evidence", order.as_str()];
    seed_staff(&pool).await;
    assert_eq!(
        report(&pool, &args, &REVIEWER),
        json!({"kind": "handover_evidence", "submissions": []}),
    );

    let packer = SessionToken::from_bytes(PACKER);
    let packing = packing_custody::record(
        &pool,
        &packer,
        &SystemClock,
        &ids,
        packing_custody::Request {
            order_id: &order,
            effect_key: "handover-worker-pack",
            confirmed: true,
            action: packing_custody::Action::Packed {
                units: &[packing_custody::Units {
                    sku: "SET-FILLED".into(),
                    units: 1,
                }],
                packages: &[vec![
                    packing_custody::Component {
                        code: "SEAT-COVER".into(),
                        quantity: 2,
                    },
                    packing_custody::Component {
                        code: "SEAT-FOAM".into(),
                        quantity: 2,
                    },
                ]],
            },
        },
    )
    .await
    .expect("committed packing parent")
    .record;
    let evidence = handover_evidence::submit(
        &pool,
        &SessionToken::from_bytes(REVIEWER),
        &SystemClock,
        &ids,
        handover_evidence::Request {
            order_id: &order,
            packing_id: &packing.id,
            effect_key: "handover-worker-submission",
            bytes: b"synthetic evidence which must never be printed",
            confirmed: true,
        },
    )
    .await
    .expect("committed evidence submission")
    .submission;
    let expected = |released| {
        json!({"kind": "handover_evidence", "submissions": [{
            "id": evidence.id,
            "packingId": packing.id,
            "generation": 1,
            "recordedAt": evidence.recorded_at,
            "packingReleased": released,
        }]})
    };
    let before = footprint(&pool).await;
    assert_eq!(report(&pool, &args, &REVIEWER), expected(false));
    for bearer in [PACKER, STALE] {
        denied(&pool, &args, &bearer, "handover evidence unavailable");
    }
    assert_eq!(footprint(&pool).await, before);
    packing_custody::record(
        &pool,
        &packer,
        &SystemClock,
        &ids,
        packing_custody::Request {
            order_id: &order,
            effect_key: "handover-worker-release",
            confirmed: true,
            action: packing_custody::Action::Released {
                target_id: &packing.id,
            },
        },
    )
    .await
    .expect("release remains independent");
    let before = footprint(&pool).await;
    assert_eq!(report(&pool, &args, &REVIEWER), expected(true));
    assert_eq!(footprint(&pool).await, before);
}
