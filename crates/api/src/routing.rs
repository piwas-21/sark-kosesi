use crate::{
    AppState, bounded, get_catalogue,
    guest::{self, GuestOrigin, GuestState},
    protected, published, ready,
};
use axum::{
    Extension, Router,
    http::StatusCode,
    middleware,
    routing::{get, post},
};
use commerce_application::guest_access::GuestLimits;
use commerce_infrastructure::guest_baskets::{GuestError, GuestSessionStore};
use tower_sessions::{SessionManagerLayer, cookie::SameSite};
pub fn router(state: AppState) -> Router {
    build(state, None)
}
pub fn router_with_guest(state: AppState, origin: GuestOrigin) -> Result<Router, GuestError> {
    let store = GuestSessionStore::new(
        state.pool.clone(),
        state.clock.clone(),
        state.identifiers.clone(),
        GuestLimits::default(),
    )?;
    let guest = Router::new()
        .route(
            "/api/guest/session",
            post(guest::issue).delete(guest::revoke),
        )
        .route("/api/guest/basket", get(guest::read).put(guest::write))
        .route("/api/guest/{locale}/quotes", post(guest::quotes::issue))
        .route("/api/guest/quotes/{id}", get(guest::quotes::read))
        .layer(Extension(GuestState {
            store: store.clone(),
            origin,
        }))
        .layer(
            SessionManagerLayer::new(store)
                .with_name("__Host-piwas-guest")
                .with_secure(true)
                .with_http_only(true)
                .with_same_site(SameSite::Lax)
                .with_path("/"),
        )
        .layer(middleware::from_fn(guest::cookies::secure));
    Ok(build(state, Some(guest)))
}
fn build(state: AppState, guest: Option<Router<AppState>>) -> Router {
    let mut app = Router::new()
        .route("/api/catalogue/{locale}", get(get_catalogue))
        .route("/api/published/{locale}", get(published::page))
        .route(
            "/api/published/{locale}/products/{sku}",
            get(published::product),
        )
        .route(
            "/api/published/{locale}/images/{sku}/{revision}/{edge}",
            get(published::image),
        );
    if let Some(guest) = guest {
        app = app.merge(guest);
    }
    app.route_layer(middleware::from_fn_with_state(state.clone(), protected))
        .route("/health/live", get(|| async { StatusCode::NO_CONTENT }))
        .route("/health/ready", get(ready))
        .layer(middleware::from_fn_with_state(state.clone(), bounded))
        .with_state(state)
}
