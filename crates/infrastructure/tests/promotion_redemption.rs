//! Synthetic private promotion redemption custody, using isolated PostgreSQL.
use commerce_application::Locale;
use commerce_infrastructure::{MIGRATOR, checkout_orders::*, checkout_quotes::*, guest_baskets::*};
use sqlx::{Executor, PgPool};
use std::sync::atomic::Ordering;
#[allow(dead_code)]
#[path = "support/checkout_quote_fixture.rs"]
mod fixture;
#[allow(dead_code)]
#[path = "support/checkout_order_fixture.rs"]
mod order;
#[allow(dead_code)]
#[path = "support/promotion_policy_fixture.rs"]
mod promotion;

async fn seed(pool: &PgPool, policy: &commerce_domain::promotion_policies::PromotionPolicy) {
    fixture::seed(pool).await;
    pool.execute("INSERT INTO staff_capabilities VALUES('author','promotion.manage'),('reviewer','promotion.review')").await.expect("independent promotion grants");
    promotion::draft(pool, policy, "promotion-draft")
        .await
        .expect("draft");
    promotion::decide(pool, "promotion-review", 1, 0, true)
        .await
        .expect("independent approval");
}
async fn effects(pool: &PgPool) -> (i64, i64, i64, i64, i64, i64, i64) {
    sqlx::query_as("SELECT (SELECT count(*) FROM checkout_orders),(SELECT count(*) FROM commerce_outbox),(SELECT count(*) FROM basket_hold_groups),(SELECT count(*) FROM stock_holds),(SELECT count(*) FROM stock_movements),(SELECT count(*) FROM promotion_redemptions),(SELECT COALESCE(sum(reserved),0)::bigint FROM stock_balances)")
        .fetch_one(pool).await.expect("atomic order, stock and ledger effects")
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn promoted_order_freezes_exact_allocations_and_replays_only_the_same_owner_and_intent(
    pool: PgPool,
) {
    seed(&pool, &promotion::policy()).await;
    let (store, time) = fixture::store(&pool, 1100);
    let (session, csrf) = fixture::guest(&store).await;
    let key = GuestCredential::from_session(&session).expect("owner");
    let (other_session, other_csrf) = fixture::guest(&store).await;
    let other = GuestCredential::from_session(&other_session).expect("other owner");
    let quoted = issue_promoted_quote(
        &store,
        &key,
        &csrf,
        Locale::Nl,
        &fixture::request("quote"),
        "COURTYARD",
    )
    .await
    .expect("approved quote")
    .snapshot;
    let intent = order::intent(&quoted.original.binding.quote_id);
    let placed = create_order(&store, &key, &csrf, "place", &intent, &order::seller())
        .await
        .expect("accepted promotion");
    assert!(!placed.replayed);
    assert!(placed.snapshot.promoted_quote.as_ref() == Some(&quoted));
    assert_eq!(
        (
            quoted.discount_minor,
            quoted.lines[0].discount_minor,
            quoted.lines[0].amounts.gross_minor,
            quoted.lines[0].amounts.net_minor,
            quoted.lines[0].amounts.vat_minor
        ),
        (500, 500, 36535, 30194, 6341)
    );
    assert_eq!(
        (
            quoted.total.gross_minor,
            quoted.total.net_minor,
            quoted.total.vat_minor
        ),
        (37745, 31194, 6551)
    );
    let stored: serde_json::Value =
        sqlx::query_scalar("SELECT snapshot FROM checkout_orders WHERE id=$1")
            .bind(&placed.snapshot.identity.id)
            .fetch_one(&pool)
            .await
            .expect("stored order");
    assert_eq!(stored["schema_version"], 2);
    assert_eq!(
        stored["quote"],
        serde_json::to_value(&quoted).expect("quote JSON")
    );
    let ledger: (String, String, String, i32, i32, i64) = sqlx::query_as("SELECT order_id,quote_id,promotion_id,promotion_revision,promotion_review_revision,discount_minor FROM promotion_redemptions").fetch_one(&pool).await.expect("one immutable debit");
    assert_eq!(
        ledger,
        (
            placed.snapshot.identity.id.clone(),
            quoted.original.binding.quote_id.clone(),
            "COURTYARD".into(),
            1,
            1,
            500
        )
    );
    let before = effects(&pool).await;
    assert_eq!(before, (1, 1, 1, 1, 2, 1, 12));
    let mut changed = intent.clone();
    changed.buyer.name = "Different Buyer".into();
    assert!(matches!(
        create_order(&store, &key, &csrf, "place", &changed, &order::seller()).await,
        Err(OrderStoreError::Conflict)
    ));
    assert!(
        create_order(
            &store,
            &other,
            &other_csrf,
            "place",
            &intent,
            &order::seller()
        )
        .await
        .is_err()
    );
    assert_eq!(effects(&pool).await, before);
    promotion::decide(&pool, "withdraw", 1, 1, false)
        .await
        .expect("withdraw");
    time.0.store(2500, Ordering::SeqCst);
    let replay = create_order(&store, &key, &csrf, "place", &intent, &order::seller())
        .await
        .expect("historical exact receipt");
    assert!(replay.replayed && replay.snapshot == placed.snapshot);
    assert_eq!(effects(&pool).await, before);
    for statement in [
        "UPDATE promotion_redemptions SET discount_minor=discount_minor",
        "DELETE FROM promotion_redemptions",
    ] {
        assert!(
            pool.execute(statement).await.is_err(),
            "redemption ledger is immutable"
        );
    }
}

async fn waiting(pool: &PgPool, minimum: i64) {
    tokio::time::timeout(std::time::Duration::from_millis(800), async {
        loop {
            let count: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock'").fetch_one(pool).await.expect("observe actual lock waits");
            if count >= minimum { return; }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }).await.expect("expected lock waits");
}

async fn competing(pool: PgPool, count_limit: u32, amount_limit: i64, discount: i64) {
    let mut policy = promotion::policy();
    policy.limits.total_redemptions = count_limit;
    policy.limits.total_discount_minor = amount_limit;
    policy.promotion.discount = commerce_domain::promotions::DiscountKind::Fixed {
        minor_units: discount,
    };
    seed(&pool, &policy).await;
    pool.execute("CREATE FUNCTION pause_redemption() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_advisory_xact_lock(851240);RETURN NEW;END;$$;CREATE TRIGGER pause_redemption AFTER INSERT ON promotion_redemptions FOR EACH ROW EXECUTE FUNCTION pause_redemption()").await.expect("contention instrument");
    let (store, _) = fixture::store(&pool, 1100);
    let (a, ac) = fixture::guest(&store).await;
    let (b, bc) = fixture::guest(&store).await;
    let ak = GuestCredential::from_session(&a).expect("first owner");
    let bk = GuestCredential::from_session(&b).expect("second owner");
    let aq = issue_promoted_quote(
        &store,
        &ak,
        &ac,
        Locale::Nl,
        &fixture::request("first"),
        "COURTYARD",
    )
    .await
    .expect("first quote")
    .snapshot;
    let bq = issue_promoted_quote(
        &store,
        &bk,
        &bc,
        Locale::Nl,
        &fixture::request("second"),
        "COURTYARD",
    )
    .await
    .expect("second quote")
    .snapshot;
    let ai = order::intent(&aq.original.binding.quote_id);
    let bi = order::intent(&bq.original.binding.quote_id);
    let mut gate = pool.begin().await.expect("contention gate");
    sqlx::query("SELECT pg_advisory_xact_lock(851240)")
        .execute(&mut *gate)
        .await
        .expect("hold first debit open");
    let first_store = store.clone();
    let first = tokio::spawn(async move {
        create_order(&first_store, &ak, &ac, "first-order", &ai, &order::seller()).await
    });
    waiting(&pool, 1).await;
    let second_store = store.clone();
    let second = tokio::spawn(async move {
        create_order(
            &second_store,
            &bk,
            &bc,
            "second-order",
            &bi,
            &order::seller(),
        )
        .await
    });
    waiting(&pool, 2).await;
    gate.rollback().await.expect("release contention gate");
    let first = first.await.expect("first task");
    let second = second.await.expect("second task");
    assert_eq!(
        usize::from(first.is_ok()) + usize::from(second.is_ok()),
        1,
        "one claim commits"
    );
    assert_eq!(
        effects(&pool).await,
        (1, 1, 1, 1, 2, 1, 12),
        "loser leaves no partial custody"
    );
    let debit: (i64, i64) =
        sqlx::query_as("SELECT count(*),sum(discount_minor)::bigint FROM promotion_redemptions")
            .fetch_one(&pool)
            .await
            .expect("committed debit");
    assert_eq!(debit, (1, discount));
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn competing_orders_cannot_claim_the_last_redemption_twice(pool: PgPool) {
    competing(pool, 1, 10000, 500).await;
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn competing_orders_cannot_claim_the_last_discount_budget_twice(pool: PgPool) {
    competing(pool, 20, 1, 1).await;
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn lifetime_capacity_survives_new_approved_revision(pool: PgPool) {
    let mut policy = promotion::policy();
    policy.limits.total_redemptions = 1;
    seed(&pool, &policy).await;
    let (store, _) = fixture::store(&pool, 1100);
    let (a, ac) = fixture::guest(&store).await;
    let ak = GuestCredential::from_session(&a).expect("owner");
    let q = issue_promoted_quote(
        &store,
        &ak,
        &ac,
        Locale::Nl,
        &fixture::request("first"),
        "COURTYARD",
    )
    .await
    .expect("first quote")
    .snapshot;
    let first = create_order(
        &store,
        &ak,
        &ac,
        "first",
        &order::intent(&q.original.binding.quote_id),
        &order::seller(),
    )
    .await
    .expect("consume lifetime capacity");
    let hold: String = sqlx::query_scalar("SELECT id FROM stock_holds WHERE owner_id=$1")
        .bind(&first.snapshot.identity.id)
        .fetch_one(&pool)
        .await
        .expect("order hold");
    commerce_infrastructure::holds::transition_hold(
        &pool,
        &hold,
        commerce_infrastructure::holds::HoldCommand::Release {
            owner_id: &first.snapshot.identity.id,
            actor_id: "synthetic-cancellation",
        },
        1150,
    )
    .await
    .expect("release the physical hold");
    assert_eq!(
        effects(&pool).await.5,
        1,
        "releasing stock never restores promotion capacity"
    );
    policy.promotion.revision = 2;
    promotion::draft(&pool, &policy, "revision-two")
        .await
        .expect("new draft");
    promotion::decide(&pool, "revision-two-review", 2, 0, true)
        .await
        .expect("new approval");
    let (b, bc) = fixture::guest(&store).await;
    let bk = GuestCredential::from_session(&b).expect("second owner");
    let q2 = issue_promoted_quote(
        &store,
        &bk,
        &bc,
        Locale::Nl,
        &fixture::request("second"),
        "COURTYARD",
    )
    .await
    .expect("indicative quote is not capacity")
    .snapshot;
    let before = effects(&pool).await;
    assert!(
        create_order(
            &store,
            &bk,
            &bc,
            "second",
            &order::intent(&q2.original.binding.quote_id),
            &order::seller()
        )
        .await
        .is_err()
    );
    assert_eq!(effects(&pool).await, before);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn withdrawal_or_expiry_after_quote_cannot_commit_order_effects(pool: PgPool) {
    seed(&pool, &promotion::policy()).await;
    let (store, time) = fixture::store(&pool, 1100);
    let (s, csrf) = fixture::guest(&store).await;
    let key = GuestCredential::from_session(&s).expect("owner");
    let q = issue_promoted_quote(
        &store,
        &key,
        &csrf,
        Locale::Nl,
        &fixture::request("quote"),
        "COURTYARD",
    )
    .await
    .expect("quote")
    .snapshot;
    let intent = order::intent(&q.original.binding.quote_id);
    time.0.store(1400, Ordering::SeqCst);
    assert!(
        create_order(&store, &key, &csrf, "expired", &intent, &order::seller())
            .await
            .is_err()
    );
    time.0.store(1100, Ordering::SeqCst);
    promotion::decide(&pool, "withdraw", 1, 1, false)
        .await
        .expect("withdraw approval");
    assert!(
        create_order(&store, &key, &csrf, "withdrawn", &intent, &order::seller())
            .await
            .is_err()
    );
    assert_eq!(effects(&pool).await, (0, 0, 0, 0, 0, 0, 0));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn native_ledger_custody_rejects_missing_and_altered_debits(pool: PgPool) {
    seed(&pool, &promotion::policy()).await;
    let (store, _) = fixture::store(&pool, 1100);
    let (s, csrf) = fixture::guest(&store).await;
    let key = GuestCredential::from_session(&s).expect("owner");
    let q = issue_promoted_quote(
        &store,
        &key,
        &csrf,
        Locale::Nl,
        &fixture::request("quote"),
        "COURTYARD",
    )
    .await
    .expect("quote")
    .snapshot;
    let intent = order::intent(&q.original.binding.quote_id);
    pool.execute("CREATE TABLE redemption_fault(mode text NOT NULL); INSERT INTO redemption_fault VALUES('missing'); CREATE FUNCTION fault_redemption() RETURNS trigger LANGUAGE plpgsql AS $$ DECLARE m text;BEGIN SELECT mode INTO m FROM redemption_fault; CASE m WHEN 'missing' THEN RETURN NULL; WHEN 'amount' THEN NEW.discount_minor=NEW.discount_minor-1; WHEN 'quote' THEN NEW.quote_id=repeat('f',32); WHEN 'promotion' THEN NEW.promotion_id='OTHER'; WHEN 'revision' THEN NEW.promotion_revision=NEW.promotion_revision+1; WHEN 'review' THEN NEW.promotion_review_revision=NEW.promotion_review_revision+1; END CASE; RETURN NEW; END; $$; CREATE TRIGGER aaa_fault_redemption BEFORE INSERT ON promotion_redemptions FOR EACH ROW EXECUTE FUNCTION fault_redemption()").await.expect("isolated native fault instrument");
    for mode in [
        "missing",
        "amount",
        "quote",
        "promotion",
        "revision",
        "review",
    ] {
        sqlx::query("UPDATE redemption_fault SET mode=$1")
            .bind(mode)
            .execute(&pool)
            .await
            .expect("fault mode");
        assert!(
            create_order(&store, &key, &csrf, "place", &intent, &order::seller())
                .await
                .is_err(),
            "native guard rejects {mode}"
        );
        assert_eq!(
            effects(&pool).await,
            (0, 0, 0, 0, 0, 0, 0),
            "all effects roll back on {mode}"
        );
    }
    pool.execute("DROP TRIGGER aaa_fault_redemption ON promotion_redemptions")
        .await
        .expect("remove fault");
    create_order(&store, &key, &csrf, "place", &intent, &order::seller())
        .await
        .expect("positive control");
    assert_eq!(effects(&pool).await, (1, 1, 1, 1, 2, 1, 12));
    assert!(sqlx::query("INSERT INTO promotion_redemptions(order_id,quote_id,promotion_id,promotion_revision,promotion_review_revision,discount_minor,created_at) SELECT order_id,quote_id,promotion_id,promotion_revision,promotion_review_revision,discount_minor,created_at FROM promotion_redemptions").execute(&pool).await.is_err(), "duplicate cannot spend twice");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn promotion_expiry_after_a_real_write_wait_rolls_back_every_effect(pool: PgPool) {
    let mut policy = promotion::policy();
    policy.promotion.expires_at = 1150;
    seed(&pool, &policy).await;
    pool.execute("CREATE FUNCTION pause_final_promotion() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_advisory_xact_lock(851241);RETURN NEW;END;$$;CREATE TRIGGER pause_final_promotion AFTER INSERT ON commerce_outbox FOR EACH ROW EXECUTE FUNCTION pause_final_promotion()").await.expect("post-write gate");
    let (store, time) = fixture::store(&pool, 1100);
    let (s, csrf) = fixture::guest(&store).await;
    let key = GuestCredential::from_session(&s).expect("owner");
    let q = issue_promoted_quote(
        &store,
        &key,
        &csrf,
        Locale::Nl,
        &fixture::request("quote"),
        "COURTYARD",
    )
    .await
    .expect("quote")
    .snapshot;
    let intent = order::intent(&q.original.binding.quote_id);
    let mut gate = pool.begin().await.expect("gate");
    sqlx::query("SELECT pg_advisory_xact_lock(851241)")
        .execute(&mut *gate)
        .await
        .expect("held");
    let work = tokio::spawn({
        let store = store.clone();
        let csrf = csrf.clone();
        let intent = intent.clone();
        let key = GuestCredential::from_session(&s).expect("owner");
        async move { create_order(&store, &key, &csrf, "place", &intent, &order::seller()).await }
    });
    waiting(&pool, 1).await;
    time.0.store(1150, Ordering::SeqCst);
    gate.rollback().await.expect("release");
    assert!(
        work.await.expect("task").is_err(),
        "fresh time must reject expired promotion"
    );
    assert_eq!(effects(&pool).await, (0, 0, 0, 0, 0, 0, 0));
    time.0.store(1100, Ordering::SeqCst);
    create_order(&store, &key, &csrf, "place", &intent, &order::seller())
        .await
        .expect("positive unexpired control");
    assert_eq!(effects(&pool).await, (1, 1, 1, 1, 2, 1, 12));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn superseded_quote_cannot_redeem_even_when_capacity_remains(pool: PgPool) {
    let mut policy = promotion::policy();
    seed(&pool, &policy).await;
    let (store, _) = fixture::store(&pool, 1100);
    let (s, csrf) = fixture::guest(&store).await;
    let key = GuestCredential::from_session(&s).expect("owner");
    let old = issue_promoted_quote(
        &store,
        &key,
        &csrf,
        Locale::Nl,
        &fixture::request("old"),
        "COURTYARD",
    )
    .await
    .expect("revision one quote")
    .snapshot;
    policy.promotion.revision = 2;
    promotion::draft(&pool, &policy, "new-draft")
        .await
        .expect("new revision");
    promotion::decide(&pool, "new-review", 2, 0, true)
        .await
        .expect("approved revision");
    assert!(
        create_order(
            &store,
            &key,
            &csrf,
            "old-order",
            &order::intent(&old.original.binding.quote_id),
            &order::seller()
        )
        .await
        .is_err()
    );
    assert_eq!(effects(&pool).await, (0, 0, 0, 0, 0, 0, 0));
    let current = issue_promoted_quote(
        &store,
        &key,
        &csrf,
        Locale::Nl,
        &fixture::request("current"),
        "COURTYARD",
    )
    .await
    .expect("current quote")
    .snapshot;
    create_order(
        &store,
        &key,
        &csrf,
        "current-order",
        &order::intent(&current.original.binding.quote_id),
        &order::seller(),
    )
    .await
    .expect("current revision positive control");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn approval_withdrawal_queued_before_checkout_is_rechecked_after_anchor_wait(pool: PgPool) {
    seed(&pool, &promotion::policy()).await;
    let (store, _) = fixture::store(&pool, 1100);
    let (s, csrf) = fixture::guest(&store).await;
    let key = GuestCredential::from_session(&s).expect("owner");
    let q = issue_promoted_quote(
        &store,
        &key,
        &csrf,
        Locale::Nl,
        &fixture::request("quote"),
        "COURTYARD",
    )
    .await
    .expect("quote")
    .snapshot;
    let mut gate = pool.begin().await.expect("gate");
    sqlx::query("SELECT id FROM promotion_policy_anchors WHERE id='COURTYARD' FOR UPDATE")
        .execute(&mut *gate)
        .await
        .expect("lock source");
    let withdrawing = tokio::spawn({
        let pool = pool.clone();
        async move { promotion::decide(&pool, "withdraw-wait", 1, 1, false).await }
    });
    waiting(&pool, 1).await;
    let placing = tokio::spawn({
        let store = store.clone();
        let key = GuestCredential::from_session(&s).expect("owner");
        let csrf = csrf.clone();
        let intent = order::intent(&q.original.binding.quote_id);
        async move { create_order(&store, &key, &csrf, "place", &intent, &order::seller()).await }
    });
    waiting(&pool, 2).await;
    gate.rollback().await.expect("unblock queued withdrawal");
    withdrawing
        .await
        .expect("review task")
        .expect("withdrawn first");
    assert!(
        placing.await.expect("checkout task").is_err(),
        "checkout must observe current rejection"
    );
    assert_eq!(effects(&pool).await, (0, 0, 0, 0, 0, 0, 0));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn cumulative_ledger_head_tracks_literal_lifetime_totals_across_revisions(pool: PgPool) {
    let mut policy = promotion::policy();
    policy.limits.total_redemptions = 2;
    policy.limits.total_discount_minor = 1000;
    seed(&pool, &policy).await;
    let (store, _) = fixture::store(&pool, 1100);
    let (a, ac) = fixture::guest(&store).await;
    let ak = GuestCredential::from_session(&a).expect("first owner");
    let first = issue_promoted_quote(
        &store,
        &ak,
        &ac,
        Locale::Nl,
        &fixture::request("first"),
        "COURTYARD",
    )
    .await
    .expect("first quote")
    .snapshot;
    create_order(
        &store,
        &ak,
        &ac,
        "first-order",
        &order::intent(&first.original.binding.quote_id),
        &order::seller(),
    )
    .await
    .expect("first debit");
    policy.promotion.revision = 2;
    promotion::draft(&pool, &policy, "second-draft")
        .await
        .expect("second draft");
    promotion::decide(&pool, "second-review", 2, 0, true)
        .await
        .expect("second review");
    let (b, bc) = fixture::guest(&store).await;
    let bk = GuestCredential::from_session(&b).expect("second owner");
    let second = issue_promoted_quote(
        &store,
        &bk,
        &bc,
        Locale::Nl,
        &fixture::request("second"),
        "COURTYARD",
    )
    .await
    .expect("second quote")
    .snapshot;
    pool.execute("CREATE FUNCTION forged_redemption_head() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN NEW.redemption_number=999; NEW.lifetime_discount_minor=1; RETURN NEW; END; $$; CREATE TRIGGER aaa_forged_redemption_head BEFORE INSERT ON promotion_redemptions FOR EACH ROW EXECUTE FUNCTION forged_redemption_head()").await.expect("supply false cumulative fields before native guard");
    create_order(
        &store,
        &bk,
        &bc,
        "second-order",
        &order::intent(&second.original.binding.quote_id),
        &order::seller(),
    )
    .await
    .expect("guard derives actual second debit");
    let rows: Vec<(i32, i64, i64, i64)> = sqlx::query_as("SELECT promotion_revision,discount_minor,redemption_number,lifetime_discount_minor FROM promotion_redemptions ORDER BY redemption_number").fetch_all(&pool).await.expect("lifetime history");
    assert_eq!(rows, vec![(1, 500, 1, 500), (2, 500, 2, 1000)]);
    assert!(pool.execute("UPDATE promotion_redemptions SET redemption_number=1,lifetime_discount_minor=0 WHERE promotion_revision=2").await.is_err(), "history cannot forge a head");
    let after: Vec<(i32, i64, i64, i64)> = sqlx::query_as("SELECT promotion_revision,discount_minor,redemption_number,lifetime_discount_minor FROM promotion_redemptions ORDER BY redemption_number").fetch_all(&pool).await.expect("unchanged history");
    assert_eq!(after, rows);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn native_debit_reconciles_corrupted_quote_gross_and_allocations(pool: PgPool) {
    seed(&pool, &promotion::policy()).await;
    let (store, _) = fixture::store(&pool, 1100);
    let (s, csrf) = fixture::guest(&store).await;
    let key = GuestCredential::from_session(&s).expect("owner");
    let q = issue_promoted_quote(
        &store,
        &key,
        &csrf,
        Locale::Nl,
        &fixture::request("quote"),
        "COURTYARD",
    )
    .await
    .expect("quote")
    .snapshot;
    let intent = order::intent(&q.original.binding.quote_id);
    // Corrupt both sides only inside the redemption transaction, after Rust has
    // validated its original input. The SQL guard must independently reject it.
    pool.execute("ALTER TABLE guest_quote_snapshots DISABLE TRIGGER guard_guest_quote; ALTER TABLE checkout_orders DISABLE TRIGGER guard_checkout_order; CREATE TABLE monetary_fault(mode text NOT NULL); INSERT INTO monetary_fault VALUES('total'); CREATE FUNCTION corrupt_redemption_source() RETURNS trigger LANGUAGE plpgsql AS $$ DECLARE m text; v jsonb; BEGIN SELECT mode INTO m FROM monetary_fault; SELECT snapshot INTO v FROM guest_quote_snapshots WHERE id=NEW.quote_id; CASE m WHEN 'total' THEN v=jsonb_set(v,'{total,gross_minor}',to_jsonb((v#>>'{total,gross_minor}')::bigint+1)); WHEN 'line' THEN v=jsonb_set(v,'{lines,0,amounts,gross_minor}',to_jsonb((v#>>'{lines,0,amounts,gross_minor}')::bigint+1)); WHEN 'allocation' THEN v=jsonb_set(v,'{lines,0,discount_minor}',to_jsonb((v#>>'{lines,0,discount_minor}')::bigint-1)); END CASE; UPDATE guest_quote_snapshots SET snapshot=v WHERE id=NEW.quote_id; UPDATE checkout_orders SET snapshot=jsonb_set(snapshot,'{quote}',v) WHERE id=NEW.order_id; RETURN NEW; END; $$; CREATE TRIGGER aaa_corrupt_redemption_source BEFORE INSERT ON promotion_redemptions FOR EACH ROW EXECUTE FUNCTION corrupt_redemption_source()").await.expect("isolated bypass instrument");
    for mode in ["total", "line", "allocation"] {
        sqlx::query("UPDATE monetary_fault SET mode=$1")
            .bind(mode)
            .execute(&pool)
            .await
            .expect("mode");
        assert!(
            create_order(&store, &key, &csrf, "place", &intent, &order::seller())
                .await
                .is_err(),
            "native guard rejects changed {mode}"
        );
        assert_eq!(effects(&pool).await, (0, 0, 0, 0, 0, 0, 0));
    }
    pool.execute("DROP TRIGGER aaa_corrupt_redemption_source ON promotion_redemptions; ALTER TABLE guest_quote_snapshots ENABLE TRIGGER guard_guest_quote; ALTER TABLE checkout_orders ENABLE TRIGGER guard_checkout_order").await.expect("restore guards");
    create_order(&store, &key, &csrf, "place", &intent, &order::seller())
        .await
        .expect("intact positive control");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn native_debit_rejects_damaged_or_customer_limited_policy(pool: PgPool) {
    seed(&pool, &promotion::policy()).await;
    let (store, _) = fixture::store(&pool, 1100);
    let (s, csrf) = fixture::guest(&store).await;
    let key = GuestCredential::from_session(&s).expect("owner");
    let q = issue_promoted_quote(
        &store,
        &key,
        &csrf,
        Locale::Nl,
        &fixture::request("quote"),
        "COURTYARD",
    )
    .await
    .expect("quote")
    .snapshot;
    let intent = order::intent(&q.original.binding.quote_id);
    pool.execute("ALTER TABLE promotion_policy_drafts DISABLE TRIGGER guard_promotion_draft; CREATE TABLE limit_fault(mode text NOT NULL); INSERT INTO limit_fault VALUES('zero-count'); CREATE FUNCTION corrupt_redemption_limits() RETURNS trigger LANGUAGE plpgsql AS $$ DECLARE m text; v jsonb; BEGIN SELECT mode INTO m FROM limit_fault; SELECT policy INTO v FROM promotion_policy_drafts WHERE id=NEW.promotion_id AND revision=NEW.promotion_revision; CASE m WHEN 'zero-count' THEN v=jsonb_set(v,'{limits,total_redemptions}','0'); WHEN 'null-count' THEN v=jsonb_set(v,'{limits,total_redemptions}','null'); WHEN 'malformed-money' THEN v=jsonb_set(v,'{limits,total_discount_minor}','\"bad\"'); WHEN 'missing-customer' THEN v=jsonb_set(v,'{limits}',(v->'limits')-'per_customer_redemptions'); WHEN 'customer' THEN v=jsonb_set(v,'{limits,per_customer_redemptions}','1'); END CASE; UPDATE promotion_policy_drafts SET policy=v WHERE id=NEW.promotion_id AND revision=NEW.promotion_revision; RETURN NEW; END; $$; CREATE TRIGGER aaa_corrupt_redemption_limits BEFORE INSERT ON promotion_redemptions FOR EACH ROW EXECUTE FUNCTION corrupt_redemption_limits()").await.expect("isolated native source fault");
    for mode in [
        "zero-count",
        "null-count",
        "malformed-money",
        "missing-customer",
        "customer",
    ] {
        sqlx::query("UPDATE limit_fault SET mode=$1")
            .bind(mode)
            .execute(&pool)
            .await
            .expect("mode");
        assert!(
            create_order(&store, &key, &csrf, "place", &intent, &order::seller())
                .await
                .is_err(),
            "native guard rejects {mode}"
        );
        assert_eq!(effects(&pool).await, (0, 0, 0, 0, 0, 0, 0));
    }
    pool.execute("DROP TRIGGER aaa_corrupt_redemption_limits ON promotion_redemptions; ALTER TABLE promotion_policy_drafts ENABLE TRIGGER guard_promotion_draft").await.expect("restore guard");
    create_order(&store, &key, &csrf, "place", &intent, &order::seller())
        .await
        .expect("intact positive control");
}

// The before-insert fault simulates a direct native writer that has already
// changed both stored quote copies and all payable amounts consistently. Rust
// has validated the original in-memory quote before this trigger runs.
async fn coherent_over_discount_is_rejected(
    pool: PgPool,
    policy: commerce_domain::promotion_policies::PromotionPolicy,
    original_discount: i64,
    amounts: [i64; 6],
) {
    let [
        line_gross,
        line_net,
        line_vat,
        total_gross,
        total_net,
        total_vat,
    ] = amounts;
    seed(&pool, &policy).await;
    let (store, _) = fixture::store(&pool, 1100);
    let (s, csrf) = fixture::guest(&store).await;
    let key = GuestCredential::from_session(&s).expect("owner");
    let quote = issue_promoted_quote(
        &store,
        &key,
        &csrf,
        Locale::Nl,
        &fixture::request("quote"),
        "COURTYARD",
    )
    .await
    .expect("approved quote")
    .snapshot;
    assert_eq!(quote.discount_minor, original_discount);
    let intent = order::intent(&quote.original.binding.quote_id);
    pool.execute("ALTER TABLE guest_quote_snapshots DISABLE TRIGGER guard_guest_quote; ALTER TABLE checkout_orders DISABLE TRIGGER guard_checkout_order; CREATE TABLE coherent_over_discount(discount bigint, line_gross bigint, line_net bigint, line_vat bigint, total_gross bigint, total_net bigint, total_vat bigint); CREATE FUNCTION forge_consistent_discount() RETURNS trigger LANGUAGE plpgsql AS $$ DECLARE f coherent_over_discount%ROWTYPE; v jsonb; BEGIN SELECT * INTO f FROM coherent_over_discount; SELECT snapshot INTO v FROM guest_quote_snapshots WHERE id=NEW.quote_id; v=jsonb_set(v,'{discount_minor}',to_jsonb(f.discount)); v=jsonb_set(v,'{lines,0,discount_minor}',to_jsonb(f.discount)); v=jsonb_set(v,'{lines,0,amounts,gross_minor}',to_jsonb(f.line_gross)); v=jsonb_set(v,'{lines,0,amounts,net_minor}',to_jsonb(f.line_net)); v=jsonb_set(v,'{lines,0,amounts,vat_minor}',to_jsonb(f.line_vat)); v=jsonb_set(v,'{total,gross_minor}',to_jsonb(f.total_gross)); v=jsonb_set(v,'{total,net_minor}',to_jsonb(f.total_net)); v=jsonb_set(v,'{total,vat_minor}',to_jsonb(f.total_vat)); UPDATE guest_quote_snapshots SET snapshot=v WHERE id=NEW.quote_id; UPDATE checkout_orders SET snapshot=jsonb_set(snapshot,'{quote}',v) WHERE id=NEW.order_id; NEW.discount_minor=f.discount; RETURN NEW; END; $$; CREATE TRIGGER aaa_forge_consistent_discount BEFORE INSERT ON promotion_redemptions FOR EACH ROW EXECUTE FUNCTION forge_consistent_discount()").await.expect("isolated native bypass");
    sqlx::query("INSERT INTO coherent_over_discount VALUES($1,$2,$3,$4,$5,$6,$7)")
        .bind(original_discount + 1)
        .bind(line_gross)
        .bind(line_net)
        .bind(line_vat)
        .bind(total_gross)
        .bind(total_net)
        .bind(total_vat)
        .execute(&pool)
        .await
        .expect("literal coherent over-discount");
    assert!(
        matches!(
            create_order(&store, &key, &csrf, "place", &intent, &order::seller()).await,
            Err(OrderStoreError::Database)
        ),
        "native debit rejects excess beyond approved terms"
    );
    assert_eq!(effects(&pool).await, (0, 0, 0, 0, 0, 0, 0));
    pool.execute("DROP TRIGGER aaa_forge_consistent_discount ON promotion_redemptions; ALTER TABLE guest_quote_snapshots ENABLE TRIGGER guard_guest_quote; ALTER TABLE checkout_orders ENABLE TRIGGER guard_checkout_order").await.expect("restore guards");
    let valid = create_order(&store, &key, &csrf, "place", &intent, &order::seller())
        .await
        .expect("unchanged approved amount succeeds");
    assert_eq!(
        valid
            .snapshot
            .promoted_quote
            .expect("promoted")
            .discount_minor,
        original_discount
    );
    assert_eq!(effects(&pool).await, (1, 1, 1, 1, 2, 1, 12));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn native_debit_rejects_coherent_excess_over_fixed_terms(pool: PgPool) {
    let mut policy = promotion::policy();
    policy.promotion.maximum_discount_minor = 1000;
    coherent_over_discount_is_rejected(pool, policy, 500, [36534, 30193, 6341, 37744, 31193, 6551])
        .await;
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn native_debit_rejects_coherent_excess_over_percentage_terms(pool: PgPool) {
    let mut policy = promotion::policy();
    policy.promotion.discount =
        commerce_domain::promotions::DiscountKind::Percentage { basis_points: 1000 };
    policy.promotion.maximum_discount_minor = 5000;
    coherent_over_discount_is_rejected(
        pool,
        policy,
        3704,
        [33330, 27545, 5785, 34540, 28545, 5995],
    )
    .await;
}

#[path = "support/redemption_claim_faults.rs"]
mod redemption_claim_faults;

#[sqlx::test(migrator = "MIGRATOR")]
async fn promoted_order_history_rejects_damaged_ledger_counters_and_transaction_marker(
    pool: PgPool,
) {
    seed(&pool, &promotion::policy()).await;
    let (store, _) = fixture::store(&pool, 1100);
    let (s, csrf) = fixture::guest(&store).await;
    let key = GuestCredential::from_session(&s).expect("owner");
    let quote = issue_promoted_quote(
        &store,
        &key,
        &csrf,
        Locale::Nl,
        &fixture::request("quote"),
        "COURTYARD",
    )
    .await
    .expect("approved quote")
    .snapshot;
    let order = create_order(
        &store,
        &key,
        &csrf,
        "place",
        &order::intent(&quote.original.binding.quote_id),
        &order::seller(),
    )
    .await
    .expect("intact order")
    .snapshot;
    assert!(owned_order(&store, &key, &order.identity.id).await.is_ok());
    pool.execute("ALTER TABLE promotion_redemptions DISABLE TRIGGER guard_promotion_redemption; ALTER TABLE promotion_redemptions DROP CONSTRAINT promotion_redemptions_redemption_number_check, DROP CONSTRAINT promotion_redemptions_lifetime_discount_minor_check; ALTER TABLE checkout_orders DISABLE TRIGGER guard_checkout_order").await.expect("simulate isolated damaged imports");
    for (number, total) in [
        (0_i64, 500_i64),
        (1_000_001, 500),
        (1, 499),
        (1, 9_007_199_254_740_992),
    ] {
        sqlx::query("UPDATE promotion_redemptions SET redemption_number=$1,lifetime_discount_minor=$2 WHERE order_id=$3").bind(number).bind(total).bind(&order.identity.id).execute(&pool).await.expect("simulate damaged receipt");
        assert!(
            matches!(
                owned_order(&store, &key, &order.identity.id).await,
                Err(OrderStoreError::InvalidStoredData)
            ),
            "invalid receipt {number}/{total} cannot authorize history"
        );
    }
    sqlx::query("UPDATE promotion_redemptions SET redemption_number=1,lifetime_discount_minor=500 WHERE order_id=$1").bind(&order.identity.id).execute(&pool).await.expect("restore receipt");
    sqlx::query("UPDATE checkout_orders SET promotion_redemption_transaction_id=NULL WHERE id=$1")
        .bind(&order.identity.id)
        .execute(&pool)
        .await
        .expect("simulate detached marker");
    assert!(matches!(
        owned_order(&store, &key, &order.identity.id).await,
        Err(OrderStoreError::InvalidStoredData)
    ));
    sqlx::query("UPDATE checkout_orders o SET promotion_redemption_transaction_id=r.transaction_id FROM promotion_redemptions r WHERE o.id=r.order_id AND o.id=$1").bind(&order.identity.id).execute(&pool).await.expect("restore marker");
    assert!(
        owned_order(&store, &key, &order.identity.id).await.is_ok(),
        "restored exact history remains readable"
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn promoted_order_reader_rejects_nonarray_or_oversized_allocations(pool: PgPool) {
    seed(&pool, &promotion::policy()).await;
    let (store, _) = fixture::store(&pool, 1100);
    let (session, csrf) = fixture::guest(&store).await;
    let key = GuestCredential::from_session(&session).expect("owner");
    let quote = issue_promoted_quote(
        &store,
        &key,
        &csrf,
        Locale::Nl,
        &fixture::request("quote"),
        "COURTYARD",
    )
    .await
    .expect("quote")
    .snapshot;
    let order = create_order(
        &store,
        &key,
        &csrf,
        "place",
        &order::intent(&quote.original.binding.quote_id),
        &order::seller(),
    )
    .await
    .expect("order")
    .snapshot;
    let original_quote = serde_json::to_value(&quote).expect("quote JSON");
    let original_order = serde_json::to_value(&order).expect("order JSON");
    assert!(owned_order(&store, &key, &order.identity.id).await.is_ok());
    pool.execute("ALTER TABLE checkout_orders DISABLE TRIGGER guard_checkout_order; ALTER TABLE guest_quote_snapshots DISABLE TRIGGER guard_guest_quote").await.expect("isolated corrupted import");
    for lines in [
        serde_json::json!(null),
        serde_json::json!([null]),
        serde_json::Value::Array(vec![original_quote["lines"][0].clone(); 21]),
    ] {
        let mut damaged = original_quote.clone();
        damaged["lines"] = lines;
        let mut damaged_order = original_order.clone();
        damaged_order["quote"] = damaged.clone();
        sqlx::query("UPDATE guest_quote_snapshots SET snapshot=$1 WHERE id=$2")
            .bind(&damaged)
            .bind(&quote.original.binding.quote_id)
            .execute(&pool)
            .await
            .expect("corrupt quote");
        sqlx::query("UPDATE checkout_orders SET snapshot=$1 WHERE id=$2")
            .bind(&damaged_order)
            .bind(&order.identity.id)
            .execute(&pool)
            .await
            .expect("corrupt order");
        assert!(
            matches!(
                owned_order(&store, &key, &order.identity.id).await,
                Err(OrderStoreError::InvalidStoredData)
            ),
            "unbounded or nonobject allocations must fail closed"
        );
    }
    sqlx::query("UPDATE guest_quote_snapshots SET snapshot=$1 WHERE id=$2")
        .bind(&original_quote)
        .bind(&quote.original.binding.quote_id)
        .execute(&pool)
        .await
        .expect("restore quote");
    sqlx::query("UPDATE checkout_orders SET snapshot=$1 WHERE id=$2")
        .bind(&original_order)
        .bind(&order.identity.id)
        .execute(&pool)
        .await
        .expect("restore order");
    assert!(
        owned_order(&store, &key, &order.identity.id).await.is_ok(),
        "intact history remains readable"
    );
}

#[path = "support/redemption_source_faults.rs"]
mod redemption_source_faults;
