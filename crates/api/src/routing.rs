use crate::{
    AppState, bounded,
    callbacks::{self, CallbackProfile},
    get_catalogue,
    guest::{self, GuestOrigin, GuestState},
    protected, published, ready,
    test_login::LoginConfig,
    verification::VerificationConfig,
};
use axum::{
    Extension, Router,
    extract::DefaultBodyLimit,
    http::StatusCode,
    middleware,
    routing::{get, post},
};
use commerce_application::guest_access::GuestLimits;
use commerce_infrastructure::guest_baskets::{GuestError, GuestSessionStore};
use tower_sessions::{SessionManagerLayer, cookie::SameSite};
pub fn router(state: AppState) -> Router {
    build(state, None, None, None, None)
}
pub fn router_with_guest(state: AppState, origin: GuestOrigin) -> Result<Router, GuestError> {
    router_with_options(state, Some(origin), None)
}
pub fn router_with_options(
    state: AppState,
    guest: Option<GuestOrigin>,
    callback: Option<CallbackProfile>,
) -> Result<Router, GuestError> {
    router_with_verification(state, guest, callback, None)
}
pub fn router_with_verification(
    state: AppState,
    guest: Option<GuestOrigin>,
    callback: Option<CallbackProfile>,
    verification: Option<VerificationConfig>,
) -> Result<Router, GuestError> {
    router_with_test_login(state, guest, callback, verification, None)
}
pub fn router_with_test_login(
    state: AppState,
    guest: Option<GuestOrigin>,
    callback: Option<CallbackProfile>,
    verification: Option<VerificationConfig>,
    login: Option<LoginConfig>,
) -> Result<Router, GuestError> {
    let guest = guest
        .map(|origin| guest_router(&state, origin))
        .transpose()?;
    Ok(build(state, guest, callback, verification, login))
}
fn guest_router(state: &AppState, origin: GuestOrigin) -> Result<Router<AppState>, GuestError> {
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
    Ok(guest)
}
fn build(
    state: AppState,
    guest: Option<Router<AppState>>,
    callback: Option<CallbackProfile>,
    verification: Option<VerificationConfig>,
    login: Option<LoginConfig>,
) -> Router {
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
    if let Some(profile) = callback {
        app = app.route("/api/callbacks/mollie", callbacks::route(profile));
    }
    if let Some(config) = verification {
        app = app.route(
            "/api/test/verification/redeem",
            post(crate::verification::redeem)
                .layer::<_, std::convert::Infallible>(DefaultBodyLimit::max(4096))
                .layer(Extension(config)),
        );
    }
    if let Some(config) = login {
        if config.linked_orders_transport {
            app = app.route(
                "/api/test/customer/linked-orders",
                get(crate::test_linked_orders::list).layer(Extension(config.clone())),
            );
        }
        if config.restock_mutation_transport {
            app = app.route(
                "/api/test/customer/restock/{locale}/{sku}",
                axum::routing::put(crate::test_restock_mutation::set)
                    .layer::<_, std::convert::Infallible>(DefaultBodyLimit::max(256))
                    .layer(Extension(config.clone())),
            );
        }
        if config.restock_transport {
            app = app.route(
                "/api/test/customer/restock/{locale}",
                get(crate::test_restock::list).layer(Extension(config.clone())),
            );
        }
        if config.favourites_transport {
            app = app.route(
                "/api/test/customer/favourites/{locale}",
                get(crate::test_favourites::list).layer(Extension(config.clone())),
            );
        }
        if config.csrf_transport {
            app = app
                .route(
                    "/api/test/customer/login-with-csrf",
                    post(crate::test_session::login)
                        .layer::<_, std::convert::Infallible>(DefaultBodyLimit::max(4096))
                        .layer(Extension(config.clone())),
                )
                .route(
                    "/api/test/customer/session",
                    axum::routing::delete(crate::test_session::logout)
                        .layer(Extension(config.clone())),
                );
        }
        app = app.route(
            "/api/test/customer/login",
            post(crate::test_login::login)
                .layer::<_, std::convert::Infallible>(DefaultBodyLimit::max(4096))
                .layer(Extension(config)),
        );
    }
    app.route_layer(middleware::from_fn_with_state(state.clone(), protected))
        .route("/health/live", get(|| async { StatusCode::NO_CONTENT }))
        .route("/health/ready", get(ready))
        .layer(middleware::from_fn_with_state(state.clone(), bounded))
        .with_state(state)
}
