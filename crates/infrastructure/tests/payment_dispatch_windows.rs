//! Known source authority can expire before stock; delayed permits must retain all deadlines.
#[allow(dead_code)]
#[path = "support/payment_fixture.rs"]
mod fixture;
use commerce_infrastructure::{
    MIGRATOR, checkout_orders::OrderStoreError, guest_baskets::GuestCredential, payment_attempts::*,
};
use sqlx::PgPool;
use std::sync::atomic::{AtomicU64, Ordering};
async fn delayed_permit(pool: PgPool, source: &str) {
    let (created_at, seller_expires_at, market_expires_at, deadline, hold_deadline) = match source {
        "seller" => (1100, 1600, None, 1600, 2000),
        "market" => (1100, 2500, Some(1600), 1600, 2000),
        "photo" => (1300, 3000, None, 2000, 2200),
        _ => panic!("unknown synthetic source"),
    };
    let (store, time, session, csrf, order) =
        fixture::accepted_with_windows(&pool, created_at, seller_expires_at, market_expires_at)
            .await;
    assert_eq!(order.identity.hold_expires_at, hold_deadline);
    let key = GuestCredential::from_session(&session).expect("owner");
    let config = fixture::config();
    let f = prepare_attempt(
        &store,
        &key,
        &csrf,
        "pay",
        &fixture::intent(&order),
        &config,
    )
    .await
    .expect("all sources live at preparation")
    .frozen;
    assert!(f.snapshot().binding.dispatch_before > deadline);
    let id = &f.snapshot().binding.id;
    let ids = fixture::Ids(AtomicU64::new(10000));
    let DispatchOutcome::Start(permit) = start_dispatch(&pool, time.as_ref(), &ids, id, &config)
        .await
        .expect("all sources live at initial start")
    else {
        panic!("initial permit")
    };
    time.0.store(
        u64::try_from(deadline).expect("synthetic positive deadline"),
        Ordering::SeqCst,
    );
    let result = permit.into_request(time.as_ref(), &config);
    if source == "seller" {
        assert!(matches!(result, Err(PaymentStoreError::Unavailable)));
    } else {
        assert!(matches!(
            result,
            Err(PaymentStoreError::Boundary(OrderStoreError::Unavailable))
        ));
    }
    assert!(matches!(
        start_dispatch(&pool, time.as_ref(), &ids, id, &config).await,
        Ok(DispatchOutcome::Reconcile {
            started_at: 1500,
            ..
        })
    ));
    assert_eq!(fixture::counts(&pool).await, (1, 3, 1, 1, 2));
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn delayed_initial_permit_cannot_outlive_seller_policy_before_stock_expires(pool: PgPool) {
    delayed_permit(pool, "seller").await;
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn delayed_initial_permit_cannot_outlive_market_policy_before_stock_expires(pool: PgPool) {
    delayed_permit(pool, "market").await;
}
#[sqlx::test(migrator = "MIGRATOR")]
async fn delayed_initial_permit_cannot_outlive_photo_rights_before_stock_expires(pool: PgPool) {
    delayed_permit(pool, "photo").await;
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn seller_policy_expired_before_preparation_never_creates_an_attempt(pool: PgPool) {
    let (store, _, session, csrf, order) =
        fixture::accepted_with_windows(&pool, 1100, 1500, None).await;
    let key = GuestCredential::from_session(&session).expect("owner");
    assert!(matches!(
        prepare_attempt(
            &store,
            &key,
            &csrf,
            "pay",
            &fixture::intent(&order),
            &fixture::config()
        )
        .await,
        Err(PaymentStoreError::Unavailable)
    ));
    assert_eq!(fixture::counts(&pool).await, (0, 1, 1, 1, 2));
}
