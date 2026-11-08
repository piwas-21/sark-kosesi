//! Verified promotion acceptance over isolated PostgreSQL, without provider I/O.
#[allow(dead_code)]
#[path = "support/admitted_login.rs"]
mod admitted_login;
#[allow(dead_code)]
#[path = "support/admitted_proofs.rs"]
mod admitted_proofs;
#[allow(dead_code)]
#[path = "support/payment_fixture.rs"]
mod payment;

use admitted_proofs::{
    customer_passwords, email_verification, verified_customer_accounts::establish,
};
use commerce_application::Locale;
use commerce_infrastructure::{
    MIGRATOR,
    checkout_orders::{
        OrderStoreError, create_order, create_verified_promoted_order, owned_order,
        owned_verified_promoted_order,
    },
    checkout_quotes::{issue_promoted_quote, issue_quote, issue_verified_promoted_quote},
    customer_sessions::CustomerSessionToken,
    guest_baskets::GuestCredential,
    guest_order_links,
    payment_attempts::prepare_attempt,
};
use payment::{order, promotion, quote as fixture};
use sqlx::{Executor, PgPool};
use std::sync::atomic::Ordering;

async fn seed(pool: &PgPool, capped: bool) {
    fixture::seed(pool).await;
    pool.execute("INSERT INTO staff_capabilities VALUES('author','promotion.manage'),('reviewer','promotion.review')").await.expect("independent promotion grants");
    let mut policy = promotion::policy();
    policy.limits.per_customer_redemptions = capped.then_some(1);
    promotion::draft(pool, &policy, "draft")
        .await
        .expect("draft");
    promotion::decide(pool, "review", 1, 0, true)
        .await
        .expect("review");
}

async fn account(pool: &PgPool, email: &str, time: &fixture::Time) -> (i64, CustomerSessionToken) {
    let proof = email_verification::issue(pool, email, time)
        .await
        .expect("verification proof");
    let id = establish(pool, email, &proof, time)
        .await
        .expect("verified account")
        .value();
    let proof = customer_passwords::issue(pool, email, time)
        .await
        .expect("password proof");
    customer_passwords::replace(pool, email, &proof, "correct horse battery staple", time)
        .await
        .expect("password setup");
    let token = admitted_login::login(pool, email, "correct horse battery staple", time)
        .await
        .expect("real login")
        .token;
    (id, token)
}

async fn effects(pool: &PgPool) -> (i64, i64, i64, i64, i64, i64, i64) {
    sqlx::query_as("SELECT (SELECT count(*) FROM checkout_orders),(SELECT count(*) FROM commerce_outbox),(SELECT count(*) FROM basket_hold_groups),(SELECT count(*) FROM stock_holds),(SELECT count(*) FROM stock_movements),(SELECT count(*) FROM promotion_redemptions),(SELECT COALESCE(sum(reserved),0)::bigint FROM stock_balances)").fetch_one(pool).await.expect("atomic effects")
}

