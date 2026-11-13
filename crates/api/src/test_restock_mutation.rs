//! Explicit test-only subscription changes; no public route or notification.
use crate::{
    ApiError, AppState, error, guest::origin::single_header, test_login::LoginConfig,
    test_session::customer_cookie, verification::decode_hex,
};
use axum::{
    Extension, Json,
    body::Bytes,
    extract::{FromRequest, Path, Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use commerce_application::Locale;
use commerce_domain::catalogue_authoring::merchant_code;
use commerce_infrastructure::{
    customer_restock::{self, RestockError},
    customer_sessions::CustomerCsrfToken,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct TestRestockMutation {
    subscribed: bool,
    /// Current bucket revision as a canonical decimal integer.
    #[schema(pattern = "^(0|[1-9][0-9]*)$", max_length = 19)]
    expected_revision: String,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct TestRestockReceipt {
    #[schema(pattern = "^(0|[1-9][0-9]*)$", max_length = 19)]
    revision: String,
}

fn revision(value: &str) -> Option<i64> {
    let bytes = value.as_bytes();
    if bytes.is_empty()
        || bytes.len() > 19
        || (bytes.len() > 1 && bytes[0] == b'0')
        || !bytes.iter().all(u8::is_ascii_digit)
    {
        return None;
    }
    value.parse().ok()
}

#[utoipa::path(put, operation_id="test_customer_restock_set", path="/api/test/customer/restock/{locale}/{sku}", params(("locale"=String,Path),("sku"=String,Path),("Origin"=String,Header),("x-csrf-token"=String,Header)), request_body=TestRestockMutation, responses((status=200,body=TestRestockReceipt,description="Resulting private bucket revision"),(status=400,body=ApiError),(status=401,body=ApiError),(status=403,body=ApiError),(status=409,body=ApiError),(status=413,body=ApiError),(status=415,body=ApiError),(status=503,body=ApiError),(status=504,body=ApiError)), security(("preview"=[],"customer_test"=[])))]
pub(crate) async fn set(
    State(state): State<AppState>,
    Extension(config): Extension<LoginConfig>,
    Path((language, sku)): Path<(String, String)>,
    request: Request,
) -> Response {
    if !config.origin.accepts(request.headers()) {
        return error(StatusCode::FORBIDDEN, "restock_origin_required");
    }
    if request.uri().query().is_some() || !merchant_code(&sku) {
        return error(StatusCode::BAD_REQUEST, "invalid_restock_request");
    }
    let Some(locale) = Locale::from_public(&language) else {
        return error(StatusCode::BAD_REQUEST, "invalid_restock_request");
    };
    if request.uri().path() != format!("/api/test/customer/restock/{language}/{sku}") {
        return error(StatusCode::BAD_REQUEST, "invalid_restock_request");
    }
    let token = customer_cookie(request.headers());
    let csrf = single_header(request.headers(), "x-csrf-token")
        .and_then(decode_hex)
        .map(CustomerCsrfToken::from_bytes);
    let (Some(token), Some(csrf)) = (token, csrf) else {
        return error(StatusCode::UNAUTHORIZED, "restock_denied");
    };
    if single_header(request.headers(), "content-type").and_then(|v| v.split(';').next())
        != Some("application/json")
    {
        return error(StatusCode::UNSUPPORTED_MEDIA_TYPE, "restock_json_required");
    }
    let bytes = match Bytes::from_request(request, &state).await {
        Ok(bytes) => bytes,
        Err(rejection) if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE => {
            return error(StatusCode::PAYLOAD_TOO_LARGE, "restock_request_too_large");
        }
        Err(_) => return error(StatusCode::BAD_REQUEST, "invalid_restock_request"),
    };
    if bytes.iter().copied().find(|b| !b.is_ascii_whitespace()) != Some(b'{') {
        return error(StatusCode::BAD_REQUEST, "invalid_restock_request");
    }
    let Ok(input) = serde_json::from_slice::<TestRestockMutation>(&bytes) else {
        return error(StatusCode::BAD_REQUEST, "invalid_restock_request");
    };
    let Some(expected) = revision(&input.expected_revision) else {
        return error(StatusCode::BAD_REQUEST, "invalid_restock_request");
    };
    let result = if input.subscribed {
        customer_restock::subscribe_with_csrf(
            &state.pool,
            &token,
            &csrf,
            state.clock.as_ref(),
            locale,
            &sku,
            expected,
        )
        .await
    } else {
        customer_restock::unsubscribe_with_csrf(
            &state.pool,
            &token,
            &csrf,
            state.clock.as_ref(),
            &sku,
            expected,
        )
        .await
    };
    match result {
        Ok(revision) => Json(TestRestockReceipt {
            revision: revision.to_string(),
        })
        .into_response(),
        Err(RestockError::Denied) => error(StatusCode::UNAUTHORIZED, "restock_denied"),
        Err(RestockError::Conflict | RestockError::Capacity) => {
            error(StatusCode::CONFLICT, "restock_conflict")
        }
        Err(RestockError::InvalidRequest) => {
            error(StatusCode::BAD_REQUEST, "invalid_restock_request")
        }
        Err(RestockError::Unavailable) => {
            error(StatusCode::SERVICE_UNAVAILABLE, "restock_unavailable")
        }
    }
}
