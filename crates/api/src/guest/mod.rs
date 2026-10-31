//! Opt-in internal guest API. Rust alone owns browser credentials and basket authority.
pub(crate) mod cookies;
mod origin;
pub(crate) mod quotes;
pub mod types;
use crate::{ApiError, error};
use axum::{
    Extension, Json,
    body::to_bytes,
    extract::Request,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use commerce_infrastructure::guest_baskets::{
    self, GuestCredential, GuestError, GuestSessionStore,
};
pub use origin::GuestOrigin;
use origin::single_header;
use serde::de::DeserializeOwned;
use tower_sessions::Session;
use types::*;
#[derive(Clone)]
pub(crate) struct GuestState {
    pub store: GuestSessionStore,
    pub origin: GuestOrigin,
}
pub(super) struct Rejection(pub StatusCode, pub &'static str);
impl IntoResponse for Rejection {
    fn into_response(self) -> Response {
        error(self.0, self.1)
    }
}
fn failure(value: GuestError) -> Response {
    let (status, code) = match value {
        GuestError::InvalidRequest => (StatusCode::BAD_REQUEST, "invalid_guest_request"),
        GuestError::Denied => (StatusCode::UNAUTHORIZED, "guest_session_required"),
        GuestError::Csrf => (StatusCode::FORBIDDEN, "guest_csrf_required"),
        GuestError::Conflict => (StatusCode::CONFLICT, "guest_basket_conflict"),
        GuestError::RateLimited => (StatusCode::TOO_MANY_REQUESTS, "guest_rate_limited"),
        _ => (StatusCode::SERVICE_UNAVAILABLE, "guest_unavailable"),
    };
    let mut response = error(status, code);
    if status == StatusCode::TOO_MANY_REQUESTS {
        response
            .headers_mut()
            .insert("retry-after", axum::http::HeaderValue::from_static("60"));
    }
    response
}
fn mutation(
    state: &GuestState,
    request: &Request,
    csrf_required: bool,
) -> Result<String, Rejection> {
    if !state.origin.accepts(request.headers()) {
        return Err(Rejection(StatusCode::FORBIDDEN, "guest_origin_required"));
    }
    if !csrf_required {
        return Ok(String::new());
    }
    single_header(request.headers(), "x-csrf-token")
        .map(String::from)
        .filter(|value| value.len() == 64)
        .ok_or(Rejection(StatusCode::FORBIDDEN, "guest_csrf_required"))
}
async fn body<T: DeserializeOwned>(request: Request) -> Result<T, Rejection> {
    if single_header(request.headers(), "content-type").and_then(|value| value.split(';').next())
        != Some("application/json")
    {
        return Err(Rejection(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "guest_json_required",
        ));
    }
    let bytes = to_bytes(request.into_body(), 4096)
        .await
        .map_err(|_| Rejection(StatusCode::PAYLOAD_TOO_LARGE, "guest_request_too_large"))?;
    types::decode(&bytes).map_err(|_| Rejection(StatusCode::BAD_REQUEST, "invalid_guest_request"))
}
#[utoipa::path(post, path="/api/guest/session", request_body=GuestIssueRequest, params(("Origin"=String,Header)), responses((status=200,body=GuestBasket),(status=400,body=ApiError),(status=403,body=ApiError),(status=413,body=ApiError),(status=415,body=ApiError),(status=429,body=ApiError),(status=503,body=ApiError)), security(("preview"=[])))]
pub(crate) async fn issue(
    Extension(state): Extension<GuestState>,
    session: Session,
    request: Request,
) -> Response {
    if let Err(response) = mutation(&state, &request, false) {
        return response.into_response();
    }
    if let Err(response) = body::<GuestIssueRequest>(request).await {
        return response.into_response();
    }
    match guest_baskets::ensure_session(&state.store, &session).await {
        Ok(view) => Json(GuestBasket::from(view)).into_response(),
        Err(value) => failure(value),
    }
}
#[utoipa::path(get, path="/api/guest/basket", responses((status=200,body=GuestBasket),(status=401,body=ApiError),(status=503,body=ApiError)), security(("preview"=[],"guest"=[])))]
pub(crate) async fn read(Extension(state): Extension<GuestState>, session: Session) -> Response {
    let key = match GuestCredential::from_session(&session) {
        Ok(key) => key,
        Err(value) => return failure(value),
    };
    match guest_baskets::read_basket(&state.store, &key).await {
        Ok(view) => Json(GuestBasket::from(view)).into_response(),
        Err(value) => failure(value),
    }
}
#[utoipa::path(put, path="/api/guest/basket", request_body=GuestBasketUpdate, params(("Origin"=String,Header),("x-csrf-token"=String,Header)), responses((status=200,body=GuestBasketReceipt),(status=400,body=ApiError),(status=401,body=ApiError),(status=403,body=ApiError),(status=409,body=ApiError),(status=413,body=ApiError),(status=415,body=ApiError),(status=429,body=ApiError),(status=503,body=ApiError)), security(("preview"=[],"guest"=[])))]
pub(crate) async fn write(
    Extension(state): Extension<GuestState>,
    session: Session,
    request: Request,
) -> Response {
    let csrf = match mutation(&state, &request, true) {
        Ok(csrf) => csrf,
        Err(response) => return response.into_response(),
    };
    let key = match GuestCredential::from_session(&session) {
        Ok(key) => key,
        Err(value) => return failure(value),
    };
    let update = match body::<GuestBasketUpdate>(request).await {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    match guest_baskets::write_basket(&state.store, &key, &csrf, &update.into()).await {
        Ok(receipt) => Json(GuestBasketReceipt::from(receipt)).into_response(),
        Err(value) => failure(value),
    }
}
#[utoipa::path(delete, path="/api/guest/session", params(("Origin"=String,Header),("x-csrf-token"=String,Header)), responses((status=204),(status=400,body=ApiError),(status=401,body=ApiError),(status=403,body=ApiError),(status=503,body=ApiError)), security(("preview"=[],"guest"=[])))]
pub(crate) async fn revoke(
    Extension(state): Extension<GuestState>,
    session: Session,
    request: Request,
) -> Response {
    let csrf = match mutation(&state, &request, true) {
        Ok(csrf) => csrf,
        Err(response) => return response.into_response(),
    };
    match to_bytes(request.into_body(), 0).await {
        Ok(_) => {}
        Err(_) => return error(StatusCode::BAD_REQUEST, "invalid_guest_request"),
    }
    match guest_baskets::revoke_session(&state.store, &session, &csrf).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(value) => failure(value),
    }
}
