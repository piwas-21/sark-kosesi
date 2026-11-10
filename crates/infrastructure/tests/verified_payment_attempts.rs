//! Private account-bound test payment custody over isolated PostgreSQL. No provider I/O.
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
use commerce_domain::checkout_orders::OrderSnapshot;
use commerce_infrastructure::{
    MIGRATOR,
    checkout_orders::{OrderStoreError, create_verified_promoted_order},
    checkout_quotes::{issue_quote, issue_verified_promoted_quote},
    customer_sessions::{self, CustomerSessionToken},
    guest_baskets::{GuestCredential, GuestSessionStore},
    guest_order_links,
    payment_attempts::{
        DispatchOutcome, PaymentStoreError,
        identities::resolve,
        owned_attempt, owned_verified_attempt, prepare_attempt, prepare_verified_attempt,
        recognition::{State, recognize},
        start_dispatch,
    },
};
use payment::{order, promotion, quote};
use sqlx::{Executor, PgPool};
use std::sync::{Arc, atomic::Ordering};
use tower_sessions::Session;

async fn account(pool: &PgPool, email: &str, time: &quote::Time) -> (i64, CustomerSessionToken) {
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

async fn setup(
    pool: &PgPool,
) -> (
    GuestSessionStore,
    Arc<quote::Time>,
    Session,
    String,
    i64,
    CustomerSessionToken,
    OrderSnapshot,
) {
    quote::seed(pool).await;
    pool.execute("INSERT INTO staff_capabilities VALUES('author','promotion.manage'),('reviewer','promotion.review')").await.expect("promotion grants");
    let mut policy = promotion::policy();
    policy.limits.per_customer_redemptions = Some(1);
    promotion::draft(pool, &policy, "draft")
        .await
        .expect("draft");
    promotion::decide(pool, "review", 1, 0, true)
        .await
        .expect("independent review");
    let (store, time) = quote::store(pool, 1100);
    let (id, customer) = account(pool, "buyer@example.test", &time).await;
    let (session, csrf) = quote::guest(&store).await;
    let key = GuestCredential::from_session(&session).expect("basket owner");
    let quoted = issue_verified_promoted_quote(
        &store,
        &key,
        &csrf,
        &customer,
        Locale::Nl,
        &quote::request("quote"),
        "COURTYARD",
    )
    .await
    .expect("verified quote")
    .snapshot;
    let placed = create_verified_promoted_order(
        &store,
        &key,
        &csrf,
        &customer,
        "place",
        &order::intent(&quoted.original.binding.quote_id),
        &order::seller(),
    )
    .await
    .expect("verified order")
    .snapshot;
    (store, time, session, csrf, id, customer, placed)
}

async fn effects(pool: &PgPool) -> (i64, i64, i64, i64, i64, i64) {
    sqlx::query_as("SELECT (SELECT count(*) FROM payment_attempts),(SELECT count(*) FROM commerce_outbox),(SELECT count(*) FROM checkout_orders),(SELECT count(*) FROM basket_hold_groups),(SELECT count(*) FROM stock_holds),(SELECT count(*) FROM promotion_redemptions)").fetch_one(pool).await.expect("custody effects")
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn verified_attempt_preserves_payable_and_requires_both_current_owners(pool: PgPool) {
    let (store, time, session, csrf, account_id, customer, placed) = setup(&pool).await;
    let (_, other_customer) = account(&pool, "other@example.test", &time).await;
    let (other_session, other_csrf) = quote::guest(&store).await;
    let key = GuestCredential::from_session(&session).expect("owner");
    let other_key = GuestCredential::from_session(&other_session).expect("other owner");
    let invalid = CustomerSessionToken::from_bytes([42; 32]);
    let intent = payment::intent(&placed);
    let config = payment::config();
    assert!(matches!(
        prepare_verified_attempt(
            &store,
            &key,
            &csrf,
            &other_customer,
            "pay",
            &intent,
            &config
        )
        .await,
        Err(PaymentStoreError::Boundary(OrderStoreError::NotFound))
    ));
    assert!(
        prepare_verified_attempt(&store, &key, &csrf, &invalid, "pay", &intent, &config)
            .await
            .is_err()
    );
    assert!(
        prepare_verified_attempt(
            &store,
            &other_key,
            &other_csrf,
            &customer,
            "pay",
            &intent,
            &config
        )
        .await
        .is_err()
    );
    assert!(
        matches!(
            prepare_attempt(&store, &key, &csrf, "pay", &intent, &config).await,
            Err(PaymentStoreError::Boundary(OrderStoreError::NotFound))
        ),
        "guest cannot prepare a bound order"
    );
    assert_eq!(effects(&pool).await, (0, 1, 1, 1, 1, 1));
    let receipt = prepare_verified_attempt(&store, &key, &csrf, &customer, "pay", &intent, &config)
        .await
        .expect("verified payment preparation");
    assert!(!receipt.replayed);
    let snapshot = receipt.frozen.snapshot();
    assert_eq!(
        (
            snapshot.binding.amount_minor,
            snapshot.binding.created_at,
            snapshot.binding.dispatch_before
        ),
        (37745, 1100, 2000)
    );
    assert_eq!(
        snapshot.binding.stock_group_id,
        placed.identity.stock_group_id
    );
    assert_eq!(snapshot.binding.order_id, placed.identity.id);
    let body: serde_json::Value = serde_json::from_str(receipt.frozen.body()).expect("body");
    assert_eq!(
        body["amount"],
        serde_json::json!({"currency":"EUR","value":"377.45"})
    );
    let stored: Option<i64> =
        sqlx::query_scalar("SELECT promotion_account_id FROM payment_attempts WHERE id=$1")
            .bind(&snapshot.binding.id)
            .fetch_one(&pool)
            .await
            .expect("persisted association");
    assert_eq!(stored, Some(account_id));
    let replay = prepare_verified_attempt(&store, &key, &csrf, &customer, "pay", &intent, &config)
        .await
        .expect("same owners replay");
    assert!(replay.replayed && replay.frozen.snapshot() == snapshot);
    assert!(
        matches!(
            prepare_attempt(&store, &key, &csrf, "pay", &intent, &config).await,
            Err(PaymentStoreError::Boundary(OrderStoreError::NotFound))
        ),
        "guest cannot replay"
    );
    assert!(
        matches!(
            prepare_verified_attempt(
                &store,
                &key,
                &csrf,
                &other_customer,
                "pay",
                &intent,
                &config
            )
            .await,
            Err(PaymentStoreError::Boundary(OrderStoreError::NotFound))
        ),
        "another account cannot replay"
    );
    assert!(matches!(
        prepare_attempt(&store, &key, &csrf, "alternate", &intent, &config).await,
        Err(PaymentStoreError::Boundary(OrderStoreError::NotFound))
    ));
    assert!(matches!(
        prepare_verified_attempt(
            &store,
            &key,
            &csrf,
            &other_customer,
            "alternate",
            &intent,
            &config
        )
        .await,
        Err(PaymentStoreError::Boundary(OrderStoreError::NotFound))
    ));
    assert!(
        owned_verified_attempt(&store, &key, &customer, &snapshot.binding.id)
            .await
            .is_ok()
    );
    assert!(
        owned_verified_attempt(&store, &other_key, &customer, &snapshot.binding.id)
            .await
            .is_err()
    );
    assert!(matches!(
        owned_verified_attempt(&store, &key, &other_customer, &snapshot.binding.id).await,
        Err(PaymentStoreError::NotFound)
    ));
    assert!(
        owned_verified_attempt(&store, &key, &invalid, &snapshot.binding.id)
            .await
            .is_err()
    );
    assert!(matches!(
        owned_attempt(&store, &key, &snapshot.binding.id).await,
        Err(PaymentStoreError::NotFound)
    ));
    assert!(
        guest_order_links::issue(&store, &key, &csrf, &placed.identity.id)
            .await
            .is_err(),
        "guest linking stays blocked"
    );
    assert_eq!(effects(&pool).await, (1, 2, 1, 1, 1, 1));
    let successor = customer_sessions::rotate(&pool, &customer, time.as_ref())
        .await
        .expect("session rotation")
        .token;
    assert!(
        owned_verified_attempt(&store, &key, &customer, &snapshot.binding.id)
            .await
            .is_err()
    );
    assert!(
        owned_verified_attempt(&store, &key, &successor, &snapshot.binding.id)
            .await
            .is_ok()
    );
    assert!(
        sqlx::query("UPDATE payment_attempts SET promotion_account_id=NULL WHERE id=$1")
            .bind(&snapshot.binding.id)
            .execute(&pool)
            .await
            .is_err(),
        "attempt association is immutable"
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn database_rejects_omitted_wrong_and_extraneous_attempt_accounts(pool: PgPool) {
    let (store, time, session, csrf, account_id, customer, placed) = setup(&pool).await;
    let (other_id, _) = account(&pool, "other@example.test", &time).await;
    let key = GuestCredential::from_session(&session).expect("owner");
    let intent = payment::intent(&placed);
    let config = payment::config();
    // This instrument runs before the native guard, after Rust's authorization.
    pool.execute("CREATE TABLE payment_account_fault(mode text, wrong_account bigint); CREATE FUNCTION test_payment_account_fault() RETURNS trigger LANGUAGE plpgsql AS $$ DECLARE f payment_account_fault%ROWTYPE; BEGIN SELECT * INTO f FROM payment_account_fault; CASE f.mode WHEN 'missing' THEN NEW.promotion_account_id=NULL; WHEN 'wrong' THEN NEW.promotion_account_id=f.wrong_account; WHEN 'extra' THEN NEW.promotion_account_id=f.wrong_account; ELSE NULL; END CASE; RETURN NEW; END; $$; CREATE TRIGGER a_payment_account_fault BEFORE INSERT ON payment_attempts FOR EACH ROW EXECUTE FUNCTION test_payment_account_fault()").await.expect("native fault instrument");
    sqlx::query("INSERT INTO payment_account_fault VALUES('missing',$1)")
        .bind(other_id)
        .execute(&pool)
        .await
        .expect("fault setup");
    let before = effects(&pool).await;
    for mode in ["missing", "wrong"] {
        sqlx::query("UPDATE payment_account_fault SET mode=$1")
            .bind(mode)
            .execute(&pool)
            .await
            .expect("fault mode");
        assert!(
            matches!(
                prepare_verified_attempt(&store, &key, &csrf, &customer, "pay", &intent, &config)
                    .await,
                Err(commerce_infrastructure::payment_attempts::PaymentStoreError::Database)
            ),
            "native guard rejects {mode}"
        );
        assert_eq!(effects(&pool).await, before);
    }
    let (guest, guest_csrf) = quote::guest(&store).await;
    let guest_key = GuestCredential::from_session(&guest).expect("other basket");
    let guest_quote = issue_quote(
        &store,
        &guest_key,
        &guest_csrf,
        Locale::Nl,
        &quote::request("guest-quote"),
    )
    .await
    .expect("guest quote")
    .snapshot;
    let guest_order = commerce_infrastructure::checkout_orders::create_order(
        &store,
        &guest_key,
        &guest_csrf,
        "guest-order",
        &order::intent(&guest_quote.binding.quote_id),
        &order::seller(),
    )
    .await
    .expect("guest order")
    .snapshot;
    let before = effects(&pool).await;
    sqlx::query("UPDATE payment_account_fault SET mode='extra',wrong_account=$1")
        .bind(account_id)
        .execute(&pool)
        .await
        .expect("extra account fault");
    assert!(
        matches!(
            prepare_attempt(
                &store,
                &guest_key,
                &guest_csrf,
                "guest-pay",
                &payment::intent(&guest_order),
                &config
            )
            .await,
            Err(commerce_infrastructure::payment_attempts::PaymentStoreError::Database)
        ),
        "native guard rejects account on guest order"
    );
    assert_eq!(effects(&pool).await, before);
    pool.execute("DROP TRIGGER a_payment_account_fault ON payment_attempts")
        .await
        .expect("remove fault");
    prepare_verified_attempt(&store, &key, &csrf, &customer, "pay", &intent, &config)
        .await
        .expect("verified positive control");
    prepare_attempt(
        &store,
        &guest_key,
        &guest_csrf,
        "guest-pay",
        &payment::intent(&guest_order),
        &config,
    )
    .await
    .expect("guest positive control");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn session_expiry_during_actual_outbox_write_rolls_back_attempt_and_event(pool: PgPool) {
    let (store, time, session, csrf, _, customer, placed) = setup(&pool).await;
    let key = GuestCredential::from_session(&session).expect("guest");
    let intent = payment::intent(&placed);
    let config = payment::config();
    pool.execute("CREATE FUNCTION test_verified_payment_wait() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.kind='payment.attempt_prepared.v1' THEN PERFORM pg_advisory_xact_lock(1936876920,1); END IF; RETURN NEW; END; $$; CREATE TRIGGER z_test_verified_payment_wait AFTER INSERT ON commerce_outbox FOR EACH ROW EXECUTE FUNCTION test_verified_payment_wait()").await.expect("post-write gate");
    let before = effects(&pool).await;
    let mut gate = pool.begin().await.expect("gate");
    sqlx::query("SELECT pg_advisory_xact_lock(1936876920,1)")
        .execute(&mut *gate)
        .await
        .expect("hold gate");
    let work = prepare_verified_attempt(&store, &key, &csrf, &customer, "pay", &intent, &config);
    let release = async {
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND wait_event='advisory' AND query LIKE '%INSERT INTO commerce_outbox%')").fetch_one(&pool).await.expect("inspect actual outbox write");
                if waiting { break; }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        }).await.expect("outbox insert reached gate");
        time.0.store(44300, Ordering::SeqCst);
        gate.rollback().await.expect("release gate");
    };
    let (result, ()) = tokio::join!(work, release);
    assert!(
        matches!(
            result,
            Err(
                commerce_infrastructure::payment_attempts::PaymentStoreError::Boundary(
                    commerce_infrastructure::checkout_orders::OrderStoreError::Denied
                )
            )
        ),
        "expired customer session rejects commit before the hold expiry check"
    );
    assert_eq!(
        effects(&pool).await,
        before,
        "attempt and event both roll back"
    );
    time.0.store(1100, Ordering::SeqCst);
    prepare_verified_attempt(&store, &key, &csrf, &customer, "pay", &intent, &config)
        .await
        .expect("live session positive control");
    assert_eq!(effects(&pool).await, (1, 2, 1, 1, 1, 1));
}

#[sqlx::test(migrations = false)]
async fn populated_schema54_guest_attempt_upgrades_with_null_account_and_replays(pool: PgPool) {
    MIGRATOR.run_to(54, &pool).await.expect("schema 54");
    let (store, _, session, csrf, placed) = payment::accepted(&pool).await;
    let key = GuestCredential::from_session(&session).expect("guest");
    let config = payment::config();
    let intent = payment::intent(&placed);
    let original = prepare_attempt(&store, &key, &csrf, "pay", &intent, &config)
        .await
        .expect("schema 54 guest attempt")
        .frozen;
    MIGRATOR.run(&pool).await.expect("upgrade to schema 55");
    let account: Option<i64> =
        sqlx::query_scalar("SELECT promotion_account_id FROM payment_attempts WHERE id=$1")
            .bind(&original.snapshot().binding.id)
            .fetch_one(&pool)
            .await
            .expect("upgraded marker");
    assert_eq!(account, None);
    assert!(
        owned_attempt(&store, &key, &original.snapshot().binding.id)
            .await
            .expect("upgraded read")
            .snapshot()
            == original.snapshot()
    );
    let replay = prepare_attempt(&store, &key, &csrf, "pay", &intent, &config)
        .await
        .expect("upgraded replay");
    assert!(replay.replayed && replay.frozen.snapshot() == original.snapshot());
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn damaged_account_marker_blocks_read_and_worker_then_logout_keeps_durable_recovery(
    pool: PgPool,
) {
    let (store, time, session, csrf, account_id, customer, order) = setup(&pool).await;
    let key = GuestCredential::from_session(&session).expect("basket owner");
    let config = payment::config();
    let receipt = prepare_verified_attempt(
        &store,
        &key,
        &csrf,
        &customer,
        "pay",
        &payment::intent(&order),
        &config,
    )
    .await
    .expect("admitted attempt");
    let attempt_id = &receipt.frozen.snapshot().binding.id;
    let ids = payment::Ids(std::sync::atomic::AtomicU64::new(10000));
    pool.execute("ALTER TABLE payment_attempts DISABLE TRIGGER guard_payment_attempt")
        .await
        .expect("isolate damaged import");
    sqlx::query("UPDATE payment_attempts SET promotion_account_id=NULL WHERE id=$1")
        .bind(attempt_id)
        .execute(&pool)
        .await
        .expect("simulate detached account marker");
    assert!(matches!(
        owned_verified_attempt(&store, &key, &customer, attempt_id).await,
        Err(PaymentStoreError::InvalidStoredData)
    ));
    assert!(
        matches!(
            start_dispatch(&pool, time.as_ref(), &ids, attempt_id, &config).await,
            Err(PaymentStoreError::InvalidStoredData)
        ),
        "trusted worker must independently reject damaged custody"
    );
    let starts: i64 = sqlx::query_scalar("SELECT count(*) FROM payment_dispatch_starts")
        .fetch_one(&pool)
        .await
        .expect("no dispatch effect");
    assert_eq!(starts, 0);
    sqlx::query("UPDATE payment_attempts SET promotion_account_id=$1 WHERE id=$2")
        .bind(account_id)
        .bind(attempt_id)
        .execute(&pool)
        .await
        .expect("restore marker");
    pool.execute("ALTER TABLE payment_attempts ENABLE TRIGGER guard_payment_attempt")
        .await
        .expect("restore immutability");
    assert!(
        owned_verified_attempt(&store, &key, &customer, attempt_id)
            .await
            .is_ok(),
        "restored exact custody reads"
    );
    customer_sessions::logout(&pool, &customer, time.as_ref())
        .await
        .expect("customer logout");
    assert!(
        owned_verified_attempt(&store, &key, &customer, attempt_id)
            .await
            .is_err()
    );
    let permit = match start_dispatch(&pool, time.as_ref(), &ids, attempt_id, &config)
        .await
        .expect("worker starts after logout")
    {
        DispatchOutcome::Start(permit) => permit,
        DispatchOutcome::Reconcile { .. } => panic!("first dispatch must be a single start"),
    };
    let frozen = permit
        .into_request(time.as_ref(), &config)
        .expect("consume local test permit");
    assert!(frozen.snapshot() == receipt.frozen.snapshot());
    assert_eq!(frozen.snapshot().binding.amount_minor, 37745);
    assert_eq!(
        frozen.snapshot().binding.stock_group_id,
        order.identity.stock_group_id
    );
    assert_eq!(frozen.body(), receipt.frozen.body());
    let custody: (i64, i64, i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM payment_dispatch_starts),(SELECT count(*) FROM commerce_outbox WHERE kind='payment.reconciliation_required.v1'),(SELECT count(*) FROM stock_holds WHERE basket_group_id=$1 AND owner_id=$2 AND state='active'),(SELECT COALESCE(sum(reserved),0)::bigint FROM stock_balances)")
        .bind(&order.identity.stock_group_id).bind(&order.identity.id).fetch_one(&pool).await.expect("durable start and original hold");
    assert_eq!(custody, (1, 1, 1, 12));
    time.0.store(44300, Ordering::SeqCst);
    let outcome = start_dispatch(&pool, time.as_ref(), &ids, attempt_id, &config)
        .await
        .expect("existing start remains reconcilable after session and hold deadlines");
    match outcome {
        DispatchOutcome::Reconcile { frozen, started_at } => {
            assert_eq!(started_at, 1100);
            assert_eq!(frozen.snapshot().binding.amount_minor, 37745);
            assert_eq!(
                frozen.snapshot().binding.stock_group_id,
                order.identity.stock_group_id
            );
            assert_eq!(frozen.body(), receipt.frozen.body());
        }
        DispatchOutcome::Start(_) => panic!("recovery must not grant a second start"),
    }
    let starts: i64 = sqlx::query_scalar("SELECT count(*) FROM payment_dispatch_starts")
        .fetch_one(&pool)
        .await
        .expect("single lifetime start");
    assert_eq!(starts, 1);

    let resolved = resolve(&pool, time.as_ref(), &ids, attempt_id, "no-observations")
        .await
        .expect("worker retains identity evidence after logout");
    assert_eq!(resolved.resolution.through_observation, 0);
    let recognized = recognize(&pool, time.as_ref(), &ids, attempt_id)
        .await
        .expect("worker recognizes the exact unconfirmed prefix");
    assert_eq!(recognized.recognition.state, State::Unconfirmed);
    assert_eq!(recognized.recognition.amount_minor, 37745);
    assert_eq!(recognized.recognition.through_observation, 0);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn altered_prepared_effect_rolls_back_attempt_and_event(pool: PgPool) {
    let (store, _, session, csrf, _, customer, order) = setup(&pool).await;
    let key = GuestCredential::from_session(&session).expect("basket owner");
    let config = payment::config();
    let intent = payment::intent(&order);
    let before = effects(&pool).await;
    pool.execute(
        r#"ALTER TABLE payment_attempts DISABLE TRIGGER guard_payment_attempt;
        CREATE FUNCTION test_attempt_effect_fault() RETURNS trigger LANGUAGE plpgsql AS $$
        BEGIN
            IF NEW.kind='payment.attempt_prepared.v1' THEN
                UPDATE payment_attempts SET effect_key='altered-effect'
                    WHERE id=NEW.payload->>'attempt_id';
            END IF;
            RETURN NEW;
        END;
        $$;
        CREATE TRIGGER test_attempt_effect_fault AFTER INSERT ON commerce_outbox
            FOR EACH ROW EXECUTE FUNCTION test_attempt_effect_fault();"#,
    )
    .await
    .expect("isolate post-event effect corruption");
    assert!(matches!(
        prepare_verified_attempt(&store, &key, &csrf, &customer, "pay", &intent, &config).await,
        Err(PaymentStoreError::InvalidStoredData)
    ));
    assert_eq!(effects(&pool).await, before);
    pool.execute("DROP TRIGGER test_attempt_effect_fault ON commerce_outbox; ALTER TABLE payment_attempts ENABLE TRIGGER guard_payment_attempt")
        .await.expect("restore guards");
    let valid = prepare_verified_attempt(&store, &key, &csrf, &customer, "pay", &intent, &config)
        .await
        .expect("original effect admits one exact attempt");
    assert!(!valid.replayed);
    assert_eq!(valid.frozen.snapshot().binding.amount_minor, 37745);
}
