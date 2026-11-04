//! Guest order custody uses real accepted orders and verified customer sessions.

#[allow(dead_code)]
#[path = "support/admitted_proofs.rs"]
mod admitted_proofs;

#[path = "support/admitted_login.rs"]
mod admitted_login;
use admitted_proofs::{
    customer_passwords, email_verification, verified_customer_accounts::establish,
};
use commerce_application::Locale;
use commerce_infrastructure::{
    MIGRATOR,
    checkout_orders::create_order,
    checkout_quotes::issue_quote,
    customer_sessions::{self, CustomerSessionToken},
    guest_baskets::{GuestCredential, GuestSessionStore},
    guest_order_links::{self, GuestOrderLinkToken, LinkError},
};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use std::{sync::atomic::Ordering, time::Duration};

#[path = "support/checkout_order_fixture.rs"]
mod fixture;
#[allow(dead_code)]
#[path = "support/checkout_quote_fixture.rs"]
mod quote_fixture;
use quote_fixture::{guest, request, seed, store};

async fn accepted_order(
    store: &GuestSessionStore,
    key: &GuestCredential,
    csrf: &str,
    suffix: &str,
) -> String {
    let quote = issue_quote(
        store,
        key,
        csrf,
        Locale::Nl,
        &request(&format!("quote-{suffix}")),
    )
    .await
    .expect("accepted quote")
    .snapshot;
    create_order(
        store,
        key,
        csrf,
        &format!("place-{suffix}"),
        &fixture::intent(&quote.binding.quote_id),
        &fixture::seller(),
    )
    .await
    .expect("accepted order")
    .snapshot
    .identity
    .id
}

async fn account(
    pool: &PgPool,
    email: &str,
    time: &quote_fixture::Time,
) -> (i64, CustomerSessionToken) {
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
    let session = admitted_login::login(pool, email, "correct horse battery staple", time)
        .await
        .expect("session");
    (id, session.token)
}

fn verifier(token: &GuestOrderLinkToken) -> Vec<u8> {
    Sha256::new()
        .chain_update(b"piwas:guest-order-link:verifier:v1\0")
        .chain_update(token.expose_bytes())
        .finalize()
        .to_vec()
}

