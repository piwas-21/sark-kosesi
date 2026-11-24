//! Run cancellation review commands against an isolated synthetic order.
#![cfg(unix)]
#[allow(dead_code)]
#[path = "../../infrastructure/tests/support/payment_fixture.rs"]
mod payment_fixture;
#[path = "support/staff_review.rs"]
mod staff_review;

use commerce_application::request_context::Clock;
use commerce_infrastructure::{
    MIGRATOR,
    checkout_cancellations::request_after_dispatch,
    guest_baskets::GuestCredential,
    packing_custody::{self, Action, Component, Request, Units},
    payment_attempts::{prepare_attempt, start_dispatch},
    runtime::SystemClock,
    staff_access::SessionToken,
};
use serde_json::json;
use sqlx::{Executor, PgPool};
use staff_review::{denied, report, settle_paid_attempt};
use std::sync::atomic::AtomicU64;

async fn staff(pool: &PgPool, id: &str, token: &[u8; 32], grants: &[&str], recent: bool) {
    sqlx::query("INSERT INTO staff_accounts(id,enabled) VALUES ($1,true)")
        .bind(id)
        .execute(pool)
        .await
        .expect("synthetic staff account");
    for grant in grants {
        sqlx::query("INSERT INTO staff_capabilities(staff_id,capability) VALUES ($1,$2)")
            .bind(id)
            .bind(grant)
            .execute(pool)
            .await
            .expect("synthetic capability");
    }
    let now = i64::try_from(SystemClock.unix_seconds().expect("clock")).expect("SQL time");
    sqlx::query("INSERT INTO staff_sessions(token_hash,staff_id,created_at,expires_at,mfa_verified_at) VALUES ($1,$2,$3,$4,$5)")
        .bind(SessionToken::from_bytes(*token).digest().as_slice())
        .bind(id).bind(now - 1).bind(now + 300).bind(recent.then_some(now - 1))
        .execute(pool).await.expect("synthetic session");
}

