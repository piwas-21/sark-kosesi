//! Explicit test-only order acceptance. No payment or public checkout authority.
use super::*;
use axum::{body::Bytes, extract::FromRequest};
use commerce_domain::checkout_orders::{Address, BuyerContact, OrderIntent, SellerPolicy};
use commerce_infrastructure::checkout_orders::{self, OrderStoreError};
use serde::{Deserialize, Deserializer, Serialize, de::MapAccess};
use std::collections::HashMap;
use utoipa::ToSchema;

#[derive(Clone)]
pub struct TestOrderConfig {
    seller: SellerPolicy,
}
impl TestOrderConfig {
    pub fn from_env(
        environment: Option<&str>,
        values: &HashMap<String, String>,
    ) -> Result<Option<Self>, &'static str> {
        let flag = values.get("TEST_GUEST_ORDER_TRANSPORT").map(String::as_str);
        let raw = values.get("TEST_GUEST_ORDER_SELLER");
        match (flag, raw) {
            (None, None) => Ok(None),
            (Some("1"), Some(raw)) if environment == Some("test") && raw.len() <= 4096 => {
                let shape: serde_json::Value =
                    serde_json::from_str(raw).map_err(|_| "Invalid test seller")?;
                if !shape.is_object()
                    || !shape
                        .get("address")
                        .is_some_and(serde_json::Value::is_object)
                    || !shape
                        .get("tax_registration")
                        .is_some_and(serde_json::Value::is_object)
                {
                    return Err("Invalid test seller");
                }
                let seller: SellerPolicy =
                    serde_json::from_str(raw).map_err(|_| "Invalid test seller")?;
                seller.validate().map_err(|_| "Invalid test seller")?;
                Ok(Some(Self { seller }))
            }
            _ => Err("Test guest order requires test mode and seller"),
        }
    }
}

