//! Real isolated database checks for private test-mode attempt custody; no provider I/O.
#[allow(dead_code)]
#[path = "support/payment_fixture.rs"]
mod fixture;
use commerce_domain::guest_baskets::BasketUpdate;
use commerce_infrastructure::{
    MIGRATOR, checkout_orders::OrderStoreError, guest_baskets::*, payment_attempts::*,
};
use sqlx::{Executor, PgPool};
use std::sync::atomic::Ordering;

#[sqlx::test(migrator = "MIGRATOR")]
async fn accepted_order_uses_its_own_scarce_hold_after_quote_expiry_and_cart_changes(pool: PgPool) {
    let (store, time, session, csrf, order) = fixture::accepted(&pool).await;
    let key = GuestCredential::from_session(&session).expect("owner");
    write_basket(
        &store,
        &key,
        &csrf,
        &BasketUpdate {
            expected_revision: 1,
            effect_key: "new-cart".into(),
            lines: vec![],
        },
    )
    .await
    .expect("later empty cart");
    let config = fixture::config();
    let intent = fixture::intent(&order);
    let receipt = prepare_attempt(&store, &key, &csrf, "pay", &intent, &config)
        .await
        .expect("accepted own reservation");
    assert!(!receipt.replayed);
    let snapshot = receipt.frozen.snapshot();
    assert_eq!(
        (
            snapshot.binding.amount_minor,
            snapshot.binding.created_at,
            snapshot.binding.dispatch_before
        ),
        (38245, 1500, 2000)
    );
    assert_eq!(snapshot.binding.order_id, order.identity.id);
    assert_eq!(
        snapshot.binding.stock_group_id,
        order.identity.stock_group_id
    );
    let body: serde_json::Value = serde_json::from_str(receipt.frozen.body()).expect("JSON");
    assert_eq!(
        body["amount"],
        serde_json::json!({"currency":"EUR","value":"382.45"})
    );
    assert_eq!(body["metadata"]["order_id"], order.identity.id);
    assert_eq!(fixture::counts(&pool).await, (1, 2, 1, 1, 2));
    assert!(
        read_basket(&store, &key)
            .await
            .expect("cart")
            .lines
            .is_empty()
    );
    let balances: Vec<(i64, i64)> =
        sqlx::query_as("SELECT on_hand,reserved FROM stock_balances ORDER BY component_code")
            .fetch_all(&pool)
            .await
            .expect("physical stock");
    assert_eq!(balances, vec![(6, 6), (6, 6)]);
    time.0.store(2500, Ordering::SeqCst);
    let mut changed = config.snapshot().clone();
    changed.profile.credential_fingerprint = "b".repeat(64);
    let changed = commerce_application::payment_requests::Configuration::capture(changed)
        .expect("rotated synthetic configuration");
    let retry = prepare_attempt(&store, &key, &csrf, "pay", &intent, &changed)
        .await
        .expect("historical receipt");
    assert!(retry.replayed && retry.frozen.snapshot() == snapshot);
    assert_eq!(retry.frozen.body(), receipt.frozen.body());
    assert!(
        owned_attempt(&store, &key, &snapshot.binding.id)
            .await
            .expect("owned history")
            .snapshot()
            == snapshot
    );
    assert!(matches!(
        prepare_attempt(&store, &key, &csrf, "alternate", &intent, &changed).await,
        Err(PaymentStoreError::Conflict)
    ));
    assert_eq!(fixture::counts(&pool).await, (1, 2, 1, 1, 2));
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn fresh_owner_csrf_confirmed_intent_and_original_effect_protect_payment_custody(
    pool: PgPool,
) {
    let (store, _, session, csrf, order) = fixture::accepted(&pool).await;
    let key = GuestCredential::from_session(&session).expect("owner");
    let (other, other_csrf) = fixture::quote::guest(&store).await;
    let other_key = GuestCredential::from_session(&other).expect("other");
    let config = fixture::config();
    let intent = fixture::intent(&order);
    assert!(matches!(
        prepare_attempt(&store, &key, &other_csrf, "pay", &intent, &config).await,
        Err(PaymentStoreError::Boundary(OrderStoreError::Csrf))
    ));
    assert!(matches!(
        prepare_attempt(&store, &other_key, &other_csrf, "pay", &intent, &config).await,
        Err(PaymentStoreError::Boundary(OrderStoreError::NotFound))
    ));
    for effect in ["", "bad key"] {
        assert!(matches!(
            prepare_attempt(&store, &key, &csrf, effect, &intent, &config).await,
            Err(PaymentStoreError::InvalidRequest)
        ));
    }
    let mut changed = intent.clone();
    changed.confirmed = false;
    assert!(matches!(
        prepare_attempt(&store, &key, &csrf, "pay", &changed, &config).await,
        Err(PaymentStoreError::InvalidRequest)
    ));
    changed.order_id = "bad".into();
    changed.confirmed = true;
    assert!(matches!(
        prepare_attempt(&store, &key, &csrf, "pay", &changed, &config).await,
        Err(PaymentStoreError::InvalidRequest)
    ));
    assert_eq!(fixture::counts(&pool).await, (0, 1, 1, 1, 2));
    let receipt = prepare_attempt(&store, &key, &csrf, "pay", &intent, &config)
        .await
        .expect("owner");
    let id = &receipt.frozen.snapshot().binding.id;
    assert!(matches!(
        owned_attempt(&store, &other_key, id).await,
        Err(PaymentStoreError::NotFound)
    ));
    assert!(matches!(
        owned_attempt(&store, &key, "bad").await,
        Err(PaymentStoreError::InvalidRequest)
    ));
    assert!(matches!(
        owned_attempt(&store, &key, &"f".repeat(32)).await,
        Err(PaymentStoreError::NotFound)
    ));
    changed.order_id = "f".repeat(32);
    assert!(matches!(
        prepare_attempt(&store, &key, &csrf, "pay", &changed, &config).await,
        Err(PaymentStoreError::Conflict)
    ));
    revoke_session(&store, &session, &csrf)
        .await
        .expect("revoked");
    assert!(matches!(
        owned_attempt(&store, &key, id).await,
        Err(PaymentStoreError::Boundary(OrderStoreError::Denied))
    ));
    assert!(matches!(
        prepare_attempt(&store, &key, &csrf, "pay", &intent, &config).await,
        Err(PaymentStoreError::Boundary(OrderStoreError::Denied))
    ));
    assert_eq!(fixture::counts(&pool).await, (1, 2, 1, 1, 2));
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn simultaneous_exact_retries_create_one_attempt_one_event_and_no_new_stock_effect(
    pool: PgPool,
) {
    let (store, _, session, csrf, order) = fixture::accepted(&pool).await;
    let key = GuestCredential::from_session(&session).expect("owner");
    let config = fixture::config();
    let intent = fixture::intent(&order);
    let (a, b) = tokio::join!(
        prepare_attempt(&store, &key, &csrf, "pay", &intent, &config),
        prepare_attempt(&store, &key, &csrf, "pay", &intent, &config)
    );
    let a = a.expect("A");
    let b = b.expect("B");
    assert_ne!(a.replayed, b.replayed);
    assert!(a.frozen.snapshot() == b.frozen.snapshot());
    assert_eq!(a.frozen.body(), b.frozen.body());
    assert_eq!(fixture::counts(&pool).await, (1, 2, 1, 1, 2));
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn policy_withdrawal_and_new_review_cannot_pay_historical_commercial_terms(pool: PgPool) {
    let (store, _, session, csrf, order) = fixture::accepted(&pool).await;
    let key = GuestCredential::from_session(&session).expect("owner");
    let config = fixture::config();
    let intent = fixture::intent(&order);
    let mut policy = fixture::quote::policy();
    policy.revision = 2;
    fixture::quote::draft(&pool, &policy).await;
    assert!(matches!(
        prepare_attempt(&store, &key, &csrf, "pay", &intent, &config).await,
        Err(PaymentStoreError::Boundary(OrderStoreError::Unavailable))
    ));
    fixture::quote::decide(&pool, 2, 0, true, "new").await;
    assert!(matches!(
        prepare_attempt(&store, &key, &csrf, "pay", &intent, &config).await,
        Err(PaymentStoreError::Unavailable)
    ));
    fixture::quote::decide(&pool, 2, 1, false, "withdraw").await;
    assert!(matches!(
        prepare_attempt(&store, &key, &csrf, "pay", &intent, &config).await,
        Err(PaymentStoreError::Boundary(OrderStoreError::Unavailable))
    ));
    assert_eq!(fixture::counts(&pool).await, (0, 1, 1, 1, 2));
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn physically_unbacked_or_missing_balances_and_expired_holds_block_new_attempts(
    pool: PgPool,
) {
    let (store, time, session, csrf, order) = fixture::accepted(&pool).await;
    let key = GuestCredential::from_session(&session).expect("owner");
    let config = fixture::config();
    let intent = fixture::intent(&order);
    pool.execute("UPDATE stock_balances SET reserved=5 WHERE component_code='SEAT-COVER'")
        .await
        .expect("simulate damaged import");
    assert!(matches!(
        prepare_attempt(&store, &key, &csrf, "pay", &intent, &config).await,
        Err(PaymentStoreError::Boundary(OrderStoreError::Unavailable))
    ));
    pool.execute("DELETE FROM stock_balances WHERE component_code='SEAT-COVER'")
        .await
        .expect("simulate absent balance");
    assert!(matches!(
        prepare_attempt(&store, &key, &csrf, "pay", &intent, &config).await,
        Err(PaymentStoreError::Boundary(OrderStoreError::Unavailable))
    ));
    pool.execute(
        "INSERT INTO stock_balances(component_code,on_hand,reserved) VALUES('SEAT-COVER',6,6)",
    )
    .await
    .expect("restore backing");
    pool.execute("ALTER TABLE stock_hold_lines DISABLE TRIGGER USER;DELETE FROM stock_hold_lines WHERE component_code='SEAT-COVER'").await.expect("simulate incomplete hold component snapshot");
    assert!(matches!(
        prepare_attempt(&store, &key, &csrf, "pay", &intent, &config).await,
        Err(PaymentStoreError::Boundary(OrderStoreError::Unavailable))
    ));
    pool.execute("INSERT INTO stock_hold_lines(hold_id,component_code,quantity) SELECT id,'SEAT-COVER',6 FROM stock_holds").await.expect("restore exact immutable hold components");
    time.0.store(2000, Ordering::SeqCst);
    assert!(matches!(
        prepare_attempt(&store, &key, &csrf, "pay", &intent, &config).await,
        Err(PaymentStoreError::Boundary(OrderStoreError::Unavailable))
    ));
    assert_eq!(fixture::counts(&pool).await, (0, 1, 1, 1, 2));
    time.0.store(1500, Ordering::SeqCst);
    assert!(
        prepare_attempt(&store, &key, &csrf, "pay", &intent, &config)
            .await
            .is_ok()
    );
}
