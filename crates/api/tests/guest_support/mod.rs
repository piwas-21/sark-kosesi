use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
    response::Response,
};
use commerce_api::{AppState, guest::GuestOrigin, router_with_guest};
use commerce_application::request_context::{Clock, ContextError, Identifiers};
use sqlx::PgPool;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use tokio::sync::Semaphore;
use tower::ServiceExt;
pub const TOKEN: &str = "fixture_guest_preview_token_12345678901234";
pub const ORIGIN: &str = "https://guest.example.test";
pub struct Time(pub AtomicU64);
impl Clock for Time {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        Ok(self.0.load(Ordering::SeqCst))
    }
}
struct Ids(AtomicU64);
impl Identifiers for Ids {
    fn next(&self) -> Result<[u8; 16], ContextError> {
        Ok(u128::from(self.0.fetch_add(1, Ordering::SeqCst) + 1).to_be_bytes())
    }
}
pub fn state(pool: &PgPool) -> (AppState, Arc<Time>) {
    let clock = Arc::new(Time(AtomicU64::new(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_secs(),
    )));
    (
        AppState {
            pool: pool.clone(),
            preview_token: Arc::from(TOKEN),
            capacity: Arc::new(Semaphore::new(32)),
            clock: clock.clone(),
            identifiers: Arc::new(Ids(AtomicU64::new(0))),
        },
        clock,
    )
}
pub fn app(pool: &PgPool) -> (Router, Arc<Time>) {
    let (state, clock) = state(pool);
    (
        router_with_guest(state, GuestOrigin::parse(ORIGIN).expect("origin")).expect("router"),
        clock,
    )
}
pub fn request(
    method: &str,
    path: &str,
    cookie: Option<&str>,
    csrf: Option<&str>,
    body: &str,
) -> Request<Body> {
    let mut r = Request::builder()
        .method(method)
        .uri(path)
        .header("authorization", format!("Bearer {TOKEN}"))
        .header("origin", ORIGIN)
        .header("content-type", "application/json");
    if let Some(cookie) = cookie {
        r = r.header("cookie", cookie);
    }
    if let Some(csrf) = csrf {
        r = r.header("x-csrf-token", csrf);
    }
    r.body(Body::from(body.to_owned())).expect("request")
}
pub async fn json(response: Response) -> serde_json::Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 16384).await.expect("body"))
        .expect("JSON")
}
pub async fn issue(app: &Router, cookie: Option<&str>) -> (String, String) {
    let response = app
        .clone()
        .oneshot(request("POST", "/api/guest/session", cookie, None, "{}"))
        .await
        .expect("issue");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["cache-control"], "private, no-store");
    let raw = response
        .headers()
        .get("set-cookie")
        .expect("secure cookie")
        .to_str()
        .expect("ASCII");
    for flag in ["Secure", "HttpOnly", "SameSite=Lax", "Path=/"] {
        assert!(raw.contains(flag), "{flag}");
    }
    assert!(!raw.contains("Domain="));
    let cookie = raw.split(';').next().expect("cookie").to_owned();
    assert!(cookie.starts_with("__Host-piwas-guest="));
    assert_eq!(cookie.len(), "__Host-piwas-guest=".len() + 22);
    let value = json(response).await;
    assert_eq!(value["revision"], 0);
    assert_eq!(value["lines"], serde_json::json!([]));
    (cookie, value["csrf_token"].as_str().expect("CSRF").into())
}
pub fn update(key: &str, revision: i32, quantity: u32) -> String {
    format!(
        r#"{{"effect_key":"{key}","expected_revision":{revision},"lines":[{{"sku":"SET-A","quantity":{quantity}}}]}}"#
    )
}
