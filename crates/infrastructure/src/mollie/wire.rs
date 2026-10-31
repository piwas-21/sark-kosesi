use super::*;
use commerce_domain::{
    Currency,
    payment_attempts::{PaymentMode, PaymentStatus},
};
use serde::Deserialize;
use serde_json::Value;
use url::Url;
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Payment {
    resource: String,
    id: String,
    profile_id: String,
    mode: PaymentMode,
    amount: Amount,
    status: PaymentStatus,
    metadata: Metadata,
    sequence_type: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Amount {
    currency: Currency,
    value: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Metadata {
    schema_version: u16,
    order_id: String,
    attempt_id: String,
}
#[derive(Deserialize)]
struct Profile {
    resource: String,
    id: String,
}
pub(super) fn profile(body: &[u8], expected: &str) -> Result<(), MollieError> {
    json(body)?;
    let profile: Profile =
        serde_json::from_slice(body).map_err(|_| MollieError::InvalidResponse)?;
    if profile.resource != "profile" || profile.id != expected {
        return Err(MollieError::ProfileMismatch);
    }
    Ok(())
}
pub(super) fn payment(
    body: &[u8],
    attempt: &AttemptSnapshot,
    expected_id: Option<&str>,
) -> Result<(ProviderPayment, Option<String>), MollieError> {
    let value = json(body)?;
    if !["amount", "metadata"]
        .iter()
        .all(|key| value.get(*key).is_some_and(Value::is_object))
    {
        return Err(MollieError::InvalidResponse);
    }
    let checkout = checkout(&value);
    let wire: Payment = serde_json::from_slice(body).map_err(|_| MollieError::InvalidResponse)?;
    if wire.sequence_type != "oneoff" || expected_id.is_some_and(|id| id != wire.id) {
        return Err(MollieError::PaymentMismatch);
    }
    let payment = ProviderPayment {
        resource: wire.resource,
        id: wire.id,
        profile_id: wire.profile_id,
        mode: wire.mode,
        currency: wire.amount.currency,
        amount_minor: minor(&wire.amount.value)?,
        metadata_schema_version: wire.metadata.schema_version,
        order_id: wire.metadata.order_id,
        attempt_id: wire.metadata.attempt_id,
        status: wire.status,
    };
    match_provider(attempt, &payment).map_err(|_| MollieError::PaymentMismatch)?;
    Ok((payment, checkout))
}
fn json(body: &[u8]) -> Result<Value, MollieError> {
    if body.len() > http::MAX_RESPONSE {
        return Err(MollieError::ResponseTooLarge);
    }
    let v: Value = serde_json::from_slice(body).map_err(|_| MollieError::InvalidResponse)?;
    if !v.is_object() {
        return Err(MollieError::InvalidResponse);
    }
    Ok(v)
}
fn minor(value: &str) -> Result<i64, MollieError> {
    if value.len() > 32 {
        return Err(MollieError::InvalidResponse);
    }
    let (whole, fraction) = value.split_once('.').ok_or(MollieError::InvalidResponse)?;
    if whole.is_empty()
        || (whole.len() > 1 && whole.starts_with('0'))
        || fraction.len() != 2
        || !whole.bytes().all(|v| v.is_ascii_digit())
        || !fraction.bytes().all(|v| v.is_ascii_digit())
    {
        return Err(MollieError::InvalidResponse);
    }
    let whole = whole
        .parse::<i64>()
        .map_err(|_| MollieError::InvalidResponse)?;
    let fraction = fraction
        .parse::<i64>()
        .map_err(|_| MollieError::InvalidResponse)?;
    whole
        .checked_mul(100)
        .and_then(|v| v.checked_add(fraction))
        .filter(|v| *v <= commerce_domain::quotes::MAX_MINOR_UNITS)
        .ok_or(MollieError::InvalidResponse)
}
/// Optional presentation links cannot discard genuine matched payment evidence.
fn checkout(value: &Value) -> Option<String> {
    let links = value.get("_links")?.as_object()?;
    let checkout = links.get("checkout")?.as_object()?;
    let href = checkout.get("href")?.as_str()?;
    if href.len() > 2048 {
        return None;
    }
    let url = Url::parse(href).ok()?;
    if url.scheme() != "https"
        || url.as_str() != href
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.port_or_known_default() != Some(443)
        || !matches!(
            url.host_str(),
            Some("mollie.com" | "www.mollie.com" | "checkout.mollie.com")
        )
    {
        return None;
    }
    Some(href.into())
}