async fn counts(pool: &PgPool) -> (i64, i64) {
    sqlx::query_as("SELECT (SELECT count(*) FROM customer_order_links), (SELECT count(*) FROM guest_order_link_proofs WHERE consumed_at IS NOT NULL)")
        .fetch_one(pool).await.expect("custody counts")
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn owner_and_current_matching_account_link_once(pool: PgPool) {
    seed(&pool).await;
    let (store, time) = store(&pool, 1100);
    let (guest_session, csrf) = guest(&store).await;
    let key = GuestCredential::from_session(&guest_session).expect("guest credential");
    let order = accepted_order(&store, &key, &csrf, "one").await;
    let (account_id, session) = account(&pool, "buyer@EXAMPLE.TEST", &time).await;
    let original: serde_json::Value =
        sqlx::query_scalar("SELECT snapshot FROM checkout_orders WHERE id=$1")
            .bind(&order)
            .fetch_one(&pool)
            .await
            .expect("accepted snapshot");
    let proof = guest_order_links::issue(&store, &key, &csrf, &order)
        .await
        .expect("owner proof");
    let stored: (Vec<u8>, String, i64, Option<i64>) = sqlx::query_as(
        "SELECT verifier,order_id,created_at,consumed_at FROM guest_order_link_proofs",
    )
    .fetch_one(&pool)
    .await
    .expect("proof custody");
    assert_eq!(stored, (verifier(&proof), order.clone(), 1100, None));
    assert_ne!(stored.0, proof.expose_bytes().to_vec());
    let receipt = guest_order_links::link(&pool, &session, &order, &proof, time.as_ref())
        .await
        .expect("link");
    assert_eq!(
        (receipt.order_id, receipt.account_id, receipt.linked_at),
        (order.clone(), account_id, 1100)
    );
    assert_eq!(counts(&pool).await, (1, 1));
    let after: serde_json::Value =
        sqlx::query_scalar("SELECT snapshot FROM checkout_orders WHERE id=$1")
            .bind(&order)
            .fetch_one(&pool)
            .await
            .expect("unchanged order");
    assert_eq!(after, original);
    assert!(matches!(
        guest_order_links::issue(&store, &key, &csrf, &order).await,
        Err(LinkError::Denied)
    ));
    assert!(matches!(
        guest_order_links::link(&pool, &session, &order, &proof, time.as_ref()).await,
        Err(LinkError::Denied)
    ));
    assert_eq!(counts(&pool).await, (1, 1));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn logout_waits_for_link_session_claim_then_revokes_it(pool: PgPool) {
    seed(&pool).await;
    let (store, time) = store(&pool, 1100);
    let (owner, csrf) = guest(&store).await;
    let key = GuestCredential::from_session(&owner).expect("owner");
    let order = accepted_order(&store, &key, &csrf, "logout-race").await;
    let (account_id, session) = account(&pool, "buyer@example.test", &time).await;
    let proof = guest_order_links::issue(&store, &key, &csrf, &order)
        .await
        .expect("proof");
    let unused = guest_order_links::issue(&store, &key, &csrf, &order)
        .await
        .expect("second proof");

    let mut blocker = pool.begin().await.expect("order blocker");
    sqlx::query("SELECT id FROM checkout_orders WHERE id=$1 FOR UPDATE")
        .bind(&order)
        .fetch_one(&mut *blocker)
        .await
        .expect("lock order");
    let worker_pool = pool.clone();
    let worker_time = time.clone();
    let worker_order = order.clone();
    let worker_session = CustomerSessionToken::from_bytes(*session.expose_bytes());
    let link = tokio::spawn(async move {
        guest_order_links::link(
            &worker_pool,
            &worker_session,
            &worker_order,
            &proof,
            worker_time.as_ref(),
        )
        .await
    });
    wait_for_query_locks(
        &pool,
        "SELECT id FROM checkout_orders WHERE id=$1 AND basket_id=$2 FOR UPDATE%",
        1,
    )
    .await;
    assert!(!link.is_finished());
    let worker_pool = pool.clone();
    let worker_time = time.clone();
    let worker_session = CustomerSessionToken::from_bytes(*session.expose_bytes());
    let logout = tokio::spawn(async move {
        customer_sessions::logout(&worker_pool, &worker_session, worker_time.as_ref()).await
    });
    wait_for_query_locks(
        &pool,
        "SELECT verified_at FROM verified_customer_accounts WHERE id=$1 FOR UPDATE%",
        1,
    )
    .await;
    assert!(!logout.is_finished());
    blocker.rollback().await.expect("release order");
    let receipt = link
        .await
        .expect("link task")
        .expect("link commits before logout");
    assert_eq!(
        (receipt.order_id, receipt.account_id),
        (order.clone(), account_id)
    );
    logout
        .await
        .expect("logout task")
        .expect("logout after link");
    assert!(matches!(
        customer_sessions::authenticate(&pool, &session, time.as_ref()).await,
        Err(customer_sessions::SessionError::Denied)
    ));
    assert!(matches!(
        guest_order_links::link(&pool, &session, &order, &unused, time.as_ref()).await,
        Err(LinkError::Denied)
    ));
    assert_eq!(counts(&pool).await, (1, 1));
}

async fn wait_for_query_locks(pool: &PgPool, pattern: &str, minimum: i64) {
    tokio::time::timeout(Duration::from_millis(750), async {
        loop {
            let waiting: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE $1")
                .bind(pattern).fetch_one(pool).await.expect("lock observer");
            if waiting >= minimum { break; }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }).await.expect("expected database lock wait");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn wrong_guest_csrf_account_order_token_and_stale_session_never_spend_proof(pool: PgPool) {
    seed(&pool).await;
    let (store, time) = store(&pool, 1100);
    let (owner, csrf) = guest(&store).await;
    let (other, other_csrf) = guest(&store).await;
    let key = GuestCredential::from_session(&owner).expect("owner");
    let other_key = GuestCredential::from_session(&other).expect("other");
    let order = accepted_order(&store, &key, &csrf, "one").await;
    let second_order = accepted_order(&store, &key, &csrf, "two").await;
    let (_, session) = account(&pool, "buyer@example.test", &time).await;
    let (_, wrong_account) = account(&pool, "else@example.test", &time).await;
    assert!(
        guest_order_links::issue(&store, &other_key, &other_csrf, &order)
            .await
            .is_err()
    );
    assert!(
        guest_order_links::issue(&store, &key, &other_csrf, &order)
            .await
            .is_err()
    );
    let proof = guest_order_links::issue(&store, &key, &csrf, &order)
        .await
        .expect("owner proof");
    let registration = email_verification::issue(&pool, "buyer@example.test", time.as_ref())
        .await
        .expect("registration token");
    let wrong_domain = GuestOrderLinkToken::from_bytes(*registration.expose_bytes());
    let wrong = GuestOrderLinkToken::from_bytes([23; 32]);
    for (actor, target, bearer) in [
        (&wrong_account, &order, &proof),
        (&session, &second_order, &proof),
        (&session, &order, &wrong_domain),
        (&session, &order, &wrong),
    ] {
        assert!(matches!(
            guest_order_links::link(&pool, actor, target, bearer, time.as_ref()).await,
            Err(LinkError::Denied)
        ));
    }
    let reset = customer_passwords::issue(&pool, "buyer@example.test", time.as_ref())
        .await
        .expect("password reset proof");
    customer_passwords::replace(
        &pool,
        "buyer@example.test",
        &reset,
        "new correct horse battery staple",
        time.as_ref(),
    )
    .await
    .expect("password replacement invalidates old session");
    assert!(matches!(
        guest_order_links::link(&pool, &session, &order, &proof, time.as_ref()).await,
        Err(LinkError::Denied)
    ));
    assert_eq!(counts(&pool).await, (0, 0));
    let current = admitted_login::login(
        &pool,
        "buyer@example.test",
        "new correct horse battery staple",
        time.as_ref(),
    )
    .await
    .expect("current session");
    assert!(
        guest_order_links::link(&pool, &current.token, &order, &proof, time.as_ref())
            .await
            .is_ok()
    );
    assert_eq!(counts(&pool).await, (1, 1));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn concurrent_proofs_cannot_create_two_links_or_consume_loser(pool: PgPool) {
    seed(&pool).await;
    let (store, time) = store(&pool, 1100);
    let (owner, csrf) = guest(&store).await;
    let key = GuestCredential::from_session(&owner).expect("owner");
    let order = accepted_order(&store, &key, &csrf, "race").await;
    let (_, session) = account(&pool, "buyer@example.test", &time).await;
    let a = guest_order_links::issue(&store, &key, &csrf, &order)
        .await
        .expect("proof a");
    let b = guest_order_links::issue(&store, &key, &csrf, &order)
        .await
        .expect("proof b");
    let mut blocker = pool.begin().await.expect("order blocker");
    sqlx::query("SELECT id FROM checkout_orders WHERE id=$1 FOR UPDATE")
        .bind(&order)
        .fetch_one(&mut *blocker)
        .await
        .expect("lock order");
    let worker_pool = pool.clone();
    let worker_time = time.clone();
    let worker_order = order.clone();
    let worker_session = CustomerSessionToken::from_bytes(*session.expose_bytes());
    let first = tokio::spawn(async move {
        guest_order_links::link(
            &worker_pool,
            &worker_session,
            &worker_order,
            &a,
            worker_time.as_ref(),
        )
        .await
    });
    let worker_pool = pool.clone();
    let worker_time = time.clone();
    let worker_order = order.clone();
    let worker_session = CustomerSessionToken::from_bytes(*session.expose_bytes());
    let second = tokio::spawn(async move {
        guest_order_links::link(
            &worker_pool,
            &worker_session,
            &worker_order,
            &b,
            worker_time.as_ref(),
        )
        .await
    });
    wait_for_query_locks(
        &pool,
        "SELECT id FROM checkout_orders WHERE id=$1 AND basket_id=$2 FOR UPDATE%",
        2,
    )
    .await;
    assert!(!first.is_finished() && !second.is_finished());
    blocker.rollback().await.expect("release order");
    let (a, b) = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(first, second)
    })
    .await
    .expect("link race completes");
    let (a, b) = (a.expect("first task"), b.expect("second task"));
    assert!(matches!(
        (a, b),
        (Ok(_), Err(LinkError::Denied)) | (Err(LinkError::Denied), Ok(_))
    ));
    assert_eq!(counts(&pool).await, (1, 1));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn expiry_after_link_write_rolls_back_proof_and_link(pool: PgPool) {
    seed(&pool).await;
    let (store, time) = store(&pool, 1100);
    let (owner, csrf) = guest(&store).await;
    let key = GuestCredential::from_session(&owner).expect("owner");
    let order = accepted_order(&store, &key, &csrf, "wait").await;
    let (_, session) = account(&pool, "buyer@example.test", &time).await;
    let proof = guest_order_links::issue(&store, &key, &csrf, &order)
        .await
        .expect("proof");
    let expires: i64 =
        sqlx::query_scalar("SELECT expires_at FROM guest_order_link_proofs WHERE verifier=$1")
            .bind(verifier(&proof))
            .fetch_one(&pool)
            .await
            .expect("expiry");
    sqlx::query("CREATE FUNCTION fence_guest_link() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_advisory_xact_lock(17543825); RETURN NEW; END $$").execute(&pool).await.expect("fence");
    sqlx::query("CREATE TRIGGER fence_guest_link AFTER INSERT ON customer_order_links FOR EACH ROW EXECUTE FUNCTION fence_guest_link()").execute(&pool).await.expect("trigger");
    let mut blocker = pool.begin().await.expect("blocker");
    sqlx::query("SELECT pg_advisory_xact_lock(17543825)")
        .execute(&mut *blocker)
        .await
        .expect("hold fence");
    let worker_pool = pool.clone();
    let worker_time = time.clone();
    let worker_order = order.clone();
    let worker_token = GuestOrderLinkToken::from_bytes(*proof.expose_bytes());
    let task = tokio::spawn(async move {
        guest_order_links::link(
            &worker_pool,
            &session,
            &worker_order,
            &worker_token,
            worker_time.as_ref(),
        )
        .await
    });
    let mut waiting = false;
    for _ in 0..300 {
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_locks WHERE locktype='advisory' AND NOT granted AND pid<>pg_backend_pid()").fetch_one(&pool).await.expect("lock observer");
        if n > 0 {
            waiting = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(waiting && !task.is_finished());
    time.0.store(expires as u64, Ordering::SeqCst);
    blocker.rollback().await.expect("release fence");
    assert!(matches!(task.await.expect("join"), Err(LinkError::Denied)));
    assert_eq!(counts(&pool).await, (0, 0));
    time.0.store(1100, Ordering::SeqCst);
    let current = admitted_login::login(
        &pool,
        "buyer@example.test",
        "correct horse battery staple",
        time.as_ref(),
    )
    .await
    .expect("current session");
    assert!(
        guest_order_links::link(&pool, &current.token, &order, &proof, time.as_ref())
            .await
            .is_ok()
    );
    assert_eq!(counts(&pool).await, (1, 1));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn native_guards_require_matching_same_transaction_pair_and_preserve_healthy_control(
    pool: PgPool,
) {
    seed(&pool).await;
    let (store, time) = store(&pool, 1100);
    let (owner, csrf) = guest(&store).await;
    let key = GuestCredential::from_session(&owner).expect("owner");
    let order = accepted_order(&store, &key, &csrf, "native").await;
    let (account_id, _) = account(&pool, "buyer@example.test", &time).await;
    let (other_id, _) = account(&pool, "other@example.test", &time).await;
    let proof = guest_order_links::issue(&store, &key, &csrf, &order)
        .await
        .expect("proof");
    let digest = verifier(&proof);
    assert!(sqlx::query("INSERT INTO customer_order_links(order_id,account_id,proof_verifier,linked_at) VALUES ($1,$2,$3,1100)")
        .bind(&order).bind(account_id).bind(&digest).execute(&pool).await.is_err(), "link cannot adopt an unconsumed proof");
    assert!(sqlx::query("UPDATE guest_order_link_proofs SET consumed_at=1100,consumed_account_id=$1 WHERE verifier=$2")
        .bind(other_id).bind(&digest).execute(&pool).await.is_err(), "different buyer cannot consume proof");
    assert!(sqlx::query("UPDATE guest_order_link_proofs SET consumed_at=1100,consumed_account_id=$1 WHERE verifier=$2")
        .bind(account_id).bind(&digest).execute(&pool).await.is_err(), "consumption cannot commit without its link");
    assert_eq!(counts(&pool).await, (0, 0));
    let mut tx = pool.begin().await.expect("cross-account transaction");
    sqlx::query("UPDATE guest_order_link_proofs SET consumed_at=1100,consumed_account_id=$1 WHERE verifier=$2")
        .bind(account_id).bind(&digest).execute(&mut *tx).await.expect("matching consumption");
    assert!(sqlx::query("INSERT INTO customer_order_links(order_id,account_id,proof_verifier,linked_at) VALUES ($1,$2,$3,1100)")
        .bind(&order).bind(other_id).bind(&digest).execute(&mut *tx).await.is_err(), "link account must equal proof consumer");
    tx.rollback().await.expect("rollback");
    assert_eq!(counts(&pool).await, (0, 0));
    let mut tx = pool.begin().await.expect("healthy transaction");
    sqlx::query("UPDATE guest_order_link_proofs SET consumed_at=1100,consumed_account_id=$1 WHERE verifier=$2")
        .bind(account_id).bind(&digest).execute(&mut *tx).await.expect("consume");
    sqlx::query("INSERT INTO customer_order_links(order_id,account_id,proof_verifier,linked_at) VALUES ($1,$2,$3,1100)")
        .bind(&order).bind(account_id).bind(&digest).execute(&mut *tx).await.expect("matching reciprocal link");
    sqlx::query("SET CONSTRAINTS require_guest_order_link IMMEDIATE")
        .execute(&mut *tx)
        .await
        .expect("complete proof pair");
    tx.commit().await.expect("commit");
    assert_eq!(counts(&pool).await, (1, 1));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn issuance_rejects_invalid_input_and_suppressed_or_altered_proof_writes(pool: PgPool) {
    seed(&pool).await;
    let (store, time) = store(&pool, 1100);
    let (owner, csrf) = guest(&store).await;
    let key = GuestCredential::from_session(&owner).expect("owner");
    let order = accepted_order(&store, &key, &csrf, "issue-controls").await;
    let (_, session) = account(&pool, "buyer@example.test", &time).await;
    assert!(matches!(
        guest_order_links::issue(&store, &key, &csrf, "invalid").await,
        Err(LinkError::Denied)
    ));

    sqlx::query("CREATE FUNCTION suppress_test_write() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RETURN NULL; END $$")
        .execute(&pool).await.expect("suppression function");
    sqlx::query("CREATE TRIGGER a_test_suppress BEFORE INSERT ON guest_order_link_proofs FOR EACH ROW EXECUTE FUNCTION suppress_test_write()")
        .execute(&pool).await.expect("suppress proof insert");
    assert!(matches!(
        guest_order_links::issue(&store, &key, &csrf, &order).await,
        Err(LinkError::Unavailable)
    ));
    sqlx::query("DROP TRIGGER a_test_suppress ON guest_order_link_proofs")
        .execute(&pool)
        .await
        .expect("remove suppression");

    // Run after the native guard, retaining the table checks while altering persisted custody.
    sqlx::query("CREATE FUNCTION alter_test_proof() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN NEW.created_at := NEW.created_at+1; NEW.expires_at := NEW.expires_at+1; RETURN NEW; END $$")
        .execute(&pool).await.expect("alteration function");
    sqlx::query("CREATE TRIGGER z_test_alter BEFORE INSERT ON guest_order_link_proofs FOR EACH ROW EXECUTE FUNCTION alter_test_proof()")
        .execute(&pool).await.expect("alter proof insert");
    assert!(matches!(
        guest_order_links::issue(&store, &key, &csrf, &order).await,
        Err(LinkError::Unavailable)
    ));
    sqlx::query("DROP TRIGGER z_test_alter ON guest_order_link_proofs")
        .execute(&pool)
        .await
        .expect("remove alteration");
    let proofs: i64 = sqlx::query_scalar("SELECT count(*) FROM guest_order_link_proofs")
        .fetch_one(&pool)
        .await
        .expect("proof count");
    assert_eq!(proofs, 0);
    let proof = guest_order_links::issue(&store, &key, &csrf, &order)
        .await
        .expect("healthy issuance");
    guest_order_links::link(&pool, &session, &order, &proof, time.as_ref())
        .await
        .expect("healthy redemption");
    assert_eq!(counts(&pool).await, (1, 1));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn redemption_rejects_suppressed_or_altered_writes_without_spending_proof(pool: PgPool) {
    seed(&pool).await;
    let (store, time) = store(&pool, 1100);
    let (owner, csrf) = guest(&store).await;
    let key = GuestCredential::from_session(&owner).expect("owner");
    let order = accepted_order(&store, &key, &csrf, "link-controls").await;
    let (_, session) = account(&pool, "buyer@example.test", &time).await;
    let proof = guest_order_links::issue(&store, &key, &csrf, &order)
        .await
        .expect("proof");
    assert!(matches!(
        guest_order_links::link(&pool, &session, "invalid", &proof, time.as_ref()).await,
        Err(LinkError::Denied)
    ));
    sqlx::query("CREATE FUNCTION suppress_test_write() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RETURN NULL; END $$")
        .execute(&pool).await.expect("suppression function");
    for (create, drop) in [
        (
            "CREATE TRIGGER a_test_suppress BEFORE UPDATE ON guest_order_link_proofs FOR EACH ROW EXECUTE FUNCTION suppress_test_write()",
            "DROP TRIGGER a_test_suppress ON guest_order_link_proofs",
        ),
        (
            "CREATE TRIGGER a_test_suppress BEFORE INSERT ON customer_order_links FOR EACH ROW EXECUTE FUNCTION suppress_test_write()",
            "DROP TRIGGER a_test_suppress ON customer_order_links",
        ),
    ] {
        sqlx::query(create)
            .execute(&pool)
            .await
            .expect("suppress write");
        assert!(matches!(
            guest_order_links::link(&pool, &session, &order, &proof, time.as_ref()).await,
            Err(LinkError::Unavailable)
        ));
        sqlx::query(drop)
            .execute(&pool)
            .await
            .expect("remove suppression");
        assert_eq!(counts(&pool).await, (0, 0));
    }

    // Simulate a damaged imported pair despite disabled reciprocal constraint triggers.
    sqlx::query("ALTER TABLE guest_order_link_proofs DISABLE TRIGGER require_guest_order_link")
        .execute(&pool)
        .await
        .expect("disable proof constraint");
    sqlx::query("ALTER TABLE customer_order_links DISABLE TRIGGER require_guest_order_link")
        .execute(&pool)
        .await
        .expect("disable link constraint");
    sqlx::query("CREATE FUNCTION alter_test_link() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN NEW.linked_at := NEW.linked_at+1; RETURN NEW; END $$")
        .execute(&pool).await.expect("alteration function");
    sqlx::query("CREATE TRIGGER z_test_alter BEFORE INSERT ON customer_order_links FOR EACH ROW EXECUTE FUNCTION alter_test_link()")
        .execute(&pool).await.expect("alter link insert");
    assert!(matches!(
        guest_order_links::link(&pool, &session, &order, &proof, time.as_ref()).await,
        Err(LinkError::Unavailable)
    ));
    sqlx::query("DROP TRIGGER z_test_alter ON customer_order_links")
        .execute(&pool)
        .await
        .expect("remove alteration");
    sqlx::query("ALTER TABLE guest_order_link_proofs ENABLE TRIGGER require_guest_order_link")
        .execute(&pool)
        .await
        .expect("restore proof constraint");
    sqlx::query("ALTER TABLE customer_order_links ENABLE TRIGGER require_guest_order_link")
        .execute(&pool)
        .await
        .expect("restore link constraint");
    assert_eq!(counts(&pool).await, (0, 0));
    guest_order_links::link(&pool, &session, &order, &proof, time.as_ref())
        .await
        .expect("healthy redemption");
    assert_eq!(counts(&pool).await, (1, 1));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn damaged_proof_and_noncanonical_account_do_not_authorize_link(pool: PgPool) {
    seed(&pool).await;
    let (store, time) = store(&pool, 1100);
    let (owner, csrf) = guest(&store).await;
    let key = GuestCredential::from_session(&owner).expect("owner");
    let order = accepted_order(&store, &key, &csrf, "damaged-proof").await;
    let (account_id, session) = account(&pool, "buyer@example.test", &time).await;
    let proof = guest_order_links::issue(&store, &key, &csrf, &order)
        .await
        .expect("proof");
    let digest = verifier(&proof);
    let mut tx = pool.begin().await.expect("isolated corruption");
    sqlx::query("ALTER TABLE guest_order_link_proofs DISABLE TRIGGER USER")
        .execute(&mut *tx)
        .await
        .expect("disable guard");
    sqlx::query(
        "UPDATE guest_order_link_proofs SET created_at=1101,expires_at=2001 WHERE verifier=$1",
    )
    .bind(&digest)
    .execute(&mut *tx)
    .await
    .expect("future proof");
    sqlx::query("ALTER TABLE guest_order_link_proofs ENABLE TRIGGER USER")
        .execute(&mut *tx)
        .await
        .expect("restore guard");
    tx.commit().await.expect("commit corruption");
    assert!(matches!(
        guest_order_links::link(&pool, &session, &order, &proof, time.as_ref()).await,
        Err(LinkError::Denied)
    ));
    assert_eq!(counts(&pool).await, (0, 0));
    let mut tx = pool.begin().await.expect("repair and corrupt mailbox");
    sqlx::query("ALTER TABLE guest_order_link_proofs DISABLE TRIGGER USER")
        .execute(&mut *tx)
        .await
        .expect("disable proof guard");
    sqlx::query(
        "UPDATE guest_order_link_proofs SET created_at=1100,expires_at=2000 WHERE verifier=$1",
    )
    .bind(&digest)
    .execute(&mut *tx)
    .await
    .expect("repair proof");
    sqlx::query("ALTER TABLE guest_order_link_proofs ENABLE TRIGGER USER")
        .execute(&mut *tx)
        .await
        .expect("restore proof guard");
    sqlx::query("ALTER TABLE verified_customer_accounts DISABLE TRIGGER USER")
        .execute(&mut *tx)
        .await
        .expect("disable account guard");
    sqlx::query(
        "UPDATE verified_customer_accounts SET email_identity='buyer@EXAMPLE.TEST' WHERE id=$1",
    )
    .bind(account_id)
    .execute(&mut *tx)
    .await
    .expect("noncanonical mailbox");
    sqlx::query("ALTER TABLE verified_customer_accounts ENABLE TRIGGER USER")
        .execute(&mut *tx)
        .await
        .expect("restore account guard");
    tx.commit().await.expect("commit mailbox corruption");
    assert!(matches!(
        guest_order_links::link(&pool, &session, &order, &proof, time.as_ref()).await,
        Err(LinkError::Denied)
    ));
    assert_eq!(counts(&pool).await, (0, 0));
    let mut tx = pool.begin().await.expect("repair mailbox");
    sqlx::query("ALTER TABLE verified_customer_accounts DISABLE TRIGGER USER")
        .execute(&mut *tx)
        .await
        .expect("disable account guard");
    sqlx::query(
        "UPDATE verified_customer_accounts SET email_identity='buyer@example.test' WHERE id=$1",
    )
    .bind(account_id)
    .execute(&mut *tx)
    .await
    .expect("repair mailbox");
    sqlx::query("ALTER TABLE verified_customer_accounts ENABLE TRIGGER USER")
        .execute(&mut *tx)
        .await
        .expect("restore account guard");
    tx.commit().await.expect("commit repair");
    guest_order_links::link(&pool, &session, &order, &proof, time.as_ref())
        .await
        .expect("healthy redemption");
    assert_eq!(counts(&pool).await, (1, 1));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn issuance_expiry_after_insert_and_failed_commit_leave_no_proof(pool: PgPool) {
    seed(&pool).await;
    let (store, time) = store(&pool, 1100);
    let (owner, csrf) = guest(&store).await;
    let key = GuestCredential::from_session(&owner).expect("owner");
    let order = accepted_order(&store, &key, &csrf, "issue-expiry").await;
    let (_, session) = account(&pool, "buyer@example.test", &time).await;
    sqlx::query("CREATE FUNCTION fence_test_issue() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_advisory_xact_lock(17543826); RETURN NEW; END $$").execute(&pool).await.expect("fence function");
    sqlx::query("CREATE TRIGGER fence_test_issue AFTER INSERT ON guest_order_link_proofs FOR EACH ROW EXECUTE FUNCTION fence_test_issue()").execute(&pool).await.expect("fence trigger");
    let mut blocker = pool.begin().await.expect("blocker");
    sqlx::query("SELECT pg_advisory_xact_lock(17543826)")
        .execute(&mut *blocker)
        .await
        .expect("hold fence");
    let worker_store = store.clone();
    let worker_time = time.clone();
    let worker_order = order.clone();
    let worker_csrf = csrf.clone();
    let task = tokio::spawn(async move {
        guest_order_links::issue(&worker_store, &key, &worker_csrf, &worker_order).await
    });
    wait_for_query_locks(&pool, "INSERT INTO guest_order_link_proofs%", 1).await;
    worker_time.0.store(2000, Ordering::SeqCst);
    blocker.rollback().await.expect("release fence");
    assert!(matches!(
        task.await.expect("issue task"),
        Err(LinkError::Denied)
    ));
    let proofs: i64 = sqlx::query_scalar("SELECT count(*) FROM guest_order_link_proofs")
        .fetch_one(&pool)
        .await
        .expect("proof count");
    assert_eq!(proofs, 0);
    time.0.store(1100, Ordering::SeqCst);
    let mut blocker = pool.begin().await.expect("rewind blocker");
    sqlx::query("SELECT pg_advisory_xact_lock(17543826)")
        .execute(&mut *blocker)
        .await
        .expect("hold rewind fence");
    let worker_store = store.clone();
    let worker_order = order.clone();
    let worker_csrf = csrf.clone();
    let worker_key = GuestCredential::from_session(&owner).expect("owner");
    let task = tokio::spawn(async move {
        guest_order_links::issue(&worker_store, &worker_key, &worker_csrf, &worker_order).await
    });
    wait_for_query_locks(&pool, "INSERT INTO guest_order_link_proofs%", 1).await;
    time.0.store(1099, Ordering::SeqCst);
    blocker.rollback().await.expect("release rewind fence");
    assert!(matches!(
        task.await.expect("rewind issue task"),
        Err(LinkError::Unavailable)
    ));
    let proofs: i64 = sqlx::query_scalar("SELECT count(*) FROM guest_order_link_proofs")
        .fetch_one(&pool)
        .await
        .expect("proof count");
    assert_eq!(proofs, 0);
    time.0.store(1100, Ordering::SeqCst);
    sqlx::query("DROP TRIGGER fence_test_issue ON guest_order_link_proofs")
        .execute(&pool)
        .await
        .expect("remove fence");
    let key = GuestCredential::from_session(&owner).expect("owner");
    sqlx::query("CREATE FUNCTION fail_test_commit() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'test commit failure'; END $$").execute(&pool).await.expect("failure function");
    sqlx::query("CREATE CONSTRAINT TRIGGER fail_test_commit AFTER INSERT ON guest_order_link_proofs DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION fail_test_commit()").execute(&pool).await.expect("deferred failure");
    assert!(matches!(
        guest_order_links::issue(&store, &key, &csrf, &order).await,
        Err(LinkError::Unavailable)
    ));
    sqlx::query("DROP TRIGGER fail_test_commit ON guest_order_link_proofs")
        .execute(&pool)
        .await
        .expect("remove failure");
    let proofs: i64 = sqlx::query_scalar("SELECT count(*) FROM guest_order_link_proofs")
        .fetch_one(&pool)
        .await
        .expect("proof count");
    assert_eq!(proofs, 0);
    let proof = guest_order_links::issue(&store, &key, &csrf, &order)
        .await
        .expect("healthy proof");
    guest_order_links::link(&pool, &session, &order, &proof, time.as_ref())
        .await
        .expect("healthy link");
    assert_eq!(counts(&pool).await, (1, 1));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn link_expiry_after_order_wait_and_failed_constraint_leave_proof_unspent(pool: PgPool) {
    seed(&pool).await;
    let (store, time) = store(&pool, 1100);
    let (owner, csrf) = guest(&store).await;
    let key = GuestCredential::from_session(&owner).expect("owner");
    let order = accepted_order(&store, &key, &csrf, "link-expiry").await;
    let (_, session) = account(&pool, "buyer@example.test", &time).await;
    let proof = guest_order_links::issue(&store, &key, &csrf, &order)
        .await
        .expect("proof");
    let mut blocker = pool.begin().await.expect("blocker");
    sqlx::query("SELECT id FROM checkout_orders WHERE id=$1 FOR UPDATE")
        .bind(&order)
        .fetch_one(&mut *blocker)
        .await
        .expect("lock order");
    let worker_pool = pool.clone();
    let worker_time = time.clone();
    let worker_order = order.clone();
    let worker_session = CustomerSessionToken::from_bytes(*session.expose_bytes());
    let worker_proof = GuestOrderLinkToken::from_bytes(*proof.expose_bytes());
    let task = tokio::spawn(async move {
        guest_order_links::link(
            &worker_pool,
            &worker_session,
            &worker_order,
            &worker_proof,
            worker_time.as_ref(),
        )
        .await
    });
    wait_for_query_locks(
        &pool,
        "SELECT id FROM checkout_orders WHERE id=$1 AND basket_id=$2 FOR UPDATE%",
        1,
    )
    .await;
    time.0.store(2000, Ordering::SeqCst);
    blocker.rollback().await.expect("release order");
    assert!(matches!(
        task.await.expect("link task"),
        Err(LinkError::Denied)
    ));
    assert_eq!(counts(&pool).await, (0, 0));
    time.0.store(1100, Ordering::SeqCst);

    // Damage the consumed proof inside the transaction; its deferred native check must fail.
    sqlx::query("ALTER TABLE guest_order_link_proofs DISABLE TRIGGER guard_guest_order_link_proof")
        .execute(&pool)
        .await
        .expect("disable immutable guard");
    sqlx::query("CREATE FUNCTION damage_test_pair() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN UPDATE guest_order_link_proofs SET consumed_at=consumed_at+1 WHERE verifier=NEW.proof_verifier; RETURN NEW; END $$").execute(&pool).await.expect("damage function");
    sqlx::query("CREATE TRIGGER damage_test_pair AFTER INSERT ON customer_order_links FOR EACH ROW EXECUTE FUNCTION damage_test_pair()").execute(&pool).await.expect("damage trigger");
    assert!(matches!(
        guest_order_links::link(&pool, &session, &order, &proof, time.as_ref()).await,
        Err(LinkError::Unavailable)
    ));
    sqlx::query("DROP TRIGGER damage_test_pair ON customer_order_links")
        .execute(&pool)
        .await
        .expect("remove damage");
    sqlx::query("ALTER TABLE guest_order_link_proofs ENABLE TRIGGER guard_guest_order_link_proof")
        .execute(&pool)
        .await
        .expect("restore immutable guard");
    assert_eq!(counts(&pool).await, (0, 0));
    guest_order_links::link(&pool, &session, &order, &proof, time.as_ref())
        .await
        .expect("healthy link");
    assert_eq!(counts(&pool).await, (1, 1));
}
