//! Explicit test-only favourite changes; no public route.
use crate::{
    ApiError, AppState, error, guest::origin::single_header, test_login::LoginConfig,
    test_revision::revision, test_session::customer_cookie, verification::decode_hex,
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
    customer_favourites::{self, FavouriteError},
    customer_sessions::CustomerCsrfToken,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct TestFavouriteMutation {
    favourite: bool,
    /// Current bucket revision as a canonical decimal integer.
    #[schema(pattern = "^(0|[1-9][0-9]*)$", max_length = 19)]
    expected_revision: String,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct TestFavouriteReceipt {
    #[schema(pattern = "^(0|[1-9][0-9]*)$", max_length = 19)]
    revision: String,
}

#[utoipa::path(put, operation_id="test_customer_favourites_set", path="/api/test/customer/favourites/{locale}/{sku}", params(("locale"=String,Path),("sku"=String,Path),("Origin"=String,Header),("x-csrf-token"=String,Header)), request_body=TestFavouriteMutation, responses((status=200,body=TestFavouriteReceipt,description="Resulting private bucket revision"),(status=400,body=ApiError),(status=401,body=ApiError),(status=403,body=ApiError),(status=409,body=ApiError),(status=413,body=ApiError),(status=415,body=ApiError),(status=503,body=ApiError),(status=504,body=ApiError)), security(("preview"=[],"customer_test"=[])))]
pub(crate) async fn set(
    State(state): State<AppState>,
    Extension(config): Extension<LoginConfig>,
    Path((language, sku)): Path<(String, String)>,
    request: Request,
) -> Response {
    if !config.origin.accepts(request.headers()) {
        return error(StatusCode::FORBIDDEN, "favourites_origin_required");
    }
    if request.uri().query().is_some() || !merchant_code(&sku) {
        return error(StatusCode::BAD_REQUEST, "invalid_favourites_request");
    }
    let Some(locale) = Locale::from_public(&language) else {
        return error(StatusCode::BAD_REQUEST, "invalid_favourites_request");
    };
    if request.uri().path() != format!("/api/test/customer/favourites/{language}/{sku}") {
        return error(StatusCode::BAD_REQUEST, "invalid_favourites_request");
    }
    let token = customer_cookie(request.headers());
    let csrf = single_header(request.headers(), "x-csrf-token")
        .and_then(decode_hex)
        .map(CustomerCsrfToken::from_bytes);
    let (Some(token), Some(csrf)) = (token, csrf) else {
        return error(StatusCode::UNAUTHORIZED, "favourites_denied");
    };
    if single_header(request.headers(), "content-type").and_then(|v| v.split(';').next())
        != Some("application/json")
    {
        return error(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "favourites_json_required",
        );
    }
    let bytes = match Bytes::from_request(request, &state).await {
        Ok(bytes) => bytes,
        Err(rejection) if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE => {
            return error(
                StatusCode::PAYLOAD_TOO_LARGE,
                "favourites_request_too_large",
            );
        }
        Err(_) => return error(StatusCode::BAD_REQUEST, "invalid_favourites_request"),
    };
    if bytes.iter().copied().find(|b| !b.is_ascii_whitespace()) != Some(b'{') {
        return error(StatusCode::BAD_REQUEST, "invalid_favourites_request");
    }
    let Ok(input) = serde_json::from_slice::<TestFavouriteMutation>(&bytes) else {
        return error(StatusCode::BAD_REQUEST, "invalid_favourites_request");
    };
    let Some(expected) = revision(&input.expected_revision) else {
        return error(StatusCode::BAD_REQUEST, "invalid_favourites_request");
    };
    let result = customer_favourites::set_with_csrf(
        &state.pool,
        &token,
        &csrf,
        state.clock.as_ref(),
        locale,
        &sku,
        input.favourite,
        expected,
    )
    .await;
    match result {
        Ok(revision) => Json(TestFavouriteReceipt {
            revision: revision.to_string(),
        })
        .into_response(),
        Err(FavouriteError::Denied) => error(StatusCode::UNAUTHORIZED, "favourites_denied"),
        Err(FavouriteError::Conflict | FavouriteError::Capacity) => {
            error(StatusCode::CONFLICT, "favourites_conflict")
        }
        Err(FavouriteError::InvalidRequest) => {
            error(StatusCode::BAD_REQUEST, "invalid_favourites_request")
        }
        Err(FavouriteError::Unavailable) => {
            error(StatusCode::SERVICE_UNAVAILABLE, "favourites_unavailable")
        }
    }
}
