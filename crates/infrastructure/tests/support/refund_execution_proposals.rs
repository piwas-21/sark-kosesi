//! Private proposal custody against genuine accepted orders and PostgreSQL.
use super::order_stock_cases::fixture::{self, accepted, footprint, observe};
use crate::{
    MIGRATOR,
    payment_attempts::recognition::recognize,
    refund_execution_proposals::{ProposalError, ProposalRequest, propose},
    refund_reservations::{RefundLine, RefundRequest, reserve},
    staff_access::SessionToken,
};
use sqlx::{Executor, PgPool};
use std::sync::atomic::Ordering;

async fn staff(pool: &PgPool, id: &str, byte: u8, grant: &str) -> SessionToken {
    let token = SessionToken::from_bytes([byte; 32]);
    sqlx::query("INSERT INTO staff_accounts(id,enabled) VALUES($1,true)")
        .bind(id)
        .execute(pool)
        .await
        .expect("staff account");
    sqlx::query("INSERT INTO staff_capabilities VALUES($1,$2)")
        .bind(id)
        .bind(grant)
        .execute(pool)
        .await
        .expect("separate grant");
    sqlx::query("INSERT INTO staff_sessions VALUES($1,$2,1000,4000,1400,NULL)")
        .bind(token.digest().as_slice())
        .bind(id)
        .execute(pool)
        .await
        .expect("session");
    token
}

