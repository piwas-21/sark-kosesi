//! Prepare and inspect refund custody through the local worker in isolated PostgreSQL.
#![cfg(unix)]
#[allow(dead_code)]
#[path = "../../infrastructure/tests/support/payment_fixture.rs"]
mod payment_fixture;
#[path = "support/recovery_file.rs"]
mod recovery_file;
#[path = "support/staff_review.rs"]
#[allow(dead_code)]
mod staff_review;

use commerce_application::request_context::Clock;
use commerce_infrastructure::{
    MIGRATOR,
    guest_baskets::GuestCredential,
    payment_attempts::{prepare_attempt, recognition, start_dispatch},
    runtime::SystemClock,
    staff_access::SessionToken,
};
use recovery_file::IntentFile;
use serde_json::{Value, json};
use sqlx::{Executor, PgPool};
use staff_review::{denied, recognize_paid_attempt, report};
use std::{fs, os::unix::fs::PermissionsExt, sync::atomic::AtomicU64};

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

async fn received_order(pool: &PgPool) -> recognition::Recognition {
    let (store, clock, session, csrf, order) = payment_fixture::accepted(pool).await;
    let owner = GuestCredential::from_session(&session).expect("fixture owner");
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
    recognize_paid_attempt(pool, clock.as_ref(), &ids, attempt).await;
    recognition::current(pool, &attempt.binding.id)
        .await
        .expect("validated current proof")
        .expect("received recognition")
}

fn allocation() -> Value {
    json!({
        "currency": "EUR",
        "lines": [{"sku":"SET-FILLED","grossMinor":2000,"netMinor":1653,"vatMinor":347}],
        "shipping": {"grossMinor":101,"netMinor":83,"vatMinor":18},
        "total": {"grossMinor":2101,"netMinor":1736,"vatMinor":365}
    })
}

fn replay_of(mut receipt: Value) -> Value {
    receipt["replayed"] = json!(true);
    receipt
}

