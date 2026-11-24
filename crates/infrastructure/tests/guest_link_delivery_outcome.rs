//! Synthetic in-process callback outcomes against isolated PostgreSQL only.
use commerce_application::Locale;
use commerce_infrastructure::{
    MIGRATOR,
    checkout_orders::create_order,
    checkout_quotes::issue_quote,
    guest_baskets::GuestCredential,
    guest_order_links::{
        self, LinkError,
        delivery::{
            self, DeliveryKey, OpenOutcome,
            outcome::{self, DeliveryState, TransportOutcome},
        },
    },
};
use sqlx::{Executor, PgPool};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

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
#[cfg(feature = "test-verification-smtp")]
#[path = "support/guest_link_test_smtp.rs"]
mod test_smtp_transport;

async fn sample(pool: &PgPool, suffix: &str) -> (Arc<quote::Time>, DeliveryKey, [u8; 32], String) {
    sample_with_buyer(pool, suffix, "buyer@example.test").await
}

async fn sample_with_buyer(
    pool: &PgPool,
    suffix: &str,
    email: &str,
) -> (Arc<quote::Time>, DeliveryKey, [u8; 32], String) {
    quote::seed(pool).await;
    let (store, clock) = quote::store(pool, 1100);
    let (session, csrf) = quote::guest(&store).await;
    let guest = GuestCredential::from_session(&session).expect("guest");
    let value = issue_quote(
        &store,
        &guest,
        &csrf,
        Locale::Nl,
        &quote::request(&format!("q-{suffix}")),
    )
    .await
    .expect("quote")
    .snapshot;
    let mut intent = fixture::intent(&value.binding.quote_id);
    intent.buyer.email = email.into();
    let id = create_order(
        &store,
        &guest,
        &csrf,
        &format!("o-{suffix}"),
        &intent,
        &fixture::seller(),
    )
    .await
    .expect("order")
    .snapshot
    .identity
    .id;
    let key = DeliveryKey::capture("guest-outcome-key", [7; 32]).expect("key");
    delivery::register_key(pool, clock.as_ref(), &key)
        .await
        .expect("register");
    let receipt = delivery::issue_for_delivery(&store, &guest, &csrf, &id, &key)
        .await
        .expect("issue");
    (clock, key, receipt.verifier, id)
}

