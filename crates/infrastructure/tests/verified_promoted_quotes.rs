//! Private verified-customer indicative quotes over isolated PostgreSQL.
#[allow(dead_code)]
#[path = "support/admitted_login.rs"]
mod admitted_login;
#[allow(dead_code)]
#[path = "support/admitted_proofs.rs"]
mod admitted_proofs;
#[allow(dead_code)]
#[path = "support/checkout_quote_fixture.rs"]
mod fixture;
#[allow(dead_code)]
#[path = "support/checkout_order_fixture.rs"]
mod order;
#[allow(dead_code)]
#[path = "support/promotion_policy_fixture.rs"]
mod promotion;

use admitted_proofs::{
    customer_passwords, email_verification, verified_customer_accounts::establish,
};
use commerce_application::Locale;
use commerce_infrastructure::{
    MIGRATOR,
    checkout_orders::create_order,
    checkout_quotes::{
        QuoteStoreError, issue_promoted_quote, issue_quote, issue_verified_promoted_quote,
        owned_promoted_quote, owned_verified_promoted_quote,
    },
    customer_sessions::{self, CustomerSessionToken},
    guest_baskets::{GuestCredential, GuestError, revoke_session},
};
use sqlx::{Executor, PgPool};
use std::sync::{Arc, atomic::Ordering};
use tower_sessions::Session;

async fn seed(pool: &PgPool) {
    fixture::seed(pool).await;
    pool.execute("INSERT INTO staff_capabilities VALUES('author','promotion.manage'),('reviewer','promotion.review')").await.expect("independent grants");
    let mut policy = promotion::policy();
    policy.limits.per_customer_redemptions = Some(1);
    promotion::draft(pool, &policy, "customer-draft")
        .await
        .expect("customer-capped policy");
    promotion::decide(pool, "customer-review", 1, 0, true)
        .await
        .expect("independent review");
}

