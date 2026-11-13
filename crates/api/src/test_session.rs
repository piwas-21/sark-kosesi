//! Explicitly enabled test-only bound-session transport; no public route.
use crate::{
    ApiError, AppState, error,
    guest::origin::single_header,
    test_login::{self, LoginConfig},
    test_login_input,
    verification::decode_hex,
};
use axum::{
    Extension, Json,
    body::to_bytes,
    extract::{Request, State},
    http::{
        HeaderMap, HeaderValue, StatusCode,
        header::{COOKIE, SET_COOKIE},
    },
    response::{IntoResponse, Response},
};
use commerce_infrastructure::customer_sessions::{
    self, CustomerCsrfToken, CustomerSessionToken, SessionError,
};
use serde::Serialize;
use tower_sessions::cookie::{Cookie, SameSite, time::Duration};
use utoipa::ToSchema;
use zeroize::Zeroizing;

const CUSTOMER_COOKIE: &str = test_login::COOKIE;

#[derive(Serialize, ToSchema)]
pub(crate) struct TestSessionProof {
    /// One independent synchronizer encoded as 64 lowercase hexadecimal characters.
    #[schema(min_length = 64, max_length = 64, pattern = "^[0-9a-f]{64}$")]
    csrf_token: String,
}

#[utoipa::path(post, operation_id="test_customer_login_with_csrf", path="/api/test/customer/login-with-csrf", params(("Origin"=String,Header)), request_body=crate::test_login_input::LoginRequest, responses((status=200,body=TestSessionProof,description="Bound synchronizer and HttpOnly secure session cookie"),(status=400,body=ApiError),(status=401,body=ApiError),(status=403,body=ApiError),(status=413,body=ApiError),(status=415,body=ApiError),(status=503,body=ApiError),(status=504,body=ApiError)), security(("preview"=[])))]
pub(crate) async fn login(
    State(state): State<AppState>,
    Extension(config): Extension<LoginConfig>,
    request: Request,
) -> Response {
    let input = match test_login_input::parse_request(&state, &config, request).await {
        Ok(input) => input,
        Err(response) => return *response,
    };
    let password = Zeroizing::new(input.password);
    match customer_sessions::login_with_csrf(
        &state.pool,
        &input.email,
        &password,
        state.clock.as_ref(),
        &config.key,
        config.limits,
    )
    .await
    {
        Ok(issued) => {
            let Some(cookie) = test_login::session_cookie(&state, issued.session) else {
                return error(StatusCode::SERVICE_UNAVAILABLE, "login_unavailable");
            };
            let proof = TestSessionProof {
                csrf_token: encode(issued.csrf.expose_bytes()),
            };
            let mut response = Json(proof).into_response();
            response.headers_mut().insert(SET_COOKIE, cookie);
            response
        }
        Err(SessionError::Denied) => error(StatusCode::UNAUTHORIZED, "login_denied"),
        Err(SessionError::Unavailable) => {
            error(StatusCode::SERVICE_UNAVAILABLE, "login_unavailable")
        }
    }
}

