//! Bounded, closed input shared by test-only login routes.
use crate::{AppState, error, guest::origin::single_header, test_login::LoginConfig};
use axum::{
    body::Bytes,
    extract::{FromRequest, Request},
    http::StatusCode,
    response::Response,
};
use serde::Deserialize;
use utoipa::ToSchema;
use zeroize::Zeroizing;

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct LoginRequest {
    /// Exact mailbox under the private identity normalization policy.
    #[schema(max_length = 254)]
    pub(crate) email: String,
    /// Password input; the private credential policy also bounds UTF-8 bytes.
    #[schema(max_length = 256)]
    pub(crate) password: String,
}

/// Shared bounded login input for the two independently selected test routes.
pub(crate) async fn parse_request(
    state: &AppState,
    config: &LoginConfig,
    request: Request,
) -> Result<LoginRequest, Box<Response>> {
    if !config.origin.accepts(request.headers()) {
        return Err(Box::new(error(
            StatusCode::FORBIDDEN,
            "login_origin_required",
        )));
    }
    if request.uri().query().is_some() {
        return Err(Box::new(error(
            StatusCode::BAD_REQUEST,
            "invalid_login_request",
        )));
    }
    if single_header(request.headers(), "content-type").and_then(|v| v.split(';').next())
        != Some("application/json")
    {
        return Err(Box::new(error(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "login_json_required",
        )));
    }
    let bytes = match Bytes::from_request(request, state).await {
        Ok(bytes) => bytes,
        Err(rejection) if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE => {
            return Err(Box::new(error(
                StatusCode::PAYLOAD_TOO_LARGE,
                "login_request_too_large",
            )));
        }
        Err(_) => {
            return Err(Box::new(error(
                StatusCode::BAD_REQUEST,
                "invalid_login_request",
            )));
        }
    };
    let bytes = Zeroizing::new(Vec::from(bytes));
    if bytes.iter().copied().find(|b| !b.is_ascii_whitespace()) != Some(b'{') {
        return Err(Box::new(error(
            StatusCode::BAD_REQUEST,
            "invalid_login_request",
        )));
    }
    let input: LoginRequest = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(_) => {
            return Err(Box::new(error(
                StatusCode::BAD_REQUEST,
                "invalid_login_request",
            )));
        }
    };
    drop(bytes);
    Ok(input)
}
