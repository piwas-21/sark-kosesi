//! Opt-in test-only redemption; no proof delivery or public account route.
use crate::{
    ApiError, AppState, error, guest::origin::single_header, test_login::LoginConfig,
    test_session::customer_cookie, verification::decode_hex,
};
use axum::{
    Extension,
    body::Bytes,
    extract::{FromRequest, Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use commerce_infrastructure::{
    customer_sessions::CustomerCsrfToken,
    guest_order_links::{self, GuestOrderLinkToken, LinkError},
};
use serde::Deserialize;
use utoipa::ToSchema;

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct TestGuestLinkRedemption {
    #[schema(min_length = 32, max_length = 32, pattern = "^[0-9a-f]{32}$")]
    order_id: String,
    #[schema(min_length = 64, max_length = 64, pattern = "^[0-9a-f]{64}$")]
    proof: String,
}

#[utoipa::path(post, operation_id="test_customer_guest_order_link_redeem", path="/api/test/customer/guest-order-link/redeem", params(("Origin"=String,Header),("x-csrf-token"=String,Header)), request_body=TestGuestLinkRedemption, responses((status=204,description="Guest order linked to current account"),(status=400,body=ApiError),(status=401,body=ApiError),(status=403,body=ApiError),(status=413,body=ApiError),(status=415,body=ApiError),(status=503,body=ApiError),(status=504,body=ApiError)), security(("preview"=[],"customer_test"=[])))]
pub(crate) async fn redeem(
    State(state): State<AppState>,
    Extension(config): Extension<LoginConfig>,
    request: Request,
) -> Response {
    if !config.origin.accepts(request.headers()) {
        return error(StatusCode::FORBIDDEN, "guest_link_origin_required");
    }
    if request.uri().query().is_some()
        || request.uri().path() != "/api/test/customer/guest-order-link/redeem"
    {
        return error(StatusCode::BAD_REQUEST, "invalid_guest_link_request");
    }
    let token = customer_cookie(request.headers());
    let csrf = single_header(request.headers(), "x-csrf-token")
        .and_then(decode_hex)
        .map(CustomerCsrfToken::from_bytes);
    let (Some(token), Some(csrf)) = (token, csrf) else {
        return error(StatusCode::UNAUTHORIZED, "guest_link_denied");
    };
    if single_header(request.headers(), "content-type").and_then(|v| v.split(';').next())
        != Some("application/json")
    {
        return error(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "guest_link_json_required",
        );
    }
    let bytes = match Bytes::from_request(request, &state).await {
        Ok(bytes) => bytes,
        Err(rejection) if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE => {
            return error(
                StatusCode::PAYLOAD_TOO_LARGE,
                "guest_link_request_too_large",
            );
        }
        Err(_) => return error(StatusCode::BAD_REQUEST, "invalid_guest_link_request"),
    };
    if bytes.iter().copied().find(|b| !b.is_ascii_whitespace()) != Some(b'{') {
        return error(StatusCode::BAD_REQUEST, "invalid_guest_link_request");
    }
    let Ok(input) = serde_json::from_slice::<TestGuestLinkRedemption>(&bytes) else {
        return error(StatusCode::BAD_REQUEST, "invalid_guest_link_request");
    };
    if input.order_id.len() != 32
        || !input
            .order_id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return error(StatusCode::BAD_REQUEST, "invalid_guest_link_request");
    }
    let Some(proof) = decode_hex(&input.proof) else {
        return error(StatusCode::BAD_REQUEST, "invalid_guest_link_request");
    };
    match guest_order_links::link_with_csrf(
        &state.pool,
        &token,
        &csrf,
        &input.order_id,
        &GuestOrderLinkToken::from_bytes(proof),
        state.clock.as_ref(),
    )
    .await
    {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(LinkError::Denied) => error(StatusCode::UNAUTHORIZED, "guest_link_denied"),
        Err(LinkError::Unavailable) => {
            error(StatusCode::SERVICE_UNAVAILABLE, "guest_link_unavailable")
        }
    }
}
