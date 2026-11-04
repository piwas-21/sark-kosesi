//! Authorized scarcity retry uses real paid orders, stock movements and staff custody.
use super::*;
use crate::{
    late_stock::disposition::{self, DecisionOutcome, DispositionError, RetryRequest},
    staff_access::{SessionToken, StaffError},
};
use commerce_application::request_context::{Clock, ContextError};
use std::sync::atomic::AtomicUsize;

struct FailAfterAuthorization(AtomicUsize);

impl Clock for FailAfterAuthorization {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        if self.0.fetch_add(1, Ordering::SeqCst) == 0 {
            Ok(2000)
        } else {
            Err(ContextError::ClockUnavailable)
        }
    }
}

async fn scarce(pool: &PgPool) -> (Fixture, ReviewCase, String) {
    let f = accepted(pool, false, true).await;
    expire_original(pool, &f).await;
    receive(
        pool,
        "disposition-count-foam",
        "SEAT-FOAM",
        WarehouseChange::Count(2),
    )
    .await;
    let receipt = recover(pool, f.time.as_ref(), f.ids.as_ref(), &f.order.identity.id)
        .await
        .expect("actual received payment and scarce frozen demand");
    let Outcome::NeedsReview(case) = receipt.outcome else {
        panic!("scarcity case")
    };
    assert_eq!(case.reason, ReviewReason::ComponentScarcity);
    let digest: String = sqlx::query_scalar(
        "SELECT late_stock_review_digest(c) FROM late_stock_review_cases c WHERE id=$1",
    )
    .bind(&case.id)
    .fetch_one(pool)
    .await
    .expect("exact case revision");
    (f, case, digest)
}

async fn staff(pool: &PgPool, capability: &str, byte: u8) -> SessionToken {
    let actor = format!("stock-disposition-{byte}");
    sqlx::query("INSERT INTO staff_accounts VALUES($1,true)")
        .bind(&actor)
        .execute(pool)
        .await
        .expect("staff account");
    sqlx::query("INSERT INTO staff_capabilities VALUES($1,$2)")
        .bind(&actor)
        .bind(capability)
        .execute(pool)
        .await
        .expect("specific grant");
    let token = SessionToken::from_bytes([byte; 32]);
    sqlx::query("INSERT INTO staff_sessions(token_hash,staff_id,created_at,expires_at,mfa_verified_at) VALUES($1,$2,1000,4600,1900)")
        .bind(token.digest().as_slice()).bind(actor).execute(pool).await.expect("recent MFA session");
    token
}

