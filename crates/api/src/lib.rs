//! Bounded, authenticated preview catalogue HTTP interface. No sales or public publishing.
#![forbid(unsafe_code)]
pub mod config;
pub mod contract;

use axum::{
    Json, Router,
    extract::{Path, Request, State},
    http::{HeaderValue, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use commerce_application::{
    Locale,
    request_context::{Clock, Identifiers, RequestContext},
};
use commerce_infrastructure::{catalogue, readiness};
use serde::Serialize;
use sqlx::PgPool;
use std::{sync::Arc, time::Duration};
use subtle::ConstantTimeEq;
use tokio::sync::Semaphore;
use tracing::Instrument;
use utoipa::ToSchema;

// The 25-user steady fixture may still drain when its 50-request burst starts.
pub const PREVIEW_REQUEST_CAPACITY: usize = 96;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub preview_token: Arc<str>,
    pub capacity: Arc<Semaphore>,
    pub clock: Arc<dyn Clock>,
    pub identifiers: Arc<dyn Identifiers>,
}

#[derive(Serialize, ToSchema)]
pub struct Product {
    pub sku: String,
    pub slug: String,
    pub title: String,
    pub description: String,
    pub care: String,
    pub minor_units: i64,
    pub currency: String,
    pub fill: String,
    pub fixture: bool,
}
#[derive(Serialize, ToSchema)]
pub struct Catalogue {
    pub items: Vec<Product>,
    pub locale: String,
    pub production_enabled: bool,
}
#[derive(Serialize, ToSchema)]
pub struct ApiError {
    pub code: String,
}

fn error(status: StatusCode, code: &str) -> Response {
    (status, Json(ApiError { code: code.into() })).into_response()
}

async fn protected(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let authenticated = request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .is_some_and(|value| secret_equal(value.as_bytes(), state.preview_token.as_bytes()));
    if !authenticated {
        return error(StatusCode::UNAUTHORIZED, "preview_authentication_required");
    }
    next.run(request).await
}

fn secret_equal(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    bool::from(left.ct_eq(right))
}

async fn bounded(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let context = RequestContext::capture(state.clock.as_ref(), state.identifiers.as_ref());
    let span = match context.as_ref() {
        Ok(context) => tracing::info_span!(
            "request",
            correlation_id = %context.id,
            unix_seconds = context.unix_seconds
        ),
        Err(_) => tracing::Span::none(),
    };
    let permit = state.capacity.try_acquire();
    let mut response = if context.is_err() {
        error(StatusCode::SERVICE_UNAVAILABLE, "context_unavailable")
    } else if permit.is_err() {
        error(StatusCode::SERVICE_UNAVAILABLE, "capacity_exceeded")
    } else {
        tokio::time::timeout(
            Duration::from_secs(1),
            next.run(request).instrument(span.clone()),
        )
        .await
        .unwrap_or_else(|_| error(StatusCode::GATEWAY_TIMEOUT, "dependency_timeout"))
    };
    if let Ok(context) = context {
        let id = HeaderValue::from_str(&context.id.to_string());
        let timestamp = HeaderValue::from_str(&context.unix_seconds.to_string());
        match (id, timestamp) {
            (Ok(id), Ok(timestamp)) => {
                response.headers_mut().insert("x-request-id", id);
                response
                    .headers_mut()
                    .insert("x-request-unix-seconds", timestamp);
            }
            _ => response = error(StatusCode::SERVICE_UNAVAILABLE, "context_unavailable"),
        }
    }
    if response.status().is_server_error() {
        tracing::warn!(parent: &span, code = "request_unavailable", http_status = response.status().as_u16());
    }
    for (key, value) in [
        ("cache-control", "private, no-store"),
        ("x-robots-tag", "noindex, nofollow"),
        ("x-content-type-options", "nosniff"),
    ] {
        response
            .headers_mut()
            .insert(key, HeaderValue::from_static(value));
    }
    response
}

#[utoipa::path(get, path="/api/catalogue/{locale}", params(("locale"=String, Path)), responses((status=200, body=Catalogue), (status=400, body=ApiError), (status=401, body=ApiError), (status=503, body=ApiError), (status=504, body=ApiError)), security(("preview"=[])))]
pub async fn get_catalogue(
    State(state): State<AppState>,
    Path(language): Path<String>,
) -> Response {
    let Some(locale) = Locale::from_public(&language) else {
        return error(StatusCode::BAD_REQUEST, "unsupported_locale");
    };
    match catalogue(&state.pool, locale).await {
        Ok(items) => Json(Catalogue {
            items: items
                .into_iter()
                .map(|item| Product {
                    sku: item.sku,
                    slug: item.slug,
                    title: item.title,
                    description: item.description,
                    care: item.care,
                    minor_units: item.price.minor_units(),
                    currency: "EUR".into(),
                    fill: item.fill,
                    fixture: item.fixture,
                })
                .collect(),
            locale: locale.internal().into(),
            production_enabled: false,
        })
        .into_response(),
        Err(_) => {
            tracing::warn!(code = "catalogue_unavailable");
            error(StatusCode::SERVICE_UNAVAILABLE, "catalogue_unavailable")
        }
    }
}

async fn ready(State(state): State<AppState>) -> Response {
    match readiness(&state.pool).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(_) => error(StatusCode::SERVICE_UNAVAILABLE, "database_unavailable"),
    }
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/catalogue/{locale}", get(get_catalogue))
        .route_layer(middleware::from_fn_with_state(state.clone(), protected))
        .route("/health/live", get(|| async { StatusCode::NO_CONTENT }))
        .route("/health/ready", get(ready))
        .layer(middleware::from_fn_with_state(state.clone(), bounded))
        .with_state(state)
}