async fn waiting(pool: &PgPool, minimum: i64) {
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let n: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock'").fetch_one(pool).await.expect("lock waits");
            if n >= minimum { return; }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }).await.expect("actual lock wait");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn same_account_competes_across_baskets_and_other_account_keeps_its_allowance(pool: PgPool) {
    seed(&pool, true).await;
    let (store, time) = fixture::store(&pool, 1100);
    let (account_id, customer) = account(&pool, "buyer@example.test", &time).await;
    let (other_id, other_customer) = account(&pool, "other@example.test", &time).await;
    let (a, ac) = fixture::guest(&store).await;
    let (b, bc) = fixture::guest(&store).await;
    let (c, cc) = fixture::guest(&store).await;
    let ak = GuestCredential::from_session(&a).expect("basket a");
    let bk = GuestCredential::from_session(&b).expect("basket b");
    let ck = GuestCredential::from_session(&c).expect("basket c");
    let aq = issue_verified_promoted_quote(
        &store,
        &ak,
        &ac,
        &customer,
        Locale::Nl,
        &fixture::request("a"),
        "COURTYARD",
    )
    .await
    .expect("quote a")
    .snapshot;
    let bq = issue_verified_promoted_quote(
        &store,
        &bk,
        &bc,
        &customer,
        Locale::Nl,
        &fixture::request("b"),
        "COURTYARD",
    )
    .await
    .expect("quote b")
    .snapshot;
    let cq = issue_verified_promoted_quote(
        &store,
        &ck,
        &cc,
        &other_customer,
        Locale::Nl,
        &fixture::request("c"),
        "COURTYARD",
    )
    .await
    .expect("quote c")
    .snapshot;
    assert_eq!(
        (
            aq.original.total.gross_minor,
            aq.discount_minor,
            aq.total.gross_minor,
            aq.total.net_minor,
            aq.total.vat_minor
        ),
        (38245, 500, 37745, 31194, 6551)
    );
    pool.execute("CREATE FUNCTION test_verified_order_gate() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_advisory_xact_lock(1936876910,1); RETURN NEW; END; $$; CREATE TRIGGER z_test_verified_order_gate AFTER INSERT ON promotion_redemptions FOR EACH ROW EXECUTE FUNCTION test_verified_order_gate()").await.expect("post-debit gate");
    let mut gate = pool.begin().await.expect("gate");
    sqlx::query("SELECT pg_advisory_xact_lock(1936876910,1)")
        .execute(&mut *gate)
        .await
        .expect("hold");
    let first = tokio::spawn({
        let store = store.clone();
        let token = CustomerSessionToken::from_bytes(*customer.expose_bytes());
        let intent = order::intent(&aq.original.binding.quote_id);
        async move {
            create_verified_promoted_order(
                &store,
                &ak,
                &ac,
                &token,
                "order-a",
                &intent,
                &order::seller(),
            )
            .await
        }
    });
    waiting(&pool, 1).await;
    let second = tokio::spawn({
        let store = store.clone();
        let token = CustomerSessionToken::from_bytes(*customer.expose_bytes());
        let intent = order::intent(&bq.original.binding.quote_id);
        async move {
            create_verified_promoted_order(
                &store,
                &bk,
                &bc,
                &token,
                "order-b",
                &intent,
                &order::seller(),
            )
            .await
        }
    });
    waiting(&pool, 2).await;
    gate.rollback().await.expect("release");
    let first = first.await.expect("first task");
    let second = second.await.expect("second task");
    assert!(first.is_ok(), "first transaction held the debit gate");
    assert!(
        matches!(second, Err(OrderStoreError::Unavailable)),
        "second transaction exhausts the same account allowance"
    );
    assert_eq!(
        effects(&pool).await,
        (1, 1, 1, 1, 2, 1, 12),
        "loser rolls back all effects"
    );
    pool.execute("DROP TRIGGER z_test_verified_order_gate ON promotion_redemptions")
        .await
        .expect("remove gate");
    let independent = create_verified_promoted_order(
        &store,
        &ck,
        &cc,
        &other_customer,
        "order-c",
        &order::intent(&cq.original.binding.quote_id),
        &order::seller(),
    )
    .await
    .expect("independent account allowance")
    .snapshot;
    assert!(independent.promoted_quote.as_ref() == Some(&cq));
    let ledger: Vec<(Option<i64>, i64, i64, i64)> = sqlx::query_as("SELECT promotion_account_id,discount_minor,redemption_number,lifetime_discount_minor FROM promotion_redemptions ORDER BY redemption_number").fetch_all(&pool).await.expect("exact ledger");
    assert_eq!(
        ledger,
        vec![
            (Some(account_id), 500, 1, 500),
            (Some(other_id), 500, 2, 1000)
        ]
    );
    assert_eq!(effects(&pool).await, (2, 2, 2, 2, 4, 2, 24));
    let mut next = promotion::policy();
    next.promotion.revision = 2;
    next.limits.per_customer_redemptions = Some(1);
    promotion::draft(&pool, &next, "draft-two")
        .await
        .expect("revision two");
    promotion::decide(&pool, "review-two", 2, 0, true)
        .await
        .expect("approval two");
    let (d, dc) = fixture::guest(&store).await;
    let dk = GuestCredential::from_session(&d).expect("basket d");
    let dq = issue_verified_promoted_quote(
        &store,
        &dk,
        &dc,
        &customer,
        Locale::Nl,
        &fixture::request("d"),
        "COURTYARD",
    )
    .await
    .expect("revision two indicative quote")
    .snapshot;
    let before = effects(&pool).await;
    assert!(
        create_verified_promoted_order(
            &store,
            &dk,
            &dc,
            &customer,
            "order-d",
            &order::intent(&dq.original.binding.quote_id),
            &order::seller()
        )
        .await
        .is_err(),
        "account limit carries across revisions"
    );
    assert_eq!(effects(&pool).await, before);
}

