//! Opt-in test-only progress for the exact order owned by a live guest.
use super::*;
use axum::extract::Path;
use commerce_domain::payment_attempts::recognition::State;
use commerce_infrastructure::checkout_orders::{
    OrderStoreError, OwnerGrossPaymentProgress, OwnerOrderLifecycle, OwnerOrderProgress,
    owned_order_progress,
};
use serde::Serialize;
use utoipa::ToSchema;

const INVALID: &str = "invalid_order_progress_request";

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
enum OrderLifecycle {
    Accepted,
    CancelledBeforeDispatch,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
enum GrossPaymentProgress {
    NoRecordedDispatch,
    NoRecognition,
    Unconfirmed,
    Received,
    NeedsReview,
}

#[derive(Serialize, ToSchema)]
pub struct GuestOrderProgress {
    order: OrderLifecycle,
    /// Pending intent is independent of the recorded order lifecycle.
    pending_cancellation: bool,
    /// Gross recognition only.
    gross_payment: GrossPaymentProgress,
}

impl From<OwnerOrderProgress> for GuestOrderProgress {
    fn from(progress: OwnerOrderProgress) -> Self {
        Self {
            order: match progress.order {
                OwnerOrderLifecycle::Accepted => OrderLifecycle::Accepted,
                OwnerOrderLifecycle::CancelledBeforeDispatch => {
                    OrderLifecycle::CancelledBeforeDispatch
                }
            },
            pending_cancellation: progress.pending_cancellation,
            gross_payment: match progress.gross_payment {
                OwnerGrossPaymentProgress::NoRecordedDispatch => {
                    GrossPaymentProgress::NoRecordedDispatch
                }
                OwnerGrossPaymentProgress::NoRecognition => GrossPaymentProgress::NoRecognition,
                OwnerGrossPaymentProgress::Recognized(State::Unconfirmed) => {
                    GrossPaymentProgress::Unconfirmed
                }
                OwnerGrossPaymentProgress::Recognized(State::Received) => {
                    GrossPaymentProgress::Received
                }
                OwnerGrossPaymentProgress::Recognized(State::NeedsReview) => {
                    GrossPaymentProgress::NeedsReview
                }
            },
        }
    }
}

#[utoipa::path(get, operation_id="test_guest_order_progress", path="/api/test/guest/orders/{id}/progress", params(("id"=String,Path),("Origin"=String,Header)), responses((status=200,body=GuestOrderProgress),(status=400,body=ApiError),(status=401,body=ApiError),(status=403,body=ApiError),(status=404,body=ApiError),(status=503,body=ApiError),(status=504,body=ApiError)), security(("preview"=[],"guest"=[])))]
pub(crate) async fn read(
    Extension(state): Extension<GuestState>,
    Path(id): Path<String>,
    session: Session,
    request: Request,
) -> Response {
    let key = match owned_order_read(&state, request, &session, &id, "progress", INVALID).await {
        Ok(key) => key,
        Err(value) => return value.into_response(),
    };
    match Box::pin(owned_order_progress(&state.store, &key, &id)).await {
        Ok(progress) => Json(GuestOrderProgress::from(progress)).into_response(),
        Err(value) => failed(value),
    }
}

fn failed(value: OrderStoreError) -> Response {
    if value == OrderStoreError::Denied {
        return error(StatusCode::UNAUTHORIZED, "guest_session_required");
    }
    let (status, code) = match value {
        OrderStoreError::InvalidRequest => (StatusCode::BAD_REQUEST, INVALID),
        OrderStoreError::NotFound => (StatusCode::NOT_FOUND, "order_progress_not_found"),
        _ => (
            StatusCode::SERVICE_UNAVAILABLE,
            "order_progress_unavailable",
        ),
    };
    error(status, code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognized_gross_states_have_only_the_closed_public_codes() {
        for (state, expected) in [
            (State::Unconfirmed, "unconfirmed"),
            (State::Received, "received"),
            (State::NeedsReview, "needs_review"),
        ] {
            let view = GuestOrderProgress::from(OwnerOrderProgress {
                order: OwnerOrderLifecycle::Accepted,
                pending_cancellation: true,
                gross_payment: OwnerGrossPaymentProgress::Recognized(state),
            });
            assert_eq!(
                serde_json::to_value(view).expect("closed projection"),
                serde_json::json!({
                    "order": "accepted", "pending_cancellation": true, "gross_payment": expected,
                })
            );
        }
    }

    #[test]
    fn native_invalid_request_maps_to_a_closed_bad_request() {
        assert_eq!(
            failed(OrderStoreError::InvalidRequest).status(),
            StatusCode::BAD_REQUEST
        );
    }
}
