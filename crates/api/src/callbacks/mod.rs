//! Opt-in private classic form intake. No provider key, fetch or payment authority.
pub(crate) mod form;
use crate::{ApiError, AppState, error};
use axum::{
    Extension,
    body::Bytes,
    extract::{DefaultBodyLimit, FromRequest, Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{MethodRouter, post},
};
use commerce_infrastructure::payment_attempts::callbacks::inbox::admit_callback;
use serde::Deserialize;
use std::sync::Arc;
use utoipa::ToSchema;

/// Trusted server configuration, never taken from an incoming notification.
#[derive(Clone)]
pub struct CallbackProfile(Arc<str>);
impl CallbackProfile {
    pub fn parse(value: &str) -> Result<Self, &'static str> {
        if !commerce_domain::payment_attempts::callback_inbox::valid_profile_id(value) {
            return Err("Invalid CALLBACK_INTAKE_PROFILE_ID");
        }
        Ok(Self(Arc::from(value)))
    }
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CallbackForm {
    /// Opaque payment identifier, at most 256 UTF-8 bytes.
    #[schema(min_length = 4, max_length = 256, pattern = "^tr_.+$")]
    pub id: String,
}
pub(crate) fn route(profile: CallbackProfile) -> MethodRouter<AppState> {
    post(intake)
        .layer::<_, std::convert::Infallible>(DefaultBodyLimit::max(form::MAX_BODY))
        .layer(Extension(profile))
}
#[utoipa::path(post,path="/api/callbacks/mollie",request_body(content=CallbackForm,content_type="application/x-www-form-urlencoded"),responses((status=200,description="Intake committed; no payment fact confirmed"),(status=400,body=ApiError),(status=401,body=ApiError),(status=413,body=ApiError),(status=415,body=ApiError),(status=503,body=ApiError),(status=504,body=ApiError)),security(("preview"=[])))]
pub(crate) async fn intake(
    State(state): State<AppState>,
    Extension(profile): Extension<CallbackProfile>,
    request: Request,
) -> Response {
    let headers = request.headers().clone();
    let bytes = match Bytes::from_request(request, &state).await {
        Ok(bytes) => bytes,
        Err(rejection) => {
            return if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE {
                error(StatusCode::PAYLOAD_TOO_LARGE, "callback_body_too_large")
            } else {
                error(StatusCode::BAD_REQUEST, "invalid_callback_body")
            };
        }
    };
    let provider_id = match form::parse(&headers, &bytes) {
        Ok(id) => id,
        Err(form::FormError::InvalidBody) => {
            return error(StatusCode::BAD_REQUEST, "invalid_callback_body");
        }
        Err(form::FormError::UnsupportedMediaType) => {
            return error(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "unsupported_callback_media_type",
            );
        }
        Err(form::FormError::TooLarge) => {
            return error(StatusCode::PAYLOAD_TOO_LARGE, "callback_body_too_large");
        }
    };
    match admit_callback(
        &state.pool,
        state.clock.as_ref(),
        state.identifiers.as_ref(),
        &profile.0,
        &provider_id,
    )
    .await
    {
        Ok(_) => StatusCode::OK.into_response(),
        Err(_) => error(
            StatusCode::SERVICE_UNAVAILABLE,
            "callback_intake_unavailable",
        ),
    }
}
