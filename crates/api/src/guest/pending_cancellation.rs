//! Test-only owner request custody after a durable dispatch start.
use super::{
    cancellation::{failed, mutation_input},
    *,
};
use axum::extract::{Path, State};
use commerce_domain::media::valid_command_id;
use commerce_infrastructure::checkout_cancellations;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

const SUFFIX: &str = "pending-cancellation";

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PendingCancellationRequest {
    /// Stable identifier for exact retries of this pending request.
    #[schema(min_length = 1, max_length = 128, pattern = "^[a-zA-Z0-9-]+$")]
    pub effect_key: String,
    /// Must be true to confirm recording the request.
    pub confirmed: bool,
}

#[derive(Serialize, ToSchema)]
pub struct PendingCancellationRecord {
    pub requested_at: i64,
}

#[derive(Serialize, ToSchema)]
pub struct PendingCancellationReceipt {
    pub requested_at: i64,
    pub replayed: bool,
}

#[derive(Serialize, ToSchema)]
pub struct PendingCancellationLookup {
    #[schema(required = true)]
    pub request: Option<PendingCancellationRecord>,
}

#[utoipa::path(post,operation_id="test_guest_request_pending_cancellation",path="/api/test/guest/orders/{id}/pending-cancellation",params(("id"=String,Path),("Origin"=String,Header),("x-csrf-token"=String,Header)),request_body=PendingCancellationRequest,responses((status=200,body=PendingCancellationReceipt),(status=400,body=ApiError),(status=401,body=ApiError),(status=403,body=ApiError),(status=404,body=ApiError),(status=409,body=ApiError),(status=413,body=ApiError),(status=415,body=ApiError),(status=503,body=ApiError),(status=504,body=ApiError)),security(("preview"=[],"guest"=[])))]
pub(crate) async fn submit(
    State(app): State<crate::AppState>,
    Extension(state): Extension<GuestState>,
    Path(id): Path<String>,
    session: Session,
    request: Request,
) -> Response {
    let (key, csrf, input) = match mutation_input::<PendingCancellationRequest>(
        &app, &state, &session, &id, request, SUFFIX,
    )
    .await
    {
        Ok(value) => value,
        Err(value) => return value.into_response(),
    };
    if !input.confirmed || !valid_command_id(&input.effect_key) {
        return error(StatusCode::BAD_REQUEST, "invalid_cancellation_request");
    }
    match Box::pin(checkout_cancellations::request_after_dispatch(
        &state.store,
        &key,
        &csrf,
        &input.effect_key,
        &id,
    ))
    .await
    {
        Ok(receipt) => Json(PendingCancellationReceipt {
            requested_at: receipt.requested_at,
            replayed: receipt.replayed,
        })
        .into_response(),
        Err(value) => failed(value),
    }
}

#[utoipa::path(get,operation_id="test_guest_pending_cancellation_status",path="/api/test/guest/orders/{id}/pending-cancellation",params(("id"=String,Path),("Origin"=String,Header)),responses((status=200,body=PendingCancellationLookup),(status=400,body=ApiError),(status=401,body=ApiError),(status=403,body=ApiError),(status=404,body=ApiError),(status=503,body=ApiError),(status=504,body=ApiError)),security(("preview"=[],"guest"=[])))]
pub(crate) async fn read(
    Extension(state): Extension<GuestState>,
    Path(id): Path<String>,
    session: Session,
    request: Request,
) -> Response {
    let key = match owned_order_read(
        &state,
        request,
        &session,
        &id,
        SUFFIX,
        "invalid_cancellation_request",
    )
    .await
    {
        Ok(value) => value,
        Err(value) => return value.into_response(),
    };
    match Box::pin(checkout_cancellations::owned_pending_cancellation(
        &state.store,
        &key,
        &id,
    ))
    .await
    {
        Ok(status) => Json(PendingCancellationLookup {
            request: status.map(|value| PendingCancellationRecord {
                requested_at: value.requested_at,
            }),
        })
        .into_response(),
        Err(value) => failed(value),
    }
}
