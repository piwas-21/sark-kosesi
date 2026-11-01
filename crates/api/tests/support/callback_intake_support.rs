use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
    response::Response,
};
use commerce_api::{AppState, callbacks::CallbackProfile, guest::GuestOrigin, router_with_options};
use commerce_application::request_context::{Clock, ContextError, Identifiers};
use sqlx::PgPool;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use tokio::sync::Semaphore;

pub const TOKEN: &str = "fixture_callback_preview_12345678901234567890";
pub const PROFILE: &str = "pfl_CALLBACK_API";
pub const PATH: &str = "/api/callbacks/mollie";
pub const ORIGIN: &str = "https://callback-guest.example.test";
pub struct Time {
    pub at: AtomicU64,
    pub fail: AtomicBool,
}
impl Clock for Time {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        if self.fail.load(Ordering::SeqCst) {
            Err(ContextError::ClockUnavailable)
        } else {
            Ok(self.at.load(Ordering::SeqCst))
        }
    }
}
pub struct Ids {
    next: AtomicU64,
    pub fail_at: AtomicU64,
}
impl Identifiers for Ids {
    fn next(&self) -> Result<[u8; 16], ContextError> {
        let next = self.next.fetch_add(1, Ordering::SeqCst) + 1;
        if self.fail_at.load(Ordering::SeqCst) == next {
            Err(ContextError::EntropyUnavailable)
        } else {
            Ok(u128::from(next).to_be_bytes())
        }
    }
}
pub fn state(pool: &PgPool) -> (AppState, Arc<Time>, Arc<Ids>) {
    let clock = Arc::new(Time {
        at: AtomicU64::new(1500),
        fail: AtomicBool::new(false),
    });
    let ids = Arc::new(Ids {
        next: AtomicU64::new(0),
        fail_at: AtomicU64::new(0),
    });
    (
        AppState {
            pool: pool.clone(),
            preview_token: Arc::from(TOKEN),
            capacity: Arc::new(Semaphore::new(32)),
            clock: clock.clone(),
            identifiers: ids.clone(),
        },
        clock,
        ids,
    )
}
pub fn configured(state: AppState, profile: &str, guest: bool) -> Router {
    router_with_options(
        state,
        guest.then(|| GuestOrigin::parse(ORIGIN).expect("fixture origin")),
        Some(CallbackProfile::parse(profile).expect("fixture profile")),
    )
    .expect("private callback router")
}
pub fn private_app(pool: &PgPool) -> (Router, Arc<Time>, Arc<Ids>) {
    let (state, clock, ids) = state(pool);
    (configured(state, PROFILE, false), clock, ids)
}
pub fn request(body: impl Into<Body>) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(PATH)
        .header("authorization", format!("Bearer {TOKEN}"))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(body.into())
        .expect("callback request")
}
pub async fn bytes(response: Response) -> Vec<u8> {
    to_bytes(response.into_body(), 16_384)
        .await
        .expect("bounded response")
        .to_vec()
}
pub async fn failure(response: Response, status: StatusCode) -> String {
    assert_eq!(response.status(), status);
    assert_eq!(response.headers()["cache-control"], "private, no-store");
    assert_eq!(response.headers()["x-robots-tag"], "noindex, nofollow");
    assert_eq!(response.headers()["x-content-type-options"], "nosniff");
    let body = String::from_utf8(bytes(response).await).expect("static UTF8");
    let json: serde_json::Value = serde_json::from_str(&body).expect("static error JSON");
    assert_eq!(json.as_object().expect("error object").len(), 1);
    assert!(json["code"].as_str().is_some_and(|value| !value.is_empty()));
    body
}
pub async fn custody(pool: &PgPool) -> (i64, i64, i64, i64) {
    sqlx::query_as("SELECT (SELECT count(*) FROM callback_intakes),(SELECT count(*) FROM callback_intake_events),(SELECT count(*) FROM callback_inbox_jobs),(SELECT count(*) FROM callback_inbox_active)")
        .fetch_one(pool).await.expect("independent custody counts")
}
pub async fn no_financial_effects(pool: &PgPool) {
    let counts: (i64, i64, i64, i64, i64, i64, i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM checkout_orders),(SELECT count(*) FROM payment_attempts),(SELECT count(*) FROM payment_observations),(SELECT count(*) FROM payment_identity_resolutions),(SELECT count(*) FROM stock_movements),(SELECT count(*) FROM stock_holds),(SELECT count(*) FROM callback_lookup_operations),(SELECT count(*) FROM commerce_outbox)")
        .fetch_one(pool).await.expect("actual unaffected aggregates");
    assert_eq!(counts, (0, 0, 0, 0, 0, 0, 0, 0));
}
pub async fn wait_for_lock(pool: &PgPool, query_prefix: &str) {
    for _ in 0..100 {
        let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND pid<>pg_backend_pid() AND wait_event_type='Lock' AND query LIKE $1)")
            .bind(format!("{query_prefix}%")).fetch_one(pool).await.expect("actual backend wait");
        if waiting {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    panic!("callback database operation never reached its actual lock wait");
}
pub fn findings<'a>(text: &str, secrets: &'a [&str]) -> Vec<&'a str> {
    secrets
        .iter()
        .copied()
        .filter(|value| text.contains(value))
        .collect()
}