fn request<'a>(
    case: &'a ReviewCase,
    digest: &'a str,
    key: &'a str,
    generation: i32,
) -> RetryRequest<'a> {
    RetryRequest {
        case_id: &case.id,
        case_digest: digest,
        effect_key: key,
        expected_generation: generation,
        reason: "physical stock rechecked",
        confirmed: true,
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn retry_rejects_active_original_released_after_scarcity_case(pool: PgPool) {
    let f = accepted(&pool, false, true).await;
    f.time.0.store(2000, Ordering::SeqCst);
    let foam_hold: String = sqlx::query_scalar("SELECT h.id FROM stock_holds h JOIN stock_hold_lines l ON l.hold_id=h.id WHERE h.basket_group_id=$1 AND l.component_code='SEAT-FOAM'")
        .bind(&f.order.identity.stock_group_id)
        .fetch_one(&pool)
        .await
        .expect("the original foam member");
    transition_hold(
        &pool,
        &foam_hold,
        HoldCommand::Expire {
            actor_id: "fixture-partial-expiry",
        },
        2000,
    )
    .await
    .expect("expire only the foam member");
    receive(
        &pool,
        "partial-expiry-count-foam",
        "SEAT-FOAM",
        WarehouseChange::Count(2),
    )
    .await;
    let Outcome::NeedsReview(case) =
        recover(&pool, f.time.as_ref(), f.ids.as_ref(), &f.order.identity.id)
            .await
            .expect("real scarcity with one active member")
            .outcome
    else {
        panic!("scarcity must retain a review case")
    };
    assert_eq!(case.reason, ReviewReason::ComponentScarcity);
    let active = case
        .footprint
        .original
        .iter()
        .find(|member| member.state == OriginalState::Active)
        .expect("one original member remains active");
    transition_hold(
        &pool,
        &active.member.hold_id,
        HoldCommand::Release {
            owner_id: &f.order.identity.id,
            actor_id: "fixture-later-release",
        },
        2000,
    )
    .await
    .expect("record genuine later release intent");
    let digest: String = sqlx::query_scalar(
        "SELECT late_stock_review_digest(c) FROM late_stock_review_cases c WHERE id=$1",
    )
    .bind(&case.id)
    .fetch_one(&pool)
    .await
    .expect("exact historical case digest");
    let token = staff(&pool, "stock.recover", 101).await;
    let before = all_effects(&pool).await;
    assert!(matches!(
        disposition::retry(
            &pool,
            &token,
            f.time.as_ref(),
            f.ids.as_ref(),
            request(&case, &digest, "10101010101010101010101010101010", 1),
        )
        .await,
        Err(DispositionError::Custody(LateStockError::NotApplicable))
    ));
    assert_eq!(all_effects(&pool).await, before);
    let decisions: i64 = sqlx::query_scalar("SELECT count(*) FROM late_stock_dispositions")
        .fetch_one(&pool)
        .await
        .expect("no retry decision");
    assert_eq!(decisions, 0);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn retry_clock_failure_after_authorization_rolls_back_and_allows_same_request(pool: PgPool) {
    let (f, case, digest) = scarce(&pool).await;
    let token = staff(&pool, "stock.recover", 102).await;
    let before = all_effects(&pool).await;
    let clock = FailAfterAuthorization(AtomicUsize::new(0));
    let key = "10201020102010201020102010201020";
    assert!(matches!(
        disposition::retry(
            &pool,
            &token,
            &clock,
            f.ids.as_ref(),
            request(&case, &digest, key, 1),
        )
        .await,
        Err(DispositionError::Custody(LateStockError::ClockUnavailable))
    ));
    assert_eq!(clock.0.load(Ordering::SeqCst), 2);
    assert_eq!(all_effects(&pool).await, before);
    let decisions: i64 = sqlx::query_scalar("SELECT count(*) FROM late_stock_dispositions")
        .fetch_one(&pool)
        .await
        .expect("failed clock creates no decision");
    assert_eq!(decisions, 0);
    let healthy = disposition::retry(
        &pool,
        &token,
        f.time.as_ref(),
        f.ids.as_ref(),
        request(&case, &digest, key, 1),
    )
    .await
    .expect("healthy clock permits the same confirmed request");
    assert!(matches!(healthy.outcome, DecisionOutcome::StillScarce));
    assert!(!healthy.replayed);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn retry_before_original_deadline_cannot_reassess_retained_case(pool: PgPool) {
    let (f, case, digest) = scarce(&pool).await;
    let token = staff(&pool, "stock.recover", 103).await;
    let before = all_effects(&pool).await;
    let key = "10301030103010301030103010301030";
    assert_eq!(f.order.identity.hold_expires_at, 2000);
    f.time.0.store(1999, Ordering::SeqCst);
    assert!(matches!(
        disposition::retry(
            &pool,
            &token,
            f.time.as_ref(),
            f.ids.as_ref(),
            request(&case, &digest, key, 1),
        )
        .await,
        Err(DispositionError::Custody(LateStockError::NotApplicable))
    ));
    assert_eq!(all_effects(&pool).await, before);
    let decisions: i64 = sqlx::query_scalar("SELECT count(*) FROM late_stock_dispositions")
        .fetch_one(&pool)
        .await
        .expect("no decision before the original deadline");
    assert_eq!(decisions, 0);
    f.time.0.store(2000, Ordering::SeqCst);
    let healthy = disposition::retry(
        &pool,
        &token,
        f.time.as_ref(),
        f.ids.as_ref(),
        request(&case, &digest, key, 1),
    )
    .await
    .expect("same request at the original deadline");
    assert!(matches!(healthy.outcome, DecisionOutcome::StillScarce));
    assert!(!healthy.replayed);
}

async fn wait_for_retry_lock(pool: &PgPool, pattern: &str, count: i64) {
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let actual: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE $1")
                .bind(pattern)
                .fetch_one(pool)
                .await
                .expect("observe real database lock waiters");
            if actual >= count {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("expected database lock wait");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn retry_rechecks_scarcity_after_real_concurrent_restock_and_rolls_back(pool: PgPool) {
    let (f, case, digest) = scarce(&pool).await;
    let token = staff(&pool, "stock.recover", 104).await;
    let before = all_effects(&pool).await;
    let mut foam = pool.begin().await.expect("first assessment foam gate");
    foam.execute(
        "SELECT component_code FROM stock_balances WHERE component_code='SEAT-FOAM' FOR UPDATE",
    )
    .await
    .expect("hold the second database-ordered component");
    let mut gate = pool.begin().await.expect("restock phase gate");
    gate.execute("SELECT pg_advisory_xact_lock(938006)")
        .await
        .expect("hold restock after savepoint rollback");
    let p = pool.clone();
    let time = f.time.clone();
    let ids = f.ids.clone();
    let retry_case = case.clone();
    let retry_digest = digest.clone();
    let key = "10401040104010401040104010401040";
    let retry = tokio::spawn(async move {
        disposition::retry(
            &p,
            &token,
            time.as_ref(),
            ids.as_ref(),
            request(&retry_case, &retry_digest, key, 1),
        )
        .await
    });
    wait_for_retry_lock(&pool, "SELECT %FROM stock_balances%FOR UPDATE", 1).await;
    let p = pool.clone();
    let restock = tokio::spawn(async move {
        let mut tx = p.begin().await.expect("queued real restock");
        tx.execute("SELECT component_code FROM stock_balances ORDER BY component_code FOR UPDATE")
            .await
            .expect("claim both components after savepoint releases them");
        tx.execute("SELECT pg_advisory_xact_lock(938006)")
            .await
            .expect("restock phase gate");
        tx.execute("UPDATE stock_balances SET on_hand=on_hand+3 WHERE component_code='SEAT-FOAM'; INSERT INTO stock_movements(effect_key,component_code,action,quantity,actor,reason,on_hand_delta,quarantine_delta,reserved_delta,on_hand_after,quarantine_after,reserved_after) VALUES('retry-race-restock','SEAT-FOAM','receive',3,'fixture-warehouse','synthetic physical custody',3,0,0,5,1,0)")
            .await
            .expect("actual restock and matching physical ledger");
        tx.commit().await.expect("commit external restock");
    });
    wait_for_retry_lock(&pool, "SELECT %FROM stock_balances%FOR UPDATE", 2).await;
    foam.commit()
        .await
        .expect("first assessment remains scarce");
    wait_for_retry_lock(&pool, "SELECT pg_advisory_xact_lock(938006)", 1).await;
    wait_for_retry_lock(&pool, "SELECT %FROM stock_balances%FOR UPDATE", 1).await;
    gate.commit().await.expect("restock wins second assessment");
    restock.await.expect("external restock task");
    assert!(matches!(
        retry.await.expect("retry task"),
        Err(DispositionError::Custody(LateStockError::StateChanged))
    ));
    let mut after = all_effects(&pool).await;
    let mut expected = before.clone();
    let before_moves = expected["moves"].as_array().expect("initial movements");
    let after_moves = after["moves"].as_array().expect("movements after restock");
    assert_eq!(after_moves.len(), before_moves.len() + 1);
    assert_eq!(&after_moves[..before_moves.len()], before_moves);
    assert_eq!(
        after_moves[before_moves.len()]["effect_key"],
        "retry-race-restock"
    );
    expected
        .as_object_mut()
        .expect("effects object")
        .remove("moves");
    expected
        .as_object_mut()
        .expect("effects object")
        .remove("balances");
    after
        .as_object_mut()
        .expect("effects object")
        .remove("moves");
    after
        .as_object_mut()
        .expect("effects object")
        .remove("balances");
    assert_eq!(after, expected, "retry leaves no durable custody effects");
    assert_eq!(
        balances(&pool).await,
        vec![
            ("SEAT-COVER".into(), 10, 1, 0),
            ("SEAT-FOAM".into(), 5, 1, 0)
        ]
    );
    let decisions: i64 = sqlx::query_scalar("SELECT count(*) FROM late_stock_dispositions")
        .fetch_one(&pool)
        .await
        .expect("changed assessment creates no decision");
    assert_eq!(decisions, 0);
    let healthy = disposition::retry(
        &pool,
        &SessionToken::from_bytes([104; 32]),
        f.time.as_ref(),
        f.ids.as_ref(),
        request(&case, &digest, key, 1),
    )
    .await
    .expect("same confirmed request can allocate after restock");
    assert!(matches!(healthy.outcome, DecisionOutcome::Allocated { .. }));
    assert!(!healthy.replayed);
    assert_eq!(allocation_counts(&pool).await, (1, 1, 1, 0, 2, 4, 3, 3, 6));
}

#[sqlx::test(migrations = false)]
async fn legacy_scarcity_custody_survives_0037_and_allows_explicit_retry(pool: PgPool) {
    MIGRATOR
        .run_to(36, &pool)
        .await
        .expect("legacy schema through 0036");
    let f = accepted(&pool, false, true).await;
    expire_original(&pool, &f).await;
    receive(
        &pool,
        "upgrade-count-foam",
        "SEAT-FOAM",
        WarehouseChange::Count(2),
    )
    .await;

    // Use the old native admission and completion guards over a genuine paid
    // order, real expired holds and actual scarce balances; commit before upgrade.
    let case_id = "a".repeat(32);
    let mut tx = pool.begin().await.expect("legacy custody transaction");
    sqlx::query("INSERT INTO late_stock_review_cases(id,order_id,attempt_id,recognition_id,group_id,actor,reason,assessed_at,footprint) SELECT $1,o.id,r.attempt_id,p.recognition_id,o.stock_group_id,'late-stock-review','component_scarcity',2000,jsonb_build_object('schema_version',1,'original',late_stock_original(o.id),'components',jsonb_build_array(jsonb_build_object('component_code','SEAT-COVER','demand',4,'active',0,'before',jsonb_build_object('on_hand',10,'quarantine',1,'reserved',0)),jsonb_build_object('component_code','SEAT-FOAM','demand',2,'active',0,'before',jsonb_build_object('on_hand',2,'quarantine',1,'reserved',0)))) FROM checkout_orders o JOIN payment_recognition_current p ON p.order_id=o.id JOIN payment_recognitions r ON r.id=p.recognition_id WHERE o.id=$2")
        .bind(&case_id).bind(&f.order.identity.id).execute(&mut *tx).await.expect("legacy native guards admit exact scarcity");
    sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) SELECT $1,c.order_id,'order.stock_recovery_review.v1',jsonb_build_object('schema_version',1,'order_id',c.order_id,'case_id',c.id,'case_sha256',late_stock_review_digest(c)),c.assessed_at FROM late_stock_review_cases c WHERE c.id=$2")
        .bind("b".repeat(32)).bind(&case_id).execute(&mut *tx).await.expect("legacy digest-bound event");
    tx.commit()
        .await
        .expect("legacy completion constraints validate and commit");
    let before: (Value, String, Value) = sqlx::query_as("SELECT to_jsonb(c),late_stock_review_digest(c),(SELECT to_jsonb(e) FROM commerce_outbox e WHERE e.aggregate_id=c.order_id AND e.kind='order.stock_recovery_review.v1') FROM late_stock_review_cases c WHERE c.id=$1")
        .bind(&case_id).fetch_one(&pool).await.expect("committed legacy custody");
    let effects_before = all_effects(&pool).await;
    MIGRATOR
        .run(&pool)
        .await
        .expect("apply actual 0037 migration");
    let after: (Value, String, Value, Option<String>) = sqlx::query_as("SELECT to_jsonb(c)-'admission_transaction_id',late_stock_review_digest(c),(SELECT to_jsonb(e) FROM commerce_outbox e WHERE e.aggregate_id=c.order_id AND e.kind='order.stock_recovery_review.v1'),c.admission_transaction_id::text FROM late_stock_review_cases c WHERE c.id=$1")
        .bind(&case_id).fetch_one(&pool).await.expect("upgraded legacy custody");
    assert_eq!(
        (after.0, &after.1, after.2),
        (before.0, &before.1, before.2)
    );
    assert_eq!(
        after.3, None,
        "legacy case has no admission transaction tag"
    );
    // The added nullable column is the only schema-induced row representation change.
    let mut effects_after = all_effects(&pool).await;
    for case in effects_after["cases"].as_array_mut().expect("one case") {
        case.as_object_mut()
            .expect("case object")
            .remove("admission_transaction_id");
    }
    assert_eq!(
        effects_after, effects_before,
        "migration preserves physical and journal history"
    );
    let Outcome::NeedsReview(case) = read_recorded(&pool, &f.order.identity.id)
        .await
        .expect("upgraded reader validates legacy custody")
        .expect("legacy review")
    else {
        panic!("legacy case must remain open")
    };
    assert_eq!(case.id, case_id);
    assert_eq!(case.reason, ReviewReason::ComponentScarcity);
    let token = staff(&pool, "stock.recover", 98).await;
    let first = disposition::retry(
        &pool,
        &token,
        f.time.as_ref(),
        f.ids.as_ref(),
        request(&case, &before.1, "11111111111111111111111111111111", 1),
    )
    .await
    .expect("authorized post-upgrade scarcity retry");
    assert!(matches!(first.outcome, DecisionOutcome::StillScarce));
    receive(
        &pool,
        "upgrade-restock-foam",
        "SEAT-FOAM",
        WarehouseChange::Receive(20),
    )
    .await;
    let success = disposition::retry(
        &pool,
        &token,
        f.time.as_ref(),
        f.ids.as_ref(),
        request(&case, &before.1, "22222222222222222222222222222222", 2),
    )
    .await
    .expect("authorized post-upgrade allocation");
    assert!(matches!(success.outcome, DecisionOutcome::Allocated { .. }));
    assert!(matches!(
        read_recorded(&pool, &f.order.identity.id)
            .await
            .expect("upgraded complete history"),
        Some(Outcome::Late(_))
    ));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn authorized_scarcity_retry_preserves_case_and_replays_each_decision_without_repeated_effects(
    pool: PgPool,
) {
    let (f, case, digest) = scarce(&pool).await;
    let token = staff(&pool, "stock.recover", 91).await;
    let case_before: Value =
        sqlx::query_scalar("SELECT to_jsonb(c) FROM late_stock_review_cases c WHERE id=$1")
            .bind(&case.id)
            .fetch_one(&pool)
            .await
            .expect("original immutable case");
    let event_before: Value = sqlx::query_scalar("SELECT to_jsonb(e) FROM commerce_outbox e WHERE aggregate_id=$1 AND kind='order.stock_recovery_review.v1'").bind(&case.order_id).fetch_one(&pool).await.expect("original immutable review event");
    let physical_before = balances(&pool).await;
    let counts_before = allocation_counts(&pool).await;
    let key1 = "11111111111111111111111111111111";
    let first = disposition::retry(
        &pool,
        &token,
        f.time.as_ref(),
        f.ids.as_ref(),
        request(&case, &digest, key1, 1),
    )
    .await
    .expect("authorized genuine scarcity assessment");
    assert!(!first.replayed);
    assert!(matches!(first.outcome, DecisionOutcome::StillScarce));
    assert_eq!(first.generation, 1);
    assert_eq!(balances(&pool).await, physical_before);
    assert_eq!(allocation_counts(&pool).await, counts_before);
    let after_first = all_effects(&pool).await;
    let replay = disposition::retry(
        &pool,
        &token,
        f.time.as_ref(),
        f.ids.as_ref(),
        request(&case, &digest, key1, 1),
    )
    .await
    .expect("idempotent scarce replay");
    assert!(replay.replayed);
    assert_eq!(replay.id, first.id);
    assert_eq!(all_effects(&pool).await, after_first);

    receive(
        &pool,
        "disposition-restock-foam",
        "SEAT-FOAM",
        WarehouseChange::Receive(20),
    )
    .await;
    let key2 = "22222222222222222222222222222222";
    let success = disposition::retry(
        &pool,
        &token,
        f.time.as_ref(),
        f.ids.as_ref(),
        request(&case, &digest, key2, 2),
    )
    .await
    .expect("fresh explicit staff approval after restock");
    assert!(!success.replayed);
    let DecisionOutcome::Allocated { allocation_id } = &success.outcome else {
        panic!("restocked exact demand allocated")
    };
    assert_eq!(success.generation, 2);
    assert_eq!(
        balances(&pool).await,
        vec![
            ("SEAT-COVER".into(), 6, 1, 0),
            ("SEAT-FOAM".into(), 20, 1, 0)
        ]
    );
    assert_eq!(allocation_counts(&pool).await, (1, 1, 1, 0, 2, 4, 3, 3, 6));
    let linked: bool = sqlx::query_scalar("SELECT d.case_id=$1 AND d.case_digest=$2 AND d.allocation_id=$3 AND a.order_id=d.order_id FROM late_stock_dispositions d JOIN late_stock_allocations a ON a.id=d.allocation_id WHERE d.id=$4")
        .bind(&case.id).bind(&digest).bind(allocation_id).bind(&success.id).fetch_one(&pool).await.expect("exact success link");
    assert!(linked);
    let decision_events: i64 = sqlx::query_scalar("SELECT count(*) FROM late_stock_dispositions d JOIN commerce_outbox e ON e.aggregate_id=d.order_id AND e.kind='order.stock_recovery_decided.v1' AND e.payload->>'decision_id'=d.id AND e.payload->>'decision_sha256'=late_stock_disposition_digest(d) WHERE d.case_id=$1").bind(&case.id).fetch_one(&pool).await.expect("digest bound decision events");
    assert_eq!(decision_events, 2);
    assert_eq!(
        sqlx::query_scalar::<_, Value>(
            "SELECT to_jsonb(c) FROM late_stock_review_cases c WHERE id=$1"
        )
        .bind(&case.id)
        .fetch_one(&pool)
        .await
        .expect("case"),
        case_before
    );
    assert_eq!(sqlx::query_scalar::<_,Value>("SELECT to_jsonb(e) FROM commerce_outbox e WHERE aggregate_id=$1 AND kind='order.stock_recovery_review.v1'").bind(&case.order_id).fetch_one(&pool).await.expect("review event"), event_before);
    let after_success = all_effects(&pool).await;
    let old = disposition::retry(
        &pool,
        &token,
        f.time.as_ref(),
        f.ids.as_ref(),
        request(&case, &digest, key1, 1),
    )
    .await
    .expect("historical decision after success");
    let current = disposition::retry(
        &pool,
        &token,
        f.time.as_ref(),
        f.ids.as_ref(),
        request(&case, &digest, key2, 2),
    )
    .await
    .expect("historical success");
    assert!(old.replayed && current.replayed);
    assert_eq!(old.id, first.id);
    assert_eq!(current.id, success.id);
    assert!(matches!(
        disposition::retry(
            &pool,
            &token,
            f.time.as_ref(),
            f.ids.as_ref(),
            request(&case, &digest, "33333333333333333333333333333333", 3),
        )
        .await,
        Err(DispositionError::Conflict)
    ));
    assert_eq!(all_effects(&pool).await, after_success);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn scarcity_retry_requires_stock_authority_confirmation_exact_case_and_bounded_generations(
    pool: PgPool,
) {
    let (f, case, digest) = scarce(&pool).await;
    let denied = staff(&pool, "stock.review", 92).await;
    let token = staff(&pool, "stock.recover", 93).await;
    let key = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let before = all_effects(&pool).await;
    assert!(matches!(
        disposition::retry(
            &pool,
            &denied,
            f.time.as_ref(),
            f.ids.as_ref(),
            request(&case, &digest, key, 1)
        )
        .await,
        Err(DispositionError::Staff(StaffError::Denied))
    ));
    let mut unconfirmed = request(&case, &digest, key, 1);
    unconfirmed.confirmed = false;
    assert!(matches!(
        disposition::retry(&pool, &token, f.time.as_ref(), f.ids.as_ref(), unconfirmed).await,
        Err(DispositionError::InvalidRequest)
    ));
    let wrong_digest = "f".repeat(64);
    assert!(
        disposition::retry(
            &pool,
            &token,
            f.time.as_ref(),
            f.ids.as_ref(),
            request(&case, &wrong_digest, key, 1)
        )
        .await
        .is_err()
    );
    assert_eq!(all_effects(&pool).await, before);
    for (generation, key) in [
        (1, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
        (2, "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
        (3, "cccccccccccccccccccccccccccccccc"),
    ] {
        let decision = disposition::retry(
            &pool,
            &token,
            f.time.as_ref(),
            f.ids.as_ref(),
            request(&case, &digest, key, generation),
        )
        .await
        .expect("one independently confirmed bounded decision");
        assert!(matches!(decision.outcome, DecisionOutcome::StillScarce));
        assert_eq!(decision.generation, generation);
    }
    let after_cap = all_effects(&pool).await;
    assert!(
        disposition::retry(
            &pool,
            &token,
            f.time.as_ref(),
            f.ids.as_ref(),
            request(&case, &digest, "dddddddddddddddddddddddddddddddd", 4)
        )
        .await
        .is_err()
    );
    assert!(
        disposition::retry(
            &pool,
            &token,
            f.time.as_ref(),
            f.ids.as_ref(),
            request(&case, &digest, key, 2)
        )
        .await
        .is_err()
    );
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM late_stock_dispositions WHERE case_id=$1")
            .bind(&case.id)
            .fetch_one(&pool)
            .await
            .expect("bounded history");
    assert_eq!(count, 3);
    assert_eq!(all_effects(&pool).await, after_cap);
    assert_eq!(allocation_counts(&pool).await, (0, 0, 1, 0, 1, 2, 0, 3, 3));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn scarcity_retry_rechecks_staff_mfa_after_waiting_for_original_custody(pool: PgPool) {
    let (f, case, digest) = scarce(&pool).await;
    let token = staff(&pool, "stock.recover", 94).await;
    let before = all_effects(&pool).await;
    let mut blocker = pool.begin().await.expect("custody blocker");
    sqlx::query("SELECT id FROM basket_hold_groups WHERE id=$1 FOR UPDATE")
        .bind(&f.order.identity.stock_group_id)
        .fetch_one(&mut *blocker)
        .await
        .expect("claim original group");
    let worker_pool = pool.clone();
    let time = f.time.clone();
    let ids = f.ids.clone();
    let worker_case = case.clone();
    let worker_digest = digest.clone();
    let task = tokio::spawn(async move {
        disposition::retry(
            &worker_pool,
            &token,
            time.as_ref(),
            ids.as_ref(),
            request(
                &worker_case,
                &worker_digest,
                "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
                1,
            ),
        )
        .await
    });
    tokio::time::timeout(std::time::Duration::from_millis(800), async {
        loop {
            let waits:i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'SELECT %FROM basket_hold_groups%FOR SHARE'").fetch_one(&pool).await.expect("observe actual original custody wait");
            if waits > 0 { break; }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }).await.expect("retry reaches original custody proof");
    f.time.0.store(2801, Ordering::SeqCst);
    blocker.commit().await.expect("release original custody");
    assert!(matches!(
        task.await.expect("worker"),
        Err(DispositionError::Staff(StaffError::Denied))
    ));
    assert_eq!(all_effects(&pool).await, before);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM late_stock_dispositions")
        .fetch_one(&pool)
        .await
        .expect("no decision after MFA expiration");
    assert_eq!(count, 0);
    f.time.0.store(2000, Ordering::SeqCst);
    let healthy = disposition::retry(
        &pool,
        &SessionToken::from_bytes([94; 32]),
        f.time.as_ref(),
        f.ids.as_ref(),
        request(&case, &digest, "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee", 1),
    )
    .await
    .expect("same request with valid MFA");
    assert!(matches!(healthy.outcome, DecisionOutcome::StillScarce));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn stock_recovery_does_not_authorize_retry_of_release_intent_case(pool: PgPool) {
    let f = accepted(&pool, true, true).await;
    let hold = &original_holds(&pool, &f).await[0];
    transition_hold(
        &pool,
        hold,
        HoldCommand::Release {
            owner_id: &f.order.identity.id,
            actor_id: "fixture-release",
        },
        1500,
    )
    .await
    .expect("real release intent");
    f.time.0.store(2000, Ordering::SeqCst);
    let Outcome::NeedsReview(case) =
        recover(&pool, f.time.as_ref(), f.ids.as_ref(), &f.order.identity.id)
            .await
            .expect("release stop")
            .outcome
    else {
        panic!("release must stop")
    };
    assert_eq!(case.reason, ReviewReason::ReleaseIntentUnresolved);
    let digest: String = sqlx::query_scalar(
        "SELECT late_stock_review_digest(c) FROM late_stock_review_cases c WHERE id=$1",
    )
    .bind(&case.id)
    .fetch_one(&pool)
    .await
    .expect("case digest");
    let token = staff(&pool, "stock.recover", 95).await;
    let before = all_effects(&pool).await;
    assert!(
        disposition::retry(
            &pool,
            &token,
            f.time.as_ref(),
            f.ids.as_ref(),
            request(&case, &digest, "ffffffffffffffffffffffffffffffff", 1)
        )
        .await
        .is_err()
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM late_stock_dispositions")
        .fetch_one(&pool)
        .await
        .expect("no unauthorized disposition");
    assert_eq!(count, 0);
    assert_eq!(all_effects(&pool).await, before);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn native_disposition_rejects_other_order_and_detached_success_with_exact_failure_and_healthy_control(
    pool: PgPool,
) {
    seed(&pool).await;
    let (store, time) = crate::payment_test_fixture::quote::store(&pool, 1100);
    let f = accepted_with_store(&pool, store.clone(), time.clone(), false, true).await;
    expire_original(&pool, &f).await;
    receive(
        &pool,
        "native-count-foam",
        "SEAT-FOAM",
        WarehouseChange::Count(2),
    )
    .await;
    let Outcome::NeedsReview(case) =
        recover(&pool, f.time.as_ref(), f.ids.as_ref(), &f.order.identity.id)
            .await
            .expect("genuine scarce case")
            .outcome
    else {
        panic!("scarcity")
    };
    let digest: String = sqlx::query_scalar(
        "SELECT late_stock_review_digest(c) FROM late_stock_review_cases c WHERE id=$1",
    )
    .bind(&case.id)
    .fetch_one(&pool)
    .await
    .expect("case digest");
    let token = staff(&pool, "stock.recover", 96).await;
    time.0.store(1100, Ordering::SeqCst);
    let other = accepted_with_store(&pool, store, time.clone(), true, false).await;
    observe(&pool, &other, "tr_DISPOSITIONOTHER", "paid", "paid").await;
    recognize(
        &pool,
        time.as_ref(),
        other.ids.as_ref(),
        &other.frozen.snapshot().binding.id,
    )
    .await
    .expect("other genuine paid recognition");
    time.0.store(2000, Ordering::SeqCst);
    let other_recognition: String = sqlx::query_scalar(
        "SELECT recognition_id FROM payment_recognition_current WHERE order_id=$1",
    )
    .bind(&other.order.identity.id)
    .fetch_one(&pool)
    .await
    .expect("independent real received order");
    let before = all_effects(&pool).await;
    let mut cross = pool.begin().await.expect("cross-order hostile decision");
    let err = sqlx::query("INSERT INTO late_stock_dispositions(id,case_id,case_digest,order_id,attempt_id,recognition_id,actor_id,effect_key,generation,reason,recorded_at,outcome,allocation_id,footprint,transaction_id) SELECT $1,c.id,$2,$3,$4,$5,'stock-disposition-96',$6,1,'physical stock rechecked',2000,'allocated',$7,NULL,pg_current_xact_id() FROM late_stock_review_cases c WHERE c.id=$8")
        .bind("a".repeat(32)).bind(&digest).bind(&other.order.identity.id).bind(&other.frozen.snapshot().binding.id).bind(&other_recognition).bind("b".repeat(32)).bind("c".repeat(32)).bind(&case.id).execute(&mut *cross).await.expect_err("another real order cannot use this case");
    let database = err.as_database_error().expect("native guard error");
    assert_eq!(database.code().as_deref(), Some("23514"));
    assert!(
        database.message().contains("exact scarcity case"),
        "{database}"
    );
    cross.rollback().await.expect("rollback hostile order");

    let mut detached = pool.begin().await.expect("detached success");
    sqlx::query("INSERT INTO late_stock_dispositions(id,case_id,case_digest,order_id,attempt_id,recognition_id,actor_id,effect_key,generation,reason,recorded_at,outcome,allocation_id,footprint,transaction_id) SELECT $1,c.id,$2,c.order_id,c.attempt_id,c.recognition_id,'stock-disposition-96',$3,1,'physical stock rechecked',2000,'allocated',$4,NULL,pg_current_xact_id() FROM late_stock_review_cases c WHERE c.id=$5")
        .bind("a".repeat(32)).bind(&digest).bind("b".repeat(32)).bind("c".repeat(32)).bind(&case.id).execute(&mut *detached).await.expect("preclaim is allowed pending full proof");
    sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) SELECT $1,d.order_id,'order.stock_recovery_decided.v1',jsonb_build_object('schema_version',1,'order_id',d.order_id,'decision_id',d.id,'decision_sha256',late_stock_disposition_digest(d)),d.recorded_at FROM late_stock_dispositions d WHERE d.id=$2")
        .bind("d".repeat(32)).bind("a".repeat(32)).execute(&mut *detached).await.expect("correct decision event eliminates missing-event masking");
    let err = detached
        .execute("SET CONSTRAINTS require_complete_late_stock_disposition IMMEDIATE")
        .await
        .expect_err(
            "allocated decision cannot commit without actual allocation and reciprocal effects",
        );
    let database = err.as_database_error().expect("deferred custody error");
    assert_eq!(database.code().as_deref(), Some("23514"));
    assert!(
        database.message().contains("complete reciprocal decision"),
        "{database}"
    );
    detached
        .rollback()
        .await
        .expect("rollback detached success");
    assert_eq!(all_effects(&pool).await, before);
    let healthy = disposition::retry(
        &pool,
        &token,
        f.time.as_ref(),
        f.ids.as_ref(),
        request(&case, &digest, "99999999999999999999999999999999", 1),
    )
    .await
    .expect("same real case accepted when still scarce");
    assert!(matches!(healthy.outcome, DecisionOutcome::StillScarce));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn suppressed_decision_or_event_never_reports_success_and_rolls_back(pool: PgPool) {
    let (f, case, digest) = scarce(&pool).await;
    let token = staff(&pool, "stock.recover", 97).await;
    let before = all_effects(&pool).await;
    pool.execute("CREATE FUNCTION fixture_suppress_decision() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RETURN NULL; END $$; CREATE TRIGGER fixture_suppress_decision BEFORE INSERT ON late_stock_dispositions FOR EACH ROW EXECUTE FUNCTION fixture_suppress_decision()").await.expect("suppress actual decision insertion");
    assert!(
        disposition::retry(
            &pool,
            &token,
            f.time.as_ref(),
            f.ids.as_ref(),
            request(&case, &digest, "77777777777777777777777777777777", 1)
        )
        .await
        .is_err()
    );
    assert_eq!(all_effects(&pool).await, before);
    pool.execute("DROP TRIGGER fixture_suppress_decision ON late_stock_dispositions; DROP FUNCTION fixture_suppress_decision(); CREATE FUNCTION fixture_suppress_event() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.kind='order.stock_recovery_decided.v1' THEN RETURN NULL; END IF; RETURN NEW; END $$; CREATE TRIGGER fixture_suppress_event BEFORE INSERT ON commerce_outbox FOR EACH ROW EXECUTE FUNCTION fixture_suppress_event()").await.expect("suppress only decision event");
    assert!(
        disposition::retry(
            &pool,
            &token,
            f.time.as_ref(),
            f.ids.as_ref(),
            request(&case, &digest, "77777777777777777777777777777777", 1)
        )
        .await
        .is_err()
    );
    assert_eq!(all_effects(&pool).await, before);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM late_stock_dispositions")
        .fetch_one(&pool)
        .await
        .expect("suppressed event rolled back decision");
    assert_eq!(count, 0);
    pool.execute("DROP TRIGGER fixture_suppress_event ON commerce_outbox; DROP FUNCTION fixture_suppress_event()").await.expect("restore normal custody");
    let result = disposition::retry(
        &pool,
        &token,
        f.time.as_ref(),
        f.ids.as_ref(),
        request(&case, &digest, "77777777777777777777777777777777", 1),
    )
    .await
    .expect("exact same request succeeds after repair");
    assert!(matches!(result.outcome, DecisionOutcome::StillScarce));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn authorized_retry_handles_maximum_twenty_by_sixty_four_distinct_physical_codes(
    pool: PgPool,
) {
    use commerce_domain::ComponentKind;
    use fixture::bounds::{accepted_lines, component, market};
    seed(&pool).await;
    let mut skus = Vec::new();
    let mut shortage_code = String::new();
    for member in 0..20 {
        let prefix = format!("RETRY-UNION-SET-{member:02}-");
        let sku = format!("{prefix}{}", "X".repeat(65 - prefix.len()));
        let mut components = Vec::new();
        for line in 0..64 {
            let kind = if line < 32 {
                ComponentKind::Cover
            } else {
                ComponentKind::Foam
            };
            let label = if kind == ComponentKind::Cover {
                "COVER"
            } else {
                "FOAM"
            };
            let prefix = format!("RETRY-{label}-{member:02}-{line:02}-");
            let code = format!("{prefix}{}", "X".repeat(65 - prefix.len()));
            component(&pool, &code, kind, 20000).await;
            if member == 0 && line == 0 {
                shortage_code = code.clone();
            }
            components.push((code, 10000));
        }
        fixture::additional_variant(&pool, &sku, true, &components).await;
        skus.push(sku);
    }
    market(&pool, &skus).await;
    let f = accepted_lines(&pool, &skus).await;
    expire_original(&pool, &f).await;
    receive(
        &pool,
        "max-retry-count",
        &shortage_code,
        WarehouseChange::Count(9999),
    )
    .await;
    let Outcome::NeedsReview(case) =
        recover(&pool, f.time.as_ref(), f.ids.as_ref(), &f.order.identity.id)
            .await
            .expect("maximum real source scarcity")
            .outcome
    else {
        panic!("one scarce component must stop entire group")
    };
    let digest: String = sqlx::query_scalar(
        "SELECT late_stock_review_digest(c) FROM late_stock_review_cases c WHERE id=$1",
    )
    .bind(&case.id)
    .fetch_one(&pool)
    .await
    .expect("maximum case digest");
    let token = staff(&pool, "stock.recover", 98).await;
    let first = disposition::retry(
        &pool,
        &token,
        f.time.as_ref(),
        f.ids.as_ref(),
        request(&case, &digest, "88888888888888888888888888888881", 1),
    )
    .await
    .expect("full 1280-code still-scarce proof within existing budgets");
    assert!(matches!(first.outcome, DecisionOutcome::StillScarce));
    assert_eq!(
        allocation_counts(&pool).await,
        (0, 0, 1, 0, 1, 20, 0, 1280, 1280)
    );
    let second = disposition::retry(
        &pool,
        &token,
        f.time.as_ref(),
        f.ids.as_ref(),
        request(&case, &digest, "88888888888888888888888888888882", 2),
    )
    .await
    .expect("second full 1280-code still-scarce proof within existing budgets");
    assert!(matches!(second.outcome, DecisionOutcome::StillScarce));
    assert_eq!(
        allocation_counts(&pool).await,
        (0, 0, 1, 0, 1, 20, 0, 1280, 1280)
    );
    receive(
        &pool,
        "max-retry-restock",
        &shortage_code,
        WarehouseChange::Receive(1),
    )
    .await;
    let result = disposition::retry(
        &pool,
        &token,
        f.time.as_ref(),
        f.ids.as_ref(),
        request(&case, &digest, "88888888888888888888888888888883", 3),
    )
    .await
    .expect("full 1280-code successful authorization within existing budgets");
    assert!(matches!(result.outcome, DecisionOutcome::Allocated { .. }));
    assert_eq!(
        allocation_counts(&pool).await,
        (1, 1, 1, 0, 2, 40, 1280, 1280, 2560)
    );
    let (total, ordinary, scarce): (i64, i64, i64) = sqlx::query_as("SELECT count(*),count(*) FILTER (WHERE on_hand=10000 AND quarantine=0 AND reserved=0),count(*) FILTER (WHERE component_code=$1 AND on_hand=0 AND quarantine=0 AND reserved=0) FROM stock_balances WHERE component_code LIKE 'RETRY-%'").bind(&shortage_code).fetch_one(&pool).await.expect("all distinct actual final balances");
    assert_eq!((total, ordinary, scarce), (1280, 1279, 1));
    let envelope: i32 = sqlx::query_scalar(
        "SELECT octet_length(footprint::text) FROM late_stock_allocations WHERE order_id=$1",
    )
    .bind(&case.order_id)
    .fetch_one(&pool)
    .await
    .expect("complete allocation envelope");
    assert!(envelope < 2_097_152);
    for (key, generation, id) in [
        ("88888888888888888888888888888881", 1, &first.id),
        ("88888888888888888888888888888882", 2, &second.id),
        ("88888888888888888888888888888883", 3, &result.id),
    ] {
        let replay = disposition::retry(
            &pool,
            &token,
            f.time.as_ref(),
            f.ids.as_ref(),
            request(&case, &digest, key, generation),
        )
        .await
        .expect("maximum-size full historical prefix replay remains within existing budgets");
        assert!(replay.replayed);
        assert_eq!(&replay.id, id);
    }
    assert_eq!(
        allocation_counts(&pool).await,
        (1, 1, 1, 0, 2, 40, 1280, 1280, 2560)
    );
    let (decisions, events): (i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM late_stock_dispositions WHERE case_id=$1),(SELECT count(*) FROM commerce_outbox WHERE aggregate_id=$2 AND kind='order.stock_recovery_decided.v1')").bind(&case.id).bind(&case.order_id).fetch_one(&pool).await.expect("replay cannot append decisions or events");
    assert_eq!((decisions, events), (3, 3));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn simultaneous_same_key_retries_serialize_to_one_decision_and_one_replay(pool: PgPool) {
    let (f, case, digest) = scarce(&pool).await;
    let _token = staff(&pool, "stock.recover", 99).await;
    let before = allocation_counts(&pool).await;
    let mut blocker = pool.begin().await.expect("hold the exact effect-key lock");
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind("late-stock-disposition:66666666666666666666666666666666")
        .execute(&mut *blocker)
        .await
        .expect("serialize real callers at key");
    let mut tasks = Vec::new();
    for _ in 0..2 {
        let p = pool.clone();
        let time = f.time.clone();
        let ids = f.ids.clone();
        let c = case.clone();
        let d = digest.clone();
        tasks.push(tokio::spawn(async move {
            disposition::retry(
                &p,
                &SessionToken::from_bytes([99; 32]),
                time.as_ref(),
                ids.as_ref(),
                request(&c, &d, "66666666666666666666666666666666", 1),
            )
            .await
        }));
    }
    tokio::time::timeout(std::time::Duration::from_millis(800), async {
        loop {
            let waiters: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'SELECT pg_advisory_xact_lock(hashtextextended%'").fetch_one(&pool).await.expect("observe both actual key waiters");
            if waiters >= 2 { break; }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }).await.expect("both requests wait concurrently on the same key");
    blocker.commit().await.expect("release concurrent retries");
    let first = tasks
        .remove(0)
        .await
        .expect("first task")
        .expect("one canonical decision");
    let second = tasks
        .remove(0)
        .await
        .expect("second task")
        .expect("exact replay");
    assert_eq!(first.id, second.id);
    assert_ne!(first.replayed, second.replayed);
    assert!(matches!(first.outcome, DecisionOutcome::StillScarce));
    assert!(matches!(second.outcome, DecisionOutcome::StillScarce));
    assert_eq!(allocation_counts(&pool).await, before);
    let (decisions, events): (i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM late_stock_dispositions WHERE case_id=$1),(SELECT count(*) FROM commerce_outbox WHERE aggregate_id=$2 AND kind='order.stock_recovery_decided.v1')").bind(&case.id).bind(&case.order_id).fetch_one(&pool).await.expect("one decision and one event");
    assert_eq!((decisions, events), (1, 1));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn open_case_reader_waiting_on_attempt_observes_concurrent_committed_allocation(
    pool: PgPool,
) {
    let (f, case, digest) = scarce(&pool).await;
    let token = staff(&pool, "stock.recover", 100).await;
    receive(
        &pool,
        "reader-race-restock",
        "SEAT-FOAM",
        WarehouseChange::Receive(20),
    )
    .await;
    let mut blocker = pool.begin().await.expect("original group barrier");
    sqlx::query("SELECT id FROM basket_hold_groups WHERE id=$1 FOR UPDATE")
        .bind(&f.order.identity.stock_group_id)
        .execute(&mut *blocker)
        .await
        .expect("block actual source proof");
    let p = pool.clone();
    let time = f.time.clone();
    let ids = f.ids.clone();
    let writer_case = case.clone();
    let writer_digest = digest.clone();
    let writer = tokio::spawn(async move {
        disposition::retry(
            &p,
            &token,
            time.as_ref(),
            ids.as_ref(),
            request(
                &writer_case,
                &writer_digest,
                "55555555555555555555555555555555",
                1,
            ),
        )
        .await
    });
    tokio::time::timeout(std::time::Duration::from_millis(800), async {
        loop {
            let waits:i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'SELECT %FROM basket_hold_groups%FOR SHARE'").fetch_one(&pool).await.expect("observe writer source wait");
            if waits > 0 { break; }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }).await.expect("writer owns attempt and waits on original group");
    let p = pool.clone();
    let order = case.order_id.clone();
    let reader = tokio::spawn(async move { read_recorded(&p, &order).await });
    tokio::time::timeout(std::time::Duration::from_millis(800), async {
        loop {
            let waits:i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'SELECT id FROM payment_attempts WHERE id=%FOR UPDATE'").fetch_one(&pool).await.expect("observe reader attempt wait");
            if waits > 0 { break; }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }).await.expect("reader captured open review and waits behind writer");
    blocker
        .commit()
        .await
        .expect("allow writer to commit allocation");
    let decision = writer
        .await
        .expect("writer task")
        .expect("successful staff disposition");
    let DecisionOutcome::Allocated { allocation_id } = decision.outcome else {
        panic!("restock allocates")
    };
    let outcome = reader
        .await
        .expect("reader task")
        .expect("reader reloads after writer commit");
    let Some(Outcome::Late(allocation)) = outcome else {
        panic!("reader must observe the committed allocation")
    };
    assert_eq!(allocation.id, allocation_id);
    assert_eq!(allocation_counts(&pool).await, (1, 1, 1, 0, 2, 4, 3, 3, 6));
}