async fn classification(pool: PgPool, expected: TransportOutcome) {
    let (clock, key, verifier, _) = sample(&pool, "classification").await;
    let effects = || async {
        sqlx::query_as::<_, (i64, i64, i64, i64)>("SELECT (SELECT count(*) FROM commerce_outbox),(SELECT count(*) FROM stock_movements),(SELECT count(*) FROM payment_attempts),(SELECT count(*) FROM customer_order_links)")
            .fetch_one(&pool).await.expect("business effects")
    };
    let before = effects().await;
    assert_eq!(
        outcome::inspect(&pool, &verifier).await,
        Ok(DeliveryState::NotStarted)
    );
    let calls = AtomicUsize::new(0);
    assert_eq!(
        delivery::claim_open_and_record(&pool, &verifier, clock.as_ref(), &key, |email, _| {
            assert_eq!(email, "buyer@example.test");
            calls.fetch_add(1, Ordering::SeqCst);
            expected
        })
        .await,
        Ok(OpenOutcome::Opened)
    );
    assert_eq!(
        outcome::inspect(&pool, &verifier).await,
        Ok(DeliveryState::Recorded(expected))
    );
    assert_eq!(
        delivery::claim_open_and_record(&pool, &verifier, clock.as_ref(), &key, |_, _| {
            calls.fetch_add(1, Ordering::SeqCst);
            expected
        })
        .await,
        Ok(OpenOutcome::AlreadyStarted)
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(effects().await, before);
    let digest_matches: bool = sqlx::query_scalar("SELECT o.ciphertext_sha256=sha256(e.ciphertext) FROM guest_link_delivery_outcomes o JOIN guest_link_delivery_envelopes e USING(verifier) WHERE o.verifier=$1").bind(verifier.as_slice()).fetch_one(&pool).await.expect("ciphertext commitment");
    assert!(digest_matches);
    assert!(
        sqlx::query("UPDATE guest_link_delivery_outcomes SET outcome='accepted' WHERE verifier=$1")
            .bind(verifier.as_slice())
            .execute(&pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("DELETE FROM guest_link_delivery_outcomes WHERE verifier=$1")
            .bind(verifier.as_slice())
            .execute(&pool)
            .await
            .is_err()
    );
    let columns: Vec<String> = sqlx::query_scalar("SELECT column_name::text FROM information_schema.columns WHERE table_schema='public' AND table_name='guest_link_delivery_outcomes' ORDER BY ordinal_position")
        .fetch_all(&pool).await.expect("columns");
    assert_eq!(
        columns,
        [
            "verifier",
            "start_transaction_id",
            "ciphertext_sha256",
            "outcome",
            "observed_at",
            "transaction_id"
        ]
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn guest_link_outcome_accepted(pool: PgPool) {
    classification(pool, TransportOutcome::Accepted).await;
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn guest_link_outcome_rejected(pool: PgPool) {
    classification(pool, TransportOutcome::ExplicitRejection).await;
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn guest_link_outcome_uncertain(pool: PgPool) {
    classification(pool, TransportOutcome::Uncertain).await;
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn guest_link_outcome_lost_result_stays_unknown_and_never_reopens(pool: PgPool) {
    let (clock, key, verifier, _) = sample(&pool, "lost").await;
    pool.execute("CREATE FUNCTION suppress_guest_link_outcome() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RETURN NULL; END $$; CREATE TRIGGER z_suppress_guest_link_outcome BEFORE INSERT ON guest_link_delivery_outcomes FOR EACH ROW EXECUTE FUNCTION suppress_guest_link_outcome()")
        .await.expect("synthetic outcome loss");
    assert_eq!(
        delivery::claim_open_and_record(&pool, &verifier, clock.as_ref(), &key, |_, _| {
            TransportOutcome::Accepted
        })
        .await,
        Err(LinkError::Unavailable)
    );
    assert_eq!(
        outcome::inspect(&pool, &verifier).await,
        Ok(DeliveryState::Unknown)
    );
    pool.execute("DROP TRIGGER z_suppress_guest_link_outcome ON guest_link_delivery_outcomes")
        .await
        .expect("restore");
    pool.execute("ALTER TABLE guest_link_delivery_envelopes DISABLE TRIGGER USER")
        .await
        .expect("fault setup");
    sqlx::query("UPDATE guest_link_delivery_envelopes SET ciphertext=set_byte(ciphertext,0,get_byte(ciphertext,0) # 1) WHERE verifier=$1").bind(verifier.as_slice()).execute(&pool).await.expect("same-length fault");
    // With no authenticated outcome, Unknown asserts only relational custody.
    assert_eq!(
        outcome::inspect(&pool, &verifier).await,
        Ok(DeliveryState::Unknown)
    );
    sqlx::query("UPDATE guest_link_delivery_envelopes SET ciphertext=set_byte(ciphertext,0,get_byte(ciphertext,0) # 1) WHERE verifier=$1").bind(verifier.as_slice()).execute(&pool).await.expect("restore ciphertext");
    pool.execute("ALTER TABLE guest_link_delivery_envelopes ENABLE TRIGGER USER")
        .await
        .expect("restore guards");
    assert_eq!(
        delivery::claim_open_and_record(&pool, &verifier, clock.as_ref(), &key, |_, _| panic!(
            "must not reopen"
        ))
        .await,
        Ok(OpenOutcome::AlreadyStarted)
    );
    assert_eq!(
        outcome::inspect(&pool, &verifier).await,
        Ok(DeliveryState::Unknown)
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn guest_link_outcome_native_guards_require_exact_prior_start(pool: PgPool) {
    let (clock, key, verifier, _) = sample(&pool, "native").await;
    let mut same = pool.begin().await.expect("same transaction");
    sqlx::query("INSERT INTO guest_link_delivery_starts(verifier,key_id,nonce_counter,key_fingerprint,started_at) SELECT e.verifier,e.key_id,e.nonce_counter,k.key_fingerprint,1100 FROM guest_link_delivery_envelopes e JOIN guest_link_delivery_keys k ON k.key_id=e.key_id WHERE e.verifier=$1")
        .bind(verifier.as_slice()).execute(&mut *same).await.expect("start in transaction");
    assert!(sqlx::query("INSERT INTO guest_link_delivery_outcomes(verifier,start_transaction_id,ciphertext_sha256,outcome,observed_at) SELECT verifier,transaction_id,(SELECT sha256(ciphertext) FROM guest_link_delivery_envelopes WHERE verifier=$1),'accepted',1100 FROM guest_link_delivery_starts WHERE verifier=$1")
        .bind(verifier.as_slice()).execute(&mut *same).await.is_err());
    same.rollback().await.expect("rollback same transaction");
    delivery::claim_and_open(&pool, &verifier, clock.as_ref(), &key, |_, _| {})
        .await
        .expect("start");
    assert!(sqlx::query("INSERT INTO guest_link_delivery_outcomes(verifier,start_transaction_id,ciphertext_sha256,outcome,observed_at) SELECT $1,pg_current_xact_id(),sha256(ciphertext),'accepted',1100 FROM guest_link_delivery_envelopes WHERE verifier=$1")
        .bind(verifier.as_slice()).execute(&pool).await.is_err());
    assert!(sqlx::query("INSERT INTO guest_link_delivery_outcomes(verifier,start_transaction_id,ciphertext_sha256,outcome,observed_at) SELECT verifier,transaction_id,(SELECT sha256(ciphertext) FROM guest_link_delivery_envelopes WHERE verifier=$1),'provider-body',1100 FROM guest_link_delivery_starts WHERE verifier=$1")
        .bind(verifier.as_slice()).execute(&pool).await.is_err());
    assert!(sqlx::query("INSERT INTO guest_link_delivery_outcomes(verifier,start_transaction_id,ciphertext_sha256,outcome,observed_at) SELECT verifier,transaction_id,(SELECT sha256(ciphertext) FROM guest_link_delivery_envelopes WHERE verifier=$1),'accepted',1099 FROM guest_link_delivery_starts WHERE verifier=$1")
        .bind(verifier.as_slice()).execute(&pool).await.is_err());
    assert!(sqlx::query("INSERT INTO guest_link_delivery_outcomes(verifier,start_transaction_id,ciphertext_sha256,outcome,observed_at) SELECT verifier,transaction_id,decode(repeat('00',32),'hex'),'accepted',1100 FROM guest_link_delivery_starts WHERE verifier=$1")
        .bind(verifier.as_slice()).execute(&pool).await.is_err());
    let mut nondurable = pool.begin().await.expect("transaction");
    sqlx::query("SET LOCAL synchronous_commit='off'")
        .execute(&mut *nondurable)
        .await
        .expect("weaken durability");
    assert!(sqlx::query("INSERT INTO guest_link_delivery_outcomes(verifier,start_transaction_id,ciphertext_sha256,outcome,observed_at) SELECT verifier,transaction_id,(SELECT sha256(ciphertext) FROM guest_link_delivery_envelopes WHERE verifier=$1),'accepted',1100 FROM guest_link_delivery_starts WHERE verifier=$1")
        .bind(verifier.as_slice()).execute(&mut *nondurable).await.is_err());
    nondurable.rollback().await.expect("rollback nondurable");
    let mut weakened = pool.begin().await.expect("transaction");
    sqlx::query("INSERT INTO guest_link_delivery_outcomes(verifier,start_transaction_id,ciphertext_sha256,outcome,observed_at) SELECT verifier,transaction_id,(SELECT sha256(ciphertext) FROM guest_link_delivery_envelopes WHERE verifier=$1),'accepted',1100 FROM guest_link_delivery_starts WHERE verifier=$1")
        .bind(verifier.as_slice()).execute(&mut *weakened).await.expect("insert");
    sqlx::query("SET LOCAL synchronous_commit='off'")
        .execute(&mut *weakened)
        .await
        .expect("weaken durability");
    assert!(weakened.commit().await.is_err());
    assert_eq!(
        outcome::inspect(&pool, &verifier).await,
        Ok(DeliveryState::Unknown)
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn guest_link_outcome_inspection_rejects_damaged_binding_and_history(pool: PgPool) {
    let (clock, key, verifier, _) = sample(&pool, "damage").await;
    delivery::claim_open_and_record(&pool, &verifier, clock.as_ref(), &key, |_, _| {
        TransportOutcome::Accepted
    })
    .await
    .expect("record");
    let original: String = sqlx::query_scalar(
        "SELECT start_transaction_id::text FROM guest_link_delivery_outcomes WHERE verifier=$1",
    )
    .bind(verifier.as_slice())
    .fetch_one(&pool)
    .await
    .expect("binding");
    pool.execute("ALTER TABLE guest_link_delivery_outcomes DISABLE TRIGGER USER")
        .await
        .expect("fault setup");
    sqlx::query("UPDATE guest_link_delivery_outcomes SET start_transaction_id=pg_current_xact_id() WHERE verifier=$1").bind(verifier.as_slice()).execute(&pool).await.expect("damage");
    assert_eq!(
        outcome::inspect(&pool, &verifier).await,
        Err(LinkError::Unavailable)
    );
    sqlx::query("UPDATE guest_link_delivery_outcomes SET start_transaction_id=$2::text::xid8 WHERE verifier=$1").bind(verifier.as_slice()).bind(&original).execute(&pool).await.expect("repair");
    pool.execute("ALTER TABLE guest_link_delivery_outcomes ENABLE TRIGGER USER")
        .await
        .expect("restore guards");
    let envelope_xid: String = sqlx::query_scalar(
        "SELECT transaction_id::text FROM guest_link_delivery_envelopes WHERE verifier=$1",
    )
    .bind(verifier.as_slice())
    .fetch_one(&pool)
    .await
    .expect("envelope");
    pool.execute("ALTER TABLE guest_link_delivery_envelopes DISABLE TRIGGER USER")
        .await
        .expect("fault setup");
    sqlx::query("UPDATE guest_link_delivery_envelopes SET transaction_id=pg_current_xact_id() WHERE verifier=$1").bind(verifier.as_slice()).execute(&pool).await.expect("damage history");
    assert_eq!(
        outcome::inspect(&pool, &verifier).await,
        Err(LinkError::Unavailable)
    );
    sqlx::query(
        "UPDATE guest_link_delivery_envelopes SET transaction_id=$2::text::xid8 WHERE verifier=$1",
    )
    .bind(verifier.as_slice())
    .bind(&envelope_xid)
    .execute(&pool)
    .await
    .expect("repair history");
    sqlx::query("UPDATE guest_link_delivery_envelopes SET ciphertext=set_byte(ciphertext,0,get_byte(ciphertext,0) # 1) WHERE verifier=$1").bind(verifier.as_slice()).execute(&pool).await.expect("same-length fault");
    assert_eq!(
        outcome::inspect(&pool, &verifier).await,
        Err(LinkError::Unavailable)
    );
    sqlx::query("UPDATE guest_link_delivery_envelopes SET ciphertext=set_byte(ciphertext,0,get_byte(ciphertext,0) # 1) WHERE verifier=$1").bind(verifier.as_slice()).execute(&pool).await.expect("restore ciphertext");
    pool.execute("ALTER TABLE guest_link_delivery_envelopes ENABLE TRIGGER USER")
        .await
        .expect("restore guards");
    assert_eq!(
        outcome::inspect(&pool, &verifier).await,
        Ok(DeliveryState::Recorded(TransportOutcome::Accepted))
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn guest_link_outcome_rejects_ciphertext_changed_after_callback_opening(pool: PgPool) {
    let (clock, key, verifier, _) = sample(&pool, "interleave").await;
    let options = (*pool.connect_options()).clone();
    let result = delivery::claim_open_and_record(&pool, &verifier, clock.as_ref(), &key, |_, _| {
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime");
            runtime.block_on(async move {
                let other = sqlx::postgres::PgPoolOptions::new().max_connections(2).connect_with(options).await.expect("separate pool");
                tokio::time::timeout(Duration::from_secs(5), async {
                    other.execute("ALTER TABLE guest_link_delivery_envelopes DISABLE TRIGGER USER").await.expect("fault setup");
                    sqlx::query("UPDATE guest_link_delivery_envelopes SET ciphertext=set_byte(ciphertext,0,get_byte(ciphertext,0) # 1) WHERE verifier=$1").bind(verifier.as_slice()).execute(&other).await.expect("same-length fault");
                    other.execute("ALTER TABLE guest_link_delivery_envelopes ENABLE TRIGGER USER").await.expect("restore guards");
                }).await.expect("callback must not retain locks");
                other.close().await;
            });
        }).join().expect("callback thread");
        TransportOutcome::Accepted
    }).await;
    assert_eq!(result, Err(LinkError::Unavailable));
    assert_eq!(
        outcome::inspect(&pool, &verifier).await,
        Ok(DeliveryState::Unknown)
    );
    pool.execute("ALTER TABLE guest_link_delivery_envelopes DISABLE TRIGGER USER")
        .await
        .expect("fault setup");
    sqlx::query("UPDATE guest_link_delivery_envelopes SET ciphertext=set_byte(ciphertext,0,get_byte(ciphertext,0) # 1) WHERE verifier=$1").bind(verifier.as_slice()).execute(&pool).await.expect("restore ciphertext");
    pool.execute("ALTER TABLE guest_link_delivery_envelopes ENABLE TRIGGER USER")
        .await
        .expect("restore guards");
    assert_eq!(
        delivery::claim_open_and_record(&pool, &verifier, clock.as_ref(), &key, |_, _| panic!(
            "must not reopen"
        ))
        .await,
        Ok(OpenOutcome::AlreadyStarted)
    );
    assert_eq!(
        outcome::inspect(&pool, &verifier).await,
        Ok(DeliveryState::Unknown)
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn guest_link_outcome_rejects_changed_start_after_callback(pool: PgPool) {
    let (clock, key, verifier, _) = sample(&pool, "changed-start").await;
    let options = (*pool.connect_options()).clone();
    let result = delivery::claim_open_and_record(&pool, &verifier, clock.as_ref(), &key, |_, _| {
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime");
            runtime.block_on(async move {
                let other = sqlx::postgres::PgPoolOptions::new().max_connections(2).connect_with(options).await.expect("separate pool");
                tokio::time::timeout(Duration::from_secs(5), async {
                    other.execute("ALTER TABLE guest_link_delivery_starts DISABLE TRIGGER USER").await.expect("fault setup");
                    sqlx::query("UPDATE guest_link_delivery_starts SET started_at=started_at+1 WHERE verifier=$1").bind(verifier.as_slice()).execute(&other).await.expect("alter start");
                    other.execute("ALTER TABLE guest_link_delivery_starts ENABLE TRIGGER USER").await.expect("restore guards");
                }).await.expect("callback must not retain locks");
                other.close().await;
            });
        }).join().expect("callback thread");
        TransportOutcome::Accepted
    }).await;
    assert_eq!(result, Err(LinkError::Unavailable));
    assert_eq!(
        outcome::inspect(&pool, &verifier).await,
        Ok(DeliveryState::Unknown)
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM guest_link_delivery_outcomes")
        .fetch_one(&pool)
        .await
        .expect("no outcome");
    assert_eq!(count, 0);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn guest_link_outcome_rejects_competing_committed_history(pool: PgPool) {
    let (clock, key, verifier, _) = sample(&pool, "competing-outcome").await;
    let options = (*pool.connect_options()).clone();
    let result = delivery::claim_open_and_record(&pool, &verifier, clock.as_ref(), &key, |_, _| {
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime");
            runtime.block_on(async move {
                let other = sqlx::postgres::PgPoolOptions::new().max_connections(2).connect_with(options).await.expect("separate pool");
                tokio::time::timeout(Duration::from_secs(5),
                    sqlx::query("INSERT INTO guest_link_delivery_outcomes(verifier,start_transaction_id,ciphertext_sha256,outcome,observed_at) SELECT s.verifier,s.transaction_id,sha256(e.ciphertext),'uncertain',1100 FROM guest_link_delivery_starts s JOIN guest_link_delivery_envelopes e USING(verifier) WHERE s.verifier=$1")
                        .bind(verifier.as_slice()).execute(&other)
                ).await.expect("callback must not retain locks").expect("committed concurrent outcome");
                other.close().await;
            });
        }).join().expect("callback thread");
        TransportOutcome::Accepted
    }).await;
    assert_eq!(result, Err(LinkError::Unavailable));
    assert_eq!(
        outcome::inspect(&pool, &verifier).await,
        Ok(DeliveryState::Recorded(TransportOutcome::Uncertain))
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM guest_link_delivery_outcomes")
        .fetch_one(&pool)
        .await
        .expect("one outcome");
    assert_eq!(count, 1);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn guest_link_outcome_rejects_altered_insert_readback(pool: PgPool) {
    let (clock, key, verifier, _) = sample(&pool, "altered-outcome").await;
    pool.execute("CREATE SEQUENCE guest_outcome_insert_seen START 1; CREATE FUNCTION observe_guest_link_outcome_insert() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM nextval('guest_outcome_insert_seen'); RETURN NEW; END $$; CREATE TRIGGER z_observe_guest_link_outcome AFTER INSERT ON guest_link_delivery_outcomes FOR EACH ROW EXECUTE FUNCTION observe_guest_link_outcome_insert()")
        .await.expect("nontransactional insert probe");
    pool.execute("CREATE FUNCTION alter_guest_link_outcome() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN NEW.observed_at=NEW.observed_at+1; RETURN NEW; END $$; CREATE TRIGGER z_alter_guest_link_outcome BEFORE INSERT ON guest_link_delivery_outcomes FOR EACH ROW EXECUTE FUNCTION alter_guest_link_outcome()")
        .await.expect("synthetic readback fault after native guard");
    assert_eq!(
        delivery::claim_open_and_record(&pool, &verifier, clock.as_ref(), &key, |_, _| {
            TransportOutcome::Accepted
        })
        .await,
        Err(LinkError::Unavailable)
    );
    let seen: (i64, bool) =
        sqlx::query_as("SELECT last_value,is_called FROM guest_outcome_insert_seen")
            .fetch_one(&pool)
            .await
            .expect("insert observed before rollback");
    assert_eq!(seen, (1, true));
    assert_eq!(
        outcome::inspect(&pool, &verifier).await,
        Ok(DeliveryState::Unknown)
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM guest_link_delivery_outcomes")
        .fetch_one(&pool)
        .await
        .expect("rollback");
    assert_eq!(count, 0);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn guest_link_outcome_after_redemption_and_expiry_stays_historical(pool: PgPool) {
    use admitted_proofs::{
        customer_passwords, email_verification, verified_customer_accounts::establish,
    };
    use commerce_infrastructure::guest_order_links::GuestOrderLinkToken;
    let (clock, key, verifier, order) = sample(&pool, "redeem").await;
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
    let session = *customer.token.expose_bytes();
    let options = (*pool.connect_options()).clone();
    let callback_clock = clock.clone();
    assert_eq!(delivery::claim_open_and_record(&pool, &verifier, clock.as_ref(), &key, |_, token| {
        let proof = GuestOrderLinkToken::from_bytes(*token.expose_bytes());
        let clock = callback_clock.clone();
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime");
            runtime.block_on(async move {
                let other = sqlx::postgres::PgPoolOptions::new().max_connections(2).connect_with(options).await.expect("separate pool");
                let session = commerce_infrastructure::customer_sessions::CustomerSessionToken::from_bytes(session);
                tokio::time::timeout(Duration::from_secs(5), guest_order_links::link(&other, &session, &order, &proof, clock.as_ref())).await.expect("callback must not retain locks").expect("link during callback");
                other.close().await;
            });
        }).join().expect("callback thread");
        callback_clock.0.store(2200, Ordering::SeqCst);
        TransportOutcome::Accepted
    }).await, Ok(OpenOutcome::Opened));
    assert_eq!(
        outcome::inspect(&pool, &verifier).await,
        Ok(DeliveryState::Recorded(TransportOutcome::Accepted))
    );
}