async fn footprint(pool: &PgPool) -> (i64, i64, i64, i64, i64) {
    sqlx::query_as("SELECT (SELECT count(*) FROM refund_reservations),(SELECT count(*) FROM refund_execution_proposals),(SELECT count(*) FROM commerce_outbox),(SELECT count(*) FROM stock_movements),(SELECT count(*) FROM payment_observations)")
        .fetch_one(pool).await.expect("private effect counts")
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn refund_reversal_cli_reports_only_validated_and_redacted_custody(pool: PgPool) {
    staff(&pool).await;
    let proof = Box::pin(received_order(&pool)).await;
    let args = [
        "--inspect-refund-reversal-evidence",
        proof.order_id.as_str(),
    ];
    let missing = [
        "--inspect-refund-reversal-evidence",
        "99999999999999999999999999999999",
    ];
    let before = footprint(&pool).await;
    for command in [args, missing] {
        denied(
            &pool,
            &command,
            &RESERVER,
            "refund reversal evidence inspection unavailable",
        );
    }
    assert_eq!(
        report(&pool, &missing, &REVIEWER),
        json!({"kind":"refund_reversal_evidence","evidence":null}),
    );
    let actual = report(&pool, &args, &REVIEWER);
    let digest = actual["evidence"]["digest"]
        .as_str()
        .expect("closed digest");
    assert_eq!(digest.len(), 64);
    assert!(
        digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    );
    assert_eq!(
        actual,
        json!({
            "kind":"refund_reversal_evidence",
            "evidence":{
                "orderId":proof.order_id,"recognitionId":proof.id,"throughObservation":1,
                "parentOperations":0,"parentPending":0,"parentComplete":0,
                "childObservations":0,"digest":digest
            }
        }),
    );
    let output = actual.to_string();
    for private in [
        proof.attempt_id.as_str(),
        proof.resolution_id.as_str(),
        "tr_SyntheticReviewPayment",
        "synthetic-proposal-reviewer",
    ] {
        assert!(
            !output.contains(private),
            "internal custody and account identity remain private"
        );
    }
    assert_eq!(report(&pool, &args, &REVIEWER), actual);
    assert_eq!(footprint(&pool).await, before);

    pool.execute("ALTER TABLE commerce_outbox DISABLE TRIGGER USER; DELETE FROM commerce_outbox WHERE kind='payment.recognized.v1'; ALTER TABLE commerce_outbox ENABLE TRIGGER USER")
        .await.expect("isolated damaged recognition custody");
    let damaged = footprint(&pool).await;
    denied(
        &pool,
        &args,
        &REVIEWER,
        "refund reversal evidence inspection unavailable",
    );
    assert_eq!(footprint(&pool).await, damaged);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn proposal_cli_returns_only_authorized_validated_allocations(pool: PgPool) {
    staff(&pool).await;
    let proof = Box::pin(received_order(&pool)).await;
    let order = proof.order_id.as_str();
    let missing_id = "99999999999999999999999999999999";
    let reserve_source = ["--inspect-refund-source", order, "reservation"];
    let proposal_source = ["--inspect-refund-source", order, "proposal"];
    let before = footprint(&pool).await;
    assert_eq!(
        report(&pool, &reserve_source, &RESERVER),
        json!({
            "kind":"refund_preparation_source","phase":"reservation",
            "source":{"orderId":order,"recognitionId":proof.id},
        })
    );
    assert_eq!(
        report(&pool, &proposal_source, &REVIEWER),
        json!({
            "kind":"refund_preparation_source","phase":"proposal",
            "source":{"orderId":order,"recognitionId":proof.id,"throughObservation":1},
        })
    );
    let empty = ["--inspect-refund-source", missing_id, "proposal"];
    assert_eq!(
        report(&pool, &empty, &REVIEWER),
        json!({
            "kind":"refund_preparation_source","phase":"proposal","source":null,
        })
    );
    for (args, token) in [
        (&reserve_source, &REVIEWER),
        (&proposal_source, &RESERVER),
        (&empty, &RESERVER),
    ] {
        denied(&pool, args, token, "refund preparation source unavailable");
    }

    let reserve_file = IntentFile::new();
    let reserve = ["--reserve-refund", order, reserve_file.path(), "--confirm"];
    let reserve_intent = json!({
        "expectedRecognitionId":proof.id,"effectKey":"refund-cli-reservation",
        "lines":[{"sku":"SET-FILLED","grossMinor":2000}],"shippingGrossMinor":101,
        "reason":"Synthetic private reservation reason",
    });
    for (key, value, error) in [
        (
            "unknown",
            json!("private-mark"),
            "refund preparation input unavailable",
        ),
        (
            "expectedRecognitionId",
            json!(missing_id),
            "refund reservation unavailable",
        ),
        (
            "lines",
            json!([{"sku":"SET-FILLED","grossMinor":2000},{"sku":"SET-FILLED","grossMinor":1}]),
            "refund reservation unavailable",
        ),
        (
            "shippingGrossMinor",
            json!(9007199254740991_i64),
            "refund reservation unavailable",
        ),
    ] {
        let mut invalid = reserve_intent.clone();
        invalid[key] = value;
        reserve_file.write(&invalid);
        denied(&pool, &reserve, &RESERVER, error);
    }
    for raw in [
        "[]",
        r#"{"expectedRecognitionId":"a","expectedRecognitionId":"b"}"#,
    ] {
        fs::write(reserve_file.path(), raw).expect("adversarial input fixture");
        denied(
            &pool,
            &reserve,
            &RESERVER,
            "refund preparation input unavailable",
        );
    }
    reserve_file.write(&reserve_intent);
    fs::set_permissions(reserve_file.path(), fs::Permissions::from_mode(0o640))
        .expect("public group permission fixture");
    denied(
        &pool,
        &reserve,
        &RESERVER,
        "refund preparation input unavailable",
    );
    fs::set_permissions(reserve_file.path(), fs::Permissions::from_mode(0o600))
        .expect("restore private fixture");
    denied(&pool, &reserve, &REVIEWER, "refund reservation unavailable");
    assert_eq!(footprint(&pool).await, before);

    let reservation = report(&pool, &reserve, &RESERVER);
    let reservation_id = reservation["id"].as_str().expect("reservation ID");
    assert_eq!(
        reservation,
        json!({
            "kind":"refund_reservation","id":reservation_id,"orderId":order,
            "generation":1,"reservedAt":reservation["reservedAt"].as_i64().expect("time"),
            "replayed":false,"allocation":allocation(),
        })
    );
    assert_eq!(
        report(&pool, &reserve, &RESERVER),
        replay_of(reservation.clone())
    );
    let mut changed = reserve_intent;
    changed["reason"] = json!("Changed private intent");
    reserve_file.write(&changed);
    denied(&pool, &reserve, &RESERVER, "refund reservation unavailable");
    assert_eq!(
        footprint(&pool).await,
        (before.0 + 1, before.1, before.2 + 1, before.3, before.4)
    );

    let proposal_file = IntentFile::new();
    let propose = ["--propose-refund", order, proposal_file.path(), "--confirm"];
    let proposal_intent = json!({
        "reservationId":reservation_id,"expectedRecognitionId":proof.id,
        "expectedThroughObservation":1,"effectKey":"refund-cli-proposal",
        "reason":"Synthetic private proposal reason",
    });
    for (key, value) in [
        ("expectedThroughObservation", json!(2)),
        ("reservationId", json!(missing_id)),
    ] {
        let mut invalid = proposal_intent.clone();
        invalid[key] = value;
        proposal_file.write(&invalid);
        denied(
            &pool,
            &propose,
            &REVIEWER,
            "refund proposal preparation unavailable",
        );
    }
    proposal_file.write(&proposal_intent);
    denied(
        &pool,
        &propose,
        &RESERVER,
        "refund proposal preparation unavailable",
    );
    denied(
        &pool,
        &[propose[0], missing_id, propose[2], propose[3]],
        &REVIEWER,
        "refund proposal preparation unavailable",
    );
    assert_eq!(
        footprint(&pool).await,
        (before.0 + 1, before.1, before.2 + 1, before.3, before.4)
    );

    let proposal = report(&pool, &propose, &REVIEWER);
    let proposal_id = proposal["id"].as_str().expect("proposal ID");
    let proposed_at = proposal["proposedAt"].as_i64().expect("proposal time");
    assert_eq!(
        proposal,
        json!({
            "kind":"refund_proposal_prepared","id":proposal_id,"orderId":order,
            "reservationId":reservation_id,"proposedAt":proposed_at,
            "replayed":false,"allocation":allocation(),
        })
    );
    assert_eq!(
        report(&pool, &propose, &REVIEWER),
        replay_of(proposal.clone())
    );
    let mut changed = proposal_intent;
    changed["effectKey"] = json!("another-proposal-for-same-reservation");
    proposal_file.write(&changed);
    denied(
        &pool,
        &propose,
        &REVIEWER,
        "refund proposal preparation unavailable",
    );
    assert_eq!(
        footprint(&pool).await,
        (before.0 + 1, before.1 + 1, before.2 + 2, before.3, before.4)
    );

    let missing = ["--inspect-refund-proposal", missing_id];
    let exact = ["--inspect-refund-proposal", proposal_id];
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
                "id": proposal_id,
                "orderId": order,
                "reservationId": reservation_id,
                "proposedAt": proposed_at,
                "allocation": allocation()
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
