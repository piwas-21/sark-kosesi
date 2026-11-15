//! Offline synthetic guest-link ciphertext custody with isolated PostgreSQL.
use commerce_application::Locale;
use commerce_infrastructure::{
    MIGRATOR,
    checkout_orders::create_order,
    checkout_quotes::issue_quote,
    guest_baskets::{GuestCredential, GuestSessionStore},
    guest_order_links::{
        self, LinkError,
        delivery::{self, DeliveryKey},
    },
};
use sqlx::{Executor, PgPool};
use std::{sync::atomic::Ordering, time::Duration};

#[path = "support/admitted_login.rs"]
mod admitted_login;
#[allow(dead_code)]
#[path = "support/admitted_proofs.rs"]
mod admitted_proofs;

#[allow(dead_code)]
#[path = "support/checkout_order_fixture.rs"]
mod fixture;
#[allow(dead_code)]
#[path = "support/checkout_quote_fixture.rs"]
mod quote;

async fn order(
    store: &GuestSessionStore,
    guest: &GuestCredential,
    csrf: &str,
    suffix: &str,
) -> String {
    let value = issue_quote(
        store,
        guest,
        csrf,
        Locale::Nl,
        &quote::request(&format!("q-{suffix}")),
    )
    .await
    .expect("quote")
    .snapshot;
    create_order(
        store,
        guest,
        csrf,
        &format!("o-{suffix}"),
        &fixture::intent(&value.binding.quote_id),
        &fixture::seller(),
    )
    .await
    .expect("order")
    .snapshot
    .identity
    .id
}
fn key() -> DeliveryKey {
    DeliveryKey::capture("guest-key-1", [7; 32]).expect("key")
}
async fn counts(pool: &PgPool) -> (i64, i64, i64) {
    sqlx::query_as("SELECT (SELECT count(*) FROM guest_link_delivery_nonces),(SELECT count(*) FROM guest_order_link_proofs),(SELECT count(*) FROM guest_link_delivery_envelopes)")
        .fetch_one(pool).await.expect("custody counts")
}

