//! Explicitly configured, loopback-only test consumer; no delivery or session authority.
use crate::{
    ApiError, AppState, error,
    guest::{GuestOrigin, origin::single_header},
};
use axum::{
    Extension,
    body::Bytes,
    extract::{FromRequest, Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use commerce_infrastructure::{
    customer_credential_admission::{AdmissionKey, AdmissionLimits},
    email_verification::{VerificationError, VerificationToken, verified_customer_accounts},
};
use serde::Deserialize;
use std::{collections::HashMap, sync::Arc};
use utoipa::ToSchema;

#[derive(Clone)]
pub struct VerificationConfig {
    origin: GuestOrigin,
    key: Arc<AdmissionKey>,
    limits: AdmissionLimits,
}
impl VerificationConfig {
    pub fn from_env(
        environment: Option<&str>,
        values: &HashMap<String, String>,
    ) -> Result<Option<Self>, &'static str> {
        let origin = values.get("TEST_VERIFICATION_ORIGIN");
        let key = values.get("TEST_VERIFICATION_ADMISSION_KEY");
        match (origin, key) {
            (None, None) => Ok(None),
            (Some(origin), Some(key)) if environment == Some("test") => {
                let origin =
                    GuestOrigin::parse(origin).map_err(|_| "Invalid test verification origin")?;
                let bytes = decode_hex(key).ok_or("Invalid test verification admission key")?;
                let key =
                    AdmissionKey::new(1, bytes).ok_or("Invalid test verification admission key")?;
                let limits = AdmissionLimits::new(120, 5)
                    .ok_or("Invalid test verification admission limits")?;
                Ok(Some(Self {
                    origin,
                    key: Arc::new(key),
                    limits,
                }))
            }
            _ => Err("Test verification requires test mode, origin and admission key"),
        }
    }
}
pub(crate) fn decode_hex(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return None;
    }
    let mut result = [0; 32];
    for (index, pair) in value.as_bytes().as_chunks::<2>().0.iter().enumerate() {
        let nibble = |b: u8| if b <= b'9' { b - b'0' } else { b - b'a' + 10 };
        result[index] = nibble(pair[0]) * 16 + nibble(pair[1]);
    }
    Some(result)
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RedeemRequest {
    /// Mailbox bound to the proof; normalized by the account primitive.
    #[schema(max_length = 254)]
    email: String,
    /// Exactly 32 bearer bytes encoded as 64 lowercase hexadecimal characters.
    #[schema(min_length = 64, max_length = 64, pattern = "^[0-9a-f]{64}$")]
    token: String,
}
#[utoipa::path(post, operation_id="redeem_test_verification", path="/api/test/verification/redeem", params(("Origin"=String,Header)), request_body=RedeemRequest, responses((status=204,description="Account established; no session issued"),(status=400,body=ApiError),(status=401,body=ApiError),(status=403,body=ApiError),(status=413,body=ApiError),(status=415,body=ApiError),(status=503,body=ApiError),(status=504,body=ApiError)), security(("preview"=[])))]
pub(crate) async fn redeem(
    State(state): State<AppState>,
    Extension(config): Extension<VerificationConfig>,
    request: Request,
) -> Response {
    if !config.origin.accepts(request.headers()) {
        return error(StatusCode::FORBIDDEN, "verification_origin_required");
    }
    if request.uri().query().is_some() {
        return error(StatusCode::BAD_REQUEST, "invalid_verification_request");
    }
    if single_header(request.headers(), "content-type").and_then(|v| v.split(';').next())
        != Some("application/json")
    {
        return error(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "verification_json_required",
        );
    }
    let bytes = match Bytes::from_request(request, &state).await {
        Ok(bytes) => bytes,
        Err(rejection) if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE => {
            return error(
                StatusCode::PAYLOAD_TOO_LARGE,
                "verification_request_too_large",
            );
        }
        Err(_) => return error(StatusCode::BAD_REQUEST, "invalid_verification_request"),
    };
    if bytes.iter().copied().find(|b| !b.is_ascii_whitespace()) != Some(b'{') {
        return error(StatusCode::BAD_REQUEST, "invalid_verification_request");
    }
    let input: RedeemRequest = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(_) => return error(StatusCode::BAD_REQUEST, "invalid_verification_request"),
    };
    let Some(token) = decode_hex(&input.token) else {
        return error(StatusCode::BAD_REQUEST, "invalid_verification_request");
    };
    match verified_customer_accounts::establish(
        &state.pool,
        &input.email,
        &VerificationToken::from_bytes(token),
        state.clock.as_ref(),
        &config.key,
        config.limits,
    )
    .await
    {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(VerificationError::Denied) => error(StatusCode::UNAUTHORIZED, "verification_denied"),
        Err(VerificationError::Unavailable) => {
            error(StatusCode::SERVICE_UNAVAILABLE, "verification_unavailable")
        }
    }
}
