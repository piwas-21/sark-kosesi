//! Exercise the refund proposal worker on an isolated synthetic payment history.
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
    payment_attempts::{prepare_attempt, start_dispatch},
    refund_execution_proposals::{ProposalReceipt, ProposalRequest, propose},
    refund_reservations::{RefundLine, RefundRequest, reserve},
    runtime::SystemClock,
    staff_access::SessionToken,
};
use serde_json::json;
use sqlx::{Executor, PgPool};
use staff_review::{denied, report, settle_paid_attempt};
use std::sync::atomic::AtomicU64;

const RESERVER: [u8; 32] = [92; 32];
const REVIEWER: [u8; 32] = [93; 32];

async fn staff(pool: &PgPool) {
    let now = i64::try_from(SystemClock.unix_seconds().expect("clock")).expect("SQL time");
    for (id, token, capability) in [
        ("synthetic-refund-reserver", RESERVER, "refund.prepare"),
        (
            "synthetic-proposal-reviewer",
            REVIEWER,
            "refund.execution.prepare",
        ),
    ] {
        sqlx::query("INSERT INTO staff_accounts(id,enabled) VALUES ($1,true)")
            .bind(id)
            .execute(pool)
            .await
            .expect("synthetic account");
        sqlx::query("INSERT INTO staff_capabilities(staff_id,capability) VALUES ($1,$2)")
            .bind(id)
            .bind(capability)
            .execute(pool)
            .await
            .expect("separate staff grant");
        sqlx::query("INSERT INTO staff_sessions(token_hash,staff_id,created_at,expires_at,mfa_verified_at) VALUES ($1,$2,$3,$4,$3)")
            .bind(SessionToken::from_bytes(token).digest().as_slice())
            .bind(id)
            .bind(now - 1)
            .bind(now + 300)
            .execute(pool)
            .await
            .expect("synthetic session");
    }
}

async fn prepare_proposal(pool: &PgPool) -> ProposalReceipt {
    let (store, clock, session, csrf, order) = payment_fixture::accepted(pool).await;
    let owner = GuestCredential::from_session(&session).expect("fixture owner");
    let order_id = &order.identity.id;
    let config = payment_fixture::config();
    let ids = payment_fixture::Ids(AtomicU64::new(92_000));
    let prepared = prepare_attempt(
        &store,
        &owner,
        &csrf,
        "refund-cli-payment",
        &payment_fixture::intent(&order),
        &config,
    )
    .await
    .expect("synthetic payment attempt");
    let attempt = prepared.frozen.snapshot();
    start_dispatch(pool, clock.as_ref(), &ids, &attempt.binding.id, &config)
        .await
        .expect("exact dispatch start");
    settle_paid_attempt(pool, clock.as_ref(), &ids, attempt).await;
    let (recognition, through): (String, i64) = sqlx::query_as(
        "SELECT r.id,r.through_observation FROM payment_recognition_current c JOIN payment_recognitions r ON r.id=c.recognition_id WHERE c.order_id=$1",
    )
    .bind(order_id)
    .fetch_one(pool)
    .await
    .expect("received recognition");
    let reservation = reserve(
        pool,
        &SessionToken::from_bytes(RESERVER),
        &SystemClock,
        &ids,
        RefundRequest {
            order_id,
            expected_recognition_id: &recognition,
            effect_key: "refund-cli-reservation",
            lines: &[RefundLine {
                sku: "SET-FILLED",
                gross_minor: 2000,
            }],
            shipping_gross_minor: 101,
            reason: "Synthetic private reservation reason",
            confirmed: true,
        },
    )
    .await
    .expect("exact refund reservation");
    propose(
        pool,
        &SessionToken::from_bytes(REVIEWER),
        &SystemClock,
        &ids,
        ProposalRequest {
            order_id,
            reservation_id: &reservation.id,
            expected_recognition_id: &recognition,
            expected_through_observation: through,
            effect_key: "refund-cli-proposal",
            reason: "Synthetic private proposal reason",
            confirmed: true,
        },
    )
    .await
    .expect("validated historical proposal")
}

async fn footprint(pool: &PgPool) -> (i64, i64, i64, i64, i64) {
    sqlx::query_as("SELECT (SELECT count(*) FROM refund_reservations),(SELECT count(*) FROM refund_execution_proposals),(SELECT count(*) FROM commerce_outbox),(SELECT count(*) FROM stock_movements),(SELECT count(*) FROM payment_observations)")
        .fetch_one(pool).await.expect("private effect counts")
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn proposal_cli_returns_only_authorized_validated_allocations(pool: PgPool) {
    staff(&pool).await;
    let proposal = Box::pin(prepare_proposal(&pool)).await;
    let missing = [
        "--inspect-refund-proposal",
        "99999999999999999999999999999999",
    ];
    let exact = ["--inspect-refund-proposal", proposal.id.as_str()];
    let before = footprint(&pool).await;
    assert_eq!(
        report(&pool, &missing, &REVIEWER),
        json!({"kind":"refund_execution_proposal","proposal":null}),
    );
    for args in [missing, exact] {
        denied(
            &pool,
            &args,
            &RESERVER,
            "refund proposal inspection unavailable",
        );
    }
    assert_eq!(
        report(&pool, &exact, &REVIEWER),
        json!({
            "kind": "refund_execution_proposal",
            "proposal": {
                "id": proposal.id,
                "orderId": proposal.order_id,
                "reservationId": proposal.reservation_id,
                "proposedAt": proposal.proposed_at,
                "allocation": {
                    "currency": "EUR",
                    "lines": [{"sku":"SET-FILLED","grossMinor":2000,"netMinor":1653,"vatMinor":347}],
                    "shipping": {"grossMinor":101,"netMinor":83,"vatMinor":18},
                    "total": {"grossMinor":2101,"netMinor":1736,"vatMinor":365}
                }
            }
        }),
    );
    assert_eq!(footprint(&pool).await, before);

    // Detach the event in this disposable fixture to distinguish damage from absence.
    pool.execute("ALTER TABLE commerce_outbox DISABLE TRIGGER USER; DELETE FROM commerce_outbox WHERE kind='order.refund_execution_proposed.v1'; ALTER TABLE commerce_outbox ENABLE TRIGGER USER")
        .await.expect("isolated damaged custody");
    let after_damage = footprint(&pool).await;
    denied(
        &pool,
        &exact,
        &REVIEWER,
        "refund proposal inspection unavailable",
    );
    assert_eq!(footprint(&pool).await, after_damage);
}
