//! Authorized scarcity retry uses real paid orders, stock movements and staff custody.
use super::*;
use crate::{
    late_stock::disposition::{self, DecisionOutcome, DispositionError, RetryRequest},
    staff_access::{SessionToken, StaffError},
};

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