async fn footprint(pool: &PgPool) -> (i64, i64, i64, i64, i64, i64) {
    sqlx::query_as("SELECT (SELECT count(*) FROM checkout_post_dispatch_requests),(SELECT count(*) FROM packing_custody),(SELECT count(*) FROM commerce_outbox),(SELECT count(*) FROM stock_movements),(SELECT count(*) FROM payment_observations),(SELECT count(*) FROM handover_evidence_submissions)")
        .fetch_one(pool).await.expect("private effect counts")
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn cancellation_cli_returns_only_authorized_recorded_evidence(pool: PgPool) {
    let (store, clock, session, csrf, order) = Box::pin(payment_fixture::accepted(&pool)).await;
    let owner = GuestCredential::from_session(&session).expect("fixture owner");
    let order_id = order.identity.id.as_str();
    let exact = ["--review-cancellation", order_id];
    let inbox = ["--review-cancellation-inbox"];
    let combined = ["--inspect-cancellation-packing", order_id];
    let review = [41; 32];
    let both = [42; 32];
    let no_mfa = [43; 32];
    staff(
        &pool,
        "cancel-review",
        &review,
        &["cancellation.review"],
        true,
    )
    .await;
    staff(
        &pool,
        "cancel-and-pack",
        &both,
        &["cancellation.review", "warehouse.manage"],
        true,
    )
    .await;
    staff(
        &pool,
        "cancel-no-mfa",
        &no_mfa,
        &["cancellation.review"],
        false,
    )
    .await;

    assert_eq!(
        report(&pool, &exact, &review),
        json!({"kind":"post_dispatch_cancellation_request","request":null})
    );
    assert_eq!(
        report(&pool, &inbox, &review),
        json!({"kind":"post_dispatch_cancellation_inbox","request":null})
    );
    assert_eq!(
        report(&pool, &combined, &both),
        json!({"kind":"post_dispatch_cancellation_packing","evidence":null})
    );
    denied(&pool, &exact, &no_mfa, "cancellation review unavailable");

    let config = payment_fixture::config();
    let ids = payment_fixture::Ids(AtomicU64::new(80_000));
    let prepared = Box::pin(prepare_attempt(
        &store,
        &owner,
        &csrf,
        "worker-pay",
        &payment_fixture::intent(&order),
        &config,
    ))
    .await
    .expect("prepared fixture attempt");
    let attempt = prepared.frozen.snapshot();
    let attempt_id = &attempt.binding.id;
    Box::pin(start_dispatch(
        &pool,
        clock.as_ref(),
        &ids,
        attempt_id,
        &config,
    ))
    .await
    .expect("durable fixture dispatch");
    Box::pin(request_after_dispatch(
        &store,
        &owner,
        &csrf,
        "private-request-effect",
        order_id,
    ))
    .await
    .expect("recorded fixture request");

    let before = footprint(&pool).await;
    let request = json!({"orderId":order_id,"requestedAt":1500});
    assert_eq!(
        report(&pool, &exact, &review),
        json!({"kind":"post_dispatch_cancellation_request","request":request})
    );
    assert_eq!(
        report(&pool, &inbox, &review),
        json!({"kind":"post_dispatch_cancellation_inbox","request":request})
    );
    assert_eq!(
        report(&pool, &[inbox[0], order_id], &review),
        json!({"kind":"post_dispatch_cancellation_inbox","request":null})
    );
    assert_eq!(
        report(&pool, &combined, &both),
        json!({"kind":"post_dispatch_cancellation_packing","evidence":{"request":request,"packing":{"status":"unavailable"}}})
    );
    denied(
        &pool,
        &combined,
        &review,
        "cancellation packing inspection unavailable",
    );
    assert_eq!(footprint(&pool).await, before);

    Box::pin(settle_paid_attempt(&pool, clock.as_ref(), &ids, attempt)).await;
    let before = footprint(&pool).await;
    assert_eq!(
        report(&pool, &combined, &both),
        json!({"kind":"post_dispatch_cancellation_packing","evidence":{"request":request,"packing":{"status":"validated","lines":[{"sku":"SET-FILLED","ordered":3,"activeAsserted":0,"notAsserted":3}],"history":[]}}})
    );
    assert_eq!(footprint(&pool).await, before);

    let packed = Box::pin(packing_custody::record(
        &pool,
        &SessionToken::from_bytes(both),
        &SystemClock,
        &ids,
        Request {
            order_id,
            effect_key: "worker-cancel-pack",
            confirmed: true,
            action: Action::Packed {
                units: &[Units {
                    sku: "SET-FILLED".into(),
                    units: 1,
                }],
                packages: &[vec![
                    Component {
                        code: "SEAT-COVER".into(),
                        quantity: 2,
                    },
                    Component {
                        code: "SEAT-FOAM".into(),
                        quantity: 2,
                    },
                ]],
            },
        },
    ))
    .await
    .expect("recorded synthetic packing assertion");
    let before = footprint(&pool).await;
    assert_eq!(
        report(&pool, &combined, &both),
        json!({"kind":"post_dispatch_cancellation_packing","evidence":{"request":request,"packing":{"status":"validated","lines":[{"sku":"SET-FILLED","ordered":3,"activeAsserted":1,"notAsserted":2}],"history":[{"id":packed.record.id,"action":"packed","targetId":null,"recordedAt":packed.record.recorded_at}]}}})
    );
    assert_eq!(footprint(&pool).await, before);

    // The fixture deliberately detaches the reciprocal event to exercise the
    // adapter even though ordinary writes cannot create this broken state.
    pool.execute("ALTER TABLE commerce_outbox DISABLE TRIGGER USER; DELETE FROM commerce_outbox WHERE kind='checkout.cancellation_requested.v1'; ALTER TABLE commerce_outbox ENABLE TRIGGER USER")
        .await.expect("isolated damaged source fixture");
    let before = footprint(&pool).await;
    denied(&pool, &exact, &review, "cancellation review unavailable");
    denied(&pool, &inbox, &review, "cancellation review unavailable");
    denied(
        &pool,
        &combined,
        &both,
        "cancellation packing inspection unavailable",
    );
    assert_eq!(footprint(&pool).await, before);
}
