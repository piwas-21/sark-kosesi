use super::*;
use commerce_application::guest_access::GuestLimits;
use commerce_infrastructure::{
    guest_baskets::GuestSessionStore,
    runtime::{SecureIdentifiers, SystemClock},
};
use std::sync::Arc;
use tower_sessions::session::Id;

#[tokio::test]
async fn raw_guest_cookie_must_match_the_selected_session_identity() {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgresql://127.0.0.1/unused")
        .expect("lazy test pool");
    let store = GuestSessionStore::new(
        pool,
        Arc::new(SystemClock),
        Arc::new(SecureIdentifiers),
        GuestLimits::default(),
    )
    .expect("guest store");
    let session = Session::new(Some(Id(1)), Arc::new(store), None);
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        "cookie",
        format!("__Host-piwas-guest={}", Id(2))
            .parse()
            .expect("cookie"),
    );
    assert!(
        exact_guest_cookie(&headers, &session).is_none(),
        "different selected session is denied"
    );
    headers.insert(
        "cookie",
        format!("__Host-piwas-guest={}", Id(1))
            .parse()
            .expect("cookie"),
    );
    assert!(
        exact_guest_cookie(&headers, &session).is_some(),
        "matching session remains accepted"
    );
}
