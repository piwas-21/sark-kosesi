//! Opt-in test-only pre-dispatch cancellation; no provider or stock effects.
use super::*;
use axum::{
    body::Bytes,
    extract::{FromRequest, Path, State},
};
use commerce_domain::{media::valid_command_id, order_stock::identifier};
use commerce_infrastructure::{checkout_cancellations, checkout_orders::OrderStoreError};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CancellationRequest {
    /// Stable identifier for exact retries of this cancellation.
    #[schema(min_length = 1, max_length = 128, pattern = "^[a-zA-Z0-9-]+$")]
    pub effect_key: String,
    /// Must be true to confirm the pre-dispatch cancellation.
    pub confirmed: bool,
}
#[derive(Serialize, ToSchema)]
pub struct CancellationResponse {
    pub replayed: bool,
}

#[utoipa::path(post,operation_id="test_guest_cancel_order",path="/api/test/guest/orders/{id}/cancellation",params(("id"=String,Path),("Origin"=String,Header),("x-csrf-token"=String,Header)),request_body=CancellationRequest,responses((status=200,body=CancellationResponse),(status=400,body=ApiError),(status=401,body=ApiError),(status=403,body=ApiError),(status=404,body=ApiError),(status=409,body=ApiError),(status=413,body=ApiError),(status=415,body=ApiError),(status=503,body=ApiError),(status=504,body=ApiError)),security(("preview"=[],"guest"=[])))]
pub(crate) async fn cancel(
    State(app): State<crate::AppState>,
    Extension(state): Extension<GuestState>,
    Path(id): Path<String>,
    session: Session,
    request: Request,
) -> Response {
    let csrf = match mutation(&state, &request, true) {
        Ok(value) => value,
        Err(value) => return value.into_response(),
    };
    if request.uri().query().is_some()
        || !identifier(&id)
        || request.uri().path() != format!("/api/test/guest/orders/{id}/cancellation")
    {
        return error(StatusCode::BAD_REQUEST, "invalid_cancellation_request");
    }
    let Some(key) = exact_guest_cookie(request.headers(), &session) else {
        return error(StatusCode::UNAUTHORIZED, "guest_session_required");
    };
    if single_header(request.headers(), "content-type").and_then(|v| v.split(';').next())
        != Some("application/json")
    {
        return error(StatusCode::UNSUPPORTED_MEDIA_TYPE, "guest_json_required");
    }
    let bytes = match Bytes::from_request(request, &app).await {
        Ok(bytes) => bytes,
        Err(value) if value.status() == StatusCode::PAYLOAD_TOO_LARGE => {
            return error(StatusCode::PAYLOAD_TOO_LARGE, "guest_request_too_large");
        }
        Err(_) => return error(StatusCode::BAD_REQUEST, "invalid_cancellation_request"),
    };
    if bytes.iter().copied().find(|b| !b.is_ascii_whitespace()) != Some(b'{') {
        return error(StatusCode::BAD_REQUEST, "invalid_cancellation_request");
    }
    let Ok(input) = serde_json::from_slice::<CancellationRequest>(&bytes) else {
        return error(StatusCode::BAD_REQUEST, "invalid_cancellation_request");
    };
    if !input.confirmed || !valid_command_id(&input.effect_key) {
        return error(StatusCode::BAD_REQUEST, "invalid_cancellation_request");
    }
    match checkout_cancellations::cancel_order(&state.store, &key, &csrf, &input.effect_key, &id)
        .await
    {
        Ok(receipt) => Json(CancellationResponse {
            replayed: receipt.replayed,
        })
        .into_response(),
        Err(value) => failed(value),
    }
}
fn failed(value: OrderStoreError) -> Response {
    let (status, code) = match value {
        OrderStoreError::InvalidRequest => {
            (StatusCode::BAD_REQUEST, "invalid_cancellation_request")
        }
        OrderStoreError::Denied => (StatusCode::UNAUTHORIZED, "guest_session_required"),
        OrderStoreError::Csrf => (StatusCode::FORBIDDEN, "guest_csrf_required"),
        OrderStoreError::NotFound => (StatusCode::NOT_FOUND, "cancellation_order_not_found"),
        OrderStoreError::Conflict => (StatusCode::CONFLICT, "cancellation_conflict"),
        _ => (StatusCode::SERVICE_UNAVAILABLE, "cancellation_unavailable"),
    };
    error(status, code)
}
