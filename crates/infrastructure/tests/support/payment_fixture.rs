//! Synthetic, isolated payment setup; never supplier, legal or provider acceptance.
#[allow(dead_code)]
#[path = "checkout_order_fixture.rs"]
pub mod order;
#[allow(dead_code)]
#[path = "checkout_quote_fixture.rs"]
pub mod quote;
use commerce_application::{Locale, payment_requests::*};
use commerce_domain::{checkout_orders::OrderSnapshot, payment_attempts::*};
use commerce_infrastructure::{
    checkout_orders::create_order, checkout_quotes::issue_quote, guest_baskets::*,
};
use sqlx::{Executor, PgPool};
use std::sync::{Arc, atomic::Ordering};
pub fn config() -> Configuration {
    Configuration::capture(ConfigurationSnapshot {
        schema_version: 1,
        profile: ProfileBinding {
            id: "pfl_SYNTHETIC_NotARealProfile".into(),
            credential_fingerprint: "a".repeat(64),
            mode: PaymentMode::Test,
        },
        method: PaymentMethod::Ideal,
        merchant_origin: "https://shop.example.test".into(),
        return_url: "https://shop.example.test/payment-return".into(),
        webhook_url: "https://shop.example.test/api/payment-webhook".into(),
    })
    .expect("synthetic explicit configuration")
}
pub async fn accepted(
    pool: &PgPool,
) -> (
    GuestSessionStore,
    Arc<quote::Time>,
    tower_sessions::Session,
    String,
    OrderSnapshot,
) {
    quote::seed(pool).await;
    pool.execute("UPDATE stock_balances SET on_hand=6")
        .await
        .expect("only exact whole order demand");
    let (store, time) = quote::store(pool, 1100);
    let (session, csrf) = quote::guest(&store).await;
    let key = GuestCredential::from_session(&session).expect("owned credential");
    let q = issue_quote(&store, &key, &csrf, Locale::Nl, &quote::request("quote"))
        .await
        .expect("indicative")
        .snapshot;
    let order = create_order(
        &store,
        &key,
        &csrf,
        "place",
        &order::intent(&q.binding.quote_id),
        &order::seller(),
    )
    .await
    .expect("accepted order")
    .snapshot;
    time.0.store(1500, Ordering::SeqCst);
    (store, time, session, csrf, order)
}
pub fn intent(order: &OrderSnapshot) -> PreparationIntent {
    PreparationIntent {
        order_id: order.identity.id.clone(),
        confirmed: true,
    }
}
pub async fn counts(pool: &PgPool) -> (i64, i64, i64, i64, i64) {
    sqlx::query_as("SELECT (SELECT count(*) FROM payment_attempts),(SELECT count(*) FROM commerce_outbox),(SELECT count(*) FROM basket_hold_groups),(SELECT count(*) FROM stock_holds),(SELECT count(*) FROM stock_movements)").fetch_one(pool).await.expect("effects")
}
pub struct Ids(pub std::sync::atomic::AtomicU64);
impl commerce_application::request_context::Identifiers for Ids {
    fn next(&self) -> Result<[u8; 16], commerce_application::request_context::ContextError> {
        Ok(u128::from(self.0.fetch_add(1, Ordering::SeqCst) + 1).to_be_bytes())
    }
}
pub async fn waiting(pool: &PgPool) {
    tokio::time::timeout(std::time::Duration::from_secs(2),async {
        loop {
            let waiting: bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'INSERT INTO commerce_outbox%')").fetch_one(pool).await.expect("actual write wait");
            if waiting { return; }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }).await.expect("reached actual post-write lock");
}