#[sqlx::test(migrations = false)]
async fn account_bound_order_requires_both_owners_and_denies_guest_custody_paths(pool: PgPool) {
    // Migration 0055 intentionally replaces the payment blocker.
    MIGRATOR.run_to(54, &pool).await.expect("schema 54");
    seed(&pool, true).await;
    let (store, time) = fixture::store(&pool, 1100);
    let (_, customer) = account(&pool, "buyer@example.test", &time).await;
    let (_, other_customer) = account(&pool, "other@example.test", &time).await;
    let (session, csrf) = fixture::guest(&store).await;
    let (other_session, other_csrf) = fixture::guest(&store).await;
    let key = GuestCredential::from_session(&session).expect("owner");
    let other_key = GuestCredential::from_session(&other_session).expect("other basket");
    let invalid = CustomerSessionToken::from_bytes([42; 32]);
    let quote = issue_verified_promoted_quote(
        &store,
        &key,
        &csrf,
        &customer,
        Locale::Nl,
        &fixture::request("quote"),
        "COURTYARD",
    )
    .await
    .expect("quote")
    .snapshot;
    let intent = order::intent(&quote.original.binding.quote_id);
    for token in [&other_customer, &invalid] {
        assert!(
            create_verified_promoted_order(
                &store,
                &key,
                &csrf,
                token,
                "place",
                &intent,
                &order::seller()
            )
            .await
            .is_err()
        );
    }
    assert!(
        create_verified_promoted_order(
            &store,
            &other_key,
            &other_csrf,
            &customer,
            "place",
            &intent,
            &order::seller()
        )
        .await
        .is_err()
    );
    assert!(
        create_order(&store, &key, &csrf, "place", &intent, &order::seller())
            .await
            .is_err()
    );
    assert_eq!(effects(&pool).await, (0, 0, 0, 0, 0, 0, 0));
    let placed = create_verified_promoted_order(
        &store,
        &key,
        &csrf,
        &customer,
        "place",
        &intent,
        &order::seller(),
    )
    .await
    .expect("accept");
    assert!(!placed.replayed);
    let id = &placed.snapshot.identity.id;
    let replay = create_verified_promoted_order(
        &store,
        &key,
        &csrf,
        &customer,
        "place",
        &intent,
        &order::seller(),
    )
    .await
    .expect("replay");
    assert!(replay.replayed && replay.snapshot == placed.snapshot);
    let mut altered = intent.clone();
    altered.buyer.name = "Different buyer".into();
    assert!(
        create_verified_promoted_order(
            &store,
            &key,
            &csrf,
            &customer,
            "place",
            &altered,
            &order::seller()
        )
        .await
        .is_err()
    );
    assert!(
        create_verified_promoted_order(
            &store,
            &key,
            &csrf,
            &other_customer,
            "place",
            &intent,
            &order::seller()
        )
        .await
        .is_err()
    );
    assert!(
        create_order(&store, &key, &csrf, "place", &intent, &order::seller())
            .await
            .is_err()
    );
    assert!(
        owned_verified_promoted_order(&store, &key, &customer, id)
            .await
            .is_ok()
    );
    assert!(
        owned_verified_promoted_order(&store, &key, &other_customer, id)
            .await
            .is_err()
    );
    assert!(
        owned_verified_promoted_order(&store, &other_key, &customer, id)
            .await
            .is_err()
    );
    assert!(
        owned_verified_promoted_order(&store, &key, &invalid, id)
            .await
            .is_err()
    );
    assert!(
        owned_order(&store, &key, id).await.is_err(),
        "guest read denied"
    );
    assert!(
        guest_order_links::issue(&store, &key, &csrf, id)
            .await
            .is_err(),
        "guest cannot issue a linking proof"
    );
    assert!(
        prepare_attempt(
            &store,
            &key,
            &csrf,
            "pay",
            &payment::intent(&placed.snapshot),
            &payment::config()
        )
        .await
        .is_err(),
        "guest payment preparation denied"
    );
    for query in [
        "INSERT INTO payment_attempts(order_id) VALUES ($1)",
        "INSERT INTO guest_order_link_proofs(order_id) VALUES ($1)",
    ] {
        let error = sqlx::query(query)
            .bind(id)
            .execute(&pool)
            .await
            .expect_err("native guest custody must reject the bound order");
        let sqlx::Error::Database(error) = error else {
            panic!("expected native PostgreSQL guard error")
        };
        assert_eq!(error.code().as_deref(), Some("23514"));
        assert_eq!(
            error.message(),
            "Account-bound order requires account-aware custody"
        );
    }
    let extras: (i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM guest_order_link_proofs),(SELECT count(*) FROM payment_attempts)").fetch_one(&pool).await.expect("no guest effects");
    assert_eq!(extras, (0, 0));
    assert_eq!(effects(&pool).await, (1, 1, 1, 1, 2, 1, 12));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn capped_revision_fails_closed_after_anonymous_lifetime_debit(pool: PgPool) {
    seed(&pool, false).await;
    let (store, time) = fixture::store(&pool, 1100);
    let (_, customer) = account(&pool, "buyer@example.test", &time).await;
    let (guest, csrf) = fixture::guest(&store).await;
    let key = GuestCredential::from_session(&guest).expect("guest");
    let first = issue_promoted_quote(
        &store,
        &key,
        &csrf,
        Locale::Nl,
        &fixture::request("anonymous"),
        "COURTYARD",
    )
    .await
    .expect("anonymous quote")
    .snapshot;
    create_order(
        &store,
        &key,
        &csrf,
        "anonymous",
        &order::intent(&first.original.binding.quote_id),
        &order::seller(),
    )
    .await
    .expect("anonymous debit");
    let (guest, csrf) = fixture::guest(&store).await;
    let key = GuestCredential::from_session(&guest).expect("verified basket");
    let uncapped = issue_verified_promoted_quote(
        &store,
        &key,
        &csrf,
        &customer,
        Locale::Nl,
        &fixture::request("uncapped"),
        "COURTYARD",
    )
    .await
    .expect("uncapped account quote")
    .snapshot;
    let before = effects(&pool).await;
    // Probe the native error after the Rust claim. Suppress the outer debit
    // only when the nested insert reaches the specific anonymous-history guard.
    pool.execute(r#"ALTER TABLE promotion_policy_drafts DISABLE TRIGGER guard_promotion_draft;
        CREATE FUNCTION test_cap_after_redemption_claim() RETURNS trigger LANGUAGE plpgsql AS $$
        BEGIN
            IF pg_trigger_depth()>1 THEN RETURN NEW; END IF;
            UPDATE promotion_policy_drafts SET policy=jsonb_set(policy,'{limits,per_customer_redemptions}','1')
                WHERE id=NEW.promotion_id AND revision=NEW.promotion_revision;
            BEGIN
                INSERT INTO promotion_redemptions(order_id,quote_id,promotion_id,promotion_revision,
                    promotion_review_revision,discount_minor,created_at,promotion_account_id)
                VALUES(NEW.order_id,NEW.quote_id,NEW.promotion_id,NEW.promotion_revision,
                    NEW.promotion_review_revision,NEW.discount_minor,NEW.created_at,NEW.promotion_account_id);
            EXCEPTION WHEN check_violation THEN
                IF SQLERRM='Capped promotion has anonymous redemption history' THEN RETURN NULL; END IF;
                RAISE;
            END;
            RAISE EXCEPTION 'Anonymous-history guard unexpectedly accepted the native debit';
        END;
        $$;
        CREATE TRIGGER aaa_test_cap_after_redemption_claim BEFORE INSERT ON promotion_redemptions
            FOR EACH ROW EXECUTE FUNCTION test_cap_after_redemption_claim();"#)
        .await.expect("inject valid cap after the Rust claim");
    assert!(matches!(
        create_verified_promoted_order(
            &store,
            &key,
            &csrf,
            &customer,
            "native-denial",
            &order::intent(&uncapped.original.binding.quote_id),
            &order::seller()
        )
        .await,
        Err(OrderStoreError::InvalidStoredData)
    ));
    assert_eq!(effects(&pool).await, before);
    pool.execute("DROP TRIGGER aaa_test_cap_after_redemption_claim ON promotion_redemptions; ALTER TABLE promotion_policy_drafts ENABLE TRIGGER guard_promotion_draft")
        .await.expect("restore guards after native denial");
    let mut policy = promotion::policy();
    policy.promotion.revision = 2;
    policy.limits.per_customer_redemptions = Some(1);
    promotion::draft(&pool, &policy, "capped-revision")
        .await
        .expect("draft");
    promotion::decide(&pool, "capped-review", 2, 0, true)
        .await
        .expect("review");
    let quote = issue_verified_promoted_quote(
        &store,
        &key,
        &csrf,
        &customer,
        Locale::Nl,
        &fixture::request("verified"),
        "COURTYARD",
    )
    .await
    .expect("indicative quote")
    .snapshot;
    let before = effects(&pool).await;
    assert!(
        create_verified_promoted_order(
            &store,
            &key,
            &csrf,
            &customer,
            "verified",
            &order::intent(&quote.original.binding.quote_id),
            &order::seller()
        )
        .await
        .is_err()
    );
    assert_eq!(effects(&pool).await, before);

    let mut uncapped_policy = promotion::policy();
    uncapped_policy.promotion.revision = 3;
    promotion::draft(&pool, &uncapped_policy, "uncapped-revision")
        .await
        .expect("uncapped draft");
    promotion::decide(&pool, "uncapped-review", 3, 0, true)
        .await
        .expect("uncapped approval");
    let accepted = issue_verified_promoted_quote(
        &store,
        &key,
        &csrf,
        &customer,
        Locale::Nl,
        &fixture::request("restored-uncapped"),
        "COURTYARD",
    )
    .await
    .expect("uncapped quote with anonymous history")
    .snapshot;
    create_verified_promoted_order(
        &store,
        &key,
        &csrf,
        &customer,
        "restored-uncapped",
        &order::intent(&accepted.original.binding.quote_id),
        &order::seller(),
    )
    .await
    .expect("healthy uncapped order preserves the lifetime ledger");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn session_expiry_during_real_order_write_rolls_back_every_effect(pool: PgPool) {
    seed(&pool, true).await;
    let (store, time) = fixture::store(&pool, 1100);
    let (_, customer) = account(&pool, "buyer@example.test", &time).await;
    let (session, csrf) = fixture::guest(&store).await;
    let key = GuestCredential::from_session(&session).expect("guest");
    let quote = issue_verified_promoted_quote(
        &store,
        &key,
        &csrf,
        &customer,
        Locale::Nl,
        &fixture::request("quote"),
        "COURTYARD",
    )
    .await
    .expect("quote")
    .snapshot;
    let intent = order::intent(&quote.original.binding.quote_id);
    pool.execute("CREATE FUNCTION test_verified_order_expiry() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_advisory_xact_lock(1936876911,1); RETURN NEW; END; $$; CREATE TRIGGER z_test_verified_order_expiry AFTER INSERT ON commerce_outbox FOR EACH ROW EXECUTE FUNCTION test_verified_order_expiry()").await.expect("post-write gate");
    let mut gate = pool.begin().await.expect("gate");
    sqlx::query("SELECT pg_advisory_xact_lock(1936876911,1)")
        .execute(&mut *gate)
        .await
        .expect("held");
    let seller = order::seller();
    let work =
        create_verified_promoted_order(&store, &key, &csrf, &customer, "place", &intent, &seller);
    let release = async {
        waiting(&pool, 1).await;
        time.0.store(44300, Ordering::SeqCst);
        gate.rollback().await.expect("release");
    };
    let (result, ()) = tokio::join!(work, release);
    assert!(
        matches!(result, Err(OrderStoreError::Denied)),
        "expired customer session rejects commit before other expiry checks"
    );
    assert_eq!(effects(&pool).await, (0, 0, 0, 0, 0, 0, 0));
    time.0.store(1100, Ordering::SeqCst);
    create_verified_promoted_order(
        &store,
        &key,
        &csrf,
        &customer,
        "place",
        &intent,
        &order::seller(),
    )
    .await
    .expect("positive control");
    assert_eq!(effects(&pool).await, (1, 1, 1, 1, 2, 1, 12));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn native_debit_rejects_account_mismatch_and_per_customer_overrun(pool: PgPool) {
    seed(&pool, false).await;
    let (store, time) = fixture::store(&pool, 1100);
    let (account_id, customer) = account(&pool, "buyer@example.test", &time).await;
    let (other_id, _) = account(&pool, "other@example.test", &time).await;
    let (a, ac) = fixture::guest(&store).await;
    let ak = GuestCredential::from_session(&a).expect("first basket");
    let aq = issue_verified_promoted_quote(
        &store,
        &ak,
        &ac,
        &customer,
        Locale::Nl,
        &fixture::request("a"),
        "COURTYARD",
    )
    .await
    .expect("first quote")
    .snapshot;
    create_verified_promoted_order(
        &store,
        &ak,
        &ac,
        &customer,
        "a",
        &order::intent(&aq.original.binding.quote_id),
        &order::seller(),
    )
    .await
    .expect("first debit");
    let (b, bc) = fixture::guest(&store).await;
    let bk = GuestCredential::from_session(&b).expect("second basket");
    let bq = issue_verified_promoted_quote(
        &store,
        &bk,
        &bc,
        &customer,
        Locale::Nl,
        &fixture::request("b"),
        "COURTYARD",
    )
    .await
    .expect("second quote")
    .snapshot;
    let intent = order::intent(&bq.original.binding.quote_id);
    // Isolated fault runs after Rust's checks but before the database guard.
    pool.execute("ALTER TABLE promotion_policy_drafts DISABLE TRIGGER guard_promotion_draft; CREATE TABLE verified_order_fault(mode text, other_account bigint); CREATE FUNCTION test_verified_debit_fault() RETURNS trigger LANGUAGE plpgsql AS $$ DECLARE f verified_order_fault%ROWTYPE; BEGIN SELECT * INTO f FROM verified_order_fault; IF f.mode='account' THEN NEW.promotion_account_id=f.other_account; ELSIF f.mode='null' THEN NEW.promotion_account_id=NULL; ELSIF f.mode='limit' THEN UPDATE promotion_policy_drafts SET policy=jsonb_set(policy,'{limits,per_customer_redemptions}','1') WHERE id=NEW.promotion_id AND revision=NEW.promotion_revision; ELSIF f.mode='sequence' THEN NEW.account_redemption_number=999; END IF; RETURN NEW; END; $$; CREATE TRIGGER aaa_test_verified_debit_fault BEFORE INSERT ON promotion_redemptions FOR EACH ROW EXECUTE FUNCTION test_verified_debit_fault()").await.expect("native fault instrument");
    sqlx::query("INSERT INTO verified_order_fault VALUES('account',$1)")
        .bind(other_id)
        .execute(&pool)
        .await
        .expect("fault");
    let before = effects(&pool).await;
    for mode in ["account", "null", "limit"] {
        sqlx::query("UPDATE verified_order_fault SET mode=$1")
            .bind(mode)
            .execute(&pool)
            .await
            .expect("fault mode");
        assert!(
            matches!(
                create_verified_promoted_order(
                    &store,
                    &bk,
                    &bc,
                    &customer,
                    "b",
                    &intent,
                    &order::seller()
                )
                .await,
                Err(OrderStoreError::Database)
            ),
            "native guard rejects {mode}"
        );
        assert_eq!(effects(&pool).await, before);
    }
    pool.execute("UPDATE verified_order_fault SET mode='sequence'")
        .await
        .expect("forged sequence");
    create_verified_promoted_order(&store, &bk, &bc, &customer, "b", &intent, &order::seller())
        .await
        .expect("valid debit and derived sequence");
    let rows: Vec<(Option<i64>, Option<i64>)> = sqlx::query_as("SELECT promotion_account_id,account_redemption_number FROM promotion_redemptions ORDER BY redemption_number").fetch_all(&pool).await.expect("account ledger");
    assert_eq!(
        rows,
        vec![(Some(account_id), Some(1)), (Some(account_id), Some(2))]
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn verified_order_rejects_unbound_original_and_promoted_quotes(pool: PgPool) {
    seed(&pool, false).await;
    let (store, time) = fixture::store(&pool, 1100);
    let (_, customer) = account(&pool, "buyer@example.test", &time).await;

    let (legacy_session, legacy_csrf) = fixture::guest(&store).await;
    let legacy_key = GuestCredential::from_session(&legacy_session).expect("legacy basket");
    let legacy = issue_quote(
        &store,
        &legacy_key,
        &legacy_csrf,
        Locale::Nl,
        &fixture::request("legacy"),
    )
    .await
    .expect("unbound original quote")
    .snapshot;
    let legacy_intent = order::intent(&legacy.binding.quote_id);
    assert!(matches!(
        create_verified_promoted_order(
            &store,
            &legacy_key,
            &legacy_csrf,
            &customer,
            "legacy-order",
            &legacy_intent,
            &order::seller()
        )
        .await,
        Err(OrderStoreError::QuoteNotFound)
    ));
    assert!(
        create_order(
            &store,
            &legacy_key,
            &legacy_csrf,
            "legacy-order",
            &legacy_intent,
            &order::seller()
        )
        .await
        .is_ok(),
        "guest can still accept original quote"
    );

    let (guest_session, guest_csrf) = fixture::guest(&store).await;
    let guest_key = GuestCredential::from_session(&guest_session).expect("promoted basket");
    let guest = issue_promoted_quote(
        &store,
        &guest_key,
        &guest_csrf,
        Locale::Nl,
        &fixture::request("guest-promo"),
        "COURTYARD",
    )
    .await
    .expect("unbound promoted quote")
    .snapshot;
    let guest_intent = order::intent(&guest.original.binding.quote_id);
    let before = effects(&pool).await;
    assert!(matches!(
        create_verified_promoted_order(
            &store,
            &guest_key,
            &guest_csrf,
            &customer,
            "promoted-order",
            &guest_intent,
            &order::seller()
        )
        .await,
        Err(OrderStoreError::QuoteNotFound)
    ));
    assert_eq!(effects(&pool).await, before, "denial creates no effects");
    assert!(
        create_order(
            &store,
            &guest_key,
            &guest_csrf,
            "promoted-order",
            &guest_intent,
            &order::seller()
        )
        .await
        .is_ok(),
        "guest can still accept promoted quote"
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn altered_consistent_order_readback_rolls_back_all_effects(pool: PgPool) {
    seed(&pool, true).await;
    let (store, time) = fixture::store(&pool, 1100);
    let (_, customer) = account(&pool, "buyer@example.test", &time).await;
    let (session, csrf) = fixture::guest(&store).await;
    let key = GuestCredential::from_session(&session).expect("basket");
    let quote = issue_verified_promoted_quote(
        &store,
        &key,
        &csrf,
        &customer,
        Locale::Nl,
        &fixture::request("readback"),
        "COURTYARD",
    )
    .await
    .expect("verified quote")
    .snapshot;
    let intent = order::intent(&quote.original.binding.quote_id);
    let before = effects(&pool).await;
    // Simulate a coherent corruption after the real order/event write. Both buyer
    // fields remain structurally valid and agree, so stored decoding succeeds;
    // only the comparison with the exact freshly constructed snapshot catches it.
    pool.execute("ALTER TABLE checkout_orders DISABLE TRIGGER guard_checkout_order; CREATE FUNCTION test_order_readback_fault() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.kind='checkout.order_created.v1' THEN UPDATE checkout_orders SET snapshot=jsonb_set(snapshot,'{buyer,name}',to_jsonb('Altered Buyer'::text)), intent=jsonb_set(intent,'{buyer,name}',to_jsonb('Altered Buyer'::text)) WHERE id=NEW.aggregate_id; END IF; RETURN NEW; END; $$; CREATE TRIGGER z_test_order_readback_fault AFTER INSERT ON commerce_outbox FOR EACH ROW EXECUTE FUNCTION test_order_readback_fault()")
        .await.expect("isolated coherent corruption");
    assert!(matches!(
        create_verified_promoted_order(
            &store,
            &key,
            &csrf,
            &customer,
            "place",
            &intent,
            &order::seller()
        )
        .await,
        Err(OrderStoreError::InvalidStoredData)
    ));
    assert_eq!(
        effects(&pool).await,
        before,
        "order, event, debit and holds rolled back"
    );
    pool.execute("DROP TRIGGER z_test_order_readback_fault ON commerce_outbox; ALTER TABLE checkout_orders ENABLE TRIGGER guard_checkout_order")
        .await.expect("restore guards");
    let accepted = create_verified_promoted_order(
        &store,
        &key,
        &csrf,
        &customer,
        "place",
        &intent,
        &order::seller(),
    )
    .await
    .expect("positive control with native guard restored");
    assert!(!accepted.replayed);
    assert_eq!(accepted.snapshot.buyer.name, intent.buyer.name);
    assert_eq!(effects(&pool).await, (1, 1, 1, 1, 2, 1, 12));
}
