//! Synthetic, isolated payment setup; never supplier, legal or provider acceptance.
#[allow(dead_code)]
#[path = "checkout_order_fixture.rs"]
pub mod order;
#[allow(dead_code)]
#[path = "promotion_policy_fixture.rs"]
pub mod promotion;
#[allow(dead_code)]
#[path = "checkout_quote_fixture.rs"]
pub mod quote;
use commerce_application::{Locale, payment_requests::*};
use commerce_domain::{checkout_orders::OrderSnapshot, payment_attempts::*};
use commerce_infrastructure::{
    checkout_orders::create_order, checkout_quotes::issue_quote, guest_baskets::*,
};
use serde_json::json;
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
    accepted_with_windows(pool, 1100, 2000, None).await
}
pub async fn accepted_with_windows(
    pool: &PgPool,
    created_at: u64,
    seller_expires_at: i64,
    market_expires_at: Option<i64>,
) -> (
    GuestSessionStore,
    Arc<quote::Time>,
    tower_sessions::Session,
    String,
    OrderSnapshot,
) {
    quote::seed(pool).await;
    if let Some(expires_at) = market_expires_at {
        let mut p = quote::policy();
        p.revision = 2;
        p.expires_at = expires_at;
        quote::draft(pool, &p).await;
        quote::decide(pool, 2, 0, true, "short-market").await;
    }
    pool.execute("UPDATE stock_balances SET on_hand=6")
        .await
        .expect("only exact whole order demand");
    let (store, time) = quote::store(pool, created_at);
    let (session, csrf) = quote::guest(&store).await;
    let key = GuestCredential::from_session(&session).expect("owned credential");
    let q = issue_quote(&store, &key, &csrf, Locale::Nl, &quote::request("quote"))
        .await
        .expect("indicative")
        .snapshot;
    let mut seller = order::seller();
    seller.expires_at = seller_expires_at;
    let order = create_order(
        &store,
        &key,
        &csrf,
        "place",
        &order::intent(&q.binding.quote_id),
        &seller,
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

/// Append one synthetic test observation and its native reciprocal event.
#[allow(dead_code)]
pub async fn append_observation(
    pool: &PgPool,
    attempt: &AttemptSnapshot,
    revision: i64,
    provider: &str,
    status: &str,
) {
    let binding = &attempt.binding;
    let observation_id = format!("{:032x}", 50_000 + revision);
    let event_id = format!("{:032x}", 51_000 + revision);
    let effect = format!("observation-{revision}");
    let at = 1500_i64;
    let observation = json!({
        "schema_version": 1, "id": observation_id, "attempt_id": binding.id,
        "effect_key": effect, "revision": revision, "recorded_at": at,
        "intent": {
            "credential_fingerprint": binding.profile.credential_fingerprint,
            "payment": {
                "resource": "payment", "id": provider, "profile_id": binding.profile.id,
                "mode": "test", "currency": "EUR", "amount_minor": binding.amount_minor,
                "metadata_schema_version": 1, "order_id": binding.order_id,
                "attempt_id": binding.id, "status": status
            }
        }
    });
    let mut tx = pool.begin().await.expect("synthetic native observation");
    sqlx::query("INSERT INTO payment_observations(id,attempt_id,effect_key,revision,recorded_at,provider_id,credential_fingerprint,status,snapshot) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)")
        .bind(&observation_id).bind(&binding.id).bind(&effect).bind(revision).bind(at)
        .bind(provider).bind(&binding.profile.credential_fingerprint).bind(status).bind(observation)
        .execute(&mut *tx).await.expect("bounded synthetic observation");
    sqlx::query("INSERT INTO commerce_outbox(id,aggregate_id,kind,payload,created_at) VALUES($1,$2,'payment.observation_recorded.v1',$3,$4)")
        .bind(event_id).bind(&binding.order_id)
        .bind(json!({"schema_version":1,"order_id":binding.order_id,"attempt_id":binding.id,"observation_id":observation_id}))
        .bind(at).execute(&mut *tx).await.expect("matching observation event");
    tx.commit().await.expect("complete synthetic observation");
}