async fn account(pool: &PgPool, email: &str, time: &fixture::Time) -> (i64, CustomerSessionToken) {
    let proof = email_verification::issue(pool, email, time)
        .await
        .expect("email proof");
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

async fn effects(pool: &PgPool) -> (i64, i64, i64, i64, i64, i64) {
    sqlx::query_as("SELECT (SELECT count(*) FROM guest_quote_snapshots),(SELECT coalesce(sum(issued),0)::bigint FROM guest_quote_quota),(SELECT count(*) FROM checkout_orders),(SELECT count(*) FROM promotion_redemptions),(SELECT count(*) FROM stock_holds),(SELECT count(*) FROM stock_movements)").fetch_one(pool).await.expect("effect counts")
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn capped_quote_requires_both_current_owners_and_has_no_checkout_effect(pool: PgPool) {
    seed(&pool).await;
    let (store, time) = fixture::store(&pool, 1100);
    let (account_id, customer) = account(&pool, "buyer@example.test", &time).await;
    let (_, other_customer) = account(&pool, "other@example.test", &time).await;
    let (session, csrf) = fixture::guest(&store).await;
    let (other_session, other_csrf) = fixture::guest(&store).await;
    let key = GuestCredential::from_session(&session).expect("guest");
    let other_key = GuestCredential::from_session(&other_session).expect("other guest");
    let missing = CustomerSessionToken::from_bytes([42; 32]);
    let request = fixture::request("verified-capped");
    let empty_guest = Session::new(None, Arc::new(store.clone()), None);
    assert!(GuestCredential::from_session(&empty_guest).is_err());
    let stock: serde_json::Value = sqlx::query_scalar(
        "SELECT jsonb_agg(to_jsonb(s) ORDER BY component_code) FROM stock_balances s",
    )
    .fetch_one(&pool)
    .await
    .expect("stock baseline");
    assert!(
        issue_promoted_quote(&store, &key, &csrf, Locale::Nl, &request, "COURTYARD")
            .await
            .is_err(),
        "legacy guest cannot satisfy customer limit"
    );
    assert!(
        issue_verified_promoted_quote(
            &store,
            &key,
            &csrf,
            &missing,
            Locale::Nl,
            &request,
            "COURTYARD"
        )
        .await
        .is_err()
    );
    assert!(
        issue_verified_promoted_quote(
            &store,
            &key,
            &other_csrf,
            &customer,
            Locale::Nl,
            &request,
            "COURTYARD"
        )
        .await
        .is_err(),
        "another guest's CSRF token cannot authorize a quote"
    );
    assert_eq!(effects(&pool).await, (0, 0, 0, 0, 0, 0));
    let receipt = issue_verified_promoted_quote(
        &store,
        &key,
        &csrf,
        &customer,
        Locale::Nl,
        &request,
        "COURTYARD",
    )
    .await
    .expect("verified capped quote");
    assert!(!receipt.replayed);
    let quote = receipt.snapshot;
    let id = &quote.original.binding.quote_id;
    assert_eq!(
        (
            quote.original.total.gross_minor,
            quote.discount_minor,
            quote.total.gross_minor
        ),
        (38245, 500, 37745)
    );
    let bound: Option<i64> =
        sqlx::query_scalar("SELECT promotion_account_id FROM guest_quote_snapshots WHERE id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .expect("account association");
    assert_eq!(bound, Some(account_id));
    assert!(
        owned_verified_promoted_quote(&store, &key, &customer, id)
            .await
            .expect("joint owner read")
            == quote
    );
    let replay = issue_verified_promoted_quote(
        &store,
        &key,
        &csrf,
        &customer,
        Locale::Nl,
        &request,
        "COURTYARD",
    )
    .await
    .expect("exact replay");
    assert!(replay.replayed && replay.snapshot == quote);
    assert!(
        owned_verified_promoted_quote(&store, &key, &other_customer, id)
            .await
            .is_err(),
        "same guest with different account denied"
    );
    assert!(
        owned_verified_promoted_quote(&store, &other_key, &customer, id)
            .await
            .is_err(),
        "other guest with same account denied"
    );
    assert!(
        owned_verified_promoted_quote(&store, &key, &missing, id)
            .await
            .is_err()
    );
    assert!(
        issue_verified_promoted_quote(
            &store,
            &key,
            &csrf,
            &other_customer,
            Locale::Nl,
            &request,
            "COURTYARD"
        )
        .await
        .is_err(),
        "other account cannot replay"
    );
    assert!(
        owned_promoted_quote(&store, &key, id).await.is_err(),
        "legacy guest read cannot access account-bound quote"
    );
    assert!(
        issue_promoted_quote(&store, &key, &csrf, Locale::Nl, &request, "COURTYARD")
            .await
            .is_err(),
        "legacy guest cannot replay account-bound quote"
    );
    assert!(
        issue_quote(&store, &key, &csrf, Locale::Nl, &request)
            .await
            .is_err(),
        "effect namespace shared with v1"
    );
    let update =
        sqlx::query("UPDATE guest_quote_snapshots SET promotion_account_id=NULL WHERE id=$1")
            .bind(id)
            .execute(&pool)
            .await;
    assert!(update.is_err(), "native quote association is immutable");
    assert!(
        create_order(
            &store,
            &key,
            &csrf,
            "guest-order",
            &order::intent(id),
            &order::seller()
        )
        .await
        .is_err(),
        "guest checkout cannot accept account-bound quote"
    );
    assert_eq!(effects(&pool).await, (1, 1, 0, 0, 0, 0));
    let after: serde_json::Value = sqlx::query_scalar(
        "SELECT jsonb_agg(to_jsonb(s) ORDER BY component_code) FROM stock_balances s",
    )
    .fetch_one(&pool)
    .await
    .expect("stock unchanged");
    assert_eq!(after, stock);
    revoke_session(&store, &session, &csrf)
        .await
        .expect("revoke guest session");
    assert!(
        owned_verified_promoted_quote(&store, &key, &customer, id)
            .await
            .is_err()
    );
    assert!(
        issue_verified_promoted_quote(
            &store,
            &key,
            &csrf,
            &customer,
            Locale::Nl,
            &request,
            "COURTYARD"
        )
        .await
        .is_err()
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn rotation_revocation_and_shared_quota_apply_to_customer_quotes(pool: PgPool) {
    seed(&pool).await;
    let (store, time) = fixture::store(&pool, 1100);
    let (_, customer) = account(&pool, "buyer@example.test", &time).await;
    let (session, csrf) = fixture::guest(&store).await;
    let key = GuestCredential::from_session(&session).expect("guest");
    let request = fixture::request("customer-first");
    let quote = issue_verified_promoted_quote(
        &store,
        &key,
        &csrf,
        &customer,
        Locale::Nl,
        &request,
        "COURTYARD",
    )
    .await
    .expect("customer quote")
    .snapshot;
    let successor = customer_sessions::rotate(&pool, &customer, time.as_ref())
        .await
        .expect("rotation")
        .token;
    assert!(
        owned_verified_promoted_quote(&store, &key, &customer, &quote.original.binding.quote_id)
            .await
            .is_err()
    );
    assert!(
        issue_verified_promoted_quote(
            &store,
            &key,
            &csrf,
            &customer,
            Locale::Nl,
            &request,
            "COURTYARD"
        )
        .await
        .is_err()
    );
    assert!(
        owned_verified_promoted_quote(&store, &key, &successor, &quote.original.binding.quote_id)
            .await
            .is_ok()
    );
    for n in 0..9 {
        issue_quote(
            &store,
            &key,
            &csrf,
            Locale::Nl,
            &fixture::request(&format!("legacy-{n}")),
        )
        .await
        .expect("shared quota");
    }
    assert!(matches!(
        issue_verified_promoted_quote(
            &store,
            &key,
            &csrf,
            &successor,
            Locale::Nl,
            &fixture::request("over"),
            "COURTYARD"
        )
        .await,
        Err(QuoteStoreError::RateLimited)
    ));
    assert!(
        issue_verified_promoted_quote(
            &store,
            &key,
            &csrf,
            &successor,
            Locale::Nl,
            &request,
            "COURTYARD"
        )
        .await
        .expect("replay after quota")
        .replayed
    );
    customer_sessions::logout(&pool, &successor, time.as_ref())
        .await
        .expect("revoke");
    assert!(
        owned_verified_promoted_quote(&store, &key, &successor, &quote.original.binding.quote_id)
            .await
            .is_err()
    );
    assert!(
        issue_verified_promoted_quote(
            &store,
            &key,
            &csrf,
            &successor,
            Locale::Nl,
            &request,
            "COURTYARD"
        )
        .await
        .is_err()
    );
    assert_eq!(effects(&pool).await, (10, 10, 0, 0, 0, 0));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn session_expiry_and_clock_reversal_after_quote_write_roll_back(pool: PgPool) {
    seed(&pool).await;
    let (store, time) = fixture::store(&pool, 1100);
    let (_, customer) = account(&pool, "buyer@example.test", &time).await;
    let (session, csrf) = fixture::guest(&store).await;
    let key = GuestCredential::from_session(&session).expect("guest");
    pool.execute("CREATE FUNCTION verified_quote_test_wait() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_advisory_xact_lock(1936876905,1); RETURN NEW; END; $$; CREATE TRIGGER z_verified_quote_test_wait AFTER INSERT ON guest_quote_snapshots FOR EACH ROW EXECUTE FUNCTION verified_quote_test_wait()").await.expect("post-write timing instrument");
    for mode in ["expired", "reversed", "positive"] {
        time.0.store(1100, Ordering::SeqCst);
        let mut blocker = pool.begin().await.expect("wait blocker");
        sqlx::query("SELECT pg_advisory_xact_lock(1936876905,1)")
            .execute(&mut *blocker)
            .await
            .expect("hold gate");
        let request = fixture::request(mode);
        let work = issue_verified_promoted_quote(
            &store,
            &key,
            &csrf,
            &customer,
            Locale::Nl,
            &request,
            "COURTYARD",
        );
        let release = async {
            tokio::time::timeout(std::time::Duration::from_millis(800), async {
                loop {
                    let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND wait_event='advisory' AND query LIKE '%INSERT INTO guest_quote_snapshots%')").fetch_one(&pool).await.expect("inspect post-insert wait");
                    if waiting { break; }
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                }
            }).await.expect("actual write reached trigger wait");
            if mode == "expired" {
                time.0.store(44300, Ordering::SeqCst);
            }
            if mode == "reversed" {
                time.0.store(1099, Ordering::SeqCst);
            }
            blocker.rollback().await.expect("resume");
        };
        let (result, ()) = tokio::join!(work, release);
        if mode == "positive" {
            assert!(result.is_ok(), "instrument admits an eligible quote");
            assert_eq!(effects(&pool).await, (1, 1, 0, 0, 0, 0));
        } else {
            if mode == "expired" {
                // The guest lives for seven days, so it remains valid at 44300;
                // the twelve-hour customer session reaches its exact deadline.
                assert!(matches!(
                    result,
                    Err(QuoteStoreError::Guest(GuestError::Denied))
                ));
            } else {
                assert!(matches!(
                    result,
                    Err(QuoteStoreError::Guest(GuestError::ClockUnavailable))
                ));
            }
            assert_eq!(
                effects(&pool).await,
                (0, 0, 0, 0, 0, 0),
                "written quote and quota rolled back"
            );
        }
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn logout_waits_for_inflight_customer_quote_then_revokes_access(pool: PgPool) {
    seed(&pool).await;
    let (store, time) = fixture::store(&pool, 1100);
    let (account_id, customer) = account(&pool, "buyer@example.test", &time).await;
    let (session, csrf) = fixture::guest(&store).await;
    let key = GuestCredential::from_session(&session).expect("guest");
    pool.execute("CREATE FUNCTION verified_quote_lock_wait() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_advisory_xact_lock(1936876906,1); RETURN NEW; END; $$; CREATE TRIGGER z_verified_quote_lock_wait AFTER INSERT ON guest_quote_snapshots FOR EACH ROW EXECUTE FUNCTION verified_quote_lock_wait()").await.expect("post-insert gate");
    let mut blocker = pool.begin().await.expect("gate transaction");
    sqlx::query("SELECT pg_advisory_xact_lock(1936876906,1)")
        .execute(&mut *blocker)
        .await
        .expect("hold gate");
    let work_store = store.clone();
    let work_key = GuestCredential::from_session(&session).expect("worker guest");
    let work_customer = CustomerSessionToken::from_bytes(*customer.expose_bytes());
    let quote_task = tokio::spawn(async move {
        issue_verified_promoted_quote(
            &work_store,
            &work_key,
            &csrf,
            &work_customer,
            Locale::Nl,
            &fixture::request("logout-race"),
            "COURTYARD",
        )
        .await
    });
    tokio::time::timeout(std::time::Duration::from_millis(800), async {
        loop {
            let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND wait_event='advisory' AND query LIKE '%INSERT INTO guest_quote_snapshots%')").fetch_one(&pool).await.expect("inspect quote gate");
            if waiting { break; }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }).await.expect("quote reached post-insert gate");
    let logout_pool = pool.clone();
    let logout_time = time.clone();
    let logout_customer = CustomerSessionToken::from_bytes(*customer.expose_bytes());
    let logout_task = tokio::spawn(async move {
        customer_sessions::logout(&logout_pool, &logout_customer, logout_time.as_ref()).await
    });
    tokio::time::timeout(std::time::Duration::from_millis(800), async {
        loop {
            let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'SELECT verified_at FROM verified_customer_accounts WHERE id=$1 FOR UPDATE%')").fetch_one(&pool).await.expect("inspect competing account lock");
            if waiting { break; }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }).await.expect("logout waited on account held by quote");
    assert!(!logout_task.is_finished());
    blocker.rollback().await.expect("release quote gate");
    let quote = quote_task
        .await
        .expect("quote task")
        .expect("quote commits while session held")
        .snapshot;
    logout_task
        .await
        .expect("logout task")
        .expect("logout completes after quote");
    let id = &quote.original.binding.quote_id;
    assert!(
        owned_verified_promoted_quote(&store, &key, &customer, id)
            .await
            .is_err(),
        "revoked session cannot read"
    );
    let current = admitted_login::login(
        &pool,
        "buyer@example.test",
        "correct horse battery staple",
        time.as_ref(),
    )
    .await
    .expect("fresh session")
    .token;
    assert!(
        owned_verified_promoted_quote(&store, &key, &current, id)
            .await
            .expect("new session reads committed quote")
            == quote
    );
    let bound: Option<i64> =
        sqlx::query_scalar("SELECT promotion_account_id FROM guest_quote_snapshots WHERE id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .expect("preserved association");
    assert_eq!(bound, Some(account_id));
    assert!(
        sqlx::query("UPDATE guest_quote_snapshots SET promotion_account_id=NULL WHERE id=$1")
            .bind(id)
            .execute(&pool)
            .await
            .is_err(),
        "committed quote remains immutable"
    );
    assert_eq!(effects(&pool).await, (1, 1, 0, 0, 0, 0));
}

#[sqlx::test(migrations = false)]
async fn populated_schema52_upgrade_keeps_legacy_quote_reads_and_replays(pool: PgPool) {
    MIGRATOR.run_to(52, &pool).await.expect("schema 52");
    fixture::seed(&pool).await;
    pool.execute("INSERT INTO staff_capabilities VALUES('author','promotion.manage'),('reviewer','promotion.review')").await.expect("independent grants");
    promotion::draft(&pool, &promotion::policy(), "upgrade-draft")
        .await
        .expect("guest policy");
    promotion::decide(&pool, "upgrade-review", 1, 0, true)
        .await
        .expect("review");
    let (store, _) = fixture::store(&pool, 1100);
    let (session, csrf) = fixture::guest(&store).await;
    let key = GuestCredential::from_session(&session).expect("owner");
    let legacy_request = fixture::request("upgrade-v1");
    let promoted_request = fixture::request("upgrade-v2");
    let legacy = issue_quote(&store, &key, &csrf, Locale::Nl, &legacy_request)
        .await
        .expect("schema 52 v1")
        .snapshot;
    let promoted = issue_promoted_quote(
        &store,
        &key,
        &csrf,
        Locale::Nl,
        &promoted_request,
        "COURTYARD",
    )
    .await
    .expect("schema 52 v2")
    .snapshot;
    MIGRATOR
        .run(&pool)
        .await
        .expect("upgrade to current schema");
    let accounts: Vec<Option<i64>> = sqlx::query_scalar(
        "SELECT promotion_account_id FROM guest_quote_snapshots ORDER BY effect_key",
    )
    .fetch_all(&pool)
    .await
    .expect("legacy rows have no account");
    assert_eq!(accounts, vec![None, None]);
    assert!(
        commerce_infrastructure::checkout_quotes::owned_quote(
            &store,
            &key,
            &legacy.binding.quote_id
        )
        .await
        .expect("upgraded v1 read")
            == legacy
    );
    assert!(
        owned_promoted_quote(&store, &key, &promoted.original.binding.quote_id)
            .await
            .expect("upgraded v2 read")
            == promoted
    );
    assert!(
        issue_quote(&store, &key, &csrf, Locale::Nl, &legacy_request)
            .await
            .expect("upgraded v1 replay")
            .replayed
    );
    assert!(
        issue_promoted_quote(
            &store,
            &key,
            &csrf,
            Locale::Nl,
            &promoted_request,
            "COURTYARD"
        )
        .await
        .expect("upgraded v2 replay")
        .replayed
    );
    assert_eq!(effects(&pool).await, (2, 2, 0, 0, 0, 0));
}

#[sqlx::test(migrations = false)]
async fn schema53_native_checkout_rejects_account_bound_quotes_but_accepts_guest(pool: PgPool) {
    // Pin this check to the migration that installs the blocker; later migrations
    // intentionally replace it when account-aware redemption is implemented.
    MIGRATOR.run_to(53, &pool).await.expect("schema 53");
    fixture::seed(&pool).await;
    pool.execute("INSERT INTO staff_capabilities VALUES('author','promotion.manage'),('reviewer','promotion.review')").await.expect("independent promotion grants");
    let mut policy = promotion::policy();
    promotion::draft(&pool, &policy, "uncapped-draft")
        .await
        .expect("uncapped draft");
    promotion::decide(&pool, "uncapped-review", 1, 0, true)
        .await
        .expect("uncapped approval");
    let (store, time) = fixture::store(&pool, 1100);
    let (_, customer) = account(&pool, "buyer@example.test", &time).await;
    let (guest, guest_csrf) = fixture::guest(&store).await;
    let guest_key = GuestCredential::from_session(&guest).expect("guest");
    let guest_quote = issue_promoted_quote(
        &store,
        &guest_key,
        &guest_csrf,
        Locale::Nl,
        &fixture::request("guest-quote"),
        "COURTYARD",
    )
    .await
    .expect("guest promoted quote")
    .snapshot;
    create_order(
        &store,
        &guest_key,
        &guest_csrf,
        "guest-order",
        &order::intent(&guest_quote.original.binding.quote_id),
        &order::seller(),
    )
    .await
    .expect("uncapped guest order remains valid");
    let (session, csrf) = fixture::guest(&store).await;
    let key = GuestCredential::from_session(&session).expect("account basket");
    for capped in [false, true] {
        if capped {
            policy.promotion.revision = 2;
            policy.limits.per_customer_redemptions = Some(1);
            promotion::draft(&pool, &policy, "capped-draft")
                .await
                .expect("capped draft");
            promotion::decide(&pool, "capped-review", 2, 0, true)
                .await
                .expect("capped approval");
        }
        let quote = issue_verified_promoted_quote(
            &store,
            &key,
            &csrf,
            &customer,
            Locale::Nl,
            &fixture::request(if capped {
                "capped-quote"
            } else {
                "uncapped-quote"
            }),
            "COURTYARD",
        )
        .await
        .expect("verified account quote")
        .snapshot;
        let binding = &quote.original.binding;
        // Directly exercise the native trigger. Dummy non-key fields cannot
        // satisfy checkout; the precise error proves the account guard ran first.
        let error = sqlx::query("INSERT INTO checkout_orders(id,basket_id,basket_revision,quote_id,effect_key,stock_group_id,created_at,hold_expires_at,intent,snapshot) VALUES($1,$2,$3,$4,'native-probe',$5,1100,2000,'{}','{}')")
            .bind("ffffffffffffffffffffffffffffffff")
            .bind(&binding.basket_id)
            .bind(binding.basket_revision)
            .bind(&binding.quote_id)
            .bind("eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee")
            .execute(&pool)
            .await
            .expect_err("native guest checkout must reject account-bound quote");
        let sqlx::Error::Database(error) = error else {
            panic!("expected PostgreSQL guard error")
        };
        assert_eq!(error.code().as_deref(), Some("23514"));
        assert_eq!(
            error.message(),
            "Account-bound quote requires account-aware checkout"
        );
    }
    let counts: (i64, i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM checkout_orders),(SELECT count(*) FROM promotion_redemptions),(SELECT count(*) FROM commerce_outbox WHERE kind='checkout.order_created.v1')").fetch_one(&pool).await.expect("only guest checkout effects");
    assert_eq!(counts, (1, 1, 1));
}