#[utoipa::path(delete, operation_id="test_customer_logout", path="/api/test/customer/session", params(("Origin"=String,Header),("x-csrf-token"=String,Header)), responses((status=204,description="Bound session revoked and cookie cleared"),(status=400,body=ApiError),(status=401,body=ApiError),(status=403,body=ApiError),(status=503,body=ApiError),(status=504,body=ApiError)), security(("preview"=[],"customer_test"=[])))]
pub(crate) async fn logout(
    State(state): State<AppState>,
    Extension(config): Extension<LoginConfig>,
    request: Request,
) -> Response {
    if !config.origin.accepts(request.headers()) {
        return error(StatusCode::FORBIDDEN, "session_origin_required");
    }
    if request.uri().query().is_some() {
        return error(StatusCode::BAD_REQUEST, "invalid_session_request");
    }
    let token = customer_cookie(request.headers());
    let proof = single_header(request.headers(), "x-csrf-token")
        .and_then(decode_hex)
        .map(CustomerCsrfToken::from_bytes);
    let (Some(token), Some(proof)) = (token, proof) else {
        return error(StatusCode::UNAUTHORIZED, "session_denied");
    };
    if to_bytes(request.into_body(), 0).await.is_err() {
        return error(StatusCode::BAD_REQUEST, "invalid_session_request");
    }
    let Some(cookie) = removal_cookie() else {
        return error(StatusCode::SERVICE_UNAVAILABLE, "session_unavailable");
    };
    match customer_sessions::logout_with_csrf(&state.pool, &token, &proof, state.clock.as_ref())
        .await
    {
        Ok(()) => {
            let mut response = StatusCode::NO_CONTENT.into_response();
            response.headers_mut().insert(SET_COOKIE, cookie);
            response
        }
        Err(SessionError::Denied) => error(StatusCode::UNAUTHORIZED, "session_denied"),
        Err(SessionError::Unavailable) => {
            error(StatusCode::SERVICE_UNAVAILABLE, "session_unavailable")
        }
    }
}

fn customer_cookie(headers: &HeaderMap) -> Option<CustomerSessionToken> {
    let raw = single_header(headers, COOKIE.as_str())?;
    if raw.len() > 4096 {
        return None;
    }
    let mut found = None;
    for part in raw.split(';') {
        let Some((name, value)) = part.trim().split_once('=') else {
            continue;
        };
        if name == CUSTOMER_COOKIE {
            if found.is_some() {
                return None;
            }
            found = Some(CustomerSessionToken::from_bytes(decode_hex(value)?));
        }
    }
    found
}
fn removal_cookie() -> Option<HeaderValue> {
    let cookie = Cookie::build((CUSTOMER_COOKIE, ""))
        .secure(true)
        .http_only(true)
        .same_site(SameSite::Lax)
        .path("/")
        .max_age(Duration::ZERO)
        .build();
    let mut header = HeaderValue::from_str(&cookie.to_string()).ok()?;
    header.set_sensitive(true);
    Some(header)
}
fn encode(bytes: &[u8; 32]) -> String {
    let mut value = String::with_capacity(64);
    for byte in bytes {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        value.push(char::from(HEX[usize::from(byte >> 4)]));
        value.push(char::from(HEX[usize::from(byte & 15)]));
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderMap, HeaderValue};
    #[test]
    fn cookie_lookup_accepts_only_one_exact_bounded_canonical_customer_pair() {
        let value = "ab".repeat(32);
        let mut headers = HeaderMap::new();
        headers.insert(
            COOKIE,
            format!("other=a; {CUSTOMER_COOKIE}={value}")
                .parse()
                .expect("header"),
        );
        assert!(customer_cookie(&headers).is_some());
        for raw in [
            format!("{CUSTOMER_COOKIE}={value}; {CUSTOMER_COOKIE}={value}"),
            format!("{CUSTOMER_COOKIE}={}", "AB".repeat(32)),
            format!("{CUSTOMER_COOKIE}=\"{value}\""),
            format!("{CUSTOMER_COOKIE}={}", "a".repeat(63)),
            format!("other={}; {CUSTOMER_COOKIE}={value}", "x".repeat(4096)),
        ] {
            headers.insert(
                COOKIE,
                HeaderValue::from_str(&raw).expect("synthetic header"),
            );
            assert!(customer_cookie(&headers).is_none());
        }
        headers.insert(
            COOKIE,
            format!("{CUSTOMER_COOKIE}={value}")
                .parse()
                .expect("header"),
        );
        headers.append(
            COOKIE,
            format!("{CUSTOMER_COOKIE}={value}")
                .parse()
                .expect("header"),
        );
        assert!(customer_cookie(&headers).is_none());
    }
}