// Serde's derived struct visitors also accept arrays. Force nested objects without
// parsing through Value, so duplicate fields are still rejected by each struct.
fn object<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct Object<T>(std::marker::PhantomData<T>);
    impl<'de, T: Deserialize<'de>> serde::de::Visitor<'de> for Object<T> {
        type Value = T;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("an object")
        }
        fn visit_map<M: MapAccess<'de>>(self, map: M) -> Result<T, M::Error> {
            T::deserialize(serde::de::value::MapAccessDeserializer::new(map))
        }
    }
    deserializer.deserialize_map(Object(std::marker::PhantomData))
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct AddressInput {
    recipient: String,
    line1: String,
    line2: Option<String>,
    city: String,
    region: Option<String>,
    country: String,
    postcode: String,
}
impl From<AddressInput> for Address {
    fn from(v: AddressInput) -> Self {
        Self {
            recipient: v.recipient,
            line1: v.line1,
            line2: v.line2,
            city: v.city,
            region: v.region,
            country: v.country,
            postcode: v.postcode,
        }
    }
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct BuyerInput {
    name: String,
    email: String,
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct TestOrderInput {
    effect_key: String,
    quote_id: String,
    expected_basket_revision: i32,
    #[serde(deserialize_with = "object")]
    buyer: BuyerInput,
    #[serde(deserialize_with = "object")]
    shipping_address: AddressInput,
    #[serde(deserialize_with = "object")]
    billing_address: AddressInput,
    accepted_terms_version: String,
    confirmed: bool,
}
#[derive(Serialize, ToSchema)]
pub(crate) struct TestOrderReceipt {
    id: String,
    replayed: bool,
}

#[utoipa::path(post,operation_id="test_guest_order_create",path="/api/test/guest/orders",params(("Origin"=String,Header),("x-csrf-token"=String,Header)),request_body=TestOrderInput,responses((status=200,body=TestOrderReceipt),(status=400,body=ApiError),(status=401,body=ApiError),(status=403,body=ApiError),(status=404,body=ApiError),(status=409,body=ApiError),(status=410,body=ApiError),(status=413,body=ApiError),(status=415,body=ApiError),(status=429,body=ApiError),(status=503,body=ApiError),(status=504,body=ApiError)),security(("preview"=[],"guest"=[])))]
pub(crate) async fn create(
    Extension(state): Extension<GuestState>,
    Extension(config): Extension<TestOrderConfig>,
    session: Session,
    request: Request,
) -> Response {
    let csrf = match mutation(&state, &request, true) {
        Ok(value) => value,
        Err(value) => return value.into_response(),
    };
    if request.uri().query().is_some() {
        return error(StatusCode::BAD_REQUEST, "invalid_order_request");
    }
    let Some(key) = exact_guest_cookie(request.headers(), &session) else {
        return error(StatusCode::UNAUTHORIZED, "guest_session_required");
    };
    if single_header(request.headers(), "content-type").and_then(|v| v.split(';').next())
        != Some("application/json")
    {
        return error(StatusCode::UNSUPPORTED_MEDIA_TYPE, "order_json_required");
    }
    let bytes = match Bytes::from_request(request, &()).await {
        Ok(bytes) => bytes,
        Err(rejection) if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE => {
            return error(StatusCode::PAYLOAD_TOO_LARGE, "order_request_too_large");
        }
        Err(_) => return error(StatusCode::BAD_REQUEST, "invalid_order_request"),
    };
    if bytes.iter().copied().find(|b| !b.is_ascii_whitespace()) != Some(b'{') {
        return error(StatusCode::BAD_REQUEST, "invalid_order_request");
    }
    let Ok(input) = serde_json::from_slice::<TestOrderInput>(&bytes) else {
        return error(StatusCode::BAD_REQUEST, "invalid_order_request");
    };
    let intent = OrderIntent {
        quote_id: input.quote_id,
        expected_basket_revision: input.expected_basket_revision,
        buyer: BuyerContact {
            name: input.buyer.name,
            email: input.buyer.email,
        },
        shipping_address: input.shipping_address.into(),
        billing_address: input.billing_address.into(),
        accepted_terms_version: input.accepted_terms_version,
        confirmed: input.confirmed,
    };
    match checkout_orders::create_order(
        &state.store,
        &key,
        &csrf,
        &input.effect_key,
        &intent,
        &config.seller,
    )
    .await
    {
        Ok(receipt) => Json(TestOrderReceipt {
            id: receipt.snapshot.identity.id,
            replayed: receipt.replayed,
        })
        .into_response(),
        Err(value) => failed(value),
    }
}
fn failed(value: OrderStoreError) -> Response {
    let (status, code) = match value {
        OrderStoreError::InvalidRequest => (StatusCode::BAD_REQUEST, "invalid_order_request"),
        OrderStoreError::Denied => (StatusCode::UNAUTHORIZED, "guest_session_required"),
        OrderStoreError::Csrf => (StatusCode::FORBIDDEN, "guest_csrf_required"),
        OrderStoreError::Conflict
        | OrderStoreError::QuoteChanged
        | OrderStoreError::Unavailable => (StatusCode::CONFLICT, "order_unavailable"),
        OrderStoreError::QuoteNotFound | OrderStoreError::NotFound => {
            (StatusCode::NOT_FOUND, "order_source_not_found")
        }
        OrderStoreError::QuoteExpired => (StatusCode::GONE, "order_quote_expired"),
        OrderStoreError::RateLimited => (StatusCode::TOO_MANY_REQUESTS, "order_rate_limited"),
        _ => (StatusCode::SERVICE_UNAVAILABLE, "order_service_unavailable"),
    };
    let mut response = error(status, code);
    if status == StatusCode::TOO_MANY_REQUESTS {
        response
            .headers_mut()
            .insert("retry-after", axum::http::HeaderValue::from_static("60"));
    }
    response
}

#[cfg(test)]
#[path = "../../tests/support/test_guest_order_unit.rs"]
mod tests;
