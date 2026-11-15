//! Isolated PostgreSQL source-corruption controls for the private opener.
use commerce_application::Locale;
use commerce_infrastructure::{
    MIGRATOR,
    checkout_orders::create_order,
    checkout_quotes::issue_quote,
    guest_baskets::GuestCredential,
    guest_order_links::{
        LinkError,
        delivery::{self, DeliveryKey, OpenOutcome},
    },
};
use sqlx::{Executor, PgPool};
use std::sync::atomic::{AtomicUsize, Ordering};

#[allow(dead_code)]
#[path = "support/checkout_order_fixture.rs"]
mod fixture;
#[allow(dead_code)]
#[path = "support/checkout_quote_fixture.rs"]
mod quote;

#[sqlx::test(migrator = "MIGRATOR")]
async fn opening_validates_persisted_proof_key_nonce_and_transaction_before_callback(pool: PgPool) {
    quote::seed(&pool).await;
    let (store, clock) = quote::store(&pool, 1100);
    let (session, csrf) = quote::guest(&store).await;
    let guest = GuestCredential::from_session(&session).expect("guest");
    let value = issue_quote(
        &store,
        &guest,
        &csrf,
        Locale::Nl,
        &quote::request("open-fault-quote"),
    )
    .await
    .expect("quote")
    .snapshot;
    let order = create_order(
        &store,
        &guest,
        &csrf,
        "open-fault-order",
        &fixture::intent(&value.binding.quote_id),
        &fixture::seller(),
    )
    .await
    .expect("order")
    .snapshot
    .identity
    .id;
    let key = DeliveryKey::capture("open-fault-key", [7; 32]).expect("key");
    delivery::register_key(&pool, clock.as_ref(), &key)
        .await
        .expect("register");
    let verifier = delivery::issue_for_delivery(&store, &guest, &csrf, &order, &key)
        .await
        .expect("proof")
        .verifier;
    let calls = AtomicUsize::new(0);

    pool.execute("ALTER TABLE guest_order_link_proofs DISABLE TRIGGER ALL")
        .await
        .expect("test fault setup");
    sqlx::query("UPDATE guest_order_link_proofs SET order_id='invalid' WHERE verifier=$1")
        .bind(verifier.as_slice())
        .execute(&pool)
        .await
        .expect("invalid proof");
    assert_eq!(
        delivery::claim_and_open(&pool, &verifier, clock.as_ref(), &key, |_, _| {
            calls.fetch_add(1, Ordering::SeqCst);
        })
        .await,
        Err(LinkError::Denied)
    );
    sqlx::query("UPDATE guest_order_link_proofs SET order_id=$2 WHERE verifier=$1")
        .bind(verifier.as_slice())
        .bind(&order)
        .execute(&pool)
        .await
        .expect("restore proof");
    pool.execute("ALTER TABLE guest_order_link_proofs ENABLE TRIGGER ALL")
        .await
        .expect("restore guards");

    pool.execute("ALTER TABLE guest_link_delivery_keys DISABLE TRIGGER USER")
        .await
        .expect("test fault setup");
    sqlx::query("UPDATE guest_link_delivery_keys SET key_fingerprint=set_byte(key_fingerprint,0,get_byte(key_fingerprint,0) # 1) WHERE key_id='open-fault-key'").execute(&pool).await.expect("alter key");
    assert_eq!(
        delivery::claim_and_open(&pool, &verifier, clock.as_ref(), &key, |_, _| {
            calls.fetch_add(1, Ordering::SeqCst);
        })
        .await,
        Err(LinkError::Denied)
    );
    sqlx::query("UPDATE guest_link_delivery_keys SET key_fingerprint=set_byte(key_fingerprint,0,get_byte(key_fingerprint,0) # 1) WHERE key_id='open-fault-key'").execute(&pool).await.expect("restore key");
    pool.execute("ALTER TABLE guest_link_delivery_keys ENABLE TRIGGER USER")
        .await
        .expect("restore guards");

    pool.execute("ALTER TABLE guest_link_delivery_nonces DISABLE TRIGGER USER")
        .await
        .expect("test fault setup");
    sqlx::query("UPDATE guest_link_delivery_nonces SET verifier=set_byte(verifier,0,get_byte(verifier,0) # 1) WHERE key_id='open-fault-key'").execute(&pool).await.expect("alter verifier");
    assert_eq!(
        delivery::claim_and_open(&pool, &verifier, clock.as_ref(), &key, |_, _| {
            calls.fetch_add(1, Ordering::SeqCst);
        })
        .await,
        Err(LinkError::Unavailable)
    );
    sqlx::query("UPDATE guest_link_delivery_nonces SET verifier=set_byte(verifier,0,get_byte(verifier,0) # 1) WHERE key_id='open-fault-key'").execute(&pool).await.expect("restore verifier");
    pool.execute("ALTER TABLE guest_link_delivery_nonces ENABLE TRIGGER USER")
        .await
        .expect("restore guards");

    let xid: String = sqlx::query_scalar(
        "SELECT transaction_id::text FROM guest_link_delivery_envelopes WHERE verifier=$1",
    )
    .bind(verifier.as_slice())
    .fetch_one(&pool)
    .await
    .expect("original xid");
    pool.execute("ALTER TABLE guest_link_delivery_envelopes DISABLE TRIGGER USER")
        .await
        .expect("test fault setup");
    sqlx::query("UPDATE guest_link_delivery_envelopes SET transaction_id=pg_current_xact_id() WHERE verifier=$1").bind(verifier.as_slice()).execute(&pool).await.expect("alter xid");
    assert_eq!(
        delivery::claim_and_open(&pool, &verifier, clock.as_ref(), &key, |_, _| {
            calls.fetch_add(1, Ordering::SeqCst);
        })
        .await,
        Err(LinkError::Unavailable)
    );
    sqlx::query(
        "UPDATE guest_link_delivery_envelopes SET transaction_id=$2::text::xid8 WHERE verifier=$1",
    )
    .bind(verifier.as_slice())
    .bind(xid)
    .execute(&pool)
    .await
    .expect("restore xid");
    pool.execute("ALTER TABLE guest_link_delivery_envelopes ENABLE TRIGGER USER")
        .await
        .expect("restore guards");

    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        delivery::claim_and_open(&pool, &verifier, clock.as_ref(), &key, |_, _| {
            calls.fetch_add(1, Ordering::SeqCst);
        })
        .await,
        Ok(OpenOutcome::Opened)
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}
