//! Explicit test-only customer login. This is not a public authentication surface.
use crate::{
    ApiError, AppState, error,
    guest::{GuestOrigin, origin::single_header},
    verification::decode_hex,
};
use axum::{
    Extension,
    body::Bytes,
    extract::{FromRequest, Request, State},
    http::{HeaderValue, StatusCode, header::SET_COOKIE},
    response::{IntoResponse, Response},
};
use commerce_infrastructure::{
    customer_credential_admission::{AdmissionKey, AdmissionLimits},
    customer_sessions::{self, IssuedSession, SessionError},
};
use serde::Deserialize;
use std::{collections::HashMap, sync::Arc};
use tower_sessions::cookie::{Cookie, SameSite, time::OffsetDateTime};
use utoipa::ToSchema;
use zeroize::Zeroizing;

const SESSION_LIFETIME: i64 = 43_200;
const COOKIE: &str = "__Host-piwas-customer";

#[derive(Clone)]
pub struct LoginConfig {
    origin: GuestOrigin,
    key: Arc<AdmissionKey>,
    limits: AdmissionLimits,
}
impl LoginConfig {
    pub fn from_env(
        environment: Option<&str>,
        values: &HashMap<String, String>,
    ) -> Result<Option<Self>, &'static str> {
        match (
            values.get("TEST_LOGIN_ORIGIN"),
            values.get("TEST_LOGIN_ADMISSION_KEY"),
        ) {
            (None, None) => Ok(None),
            (Some(origin), Some(key)) if environment == Some("test") => {
                let origin = GuestOrigin::parse(origin).map_err(|_| "Invalid test login origin")?;
                let bytes = decode_hex(key).ok_or("Invalid test login admission key")?;
                let key = AdmissionKey::new(1, bytes).ok_or("Invalid test login admission key")?;
                let limits =
                    AdmissionLimits::new(120, 5).ok_or("Invalid test login admission limits")?;
                Ok(Some(Self {
                    origin,
                    key: Arc::new(key),
                    limits,
                }))
            }
            _ => Err("Test login requires test mode, origin and admission key"),
        }
    }
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct LoginRequest {
    /// Exact mailbox under the private identity normalization policy.
    #[schema(max_length = 254)]
    email: String,
    /// Password input; the private credential policy also bounds UTF-8 bytes.
    #[schema(max_length = 256)]
    password: String,
}

#[utoipa::path(post, operation_id="test_customer_login", path="/api/test/customer/login", params(("Origin"=String,Header)), request_body=LoginRequest, responses((status=204,description="Session issued in an HttpOnly Secure SameSite=Lax host-only cookie"),(status=400,body=ApiError),(status=401,body=ApiError),(status=403,body=ApiError),(status=413,body=ApiError),(status=415,body=ApiError),(status=503,body=ApiError),(status=504,body=ApiError)), security(("preview"=[])))]
pub(crate) async fn login(
    State(state): State<AppState>,
    Extension(config): Extension<LoginConfig>,
    request: Request,
) -> Response {
    if !config.origin.accepts(request.headers()) {
        return error(StatusCode::FORBIDDEN, "login_origin_required");
    }
    if request.uri().query().is_some() {
        return error(StatusCode::BAD_REQUEST, "invalid_login_request");
    }
    if single_header(request.headers(), "content-type").and_then(|v| v.split(';').next())
        != Some("application/json")
    {
        return error(StatusCode::UNSUPPORTED_MEDIA_TYPE, "login_json_required");
    }
    let bytes = match Bytes::from_request(request, &state).await {
        Ok(bytes) => bytes,
        Err(rejection) if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE => {
            return error(StatusCode::PAYLOAD_TOO_LARGE, "login_request_too_large");
        }
        Err(_) => return error(StatusCode::BAD_REQUEST, "invalid_login_request"),
    };
    let bytes = Zeroizing::new(Vec::from(bytes));
    if bytes.iter().copied().find(|b| !b.is_ascii_whitespace()) != Some(b'{') {
        return error(StatusCode::BAD_REQUEST, "invalid_login_request");
    }
    let input: LoginRequest = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(_) => return error(StatusCode::BAD_REQUEST, "invalid_login_request"),
    };
    drop(bytes);
    let password = Zeroizing::new(input.password);
    match customer_sessions::login(
        &state.pool,
        &input.email,
        &password,
        state.clock.as_ref(),
        &config.key,
        config.limits,
    )
    .await
    {
        Ok(session) => match session_cookie(&state, session) {
            Some(cookie) => {
                let mut response = StatusCode::NO_CONTENT.into_response();
                response.headers_mut().insert(SET_COOKIE, cookie);
                response
            }
            None => error(StatusCode::SERVICE_UNAVAILABLE, "login_unavailable"),
        },
        Err(SessionError::Denied) => error(StatusCode::UNAUTHORIZED, "login_denied"),
        Err(SessionError::Unavailable) => {
            error(StatusCode::SERVICE_UNAVAILABLE, "login_unavailable")
        }
    }
}