async fn reservation(pool: &PgPool, f: &fixture::Fixture) -> (String, String, i64) {
    let finance = staff(pool, "reservation-finance", 94, "refund.prepare").await;
    let (proof, through): (String, i64) = sqlx::query_as("SELECT r.id,r.through_observation FROM payment_recognition_current c JOIN payment_recognitions r ON r.id=c.recognition_id WHERE c.order_id=$1")
        .bind(&f.order.identity.id).fetch_one(pool).await.expect("received recognition");
    let lines = [
        RefundLine {
            sku: "SET-COVERS",
            gross_minor: 1000,
        },
        RefundLine {
            sku: "SET-FILLED",
            gross_minor: 2000,
        },
    ];
    let r = reserve(
        pool,
        &finance,
        f.time.as_ref(),
        f.ids.as_ref(),
        RefundRequest {
            order_id: &f.order.identity.id,
            expected_recognition_id: &proof,
            effect_key: "proposal-reservation",
            lines: &lines,
            shipping_gross_minor: 101,
            reason: "Synthetic partial reservation",
            confirmed: true,
        },
    )
    .await
    .expect("genuine reservation");
    (r.id, proof, through)
}
fn request<'a>(
    order: &'a str,
    reservation: &'a str,
    proof: &'a str,
    through: i64,
    key: &'a str,
) -> ProposalRequest<'a> {
    ProposalRequest {
        order_id: order,
        reservation_id: reservation,
        expected_recognition_id: proof,
        expected_through_observation: through,
        effect_key: key,
        reason: "Synthetic execution proposal",
        confirmed: true,
    }
}
async fn counts(pool: &PgPool) -> (i64, i64) {
    sqlx::query_as("SELECT (SELECT count(*) FROM refund_execution_proposals),(SELECT count(*) FROM commerce_outbox WHERE kind='order.refund_execution_proposed.v1')")
        .fetch_one(pool).await.expect("proposal and event counts")
}
async fn wait_for_attempt_claim(pool: &PgPool) {
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'SELECT id FROM payment_attempts WHERE id=%FOR UPDATE%')")
                .fetch_one(pool).await.expect("inspect real lock wait");
            if waiting { break; }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }).await.expect("proposal reached attempt lock");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn suppressed_proposal_or_event_never_leaves_partial_custody(pool: PgPool) {
    let f = accepted(&pool, false, true).await;
    let (reservation, proof, through) = reservation(&pool, &f).await;
    let token = staff(&pool, "proposal-finance", 95, "refund.execution.prepare").await;
    let run = || {
        propose(
            &pool,
            &token,
            f.time.as_ref(),
            f.ids.as_ref(),
            request(
                &f.order.identity.id,
                &reservation,
                &proof,
                through,
                "suppressed-proposal",
            ),
        )
    };
    pool.execute("CREATE FUNCTION suppress_test_proposal() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RETURN NULL; END; $$")
        .await.expect("isolated test fault");
    pool.execute("CREATE TRIGGER z_suppress_test_proposal BEFORE INSERT ON refund_execution_proposals FOR EACH ROW EXECUTE FUNCTION suppress_test_proposal()")
        .await.expect("suppress the proposal");
    assert_eq!(run().await, Err(ProposalError::InvalidStoredData));
    assert_eq!(counts(&pool).await, (0, 0));
    pool.execute("DROP TRIGGER z_suppress_test_proposal ON refund_execution_proposals")
        .await
        .expect("restore proposal writes");
    pool.execute("CREATE TRIGGER z_suppress_test_event BEFORE INSERT ON commerce_outbox FOR EACH ROW WHEN (NEW.kind='order.refund_execution_proposed.v1') EXECUTE FUNCTION suppress_test_proposal()")
        .await.expect("suppress the reciprocal event");
    assert!(
        run().await.is_err(),
        "the native constraint must reject the missing event"
    );
    assert_eq!(counts(&pool).await, (0, 0));
    pool.execute("DROP TRIGGER z_suppress_test_event ON commerce_outbox")
        .await
        .expect("restore event writes");
    assert!(
        !run()
            .await
            .expect("same intent can be prepared after recovery")
            .replayed
    );
    assert_eq!(counts(&pool).await, (1, 1));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn proposal_preserves_exact_partial_allocation_and_has_no_financial_or_stock_effect(
    pool: PgPool,
) {
    let f = accepted(&pool, false, true).await;
    let (reservation, proof, through) = reservation(&pool, &f).await;
    let token = staff(&pool, "proposal-finance", 95, "refund.execution.prepare").await;
    let stock = footprint(&pool).await;
    let original = fixture::unchanged(&pool).await;
    let before: Option<serde_json::Value> = sqlx::query_scalar("SELECT jsonb_agg(to_jsonb(e) ORDER BY id) FROM commerce_outbox e WHERE kind <> 'order.refund_execution_proposed.v1'").fetch_one(&pool).await.expect("existing events");
    let p = propose(
        &pool,
        &token,
        f.time.as_ref(),
        f.ids.as_ref(),
        request(
            &f.order.identity.id,
            &reservation,
            &proof,
            through,
            "proposal-one",
        ),
    )
    .await
    .expect("confirmed proposal");
    assert!(!p.replayed);
    assert_eq!(
        (
            &p.order_id,
            &p.reservation_id,
            &p.recognition_id,
            p.through_observation
        ),
        (&f.order.identity.id, &reservation, &proof, through)
    );
    let lines = p
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
            p.allocation.shipping.gross_minor,
            p.allocation.shipping.net_minor,
            p.allocation.shipping.vat_minor
        ),
        (101, 83, 18)
    );
    assert_eq!(
        (
            p.allocation.total.gross_minor,
            p.allocation.total.net_minor,
            p.allocation.total.vat_minor
        ),
        (3101, 2562, 539)
    );
    assert_eq!(counts(&pool).await, (1, 1));
    assert_eq!(footprint(&pool).await, stock);
    assert_eq!(fixture::unchanged(&pool).await, original);
    let after: Option<serde_json::Value> = sqlx::query_scalar("SELECT jsonb_agg(to_jsonb(e) ORDER BY id) FROM commerce_outbox e WHERE kind <> 'order.refund_execution_proposed.v1'").fetch_one(&pool).await.expect("unchanged events");
    assert_eq!(after, before);
    let unchanged: bool = sqlx::query_scalar("SELECT p.allocation=r.allocation AND refund_execution_proposal_source(p,true) AND e.payload=refund_execution_proposal_event(p) FROM refund_execution_proposals p JOIN refund_reservations r ON r.id=p.reservation_id JOIN commerce_outbox e ON e.id=p.event_id WHERE p.id=$1").bind(&p.id).fetch_one(&pool).await.expect("native positive source and event controls");
    assert!(unchanged);
    assert!(
        pool.execute("UPDATE refund_execution_proposals SET reason='changed'")
            .await
            .is_err()
    );
    assert!(
        pool.execute("DELETE FROM refund_execution_proposals")
            .await
            .is_err()
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn proposal_replay_requires_live_actor_and_survives_later_payment_ambiguity(pool: PgPool) {
    let f = accepted(&pool, false, true).await;
    let (reservation, proof, through) = reservation(&pool, &f).await;
    let token = staff(&pool, "proposal-finance", 95, "refund.execution.prepare").await;
    let other = staff(&pool, "other-finance", 96, "refund.execution.prepare").await;
    let order = &f.order.identity.id;
    let first = propose(
        &pool,
        &token,
        f.time.as_ref(),
        f.ids.as_ref(),
        request(order, &reservation, &proof, through, "proposal-replay"),
    )
    .await
    .expect("initial proposal");
    assert!(
        propose(
            &pool,
            &other,
            f.time.as_ref(),
            f.ids.as_ref(),
            request(order, &reservation, &proof, through, "proposal-replay")
        )
        .await
        .is_err(),
        "different actor cannot claim receipt"
    );
    let mut changed = request(order, &reservation, &proof, through, "proposal-replay");
    changed.reason = "Changed intent";
    assert!(
        propose(&pool, &token, f.time.as_ref(), f.ids.as_ref(), changed)
            .await
            .is_err()
    );
    assert!(
        propose(
            &pool,
            &token,
            f.time.as_ref(),
            f.ids.as_ref(),
            request(order, &reservation, &proof, through, "proposal-another-key")
        )
        .await
        .is_err()
    );
    for altered in [
        request(
            order,
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            &proof,
            through,
            "proposal-replay",
        ),
        request(
            order,
            &reservation,
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            through,
            "proposal-replay",
        ),
        request(order, &reservation, &proof, through + 1, "proposal-replay"),
    ] {
        assert!(
            matches!(
                propose(&pool, &token, f.time.as_ref(), f.ids.as_ref(), altered).await,
                Err(ProposalError::Conflict)
            ),
            "each changed source binding conflicts with the receipt"
        );
    }
    observe(
        &pool,
        &f,
        "tr_AMBIGUOUS",
        "open",
        "proposal-later-ambiguity",
    )
    .await;
    recognize(
        &pool,
        f.time.as_ref(),
        f.ids.as_ref(),
        &f.frozen.snapshot().binding.id,
    )
    .await
    .expect("later needs-review recognition");
    let replay = propose(
        &pool,
        &token,
        f.time.as_ref(),
        f.ids.as_ref(),
        request(order, &reservation, &proof, through, "proposal-replay"),
    )
    .await
    .expect("historical replay");
    assert!(replay.replayed);
    assert_eq!(replay.id, first.id);
    assert_eq!(replay.allocation, first.allocation);
    f.time.0.store(2301, Ordering::SeqCst);
    assert!(
        propose(
            &pool,
            &token,
            f.time.as_ref(),
            f.ids.as_ref(),
            request(order, &reservation, &proof, through, "proposal-replay")
        )
        .await
        .is_err(),
        "expired MFA prevents replay"
    );
    assert_eq!(counts(&pool).await, (1, 1));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn authority_and_exact_current_watermark_gate_new_proposals(pool: PgPool) {
    let f = accepted(&pool, false, true).await;
    let (reservation, proof, through) = reservation(&pool, &f).await;
    let token = staff(&pool, "proposal-finance", 95, "refund.execution.prepare").await;
    let reservation_only = staff(&pool, "reservation-only", 96, "refund.prepare").await;
    let revoked = staff(&pool, "revoked", 97, "refund.execution.prepare").await;
    let expired = staff(&pool, "expired", 98, "refund.execution.prepare").await;
    sqlx::query("UPDATE staff_sessions SET expires_at=1499 WHERE token_hash=$1")
        .bind(expired.digest().as_slice())
        .execute(&pool)
        .await
        .expect("expire session");
    sqlx::query("UPDATE staff_sessions SET revoked_at=1500 WHERE token_hash=$1")
        .bind(revoked.digest().as_slice())
        .execute(&pool)
        .await
        .expect("revoke session");
    let order = &f.order.identity.id;
    for (actor, key) in [
        (&reservation_only, "reservation-only"),
        (&revoked, "revoked"),
        (&expired, "expired"),
    ] {
        assert!(matches!(
            propose(
                &pool,
                actor,
                f.time.as_ref(),
                f.ids.as_ref(),
                request(order, &reservation, &proof, through, key)
            )
            .await,
            Err(ProposalError::Denied)
        ));
    }
    assert!(
        propose(
            &pool,
            &token,
            f.time.as_ref(),
            f.ids.as_ref(),
            request(order, &reservation, &proof, through + 1, "wrong-watermark")
        )
        .await
        .is_err()
    );
    assert!(
        propose(
            &pool,
            &token,
            f.time.as_ref(),
            f.ids.as_ref(),
            request(
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                &reservation,
                &proof,
                through,
                "wrong-order"
            )
        )
        .await
        .is_err()
    );
    let mut unconfirmed = request(order, &reservation, &proof, through, "unconfirmed");
    unconfirmed.confirmed = false;
    assert!(
        propose(&pool, &token, f.time.as_ref(), f.ids.as_ref(), unconfirmed)
            .await
            .is_err()
    );
    f.time.0.store(2301, Ordering::SeqCst);
    assert!(
        propose(
            &pool,
            &token,
            f.time.as_ref(),
            f.ids.as_ref(),
            request(order, &reservation, &proof, through, "stale-mfa")
        )
        .await
        .is_err()
    );
    f.time.0.store(1500, Ordering::SeqCst);
    assert_eq!(counts(&pool).await, (0, 0));
    propose(
        &pool,
        &token,
        f.time.as_ref(),
        f.ids.as_ref(),
        request(order, &reservation, &proof, through, "positive-control"),
    )
    .await
    .expect("same source with live independent authority succeeds");
    assert_eq!(counts(&pool).await, (1, 1));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn competing_proposals_claim_reservation_once(pool: PgPool) {
    let f = accepted(&pool, false, true).await;
    let (reservation, proof, through) = reservation(&pool, &f).await;
    let token = staff(&pool, "proposal-finance", 95, "refund.execution.prepare").await;
    let order = &f.order.identity.id;
    let (a, b) = tokio::join!(
        propose(
            &pool,
            &token,
            f.time.as_ref(),
            f.ids.as_ref(),
            request(order, &reservation, &proof, through, "contender-a")
        ),
        propose(
            &pool,
            &token,
            f.time.as_ref(),
            f.ids.as_ref(),
            request(order, &reservation, &proof, through, "contender-b")
        )
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    assert_eq!(counts(&pool).await, (1, 1));
}

// Stage a native proposal from independently persisted reservation/payment facts.
// The caller supplies the exact event in the same transaction when appropriate.
const RAW_PROPOSAL: &str = r#"INSERT INTO refund_execution_proposals
 (id,order_id,attempt_id,reservation_id,reservation_sha256,recognition_id,through_observation,
  profile_id,payment_id,actor_id,effect_key,reason,proposed_at,allocation,event_id,transaction_id)
 SELECT 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',r.order_id,r.attempt_id,r.id,refund_reservation_digest(r),
  p.id,p.through_observation+$2,a.configuration->'profile'->>'id',s.summary->'paid_provider_ids'->>0,
  'proposal-finance','native-proposal','Synthetic native proposal',1500,r.allocation,
  'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb',pg_current_xact_id()
 FROM refund_reservations r JOIN payment_recognitions p ON p.id=r.recognition_id
 JOIN payment_identity_resolutions s ON s.id=p.resolution_id JOIN payment_attempts a ON a.id=r.attempt_id
 WHERE r.id=$1"#;
const RAW_EVENT: &str = "INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) SELECT event_id,order_id,'order.refund_execution_proposed.v1',refund_execution_proposal_event(p),proposed_at FROM refund_execution_proposals p WHERE effect_key='native-proposal'";

#[sqlx::test(migrator = "MIGRATOR")]
async fn native_constraints_require_reciprocal_event_exact_binding_and_seal_after_flush(
    pool: PgPool,
) {
    let f = accepted(&pool, false, true).await;
    let (reservation, _, _) = reservation(&pool, &f).await;
    let _token = staff(&pool, "proposal-finance", 95, "refund.execution.prepare").await;
    let mut tx = pool.begin().await.expect("orphan transaction");
    sqlx::query(RAW_PROPOSAL)
        .bind(&reservation)
        .bind(0_i64)
        .execute(&mut *tx)
        .await
        .expect("stage proposal");
    assert!(
        tx.commit().await.is_err(),
        "proposal cannot commit without reciprocal event"
    );
    assert_eq!(counts(&pool).await, (0, 0));
    assert!(sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) VALUES('cccccccccccccccccccccccccccccccc',$1,'order.refund_execution_proposed.v1',jsonb_build_object('schema_version',1,'order_id',$1,'proposal_id','aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa','proposal_sha256',repeat('0',64)),1500)").bind(&f.order.identity.id).execute(&pool).await.is_err(),"orphan event cannot be inserted");
    let mut tx = pool.begin().await.expect("malformed transaction");
    sqlx::query(RAW_PROPOSAL)
        .bind(&reservation)
        .bind(1_i64)
        .execute(&mut *tx)
        .await
        .expect("stage wrong watermark");
    tx.execute(RAW_EVENT)
        .await
        .expect("matching event alone is insufficient");
    assert!(
        tx.commit().await.is_err(),
        "native source rejects wrong watermark"
    );
    assert_eq!(counts(&pool).await, (0, 0));
    let mut tx = pool.begin().await.expect("valid transaction");
    sqlx::query(RAW_PROPOSAL)
        .bind(&reservation)
        .bind(0_i64)
        .execute(&mut *tx)
        .await
        .expect("stage exact proposal");
    tx.execute(RAW_EVENT).await.expect("stage exact event");
    tx.execute("SET CONSTRAINTS require_complete_refund_execution_proposal IMMEDIATE")
        .await
        .expect("exact source and event pass");
    assert!(
        sqlx::query(
            "UPDATE payment_recognition_current SET recognition_id=recognition_id WHERE order_id=$1"
        )
        .bind(&f.order.identity.id)
        .execute(&mut *tx)
        .await
        .is_err(),
        "source is sealed for remainder of transaction"
    );
    tx.rollback().await.expect("discard failed transaction");
    assert_eq!(counts(&pool).await, (0, 0));
}

#[sqlx::test(migrations = false)]
async fn schema48_upgrade_preserves_existing_reservation_and_accepts_proposal(pool: PgPool) {
    MIGRATOR.run_to(47, &pool).await.expect("schema 47");
    let f = accepted(&pool, false, true).await;
    let (reservation, proof, through) = reservation(&pool, &f).await;
    let stock = footprint(&pool).await;
    let old_events: Vec<serde_json::Value> =
        sqlx::query_scalar("SELECT to_jsonb(e) FROM commerce_outbox e ORDER BY id")
            .fetch_all(&pool)
            .await
            .expect("existing events");
    let old_reservation: serde_json::Value =
        sqlx::query_scalar("SELECT to_jsonb(r) FROM refund_reservations r WHERE id=$1")
            .bind(&reservation)
            .fetch_one(&pool)
            .await
            .expect("existing reservation");
    MIGRATOR.run_to(48, &pool).await.expect("upgrade schema");
    assert_eq!(footprint(&pool).await, stock);
    assert_eq!(
        sqlx::query_scalar::<_, serde_json::Value>(
            "SELECT to_jsonb(e) FROM commerce_outbox e ORDER BY id"
        )
        .fetch_all(&pool)
        .await
        .expect("events preserved"),
        old_events
    );
    assert_eq!(
        sqlx::query_scalar::<_, serde_json::Value>(
            "SELECT to_jsonb(r) FROM refund_reservations r WHERE id=$1"
        )
        .bind(&reservation)
        .fetch_one(&pool)
        .await
        .expect("reservation preserved"),
        old_reservation
    );
    let token = staff(&pool, "proposal-finance", 95, "refund.execution.prepare").await;
    propose(
        &pool,
        &token,
        f.time.as_ref(),
        f.ids.as_ref(),
        request(
            &f.order.identity.id,
            &reservation,
            &proof,
            through,
            "upgrade-proposal",
        ),
    )
    .await
    .expect("preserved source accepts first proposal");
    assert_eq!(counts(&pool).await, (1, 1));
    assert_eq!(footprint(&pool).await, stock);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn stale_received_proof_cannot_create_a_new_proposal(pool: PgPool) {
    let f = accepted(&pool, false, true).await;
    let (reservation, proof, through) = reservation(&pool, &f).await;
    let token = staff(&pool, "proposal-finance", 95, "refund.execution.prepare").await;
    observe(
        &pool,
        &f,
        "tr_AMBIGUOUS",
        "open",
        "stale-proposal-observation",
    )
    .await;
    recognize(
        &pool,
        f.time.as_ref(),
        f.ids.as_ref(),
        &f.frozen.snapshot().binding.id,
    )
    .await
    .expect("new ambiguous recognition");
    assert!(
        propose(
            &pool,
            &token,
            f.time.as_ref(),
            f.ids.as_ref(),
            request(
                &f.order.identity.id,
                &reservation,
                &proof,
                through,
                "stale-proposal"
            )
        )
        .await
        .is_err()
    );
    assert_eq!(counts(&pool).await, (0, 0));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn mfa_expiry_during_attempt_lock_wait_rolls_back_and_fresh_retry_succeeds(pool: PgPool) {
    let f = accepted(&pool, false, true).await;
    let (reservation, proof, through) = reservation(&pool, &f).await;
    let token = staff(&pool, "proposal-finance", 95, "refund.execution.prepare").await;
    let order = f.order.identity.id.clone();
    let mut blocker = pool.begin().await.expect("attempt blocker");
    sqlx::query("SELECT id FROM payment_attempts WHERE id=$1 FOR UPDATE")
        .bind(&f.frozen.snapshot().binding.id)
        .execute(&mut *blocker)
        .await
        .expect("hold canonical attempt lock");
    let worker_pool = pool.clone();
    let worker_clock = f.time.clone();
    let worker_ids = f.ids.clone();
    let worker_order = order.clone();
    let worker_reservation = reservation.clone();
    let worker_proof = proof.clone();
    let worker = tokio::spawn(async move {
        let result = propose(
            &worker_pool,
            &token,
            worker_clock.as_ref(),
            worker_ids.as_ref(),
            request(
                &worker_order,
                &worker_reservation,
                &worker_proof,
                through,
                "wait-mfa",
            ),
        )
        .await;
        (token, result)
    });
    wait_for_attempt_claim(&pool).await;
    f.time.0.store(2301, Ordering::SeqCst);
    blocker.rollback().await.expect("release attempt lock");
    let (token, result) = worker.await.expect("worker completed");
    assert!(
        matches!(result, Err(ProposalError::Denied)),
        "MFA expires while waiting"
    );
    assert_eq!(counts(&pool).await, (0, 0));
    f.time.0.store(1500, Ordering::SeqCst);
    propose(
        &pool,
        &token,
        f.time.as_ref(),
        f.ids.as_ref(),
        request(&order, &reservation, &proof, through, "wait-mfa"),
    )
    .await
    .expect("same key succeeds with fresh authority");
    assert_eq!(counts(&pool).await, (1, 1));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn existing_other_paid_order_cannot_adopt_reservation_or_recognition(pool: PgPool) {
    use crate::payment_attempts::{identities::resolve, observations::record_verified};
    let f = accepted(&pool, false, true).await;
    let (reservation, proof, through) = reservation(&pool, &f).await;
    let token = staff(&pool, "proposal-finance", 95, "refund.execution.prepare").await;
    let second = super::identity_ownership::additional(
        &pool,
        f.time.clone(),
        &f.frozen.snapshot().binding.profile.id,
    )
    .await;
    let observation = fixture::verified(&second, "tr_SECOND_ORDER", "paid").await;
    record_verified(
        &pool,
        f.time.as_ref(),
        f.ids.as_ref(),
        &second.snapshot().binding.id,
        "second-paid",
        &observation,
    )
    .await
    .expect("second authenticated paid observation");
    resolve(
        &pool,
        f.time.as_ref(),
        f.ids.as_ref(),
        &second.snapshot().binding.id,
        "second-paid",
    )
    .await
    .expect("second provider identity");
    let other = recognize(
        &pool,
        f.time.as_ref(),
        f.ids.as_ref(),
        &second.snapshot().binding.id,
    )
    .await
    .expect("second paid recognition")
    .recognition;
    let other_order = &second.snapshot().binding.order_id;
    assert_ne!(other_order, &f.order.identity.id);
    assert!(
        propose(
            &pool,
            &token,
            f.time.as_ref(),
            f.ids.as_ref(),
            request(
                other_order,
                &reservation,
                &proof,
                through,
                "cross-reservation"
            )
        )
        .await
        .is_err(),
        "other order cannot adopt first reservation and proof"
    );
    assert!(
        propose(
            &pool,
            &token,
            f.time.as_ref(),
            f.ids.as_ref(),
            request(
                &f.order.identity.id,
                &reservation,
                &other.id,
                other.through_observation,
                "cross-recognition"
            )
        )
        .await
        .is_err(),
        "first reservation cannot adopt second order recognition"
    );
    assert_eq!(counts(&pool).await, (0, 0));
    propose(
        &pool,
        &token,
        f.time.as_ref(),
        f.ids.as_ref(),
        request(
            &f.order.identity.id,
            &reservation,
            &proof,
            through,
            "cross-control",
        ),
    )
    .await
    .expect("original proof remains usable");
    assert_eq!(counts(&pool).await, (1, 1));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn newer_received_recognition_can_propose_the_original_reserved_allocation(pool: PgPool) {
    let f = accepted(&pool, false, true).await;
    let (reservation, old_proof, old_through) = reservation(&pool, &f).await;
    let token = staff(&pool, "proposal-finance", 95, "refund.execution.prepare").await;
    let saved: serde_json::Value =
        sqlx::query_scalar("SELECT allocation FROM refund_reservations WHERE id=$1")
            .bind(&reservation)
            .fetch_one(&pool)
            .await
            .expect("original partial allocation");
    let attempt = &f.frozen.snapshot().binding.id;
    let order = &f.order.identity.id;
    let mut blocker = pool.begin().await.expect("source writer");
    sqlx::query("SELECT id FROM payment_attempts WHERE id=$1 FOR UPDATE")
        .bind(attempt)
        .execute(&mut *blocker)
        .await
        .expect("canonical claim");
    let waiter = propose(
        &pool,
        &token,
        f.time.as_ref(),
        f.ids.as_ref(),
        request(
            order,
            &reservation,
            &old_proof,
            old_through,
            "new-current-proof",
        ),
    );
    let change = async {
        wait_for_attempt_claim(&pool).await;
        // Repeat the exact authenticated paid projection at the next revision while
        // holding the claim. Native guards require a new observation and event.
        let id = "dddddddddddddddddddddddddddddddd";
        let result = sqlx::query(r#"
            INSERT INTO payment_observations
                (id,attempt_id,effect_key,revision,recorded_at,provider_id,credential_fingerprint,status,snapshot)
            SELECT $3,attempt_id,'same-payment-later',revision+1,recorded_at,provider_id,credential_fingerprint,status,
                snapshot||jsonb_build_object('id',$3::text,'effect_key','same-payment-later','revision',revision+1)
            FROM payment_observations WHERE attempt_id=$1 AND revision=$2
        "#).bind(attempt).bind(old_through).bind(id)
            .execute(&mut *blocker).await.expect("native next paid observation");
        assert_eq!(result.rows_affected(), 1);
        sqlx::query(
            r#"
            INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at)
            SELECT 'eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee',$2,'payment.observation_recorded.v1',
                jsonb_build_object('schema_version',1,'order_id',$2::text,'attempt_id',attempt_id,
                    'observation_id',id),recorded_at FROM payment_observations WHERE id=$1
        "#,
        )
        .bind(id)
        .bind(order)
        .execute(&mut *blocker)
        .await
        .expect("exact next observation event");
        blocker
            .commit()
            .await
            .expect("source update commits ahead of waiting proposal");
    };
    let (stale, ()) = tokio::join!(waiter, change);
    assert_eq!(stale, Err(ProposalError::Unavailable));
    assert_eq!(counts(&pool).await, (0, 0));
    crate::payment_attempts::identities::resolve(
        &pool,
        f.time.as_ref(),
        f.ids.as_ref(),
        attempt,
        "same-payment-later",
    )
    .await
    .expect("current identity resolution after source commit");
    let current = recognize(
        &pool,
        f.time.as_ref(),
        f.ids.as_ref(),
        &f.frozen.snapshot().binding.id,
    )
    .await
    .expect("new recognition of the same paid payment")
    .recognition;
    assert_ne!(current.id, old_proof);
    assert!(current.through_observation > old_through);
    f.time.0.store(1499, Ordering::SeqCst);
    let premature = propose(
        &pool,
        &token,
        f.time.as_ref(),
        f.ids.as_ref(),
        request(
            order,
            &reservation,
            &current.id,
            current.through_observation,
            "new-current-proof",
        ),
    )
    .await;
    assert_eq!(premature, Err(ProposalError::ClockUnavailable));
    assert_eq!(counts(&pool).await, (0, 0));
    f.time.0.store(1500, Ordering::SeqCst);
    let receipt = propose(
        &pool,
        &token,
        f.time.as_ref(),
        f.ids.as_ref(),
        request(
            &f.order.identity.id,
            &reservation,
            &current.id,
            current.through_observation,
            "new-current-proof",
        ),
    )
    .await
    .expect("new current proof preserves older exact reservation");
    assert_eq!(receipt.recognition_id, current.id);
    assert_eq!(receipt.reservation_id, reservation);
    let stored: (serde_json::Value, serde_json::Value, String) = sqlx::query_as("SELECT p.allocation,r.allocation,r.recognition_id FROM refund_execution_proposals p JOIN refund_reservations r ON r.id=p.reservation_id WHERE p.id=$1")
        .bind(&receipt.id).fetch_one(&pool).await.expect("both immutable source allocations");
    assert_eq!(stored.0, saved);
    assert_eq!(stored.1, saved);
    assert_eq!(stored.2, old_proof);
    assert_eq!(
        (
            receipt.allocation.total.gross_minor,
            receipt.allocation.total.net_minor,
            receipt.allocation.total.vat_minor
        ),
        (3101, 2562, 539)
    );
    assert_eq!(counts(&pool).await, (1, 1));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn replay_fails_closed_when_committed_reciprocal_event_is_missing(pool: PgPool) {
    let f = accepted(&pool, false, true).await;
    let (reservation, proof, through) = reservation(&pool, &f).await;
    let token = staff(&pool, "proposal-finance", 95, "refund.execution.prepare").await;
    let order = &f.order.identity.id;
    let first = propose(
        &pool,
        &token,
        f.time.as_ref(),
        f.ids.as_ref(),
        request(order, &reservation, &proof, through, "event-custody"),
    )
    .await
    .expect("genuine complete proposal");
    let replay = propose(
        &pool,
        &token,
        f.time.as_ref(),
        f.ids.as_ref(),
        request(order, &reservation, &proof, through, "event-custody"),
    )
    .await
    .expect("healthy replay positive control");
    assert!(replay.replayed);
    assert_eq!(replay.id, first.id);
    // Only the disposable sqlx::test database is corrupted, after the valid control.
    pool.execute("ALTER TABLE commerce_outbox DISABLE TRIGGER USER")
        .await
        .expect("temporarily permit simulated damaged history");
    sqlx::query("DELETE FROM commerce_outbox WHERE id=(SELECT event_id FROM refund_execution_proposals WHERE id=$1)")
        .bind(&first.id).execute(&pool).await.expect("simulate missing event");
    pool.execute("ALTER TABLE commerce_outbox ENABLE TRIGGER USER")
        .await
        .expect("restore event guards");
    assert!(
        matches!(
            propose(
                &pool,
                &token,
                f.time.as_ref(),
                f.ids.as_ref(),
                request(order, &reservation, &proof, through, "event-custody")
            )
            .await,
            Err(ProposalError::InvalidStoredData)
        ),
        "a detached receipt cannot be replayed"
    );
    assert_eq!(counts(&pool).await, (1, 0));
}