#[sqlx::test(migrations = false)]
async fn guest_order_link_delivery_populated_66_through_68_preserves_legacy_proof(pool: PgPool) {
    use admitted_proofs::{
        customer_passwords, email_verification, verified_customer_accounts::establish,
    };
    use sha2::{Digest, Sha256};

    MIGRATOR.run_to(66, &pool).await.expect("legacy schema");
    quote::seed(&pool).await;
    let (store, clock) = quote::store(&pool, 1100);
    let (session, csrf) = quote::guest(&store).await;
    let guest = GuestCredential::from_session(&session).expect("guest");
    let id = order(&store, &guest, &csrf, "upgrade").await;
    let legacy = guest_order_links::issue(&store, &guest, &csrf, &id)
        .await
        .expect("legacy proof");
    let verifier = Sha256::new()
        .chain_update(b"piwas:guest-order-link:verifier:v1\0")
        .chain_update(legacy.expose_bytes())
        .finalize()
        .to_vec();
    let read_proof = || async {
        sqlx::query_as::<_, (String, String, i64, i64, Option<i64>)>(
            "SELECT order_id,basket_id,created_at,expires_at,consumed_at FROM guest_order_link_proofs WHERE verifier=$1",
        )
        .bind(&verifier)
        .fetch_one(&pool)
        .await
        .expect("legacy proof retained")
    };
    let before = read_proof().await;
    MIGRATOR.run_to(67, &pool).await.expect("packing upgrade");
    assert_eq!(read_proof().await, before);
    MIGRATOR.run_to(68, &pool).await.expect("delivery upgrade");
    assert_eq!(read_proof().await, before);
    let legacy_mode: bool = sqlx::query_scalar("SELECT delivery_mode='direct' AND delivery_transaction_id IS NULL FROM guest_order_link_proofs WHERE verifier=$1")
        .bind(&verifier).fetch_one(&pool).await.expect("legacy mode");
    assert!(legacy_mode);

    let delivery_key = key();
    delivery::register_key(&pool, clock.as_ref(), &delivery_key)
        .await
        .expect("register key");
    delivery::issue_for_delivery(&store, &guest, &csrf, &id, &delivery_key)
        .await
        .expect("new encrypted proof");
    assert_eq!(counts(&pool).await, (1, 2, 1));
    MIGRATOR.run_to(69, &pool).await.expect("start upgrade");
    assert_eq!(read_proof().await, before);
    assert_eq!(counts(&pool).await, (1, 2, 1));

    let email = "buyer@example.test";
    let proof = email_verification::issue(&pool, email, clock.as_ref())
        .await
        .expect("email proof");
    establish(&pool, email, &proof, clock.as_ref())
        .await
        .expect("verified account");
    let proof = customer_passwords::issue(&pool, email, clock.as_ref())
        .await
        .expect("password proof");
    customer_passwords::replace(
        &pool,
        email,
        &proof,
        "correct horse battery staple",
        clock.as_ref(),
    )
    .await
    .expect("password");
    let customer =
        admitted_login::login(&pool, email, "correct horse battery staple", clock.as_ref())
            .await
            .expect("customer session");
    guest_order_links::link(&pool, &customer.token, &id, &legacy, clock.as_ref())
        .await
        .expect("legacy proof remains redeemable");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn guest_order_link_delivery_authorized_issuance_retains_exact_ciphertext_without_recipient_or_bearer(
    pool: PgPool,
) {
    quote::seed(&pool).await;
    let (store, clock) = quote::store(&pool, 1100);
    let (session, csrf) = quote::guest(&store).await;
    let guest = GuestCredential::from_session(&session).expect("guest");
    let id = order(&store, &guest, &csrf, "valid").await;
    let delivery_key = key();
    delivery::register_key(&pool, clock.as_ref(), &delivery_key)
        .await
        .expect("register");
    delivery::register_key(&pool, clock.as_ref(), &delivery_key)
        .await
        .expect("same key");
    let wrong_material = DeliveryKey::capture("guest-key-1", [8; 32]).expect("other key");
    assert!(matches!(
        delivery::register_key(&pool, clock.as_ref(), &wrong_material).await,
        Err(LinkError::Unavailable)
    ));
    let alias = DeliveryKey::capture("guest-key-2", [7; 32]).expect("alias");
    assert!(matches!(
        delivery::register_key(&pool, clock.as_ref(), &alias).await,
        Err(LinkError::Unavailable)
    ));
    assert!(matches!(
        delivery::issue_for_delivery(&store, &guest, &"0".repeat(64), &id, &delivery_key).await,
        Err(LinkError::Denied)
    ));
    assert_eq!(counts(&pool).await, (0, 0, 0));
    let receipt = delivery::issue_for_delivery(&store, &guest, &csrf, &id, &delivery_key)
        .await
        .expect("issue");
    assert_eq!((receipt.created_at, receipt.expires_at), (1100, 2000));
    assert_eq!(counts(&pool).await, (1, 1, 1));
    let (proof, body, ciphertext): (bool, String, Vec<u8>) = sqlx::query_as("SELECT p.delivery_mode='envelope' AND p.order_id=$2 AND p.basket_id=(SELECT basket_id FROM checkout_orders WHERE id=$2) AND p.delivery_transaction_id=e.transaction_id AND p.consumed_at IS NULL, row_to_json(e)::text,e.ciphertext FROM guest_order_link_proofs p JOIN guest_link_delivery_envelopes e USING(verifier) WHERE p.verifier=$1")
        .bind(receipt.verifier.as_slice()).bind(&id).fetch_one(&pool).await.expect("stored custody");
    assert!(proof);
    assert_eq!(ciphertext.len(), 48);
    assert!(!body.contains("buyer@example.test"));
    assert!(!format!("{receipt:?}").contains("buyer@example.test"));
    let direct = guest_order_links::issue(&store, &guest, &csrf, &id)
        .await
        .expect("legacy direct proof");
    assert_eq!(direct.expose_bytes().len(), 32);
    assert_eq!(counts(&pool).await, (1, 2, 1));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn guest_order_link_delivery_failed_envelope_write_burns_nonce_and_rolls_back_proof(
    pool: PgPool,
) {
    quote::seed(&pool).await;
    let (store, clock) = quote::store(&pool, 1100);
    let (session, csrf) = quote::guest(&store).await;
    let guest = GuestCredential::from_session(&session).expect("guest");
    let id = order(&store, &guest, &csrf, "fault").await;
    let delivery_key = key();
    delivery::register_key(&pool, clock.as_ref(), &delivery_key)
        .await
        .expect("register");
    pool.execute("CREATE FUNCTION fail_guest_envelope() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'synthetic'; END $$").await.expect("fault");
    pool.execute("CREATE TRIGGER fail_guest_envelope BEFORE INSERT ON guest_link_delivery_envelopes FOR EACH ROW EXECUTE FUNCTION fail_guest_envelope()").await.expect("trigger");
    assert!(matches!(
        delivery::issue_for_delivery(&store, &guest, &csrf, &id, &delivery_key).await,
        Err(LinkError::Unavailable)
    ));
    assert_eq!(counts(&pool).await, (1, 0, 0));
    pool.execute("DROP TRIGGER fail_guest_envelope ON guest_link_delivery_envelopes")
        .await
        .expect("remove fault");
    delivery::issue_for_delivery(&store, &guest, &csrf, &id, &delivery_key)
        .await
        .expect("new issue");
    assert_eq!(counts(&pool).await, (2, 1, 1));
    let nonce: i64 = sqlx::query_scalar("SELECT nonce_counter FROM guest_link_delivery_envelopes")
        .fetch_one(&pool)
        .await
        .expect("nonce");
    assert_eq!(nonce, 2);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn guest_order_link_delivery_guest_expiry_during_nonce_lock_burns_nonce_without_proof(
    pool: PgPool,
) {
    quote::seed(&pool).await;
    let (store, clock) = quote::store(&pool, 1100);
    let (session, csrf) = quote::guest(&store).await;
    let guest = GuestCredential::from_session(&session).expect("guest");
    let id = order(&store, &guest, &csrf, "wait").await;
    let delivery_key = key();
    delivery::register_key(&pool, clock.as_ref(), &delivery_key)
        .await
        .expect("register");
    let expires: i64 = sqlx::query_scalar("SELECT expires_at FROM guest_sessions")
        .fetch_one(&pool)
        .await
        .expect("expiry");
    let mut lock = pool.begin().await.expect("lock");
    sqlx::query(
        "SELECT key_id FROM guest_link_delivery_keys WHERE key_id='guest-key-1' FOR UPDATE",
    )
    .fetch_one(&mut *lock)
    .await
    .expect("held");
    let task = tokio::spawn(async move {
        delivery::issue_for_delivery(&store, &guest, &csrf, &id, &delivery_key).await
    });
    tokio::time::timeout(Duration::from_millis(750), async {
        loop {
            let waiting: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE '%guest_link_delivery_keys%FOR UPDATE%'").fetch_one(&pool).await.expect("observer");
            if waiting > 0 { break; }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }).await.expect("actual key lock wait");
    clock.0.store(expires as u64, Ordering::SeqCst);
    lock.commit().await.expect("release");
    assert!(matches!(task.await.expect("join"), Err(LinkError::Denied)));
    assert_eq!(counts(&pool).await, (1, 0, 0));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn guest_order_link_delivery_concurrent_issues_allocate_distinct_nonces_with_matching_proofs(
    pool: PgPool,
) {
    quote::seed(&pool).await;
    let (store, clock) = quote::store(&pool, 1100);
    let (a, csrf_a) = quote::guest(&store).await;
    let (b, csrf_b) = quote::guest(&store).await;
    let guest_a = GuestCredential::from_session(&a).expect("guest a");
    let guest_b = GuestCredential::from_session(&b).expect("guest b");
    let order_a = order(&store, &guest_a, &csrf_a, "race-a").await;
    let order_b = order(&store, &guest_b, &csrf_b, "race-b").await;
    let delivery_key = key();
    delivery::register_key(&pool, clock.as_ref(), &delivery_key)
        .await
        .expect("register");
    let (left, right) = tokio::join!(
        delivery::issue_for_delivery(&store, &guest_a, &csrf_a, &order_a, &delivery_key),
        delivery::issue_for_delivery(&store, &guest_b, &csrf_b, &order_b, &delivery_key),
    );
    let left = left.expect("first issue");
    let right = right.expect("second issue");
    assert_ne!(left.verifier, right.verifier);
    assert_eq!(counts(&pool).await, (2, 2, 2));
    let counters: Vec<i64> = sqlx::query_scalar(
        "SELECT nonce_counter FROM guest_link_delivery_nonces ORDER BY nonce_counter",
    )
    .fetch_all(&pool)
    .await
    .expect("allocated counters");
    assert_eq!(counters, vec![1, 2]);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn guest_order_link_delivery_native_guards_deny_detached_or_same_transaction_proof_envelope(
    pool: PgPool,
) {
    quote::seed(&pool).await;
    let (store, clock) = quote::store(&pool, 1100);
    let (session, csrf) = quote::guest(&store).await;
    let guest = GuestCredential::from_session(&session).expect("guest");
    let id = order(&store, &guest, &csrf, "native").await;
    let basket: String = sqlx::query_scalar("SELECT basket_id FROM checkout_orders WHERE id=$1")
        .bind(&id)
        .fetch_one(&pool)
        .await
        .expect("basket");
    delivery::register_key(&pool, clock.as_ref(), &key())
        .await
        .expect("register");
    let mut tx = pool.begin().await.expect("transaction");
    sqlx::query("INSERT INTO guest_order_link_proofs(verifier,order_id,basket_id,created_at,expires_at,delivery_mode) VALUES($1,$2,$3,1100,2000,'envelope')")
        .bind([1_u8;32].as_slice()).bind(&id).bind(&basket).execute(&mut *tx).await.expect("unpaired proof");
    assert!(
        sqlx::query("SET CONSTRAINTS require_guest_link_proof_envelope IMMEDIATE")
            .execute(&mut *tx)
            .await
            .is_err()
    );
    tx.rollback().await.expect("rollback");
    let mut tx = pool.begin().await.expect("transaction");
    sqlx::query("SET LOCAL synchronous_commit='on'")
        .execute(&mut *tx)
        .await
        .expect("durable");
    let nonce: i64 = sqlx::query_scalar("INSERT INTO guest_link_delivery_nonces(key_id,verifier,reserved_at) VALUES('guest-key-1',$1,1100) RETURNING nonce_counter")
        .bind([2_u8;32].as_slice()).fetch_one(&mut *tx).await.expect("nonce");
    sqlx::query("INSERT INTO guest_order_link_proofs(verifier,order_id,basket_id,created_at,expires_at,delivery_mode) VALUES($1,$2,$3,1100,2000,'envelope')")
        .bind([2_u8;32].as_slice()).bind(&id).bind(&basket).execute(&mut *tx).await.expect("proof");
    assert!(sqlx::query("INSERT INTO guest_link_delivery_envelopes(verifier,key_id,nonce_counter,envelope_version,ciphertext) VALUES($1,'guest-key-1',$2,1,$3)")
        .bind([2_u8;32].as_slice()).bind(nonce).bind([3_u8;48].as_slice()).execute(&mut *tx).await.is_err());
    tx.rollback().await.expect("rollback");
    assert_eq!(counts(&pool).await, (0, 0, 0));
    delivery::issue_for_delivery(&store, &guest, &csrf, &id, &key())
        .await
        .expect("healthy pair");
    assert_eq!(counts(&pool).await, (1, 1, 1));
    assert!(
        pool.execute(
            "UPDATE guest_link_delivery_envelopes SET ciphertext=decode(repeat('00',48),'hex')"
        )
        .await
        .is_err()
    );
    assert!(
        pool.execute("UPDATE guest_link_delivery_nonces SET reserved_at=reserved_at")
            .await
            .is_err()
    );
}

#[sqlx::test(migrations = false)]
async fn guest_order_link_delivery_populated_upgrade_preserves_existing_direct_proof_redemption(
    pool: PgPool,
) {
    use admitted_proofs::{customer_passwords, email_verification, verified_customer_accounts};
    MIGRATOR.run_to(66, &pool).await.expect("prior schema");
    quote::seed(&pool).await;
    let (store, clock) = quote::store(&pool, 1100);
    let (session, csrf) = quote::guest(&store).await;
    let guest = GuestCredential::from_session(&session).expect("guest");
    let id = order(&store, &guest, &csrf, "upgrade").await;
    let direct = guest_order_links::issue(&store, &guest, &csrf, &id)
        .await
        .expect("legacy proof");
    MIGRATOR.run(&pool).await.expect("delivery migration");
    let modes: Vec<(String, bool)> = sqlx::query_as(
        "SELECT delivery_mode,delivery_transaction_id IS NULL FROM guest_order_link_proofs",
    )
    .fetch_all(&pool)
    .await
    .expect("legacy mode");
    assert_eq!(modes, vec![("direct".into(), true)]);
    let email = "buyer@example.test";
    let proof = email_verification::issue(&pool, email, clock.as_ref())
        .await
        .expect("email proof");
    verified_customer_accounts::establish(&pool, email, &proof, clock.as_ref())
        .await
        .expect("verified account");
    let proof = customer_passwords::issue(&pool, email, clock.as_ref())
        .await
        .expect("password proof");
    customer_passwords::replace(
        &pool,
        email,
        &proof,
        "correct horse battery staple",
        clock.as_ref(),
    )
    .await
    .expect("password");
    let session =
        admitted_login::login(&pool, email, "correct horse battery staple", clock.as_ref())
            .await
            .expect("login");
    guest_order_links::link(&pool, &session.token, &id, &direct, clock.as_ref())
        .await
        .expect("legacy redemption");
    assert_eq!(counts(&pool).await, (0, 1, 0));
}

#[test]
fn guest_order_link_delivery_key_validation_rejects_invalid_material_and_ids() {
    for id in ["", "_prefix", "has.dot", &"a".repeat(65)] {
        assert!(matches!(
            DeliveryKey::capture(id, [7; 32]),
            Err(LinkError::Unavailable)
        ));
    }
    assert!(matches!(
        DeliveryKey::capture("key", [0; 32]),
        Err(LinkError::Unavailable)
    ));
    assert!(DeliveryKey::capture("A-1_valid", [7; 32]).is_ok());
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn guest_order_link_delivery_bad_authority_key_and_rewound_registration_fail_closed(
    pool: PgPool,
) {
    quote::seed(&pool).await;
    let (store, clock) = quote::store(&pool, 1100);
    let (session, csrf) = quote::guest(&store).await;
    let guest = GuestCredential::from_session(&session).expect("guest");
    let (foreign, _) = quote::guest(&store).await;
    let foreign = GuestCredential::from_session(&foreign).expect("foreign");
    let id = order(&store, &guest, &csrf, "authority").await;
    let delivery_key = key();
    assert!(matches!(
        delivery::issue_for_delivery(&store, &guest, &csrf, "invalid", &delivery_key).await,
        Err(LinkError::Denied)
    ));
    assert!(matches!(
        delivery::issue_for_delivery(&store, &foreign, &csrf, &id, &delivery_key).await,
        Err(LinkError::Denied)
    ));
    assert!(matches!(
        delivery::issue_for_delivery(&store, &guest, &csrf, &id, &delivery_key).await,
        Err(LinkError::Unavailable)
    ));
    assert_eq!(counts(&pool).await, (0, 0, 0));
    delivery::register_key(&pool, clock.as_ref(), &delivery_key)
        .await
        .expect("register");
    clock.0.store(1000, Ordering::SeqCst);
    assert!(matches!(
        delivery::register_key(&pool, clock.as_ref(), &delivery_key).await,
        Err(LinkError::Unavailable)
    ));
    clock.0.store(1100, Ordering::SeqCst);
    delivery::issue_for_delivery(&store, &guest, &csrf, &id, &delivery_key)
        .await
        .expect("healthy issue");
    assert_eq!(counts(&pool).await, (1, 1, 1));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn guest_order_link_delivery_altered_key_nonce_and_envelope_writes_fail_closed(pool: PgPool) {
    quote::seed(&pool).await;
    let (store, clock) = quote::store(&pool, 1100);
    let (session, csrf) = quote::guest(&store).await;
    let guest = GuestCredential::from_session(&session).expect("guest");
    let id = order(&store, &guest, &csrf, "alter").await;
    let delivery_key = key();
    pool.execute("CREATE FUNCTION fixture_key_tamper() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN NEW.registered_at=NEW.registered_at+1; RETURN NEW; END $$").await.expect("function");
    pool.execute("CREATE TRIGGER zz_key_tamper BEFORE INSERT ON guest_link_delivery_keys FOR EACH ROW EXECUTE FUNCTION fixture_key_tamper()").await.expect("trigger");
    assert!(matches!(
        delivery::register_key(&pool, clock.as_ref(), &delivery_key).await,
        Err(LinkError::Unavailable)
    ));
    pool.execute("DROP TRIGGER zz_key_tamper ON guest_link_delivery_keys")
        .await
        .expect("drop");
    delivery::register_key(&pool, clock.as_ref(), &delivery_key)
        .await
        .expect("register");
    pool.execute("CREATE FUNCTION fixture_nonce_tamper() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN NEW.reserved_at=NEW.reserved_at+1; RETURN NEW; END $$").await.expect("function");
    pool.execute("CREATE TRIGGER zz_nonce_tamper BEFORE INSERT ON guest_link_delivery_nonces FOR EACH ROW EXECUTE FUNCTION fixture_nonce_tamper()").await.expect("trigger");
    assert!(matches!(
        delivery::issue_for_delivery(&store, &guest, &csrf, &id, &delivery_key).await,
        Err(LinkError::Unavailable)
    ));
    assert_eq!(counts(&pool).await, (0, 0, 0));
    pool.execute("DROP TRIGGER zz_nonce_tamper ON guest_link_delivery_nonces")
        .await
        .expect("drop");
    pool.execute("CREATE FUNCTION fixture_envelope_tamper() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN NEW.ciphertext=decode(repeat('00',48),'hex'); RETURN NEW; END $$").await.expect("function");
    pool.execute("CREATE TRIGGER zz_envelope_tamper BEFORE INSERT ON guest_link_delivery_envelopes FOR EACH ROW EXECUTE FUNCTION fixture_envelope_tamper()").await.expect("trigger");
    assert!(matches!(
        delivery::issue_for_delivery(&store, &guest, &csrf, &id, &delivery_key).await,
        Err(LinkError::Unavailable)
    ));
    assert_eq!(counts(&pool).await, (1, 0, 0));
    pool.execute("DROP TRIGGER zz_envelope_tamper ON guest_link_delivery_envelopes")
        .await
        .expect("drop");
    delivery::issue_for_delivery(&store, &guest, &csrf, &id, &delivery_key)
        .await
        .expect("healthy issue");
    assert_eq!(counts(&pool).await, (2, 1, 1));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn guest_order_link_delivery_expired_proof_after_envelope_wait_rolls_back_pair(pool: PgPool) {
    quote::seed(&pool).await;
    let (store, clock) = quote::store(&pool, 1100);
    let (session, csrf) = quote::guest(&store).await;
    let guest = GuestCredential::from_session(&session).expect("guest");
    let id = order(&store, &guest, &csrf, "proof-wait").await;
    let delivery_key = key();
    delivery::register_key(&pool, clock.as_ref(), &delivery_key)
        .await
        .expect("register");
    pool.execute("CREATE TABLE fixture_guest_link_gate(id integer PRIMARY KEY); INSERT INTO fixture_guest_link_gate VALUES(1)").await.expect("gate");
    pool.execute("CREATE FUNCTION fixture_guest_link_wait() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM id FROM fixture_guest_link_gate WHERE id=1 FOR UPDATE; RETURN NEW; END $$").await.expect("function");
    pool.execute("CREATE TRIGGER zz_guest_link_wait BEFORE INSERT ON guest_link_delivery_envelopes FOR EACH ROW EXECUTE FUNCTION fixture_guest_link_wait()").await.expect("trigger");
    let mut gate = pool.begin().await.expect("transaction");
    sqlx::query("SELECT id FROM fixture_guest_link_gate WHERE id=1 FOR UPDATE")
        .fetch_one(&mut *gate)
        .await
        .expect("locked");
    let task = tokio::spawn(async move {
        delivery::issue_for_delivery(&store, &guest, &csrf, &id, &delivery_key).await
    });
    tokio::time::timeout(Duration::from_millis(750), async {
        loop {
            let waiting: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE '%INSERT INTO guest_link_delivery_envelopes%'").fetch_one(&pool).await.expect("observer");
            if waiting > 0 { break; }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }).await.expect("envelope write waits");
    clock.0.store(2000, Ordering::SeqCst);
    gate.commit().await.expect("release");
    assert!(matches!(task.await.expect("join"), Err(LinkError::Denied)));
    assert_eq!(counts(&pool).await, (1, 0, 0));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn guest_order_link_delivery_corrupt_registration_is_not_accepted(pool: PgPool) {
    quote::seed(&pool).await;
    let (_, clock) = quote::store(&pool, 1100);
    let delivery_key = key();
    delivery::register_key(&pool, clock.as_ref(), &delivery_key)
        .await
        .expect("register");
    // Corrupt native custody only inside this disposable database to prove the reader fails closed.
    pool.execute("ALTER TABLE guest_link_delivery_keys DROP CONSTRAINT guest_link_delivery_keys_registered_at_check; ALTER TABLE guest_link_delivery_keys DISABLE TRIGGER guard_guest_link_delivery_key; UPDATE guest_link_delivery_keys SET registered_at=-1; ALTER TABLE guest_link_delivery_keys ENABLE TRIGGER guard_guest_link_delivery_key").await.expect("synthetic corruption");
    assert!(matches!(
        delivery::register_key(&pool, clock.as_ref(), &delivery_key).await,
        Err(LinkError::Unavailable)
    ));
    assert_eq!(counts(&pool).await, (0, 0, 0));
}

async fn wait_on_key(pool: &PgPool) {
    tokio::time::timeout(Duration::from_millis(750), async {
        loop {
            let waiting: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE '%guest_link_delivery_keys%FOR UPDATE%'")
                .fetch_one(pool).await.expect("observer");
            if waiting > 0 { break; }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }).await.expect("actual key lock wait");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn guest_order_link_delivery_changed_guest_scope_after_nonce_wait_is_denied(pool: PgPool) {
    quote::seed(&pool).await;
    let (store, clock) = quote::store(&pool, 1100);
    let (session, csrf) = quote::guest(&store).await;
    let guest = GuestCredential::from_session(&session).expect("guest");
    let (_foreign, _) = quote::guest(&store).await;
    let id = order(&store, &guest, &csrf, "scope-change").await;
    let delivery_key = key();
    delivery::register_key(&pool, clock.as_ref(), &delivery_key)
        .await
        .expect("register");
    let basket: String = sqlx::query_scalar("SELECT basket_id FROM checkout_orders WHERE id=$1")
        .bind(&id)
        .fetch_one(&pool)
        .await
        .expect("basket");
    let other: String =
        sqlx::query_scalar("SELECT basket_id FROM guest_sessions WHERE basket_id<>$1 LIMIT 1")
            .bind(&basket)
            .fetch_one(&pool)
            .await
            .expect("other basket");
    let mut lock = pool.begin().await.expect("lock");
    sqlx::query(
        "SELECT key_id FROM guest_link_delivery_keys WHERE key_id='guest-key-1' FOR UPDATE",
    )
    .fetch_one(&mut *lock)
    .await
    .expect("locked");
    let task = tokio::spawn(async move {
        delivery::issue_for_delivery(&store, &guest, &csrf, &id, &delivery_key).await
    });
    wait_on_key(&pool).await;
    pool.execute("ALTER TABLE guest_sessions DISABLE TRIGGER guest_session_audit")
        .await
        .expect("synthetic corruption");
    sqlx::query("UPDATE guest_sessions SET basket_id=$2 WHERE basket_id=$1")
        .bind(&basket)
        .bind(&other)
        .execute(&pool)
        .await
        .expect("change scope");
    pool.execute("ALTER TABLE guest_sessions ENABLE TRIGGER guest_session_audit")
        .await
        .expect("restore trigger");
    lock.commit().await.expect("release");
    assert!(matches!(task.await.expect("join"), Err(LinkError::Denied)));
    assert_eq!(counts(&pool).await, (1, 0, 0));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn guest_order_link_delivery_changed_order_recipient_after_nonce_wait_is_denied(
    pool: PgPool,
) {
    quote::seed(&pool).await;
    let (store, clock) = quote::store(&pool, 1100);
    let (session, csrf) = quote::guest(&store).await;
    let guest = GuestCredential::from_session(&session).expect("guest");
    let id = order(&store, &guest, &csrf, "buyer-change").await;
    let delivery_key = key();
    delivery::register_key(&pool, clock.as_ref(), &delivery_key)
        .await
        .expect("register");
    let mut lock = pool.begin().await.expect("lock");
    sqlx::query(
        "SELECT key_id FROM guest_link_delivery_keys WHERE key_id='guest-key-1' FOR UPDATE",
    )
    .fetch_one(&mut *lock)
    .await
    .expect("locked");
    let task_id = id.clone();
    let task = tokio::spawn(async move {
        delivery::issue_for_delivery(&store, &guest, &csrf, &task_id, &delivery_key).await
    });
    wait_on_key(&pool).await;
    pool.execute("ALTER TABLE checkout_orders DISABLE TRIGGER guard_checkout_order")
        .await
        .expect("synthetic corruption");
    sqlx::query("UPDATE checkout_orders SET intent=jsonb_set(intent,'{buyer,email}',to_jsonb('other@example.test'::text)), snapshot=jsonb_set(snapshot,'{buyer,email}',to_jsonb('other@example.test'::text)) WHERE id=$1")
        .bind(&id).execute(&pool).await.expect("change recipient");
    pool.execute("ALTER TABLE checkout_orders ENABLE TRIGGER guard_checkout_order")
        .await
        .expect("restore trigger");
    lock.commit().await.expect("release");
    assert!(matches!(task.await.expect("join"), Err(LinkError::Denied)));
    assert_eq!(counts(&pool).await, (1, 0, 0));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn guest_order_link_delivery_commit_failure_preserves_nonce_without_proof(pool: PgPool) {
    quote::seed(&pool).await;
    let (store, clock) = quote::store(&pool, 1100);
    let (session, csrf) = quote::guest(&store).await;
    let guest = GuestCredential::from_session(&session).expect("guest");
    let id = order(&store, &guest, &csrf, "commit-fault").await;
    let delivery_key = key();
    delivery::register_key(&pool, clock.as_ref(), &delivery_key)
        .await
        .expect("register");
    pool.execute("CREATE FUNCTION fixture_guest_link_commit_error() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'synthetic'; END $$").await.expect("function");
    pool.execute("CREATE CONSTRAINT TRIGGER fixture_guest_link_commit_error AFTER INSERT ON guest_order_link_proofs DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION fixture_guest_link_commit_error()").await.expect("trigger");
    assert!(matches!(
        delivery::issue_for_delivery(&store, &guest, &csrf, &id, &delivery_key).await,
        Err(LinkError::Unavailable)
    ));
    assert_eq!(counts(&pool).await, (1, 0, 0));
    pool.execute("DROP TRIGGER fixture_guest_link_commit_error ON guest_order_link_proofs")
        .await
        .expect("restore");
    delivery::issue_for_delivery(&store, &guest, &csrf, &id, &delivery_key)
        .await
        .expect("healthy issue");
    assert_eq!(counts(&pool).await, (2, 1, 1));
}

struct FinalFenceClock {
    calls: std::sync::atomic::AtomicUsize,
    switch_at: std::sync::atomic::AtomicUsize,
    value: std::sync::atomic::AtomicU64,
}
impl commerce_application::request_context::Clock for FinalFenceClock {
    fn unix_seconds(&self) -> Result<u64, commerce_application::request_context::ContextError> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        if call == self.switch_at.load(Ordering::SeqCst) {
            self.value.store(2000, Ordering::SeqCst);
        }
        Ok(self.value.load(Ordering::SeqCst))
    }
}
struct UnusedIds;
impl commerce_application::request_context::Identifiers for UnusedIds {
    fn next(&self) -> Result<[u8; 16], commerce_application::request_context::ContextError> {
        Ok([1; 16])
    }
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn guest_order_link_delivery_expiry_at_final_outcome_fence_rolls_back_pair(pool: PgPool) {
    use std::sync::{
        Arc,
        atomic::{AtomicU64, AtomicUsize},
    };
    quote::seed(&pool).await;
    let (original, clock) = quote::store(&pool, 1100);
    let (session, csrf) = quote::guest(&original).await;
    let guest = GuestCredential::from_session(&session).expect("guest");
    let id = order(&original, &guest, &csrf, "final-fence").await;
    let key = key();
    delivery::register_key(&pool, clock.as_ref(), &key)
        .await
        .expect("register");
    let measured = Arc::new(FinalFenceClock {
        calls: AtomicUsize::new(0),
        switch_at: AtomicUsize::new(usize::MAX),
        value: AtomicU64::new(1100),
    });
    let store = GuestSessionStore::new(
        pool.clone(),
        measured.clone(),
        Arc::new(UnusedIds),
        commerce_application::guest_access::GuestLimits::default(),
    )
    .expect("store");
    delivery::issue_for_delivery(&store, &guest, &csrf, &id, &key)
        .await
        .expect("baseline issue");
    let calls = measured.calls.swap(0, Ordering::SeqCst);
    assert!(calls > 1);
    measured.switch_at.store(calls, Ordering::SeqCst);
    assert!(matches!(
        delivery::issue_for_delivery(&store, &guest, &csrf, &id, &key).await,
        Err(LinkError::Denied)
    ));
    assert_eq!(counts(&pool).await, (2, 1, 1));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn guest_order_link_delivery_rejects_prior_nonce_for_different_verifier(pool: PgPool) {
    quote::seed(&pool).await;
    let (store, clock) = quote::store(&pool, 1100);
    let (session, csrf) = quote::guest(&store).await;
    let guest = GuestCredential::from_session(&session).expect("guest");
    let id = order(&store, &guest, &csrf, "wrong-nonce").await;
    let basket: String = sqlx::query_scalar("SELECT basket_id FROM checkout_orders WHERE id=$1")
        .bind(&id)
        .fetch_one(&pool)
        .await
        .expect("basket");
    delivery::register_key(&pool, clock.as_ref(), &key())
        .await
        .expect("register");
    let mut tx = pool.begin().await.expect("nonce transaction");
    sqlx::query("SET LOCAL synchronous_commit='on'")
        .execute(&mut *tx)
        .await
        .expect("durable");
    let counter: i64 = sqlx::query_scalar("INSERT INTO guest_link_delivery_nonces(key_id,verifier,reserved_at) VALUES('guest-key-1',$1,1100) RETURNING nonce_counter")
        .bind([3_u8;32].as_slice()).fetch_one(&mut *tx).await.expect("prior nonce");
    tx.commit().await.expect("commit nonce");
    let mut tx = pool.begin().await.expect("proof transaction");
    sqlx::query("SET LOCAL synchronous_commit='on'")
        .execute(&mut *tx)
        .await
        .expect("durable");
    sqlx::query("INSERT INTO guest_order_link_proofs(verifier,order_id,basket_id,created_at,expires_at,delivery_mode) VALUES($1,$2,$3,1100,2000,'envelope')")
        .bind([4_u8;32].as_slice()).bind(&id).bind(&basket).execute(&mut *tx).await.expect("proof");
    assert!(sqlx::query("INSERT INTO guest_link_delivery_envelopes(verifier,key_id,nonce_counter,envelope_version,ciphertext) VALUES($1,'guest-key-1',$2,1,$3)")
        .bind([4_u8;32].as_slice()).bind(counter).bind([3_u8;48].as_slice()).execute(&mut *tx).await.is_err());
    tx.rollback().await.expect("rollback");
    assert_eq!(counts(&pool).await, (1, 0, 0));
    delivery::issue_for_delivery(&store, &guest, &csrf, &id, &key())
        .await
        .expect("healthy");
    assert_eq!(counts(&pool).await, (2, 1, 1));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn guest_order_link_delivery_upgrades_async_session_to_durable_commits(pool: PgPool) {
    let single = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_with(pool.connect_options().as_ref().clone())
        .await
        .expect("single connection pool");
    quote::seed(&single).await;
    let (store, clock) = quote::store(&single, 1100);
    let (session, csrf) = quote::guest(&store).await;
    let guest = GuestCredential::from_session(&session).expect("guest");
    let id = order(&store, &guest, &csrf, "sync-commit").await;
    single
        .execute("SET synchronous_commit='off'")
        .await
        .expect("async session");
    let delivery_key = key();
    delivery::register_key(&single, clock.as_ref(), &delivery_key)
        .await
        .expect("durable registration");
    let mut unsafe_tx = single.begin().await.expect("transaction");
    assert!(sqlx::query("INSERT INTO guest_link_delivery_nonces(key_id,verifier,reserved_at) VALUES('guest-key-1',$1,1100)")
        .bind([8_u8;32].as_slice()).execute(&mut *unsafe_tx).await.is_err());
    unsafe_tx.rollback().await.expect("rollback");
    delivery::issue_for_delivery(&store, &guest, &csrf, &id, &delivery_key)
        .await
        .expect("locally durable issue");
    assert_eq!(counts(&single).await, (1, 1, 1));
    let setting: String = sqlx::query_scalar("SHOW synchronous_commit")
        .fetch_one(&single)
        .await
        .expect("session setting");
    assert_eq!(setting, "off");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn guest_link_open_commits_one_start_and_only_fresh_winner_receives_bearer(pool: PgPool) {
    use delivery::OpenOutcome;
    use sha2::{Digest, Sha256};
    quote::seed(&pool).await;
    let (store, clock) = quote::store(&pool, 1100);
    let (session, csrf) = quote::guest(&store).await;
    let guest = GuestCredential::from_session(&session).expect("guest");
    let id = order(&store, &guest, &csrf, "open").await;
    let key = key();
    delivery::register_key(&pool, clock.as_ref(), &key)
        .await
        .expect("key");
    let receipt = delivery::issue_for_delivery(&store, &guest, &csrf, &id, &key)
        .await
        .expect("issue");
    let wrong = DeliveryKey::capture("guest-key-1", [8; 32]).expect("wrong key");
    assert!(matches!(
        delivery::claim_and_open(
            &pool,
            &receipt.verifier,
            clock.as_ref(),
            &wrong,
            |_, _| panic!("wrong key opened")
        )
        .await,
        Err(LinkError::Denied)
    ));
    let mut captured = None;
    let result = delivery::claim_and_open(
        &pool,
        &receipt.verifier,
        clock.as_ref(),
        &key,
        |email, token| {
            assert_eq!(email, "buyer@example.test");
            captured = Some(*token.expose_bytes());
        },
    )
    .await
    .expect("open");
    assert_eq!(result, OpenOutcome::Opened);
    let verifier = Sha256::new()
        .chain_update(b"piwas:guest-order-link:verifier:v1\0")
        .chain_update(captured.expect("bearer"))
        .finalize();
    assert_eq!(verifier[..], receipt.verifier);
    assert_eq!(
        delivery::claim_and_open(
            &pool,
            &receipt.verifier,
            clock.as_ref(),
            &key,
            |_, _| panic!("reopened")
        )
        .await
        .expect("already"),
        OpenOutcome::AlreadyStarted
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM guest_link_delivery_starts")
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(count, 1);
    assert!(
        sqlx::query("DELETE FROM guest_link_delivery_starts WHERE verifier=$1")
            .bind(receipt.verifier.as_slice())
            .execute(&pool)
            .await
            .is_err()
    );
    assert!(matches!(
        delivery::claim_and_open(&pool, &[0; 32], clock.as_ref(), &key, |_, _| panic!(
            "unknown opened"
        ))
        .await,
        Err(LinkError::Denied)
    ));
    use admitted_proofs::{
        customer_passwords, email_verification, verified_customer_accounts::establish,
    };
    let email = "buyer@example.test";
    let proof = email_verification::issue(&pool, email, clock.as_ref())
        .await
        .expect("email proof");
    establish(&pool, email, &proof, clock.as_ref())
        .await
        .expect("account");
    let proof = customer_passwords::issue(&pool, email, clock.as_ref())
        .await
        .expect("password proof");
    customer_passwords::replace(
        &pool,
        email,
        &proof,
        "correct horse battery staple",
        clock.as_ref(),
    )
    .await
    .expect("password");
    let customer =
        admitted_login::login(&pool, email, "correct horse battery staple", clock.as_ref())
            .await
            .expect("customer");
    let proof = guest_order_links::GuestOrderLinkToken::from_bytes(captured.expect("bearer"));
    guest_order_links::link(&pool, &customer.token, &id, &proof, clock.as_ref())
        .await
        .expect("opened proof redeems");
    assert!(matches!(
        delivery::claim_and_open(
            &pool,
            &receipt.verifier,
            clock.as_ref(),
            &key,
            |_, _| panic!("consumed reopened")
        )
        .await,
        Err(LinkError::Denied)
    ));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn guest_link_open_rechecks_expiry_after_proof_lock_wait(pool: PgPool) {
    quote::seed(&pool).await;
    let (store, clock) = quote::store(&pool, 1100);
    let (session, csrf) = quote::guest(&store).await;
    let guest = GuestCredential::from_session(&session).expect("guest");
    let id = order(&store, &guest, &csrf, "open-wait").await;
    let key = key();
    delivery::register_key(&pool, clock.as_ref(), &key)
        .await
        .expect("key");
    let receipt = delivery::issue_for_delivery(&store, &guest, &csrf, &id, &key)
        .await
        .expect("issue");
    let mut lock = pool.begin().await.expect("tx");
    sqlx::query("SELECT verifier FROM guest_order_link_proofs WHERE verifier=$1 FOR UPDATE")
        .bind(receipt.verifier.as_slice())
        .fetch_one(&mut *lock)
        .await
        .expect("lock");
    let other = pool.clone();
    let wait_clock = clock.clone();
    let verifier = receipt.verifier;
    let pending = tokio::spawn(async move {
        delivery::claim_and_open(&other, &verifier, wait_clock.as_ref(), &key, |_, _| {
            panic!("expired opened")
        })
        .await
    });
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(!pending.is_finished());
    clock.0.store(receipt.expires_at as u64, Ordering::SeqCst);
    lock.commit().await.expect("release");
    assert!(matches!(
        pending.await.expect("join"),
        Err(LinkError::Denied)
    ));
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM guest_link_delivery_starts")
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(count, 0);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn guest_link_open_concurrent_start_has_one_winner(pool: PgPool) {
    use delivery::OpenOutcome;
    use std::sync::atomic::AtomicUsize;
    quote::seed(&pool).await;
    let (store, clock) = quote::store(&pool, 1100);
    let (session, csrf) = quote::guest(&store).await;
    let guest = GuestCredential::from_session(&session).expect("guest");
    let id = order(&store, &guest, &csrf, "open-race").await;
    let key = key();
    delivery::register_key(&pool, clock.as_ref(), &key)
        .await
        .expect("key");
    let receipt = delivery::issue_for_delivery(&store, &guest, &csrf, &id, &key)
        .await
        .expect("issue");
    let calls = AtomicUsize::new(0);
    let (first, second) = tokio::join!(
        delivery::claim_and_open(&pool, &receipt.verifier, clock.as_ref(), &key, |_, _| {
            calls.fetch_add(1, Ordering::SeqCst);
        }),
        delivery::claim_and_open(&pool, &receipt.verifier, clock.as_ref(), &key, |_, _| {
            calls.fetch_add(1, Ordering::SeqCst);
        }),
    );
    let outcomes = [first.expect("first"), second.expect("second")];
    assert!(outcomes.contains(&OpenOutcome::Opened));
    assert!(outcomes.contains(&OpenOutcome::AlreadyStarted));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn guest_link_open_authentication_failure_burns_start_and_cannot_retry(pool: PgPool) {
    quote::seed(&pool).await;
    let (store, clock) = quote::store(&pool, 1100);
    let (session, csrf) = quote::guest(&store).await;
    let guest = GuestCredential::from_session(&session).expect("guest");
    let id = order(&store, &guest, &csrf, "open-corrupt").await;
    let key = key();
    delivery::register_key(&pool, clock.as_ref(), &key)
        .await
        .expect("key");
    let receipt = delivery::issue_for_delivery(&store, &guest, &csrf, &id, &key)
        .await
        .expect("issue");
    // Simulate an at-rest same-length corruption without changing ordinary writer guards.
    sqlx::query("ALTER TABLE guest_link_delivery_envelopes DISABLE TRIGGER guard_guest_link_delivery_envelope").execute(&pool).await.expect("disable guard");
    sqlx::query("UPDATE guest_link_delivery_envelopes SET ciphertext=set_byte(ciphertext,0,get_byte(ciphertext,0) # 1) WHERE verifier=$1").bind(receipt.verifier.as_slice()).execute(&pool).await.expect("corrupt");
    sqlx::query("ALTER TABLE guest_link_delivery_envelopes ENABLE TRIGGER guard_guest_link_delivery_envelope").execute(&pool).await.expect("restore guard");
    assert!(matches!(
        delivery::claim_and_open(
            &pool,
            &receipt.verifier,
            clock.as_ref(),
            &key,
            |_, _| panic!("corruption opened")
        )
        .await,
        Err(LinkError::Unavailable)
    ));
    assert_eq!(
        delivery::claim_and_open(
            &pool,
            &receipt.verifier,
            clock.as_ref(),
            &key,
            |_, _| panic!("corruption retried")
        )
        .await
        .expect("already"),
        delivery::OpenOutcome::AlreadyStarted
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM guest_link_delivery_starts")
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(count, 1);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn guest_link_start_native_guards_bind_source_and_separate_consumption(pool: PgPool) {
    use admitted_proofs::{email_verification, verified_customer_accounts::establish};
    quote::seed(&pool).await;
    let (store, clock) = quote::store(&pool, 1100);
    let (session, csrf) = quote::guest(&store).await;
    let guest = GuestCredential::from_session(&session).expect("guest");
    let id = order(&store, &guest, &csrf, "start-native").await;
    let key = key();
    delivery::register_key(&pool, clock.as_ref(), &key)
        .await
        .expect("key");
    let receipt = delivery::issue_for_delivery(&store, &guest, &csrf, &id, &key)
        .await
        .expect("issue");
    let verify = email_verification::issue(&pool, "buyer@example.test", clock.as_ref())
        .await
        .expect("verification");
    establish(&pool, "buyer@example.test", &verify, clock.as_ref())
        .await
        .expect("account");
    let account: i64 = sqlx::query_scalar(
        "SELECT id FROM verified_customer_accounts WHERE email_identity='buyer@example.test'",
    )
    .fetch_one(&pool)
    .await
    .expect("id");
    let insertion = "INSERT INTO guest_link_delivery_starts(verifier,key_id,nonce_counter,key_fingerprint,started_at) SELECT e.verifier,e.key_id,e.nonce_counter,k.key_fingerprint,$2 FROM guest_link_delivery_envelopes e JOIN guest_link_delivery_keys k ON k.key_id=e.key_id WHERE e.verifier=$1";
    assert!(
        sqlx::query(insertion)
            .bind(receipt.verifier.as_slice())
            .bind(receipt.expires_at)
            .execute(&pool)
            .await
            .is_err()
    );
    let mut tx = pool.begin().await.expect("tx");
    sqlx::query("SET LOCAL synchronous_commit='on'")
        .execute(&mut *tx)
        .await
        .expect("durable");
    sqlx::query(insertion)
        .bind(receipt.verifier.as_slice())
        .bind(receipt.created_at)
        .execute(&mut *tx)
        .await
        .expect("start");
    sqlx::query("SET CONSTRAINTS require_guest_link_delivery_start IMMEDIATE")
        .execute(&mut *tx)
        .await
        .expect("flush");
    let error = sqlx::query("UPDATE guest_order_link_proofs SET consumed_at=$2,consumed_account_id=$3 WHERE verifier=$1")
        .bind(receipt.verifier.as_slice()).bind(receipt.created_at).bind(account).execute(&mut *tx).await.expect_err("same transaction must fail");
    assert!(
        error
            .to_string()
            .contains("Start and consumption require separate transactions")
    );
    tx.rollback().await.expect("rollback");
    // Flushing the start check cannot permit linking the same order via a second proof.
    let second = delivery::issue_for_delivery(&store, &guest, &csrf, &id, &key)
        .await
        .expect("second proof");
    let mut tx = pool.begin().await.expect("tx");
    sqlx::query("SET LOCAL synchronous_commit='on'")
        .execute(&mut *tx)
        .await
        .expect("durable");
    sqlx::query(insertion)
        .bind(receipt.verifier.as_slice())
        .bind(receipt.created_at)
        .execute(&mut *tx)
        .await
        .expect("start");
    sqlx::query("SET CONSTRAINTS require_guest_link_delivery_start IMMEDIATE")
        .execute(&mut *tx)
        .await
        .expect("flush");
    sqlx::query("UPDATE guest_order_link_proofs SET consumed_at=$2,consumed_account_id=$3 WHERE verifier=$1")
        .bind(second.verifier.as_slice()).bind(second.created_at).bind(account).execute(&mut *tx).await.expect("second proof consumed");
    let error = sqlx::query("INSERT INTO customer_order_links(order_id,account_id,proof_verifier,linked_at,linked_xid) VALUES($1,$2,$3,$4,NULL)")
        .bind(&id).bind(account).bind(second.verifier.as_slice()).bind(second.created_at).execute(&mut *tx).await.expect_err("same transaction order linkage must fail");
    assert!(
        error
            .to_string()
            .contains("Start and order linkage require separate transactions")
    );
    tx.rollback().await.expect("rollback");
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM guest_link_delivery_starts")
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(count, 0);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn guest_link_open_refuses_expiry_after_durable_start(pool: PgPool) {
    use commerce_application::request_context::{Clock, ContextError};
    use std::sync::atomic::AtomicUsize;
    struct ExpiresAfterStart(AtomicUsize);
    impl Clock for ExpiresAfterStart {
        fn unix_seconds(&self) -> Result<u64, ContextError> {
            Ok(if self.0.fetch_add(1, Ordering::SeqCst) < 6 {
                1100
            } else {
                2000
            })
        }
    }
    quote::seed(&pool).await;
    let (store, clock) = quote::store(&pool, 1100);
    let (session, csrf) = quote::guest(&store).await;
    let guest = GuestCredential::from_session(&session).expect("guest");
    let id = order(&store, &guest, &csrf, "post-start-expiry").await;
    let key = key();
    delivery::register_key(&pool, clock.as_ref(), &key)
        .await
        .expect("key");
    let receipt = delivery::issue_for_delivery(&store, &guest, &csrf, &id, &key)
        .await
        .expect("issue");
    let late = ExpiresAfterStart(AtomicUsize::new(0));
    assert!(matches!(
        delivery::claim_and_open(&pool, &receipt.verifier, &late, &key, |_, _| panic!(
            "expired opened"
        ))
        .await,
        Err(LinkError::Denied)
    ));
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM guest_link_delivery_starts WHERE verifier=$1")
            .bind(receipt.verifier.as_slice())
            .fetch_one(&pool)
            .await
            .expect("start committed");
    assert_eq!(count, 1);
    assert_eq!(
        delivery::claim_and_open(
            &pool,
            &receipt.verifier,
            clock.as_ref(),
            &key,
            |_, _| panic!("uncertain start retried")
        )
        .await
        .expect("already"),
        delivery::OpenOutcome::AlreadyStarted
    );
}

// Wait for an actual server-side lock waiter, not merely a scheduled Rust task.
async fn wait_for_lock_query(pool: &PgPool, pattern: &str) {
    tokio::time::timeout(Duration::from_millis(800), async {
        loop {
            let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND pid<>pg_backend_pid() AND wait_event_type='Lock' AND cardinality(pg_blocking_pids(pid))>0 AND query LIKE $1)")
                .bind(pattern).fetch_one(pool).await.expect("observe waiter");
            if waiting {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("expected PostgreSQL lock waiter");
}

async fn compete_link_with_open(pool: PgPool, after_start: bool) {
    use admitted_proofs::{
        customer_passwords, email_verification, verified_customer_accounts::establish,
    };
    use sha2::{Digest, Sha256};
    use std::sync::{Arc, atomic::AtomicUsize};

    quote::seed(&pool).await;
    let (store, clock) = quote::store(&pool, 1100);
    let (session, csrf) = quote::guest(&store).await;
    let guest = GuestCredential::from_session(&session).expect("guest");
    let id = order(
        &store,
        &guest,
        &csrf,
        if after_start {
            "compete-after"
        } else {
            "compete-before"
        },
    )
    .await;
    let key = key();
    delivery::register_key(&pool, clock.as_ref(), &key)
        .await
        .expect("key");
    let receipt = delivery::issue_for_delivery(&store, &guest, &csrf, &id, &key)
        .await
        .expect("encrypted proof");
    let other = guest_order_links::issue(&store, &guest, &csrf, &id)
        .await
        .expect("other proof");
    let other_verifier = Sha256::new()
        .chain_update(b"piwas:guest-order-link:verifier:v1\0")
        .chain_update(other.expose_bytes())
        .finalize();
    let email = "buyer@example.test";
    let proof = email_verification::issue(&pool, email, clock.as_ref())
        .await
        .expect("email proof");
    establish(&pool, email, &proof, clock.as_ref())
        .await
        .expect("account");
    let proof = customer_passwords::issue(&pool, email, clock.as_ref())
        .await
        .expect("password proof");
    customer_passwords::replace(
        &pool,
        email,
        &proof,
        "correct horse battery staple",
        clock.as_ref(),
    )
    .await
    .expect("password");
    let customer =
        admitted_login::login(&pool, email, "correct horse battery staple", clock.as_ref())
            .await
            .expect("session");

    // Both locks are held by this connection; all triggers exist only in the isolated test database.
    let start_lock: i64 = if after_start { 7138401 } else { 7138301 };
    let link_lock: i64 = if after_start { 7138402 } else { 7138302 };
    let mut controller = pool.acquire().await.expect("controller");
    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(link_lock)
        .execute(&mut *controller)
        .await
        .expect("link latch");
    if after_start {
        sqlx::query("SELECT pg_advisory_lock($1)")
            .bind(start_lock)
            .execute(&mut *controller)
            .await
            .expect("start latch");
        sqlx::query("CREATE FUNCTION fixture_pause_start() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_advisory_xact_lock(7138401); RETURN NEW; END $$")
            .execute(&pool).await.expect("start function");
        sqlx::query("CREATE TRIGGER zz_fixture_pause_start BEFORE INSERT ON guest_link_delivery_starts FOR EACH ROW EXECUTE FUNCTION fixture_pause_start()")
            .execute(&pool).await.expect("start trigger");
    }
    let link_function = if after_start {
        "CREATE FUNCTION fixture_pause_link() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_advisory_xact_lock(7138402); RETURN NEW; END $$"
    } else {
        "CREATE FUNCTION fixture_pause_link() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_advisory_xact_lock(7138302); RETURN NEW; END $$"
    };
    sqlx::query(link_function)
        .execute(&pool)
        .await
        .expect("link function");
    sqlx::query("CREATE TRIGGER zz_fixture_pause_link BEFORE INSERT ON customer_order_links FOR EACH ROW EXECUTE FUNCTION fixture_pause_link()")
        .execute(&pool).await.expect("link trigger");

    let calls = Arc::new(AtomicUsize::new(0));
    let spawn_open = || {
        let pool = pool.clone();
        let clock = clock.clone();
        let calls = calls.clone();
        let verifier = receipt.verifier;
        let key = DeliveryKey::capture("guest-key-1", [7; 32]).expect("key");
        tokio::spawn(async move {
            delivery::claim_and_open(&pool, &verifier, clock.as_ref(), &key, |_, _| {
                calls.fetch_add(1, Ordering::SeqCst);
            })
            .await
        })
    };
    let spawn_link = || {
        let pool = pool.clone();
        let clock = clock.clone();
        let id = id.clone();
        tokio::spawn(async move {
            guest_order_links::link(&pool, &customer.token, &id, &other, clock.as_ref()).await
        })
    };
    let (opening, linking) = if after_start {
        let opening = spawn_open();
        wait_for_lock_query(&pool, "%INSERT INTO guest_link_delivery_starts%").await;
        let linking = spawn_link();
        wait_for_lock_query(&pool, "%SELECT id FROM checkout_orders%FOR UPDATE%").await;
        sqlx::query("SELECT pg_advisory_unlock($1)")
            .bind(start_lock)
            .execute(&mut *controller)
            .await
            .expect("release start");
        wait_for_lock_query(&pool, "%INSERT INTO customer_order_links%").await;
        wait_for_lock_query(&pool, "%SELECT id FROM checkout_orders%FOR UPDATE%").await;
        (opening, linking)
    } else {
        let linking = spawn_link();
        wait_for_lock_query(&pool, "%INSERT INTO customer_order_links%").await;
        let opening = spawn_open();
        wait_for_lock_query(&pool, "%SELECT id FROM checkout_orders%FOR UPDATE%").await;
        (opening, linking)
    };
    let starts: i64 =
        sqlx::query_scalar("SELECT count(*) FROM guest_link_delivery_starts WHERE verifier=$1")
            .bind(receipt.verifier.as_slice())
            .fetch_one(&pool)
            .await
            .expect("start visibility");
    assert_eq!(starts, i64::from(after_start));
    assert!(!opening.is_finished());
    assert!(!linking.is_finished());
    sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(link_lock)
        .execute(&mut *controller)
        .await
        .expect("release link");
    linking
        .await
        .expect("link task")
        .expect("competing proof links order");
    assert!(matches!(
        opening.await.expect("open task"),
        Err(LinkError::Denied)
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let (linked_verifier, still_unconsumed): (Vec<u8>, bool) = sqlx::query_as("SELECT (SELECT proof_verifier FROM customer_order_links WHERE order_id=$2), consumed_at IS NULL AND consumed_account_id IS NULL AND consumed_xid IS NULL FROM guest_order_link_proofs WHERE verifier=$1")
        .bind(receipt.verifier.as_slice()).bind(&id).fetch_one(&pool).await.expect("result custody");
    assert_eq!(linked_verifier.as_slice(), &other_verifier[..]);
    assert!(still_unconsumed);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn guest_link_open_waits_for_competing_order_link_before_start(pool: PgPool) {
    compete_link_with_open(pool, false).await;
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn guest_link_open_waits_for_competing_order_link_after_committed_start(pool: PgPool) {
    compete_link_with_open(pool, true).await;
}