fn session_cookie(state: &AppState, session: IssuedSession) -> Option<HeaderValue> {
    let now = i64::try_from(state.clock.unix_seconds().ok()?).ok()?;
    let expires = session.identity.expires_at;
    let remaining = expires.checked_sub(now)?;
    if !(1..=SESSION_LIFETIME).contains(&remaining) {
        return None;
    }
    let expiry = OffsetDateTime::from_unix_timestamp(expires).ok()?;
    let mut value = String::with_capacity(64);
    for byte in session.token.expose_bytes() {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        value.push(char::from(HEX[usize::from(byte >> 4)]));
        value.push(char::from(HEX[usize::from(byte & 15)]));
    }
    let cookie = Cookie::build((COOKIE, value))
        .secure(true)
        .http_only(true)
        .same_site(SameSite::Lax)
        .path("/")
        .expires(expiry)
        .build();
    let mut header = HeaderValue::from_str(&cookie.to_string()).ok()?;
    header.set_sensitive(true);
    Some(header)
}

#[cfg(test)]
mod tests {
    use super::*;
    use commerce_application::request_context::{Clock, ContextError, Identifiers};
    use commerce_infrastructure::customer_sessions::{CustomerIdentity, CustomerSessionToken};
    use tokio::sync::Semaphore;

    struct FixedClock(Option<u64>);
    impl Clock for FixedClock {
        fn unix_seconds(&self) -> Result<u64, ContextError> {
            self.0.ok_or(ContextError::ClockUnavailable)
        }
    }
    struct FixedIds;
    impl Identifiers for FixedIds {
        fn next(&self) -> Result<[u8; 16], ContextError> {
            Ok([1; 16])
        }
    }
    fn state(now: Option<u64>) -> AppState {
        AppState {
            pool: sqlx::PgPool::connect_lazy("postgresql://localhost/unused").expect("lazy pool"),
            preview_token: Arc::from("fixture"),
            capacity: Arc::new(Semaphore::new(1)),
            clock: Arc::new(FixedClock(now)),
            identifiers: Arc::new(FixedIds),
        }
    }
    fn session(expires_at: i64) -> IssuedSession {
        IssuedSession {
            token: CustomerSessionToken::from_bytes([1; 32]),
            identity: CustomerIdentity {
                account_id: 1,
                credential_revision: 1,
                expires_at,
            },
        }
    }
    #[tokio::test]
    async fn cookie_fails_closed_on_missing_rewound_expired_or_unrepresentable_clock() {
        assert!(session_cookie(&state(Some(1000)), session(44_200)).is_some());
        for (now, expires) in [
            (None, 44_200),
            (Some(u64::MAX), 44_200),
            (Some(0), 44_200),
            (Some(44_200), 44_200),
            (Some(9_007_199_254_740_990), 9_007_199_254_740_991),
        ] {
            assert!(session_cookie(&state(now), session(expires)).is_none());
        }
    }
}
