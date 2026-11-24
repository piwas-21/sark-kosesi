//! Browser-only synthetic dispatch and pending-request checks in a guarded database.
use crate::guarded_pool;
use commerce_application::{
    guest_access::GuestLimits,
    payment_requests::{Configuration, ConfigurationSnapshot, PaymentMethod},
    request_context::Clock,
};
use commerce_domain::{
    order_stock::identifier,
    payment_attempts::{PaymentMode, PreparationIntent, ProfileBinding},
};
use commerce_infrastructure::{
    checkout_cancellations::inspect_post_dispatch_request,
    guest_baskets::{GuestCredential, GuestSessionStore},
    payment_attempts::{DispatchOutcome, prepare_attempt, start_dispatch},
    runtime::{SecureIdentifiers, SystemClock},
    staff_access::SessionToken,
};
use sqlx::Executor;
use std::sync::Arc;
use tower_sessions::{Session, session::Id};

fn order_id() -> String {
    let order = std::env::var("GUEST_BROWSER_ORDER_ID").expect("isolated fixture order");
    assert!(identifier(&order));
    order
}

#[tokio::test]
#[ignore = "manual browser harness records a synthetic start for its owned order"]
async fn seed_browser_pending_cancellation_start() {
    let pool = guarded_pool().await;
    let order = order_id();
    let cookie: Id = std::env::var("GUEST_BROWSER_GUEST_COOKIE")
        .expect("fixture guest cookie")
        .parse()
        .expect("guest session ID");
    let csrf = std::env::var("GUEST_BROWSER_GUEST_CSRF").expect("fixture synchronizer");
    let store = GuestSessionStore::new(
        pool.clone(),
        Arc::new(SystemClock),
        Arc::new(SecureIdentifiers),
        GuestLimits::default(),
    )
    .expect("guarded guest store");
    let session = Session::new(Some(cookie), Arc::new(store.clone()), None);
    let guest = GuestCredential::from_session(&session).expect("owned guest credential");
    let fixture = Configuration::capture(ConfigurationSnapshot {
        schema_version: 1,
        profile: ProfileBinding {
            id: "pfl_SyntheticBrowserPending".into(),
            credential_fingerprint: "a".repeat(64),
            mode: PaymentMode::Test,
        },
        method: PaymentMethod::Ideal,
        merchant_origin: "https://guest-fixture.example.test".into(),
        return_url: "https://guest-fixture.example.test/payment-return".into(),
        webhook_url: "https://guest-fixture.example.test/payment-callback".into(),
    })
    .expect("explicit offline payment fixture configuration");
    let prepared = Box::pin(prepare_attempt(
        &store,
        &guest,
        &csrf,
        "browser-pending-attempt",
        &PreparationIntent {
            order_id: order,
            confirmed: true,
        },
        &fixture,
    ))
    .await
    .expect("prepare exact synthetic attempt");
    let attempt = &prepared.frozen.snapshot().binding.id;
    let start = Box::pin(start_dispatch(
        &pool,
        &SystemClock,
        &SecureIdentifiers,
        attempt,
        &fixture,
    ))
    .await
    .expect("commit exact synthetic dispatch start");
    // The fixture retains only durable custody and never consumes the permit.
    assert!(matches!(start, DispatchOutcome::Start(_)));
    pool.close().await;
}

#[tokio::test]
#[ignore = "manual browser harness verifies the exact pending order and preserved effects"]
async fn verify_browser_pending_cancellation_effects() {
    let pool = guarded_pool().await;
    let order = order_id();
    let requested_at: i64 = std::env::var("GUEST_BROWSER_PENDING_REQUESTED_AT")
        .expect("browser receipt time")
        .parse()
        .expect("integer receipt time");
    assert!(requested_at >= 0);
    let counts: (i64, i64, i64, i64, i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM checkout_post_dispatch_requests WHERE order_id=$1),(SELECT count(*) FROM commerce_outbox WHERE aggregate_id=$1 AND kind='checkout.cancellation_requested.v1'),(SELECT count(*) FROM checkout_order_cancellations WHERE order_id=$1),(SELECT count(*) FROM refund_reservations WHERE order_id=$1),(SELECT count(*) FROM payment_observations p JOIN payment_attempts a ON a.id=p.attempt_id WHERE a.order_id=$1),(SELECT count(*) FROM payment_dispatch_starts d JOIN payment_attempts a ON a.id=d.attempt_id WHERE a.order_id=$1)")
        .bind(&order).fetch_one(&pool).await.expect("scope exact durable effects to the second order");
    assert_eq!(counts, (1, 1, 0, 0, 0, 1));
    let pending_event: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM checkout_post_dispatch_requests r JOIN commerce_outbox e ON e.id=r.event_id WHERE r.order_id=$1 AND r.requested_at=$2 AND checkout_post_dispatch_request_source(r) AND e.aggregate_id=r.order_id AND e.created_at=r.requested_at AND e.payload=checkout_post_dispatch_request_event(r))")
        .bind(&order).bind(requested_at).fetch_one(&pool).await.expect("pending intent and exact browser receipt");
    assert!(pending_event);
    let physical: (i64, i64, i64, i64, i64, i64) = sqlx::query_as("WITH holds AS (SELECT h.id,h.state FROM stock_holds h JOIN checkout_orders o ON o.stock_group_id=h.basket_group_id WHERE o.id=$1) SELECT (SELECT count(*) FROM holds),(SELECT count(*) FROM holds WHERE state<>'active'),(SELECT count(*) FROM stock_movements m JOIN holds h ON h.id=m.hold_id WHERE m.action<>'reserve' OR m.on_hand_delta<>0 OR m.reserved_delta<=0),(SELECT count(*) FROM order_stock_allocations WHERE order_id=$1),(SELECT count(*) FROM stock_movements m JOIN holds h ON h.id=m.hold_id),(SELECT count(*) FROM stock_hold_lines l JOIN holds h ON h.id=l.hold_id)")
        .bind(&order).fetch_one(&pool).await.expect("original reservation and absence of physical effects");
    assert!(physical.0 > 0);
    assert_eq!((physical.1, physical.2, physical.3), (0, 0, 0));
    assert!(physical.4 > 0);
    assert_eq!(
        physical.4, physical.5,
        "original reserve ledger per hold line"
    );
    let now = i64::try_from(SystemClock.unix_seconds().expect("current fixture clock"))
        .expect("SQL clock");
    let reviewer = SessionToken::from_bytes([88; 32]);
    pool.execute("INSERT INTO staff_accounts VALUES('browser-cancellation-reviewer',true); INSERT INTO staff_capabilities VALUES('browser-cancellation-reviewer','cancellation.review')")
        .await.expect("scoped synthetic reviewer grant");
    sqlx::query("INSERT INTO staff_sessions(token_hash,staff_id,created_at,expires_at,mfa_verified_at) VALUES($1,'browser-cancellation-reviewer',$2,$3,$2)")
        .bind(reviewer.digest().as_slice()).bind(now).bind(now + 120)
        .execute(&pool).await.expect("synthetic recent-MFA review session");
    let review = Box::pin(inspect_post_dispatch_request(
        &pool,
        &reviewer,
        &SystemClock,
        &order,
    ))
    .await
    .expect("staff validated exact pending custody")
    .expect("recorded customer request");
    assert_eq!(review.order_id, order);
    assert_eq!(review.requested_at, requested_at);
    pool.close().await;
}
