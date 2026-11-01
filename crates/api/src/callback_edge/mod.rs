//! Socket-peer-gated classic callback edge; no database or provider credentials.
mod config;
use crate::{callbacks::form, error};
use axum::{
    Router,
    body::Bytes,
    extract::{ConnectInfo, DefaultBodyLimit, FromRequest, Request, State},
    http::{HeaderValue, StatusCode},
    middleware,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use commerce_infrastructure::callback_forwarder::{CALLBACK_PATH, CallbackForwarder};
pub use config::Config;
use std::{
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::Duration,
};
use tokio::sync::Semaphore;
struct EdgeState {
    peers: Vec<IpAddr>,
    forwarder: CallbackForwarder,
    capacity: Semaphore,
    revision: Option<HeaderValue>,
}
pub fn router(config: Config) -> Router {
    let state = Arc::new(EdgeState {
        peers: config.peers,
        forwarder: config.forwarder,
        capacity: Semaphore::new(16),
        revision: config.revision_header,
    });
    Router::new()
        .route(CALLBACK_PATH, post(callback))
        .route("/health/live", get(|| async { StatusCode::NO_CONTENT }))
        .layer(DefaultBodyLimit::max(form::MAX_BODY))
        .layer(middleware::map_response_with_state(state.clone(), harden))
        .with_state(state)
}
async fn callback(State(state): State<Arc<EdgeState>>, request: Request) -> Response {
    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|v| v.0.ip());
    if !peer.is_some_and(|ip| state.peers.contains(&ip)) {
        return error(StatusCode::FORBIDDEN, "callback_peer_denied");
    }
    let Ok(_permit) = state.capacity.try_acquire() else {
        return unavailable();
    };
    match tokio::time::timeout(Duration::from_secs(3), forward(&state, request)).await {
        Ok(response) => response,
        Err(_) => unavailable(),
    }
}
async fn forward(state: &EdgeState, request: Request) -> Response {
    let headers = request.headers().clone();
    let bytes =
        match tokio::time::timeout(Duration::from_secs(1), Bytes::from_request(request, &())).await
        {
            Ok(Ok(bytes)) => bytes,
            Ok(Err(rejection)) => {
                return if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE {
                    error(StatusCode::PAYLOAD_TOO_LARGE, "callback_body_too_large")
                } else {
                    error(StatusCode::BAD_REQUEST, "invalid_callback_body")
                };
            }
            Err(_) => return unavailable(),
        };
    if let Err(rejection) = form::parse(&headers, &bytes) {
        return match rejection {
            form::FormError::InvalidBody => error(StatusCode::BAD_REQUEST, "invalid_callback_body"),
            form::FormError::UnsupportedMediaType => error(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "unsupported_callback_media_type",
            ),
            form::FormError::TooLarge => {
                error(StatusCode::PAYLOAD_TOO_LARGE, "callback_body_too_large")
            }
        };
    }
    match state.forwarder.forward(&bytes).await {
        Ok(()) => StatusCode::OK.into_response(),
        Err(_) => unavailable(),
    }
}
fn unavailable() -> Response {
    error(StatusCode::SERVICE_UNAVAILABLE, "callback_edge_unavailable")
}
async fn harden(State(state): State<Arc<EdgeState>>, mut response: Response) -> Response {
    for (key, value) in [
        ("cache-control", "no-store"),
        ("x-robots-tag", "noindex, nofollow"),
        ("x-content-type-options", "nosniff"),
        ("referrer-policy", "no-referrer"),
    ] {
        response
            .headers_mut()
            .insert(key, HeaderValue::from_static(value));
    }
    if let Some(revision) = &state.revision {
        response
            .headers_mut()
            .insert("x-piwas-preview-revision", revision.clone());
    }
    response
}
